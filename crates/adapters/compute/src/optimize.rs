//! The LP/MILP card on `microlp` (pure Rust simplex with branch and
//! bound).
//!
//! The objective and constraints are expressions of the plot parser,
//! checked to be linear and read as coefficients. microlp returns the
//! primal solution only, so the duals come from solving the dual program
//! with microlp as well: with every finite variable bound written as a
//! row, the dual of `min cᵀx, Ax (≤ ≥ =) b` is `max bᵀy, Aᵀy = c` with
//! `y ≥ 0` on `≥` rows and `y ≤ 0` on `≤` rows. Its optimum equals the
//! primal one (strong duality, checked); the duals of the bound rows are
//! the reduced costs. Shadow prices are reported in the user's direction:
//! the change of the objective per unit increase of a right-hand side. A
//! MILP gets the duals of the LP with its integers fixed at the solution,
//! and says so. Ranging (how far a coefficient may move before the basis
//! changes) needs the basis, which microlp does not expose: not reported.
//!
//! The branch and bound has a node limit instead of a time limit, so the
//! same problem always gives the same answer and a recompute matches.

use application::ports::compute::{ComputeError, OptimizeRequest};
use domain::computed::{
    Assumption, AssumptionStatus, Computed, ComputedBody, ConstraintOp, Narrative, OptConstraint,
    OptVariable, OptimizationBody, OptimizationStatus, Sense, UncertaintyKind, WarningSeverity,
    apa,
};
use microlp::{ComparisonOp, OptimizationDirection, Problem, SolutionStatus, SolveOptions};
use serde_json::Value;

use crate::data::key_part;
use crate::envelope::{self, number, warning};
use crate::exprs::{check_names, linear, split_relation};

/// Backend recorded in the envelope.
pub const MICROLP: &str = "microlp 0.6.0";
/// Most variables.
pub const MAX_VARIABLES: usize = 2000;
/// Most constraints.
pub const MAX_CONSTRAINTS: usize = 5000;
/// Branch-and-bound node budget of a MILP.
pub const NODE_LIMIT: u64 = 200_000;
/// A constraint binds when its slack is below this, relative to its size.
const BINDING_TOL: f64 = 1e-7;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// A row of the problem in minimisation form: `a·x op b`.
#[derive(Debug, Clone)]
struct Row {
    coefficients: Vec<f64>,
    op: ConstraintOp,
    rhs: f64,
}

fn comparison(op: ConstraintOp) -> ComparisonOp {
    match op {
        ConstraintOp::Le => ComparisonOp::Le,
        ConstraintOp::Ge => ComparisonOp::Ge,
        ConstraintOp::Eq => ComparisonOp::Eq,
    }
}

/// A primal solve: status, objective (user direction), values.
struct Primal {
    status: OptimizationStatus,
    objective: Option<f64>,
    values: Vec<f64>,
    gap: Option<f64>,
}

