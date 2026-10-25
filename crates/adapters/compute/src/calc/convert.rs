//! One unit conversion: the result, the factor, whether the factor is
//! exact, the dimension and the chain through the coherent SI unit.
//!
//! Exactness is judged from the factor itself: a factor (or its inverse)
//! that is a short decimal, such as 0.3048 or 1.609344, follows from
//! definitions that are exact (the inch is 25.4 mm since 1959); a factor
//! whose digits never end within double precision comes from a rounded
//! definition. Units numbat itself defines by a rounded value (psi) are
//! never called exact.

use application::ports::compute::{ComputeError, ConvertRequest};
use domain::computed::{
    Computed, ComputedBody, ComputedValue, ConversionStep, InputRef, Quantity, QuantityBody,
};

use super::{context, evaluate, format_options, syntax};
use crate::envelope;
use crate::reproduce::{py_str, shell_word};

/// Units numbat defines by a rounded value.
const ROUNDED_DEFINITIONS: &[&str] = &["psi", "ksi", "mpsi"];

/// Temperature scales with an offset, by the names users write, and the
/// name numbat knows.
const AFFINE: &[(&[&str], &str, &str)] = &[
    (
        &["°C", "degC", "celsius", "Celsius", "degree_celsius"],
        "°C",
        "degC",
    ),
    (
        &[
            "°F",
            "degF",
            "fahrenheit",
            "Fahrenheit",
            "degree_fahrenheit",
        ],
        "°F",
        "degF",
    ),
];

/// The numbat and pint names of an offset temperature scale.
fn affine(unit: &str) -> Option<(&'static str, &'static str)> {
    AFFINE
        .iter()
        .find(|(names, _, _)| names.contains(&unit))
        .map(|(_, numbat, pint)| (*numbat, *pint))
}

/// Whether `x` has at most ten significant decimal digits.
fn short_decimal(x: f64) -> bool {
    if !(x.is_finite() && x != 0.0) {
        return false;
    }
    let text = format!("{:.11e}", x.abs());
    let mantissa = text.split('e').next().unwrap_or_default();
    let digits = mantissa.replace('.', "");
    digits.trim_end_matches('0').len() <= 10
}

/// Whether a conversion factor is exact by definition.
fn exact_factor(factor: f64, units: &[&str]) -> bool {
    let rounded = units.iter().any(|u| {
        syntax::identifiers(u)
            .iter()
            .any(|name| ROUNDED_DEFINITIONS.contains(&name.as_str()))
    });
    !rounded && (short_decimal(factor) || short_decimal(1.0 / factor))
}

/// `x` with twelve significant digits, trailing zeros dropped.
fn short(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return x.to_string();
    }
    // log10 of a finite non-zero number fits in i32.
    #[allow(clippy::cast_possible_truncation)]
    let exponent = x.abs().log10().floor() as i32;
    if (-5..15).contains(&exponent) {
        let decimals = usize::try_from((11 - exponent).max(0)).unwrap_or(0);
        let text = format!("{x:.decimals$}");
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            text
        }
    } else {
        let text = format!("{x:.11e}");
        let (mantissa, exp) = text.split_once('e').unwrap_or((&text, "0"));
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{mantissa}e{exp}")
    }
}

