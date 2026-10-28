//! Power and sample size, as R's `pwr` package computes them: noncentral
//! t for the t-tests (`pwr.t.test`), noncentral F for the one-way ANOVA
//! (`pwr.anova.test`, λ = k·n·f²) and the arcsine normal approximation
//! with Cohen's h for proportions (`pwr.2p.test`, `pwr.p.test`).
//!
//! The quantity left out is solved for by bisection on the monotone power
//! function, to a relative 1e-10 (R's `uniroot` stops near 1e-4, so its
//! printed digits can differ in the fifth place). The card also gets the
//! power curve over n at the given effect and α.

use application::ports::compute::{ComputeError, PowerRequest};
use domain::computed::{
    Alternative, Assumption, AssumptionStatus, Computed, ComputedBody, Curve, Narrative, PowerBody,
    PowerTarget, PowerTest, UncertaintyKind, WarningSeverity,
};
use serde_json::Value;

use crate::envelope::{self, bounded, count, labelled, number, warning};
use crate::math::{pnf_upper, pnorm, pnorm_upper, pnt_or_normal, qf_upper, qnorm, qt};

/// Points of the power curve.
pub const CURVE_POINTS: usize = 60;
/// Largest n searched.
const MAX_N: f64 = 1e7;
/// Smallest n of the t-tests and ANOVA (R's `2 + 1e-10`).
const MIN_N: f64 = 2.0 + 1e-10;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// The design and its fixed settings.
#[derive(Debug, Clone, Copy)]
pub struct Design {
    /// Which test.
    pub test: PowerTest,
    /// Groups of an ANOVA.
    pub groups: f64,
    /// Direction.
    pub alternative: Alternative,
}

impl Design {
    /// Power at sample size `n` (per group where the design has groups),
    /// effect `es` and level `alpha`.
    pub fn power(&self, n: f64, es: f64, alpha: f64) -> f64 {
        match self.test {
            PowerTest::TwoSampleT | PowerTest::OneSampleT | PowerTest::PairedT => {
                let samples = if self.test == PowerTest::TwoSampleT {
                    2.0
                } else {
                    1.0
                };
                let nu = (n - 1.0) * samples;
                let ncp = (n / samples).sqrt() * es;
                match self.alternative {
                    Alternative::TwoSided => {
                        let q = qt(1.0 - alpha / 2.0, nu);
                        (1.0 - pnt_or_normal(q, nu, ncp)) + pnt_or_normal(-q, nu, ncp)
                    }
                    Alternative::Greater => 1.0 - pnt_or_normal(qt(1.0 - alpha, nu), nu, ncp),
                    Alternative::Less => pnt_or_normal(qt(alpha, nu), nu, ncp),
                }
            }
            PowerTest::Anova => {
                let k = self.groups;
                let (d1, d2) = (k - 1.0, (n - 1.0) * k);
                pnf_upper(qf_upper(alpha, d1, d2), d1, d2, k * n * es * es)
            }
            PowerTest::TwoProportions | PowerTest::OneProportion => {
                let shift = if self.test == PowerTest::TwoProportions {
                    es * (n / 2.0).sqrt()
                } else {
                    es * n.sqrt()
                };
                match self.alternative {
                    Alternative::TwoSided => {
                        pnorm_upper(qnorm(1.0 - alpha / 2.0) - shift)
                            + pnorm(qnorm(alpha / 2.0) - shift)
                    }
                    Alternative::Greater => pnorm_upper(qnorm(1.0 - alpha) - shift),
                    Alternative::Less => pnorm(qnorm(alpha) - shift),
                }
            }
        }
        .clamp(0.0, 1.0)
    }

    /// The smallest n of the design.
    fn min_n(&self) -> f64 {
        match self.test {
            PowerTest::TwoProportions | PowerTest::OneProportion => 1.0,
            _ => MIN_N,
        }
    }
}