/// Solves the primal with bounds and integrality as given.
fn solve_primal(
    sense: Sense,
    c: &[f64],
    bounds: &[(f64, f64, bool)],
    rows: &[Row],
) -> Result<Primal, String> {
    let direction = match sense {
        Sense::Maximize => OptimizationDirection::Maximize,
        Sense::Minimize => OptimizationDirection::Minimize,
    };
    let mut problem = Problem::new(direction);
    let vars: Vec<_> = c
        .iter()
        .zip(bounds)
        .map(|(&coef, &(lo, hi, integer))| {
            if integer {
                let clamp = |v: f64| {
                    // Bounds clamped to i32 before the cast; ±∞ become the ends.
                    #[allow(clippy::cast_possible_truncation)]
                    let i = v.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
                    i
                };
                problem.add_integer_var(coef, (clamp(lo.ceil()), clamp(hi.floor())))
            } else {
                problem.add_var(coef, (lo, hi))
            }
        })
        .collect();
    for row in rows {
        let terms: Vec<(microlp::Variable, f64)> = row
            .coefficients
            .iter()
            .enumerate()
            .filter(|(_, a)| **a != 0.0)
            .map(|(j, a)| (vars[j], *a))
            .collect();
        problem.add_constraint(terms.as_slice(), comparison(row.op), row.rhs);
    }
    let mut options = SolveOptions::default();
    options.node_limit = Some(NODE_LIMIT);
    let outcome = match problem.solve_with(options) {
        Ok(outcome) => outcome,
        Err(microlp::Error::Infeasible) => {
            return Ok(Primal {
                status: OptimizationStatus::Infeasible,
                objective: None,
                values: Vec::new(),
                gap: None,
            });
        }
        Err(microlp::Error::Unbounded) => {
            return Ok(Primal {
                status: OptimizationStatus::Unbounded,
                objective: None,
                values: Vec::new(),
                gap: None,
            });
        }
        Err(e) => return Err(e.to_string()),
    };
    let Some(solution) = outcome.solution() else {
        return Ok(Primal {
            status: OptimizationStatus::Stopped,
            objective: None,
            values: Vec::new(),
            gap: None,
        });
    };
    let values = vars
        .iter()
        .zip(bounds)
        .map(|(&v, &(_, _, integer))| {
            let raw = solution.var_value_raw(v);
            if integer { raw.round() } else { raw }
        })
        .collect();
    Ok(Primal {
        status: match solution.status() {
            SolutionStatus::Optimal => OptimizationStatus::Optimal,
            SolutionStatus::Feasible => OptimizationStatus::Feasible,
        },
        objective: Some(solution.objective()),
        values,
        gap: solution.gap(),
    })
}

/// Duals of the LP (no integers) `sense cᵀx` with `bounds` and `rows`:
/// shadow prices of the rows and reduced costs of the variables, in the
/// user's direction, and the dual objective. `None` when the dual solve
/// fails.
fn duals(
    sense: Sense,
    c: &[f64],
    bounds: &[(f64, f64, bool)],
    rows: &[Row],
) -> Option<(Vec<f64>, Vec<f64>, f64)> {
    let n = c.len();
    // Minimisation form.
    let sign = if sense == Sense::Maximize { -1.0 } else { 1.0 };
    let c_min: Vec<f64> = c.iter().map(|x| sign * x).collect();
    // Every row, user rows first, then the finite bounds.
    let mut all: Vec<Row> = rows.to_vec();
    let mut bound_rows: Vec<(usize, usize)> = Vec::new();
    for (j, &(lo, hi, _)) in bounds.iter().enumerate() {
        let unit = |j: usize| {
            let mut a = vec![0.0; n];
            a[j] = 1.0;
            a
        };
        if lo.is_finite() && hi.is_finite() && (lo - hi).abs() < f64::EPSILON {
            bound_rows.push((j, all.len()));
            all.push(Row {
                coefficients: unit(j),
                op: ConstraintOp::Eq,
                rhs: lo,
            });
            continue;
        }
        if lo.is_finite() {
            bound_rows.push((j, all.len()));
            all.push(Row {
                coefficients: unit(j),
                op: ConstraintOp::Ge,
                rhs: lo,
            });
        }
        if hi.is_finite() {
            bound_rows.push((j, all.len()));
            all.push(Row {
                coefficients: unit(j),
                op: ConstraintOp::Le,
                rhs: hi,
            });
        }
    }
    let mut dual = Problem::new(OptimizationDirection::Maximize);
    let ys: Vec<_> = all
        .iter()
        .map(|row| {
            let range = match row.op {
                ConstraintOp::Ge => (0.0, f64::INFINITY),
                ConstraintOp::Le => (f64::NEG_INFINITY, 0.0),
                ConstraintOp::Eq => (f64::NEG_INFINITY, f64::INFINITY),
            };
            dual.add_var(row.rhs, range)
        })
        .collect();
    for (j, &cj) in c_min.iter().enumerate() {
        let terms: Vec<(microlp::Variable, f64)> = all
            .iter()
            .enumerate()
            .filter(|(_, row)| row.coefficients[j] != 0.0)
            .map(|(r, row)| (ys[r], row.coefficients[j]))
            .collect();
        if terms.is_empty() {
            if cj != 0.0 {
                return None;
            }
            continue;
        }
        dual.add_constraint(terms.as_slice(), ComparisonOp::Eq, cj);
    }
    let outcome = dual.solve().ok()?;
    let solution = outcome.solution()?;
    let y: Vec<f64> = ys.iter().map(|&v| solution.var_value_raw(v)).collect();
    let prices: Vec<f64> = (0..rows.len()).map(|r| sign * y[r]).collect();
    let mut reduced = vec![0.0; n];
    for &(j, r) in &bound_rows {
        reduced[j] += sign * y[r];
    }
    Some((prices, reduced, sign * solution.objective()))
}

