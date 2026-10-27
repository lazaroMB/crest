//! `crest-calc`: calc sheets and unit conversions on numbat.
//!
//! A sheet is a list of lines `name = expression` (or bare expressions)
//! evaluated top to bottom in one numbat context with its embedded prelude
//! (SI, imperial, US customary and the other unit modules; no currency
//! exchange rates, no file system modules, no plotting). Before a line runs
//! the engine checks its names against the sheet: a name used before the
//! line that defines it, a circular definition or a dependency on a line
//! that failed gets a clear error on that line, and the other lines still
//! evaluate. Each line records its value, numbat's display, the unit, the
//! dimension, TeX of the line and how ambiguous unit names were read.
//!
//! [`convert`] (in `convert.rs`) converts one value and reports the
//! factor, whether it is exact, the dimension and the chain through SI.

mod convert;
pub mod latex;
mod notes;
pub mod syntax;

use std::sync::{Mutex, PoisonError};

use application::ports::compute::{CalcRequest, ComputeError};
use domain::computed::{
    Assumption, AssumptionStatus, CalcLine, CalcSheetBody, Computed, ComputedBody, ComputedStatus,
    ComputedValue, InputRef, WarningSeverity,
};
use numbat::markup::plain_text_format;
use numbat::module_importer::BuiltinModuleImporter;
use numbat::resolver::CodeSource;
use numbat::value::Value;
use numbat::{
    Context, FormatOptions, InterpreterResult, InterpreterSettings, NameResolutionError,
    NumbatError, TypeCheckError,
};

pub use convert::convert;

use crate::envelope;

/// Statements a calc line may not hold: the sheet is expressions only.
const STATEMENT_KEYWORDS: &[&str] = &["fn", "unit", "dimension", "struct", "use", "let"];

/// A numbat context with the prelude loaded, built once and cloned per
/// sheet (loading the prelude takes tens of milliseconds, a clone well
/// under one).
fn context() -> Result<Context, ComputeError> {
    static PRELUDE: Mutex<Option<Context>> = Mutex::new(None);
    let mut cached = PRELUDE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(ctx) = cached.as_ref() {
        return Ok(ctx.clone());
    }
    let mut ctx = Context::new(BuiltinModuleImporter::default());
    ctx.load_currency_module_on_demand(false);
    let _loaded = ctx
        .interpret_with_settings(&mut quiet(), "use prelude", CodeSource::Internal)
        .map_err(|e| ComputeError::Invalid(format!("numbat prelude did not load: {e}")))?;
    *cached = Some(ctx.clone());
    Ok(ctx)
}

/// Interpreter settings that drop `print` output.
fn quiet() -> InterpreterSettings {
    InterpreterSettings {
        print_fn: Box::new(|_| {}),
    }
}

/// How results are printed.
fn format_options(significant_digits: u32) -> FormatOptions {
    FormatOptions {
        digit_separator: String::new(),
        significant_digits: usize::try_from(significant_digits.clamp(1, 15)).unwrap_or(6),
        ..FormatOptions::default()
    }
}

/// One evaluated expression.
#[derive(Debug, Clone, PartialEq)]
struct Evaluated {
    /// Magnitude, for quantities.
    value: Option<f64>,
    /// The magnitude as numbat prints it.
    number: Option<String>,
    /// Unit as numbat prints it; empty for plain numbers.
    unit: String,
    /// numbat's rendering of the whole value.
    display: String,
    /// Dimension name(s), e.g. `Energy or Torque`.
    dimension: Option<String>,
}

