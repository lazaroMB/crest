//! Expressions of the solver cards, on the plot expression parser
//! (`domain::plot::expr`): checking names, splitting a relation such as
//! `2*x + 3*y <= 120` at its comparison, and reading the coefficients of a
//! linear one.

use domain::computed::ConstraintOp;
use domain::plot::expr::{Expr, is_builtin};

/// Whether `name` can be a variable: an identifier that is not a function
/// or constant of the parser.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !is_builtin(name)
}

/// Checks a list of variable names: valid and distinct.
///
/// # Errors
///
/// The first bad or repeated name.
pub fn check_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for name in names {
        if !valid_name(name) {
            return Err(format!(
                "{name:?} cannot be a variable name: use letters, digits and _, not a function \
                 or constant (pi, e, sin, …)"
            ));
        }
        if !seen.insert(name) {
            return Err(format!("variable {name:?} appears twice"));
        }
    }
    Ok(())
}

/// Parses `source` over `names`.
///
/// # Errors
///
/// The parser's message with the expression.
pub fn parse(source: &str, names: &[&str]) -> Result<Expr, String> {
    Expr::parse(source, names).map_err(|e| format!("`{source}`: {e}"))
}

/// Splits a relation at its comparison: `(lhs, op, rhs)`. `≤`, `≥`, `<=`,
/// `>=`, `=` and `==` are understood.
///
/// # Errors
///
/// When there is no comparison or more than one.
pub fn split_relation(source: &str) -> Result<(String, ConstraintOp, String), String> {
    const OPS: [(&str, ConstraintOp); 6] = [
        ("<=", ConstraintOp::Le),
        (">=", ConstraintOp::Ge),
        ("==", ConstraintOp::Eq),
        ("≤", ConstraintOp::Le),
        ("≥", ConstraintOp::Ge),
        ("=", ConstraintOp::Eq),
    ];
    for (symbol, op) in OPS {
        if let Some((lhs, rhs)) = source.split_once(symbol) {
            let rest_has_op = ["<", ">", "=", "≤", "≥"].iter().any(|s| rhs.contains(s))
                || lhs.contains('<')
                || lhs.contains('>');
            if rest_has_op {
                return Err(format!("`{source}` has more than one comparison"));
            }
            return Ok((lhs.trim().to_owned(), op, rhs.trim().to_owned()));
        }
    }
    if source.contains('<') || source.contains('>') {
        return Err(format!(
            "`{source}`: strict < and > are not supported; use <= or >="
        ));
    }
    Err(format!("`{source}` has no comparison (<=, >= or =)"))
}

/// Coefficients `a` and constant `c` with `lhs − rhs = a·v + c`, checked
/// to be linear by evaluating at two more points.
///
/// # Errors
///
/// A parse error, or when the difference is not linear in the variables.
pub fn linear(lhs: &str, rhs: &str, names: &[&str]) -> Result<(Vec<f64>, f64), String> {
    let (l, r) = (parse(lhs, names)?, parse(rhs, names)?);
    let g = |v: &[f64]| l.eval(v) - r.eval(v);
    let n = names.len();
    let zero = vec![0.0; n];
    let c = g(&zero);
    let mut a = Vec::with_capacity(n);
    for j in 0..n {
        let mut e = zero.clone();
        e[j] = 1.0;
        a.push(g(&e) - c);
    }
    let checks = [
        (0..n).map(|j| 1.5 + 0.7 * j as f64).collect::<Vec<f64>>(),
        (0..n).map(|j| -2.3 + 0.37 * (j * j) as f64).collect(),
    ];
    for point in &checks {
        let predicted = c + a.iter().zip(point).map(|(a, v)| a * v).sum::<f64>();
        let actual = g(point);
        let scale = 1.0 + actual.abs().max(predicted.abs());
        if !(actual.is_finite() && (actual - predicted).abs() <= 1e-9 * scale) {
            return Err(format!(
                "`{lhs}` vs `{rhs}` is not linear in the variables (a product, power or \
                 function of a variable)"
            ));
        }
    }
    if !c.is_finite() || a.iter().any(|x| !x.is_finite()) {
        return Err(format!("`{lhs}` vs `{rhs}` is not finite at 0"));
    }
    Ok((a, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relations_split_and_linear_coefficients_are_read() {
        let (l, op, r) = split_relation("2x + 3*y <= 120").unwrap();
        assert_eq!(
            (l.as_str(), op, r.as_str()),
            ("2x + 3*y", ConstraintOp::Le, "120")
        );
        assert!(split_relation("x < 3").is_err());
        assert!(split_relation("x + y").is_err());
        assert!(split_relation("1 <= x <= 3").is_err());
        let (a, c) = linear("2x + 3*y", "120 - y", &["x", "y"]).unwrap();
        assert_eq!(a, [2.0, 4.0]);
        assert_eq!(c, -120.0);
        assert!(linear("x*y", "1", &["x", "y"]).is_err());
        assert!(linear("x^2", "0", &["x"]).is_err());
        assert!(check_names(["x", "pi"]).is_err());
        assert!(check_names(["a", "a"]).is_err());
        assert!(check_names(["chairs", "x_2"]).is_ok());
    }
}