/// Runs an optimisation request.
pub fn run(
    request: &OptimizeRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    if request.variables.is_empty() {
        return Err(invalid("give at least one variable"));
    }
    if request.variables.len() > MAX_VARIABLES || request.constraints.len() > MAX_CONSTRAINTS {
        return Err(invalid(format!(
            "at most {MAX_VARIABLES} variables and {MAX_CONSTRAINTS} constraints"
        )));
    }
    let names: Vec<&str> = request.variables.iter().map(|v| v.name.as_str()).collect();
    check_names(names.iter().copied()).map_err(invalid)?;
    let (c, constant) = linear(&request.objective, "0", &names).map_err(invalid)?;
    let mut bounds = Vec::with_capacity(names.len());
    for v in &request.variables {
        let (lo, hi) = if v.binary {
            (0.0, 1.0)
        } else {
            (
                v.lower.unwrap_or(f64::NEG_INFINITY),
                v.upper.unwrap_or(f64::INFINITY),
            )
        };
        if lo.is_nan() || hi.is_nan() || lo > hi {
            return Err(invalid(format!("variable {} has lower > upper", v.name)));
        }
        bounds.push((lo, hi, v.integer || v.binary));
    }
    let mut rows = Vec::with_capacity(request.constraints.len());
    let mut constraint_names = Vec::with_capacity(request.constraints.len());
    for (i, constraint) in request.constraints.iter().enumerate() {
        let name = constraint
            .name
            .clone()
            .unwrap_or_else(|| format!("c{}", i + 1));
        let (lhs, op, rhs) = split_relation(&constraint.expr).map_err(invalid)?;
        let (a, k) = linear(&lhs, &rhs, &names).map_err(|e| invalid(format!("{name}: {e}")))?;
        if a.iter().all(|x| *x == 0.0) {
            return Err(invalid(format!(
                "{name}: `{}` has no variable in it",
                constraint.expr
            )));
        }
        rows.push(Row {
            coefficients: a,
            op,
            rhs: -k,
        });
        constraint_names.push(name);
    }
    let integer = bounds.iter().any(|b| b.2);
    let mut engine = envelope::solve_engine();
    engine.backend = Some(MICROLP.into());
    let kind = if integer { "solve.milp" } else { "solve.lp" };
    let name = if integer {
        "Mixed-integer linear program"
    } else {
        "Linear program"
    };
    let mut computed = Computed::new(id, kind, name, engine);
    computed.uncertainty_kind = Some(UncertaintyKind::Exact);
    computed.inputs.push(domain::computed::InputRef {
        name: "model".into(),
        role: Some("param".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: Some(request.constraints.len() as u64),
        columns: names.iter().map(|n| (*n).to_owned()).collect(),
        filter: None,
    });
    let primal = solve_primal(request.sense, &c, &bounds, &rows)
        .map_err(|e| invalid(format!("the solver failed: {e}")))?;
    let solved = matches!(
        primal.status,
        OptimizationStatus::Optimal | OptimizationStatus::Feasible
    );
    // Duals: of the LP itself, or of the MILP with integers fixed.
    let dual_bounds: Vec<(f64, f64, bool)> = if integer && solved {
        bounds
            .iter()
            .zip(&primal.values)
            .map(|(&(lo, hi, int), &x)| if int { (x, x, false) } else { (lo, hi, false) })
            .collect()
    } else {
        bounds.iter().map(|&(lo, hi, _)| (lo, hi, false)).collect()
    };
    let dual = if solved {
        duals(request.sense, &c, &dual_bounds, &rows)
    } else {
        None
    };
    let objective = primal.objective.map(|o| o + constant);
    let variables: Vec<OptVariable> = request
        .variables
        .iter()
        .enumerate()
        .map(|(j, v)| OptVariable {
            name: v.name.clone(),
            value: primal.values.get(j).copied(),
            lower: bounds[j].0.is_finite().then_some(bounds[j].0),
            upper: bounds[j].1.is_finite().then_some(bounds[j].1),
            integer: bounds[j].2,
            objective_coefficient: c[j],
            reduced_cost: dual.as_ref().map(|d| d.1[j]),
        })
        .collect();
    let constraints: Vec<OptConstraint> = rows
        .iter()
        .zip(&constraint_names)
        .zip(&request.constraints)
        .enumerate()
        .map(|(i, ((row, name), given))| {
            let activity = solved.then(|| {
                row.coefficients
                    .iter()
                    .zip(&primal.values)
                    .map(|(a, x)| a * x)
                    .sum::<f64>()
            });
            let slack = activity.map(|act| match row.op {
                ConstraintOp::Le => row.rhs - act,
                ConstraintOp::Ge => act - row.rhs,
                ConstraintOp::Eq => (row.rhs - act).abs(),
            });
            let binding = slack.is_some_and(|s| s.abs() <= BINDING_TOL * (1.0 + row.rhs.abs()));
            OptConstraint {
                name: name.clone(),
                expr: given.expr.clone(),
                coefficients: row.coefficients.clone(),
                op: row.op,
                rhs: row.rhs,
                activity,
                slack,
                binding,
                dual: dual.as_ref().map(|d| d.0[i]),
            }
        })
        .collect();
    if let (Some(obj), Some((_, _, dual_obj))) = (primal.objective, &dual)
        && !integer
        && (obj - dual_obj).abs() > 1e-6 * (1.0 + obj.abs())
    {
        computed.warnings.push(warning(
            "duality_gap",
            WarningSeverity::Warn,
            format!(
                "the dual optimum {} differs from the primal {}: the duals may be inaccurate",
                apa::significant(*dual_obj, 6),
                apa::significant(obj, 6)
            ),
        ));
    }
    let values = &mut computed.values;
    if let Some(obj) = objective {
        values.insert("objective".into(), number(obj, 4));
    }
    for v in &variables {
        if let Some(x) = v.value {
            let decimals = if v.integer { 0 } else { 4 };
            values.insert(format!("value.{}", key_part(&v.name)), number(x, decimals));
        }
        if let Some(rc) = v.reduced_cost {
            values.insert(format!("reduced_cost.{}", key_part(&v.name)), number(rc, 4));
        }
    }
    for k in &constraints {
        if let Some(s) = k.slack {
            values.insert(format!("slack.{}", key_part(&k.name)), number(s, 4));
        }
        if let Some(d) = k.dual {
            values.insert(format!("dual.{}", key_part(&k.name)), number(d, 4));
        }
    }
    if let Some(gap) = primal.gap.filter(|_| integer) {
        values.insert("mip_gap".into(), number(gap, 6));
    }
    let params = &mut computed.method.params;
    params.insert("status".into(), Value::from(primal.status.label()));
    params.insert(
        "sense".into(),
        Value::from(match request.sense {
            Sense::Maximize => "maximize",
            Sense::Minimize => "minimize",
        }),
    );
    params.insert(
        "ranging".into(),
        Value::from("not available: microlp exposes no basis"),
    );
    if integer {
        params.insert("node_limit".into(), Value::from(NODE_LIMIT));
        params.insert(
            "duals".into(),
            Value::from("of the LP with the integer variables fixed at the solution"),
        );
    }
    computed.method.why = Some(if integer {
        "branch and bound on the LP relaxation (simplex)".into()
    } else {
        "simplex; duals from the dual program".into()
    });
    match primal.status {
        OptimizationStatus::Infeasible => computed.warnings.push(warning(
            "infeasible",
            WarningSeverity::Error,
            "no point satisfies every constraint and bound; relax a constraint (the solver gives no \
             irreducible infeasible subset)",
        )),
        OptimizationStatus::Unbounded => computed.warnings.push(warning(
            "unbounded",
            WarningSeverity::Error,
            "the objective improves without limit: a bound or constraint is missing",
        )),
        OptimizationStatus::Stopped | OptimizationStatus::Feasible => {
            computed.warnings.push(warning(
                "node_limit",
                WarningSeverity::Warn,
                format!(
                    "branch and bound stopped at {NODE_LIMIT} nodes: the solution is not proven \
                     optimal"
                ),
            ));
        }
        OptimizationStatus::Optimal => {}
    }
    if solved {
        // Degenerate: more binding rows (with active bounds) than variables.
        let active_bounds = variables
            .iter()
            .filter(|v| {
                let at = |b: Option<f64>| {
                    b.zip(v.value)
                        .is_some_and(|(b, x)| (b - x).abs() <= BINDING_TOL * (1.0 + b.abs()))
                };
                at(v.lower) || at(v.upper)
            })
            .count();
        let binding = constraints.iter().filter(|k| k.binding).count();
        if binding + active_bounds > variables.len() {
            computed.assumptions.push(Assumption {
                id: "nondegenerate".into(),
                check: Some("active constraints ≤ variables".into()),
                #[allow(clippy::cast_precision_loss)]
                stat: Some((binding + active_bounds) as f64),
                p: None,
                status: AssumptionStatus::Warn,
                consequence: Some(
                    "degenerate optimum: the duals are one of several valid sets".into(),
                ),
                note: None,
            });
        }
    }
    reproduce(&mut computed, request);
    computed.narrative = Some(Narrative {
        template: "optimization".into(),
        text_slots: vec!["objective".into()],
    });
    computed.method.alternatives = Vec::new();
    let body = OptimizationBody {
        sense: request.sense,
        objective_expr: request.objective.clone(),
        status: primal.status,
        objective,
        integer,
        mip_gap: primal.gap.filter(|_| integer),
        variables,
        constraints,
    };
    Ok((computed, ComputedBody::Optimization(body)))
}

fn reproduce(computed: &mut Computed, request: &OptimizeRequest) {
    let mut lines = vec![format!(
        "import pulp; m = pulp.LpProblem('m', pulp.{})",
        match request.sense {
            Sense::Maximize => "LpMaximize",
            Sense::Minimize => "LpMinimize",
        }
    )];
    for v in &request.variables {
        let category = if v.binary {
            "Binary"
        } else if v.integer {
            "Integer"
        } else {
            "Continuous"
        };
        let bound = |b: Option<f64>| b.map_or_else(|| "None".to_owned(), |b| format!("{b}"));
        lines.push(format!(
            "{n} = pulp.LpVariable('{n}', {}, {}, '{category}')",
            bound(v.lower),
            bound(v.upper),
            n = v.name
        ));
    }
    lines.push(format!("m += {}", request.objective.replace('^', "**")));
    for (i, k) in request.constraints.iter().enumerate() {
        let name = k.name.clone().unwrap_or_else(|| format!("c{}", i + 1));
        let expr = k.expr.replace("≤", "<=").replace("≥", ">=");
        let expr = if expr.contains("<=") || expr.contains(">=") || expr.contains("==") {
            expr
        } else {
            expr.replace('=', "==")
        };
        lines.push(format!("m += ({}), '{name}'", expr.replace('^', "**")));
    }
    lines.push(
        "m.solve(); [(c, m.constraints[c].pi, m.constraints[c].slack) for c in m.constraints]"
            .into(),
    );
    computed.reproduce.python = Some(lines.join("; "));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duals_of_a_tiny_lp() {
        // max x + y, x ≤ 2, y ≤ 3: duals 1 and 1.
        let rows = vec![
            Row {
                coefficients: vec![1.0, 0.0],
                op: ConstraintOp::Le,
                rhs: 2.0,
            },
            Row {
                coefficients: vec![0.0, 1.0],
                op: ConstraintOp::Le,
                rhs: 3.0,
            },
        ];
        let bounds = [(0.0, f64::INFINITY, false); 2];
        let (prices, reduced, objective) =
            duals(Sense::Maximize, &[1.0, 1.0], &bounds, &rows).unwrap();
        assert!((prices[0] - 1.0).abs() < 1e-9 && (prices[1] - 1.0).abs() < 1e-9);
        assert!(reduced.iter().all(|r| r.abs() < 1e-9));
        assert!((objective - 5.0).abs() < 1e-9);
    }
}
