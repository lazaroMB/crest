//! The equation-solver card: one equation or a square system, written as
//! expressions of the plot parser.
//!
//! - One unknown with a `range`: `lhs − rhs` is sampled on a grid of
//!   [`SCAN_POINTS`] points; every sign change (and every grid point where
//!   it is exactly 0) is refined by bisection to the tolerance, so all
//!   roots that cross zero are found (a double root that only touches
//!   zero is not, and the card says so).
//! - Otherwise damped Newton from the guesses: the Jacobian by central
//!   differences, steps halved until the residual falls, Gaussian
//!   elimination with partial pivoting for the step.
//!
//! The card checks the answer by plugging it back into every equation
//! (both sides and their difference), and gives the condition number of
//! the Jacobian at the solution, from its singular values.

use application::ports::compute::{ComputeError, SolveRequest};
use domain::computed::{
    Assumption, AssumptionStatus, Computed, ComputedBody, Curve, EquationsBody, Narrative,
    PlugBack, SolveMethod, SolvedUnknown, UncertaintyKind, WarningSeverity,
};
use domain::plot::expr::Expr;
use serde_json::Value;

use crate::data::key_part;
use crate::envelope::{self, count, significant, warning};
use crate::exprs::{check_names, parse};
use crate::linalg::singular_values;

/// Grid points of a bracket scan.
pub const SCAN_POINTS: usize = 4000;
/// Points of the curve drawn for one unknown.
const CURVE_POINTS: usize = 400;
/// Most equations.
pub const MAX_EQUATIONS: usize = 50;
/// Most roots reported by a scan.
const MAX_ROOTS: usize = 100;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// One equation: both sides parsed.
struct Equation {
    text: String,
    lhs: Expr,
    rhs: Expr,
}

impl Equation {
    fn residual(&self, v: &[f64]) -> f64 {
        self.lhs.eval(v) - self.rhs.eval(v)
    }
}

fn parse_equation(text: &str, names: &[&str]) -> Result<Equation, String> {
    let (lhs, rhs) = match text.split_once("==").or_else(|| text.split_once('=')) {
        Some((l, r)) => (l.trim(), r.trim()),
        None => (text.trim(), "0"),
    };
    if rhs.contains('=') {
        return Err(format!("`{text}` has more than one `=`"));
    }
    Ok(Equation {
        text: text.to_owned(),
        lhs: parse(lhs, names)?,
        rhs: parse(rhs, names)?,
    })
}

fn max_abs(v: &[f64]) -> f64 {
    v.iter().fold(0.0_f64, |m, x| m.max(x.abs()))
}

