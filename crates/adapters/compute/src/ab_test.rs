//! The A/B test card: two conversion rates compared.
//!
//! - Rates with Wilson intervals; the absolute lift with Newcombe's hybrid
//!   score interval (method 10 of Newcombe 1998), the relative lift with
//!   the Katz log-ratio interval.
//! - The p-value from the pooled two-proportion z-test (`prop.test(...,
//!   correct = FALSE)`), or Fisher's exact test when an expected count of
//!   the 2×2 table is below 5.
//! - P(B > A) under uniform Beta(1, 1) priors, exactly (Evan Miller's sum).
//! - The sample-ratio check: χ² of the visitor counts against the planned
//!   split; p < .001 flags a mismatch, which invalidates the comparison.
//! - Peeking: several looks at a fixed α inflate the false-positive rate;
//!   the warning quotes Armitage, McPherson and Rowe's (1969) rates.
//! - The minimum detectable effect at 80% power and, for a given minimum
//!   lift, the power to detect it (normal approximations).

use application::ports::compute::{AbTestRequest, ComputeError};
use domain::computed::{
    AbArm, AbMethod, AbTestBody, Alternative, Assumption, AssumptionStatus, Computed, ComputedBody,
    ComputedValue, Interval, Narrative, UncertaintyKind, ValueFormat, WarningSeverity, apa,
};
use serde_json::Value;
use statrs::function::beta::ln_beta;

use crate::envelope::{self, bounded, count, number, p_value, warning};
use crate::math::{p_from_z, pchisq_upper, pnorm, qnorm};
use crate::tests::fisher_p;

/// SRM p-values below this flag a mismatch.
pub const SRM_ALPHA: f64 = 0.001;
/// Power the minimum detectable effect is computed for.
const MDE_POWER: f64 = 0.8;