/// The coherent SI unit of a numbat base representation: numbat's base
/// unit of mass is the gram.
fn si_unit(base: &str) -> String {
    let mut out = String::with_capacity(base.len() + 1);
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word == "g" {
            out.push_str("kg");
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in base.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn invalid(message: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(message.into())
}

/// Converts `request`.
///
/// # Errors
///
/// [`ComputeError::Invalid`] for unknown units, a dimension mismatch or an
/// offset temperature without a `value`.
pub fn convert(
    request: &ConvertRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    let options = format_options(request.significant_digits);
    let mut ctx = context()?;
    let from = request.from.trim();
    let to = request.to.trim();
    let from_affine = affine(from);
    let to_affine = affine(to);

    // The quantity converted, as numbat code, for a magnitude `v`.
    let quantity = |v: f64| -> Option<String> {
        match (from_affine, request.value) {
            (Some((name, _)), Some(_)) => Some(format!("({v}) {name}")),
            (None, Some(_)) => Some(format!("({v}) × ({from})")),
            (_, None) => None,
        }
    };
    let target = |code: &str| match to_affine {
        Some((name, _)) => format!("({code}) -> {name}"),
        None => format!("({code}) -> ({to})"),
    };
    let fail = |e: String| invalid(format!("cannot convert `{from}` to `{to}`: {e}"));

    let (from_quantity, from_code) = match request.value {
        Some(v) => {
            let code = quantity(v).unwrap_or_default();
            // Shown as given: `65 mi/h`, `20 °C`.
            let unit = from_affine.map_or_else(|| from.to_owned(), |(n, _)| n.to_owned());
            (
                Quantity {
                    value: v,
                    unit,
                    uncertainty: request.uncertainty,
                },
                code,
            )
        }
        None => {
            if from_affine.is_some() {
                return Err(invalid(format!(
                    "give `value` with an offset temperature unit such as `{from}`"
                )));
            }
            let done = evaluate(&mut ctx, from, &options).map_err(fail)?;
            let value = done
                .value
                .ok_or_else(|| invalid(format!("`{from}` is not a quantity")))?;
            (
                Quantity {
                    value,
                    unit: done.unit,
                    uncertainty: request.uncertainty,
                },
                format!("({from})"),
            )
        }
    };
    let result = evaluate(&mut ctx, &target(&from_code), &options).map_err(fail)?;
    let to_value = result
        .value
        .ok_or_else(|| invalid(format!("`{to}` did not give a number")))?;
    let to_unit = match to_affine {
        Some((name, _)) => name.to_owned(),
        None => result.unit.clone(),
    };

    // The factor: what one `from` unit is in `to` units, or for an offset
    // scale the slope of the conversion.
    let (factor, dimension, offset) = if from_affine.is_some() || to_affine.is_some() {
        let mut at = |v: f64| -> Result<f64, ComputeError> {
            let code = match (from_affine, request.value) {
                (_, Some(_)) => quantity(v).unwrap_or_default(),
                (_, None) => format!(
                    "({v}) × ({from_quantity_unit})",
                    from_quantity_unit = from_quantity.unit
                ),
            };
            evaluate(&mut ctx, &target(&code), &options)
                .map_err(fail)?
                .value
                .ok_or_else(|| invalid("no value"))
        };
        let zero = at(0.0)?;
        let one = at(1.0)?;
        let slope: f64 = short(one - zero).parse().unwrap_or(one - zero);
        let offset: f64 = short(zero).parse().unwrap_or(zero);
        (slope, "Temperature".to_owned(), Some(offset))
    } else {
        let unit_code = if from_quantity.unit.is_empty() {
            "1".to_owned()
        } else {
            format!("1 × ({})", from_quantity.unit)
        };
        let one = evaluate(&mut ctx, &target(&unit_code), &options).map_err(fail)?;
        let factor = one.value.unwrap_or(f64::NAN);
        let dimension = one
            .dimension
            .clone()
            .unwrap_or_else(|| "Dimensionless".into());
        (factor, dimension, None)
    };
    let exact = exact_factor(factor, &[from, to]);
    let chain = match offset {
        Some(offset) => vec![ConversionStep {
            from_unit: from_quantity.unit.clone(),
            to_unit: to_unit.clone(),
            factor,
            note: Some(format!(
                "offset scale: 0 {} = {} {to_unit}",
                from_quantity.unit,
                short(offset)
            )),
        }],
        None => chain(
            &mut ctx,
            &from_quantity.unit,
            to,
            &to_unit,
            factor,
            &options,
        ),
    };
    let uncertainty = request.uncertainty.map(|u| u * factor.abs());

    let mut computed = Computed::new(
        id,
        "calc.convert",
        "Unit conversion",
        envelope::calc_engine(),
    );
    computed.method.why = Some(if exact {
        "the factor follows from exact unit definitions".into()
    } else {
        "the factor comes from a rounded unit definition".into()
    });
    let request_bytes = serde_json::to_vec(request).unwrap_or_default();
    computed.inputs.push(InputRef {
        name: "conversion".into(),
        role: Some("quantity".into()),
        path: None,
        hash: Some(envelope::hash(&request_bytes)),
        rows: None,
        columns: Vec::new(),
        filter: None,
    });
    let sig = request.significant_digits;
    computed.values.insert(
        "result".into(),
        ComputedValue {
            display: result.number.clone(),
            unit: Some(to_unit.clone()),
            sig: Some(sig),
            label: Some(dimension.clone()),
            ..ComputedValue::new(to_value)
        },
    );
    computed.values.insert(
        "factor".into(),
        ComputedValue {
            display: Some(short(factor)),
            label: Some(if exact { "exact" } else { "rounded" }.into()),
            ..ComputedValue::new(factor)
        },
    );
    if let Some(u) = uncertainty {
        computed.values.insert(
            "uncertainty".into(),
            ComputedValue {
                display: Some(short(u)),
                unit: Some(to_unit.clone()),
                ..ComputedValue::new(u)
            },
        );
    }
    computed.reproduce.shell = Some(format!("numbat -e {}", shell_word(&target(&from_code))));
    computed.reproduce.python = Some(pint_line(request, to));
    let body = QuantityBody {
        from: from_quantity,
        to: Quantity {
            value: to_value,
            unit: to_unit,
            uncertainty,
        },
        factor,
        exact,
        dimension,
        chain,
    };
    Ok((computed, ComputedBody::Quantity(body)))
}

/// The steps from `from_unit` to `to` through the coherent SI unit.
fn chain(
    ctx: &mut numbat::Context,
    from_unit: &str,
    to: &str,
    to_unit: &str,
    factor: f64,
    options: &numbat::FormatOptions,
) -> Vec<ConversionStep> {
    let note = |f: f64, units: &[&str]| {
        Some(if exact_factor(f, units) {
            "exact".to_owned()
        } else {
            "rounded".to_owned()
        })
    };
    let direct = vec![ConversionStep {
        from_unit: from_unit.to_owned(),
        to_unit: to_unit.to_owned(),
        factor,
        note: note(factor, &[from_unit, to]),
    }];
    if from_unit.is_empty() {
        return direct;
    }
    // numbat's base representation of the unit, e.g. `g·m²/s²`.
    let Ok(base) = base_unit(ctx, from_unit) else {
        return direct;
    };
    let si = si_unit(&base);
    if si == from_unit || si == to_unit || si.is_empty() {
        return direct;
    }
    let step = |ctx: &mut numbat::Context, a: &str, b: &str| -> Option<f64> {
        evaluate(ctx, &format!("(1 × ({a})) -> ({b})"), options)
            .ok()
            .and_then(|e| e.value)
    };
    match (step(ctx, from_unit, &si), step(ctx, &si, to)) {
        (Some(a), Some(b)) => vec![
            ConversionStep {
                from_unit: from_unit.to_owned(),
                to_unit: si.clone(),
                factor: a,
                note: note(a, &[from_unit]),
            },
            ConversionStep {
                from_unit: si,
                to_unit: to_unit.to_owned(),
                factor: b,
                note: note(b, &[to]),
            },
        ],
        _ => direct,
    }
}

/// The base-unit representation of `unit`, as numbat prints it.
fn base_unit(ctx: &mut numbat::Context, unit: &str) -> Result<String, String> {
    use numbat::resolver::CodeSource;
    use numbat::value::Value;
    let code = format!("1 × ({unit})");
    let (_, result) = ctx
        .interpret_with_settings(&mut super::quiet(), &code, CodeSource::Text)
        .map_err(|e| e.to_string())?;
    match result {
        numbat::InterpreterResult::Value(Value::Quantity(q)) => {
            Ok(q.to_base_unit_representation().unit().to_string())
        }
        _ => Err("not a quantity".into()),
    }
}

/// The same conversion with Python's pint.
fn pint_line(request: &ConvertRequest, to: &str) -> String {
    let pint = |unit: &str| affine(unit).map_or_else(|| unit.to_owned(), |(_, p)| p.to_owned());
    let quantity = match request.value {
        Some(v) => format!("u.Quantity({v}, {})", py_str(&pint(&request.from))),
        None => format!("u({})", py_str(&request.from)),
    };
    format!(
        "import pint; u = pint.UnitRegistry(); print({quantity}.to({}))",
        py_str(&pint(to))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(value: Option<f64>, from: &str, to: &str) -> (Computed, QuantityBody) {
        let request = ConvertRequest {
            value,
            from: from.into(),
            to: to.into(),
            uncertainty: None,
            significant_digits: 6,
        };
        let (computed, body) =
            convert(&request, "ca_test".into()).unwrap_or_else(|e| panic!("{e}"));
        let ComputedBody::Quantity(body) = body else {
            panic!("not a quantity");
        };
        (computed, body)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn feet_and_inches_to_centimetres_is_exact() {
        let (computed, body) = run(None, "5 ft + 3 in", "cm");
        assert!(close(body.to.value, 160.02));
        assert_eq!(body.to.unit, "cm");
        // numbat sums in the smaller unit: 63 in.
        assert_eq!(body.from.unit, "in");
        assert!(close(body.from.value, 63.0));
        assert!(close(body.factor, 2.54));
        assert!(body.exact);
        assert_eq!(body.dimension, "Length");
        assert_eq!(
            computed.value("result").map(ComputedValue::display_text),
            Some("160.02 cm".into())
        );
        assert_eq!(body.chain.len(), 2, "{:?}", body.chain);
        assert_eq!(body.chain[0].to_unit, "m");
        assert!(close(body.chain[0].factor, 0.0254));
        assert!(close(body.chain[1].factor, 100.0));
        assert_eq!(
            computed.reproduce.shell.as_deref(),
            Some("numbat -e '((5 ft + 3 in)) -> (cm)'")
        );
    }

    #[test]
    fn speeds_and_rounded_definitions() {
        let (_, body) = run(Some(100.0), "km/h", "mph");
        assert!(close(body.to.value, 62.137_119_223_733_4));
        // 1 mi = 1.609344 km exactly, so the inverse factor is exact too.
        assert!(body.exact);
        let (_, body) = run(Some(30.0), "psi", "kPa");
        assert!(!body.exact);
        assert!(close(body.factor, 6.894_757));
    }

    #[test]
    fn energy_with_mass_in_kilograms() {
        let (computed, body) = run(Some(2.0), "kWh", "MJ");
        assert!(close(body.to.value, 7.2));
        assert!(body.exact);
        assert_eq!(body.chain[0].to_unit, "kg·m²/s²");
        assert!(
            computed
                .reproduce
                .python
                .as_deref()
                .unwrap_or("")
                .contains("u.Quantity(2, 'kWh').to('MJ')")
        );
    }

    #[test]
    fn temperatures_convert_with_their_offset() {
        let (computed, body) = run(Some(25.0), "degC", "degF");
        assert!(close(body.to.value, 77.0));
        assert_eq!(body.to.unit, "°F");
        assert!(close(body.factor, 1.8));
        assert!(body.exact);
        assert_eq!(body.dimension, "Temperature");
        assert!(
            body.chain[0]
                .note
                .as_deref()
                .unwrap_or("")
                .contains("0 °C = 32 °F")
        );
        assert!(
            computed
                .reproduce
                .python
                .as_deref()
                .unwrap_or("")
                .contains("'degC'")
        );
        let (_, body) = run(Some(300.0), "K", "°C");
        assert!(close(body.to.value, 26.85));
    }

    #[test]
    fn uncertainty_propagates_linearly() {
        let request = ConvertRequest {
            value: Some(10.0),
            from: "in".into(),
            to: "mm".into(),
            uncertainty: Some(0.5),
            significant_digits: 6,
        };
        let (computed, body) = convert(&request, "x".into()).unwrap_or_else(|e| panic!("{e}"));
        let ComputedBody::Quantity(body) = body else {
            panic!("not a quantity");
        };
        assert!(close(body.to.uncertainty.unwrap_or_default(), 12.7));
        assert!(computed.values.contains_key("uncertainty"));
    }

    #[test]
    fn mismatches_and_unknown_units_are_invalid() {
        for (from, to) in [("kg", "m"), ("furlongz", "m"), ("m", "")] {
            let request = ConvertRequest {
                value: Some(1.0),
                from: from.into(),
                to: to.into(),
                uncertainty: None,
                significant_digits: 6,
            };
            assert!(
                matches!(convert(&request, "x".into()), Err(ComputeError::Invalid(_))),
                "{from} → {to}"
            );
        }
    }

    #[test]
    fn short_decimals() {
        assert!(short_decimal(0.304_799_999_999_999_96));
        assert!(short_decimal(1.609_344));
        assert!(!short_decimal(1.0 / 1.609_344));
        assert_eq!(short(30.479_999_999_999_997), "30.48");
        assert_eq!(short(1.8), "1.8");
        assert_eq!(si_unit("g·m²/s²"), "kg·m²/s²");
        assert_eq!(si_unit("mg"), "mg");
    }
}