/// Evaluates `code` in `ctx`.
fn evaluate(ctx: &mut Context, code: &str, options: &FormatOptions) -> Result<Evaluated, String> {
    let (statements, result) = ctx
        .interpret_with_settings(&mut quiet(), code, CodeSource::Text)
        .map_err(|e| error_text(&e))?;
    let typed = plain_text_format(
        &result.to_markup(
            statements.last(),
            ctx.dimension_registry(),
            true,
            false,
            options,
        ),
        false,
    )
    .to_string();
    let dimension = typed
        .trim_end()
        .rsplit_once("    [")
        .and_then(|(_, t)| t.strip_suffix(']'))
        .map(str::to_owned);
    match result {
        InterpreterResult::Value(Value::Quantity(q)) => {
            let value = q.unsafe_value().to_f64();
            let raw = q.unsafe_value().pretty_print_with(options).to_string();
            let number = tidy(&raw);
            let display = plain_text_format(&q.pretty_print_with(options), false)
                .trim()
                .to_owned();
            let display = match display.strip_prefix(raw.as_str()) {
                Some(rest) => format!("{number}{rest}"),
                None => display,
            };
            Ok(Evaluated {
                value: Some(value),
                number: Some(number),
                unit: q.unit().to_string(),
                display,
                dimension,
            })
        }
        InterpreterResult::Value(other) => Ok(Evaluated {
            value: None,
            number: None,
            unit: String::new(),
            display: plain_text_format(&other.pretty_print(), false)
                .trim()
                .to_owned(),
            dimension,
        }),
        InterpreterResult::Continue => Err("the line has no value".into()),
    }
}

/// A printed number without a meaningless `.0` (numbat prints a float
/// that rounds to an integer as `150.0`).
fn tidy(number: &str) -> String {
    let (mantissa, exponent) = match number.split_once('e') {
        Some((m, e)) => (m, Some(e)),
        None => (number, None),
    };
    let mantissa = match mantissa.strip_suffix(".0") {
        Some(m) => m,
        None => mantissa,
    };
    match exponent {
        Some(e) => format!("{mantissa}e{e}"),
        None => mantissa.to_owned(),
    }
}