/// The root of increasing `f − target` on `[lo, hi]` by bisection; `None`
/// when `target` lies outside `[f(lo), f(hi)]`.
fn solve(f: impl Fn(f64) -> f64, target: f64, mut lo: f64, mut hi: f64) -> Option<f64> {
    let (flo, fhi) = (f(lo) - target, f(hi) - target);
    if !(flo.is_finite() && fhi.is_finite()) || flo > 0.0 || fhi < 0.0 {
        return None;
    }
    for _ in 0..300 {
        let mid = 0.5 * (lo + hi);
        if f(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo <= 1e-10 * mid.abs().max(1e-12) {
            break;
        }
    }
    Some(0.5 * (lo + hi))
}

/// Cohen's h of two proportions.
pub fn cohens_h(p1: f64, p2: f64) -> f64 {
    2.0 * p1.sqrt().asin() - 2.0 * p2.sqrt().asin()
}

/// Cohen's verbal label of an effect size.
fn effect_label(test: PowerTest, es: f64) -> &'static str {
    let (small, medium, large) = match test {
        PowerTest::Anova => (0.1, 0.25, 0.4),
        _ => (0.2, 0.5, 0.8),
    };
    match es.abs() {
        e if e >= large => "large",
        e if e >= medium => "medium",
        e if e >= small => "small",
        _ => "negligible",
    }
}

