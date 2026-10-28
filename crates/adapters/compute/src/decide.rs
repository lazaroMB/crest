//! The decision-matrix card: weighted sum of normalised scores, ranks,
//! and the weight sensitivity of first place.
//!
//! Sensitivity moves one criterion's weight w_k and rescales the others in
//! proportion, so the weights still sum to 1. Each option's total is then
//! linear in w_k: T_i(w) = w·s_ik + (1 − w)/(1 − w_k)·R_i with
//! R_i = Σ_{j≠k} w_j·s_ij, so the weight at which another option catches
//! the leader is found exactly. The card reports, for every criterion, the
//! nearest such weight below and above the current one and who takes the
//! lead there; ties are flagged.

use application::ports::compute::{ComputeError, DecideRequest};
use domain::computed::{
    Assumption, AssumptionStatus, Computed, ComputedBody, CriterionDirection, DecisionCriterion,
    DecisionMatrixBody, DecisionOption, Narrative, Normalization, UncertaintyKind, WarningSeverity,
    WeightSensitivity, WeightThreshold, apa,
};
use serde_json::Value;

use crate::data::key_part;
use crate::envelope::{self, bounded, number, warning};

/// Most criteria and options.
pub const MAX_SIZE: usize = 200;
/// Totals closer than this are a tie.
const TIE: f64 = 1e-9;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// Normalised scores of one criterion's column.
fn normalize(
    column: &[f64],
    direction: CriterionDirection,
    how: Normalization,
) -> Result<Vec<f64>, String> {
    let (min, max) = column
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
            (lo.min(*x), hi.max(*x))
        });
    let cost = direction == CriterionDirection::Cost;
    Ok(match how {
        Normalization::MinMax => {
            let range = max - min;
            column
                .iter()
                .map(|x| {
                    if range == 0.0 {
                        1.0
                    } else if cost {
                        (max - x) / range
                    } else {
                        (x - min) / range
                    }
                })
                .collect()
        }
        Normalization::Max => {
            if min <= 0.0 {
                return Err("`max` normalisation needs positive scores".into());
            }
            column
                .iter()
                .map(|x| if cost { min / x } else { x / max })
                .collect()
        }
        Normalization::None => column.iter().map(|x| if cost { -x } else { *x }).collect(),
    })
}

/// Ranks of `totals` (1 = best; ties share the better rank).
fn ranks(totals: &[f64]) -> Vec<u32> {
    totals
        .iter()
        .map(|t| {
            let better = totals.iter().filter(|o| **o > *t + TIE).count();
            u32::try_from(better + 1).unwrap_or(u32::MAX)
        })
        .collect()
}

/// A weight at which option `.1` ties the leader.
type Crossing = Option<(f64, usize)>;

/// For criterion `k`: the nearest weights below and above `w[k]` at which
/// another option ties the leader `lead`.
fn sensitivity(k: usize, w: &[f64], s: &[Vec<f64>], lead: usize) -> (Crossing, Crossing) {
    let wk = w[k];
    if (1.0 - wk).abs() < 1e-12 {
        return (None, None);
    }
    // T_i(x) = a_i + b_i·x on x = the new weight of k.
    let line = |i: usize| {
        let rest: f64 = (0..w.len())
            .filter(|j| *j != k)
            .map(|j| w[j] * s[i][j])
            .sum();
        let scale = rest / (1.0 - wk);
        (scale, s[i][k] - scale)
    };
    let (a0, b0) = line(lead);
    let (mut below, mut above): (Crossing, Crossing) = (None, None);
    for i in (0..s.len()).filter(|i| *i != lead) {
        let (a, b) = line(i);
        let slope = b - b0;
        if slope.abs() < 1e-15 {
            continue;
        }
        let x = (a0 - a) / slope;
        if !(0.0..=1.0).contains(&x) {
            continue;
        }
        if x < wk - 1e-12 && below.is_none_or(|(bx, _)| x > bx) {
            below = Some((x, i));
        } else if x > wk + 1e-12 && above.is_none_or(|(ax, _)| x < ax) {
            above = Some((x, i));
        }
    }
    (below, above)
}