/// A one-line message for a numbat error.
fn error_text(error: &NumbatError) -> String {
    let text = match error {
        NumbatError::TypeCheckError(TypeCheckError::IncompatibleDimensions(e)) => {
            let side = |ty: String, names: &[numbat::compact_str::CompactString]| {
                if names.is_empty() {
                    ty
                } else {
                    format!("{ty} ({})", names.join(" or "))
                }
            };
            format!(
                "incompatible dimensions in {}: {} is {}, {} is {}",
                e.operation,
                e.expected_name.trim(),
                side(e.expected_type.to_string(), &e.expected_dimensions),
                e.actual_name.trim(),
                side(e.actual_type.to_string(), &e.actual_dimensions)
            )
        }
        NumbatError::TypeCheckError(TypeCheckError::UnknownIdentifier(_, name, suggestion)) => {
            match suggestion {
                Some(s) => format!("unknown unit or name `{name}` (did you mean `{s}`?)"),
                None => format!("unknown unit or name `{name}`"),
            }
        }
        NumbatError::NameResolutionError(NameResolutionError::IdentifierClash {
            conflicting_identifier,
            ..
        }) => format!(
            "`{conflicting_identifier}` is already a unit, constant or function; pick another \
             name such as `{conflicting_identifier}_1`"
        ),
        other => other.to_string(),
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A sheet line split into its name and expression.
#[derive(Debug, Clone, PartialEq)]
struct Line {
    /// Position in the request, from 1.
    number: usize,
    name: Option<String>,
    expr: String,
}

/// `name = expr` (an optional leading `let` is dropped) or a bare
/// expression; `==` is a comparison, not a definition.
fn split(number: usize, text: &str) -> Line {
    let text = text.trim();
    let body = text.strip_prefix("let ").map_or(text, str::trim_start);
    if let Some((name, expr)) = body.split_once('=') {
        let name = name.trim();
        let is_name = matches!(
            syntax::tokens(name).as_slice(),
            [syntax::Token::Ident(n)] if n == name
        );
        if is_name && !expr.starts_with('=') && !name.ends_with(['<', '>', '!']) {
            return Line {
                number,
                name: Some(name.to_owned()),
                expr: expr.trim().to_owned(),
            };
        }
    }
    Line {
        number,
        name: None,
        expr: text.to_owned(),
    }
}

/// The key of a line's value: its name when it can be a slot key, else
/// `line_<n>`.
fn value_key(line: &Line) -> String {
    match &line.name {
        Some(name) if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => name.clone(),
        _ => format!("line_{}", line.number),
    }
}

/// Whether the names reachable from `start` through the sheet's
/// definitions lead back to `target`; the path when they do.
fn cycle(lines: &[Line], start: &str, target: &str) -> Option<Vec<String>> {
    let mut path = vec![start.to_owned()];
    let mut seen = Vec::new();
    fn walk(
        lines: &[Line],
        name: &str,
        target: &str,
        path: &mut Vec<String>,
        seen: &mut Vec<String>,
    ) -> bool {
        if seen.iter().any(|s| s == name) {
            return false;
        }
        seen.push(name.to_owned());
        let Some(line) = lines.iter().find(|l| l.name.as_deref() == Some(name)) else {
            return false;
        };
        for used in syntax::identifiers(&line.expr) {
            if used == target {
                path.push(used);
                return true;
            }
            if lines
                .iter()
                .any(|l| l.name.as_deref() == Some(used.as_str()))
            {
                path.push(used.clone());
                if walk(lines, &used, target, path, seen) {
                    return true;
                }
                path.pop();
            }
        }
        false
    }
    walk(lines, start, target, &mut path, &mut seen).then_some(path)
}

/// Problems with the order of the sheet, found before numbat runs.
fn order_error(lines: &[Line], index: usize, failed: &[(String, usize)]) -> Option<String> {
    let line = &lines[index];
    let earlier = &lines[..index];
    if let Some(name) = &line.name
        && let Some(first) = earlier.iter().find(|l| l.name.as_ref() == Some(name))
    {
        return Some(format!(
            "`{name}` is already defined on line {}; use a new name",
            first.number
        ));
    }
    if line.expr.is_empty() {
        return Some("the line has no expression after `=`".into());
    }
    if line.expr.contains([';', '\n']) {
        return Some("one expression per line".into());
    }
    if let Some(syntax::Token::Ident(word)) = syntax::tokens(&line.expr).first()
        && STATEMENT_KEYWORDS.contains(&word.as_str())
    {
        return Some(format!(
            "`{word}` statements are not allowed; write `name = expression` lines"
        ));
    }
    for used in syntax::identifiers(&line.expr) {
        if line.name.as_deref() == Some(used.as_str()) {
            return Some(format!("`{used}` is defined in terms of itself"));
        }
        if let Some((_, at)) = failed.iter().find(|(n, _)| *n == used) {
            return Some(format!(
                "depends on `{used}`, which has an error (line {at})"
            ));
        }
        let defined_before = earlier.iter().any(|l| l.name.as_ref() == Some(&used));
        if defined_before {
            continue;
        }
        if let Some(later) = lines[index + 1..]
            .iter()
            .find(|l| l.name.as_ref() == Some(&used))
        {
            if let Some(name) = &line.name
                && let Some(path) = cycle(lines, &used, name)
            {
                let mut path = path;
                path.insert(0, name.clone());
                return Some(format!("circular definition: {}", path.join(" → ")));
            }
            return Some(format!(
                "uses `{used}` before it is defined on line {}; move this line below it",
                later.number
            ));
        }
    }
    None
}

/// Evaluates a calc sheet.
///
/// # Errors
///
/// [`ComputeError::Invalid`] for a sheet without lines.
pub fn sheet(request: &CalcRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    let lines: Vec<Line> = request
        .lines
        .iter()
        .enumerate()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(i, text)| split(i + 1, text))
        .collect();
    if lines.is_empty() {
        return Err(ComputeError::Invalid("the sheet has no lines".into()));
    }
    let options = format_options(request.significant_digits);
    let mut ctx = context()?;
    let mut computed = Computed::new(id, "calc.sheet", "Calc sheet", envelope::calc_engine());
    computed.method.why = Some(
        "numbat checks the dimension of every operation and converts units by their \
         definitions"
            .into(),
    );
    computed.method.params.insert(
        "significant_digits".into(),
        serde_json::Value::from(request.significant_digits),
    );
    let sheet_bytes = serde_json::to_vec(&request.lines).unwrap_or_default();
    computed.inputs.push(InputRef {
        name: "sheet".into(),
        role: Some("sheet".into()),
        path: None,
        hash: Some(envelope::hash(&sheet_bytes)),
        rows: Some(lines.len() as u64),
        columns: Vec::new(),
        filter: None,
    });

    let names: Vec<String> = lines.iter().filter_map(|l| l.name.clone()).collect();
    let mut defined: Vec<String> = Vec::new();
    let mut failed: Vec<(String, usize)> = Vec::new();
    let mut out = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let uses = syntax::identifiers(&line.expr);
        let mut calc = CalcLine {
            name: line.name.clone(),
            expr: line.expr.clone(),
            value: None,
            display: None,
            unit: None,
            latex: None,
            dimension: None,
            error: None,
            notes: notes::interpretations(&uses, &names),
        };
        let result = match order_error(&lines, index, &failed) {
            Some(error) => Err(error),
            None => match &line.name {
                Some(name) => ctx
                    .interpret_with_settings(
                        &mut quiet(),
                        &format!("let {name} = {}", line.expr),
                        CodeSource::Text,
                    )
                    .map_err(|e| error_text(&e))
                    .and_then(|_| evaluate(&mut ctx, name, &options)),
                None => evaluate(&mut ctx, &line.expr, &options),
            },
        };
        match result {
            Ok(done) if done.value.is_some_and(|v| !v.is_finite()) => {
                calc.display = Some(done.display);
                calc.error = Some("the result is not a finite number".into());
            }
            Ok(done) => {
                let result_tex = done.number.as_deref().map(|n| latex::result(n, &done.unit));
                calc.latex = latex::line(
                    line.name.as_deref(),
                    &line.expr,
                    &defined,
                    result_tex.as_deref(),
                );
                if let (Some(v), Some(number)) = (done.value, &done.number) {
                    computed.values.insert(
                        value_key(line),
                        ComputedValue {
                            display: Some(number.clone()),
                            unit: (!done.unit.is_empty()).then(|| done.unit.clone()),
                            sig: Some(request.significant_digits),
                            label: done.dimension.clone(),
                            ..ComputedValue::new(v)
                        },
                    );
                }
                calc.value = done.value;
                calc.unit = (!done.unit.is_empty()).then_some(done.unit);
                calc.display = Some(done.display);
                calc.dimension = done.dimension;
            }
            Err(error) => calc.error = Some(error),
        }
        match (&calc.error, &line.name) {
            (Some(_), Some(name)) => failed.push((name.clone(), line.number)),
            (None, Some(name)) => defined.push(name.clone()),
            _ => {}
        }
        if let Some(error) = &calc.error {
            let label = line
                .name
                .as_ref()
                .map_or_else(String::new, |n| format!(" (`{n}`)"));
            computed.warnings.push(envelope::warning(
                "line_error",
                WarningSeverity::Warn,
                format!("line {}{label}: {error}", line.number),
            ));
        }
        for note in &calc.notes {
            let id = format!("unit_reading_{}", computed.assumptions.len() + 1);
            if !computed
                .assumptions
                .iter()
                .any(|a| a.consequence.as_deref() == Some(note))
            {
                computed.assumptions.push(Assumption {
                    id,
                    check: None,
                    stat: None,
                    p: None,
                    status: AssumptionStatus::NotChecked,
                    consequence: Some(note.clone()),
                    note: Some("how the engine read an ambiguous unit; confirm".into()),
                });
            }
        }
        out.push(calc);
    }
    if out.iter().all(|l| l.error.is_some()) {
        let first = out.iter().find_map(|l| l.error.clone()).unwrap_or_default();
        computed.status = ComputedStatus::Error;
        computed.error = Some(format!("no line could be evaluated: {first}"));
    }
    computed.reproduce.shell = Some(shell_line(&lines));
    Ok((
        computed,
        ComputedBody::CalcSheet(CalcSheetBody { lines: out }),
    ))
}