/// Runs a power request.
pub fn run(request: &PowerRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    let test = request.test;
    let proportions = matches!(test, PowerTest::TwoProportions | PowerTest::OneProportion);
    let mut effect = request.effect;
    if let (Some(p1), Some(p2)) = (request.p1, request.p2) {
        if !proportions {
            return Err(invalid("`p1` and `p2` are for the proportion tests"));
        }
        if effect.is_some() {
            return Err(invalid("give `effect` or `p1` and `p2`, not both"));
        }
        if !(0.0..=1.0).contains(&p1) || !(0.0..=1.0).contains(&p2) {
            return Err(invalid("`p1` and `p2` must be proportions between 0 and 1"));
        }
        effect = Some(cohens_h(p1, p2));
    } else if request.p1.is_some() || request.p2.is_some() {
        return Err(invalid("give both `p1` and `p2`"));
    }
    let target = match request.solve_for {
        Some(target) => target,
        None => {
            let missing: Vec<PowerTarget> = [
                (request.n.is_none(), PowerTarget::N),
                (effect.is_none(), PowerTarget::Effect),
                (request.power.is_none(), PowerTarget::Power),
            ]
            .into_iter()
            .filter_map(|(m, t)| m.then_some(t))
            .collect();
            match missing.as_slice() {
                [one] => *one,
                [] if request.alpha.is_none() => PowerTarget::Alpha,
                _ => {
                    return Err(invalid(
                        "give exactly three of `n`, `effect` (or `p1` and `p2`), `power` and \
                         `alpha`; the fourth is solved for",
                    ));
                }
            }
        }
    };
    let alpha = match (target, request.alpha) {
        (PowerTarget::Alpha, _) => None,
        (_, Some(a)) => Some(a),
        (_, None) => Some(0.05),
    };
    let check = |name: &str, value: Option<f64>, ok: &dyn Fn(f64) -> bool| match value {
        Some(v) if !(v.is_finite() && ok(v)) => Err(invalid(format!("`{name}` is out of range"))),
        _ => Ok(()),
    };
    check("alpha", alpha, &|a| a > 0.0 && a < 1.0)?;
    check("power", request.power, &|p| p > 0.0 && p < 1.0)?;
    check("n", request.n, &|n| n >= 1.0)?;
    check("effect", effect, &|e| e != 0.0)?;
    let groups = match (test, request.groups) {
        (PowerTest::Anova, Some(k)) if k >= 2 => f64::from(k),
        (PowerTest::Anova, _) => return Err(invalid("an ANOVA needs `groups` ≥ 2")),
        _ => 2.0,
    };
    let design = Design {
        test,
        groups,
        alternative: request.alternative,
    };
    let min_n = design.min_n();
    if request.n.is_some_and(|n| n < min_n) && target != PowerTarget::N {
        return Err(invalid(format!("`n` must be at least {}", min_n.ceil())));
    }
    // The t and proportion tests take a signed effect; two-sided uses its
    // size, `less` a negative one (as pwr).
    let signed = |e: f64| match (test, request.alternative) {
        (PowerTest::Anova, _) | (_, Alternative::TwoSided) => e.abs(),
        _ => e,
    };
    let effect = effect.map(signed);
    let mut computed = Computed::new(id, "stats.power", "Power analysis", envelope::engine());
    computed.uncertainty_kind = Some(UncertaintyKind::Exact);
    let solved = match target {
        PowerTarget::Power => match (request.n, effect, alpha) {
            (Some(n), Some(e), Some(a)) => Some(design.power(n, e, a)),
            _ => None,
        },
        PowerTarget::N => match (effect, alpha, request.power) {
            (Some(e), Some(a), Some(p)) => solve(|n| design.power(n, e, a), p, min_n, MAX_N),
            _ => None,
        },
        PowerTarget::Effect => match (request.n, alpha, request.power) {
            (Some(n), Some(a), Some(p)) => {
                let less = request.alternative == Alternative::Less && test != PowerTest::Anova;
                if less {
                    solve(|e| design.power(n, -e, a), p, 1e-10, 10.0).map(|e| -e)
                } else {
                    solve(|e| design.power(n, e, a), p, 1e-10, 10.0)
                }
            }
            _ => None,
        },
        PowerTarget::Alpha => match (request.n, effect, request.power) {
            (Some(n), Some(e), Some(p)) => solve(|a| design.power(n, e, a), p, 1e-10, 1.0 - 1e-10),
            _ => None,
        },
    };
    let missing_input = solved.is_none()
        && match target {
            PowerTarget::Power => request.n.is_none() || effect.is_none(),
            PowerTarget::N => effect.is_none() || request.power.is_none(),
            PowerTarget::Effect => request.n.is_none() || request.power.is_none(),
            PowerTarget::Alpha => {
                request.n.is_none() || effect.is_none() || request.power.is_none()
            }
        };
    if missing_input {
        return Err(invalid(format!(
            "to solve for {} give the other three of n, effect, power and alpha",
            target.label()
        )));
    }
    let (n, es, a, power) = match (target, solved) {
        (PowerTarget::Power, Some(p)) => (
            request.n.unwrap_or(min_n),
            effect.unwrap_or(0.0),
            alpha.unwrap_or(0.05),
            p,
        ),
        (PowerTarget::N, Some(n)) => (
            n,
            effect.unwrap_or(0.0),
            alpha.unwrap_or(0.05),
            request.power.unwrap_or(0.8),
        ),
        (PowerTarget::Effect, Some(e)) => (
            request.n.unwrap_or(min_n),
            e,
            alpha.unwrap_or(0.05),
            request.power.unwrap_or(0.8),
        ),
        (PowerTarget::Alpha, Some(a)) => (
            request.n.unwrap_or(min_n),
            effect.unwrap_or(0.0),
            a,
            request.power.unwrap_or(0.8),
        ),
        (_, None) => {
            let body = empty_body(request, target, &design);
            envelope::fail(
                &mut computed,
                format!(
                    "no {} reaches the requested power with these settings (n up to {MAX_N:e}, \
                     effect up to 10, α below 1)",
                    target.label()
                ),
            );
            return Ok((computed, ComputedBody::Power(body)));
        }
    };
    let symbol = test.effect_symbol();
    let values = &mut computed.values;
    values.insert("n".into(), number(n, 2));
    values.insert("power".into(), bounded(power, 3));
    values.insert("alpha".into(), bounded(a, 3));
    values.insert(
        "effect".into(),
        labelled(number(es, 3), effect_label(test, es), "Cohen 1988"),
    );
    if target == PowerTarget::N {
        // A whole, positive sample size.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let whole = n.ceil() as u64;
        values.insert("n_required".into(), count(whole));
        if test.n_per_group() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let total = whole * groups as u64;
            values.insert("n_total".into(), count(total));
        }
    }
    if let (Some(p1), Some(p2)) = (request.p1, request.p2) {
        values.insert("p1".into(), bounded(p1, 3));
        values.insert("p2".into(), bounded(p2, 3));
    }
    let params = &mut computed.method.params;
    params.insert("test".into(), Value::from(test.label()));
    params.insert("solved_for".into(), Value::from(target.label()));
    params.insert("effect_symbol".into(), Value::from(symbol));
    params.insert(
        "alternative".into(),
        Value::from(match request.alternative {
            Alternative::TwoSided => "two-sided",
            Alternative::Less => "less",
            Alternative::Greater => "greater",
        }),
    );
    computed.method.name = format!("Power analysis: {}", test.label());
    computed.method.why = Some(format!(
        "solved for the {} from the other three quantities",
        target.label()
    ));
    computed.method.alternatives = Vec::new();
    computed.assumptions = assumptions(test);
    if power >= 0.99 {
        computed.warnings.push(warning(
            "power_near_one",
            WarningSeverity::Info,
            "power is at least .99: a smaller sample would do for this effect",
        ));
    }
    if target == PowerTarget::Power && power < 0.5 {
        computed.warnings.push(warning(
            "low_power",
            WarningSeverity::Warn,
            "power is below .50: a non-significant result says little about the effect",
        ));
    }
    if test == PowerTest::OneProportion || test == PowerTest::TwoProportions {
        computed.warnings.push(warning(
            "normal_approximation",
            WarningSeverity::Info,
            "the arcsine normal approximation of pwr; exact binomial power can differ for small n",
        ));
    }
    computed.inputs.push(domain::computed::InputRef {
        name: "parameters".into(),
        role: Some("param".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: None,
        columns: Vec::new(),
        filter: None,
    });
    reproduce(&mut computed, request, &design, target, n, es, a, power);
    computed.narrative = Some(Narrative {
        template: "power".into(),
        text_slots: ["n", "n_required", "n_total", "effect", "alpha", "power"]
            .into_iter()
            .filter(|k| computed.values.contains_key(*k))
            .map(str::to_owned)
            .collect(),
    });
    let body = PowerBody {
        test,
        solved_for: target,
        alternative: request.alternative,
        n,
        effect: es,
        alpha: a,
        power,
        groups: (test == PowerTest::Anova).then(|| request.groups.unwrap_or(2)),
        curve: curve(&design, n, es, a),
    };
    Ok((computed, ComputedBody::Power(body)))
}