/// False-positive rate of a two-sided test at nominal α = .05 looked at
/// `k` times on accumulating normal data (Armitage, McPherson and Rowe
/// 1969, table 2).
const PEEKING: [(u32, f64); 9] = [
    (1, 0.05),
    (2, 0.083),
    (3, 0.107),
    (4, 0.126),
    (5, 0.142),
    (10, 0.193),
    (20, 0.248),
    (50, 0.320),
    (100, 0.374),
];

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// Wilson score interval of `x` successes in `n` at critical value `z`.
pub fn wilson(x: f64, n: f64, z: f64) -> (f64, f64) {
    let p = x / n;
    let z2 = z * z;
    let centre = (p + z2 / (2.0 * n)) / (1.0 + z2 / n);
    let half = z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / (1.0 + z2 / n);
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// Newcombe's hybrid score interval of `p2 − p1` from the Wilson
/// intervals of each rate.
pub fn newcombe(p1: f64, (l1, u1): (f64, f64), p2: f64, (l2, u2): (f64, f64)) -> (f64, f64) {
    let d = p2 - p1;
    (
        d - ((p2 - l2).powi(2) + (u1 - p1).powi(2)).sqrt(),
        d + ((u2 - p2).powi(2) + (p1 - l1).powi(2)).sqrt(),
    )
}

/// P(p_B > p_A) for Beta(1 + x, 1 + n − x) posteriors (uniform priors),
/// by Evan Miller's closed form; `None` beyond a million conversions in B.
pub fn prob_b_beats_a(xa: u64, na: u64, xb: u64, nb: u64) -> Option<f64> {
    if xb > 1_000_000 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let (aa, ba, ab, bb) = (
        1.0 + xa as f64,
        1.0 + (na - xa) as f64,
        1.0 + xb as f64,
        1.0 + (nb - xb) as f64,
    );
    let base = ln_beta(aa, ba);
    let mut total = 0.0;
    let mut i = 0.0;
    while i < ab {
        total += (ln_beta(aa + i, ba + bb) - (bb + i).ln() - ln_beta(1.0 + i, bb) - base).exp();
        i += 1.0;
    }
    Some(total.clamp(0.0, 1.0))
}

/// The false-positive rate after `k` looks at α = .05, from [`PEEKING`]
/// (the largest tabulated `k` not above it).
fn peeking_rate(k: u32) -> f64 {
    PEEKING
        .iter()
        .rev()
        .find(|(looks, _)| *looks <= k)
        .map_or(0.05, |(_, rate)| *rate)
}

/// A rate shown as a percentage.
fn percent(v: f64, decimals: u32) -> ComputedValue {
    ComputedValue {
        display: Some(format!("{}%", apa::number(v * 100.0, decimals))),
        decimals: Some(decimals),
        fmt: Some(ValueFormat::Percent),
        ..ComputedValue::new(v)
    }
}

/// A difference of rates shown in percentage points, with its interval.
fn points(v: f64, level: f64, (lo, hi): (f64, f64), method: &str) -> ComputedValue {
    let mut value = ComputedValue {
        display: Some(format!("{} pp", apa::number(v * 100.0, 2))),
        decimals: Some(4),
        ..ComputedValue::new(v)
    };
    if lo.is_finite() && hi.is_finite() {
        value.ci = Some(Interval {
            level,
            lo,
            hi,
            method: Some(method.into()),
        });
    }
    value
}

/// Runs an A/B request.
pub fn run(request: &AbTestRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    let (a, b) = (&request.control, &request.treatment);
    for (arm, which) in [(a, "control"), (b, "treatment")] {
        if arm.visitors == 0 {
            return Err(invalid(format!("the {which} arm has no visitors")));
        }
        if arm.conversions > arm.visitors {
            return Err(invalid(format!(
                "the {which} arm has more conversions than visitors"
            )));
        }
    }
    if !(request.conf_level > 0.0 && request.conf_level < 1.0) {
        return Err(invalid("`conf_level` must be between 0 and 1"));
    }
    if !(request.expected_split > 0.0 && request.expected_split < 1.0) {
        return Err(invalid("`expected_split` must be between 0 and 1"));
    }
    if request.looks == 0 {
        return Err(invalid("`looks` counts this look, so it is at least 1"));
    }
    let level = request.conf_level;
    let alpha = 1.0 - level;
    let z_crit = qnorm(1.0 - alpha / 2.0);
    #[allow(clippy::cast_precision_loss)]
    let (x1, n1, x2, n2) = (
        a.conversions as f64,
        a.visitors as f64,
        b.conversions as f64,
        b.visitors as f64,
    );
    let (p1, p2) = (x1 / n1, x2 / n2);
    let (w1, w2) = (wilson(x1, n1, z_crit), wilson(x2, n2, z_crit));
    let arm = |input: &application::ports::compute::ArmCounts, default: &str, p, (lo, hi)| AbArm {
        name: input.name.clone().unwrap_or_else(|| default.to_owned()),
        visitors: input.visitors,
        conversions: input.conversions,
        rate: p,
        rate_lo: lo,
        rate_hi: hi,
    };
    let arms = vec![arm(a, "A", p1, w1), arm(b, "B", p2, w2)];
    let (name_a, name_b) = (arms[0].name.clone(), arms[1].name.clone());

    let mut computed = Computed::new(id, "stats.ab_test", "A/B test", envelope::engine());
    computed.uncertainty_kind = Some(UncertaintyKind::FrequentistCi);
    computed.inputs.push(domain::computed::InputRef {
        name: "counts".into(),
        role: Some("data".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(&(a, b)).unwrap_or_default(),
        )),
        rows: Some(a.visitors + b.visitors),
        columns: Vec::new(),
        filter: None,
    });

    // The 2×2 table with B first, so an odds ratio above 1 favours B.
    let table = vec![vec![x2, n2 - x2], vec![x1, n1 - x1]];
    let total = n1 + n2;
    let conv = x1 + x2;
    let min_expected = [
        n2 * conv / total,
        n2 * (total - conv) / total,
        n1 * conv / total,
        n1 * (total - conv) / total,
    ]
    .into_iter()
    .fold(f64::INFINITY, f64::min);
    let method = if min_expected < 5.0 {
        AbMethod::FisherExact
    } else {
        AbMethod::ZTest
    };
    let pooled = conv / total;
    let se0 = (pooled * (1.0 - pooled) * (1.0 / n1 + 1.0 / n2)).sqrt();
    let z = (p2 - p1) / se0;
    let p = match method {
        AbMethod::ZTest => p_from_z(z, request.alternative),
        AbMethod::FisherExact => fisher_p(&table, request.alternative).unwrap_or(f64::NAN),
    };
    let decimals = 2;
    let values = &mut computed.values;
    values.insert("control_rate".into(), percent(p1, decimals));
    values.insert("treatment_rate".into(), percent(p2, decimals));
    values.insert(
        "abs_lift".into(),
        points(
            p2 - p1,
            level,
            newcombe(p1, w1, p2, w2),
            "newcombe_hybrid_score",
        ),
    );
    if x1 > 0.0 && x2 > 0.0 {
        let se = (1.0 / x2 - 1.0 / n2 + 1.0 / x1 - 1.0 / n1).sqrt();
        let ratio = p2 / p1;
        let lo = (ratio.ln() - z_crit * se).exp() - 1.0;
        let hi = (ratio.ln() + z_crit * se).exp() - 1.0;
        let mut rel = percent(ratio - 1.0, 1);
        rel.ci = Some(Interval {
            level,
            lo,
            hi,
            method: Some("katz_log_ratio".into()),
        });
        values.insert("rel_lift".into(), rel);
    }
    if method == AbMethod::ZTest {
        values.insert("statistic".into(), number(z, 2));
    }
    values.insert("p_value".into(), p_value(p));
    values.insert("visitors_control".into(), count(a.visitors));
    values.insert("visitors_treatment".into(), count(b.visitors));
    if let Some(prob) = prob_b_beats_a(a.conversions, a.visitors, b.conversions, b.visitors) {
        values.insert("prob_b_better".into(), bounded(prob, 3));
    }
    // Sample-ratio mismatch.
    let s = request.expected_split;
    let (e1, e2) = (total * (1.0 - s), total * s);
    let srm_chi2 = (n1 - e1).powi(2) / e1 + (n2 - e2).powi(2) / e2;
    let srm_p = pchisq_upper(srm_chi2, 1.0);
    let mismatch = srm_p < SRM_ALPHA;
    values.insert("srm_chi2".into(), number(srm_chi2, 2));
    values.insert("srm_p".into(), p_value(srm_p));
    // The smallest lift the sample detects with 80% power.
    let z_power = qnorm(MDE_POWER);
    let z_alpha = match request.alternative {
        Alternative::TwoSided => z_crit,
        _ => qnorm(1.0 - alpha),
    };
    let mde = (z_alpha + z_power) * (p1 * (1.0 - p1) * (1.0 / n1 + 1.0 / n2)).sqrt();
    values.insert("mde".into(), points(mde, level, (f64::NAN, f64::NAN), ""));
    if let Some(lift) = request.minimum_lift {
        let q2 = (p1 + lift).clamp(0.0, 1.0);
        let se = (p1 * (1.0 - p1) / n1 + q2 * (1.0 - q2) / n2).sqrt();
        let power = pnorm(lift.abs() / se - z_alpha);
        values.insert("power".into(), bounded(power, 3));
    }

    let params = &mut computed.method.params;
    params.insert("conf_level".into(), Value::from(level));
    params.insert("expected_split".into(), Value::from(s));
    params.insert("looks".into(), Value::from(request.looks));
    params.insert(
        "alternative".into(),
        Value::from(match request.alternative {
            Alternative::TwoSided => "two-sided",
            Alternative::Less => "less (B below A)",
            Alternative::Greater => "greater (B above A)",
        }),
    );
    computed.method.name = format!("A/B test: {}", method.label());
    computed.method.why = Some(match method {
        AbMethod::ZTest => "every expected count is at least 5, so the z-test holds".into(),
        AbMethod::FisherExact => format!(
            "an expected count is {} < 5, so Fisher's exact test",
            apa::number(min_expected, 2)
        ),
    });
    computed.method.alternatives = vec![match method {
        AbMethod::ZTest => "fisher".into(),
        AbMethod::FisherExact => "z_test".into(),
    }];
    computed.assumptions = vec![
        Assumption {
            id: "sample_ratio".into(),
            check: Some("χ² of visitors against the planned split".into()),
            stat: Some(srm_chi2),
            p: Some(srm_p),
            status: if mismatch {
                AssumptionStatus::Fail
            } else {
                AssumptionStatus::Pass
            },
            consequence: mismatch
                .then(|| "the arms differ by more than chance: find the cause first".into()),
            note: None,
        },
        Assumption {
            id: "independence".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some(
                "each unit counted once in one arm, randomised; a design property, confirm".into(),
            ),
        },
    ];
    if mismatch {
        computed.warnings.push(warning(
            "sample_ratio_mismatch",
            WarningSeverity::Error,
            format!(
                "sample-ratio mismatch: {} vs {} visitors for a planned {}/{} split (χ² p {}); \
                 assignment or logging is broken, so the lift cannot be trusted",
                a.visitors,
                b.visitors,
                apa::number((1.0 - s) * 100.0, 0),
                apa::number(s * 100.0, 0),
                {
                    let (rel, v) = apa::p_value_parts(srm_p);
                    format!("{rel} {v}")
                }
            ),
        ));
    }
    if request.looks > 1 {
        computed.warnings.push(warning(
            "peeking",
            WarningSeverity::Warn,
            format!(
                "the results were looked at {} times: stopping at the first p < .05 makes \
                 false positives about {} likely instead of .05 (Armitage et al. 1969); \
                 use a sequential design or fix the sample size in advance",
                request.looks,
                apa::bounded(peeking_rate(request.looks), 2)
            ),
        ));
    }
    if let Some(planned) = request.planned_visitors
        && a.visitors.min(b.visitors) < planned
    {
        computed.warnings.push(warning(
            "stopped_early",
            WarningSeverity::Warn,
            format!(
                "{} of the planned {planned} visitors per arm so far: the test has less power \
                 than planned",
                a.visitors.min(b.visitors)
            ),
        ));
    }
    reproduce(&mut computed, request);
    computed.narrative = Some(Narrative {
        template: "ab_test".into(),
        text_slots: [
            "control_rate",
            "treatment_rate",
            "abs_lift",
            "rel_lift",
            "statistic",
            "p_value",
            "prob_b_better",
        ]
        .into_iter()
        .filter(|k| computed.values.contains_key(*k))
        .map(str::to_owned)
        .collect(),
    });
    computed.method.params.insert(
        "arms".into(),
        Value::from(format!("{name_a} (control), {name_b} (treatment)")),
    );
    let body = AbTestBody {
        arms,
        expected_split: s,
        conf_level: level,
        method,
        looks: request.looks,
        sample_ratio_mismatch: mismatch,
    };
    Ok((computed, ComputedBody::AbTest(body)))
}