/// Runs a decision request.
pub fn run(request: &DecideRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    let (k, n) = (request.criteria.len(), request.options.len());
    if k == 0 || n < 2 || k > MAX_SIZE || n > MAX_SIZE {
        return Err(invalid(format!(
            "give 1 to {MAX_SIZE} criteria and 2 to {MAX_SIZE} options"
        )));
    }
    if request
        .criteria
        .iter()
        .any(|c| !(c.weight.is_finite() && c.weight >= 0.0))
    {
        return Err(invalid("weights must be non-negative numbers"));
    }
    let weight_sum: f64 = request.criteria.iter().map(|c| c.weight).sum();
    if weight_sum <= 0.0 {
        return Err(invalid("at least one weight must be positive"));
    }
    for o in &request.options {
        if o.scores.len() != k {
            return Err(invalid(format!(
                "option {} has {} scores for {k} criteria",
                o.name,
                o.scores.len()
            )));
        }
        if o.scores.iter().any(|x| !x.is_finite()) {
            return Err(invalid(format!("option {} has a non-finite score", o.name)));
        }
    }
    let w: Vec<f64> = request
        .criteria
        .iter()
        .map(|c| c.weight / weight_sum)
        .collect();
    // Normalised scores, options × criteria.
    let mut s = vec![vec![0.0; k]; n];
    for (j, c) in request.criteria.iter().enumerate() {
        let column: Vec<f64> = request.options.iter().map(|o| o.scores[j]).collect();
        let normalized = normalize(&column, c.direction, request.normalization)
            .map_err(|e| invalid(format!("{}: {e}", c.name)))?;
        for (i, v) in normalized.into_iter().enumerate() {
            s[i][j] = v;
        }
    }
    let totals: Vec<f64> = s
        .iter()
        .map(|row| row.iter().zip(&w).map(|(x, wj)| x * wj).sum())
        .collect();
    let rank = ranks(&totals);
    let lead = (0..n)
        .max_by(|&a, &b| totals[a].total_cmp(&totals[b]).then(b.cmp(&a)))
        .unwrap_or(0);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| totals[b].total_cmp(&totals[a]).then(a.cmp(&b)));
    let second = order.get(1).copied().unwrap_or(lead);
    let names: Vec<&str> = request.options.iter().map(|o| o.name.as_str()).collect();
    let sens: Vec<WeightSensitivity> = request
        .criteria
        .iter()
        .enumerate()
        .map(|(j, c)| {
            let (below, above) = sensitivity(j, &w, &s, lead);
            let threshold = |t: Option<(f64, usize)>| {
                t.map(|(weight, i)| WeightThreshold {
                    weight,
                    new_leader: names[i].to_owned(),
                })
            };
            WeightSensitivity {
                criterion: c.name.clone(),
                weight: w[j],
                lower: threshold(below),
                upper: threshold(above),
            }
        })
        .collect();
    let mut engine = envelope::solve_engine();
    engine.backend = Some("crest weighted sum".into());
    let mut computed = Computed::new(id, "solve.decision_matrix", "Decision matrix", engine);
    computed.uncertainty_kind = Some(UncertaintyKind::Exact);
    computed.inputs.push(domain::computed::InputRef {
        name: "matrix".into(),
        role: Some("data".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: Some(n as u64),
        columns: request.criteria.iter().map(|c| c.name.clone()).collect(),
        filter: None,
    });
    let values = &mut computed.values;
    for (i, name) in names.iter().enumerate() {
        values.insert(format!("total.{}", key_part(name)), number(totals[i], 3));
    }
    for (j, c) in request.criteria.iter().enumerate() {
        values.insert(format!("weight.{}", key_part(&c.name)), bounded(w[j], 3));
    }
    values.insert("margin".into(), number(totals[lead] - totals[second], 3));
    let nearest = sens
        .iter()
        .flat_map(|s| {
            [s.lower.as_ref(), s.upper.as_ref()]
                .into_iter()
                .flatten()
                .map(move |t| ((t.weight - s.weight).abs(), s, t))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((change, _, _)) = nearest {
        values.insert("min_weight_change".into(), bounded(change, 3));
    }
    let params = &mut computed.method.params;
    params.insert(
        "normalization".into(),
        Value::from(request.normalization.label()),
    );
    params.insert("leader".into(), Value::from(names[lead]));
    params.insert("runner_up".into(), Value::from(names[second]));
    if let Some((_, s, t)) = nearest {
        params.insert(
            "most_sensitive".into(),
            Value::from(format!(
                "{} at {} (now {}) gives the lead to {}",
                s.criterion,
                apa::number(t.weight * 100.0, 1),
                apa::number(s.weight * 100.0, 1),
                t.new_leader
            )),
        );
    }
    computed.method.why = Some(format!(
        "weighted sum of {} scores with the weights rescaled to sum 1",
        request.normalization.label()
    ));
    computed.method.alternatives = Vec::new();
    let tied: Vec<&str> = (0..n).filter(|&i| rank[i] == 1).map(|i| names[i]).collect();
    if tied.len() > 1 {
        computed.warnings.push(warning(
            "tie",
            WarningSeverity::Warn,
            format!("first place is a tie between {}", tied.join(" and ")),
        ));
    } else if (totals[lead] - totals[second]).abs() < 0.02 {
        computed.warnings.push(warning(
            "near_tie",
            WarningSeverity::Info,
            format!(
                "{} leads {} by only {}: small changes in scores or weights swap them",
                names[lead],
                names[second],
                apa::number(totals[lead] - totals[second], 3)
            ),
        ));
    }
    computed.assumptions.push(Assumption {
        id: "compensatory".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some(
            "a weighted sum lets a strength offset a weakness; check no option fails a must-have"
                .into(),
        ),
    });
    if request.normalization == Normalization::MinMax {
        computed.assumptions.push(Assumption {
            id: "range_dependence".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some(
                "min-max scores depend on the options compared: adding one can reorder the rest"
                    .into(),
            ),
        });
    }
    computed.narrative = Some(Narrative {
        template: "decision_matrix".into(),
        text_slots: vec!["margin".into(), "min_weight_change".into()],
    });
    let body = DecisionMatrixBody {
        normalization: request.normalization,
        criteria: request
            .criteria
            .iter()
            .zip(&w)
            .map(|(c, wj)| DecisionCriterion {
                name: c.name.clone(),
                raw_weight: c.weight,
                weight: *wj,
                direction: c.direction,
            })
            .collect(),
        options: request
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| DecisionOption {
                name: o.name.clone(),
                scores: o.scores.clone(),
                normalized: s[i].clone(),
                total: totals[i],
                rank: rank[i],
            })
            .collect(),
        sensitivity: sens,
    };
    Ok((computed, ComputedBody::DecisionMatrix(body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_share_ties_and_normalization_reverses_costs() {
        assert_eq!(ranks(&[0.5, 0.7, 0.7, 0.1]), [3, 1, 1, 4]);
        let costs = normalize(
            &[100.0, 80.0, 120.0],
            CriterionDirection::Cost,
            Normalization::MinMax,
        );
        assert_eq!(costs, Ok(vec![0.5, 1.0, 0.0]));
        let ratio = normalize(&[2.0, 4.0], CriterionDirection::Benefit, Normalization::Max);
        assert_eq!(ratio, Ok(vec![0.5, 1.0]));
        assert!(normalize(&[0.0, 1.0], CriterionDirection::Cost, Normalization::Max).is_err());
    }
}
