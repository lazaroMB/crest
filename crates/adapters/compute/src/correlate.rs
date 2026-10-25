//! Correlation matrices: Pearson, Spearman and Kendall's τ-b on pairwise
//! complete observations, with Holm, Benjamini-Hochberg or Bonferroni
//! adjustment over the pairs.
//!
//! p-values: Pearson from t on n − 2 df; Spearman exact by enumeration for
//! n ≤ 9 without ties, else the t approximation (scipy's; R uses an
//! Edgeworth series above 9); Kendall exact for n < 50 without ties (R's
//! rule), else the normal approximation with the tie-corrected variance.

use application::ports::compute::{ComputeError, CorrelateRequest};
use domain::computed::{
    Alternative, Assumption, AssumptionStatus, Computed, ComputedBody, CorrelationBody,
    CorrelationMethod, Narrative, PAdjust, UncertaintyKind, WarningSeverity,
};
use serde_json::Value;

use crate::checks::shapiro_wilk;
use crate::data::{self, key_part};
use crate::envelope::{self, bounded, count, df, labelled, p_value, warning, with_ci};
use crate::math::{p_from_t, p_from_z, qnorm};
use crate::rank::{inversion_counts, ranks, tails};
use crate::reproduce::{py_frame, py_str, r_col, r_frame, r_str};
use crate::tests::r_label;

/// A coefficient and its raw p-value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pair {
    /// Coefficient.
    pub r: f64,
    /// Two-sided p-value.
    pub p: f64,
}

fn pearson_r(x: &[f64], y: &[f64]) -> Option<f64> {
    let mx = crate::describe::mean(x);
    let my = crate::describe::mean(y);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (a, b) in x.iter().zip(y) {
        let (da, db) = (a - mx, b - my);
        sxy += da * db;
        sxx += da * da;
        syy += db * db;
    }
    (sxx > 0.0 && syy > 0.0).then(|| (sxy / (sxx * syy).sqrt()).clamp(-1.0, 1.0))
}

fn t_p(r: f64, n: usize) -> f64 {
    let dof = n as f64 - 2.0;
    if r.abs() >= 1.0 {
        return 0.0;
    }
    p_from_t(r * (dof / (1.0 - r * r)).sqrt(), dof, Alternative::TwoSided)
}

/// Pearson's r.
pub fn pearson(x: &[f64], y: &[f64]) -> Option<Pair> {
    let r = pearson_r(x, y)?;
    Some(Pair {
        r,
        p: t_p(r, x.len()),
    })
}

/// Spearman's ρ.
pub fn spearman(x: &[f64], y: &[f64], alternative: Alternative) -> Option<(Pair, bool)> {
    let (rx, tx) = ranks(x);
    let (ry, ty) = ranks(y);
    let r = pearson_r(&rx, &ry)?;
    let n = x.len();
    if n <= 9 && tx.is_empty() && ty.is_empty() {
        let s: f64 = rx.iter().zip(&ry).map(|(a, b)| (a - b).powi(2)).sum();
        let (lower, upper) = spearman_exact(n, s);
        let p = match alternative {
            Alternative::Greater => lower,
            Alternative::Less => upper,
            Alternative::TwoSided => (2.0 * lower.min(upper)).min(1.0),
        };
        return Some((Pair { r, p }, true));
    }
    let p = match alternative {
        Alternative::TwoSided => t_p(r, n),
        _ => {
            let dof = n as f64 - 2.0;
            let t = r * (dof / (1.0 - r * r).max(f64::MIN_POSITIVE)).sqrt();
            p_from_t(t, dof, alternative)
        }
    };
    Some((Pair { r, p }, false))
}