fn empty_body(request: &PowerRequest, target: PowerTarget, design: &Design) -> PowerBody {
    PowerBody {
        test: request.test,
        solved_for: target,
        alternative: request.alternative,
        n: request.n.unwrap_or(design.min_n()),
        effect: request.effect.unwrap_or(0.0),
        alpha: request.alpha.unwrap_or(0.05),
        power: request.power.unwrap_or(0.0),
        groups: request.groups,
        curve: Curve::default(),
    }
}

/// Power over n from a quarter to three times `n` (at least 3 to 30).
fn curve(design: &Design, n: f64, es: f64, alpha: f64) -> Curve {
    let lo = (n * 0.25).max(design.min_n());
    let hi = (n * 3.0).max(lo + 10.0);
    let x: Vec<f64> = (0..CURVE_POINTS)
        .map(|i| lo + (hi - lo) * i as f64 / (CURVE_POINTS - 1) as f64)
        .collect();
    let y = x.iter().map(|&m| design.power(m, es, alpha)).collect();
    Curve { x, y }
}

fn assumptions(test: PowerTest) -> Vec<Assumption> {
    let design = |id: &str, note: &str| Assumption {
        id: id.into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some(note.into()),
    };
    let mut list = vec![design(
        "effect_size",
        "the effect size is an assumption (a pilot, the literature or the smallest effect \
         that matters); power is only as good as it",
    )];
    match test {
        PowerTest::TwoSampleT | PowerTest::Anova => {
            list.push(design(
                "normality_equal_variance",
                "normal outcomes with equal variances and equal group sizes",
            ));
        }
        PowerTest::OneSampleT | PowerTest::PairedT => {
            list.push(design(
                "normality",
                "normally distributed outcomes (or differences)",
            ));
        }
        PowerTest::TwoProportions | PowerTest::OneProportion => {
            list.push(design("independence", "independent binary outcomes"));
        }
    }
    list
}

