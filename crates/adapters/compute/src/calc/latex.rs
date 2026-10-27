//! TeX of calc lines: the expression as written (fractions for `/`,
//! exponents, `\cdot` for `*`, units upright) and the result.
//!
//! Variables of the sheet are italic (`v_0` → `v_{0}`), units and
//! constants upright, known functions use their TeX operator. The tree
//! keeps the user's parentheses, so the typeset line groups exactly as
//! numbat evaluates it (`1/2 m` shows as `\frac{1}{2\,\mathrm{m}}`).

use super::syntax::{Expr, parse};

/// Greek letters written out, as names numbat and users use.
const GREEK: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    "lambda", "mu", "nu", "xi", "pi", "rho", "sigma", "tau", "upsilon", "phi", "chi", "psi",
    "omega", "Gamma", "Delta", "Theta", "Lambda", "Xi", "Pi", "Sigma", "Phi", "Psi", "Omega",
];

/// Functions with a TeX operator of their own.
const OPERATORS: &[(&str, &str)] = &[
    ("sin", "\\sin"),
    ("cos", "\\cos"),
    ("tan", "\\tan"),
    ("asin", "\\arcsin"),
    ("acos", "\\arccos"),
    ("atan", "\\arctan"),
    ("sinh", "\\sinh"),
    ("cosh", "\\cosh"),
    ("tanh", "\\tanh"),
    ("exp", "\\exp"),
    ("ln", "\\ln"),
    ("log", "\\log"),
    ("log10", "\\log_{10}"),
    ("log2", "\\log_{2}"),
];

/// Turns names into TeX: which ones are sheet variables.
pub struct Namer<'a> {
    /// Names defined on earlier lines.
    pub variables: &'a [String],
}

impl Namer<'_> {
    fn name(&self, name: &str) -> String {
        if self.variables.iter().any(|v| v == name) {
            variable(name)
        } else if let Some(greek) = greek(name) {
            greek
        } else {
            unit(name)
        }
    }
}

/// `v_0` → `v_{0}`, `m_car` → `m_{\mathrm{car}}`, `alpha` → `\alpha`.
pub fn variable(name: &str) -> String {
    let (base, sub) = match name.split_once('_') {
        Some((base, sub)) if !base.is_empty() && !sub.is_empty() => (base, Some(sub)),
        _ => (name, None),
    };
    let base = greek(base).unwrap_or_else(|| {
        if base.chars().count() == 1 {
            escape(base)
        } else {
            format!("\\mathit{{{}}}", escape(base))
        }
    });
    match sub {
        Some(sub) if sub.chars().count() == 1 || sub.chars().all(|c| c.is_ascii_digit()) => {
            format!("{base}_{{{}}}", escape(sub))
        }
        Some(sub) => format!("{base}_{{\\mathrm{{{}}}}}", escape(sub)),
        None => base,
    }
}

fn greek(name: &str) -> Option<String> {
    if GREEK.contains(&name) {
        return Some(format!("\\{name}"));
    }
    let letter = match name {
        "π" => "\\pi",
        "τ" => "\\tau",
        "α" => "\\alpha",
        "β" => "\\beta",
        "γ" => "\\gamma",
        "δ" => "\\delta",
        "ε" => "\\varepsilon",
        "θ" => "\\theta",
        "λ" => "\\lambda",
        "μ" => "\\mu",
        "ρ" => "\\rho",
        "σ" => "\\sigma",
        "φ" => "\\varphi",
        "ω" => "\\omega",
        "Ω" => "\\Omega",
        "Δ" => "\\Delta",
        _ => return None,
    };
    Some(letter.into())
}

/// A unit upright: `km` → `\mathrm{km}`, `µm` → `\mu\mathrm{m}`, `°C` →
/// `{}^{\circ}\mathrm{C}`.
pub fn unit(name: &str) -> String {
    match name {
        "Ω" => return "\\Omega".into(),
        "%" => return "\\%".into(),
        "°" => return "{}^{\\circ}".into(),
        _ => {}
    }
    if let Some(rest) = name.strip_prefix('°') {
        return format!("{{}}^{{\\circ}}\\mathrm{{{}}}", escape(rest));
    }
    for micro in ['µ', 'μ'] {
        if let Some(rest) = name.strip_prefix(micro)
            && !rest.is_empty()
        {
            return format!("\\mu\\mathrm{{{}}}", escape(rest));
        }
    }
    format!("\\mathrm{{{}}}", escape(name))
}

/// Escapes TeX specials in a name.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '_' | '%' | '$' | '&' | '#' => {
                out.push('\\');
                out.push(c);
            }
            '€' => out.push_str("\\text{€}"),
            '£' => out.push_str("\\pounds{}"),
            c => out.push(c),
        }
    }
    out
}