/// P(S ≤ s) and P(S ≥ s) for S = Σ d² over all permutations of n ranks.
fn spearman_exact(n: usize, s: f64) -> (f64, f64) {
    let mut perm: Vec<usize> = (0..n).collect();
    let (mut le, mut ge, mut total) = (0u64, 0u64, 0u64);
    let mut visit = |perm: &[usize]| {
        let v: f64 = perm
            .iter()
            .enumerate()
            .map(|(i, p)| (i as f64 - *p as f64).powi(2))
            .sum();
        total += 1;
        le += u64::from(v <= s + 1e-9);
        ge += u64::from(v >= s - 1e-9);
    };
    // Heap's algorithm.
    let mut c = vec![0usize; n];
    visit(&perm);
    let mut i = 0;
    while i < n {
        if c[i] < i {
            if i % 2 == 0 {
                perm.swap(0, i);
            } else {
                perm.swap(c[i], i);
            }
            visit(&perm);
            c[i] += 1;
            i = 0;
        } else {
            c[i] = 0;
            i += 1;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let (le, ge, total) = (le as f64, ge as f64, total as f64);
    (le / total, ge / total)
}

/// Kendall's τ-b; `true` when the p-value is exact.
pub fn kendall(x: &[f64], y: &[f64], alternative: Alternative) -> Option<(Pair, bool)> {
    let n = x.len();
    let (mut concordant, mut discordant) = (0i64, 0i64);
    let (mut ties_x, mut ties_y) = (0i64, 0i64);
    for i in 0..n {
        for j in i + 1..n {
            let dx = x[i] - x[j];
            let dy = y[i] - y[j];
            if dx == 0.0 {
                ties_x += 1;
            }
            if dy == 0.0 {
                ties_y += 1;
            }
            let s = dx * dy;
            if s > 0.0 {
                concordant += 1;
            } else if s < 0.0 {
                discordant += 1;
            }
        }
    }
    let pairs = (n * n.saturating_sub(1) / 2) as f64;
    #[allow(clippy::cast_precision_loss)]
    let (c, d, tx, ty) = (
        concordant as f64,
        discordant as f64,
        ties_x as f64,
        ties_y as f64,
    );
    let denominator = ((pairs - tx) * (pairs - ty)).sqrt();
    if denominator == 0.0 {
        return None;
    }
    let tau = (c - d) / denominator;
    let (_, gx) = ranks(x);
    let (_, gy) = ranks(y);
    if n < 50 && gx.is_empty() && gy.is_empty() {
        let counts = inversion_counts(n);
        // P(T ≥ c) for T concordant = P(D ≤ pairs − c) for D discordant.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (lower_d, upper_d) = tails(&counts, d as usize);
        // Greater (positive association) = few discordant pairs.
        let p = match alternative {
            Alternative::Greater => lower_d,
            Alternative::Less => upper_d,
            Alternative::TwoSided => (2.0 * lower_d.min(upper_d)).min(1.0),
        };
        return Some((Pair { r: tau, p }, true));
    }
    let nf = n as f64;
    let sum = |groups: &[usize], f: &dyn Fn(f64) -> f64| -> f64 {
        groups.iter().map(|&t| f(t as f64)).sum()
    };
    let v0 = nf * (nf - 1.0) * (2.0 * nf + 5.0);
    let vt = sum(&gx, &|t| t * (t - 1.0) * (2.0 * t + 5.0));
    let vu = sum(&gy, &|t| t * (t - 1.0) * (2.0 * t + 5.0));
    let v1 = sum(&gx, &|t| t * (t - 1.0)) * sum(&gy, &|t| t * (t - 1.0));
    let v2 = sum(&gx, &|t| t * (t - 1.0) * (t - 2.0)) * sum(&gy, &|t| t * (t - 1.0) * (t - 2.0));
    let var = (v0 - vt - vu) / 18.0
        + v1 / (2.0 * nf * (nf - 1.0))
        + v2 / (9.0 * nf * (nf - 1.0) * (nf - 2.0));
    let z = (c - d) / var.sqrt();
    Some((
        Pair {
            r: tau,
            p: p_from_z(z, alternative),
        },
        false,
    ))
}

/// Adjusts a family of p-values.
pub fn adjust(p: &[f64], method: PAdjust) -> Vec<f64> {
    let m = p.len();
    let mf = m as f64;
    let mut order: Vec<usize> = (0..m).collect();
    order.sort_by(|&a, &b| p[a].total_cmp(&p[b]));
    let mut out = vec![0.0; m];
    match method {
        PAdjust::None => out.copy_from_slice(p),
        PAdjust::Bonferroni => {
            for (o, x) in out.iter_mut().zip(p) {
                *o = (x * mf).min(1.0);
            }
        }
        PAdjust::Holm => {
            let mut running = 0.0f64;
            for (rank, &i) in order.iter().enumerate() {
                running = running.max((mf - rank as f64) * p[i]);
                out[i] = running.min(1.0);
            }
        }
        PAdjust::Bh => {
            let mut running = 1.0f64;
            for (rank, &i) in order.iter().enumerate().rev() {
                running = running.min(mf / (rank as f64 + 1.0) * p[i]);
                out[i] = running.min(1.0);
            }
        }
    }
    out
}

fn method_name(method: CorrelationMethod) -> (&'static str, &'static str, &'static str) {
    match method {
        CorrelationMethod::Pearson => ("pearson", "Pearson correlation", "r"),
        CorrelationMethod::Spearman => ("spearman", "Spearman rank correlation", "rₛ"),
        CorrelationMethod::Kendall => ("kendall", "Kendall's τ-b", "τ"),
    }
}

/// Runs a correlation request.
#[allow(clippy::too_many_lines)]
pub fn run(
    request: &CorrelateRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    let data = &request.data;
    let names: Vec<String> = if request.variables.is_empty() {
        data.columns
            .iter()
            .filter(|c| data::is_numeric(c))
            .map(|c| c.name.clone())
            .collect()
    } else {
        request.variables.clone()
    };
    if names.len() < 2 {
        return Err(ComputeError::Invalid(
            "a correlation needs at least two numeric columns".into(),
        ));
    }
    if !(request.conf_level > 0.0 && request.conf_level < 1.0) {
        return Err(ComputeError::Invalid(
            "`conf_level` must be between 0 and 1".into(),
        ));
    }
    let columns: Vec<Vec<Option<f64>>> = names
        .iter()
        .map(|n| data::numeric(data, n))
        .collect::<Result<_, _>>()?;
    let (method_key, method_label, symbol) = method_name(request.method);
    let mut computed = Computed::new(
        id,
        format!("stats.correlation_{method_key}"),
        method_label,
        envelope::engine(),
    );
    computed.uncertainty_kind = Some(UncertaintyKind::FrequentistCi);
    computed.method.alternatives = [
        CorrelationMethod::Pearson,
        CorrelationMethod::Spearman,
        CorrelationMethod::Kendall,
    ]
    .into_iter()
    .filter(|m| *m != request.method)
    .map(|m| method_name(m).0.to_owned())
    .collect();
    let params = &mut computed.method.params;
    params.insert("method".into(), Value::from(method_key));
    params.insert(
        "adjustment".into(),
        serde_json::to_value(request.adjustment).unwrap_or_default(),
    );
    params.insert("missing".into(), Value::from("pairwise complete"));
    let size = names.len();
    let mut r = vec![vec![None; size]; size];
    let mut p = vec![vec![None; size]; size];
    let mut n = vec![vec![0u64; size]; size];
    let mut family: Vec<(usize, usize, f64)> = Vec::new();
    let mut exact_any = false;
    let mut constant = Vec::new();
    for i in 0..size {
        n[i][i] = data::present(&columns[i]).len() as u64;
        r[i][i] = Some(1.0);
        for j in i + 1..size {
            let (x, y): (Vec<f64>, Vec<f64>) = columns[i]
                .iter()
                .zip(&columns[j])
                .filter_map(|(a, b)| a.zip(*b))
                .unzip();
            n[i][j] = x.len() as u64;
            n[j][i] = x.len() as u64;
            if x.len() < 3 {
                continue;
            }
            let pair = match request.method {
                CorrelationMethod::Pearson => pearson(&x, &y),
                CorrelationMethod::Spearman => {
                    spearman(&x, &y, Alternative::TwoSided).map(|(p, e)| {
                        exact_any |= e;
                        p
                    })
                }
                CorrelationMethod::Kendall => {
                    kendall(&x, &y, Alternative::TwoSided).map(|(p, e)| {
                        exact_any |= e;
                        p
                    })
                }
            };
            match pair {
                Some(pair) => {
                    r[i][j] = Some(pair.r);
                    r[j][i] = Some(pair.r);
                    family.push((i, j, pair.p));
                }
                None => constant.push(format!("{} × {}", names[i], names[j])),
            }
        }
    }
    let raw: Vec<f64> = family.iter().map(|f| f.2).collect();
    let adjusted = adjust(&raw, request.adjustment);
    for ((i, j, _), q) in family.iter().zip(&adjusted) {
        p[*i][*j] = Some(*q);
        p[*j][*i] = Some(*q);
    }
    if exact_any {
        computed.method.params.insert(
            "p_method".into(),
            Value::from("exact where n is small and untied"),
        );
    }
    // Values: one triple per pair; the headline keys for a single pair.
    for ((i, j, _), q) in family.iter().zip(&adjusted) {
        let key = format!("{}.{}", key_part(&names[*i]), key_part(&names[*j]));
        if let Some(rv) = r[*i][*j] {
            computed.values.insert(format!("r.{key}"), bounded(rv, 2));
        }
        computed.values.insert(format!("p.{key}"), p_value(*q));
        computed.values.insert(format!("n.{key}"), count(n[*i][*j]));
    }
    if let [(0, 1, _)] = family.as_slice()
        && let Some(rv) = r[0][1]
    {
        let pairs = n[0][1];
        let mut value = labelled(bounded(rv, 2), r_label(rv), "Cohen 1988");
        if request.method == CorrelationMethod::Pearson && pairs > 3 {
            // Fisher z interval.
            let z = rv.atanh();
            let half = qnorm(1.0 - (1.0 - request.conf_level) / 2.0) / ((pairs - 3) as f64).sqrt();
            value = with_ci(
                value,
                request.conf_level,
                (z - half).tanh(),
                (z + half).tanh(),
                "fisher_z",
            );
        }
        computed.values.insert("r".into(), value.clone());
        computed.values.insert("effect".into(), value);
        computed.values.insert("statistic".into(), bounded(rv, 2));
        computed
            .values
            .insert("p_value".into(), p_value(adjusted[0]));
        computed.values.insert("n".into(), count(pairs));
        #[allow(clippy::cast_precision_loss)]
        computed.values.insert("df".into(), df(pairs as f64 - 2.0));
        computed.narrative = Some(Narrative {
            template: "apa_correlation".into(),
            text_slots: vec!["r".into(), "df".into(), "p_value".into()],
        });
    } else {
        computed.narrative = Some(Narrative {
            template: "correlation_matrix".into(),
            text_slots: Vec::new(),
        });
    }
    computed
        .method
        .params
        .insert("symbol".into(), Value::from(symbol));

    // Assumptions.
    if request.method == CorrelationMethod::Pearson {
        computed.assumptions.push(Assumption {
            id: "linearity".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some("Pearson measures linear association; look at the scatter plot".into()),
        });
        for (name, column) in names.iter().zip(&columns) {
            let xs = data::present(column);
            let check = shapiro_wilk(&xs);
            let fail = check.is_some_and(|c| c.p < 0.05);
            computed.assumptions.push(Assumption {
                id: format!("normality_{}", key_part(name)),
                check: check.map(|_| "Shapiro-Wilk".into()),
                stat: check.map(|c| c.stat),
                p: check.map(|c| c.p),
                status: match check {
                    Some(_) if fail => AssumptionStatus::Fail,
                    Some(_) => AssumptionStatus::Pass,
                    None => AssumptionStatus::NotChecked,
                },
                consequence: fail.then(|| "p-values assume normality; consider spearman".into()),
                note: Some(format!("normality of {name}")),
            });
        }
    } else {
        computed.assumptions.push(Assumption {
            id: "monotonicity".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some("rank correlations measure monotonic association".into()),
        });
    }
    computed.assumptions.push(Assumption {
        id: "independence".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some("rows independent of each other; confirm".into()),
    });

    // Misuse warnings.
    if family.len() > 1 && request.adjustment == PAdjust::None {
        computed.warnings.push(warning(
            "multiple_comparisons",
            WarningSeverity::Warn,
            format!(
                "{} correlations tested without adjustment; use holm or bh",
                family.len()
            ),
        ));
    }
    if !constant.is_empty() {
        computed.warnings.push(warning(
            "constant_column",
            WarningSeverity::Warn,
            format!(
                "no correlation for {}: a column is constant",
                constant.join(", ")
            ),
        ));
    }
    let min_n = family
        .iter()
        .map(|(i, j, _)| n[*i][*j])
        .min()
        .unwrap_or_default();
    if !family.is_empty() && min_n < 20 {
        computed.warnings.push(warning(
            "small_n",
            WarningSeverity::Info,
            format!("as few as {min_n} pairs: correlations are imprecise"),
        ));
    }
    if family.is_empty() {
        envelope::fail(
            &mut computed,
            "no pair has at least 3 complete, non-constant observations",
        );
    }

    let used_names: Vec<&str> = names.iter().map(String::as_str).collect();
    let used = data::used_columns(data, &used_names);
    computed.inputs.push(envelope::input(data, &used, None));
    let quoted: Vec<String> = names.iter().map(|n| py_str(n)).collect();
    let adjust_py = match request.adjustment {
        PAdjust::None => String::new(),
        PAdjust::Holm => {
            "; adjust with statsmodels.stats.multitest.multipletests(p, method='holm')".into()
        }
        PAdjust::Bh => {
            "; adjust with statsmodels.stats.multitest.multipletests(p, method='fdr_bh')".into()
        }
        PAdjust::Bonferroni => {
            "; adjust with statsmodels.stats.multitest.multipletests(p, method='bonferroni')".into()
        }
    };
    let adjust_r = match request.adjustment {
        PAdjust::None => String::new(),
        PAdjust::Holm => "; p.adjust(p, \"holm\")".into(),
        PAdjust::Bh => "; p.adjust(p, \"BH\")".into(),
        PAdjust::Bonferroni => "; p.adjust(p, \"bonferroni\")".into(),
    };
    if size == 2 {
        let scipy = match request.method {
            CorrelationMethod::Pearson => "pearsonr",
            CorrelationMethod::Spearman => "spearmanr",
            CorrelationMethod::Kendall => "kendalltau",
        };
        computed.reproduce.python = Some(format!(
            "import scipy.stats as st; {}; d = df[[{}]].dropna(); st.{scipy}(d[{}], d[{}])",
            py_frame(data, &used),
            quoted.join(", "),
            quoted[0],
            quoted[1]
        ));
        computed.reproduce.r = Some(format!(
            "{}; cor.test({}, {}, method = \"{method_key}\")",
            r_frame(data, &used),
            r_col(&names[0]),
            r_col(&names[1])
        ));
    } else {
        computed.reproduce.python = Some(format!(
            "{}; df[[{}]].corr(method='{method_key}'){adjust_py}",
            py_frame(data, &used),
            quoted.join(", ")
        ));
        let r_names: Vec<String> = names.iter().map(|n| r_str(n)).collect();
        computed.reproduce.r = Some(format!(
            "{}; cor(df[c({})], method = \"{method_key}\", use = \"pairwise.complete.obs\"){adjust_r}",
            r_frame(data, &used),
            r_names.join(", ")
        ));
    }
    let request_json = serde_json::to_value(
        application::ports::compute::ComputeRequest::Correlate(request.clone()),
    )
    .unwrap_or_default();
    computed.reproduce.lua = Some(crate::reproduce::lua(
        "correlate",
        &request_json,
        Some(data),
    ));
    let body = ComputedBody::Correlation(CorrelationBody {
        method: request.method,
        adjustment: request.adjustment,
        variables: names,
        r,
        p,
        n,
    });
    Ok((computed, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjustments_match_r_p_adjust() {
        // p.adjust(c(0.01, 0.02, 0.03, 0.04), "holm") = 0.04 0.06 0.06 0.06
        // p.adjust(c(0.01, 0.02, 0.03, 0.04), "BH") = 0.04 0.04 0.04 0.04
        let p = [0.01, 0.04, 0.03, 0.02];
        let close = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-12);
        assert!(close(&adjust(&p, PAdjust::Holm), &[0.04, 0.06, 0.06, 0.06]));
        assert!(close(&adjust(&p, PAdjust::Bh), &[0.04, 0.04, 0.04, 0.04]));
        assert!(close(
            &adjust(&p, PAdjust::Bonferroni),
            &[0.04, 0.16, 0.12, 0.08]
        ));
        assert!(close(&adjust(&p, PAdjust::None), &p));
    }
}