/// `numbat -e '…'` running the whole sheet.
fn shell_line(lines: &[Line]) -> String {
    let program: Vec<String> = lines
        .iter()
        .map(|l| match &l.name {
            Some(name) => format!("let {name} = {}", l.expr),
            None => l.expr.clone(),
        })
        .collect();
    format!(
        "numbat -e {}",
        crate::reproduce::shell_word(&program.join("; "))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str]) -> (Computed, Vec<CalcLine>) {
        let request = CalcRequest {
            lines: lines.iter().map(|l| (*l).to_owned()).collect(),
            significant_digits: 6,
        };
        let (computed, body) = sheet(&request, "ca_test".into()).unwrap_or_else(|e| panic!("{e}"));
        let ComputedBody::CalcSheet(body) = body else {
            panic!("not a sheet");
        };
        (computed, body.lines)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn evaluates_si_arithmetic_with_names() {
        let (computed, lines) = run(&[
            "m_car = 1200 kg",
            "v = 100 km/h",
            "E = 0.5 * m_car * v^2 -> kJ",
            "F_w = 70 kg * 9.81 m/s^2",
        ]);
        assert!(lines.iter().all(|l| l.error.is_none()), "{lines:#?}");
        let e = &lines[2];
        assert!(close(e.value.unwrap_or_default(), 462.962_962_962_963));
        assert_eq!(e.unit.as_deref(), Some("kJ"));
        assert_eq!(e.dimension.as_deref(), Some("Energy or Torque"));
        assert_eq!(e.display.as_deref(), Some("462.963 kJ"));
        assert_eq!(
            e.latex.as_deref(),
            Some("E = 0.5 \\cdot m_{\\mathrm{car}} \\cdot v^{2} = 462.963\\,\\mathrm{kJ}")
        );
        assert_eq!(lines[3].display.as_deref(), Some("686.7 N"));
        assert_eq!(lines[3].dimension.as_deref(), Some("Force"));
        let value = computed.value("E").unwrap_or_else(|| panic!("no E"));
        assert_eq!(value.display_text(), "462.963 kJ");
        assert_eq!(computed.status, ComputedStatus::Ok);
        assert!(computed.warnings.is_empty());
        assert_eq!(
            computed.reproduce.shell.as_deref(),
            Some(
                "numbat -e 'let m_car = 1200 kg; let v = 100 km/h; let E = 0.5 * m_car * \
                 v^2 -> kJ; let F_w = 70 kg * 9.81 m/s^2'"
            )
        );
    }

    #[test]
    fn converts_imperial_lengths_exactly() {
        let (computed, lines) = run(&["h_1 = 5 ft + 3 in", "h_1 -> cm"]);
        assert_eq!(lines[1].display.as_deref(), Some("160.02 cm"));
        assert!(close(lines[1].value.unwrap_or_default(), 160.02));
        assert_eq!(lines[1].dimension.as_deref(), Some("Length"));
        assert_eq!(
            computed.value("line_2").map(ComputedValue::display_text),
            Some("160.02 cm".into())
        );
    }

    #[test]
    fn energy_and_power() {
        let (_, lines) = run(&["P = 1.5 kW", "t = 3 h", "E = P * t -> kWh", "E -> MJ"]);
        assert_eq!(lines[2].display.as_deref(), Some("4.5 kWh"));
        assert_eq!(lines[3].display.as_deref(), Some("16.2 MJ"));
        assert_eq!(lines[0].dimension.as_deref(), Some("Power"));
    }

    #[test]
    fn a_dimension_error_stays_on_its_line() {
        let (computed, lines) = run(&["dist = 3 m", "bad = dist + 2 s", "ok = 2 dist"]);
        let error = lines[1].error.as_deref().unwrap_or_default();
        assert!(
            error.starts_with("incompatible dimensions in addition"),
            "{error}"
        );
        assert!(
            error.contains("Length") && error.contains("Time"),
            "{error}"
        );
        assert_eq!(lines[1].value, None);
        assert_eq!(lines[2].display.as_deref(), Some("6 m"));
        assert_eq!(computed.status, ComputedStatus::Ok);
        assert_eq!(computed.warnings.len(), 1);
        assert!(computed.warnings[0].text.starts_with("line 2 (`bad`)"));
        assert!(!computed.values.contains_key("bad"));
    }

    #[test]
    fn order_problems_get_clear_errors() {
        let (_, lines) = run(&[
            "a = b + 1",
            "b = 2",
            "c = a * 2",
            "x = y",
            "y = x",
            "b = 3",
            "h = 2",
            "z = z + 1",
        ]);
        let errors: Vec<&str> = lines
            .iter()
            .map(|l| l.error.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(
            errors[0],
            "uses `b` before it is defined on line 2; move this line below it"
        );
        assert_eq!(errors[1], "");
        assert_eq!(errors[2], "depends on `a`, which has an error (line 1)");
        assert_eq!(errors[3], "circular definition: x → y → x");
        assert_eq!(errors[4], "depends on `x`, which has an error (line 4)");
        assert_eq!(
            errors[5],
            "`b` is already defined on line 2; use a new name"
        );
        assert!(errors[6].contains("`h` is already a unit"), "{}", errors[6]);
        assert_eq!(errors[7], "`z` is defined in terms of itself");
    }

    #[test]
    fn notes_ambiguous_units_and_rejects_statements() {
        let (computed, lines) = run(&["load = 2 ton -> lb", "fn f(x) = x", "use extra"]);
        assert_eq!(lines[0].notes.len(), 2, "ton and lb");
        assert!(lines[0].notes[0].contains("metric tonne"));
        assert_eq!(computed.assumptions.len(), 2);
        assert_eq!(computed.assumptions[0].status, AssumptionStatus::NotChecked);
        assert!(lines[1].error.as_deref().unwrap_or("").contains("`fn`"));
        assert!(lines[2].error.as_deref().unwrap_or("").contains("`use`"));
    }

    #[test]
    fn a_sheet_where_nothing_evaluates_is_an_error() {
        let (computed, lines) = run(&["1 m + 1 s", "foo_bar_baz"]);
        assert_eq!(computed.status, ComputedStatus::Error);
        assert!(
            lines[1]
                .error
                .as_deref()
                .unwrap_or("")
                .starts_with("unknown unit or name `foo_bar_baz`")
        );
        let empty = CalcRequest {
            lines: vec![" ".into()],
            significant_digits: 6,
        };
        assert!(matches!(
            sheet(&empty, "x".into()),
            Err(ComputeError::Invalid(_))
        ));
    }

    #[test]
    fn tidies_integral_floats() {
        assert_eq!(tidy("150.0"), "150");
        assert_eq!(tidy("1.0e+6"), "1e+6");
        assert_eq!(tidy("2.05"), "2.05");
        assert_eq!(tidy("-3"), "-3");
    }

    #[test]
    fn splits_definitions_from_comparisons() {
        assert_eq!(split(1, "let x = 2").name.as_deref(), Some("x"));
        assert_eq!(split(1, "a == b").name, None);
        assert_eq!(split(1, "a <= b").name, None);
        assert_eq!(split(1, "2 m = 3").name, None);
    }
}