/// A number as written: digit separators dropped, `1.5e3` as
/// `1.5 \times 10^{3}`.
pub fn number(text: &str) -> String {
    let text = text.replace('_', "");
    if text.starts_with("0x") || text.starts_with("0o") || text.starts_with("0b") {
        return format!("\\mathtt{{{text}}}");
    }
    match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => {
            let exponent = exponent.trim_start_matches('+');
            if mantissa == "1" {
                format!("10^{{{exponent}}}")
            } else {
                format!("{mantissa} \\times 10^{{{exponent}}}")
            }
        }
        None => text,
    }
}

fn strip(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(inner) => strip(inner),
        expr => expr,
    }
}

impl Namer<'_> {
    /// The TeX of `expr`.
    pub fn expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::Number(n) => number(n),
            Expr::Ident(name) => self.name(name),
            Expr::Paren(inner) => format!("\\left({}\\right)", self.expr(inner)),
            Expr::Unary(op, inner) => {
                let inner_tex = self.expr(inner);
                let op = if *op == "-" { "-" } else { "+" };
                match strip(inner) {
                    Expr::Binary("+" | "-", ..) if !matches!(**inner, Expr::Paren(_)) => {
                        format!("{op}\\left({inner_tex}\\right)")
                    }
                    _ => format!("{op}{inner_tex}"),
                }
            }
            Expr::Binary("/", num, den) => format!(
                "\\frac{{{}}}{{{}}}",
                self.expr(strip(num)),
                self.expr(strip(den))
            ),
            Expr::Binary(op, left, right) => {
                let op = match *op {
                    "*" | "·" => "\\cdot",
                    "×" => "\\times",
                    "+" => "+",
                    _ => "-",
                };
                format!("{} {op} {}", self.expr(left), self.expr(right))
            }
            Expr::Implicit(left, right) => {
                let sep = match (strip(left), strip(right)) {
                    (_, Expr::Number(_)) => " \\cdot ",
                    _ => "\\,",
                };
                format!("{}{sep}{}", self.expr(left), self.expr(right))
            }
            Expr::Power(base, exponent) => {
                let base_tex = self.expr(base);
                let base_tex = match **base {
                    Expr::Unary(..)
                    | Expr::Binary(..)
                    | Expr::Implicit(..)
                    | Expr::Power(..)
                    | Expr::Convert(..) => format!("\\left({base_tex}\\right)"),
                    _ => base_tex,
                };
                format!("{base_tex}^{{{}}}", self.expr(strip(exponent)))
            }
            Expr::Factorial(inner) => format!("{}!", self.expr(inner)),
            Expr::Call(name, args) => self.call(name, args),
            Expr::Convert(value, target) => {
                format!("{} \\rightarrow {}", self.expr(value), self.expr(target))
            }
        }
    }

    fn call(&self, name: &str, args: &[Expr]) -> String {
        let joined = args
            .iter()
            .map(|a| self.expr(a))
            .collect::<Vec<_>>()
            .join(", ");
        match (name, args) {
            ("sqrt", [arg]) => format!("\\sqrt{{{}}}", self.expr(strip(arg))),
            ("cbrt", [arg]) => format!("\\sqrt[3]{{{}}}", self.expr(strip(arg))),
            ("abs", [arg]) => format!("\\left|{}\\right|", self.expr(strip(arg))),
            _ => {
                let op = OPERATORS.iter().find(|(n, _)| *n == name).map_or_else(
                    || format!("\\operatorname{{{}}}", escape(name)),
                    |(_, op)| (*op).to_owned(),
                );
                format!("{op}\\left({joined}\\right)")
            }
        }
    }
}

/// Whether `expr` is a bare literal such as `100 km/h` or `9.81 m/s^2`,
/// whose value needs no `= result`.
pub fn is_literal(expr: &Expr, variables: &[String]) -> bool {
    fn units_only(expr: &Expr, variables: &[String]) -> bool {
        match expr {
            Expr::Ident(name) => !variables.contains(name) && greek(name).is_none(),
            Expr::Paren(inner) => units_only(inner, variables),
            Expr::Binary("*" | "·" | "×" | "/", a, b) | Expr::Implicit(a, b) => {
                units_only(a, variables) && units_only(b, variables)
            }
            Expr::Power(base, exponent) => {
                units_only(base, variables)
                    && matches!(strip(exponent), Expr::Number(_) | Expr::Unary("-", _))
            }
            _ => false,
        }
    }
    match expr {
        Expr::Number(_) => true,
        Expr::Unary("-", inner) => matches!(**inner, Expr::Number(_)),
        Expr::Implicit(value, unit) => {
            matches!(**value, Expr::Number(_)) && units_only(unit, variables)
        }
        Expr::Binary("/", num, den) => is_literal(num, variables) && units_only(den, variables),
        _ => false,
    }
}