fn reproduce(computed: &mut Computed, request: &AbTestRequest) {
    let (a, b) = (&request.control, &request.treatment);
    let (alt_r, alt_py) = match request.alternative {
        Alternative::TwoSided => ("two.sided", "two-sided"),
        Alternative::Less => ("less", "smaller"),
        Alternative::Greater => ("greater", "larger"),
    };
    let s = request.expected_split;
    computed.reproduce.r = Some(format!(
        "prop.test(c({}, {}), c({}, {}), alternative = \"{alt_r}\", conf.level = {}, \
         correct = FALSE); chisq.test(c({}, {}), p = c({}, {s}))",
        b.conversions,
        a.conversions,
        b.visitors,
        a.visitors,
        request.conf_level,
        a.visitors,
        b.visitors,
        1.0 - s
    ));
    computed.reproduce.python = Some(format!(
        "from statsmodels.stats.proportion import proportions_ztest, confint_proportions_2indep; \
         proportions_ztest([{}, {}], [{}, {}], alternative='{alt_py}'); \
         confint_proportions_2indep({}, {}, {}, {}, method='newcomb')",
        b.conversions,
        a.conversions,
        b.visitors,
        a.visitors,
        b.conversions,
        b.visitors,
        a.conversions,
        a.visitors
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newcombe_matches_his_example() {
        // Newcombe (1998), example (a): 56/70 − 48/80, method 10:
        // 0.0524 to 0.3339.
        let z = qnorm(0.975);
        let (w1, w2) = (wilson(48.0, 80.0, z), wilson(56.0, 70.0, z));
        let (lo, hi) = newcombe(0.6, w1, 0.8, w2);
        assert!((lo - 0.0524).abs() < 5e-5, "{lo}");
        assert!((hi - 0.3339).abs() < 5e-5, "{hi}");
    }

    #[test]
    fn b_beats_a_matches_numeric_integration() {
        // ∫ f_B(x) F_A(x) dx by scipy.integrate.quad.
        let p = prob_b_beats_a(48, 80, 56, 70).unwrap_or(f64::NAN);
        assert!((p - 0.995_929_539_0).abs() < 1e-9, "{p}");
        let p = prob_b_beats_a(200, 10_000, 230, 10_000).unwrap_or(f64::NAN);
        assert!((p - 0.928_017_068_1).abs() < 1e-9, "{p}");
        assert_eq!(peeking_rate(7), 0.142);
        assert_eq!(peeking_rate(1), 0.05);
    }
}