#[allow(clippy::too_many_arguments)]
fn reproduce(
    computed: &mut Computed,
    request: &PowerRequest,
    design: &Design,
    target: PowerTarget,
    n: f64,
    es: f64,
    alpha: f64,
    power: f64,
) {
    let fmt = |x: f64| format!("{x}");
    let given = |t: PowerTarget, v: f64| {
        if t == target {
            "NULL".to_owned()
        } else {
            fmt(v)
        }
    };
    let given_py = |t: PowerTarget, v: f64| {
        if t == target {
            "None".to_owned()
        } else {
            fmt(v)
        }
    };
    let alt = match request.alternative {
        Alternative::TwoSided => "two.sided",
        Alternative::Less => "less",
        Alternative::Greater => "greater",
    };
    let alt_py = match request.alternative {
        Alternative::TwoSided => "two-sided",
        Alternative::Less => "smaller",
        Alternative::Greater => "larger",
    };
    let (r_n, r_es, r_a, r_p) = (
        given(PowerTarget::N, n),
        given(PowerTarget::Effect, es),
        given(PowerTarget::Alpha, alpha),
        given(PowerTarget::Power, power),
    );
    let (py_n, py_es, py_a, py_p) = (
        given_py(PowerTarget::N, n),
        given_py(PowerTarget::Effect, es),
        given_py(PowerTarget::Alpha, alpha),
        given_py(PowerTarget::Power, power),
    );
    let (r, py) = match design.test {
        PowerTest::TwoSampleT | PowerTest::OneSampleT | PowerTest::PairedT => {
            let kind = match design.test {
                PowerTest::TwoSampleT => "two.sample",
                PowerTest::OneSampleT => "one.sample",
                _ => "paired",
            };
            let class = if design.test == PowerTest::TwoSampleT {
                "TTestIndPower"
            } else {
                "TTestPower"
            };
            (
                format!(
                    "pwr::pwr.t.test(n = {r_n}, d = {r_es}, sig.level = {r_a}, power = {r_p}, \
                     type = \"{kind}\", alternative = \"{alt}\")"
                ),
                format!(
                    "from statsmodels.stats.power import {class}; {class}().solve_power(\
                     effect_size={py_es}, nobs1={py_n}, alpha={py_a}, power={py_p}, \
                     alternative='{alt_py}')"
                )
                .replace(
                    "nobs1=",
                    if class == "TTestPower" {
                        "nobs="
                    } else {
                        "nobs1="
                    },
                ),
            )
        }
        PowerTest::Anova => (
            format!(
                "pwr::pwr.anova.test(k = {}, n = {r_n}, f = {r_es}, sig.level = {r_a}, power = {r_p})",
                design.groups
            ),
            format!(
                "from statsmodels.stats.power import FTestAnovaPower; FTestAnovaPower().\
                 solve_power(effect_size={py_es}, nobs={}, alpha={py_a}, power={py_p}, \
                 k_groups={})  # nobs counts all groups",
                if target == PowerTarget::N {
                    "None".to_owned()
                } else {
                    fmt(n * design.groups)
                },
                design.groups
            ),
        ),
        PowerTest::TwoProportions => (
            format!(
                "pwr::pwr.2p.test(h = {r_es}, n = {r_n}, sig.level = {r_a}, power = {r_p}, \
                 alternative = \"{alt}\")"
            ),
            format!(
                "from statsmodels.stats.power import NormalIndPower; NormalIndPower().\
                 solve_power(effect_size={py_es}, nobs1={py_n}, alpha={py_a}, power={py_p}, \
                 ratio=1, alternative='{alt_py}')"
            ),
        ),
        PowerTest::OneProportion => (
            format!(
                "pwr::pwr.p.test(h = {r_es}, n = {r_n}, sig.level = {r_a}, power = {r_p}, \
                 alternative = \"{alt}\")"
            ),
            format!(
                "from statsmodels.stats.power import NormalIndPower; NormalIndPower().\
                 solve_power(effect_size={py_es}, nobs1={py_n}, alpha={py_a}, power={py_p}, \
                 ratio=0, alternative='{alt_py}')"
            ),
        ),
    };
    computed.reproduce.r = Some(r);
    computed.reproduce.python = Some(py);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_power_curve_rises_with_n() {
        let design = Design {
            test: PowerTest::TwoSampleT,
            groups: 2.0,
            alternative: Alternative::TwoSided,
        };
        let c = curve(&design, 25.5, 0.8, 0.05);
        assert_eq!(c.x.len(), CURVE_POINTS);
        assert!(c.y.windows(2).all(|w| w[0] <= w[1] + 1e-12));
        // Power at the size of the test itself is α (no effect).
        assert!((design.power(30.0, 1e-12, 0.05) - 0.05).abs() < 1e-6);
        assert!((cohens_h(0.65, 0.45) - 0.404_860_075_2).abs() < 1e-9);
    }
}