/// Solves `a·x = b` by Gaussian elimination with partial pivoting; `None`
/// when a pivot vanishes.
pub fn gauss(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (top, rest) = a.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for (offset, row) in rest.iter_mut().enumerate() {
            let f = row[col] / pivot_row[col];
            for (cell, p) in row.iter_mut().zip(pivot_row).skip(col) {
                *cell -= f * p;
            }
            b[col + 1 + offset] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The Jacobian of `f` at `x` by central differences (rows = equations).
fn jacobian(f: &dyn Fn(&[f64]) -> Vec<f64>, x: &[f64]) -> Vec<Vec<f64>> {
    let n = x.len();
    let mut columns = Vec::with_capacity(n);
    for j in 0..n {
        let h = 1e-7 * (1.0 + x[j].abs());
        let (mut up, mut down) = (x.to_vec(), x.to_vec());
        up[j] += h;
        down[j] -= h;
        let (fu, fd) = (f(&up), f(&down));
        columns.push(
            fu.iter()
                .zip(&fd)
                .map(|(a, b)| (a - b) / (2.0 * h))
                .collect::<Vec<f64>>(),
        );
    }
    let m = columns.first().map_or(0, Vec::len);
    (0..m)
        .map(|i| columns.iter().map(|c| c[i]).collect())
        .collect()
}

/// Damped Newton: (solution, iterations, converged).
fn newton(
    f: &dyn Fn(&[f64]) -> Vec<f64>,
    start: Vec<f64>,
    tol: f64,
    max_iterations: u32,
) -> (Vec<f64>, u32, bool) {
    let mut x = start;
    let mut fx = f(&x);
    let mut norm = max_abs(&fx);
    for it in 0..max_iterations {
        if norm <= tol {
            return (x, it, true);
        }
        let j = jacobian(f, &x);
        let Some(step) = gauss(j, fx.iter().map(|v| -v).collect()) else {
            return (x, it, false);
        };
        let mut t = 1.0;
        let mut improved = false;
        for _ in 0..40 {
            let trial: Vec<f64> = x.iter().zip(&step).map(|(a, s)| a + t * s).collect();
            let ft = f(&trial);
            let nt = max_abs(&ft);
            if nt.is_finite() && nt < norm {
                x = trial;
                fx = ft;
                norm = nt;
                improved = true;
                break;
            }
            t *= 0.5;
        }
        if !improved {
            return (x, it + 1, norm <= tol);
        }
    }
    let converged = norm <= tol;
    (x, max_iterations, converged)
}

/// Bisection of `g` on `[a, b]` with a sign change, to |g| ≤ `tol` or an
/// interval at machine precision: (root, halvings).
fn bisect(g: &dyn Fn(f64) -> f64, mut a: f64, mut b: f64, tol: f64) -> (f64, u32) {
    let mut ga = g(a);
    let mut steps = 0;
    loop {
        let m = 0.5 * (a + b);
        let gm = g(m);
        steps += 1;
        if gm == 0.0
            || gm.abs() <= tol * 1e-3
            || (b - a).abs() <= 4.0 * f64::EPSILON * m.abs().max(1.0)
            || steps >= 200
        {
            return (m, steps);
        }
        if (gm > 0.0) == (ga > 0.0) {
            a = m;
            ga = gm;
        } else {
            b = m;
        }
    }
}

/// Every root of `g` crossing zero in `[lo, hi]`, with the halvings used.
fn scan(g: &dyn Fn(f64) -> f64, lo: f64, hi: f64, tol: f64) -> (Vec<f64>, u32) {
    let step = (hi - lo) / SCAN_POINTS as f64;
    let mut roots: Vec<f64> = Vec::new();
    let mut steps = 0;
    let mut prev = (lo, g(lo));
    if prev.1 == 0.0 {
        roots.push(lo);
    }
    for i in 1..=SCAN_POINTS {
        let x = lo + step * i as f64;
        let gx = g(x);
        if gx == 0.0 {
            roots.push(x);
        } else if prev.1.is_finite()
            && gx.is_finite()
            && prev.1 != 0.0
            && (prev.1 > 0.0) != (gx > 0.0)
        {
            let (root, used) = bisect(g, prev.0, x, tol);
            steps += used;
            // A pole flips the sign too: keep only true zeros.
            if g(root).abs() <= tol.max(1e-9 * (1.0 + root.abs())) {
                roots.push(root);
            }
        }
        if roots.len() >= MAX_ROOTS {
            break;
        }
        prev = (x, gx);
    }
    (roots, steps)
}

/// Runs a solve request.
pub fn run(request: &SolveRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    let n = request.unknowns.len();
    if n == 0 || request.equations.is_empty() {
        return Err(invalid("give the equations and their unknowns"));
    }
    if request.equations.len() != n {
        return Err(invalid(format!(
            "{} equations in {n} unknowns: the solver needs as many equations as unknowns",
            request.equations.len()
        )));
    }
    if n > MAX_EQUATIONS {
        return Err(invalid(format!("at most {MAX_EQUATIONS} equations")));
    }
    if !(request.tolerance > 0.0 && request.tolerance.is_finite()) {
        return Err(invalid("`tolerance` must be a positive number"));
    }
    let names: Vec<&str> = request.unknowns.iter().map(|u| u.name.as_str()).collect();
    check_names(names.iter().copied()).map_err(invalid)?;
    let equations: Vec<Equation> = request
        .equations
        .iter()
        .map(|e| parse_equation(e, &names))
        .collect::<Result<_, _>>()
        .map_err(invalid)?;
    let f = |v: &[f64]| -> Vec<f64> { equations.iter().map(|e| e.residual(v)).collect() };
    let tol = request.tolerance;
    let mut engine = envelope::solve_engine();
    engine.backend = Some("crest numeric (Newton, bisection)".into());
    let mut computed = Computed::new(id, "solve.equations", "Equation solver", engine);
    computed.uncertainty_kind = Some(UncertaintyKind::Exact);
    computed.inputs.push(domain::computed::InputRef {
        name: "equations".into(),
        role: Some("param".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: Some(n as u64),
        columns: names.iter().map(|n| (*n).to_owned()).collect(),
        filter: None,
    });
    let (method, solution, roots, iterations, converged, curve) = match request.range {
        Some([lo, hi]) if n == 1 => {
            if !(lo.is_finite() && hi.is_finite() && lo < hi) {
                return Err(invalid("`range` must be [low, high] with low < high"));
            }
            let g = |x: f64| equations[0].residual(&[x]);
            let (roots, steps) = scan(&g, lo, hi, tol);
            let step = (hi - lo) / (CURVE_POINTS - 1) as f64;
            let xs: Vec<f64> = (0..CURVE_POINTS).map(|i| lo + step * i as f64).collect();
            let (cx, cy): (Vec<f64>, Vec<f64>) = xs
                .iter()
                .map(|&x| (x, g(x)))
                .filter(|(_, y)| y.is_finite())
                .unzip();
            let first = roots.first().copied();
            let converged = first.is_some();
            (
                SolveMethod::BracketScan,
                vec![first.unwrap_or(f64::NAN)],
                roots,
                steps,
                converged,
                Some(Curve { x: cx, y: cy }),
            )
        }
        Some(_) => return Err(invalid("`range` is for a single unknown")),
        None => {
            let start: Vec<f64> = request
                .unknowns
                .iter()
                .map(|u| u.guess.unwrap_or(1.0))
                .collect();
            let (x, it, ok) = newton(&f, start, tol, request.max_iterations);
            let curve = (n == 1).then(|| {
                let span = 2.0 * (1.0 + x[0].abs());
                let step = 2.0 * span / (CURVE_POINTS - 1) as f64;
                let (cx, cy) = (0..CURVE_POINTS)
                    .map(|i| {
                        let v = x[0] - span + step * i as f64;
                        (v, equations[0].residual(&[v]))
                    })
                    .filter(|(_, y)| y.is_finite())
                    .unzip();
                Curve { x: cx, y: cy }
            });
            (SolveMethod::Newton, x, Vec::new(), it, ok, curve)
        }
    };
    let finite = solution.iter().all(|v| v.is_finite());
    let plug_back: Vec<PlugBack> = if finite {
        equations
            .iter()
            .map(|e| {
                let (l, r) = (e.lhs.eval(&solution), e.rhs.eval(&solution));
                PlugBack {
                    equation: e.text.clone(),
                    lhs: l,
                    rhs: r,
                    residual: l - r,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let residual_norm = max_abs(&plug_back.iter().map(|p| p.residual).collect::<Vec<f64>>());
    let condition = (method == SolveMethod::Newton && finite).then(|| {
        let j = jacobian(&f, &solution);
        let columns: Vec<Vec<f64>> = (0..n)
            .map(|c| j.iter().map(|row| row[c]).collect())
            .collect();
        let s = singular_values(&columns);
        match (s.first(), s.last()) {
            (Some(&hi), Some(&lo)) if lo > 0.0 => hi / lo,
            _ => f64::INFINITY,
        }
    });
    let values = &mut computed.values;
    if finite {
        for (name, x) in names.iter().zip(&solution) {
            values.insert(key_part(name), significant(*x, 10));
        }
    }
    for (i, r) in roots.iter().enumerate() {
        values.insert(format!("root_{}", i + 1), significant(*r, 10));
    }
    if method == SolveMethod::BracketScan {
        values.insert("roots".into(), count(roots.len() as u64));
    }
    if finite {
        values.insert("residual_norm".into(), significant(residual_norm, 3));
    }
    values.insert("iterations".into(), count(u64::from(iterations)));
    if let Some(c) = condition.filter(|c| c.is_finite()) {
        values.insert("condition_number".into(), significant(c, 3));
    }
    let params = &mut computed.method.params;
    params.insert("tolerance".into(), Value::from(tol));
    params.insert("method".into(), Value::from(method.label()));
    computed.method.name = format!("Equation solver: {}", method.label());
    computed.method.why = Some(match method {
        SolveMethod::BracketScan => format!(
            "one unknown with a range: every sign change on {SCAN_POINTS} grid points, refined"
        ),
        SolveMethod::Newton => "Newton from the guesses (no range given)".into(),
    });
    if !converged {
        let why = match method {
            SolveMethod::BracketScan => {
                "no root crosses zero in the range (a double root that only touches zero is not \
                 found; widen the range or use a guess)"
                    .to_owned()
            }
            SolveMethod::Newton => format!(
                "Newton did not reach |residual| ≤ {tol:e} in {iterations} iterations; try other \
                 guesses"
            ),
        };
        computed
            .warnings
            .push(warning("not_converged", WarningSeverity::Error, why));
    }
    if condition.is_some_and(|c| !c.is_finite() || c > 1e10) {
        computed.warnings.push(warning(
            "ill_conditioned",
            WarningSeverity::Warn,
            "the Jacobian at the solution is (nearly) singular: the solution may not be \
             unique and small changes move it a lot",
        ));
    }
    if method == SolveMethod::Newton {
        computed.assumptions.push(Assumption {
            id: "unique_root".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some("Newton finds the root nearest the guesses; other roots may exist".into()),
        });
    }
    if roots.len() >= MAX_ROOTS {
        computed.warnings.push(warning(
            "many_roots",
            WarningSeverity::Info,
            format!("only the first {MAX_ROOTS} roots are listed"),
        ));
    }
    computed.reproduce.python = Some(match (method, request.range) {
        (SolveMethod::BracketScan, Some([lo, hi])) => format!(
            "import numpy as np, scipy.optimize as so; f = lambda {v}: ({e}); xs = np.linspace({lo}, \
             {hi}, {SCAN_POINTS} + 1); [so.brentq(f, a, b) for a, b in zip(xs, xs[1:]) if f(a) * f(b) < 0]",
            v = names[0],
            e = py_residual(&request.equations[0])
        ),
        _ => format!(
            "import scipy.optimize as so; f = lambda v: [{}]; so.root(f, [{}])",
            request
                .equations
                .iter()
                .map(|e| unpack(&py_residual(e), &names))
                .collect::<Vec<_>>()
                .join(", "),
            request
                .unknowns
                .iter()
                .map(|u| u.guess.unwrap_or(1.0).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    });
    computed.narrative = Some(Narrative {
        template: "equations".into(),
        text_slots: vec!["residual_norm".into()],
    });
    let body = EquationsBody {
        method,
        converged,
        unknowns: names
            .iter()
            .zip(&solution)
            .map(|(n, v)| SolvedUnknown {
                name: (*n).to_owned(),
                value: *v,
            })
            .collect(),
        roots,
        plug_back,
        residual_norm,
        iterations,
        tolerance: tol,
        condition_number: condition.filter(|c| c.is_finite()),
        curve,
    };
    if !finite {
        envelope::fail(&mut computed, "the solver found no finite solution");
    }
    Ok((computed, ComputedBody::Equations(body)))
}

/// `lhs - (rhs)` in Python syntax.
fn py_residual(equation: &str) -> String {
    let e = equation.replace('^', "**");
    match e.split_once("==").or_else(|| e.split_once('=')) {
        Some((l, r)) => format!("({}) - ({})", l.trim(), r.trim()),
        None => e,
    }
}

/// Replaces each unknown by `v[i]` for a vector-valued lambda.
fn unpack(expr: &str, names: &[&str]) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        match names.iter().position(|n| *n == word.as_str()) {
            Some(i) => out.push_str(&format!("v[{i}]")),
            None => out.push_str(word),
        }
        word.clear();
    };
    for c in expr.chars() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauss_and_unpack() {
        let x = gauss(vec![vec![2.0, 1.0], vec![1.0, 3.0]], vec![3.0, 5.0]).unwrap();
        assert!((x[0] - 0.8).abs() < 1e-12 && (x[1] - 1.4).abs() < 1e-12);
        assert!(gauss(vec![vec![1.0, 2.0], vec![2.0, 4.0]], vec![1.0, 2.0]).is_none());
        assert_eq!(
            unpack("x**2 + y1 - x", &["x", "y1"]),
            "v[0]**2 + v[1] - v[0]"
        );
        assert_eq!(py_residual("x^2 = 2"), "(x**2) - (2)");
    }
}