/// The TeX of a result: `686.7` and `kg·m²/s²` → `686.7\,\frac{…}{…}`.
pub fn result(display_number: &str, unit_text: &str) -> String {
    let number = number(display_number.trim());
    if unit_text.is_empty() {
        return number;
    }
    let unit_tex = parse(unit_text).map_or_else(
        || unit(unit_text),
        |expr| Namer { variables: &[] }.expr(&expr),
    );
    format!("{number}\\,{unit_tex}")
}

/// The TeX of a whole line: `name = expr = result`, the result left out
/// for a bare literal and a final conversion folded into it. `None` when
/// the expression is outside the subset [`parse`] reads.
pub fn line(
    name: Option<&str>,
    expr: &str,
    variables: &[String],
    result_tex: Option<&str>,
) -> Option<String> {
    let parsed = parse(expr)?;
    let shown = match &parsed {
        Expr::Convert(value, _) => value.as_ref(),
        expr => expr,
    };
    let namer = Namer { variables };
    let mut parts = Vec::new();
    if let Some(name) = name {
        parts.push(variable(name));
    }
    parts.push(namer.expr(shown));
    if let Some(result_tex) = result_tex
        && !(is_literal(shown, variables) && !matches!(parsed, Expr::Convert(..)))
    {
        parts.push(result_tex.to_owned());
    }
    Some(parts.join(" = "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(expr: &str, variables: &[&str]) -> String {
        let variables: Vec<String> = variables.iter().map(|v| (*v).to_owned()).collect();
        let parsed = parse(expr).unwrap_or_else(|| panic!("parse {expr}"));
        Namer {
            variables: &variables,
        }
        .expr(&parsed)
    }

    #[test]
    fn typesets_units_fractions_and_powers() {
        assert_eq!(
            tex("9.81 m/s^2", &[]),
            "\\frac{9.81\\,\\mathrm{m}}{\\mathrm{s}^{2}}"
        );
        assert_eq!(
            tex("0.5 * m_car * v^2", &["m_car", "v"]),
            "0.5 \\cdot m_{\\mathrm{car}} \\cdot v^{2}"
        );
        assert_eq!(
            tex("sqrt(2 g_0 h_1)", &["g_0", "h_1"]),
            "\\sqrt{2\\,g_{0}\\,h_{1}}"
        );
        assert_eq!(tex("(a + b)^2", &["a", "b"]), "\\left(a + b\\right)^{2}");
        assert_eq!(tex("1.5e3 W", &[]), "1.5 \\times 10^{3}\\,\\mathrm{W}");
        assert_eq!(tex("20 °C", &[]), "20\\,{}^{\\circ}\\mathrm{C}");
        assert_eq!(tex("5 µm", &[]), "5\\,\\mu\\mathrm{m}");
        assert_eq!(
            tex("sin(30 deg)", &[]),
            "\\sin\\left(30\\,\\mathrm{deg}\\right)"
        );
        assert_eq!(tex("2 pi r", &["r"]), "2\\,\\pi\\,r");
        assert_eq!(
            tex("speed_avg", &["speed_avg"]),
            "\\mathit{speed}_{\\mathrm{avg}}"
        );
    }

    #[test]
    fn a_line_shows_the_result_unless_the_expression_is_a_literal() {
        let vars = vec!["P".to_owned()];
        assert_eq!(
            line(Some("P"), "3 kW", &[], Some("3\\,\\mathrm{kW}")).as_deref(),
            Some("P = 3\\,\\mathrm{kW}")
        );
        assert_eq!(
            line(
                Some("E"),
                "P * 2 h -> kWh",
                &vars,
                Some(&result("6", "kWh"))
            )
            .as_deref(),
            Some("E = P \\cdot 2\\,\\mathrm{h} = 6\\,\\mathrm{kWh}")
        );
        assert_eq!(
            result("686.7", "kg·m/s²"),
            "686.7\\,\\frac{\\mathrm{kg} \\cdot \\mathrm{m}}{\\mathrm{s}^{2}}"
        );
        assert_eq!(
            result("1.60218e-19", "J"),
            "1.60218 \\times 10^{-19}\\,\\mathrm{J}"
        );
        assert_eq!(line(None, "if a then b else c", &[], None), None);
    }
}
