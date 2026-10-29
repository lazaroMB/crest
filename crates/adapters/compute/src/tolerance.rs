//! The tolerance stack-up card: worst case, RSS and a seeded Monte Carlo
//! of a one-dimensional chain, with each dimension's contribution.
//!
//! Each dimension is centred in its band (`nominal + (plus − minus)/2`)
//! with half-width `t = (plus + minus)/2`. Worst case: centre ± Σt. RSS:
//! centre ± √Σt², the classic statistical stack, which reads each
//! tolerance as `sigmas` (3) standard deviations. The Monte Carlo draws
//! every dimension from a normal with σ = t/`sigmas` or a uniform over its
//! band, on the ChaCha20 stream of the Monte Carlo card (`rng`,
//! `simulate::draw`), and reports the stack's mean, standard deviation,
//! the 0.135% and 99.865% quantiles, and the share outside the limits
//! with its Monte Carlo standard error.

use application::ports::compute::{ComputeError, SimDistribution, ToleranceRequest};
use domain::computed::{
    Assumption, AssumptionStatus, Computed, ComputedBody, ComputedValue, Interval, Narrative,
    StackDimension, ToleranceBody, ToleranceDistribution, UncertaintyKind, WarningSeverity, apa,
};
use serde_json::Value;

use crate::distribution::{bins, histogram};
use crate::envelope::{self, auto_decimals, bounded, number, warning};
use crate::rng::{RNG_NAME, Rng};
use crate::simulate::draw;

/// Fewest and most draws.
const SAMPLES: (u32, u32) = (1_000, 2_000_000);
/// Most dimensions.
pub const MAX_DIMENSIONS: usize = 200;
/// Histogram bins.
const BINS: u32 = 50;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// Quantile `p` of sorted `xs` (R type 7).
fn quantile(xs: &[f64], p: f64) -> f64 {
    let h = (xs.len() - 1) as f64 * p;
    let (lo, hi) = (h.floor() as usize, h.ceil() as usize);
    xs[lo] + (h - h.floor()) * (xs[hi] - xs[lo])
}

/// Runs a tolerance request.
#[allow(clippy::too_many_lines)]
pub fn run(
    request: &ToleranceRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    if request.dimensions.is_empty() || request.dimensions.len() > MAX_DIMENSIONS {
        return Err(invalid(format!("give 1 to {MAX_DIMENSIONS} dimensions")));
    }
    if !(SAMPLES.0..=SAMPLES.1).contains(&request.samples) {
        return Err(invalid(format!(
            "`samples` must be between {} and {}",
            SAMPLES.0, SAMPLES.1
        )));
    }
    if !(request.sigmas.is_finite() && request.sigmas > 0.0) {
        return Err(invalid("`sigmas` must be a positive number"));
    }
    if let (Some(lo), Some(hi)) = (request.lower_limit, request.upper_limit)
        && lo >= hi
    {
        return Err(invalid("`lower_limit` must be below `upper_limit`"));
    }
    // Deviations of each dimension.
    let mut dims: Vec<StackDimension> = Vec::with_capacity(request.dimensions.len());
    for d in &request.dimensions {
        let (plus, minus) = match (d.tolerance, d.plus, d.minus) {
            (Some(t), None, None) => (t, t),
            (None, Some(p), Some(m)) => (p, m),
            (None, Some(p), None) => (p, 0.0),
            (None, None, Some(m)) => (0.0, m),
            (None, None, None) => {
                return Err(invalid(format!(
                    "{}: give `tolerance` or `plus`/`minus`",
                    d.name
                )));
            }
            _ => {
                return Err(invalid(format!(
                    "{}: give `tolerance` or `plus`/`minus`, not both",
                    d.name
                )));
            }
        };
        if ![d.nominal, plus, minus].iter().all(|x| x.is_finite()) || plus < 0.0 || minus < 0.0 {
            return Err(invalid(format!(
                "{}: the nominal must be a number and the deviations non-negative magnitudes \
                 (`minus: 0.02` for −0.02)",
                d.name
            )));
        }
        let half = (plus + minus) / 2.0;
        let sd = match d.distribution {
            ToleranceDistribution::Normal => half / request.sigmas,
            ToleranceDistribution::Uniform => half / 3f64.sqrt(),
        };
        dims.push(StackDimension {
            name: d.name.clone(),
            nominal: d.nominal,
            plus,
            minus,
            direction: d.direction,
            distribution: d.distribution,
            center: d.nominal + (plus - minus) / 2.0,
            half_width: half,
            sd,
            worst_case_share: 0.0,
            rss_share: 0.0,
            variance_share: 0.0,
        });
    }
    let nominal: f64 = dims.iter().map(|d| d.direction.sign() * d.nominal).sum();
    let center: f64 = dims.iter().map(|d| d.direction.sign() * d.center).sum();
    let sum_t: f64 = dims.iter().map(|d| d.half_width).sum();
    let sum_t2: f64 = dims.iter().map(|d| d.half_width * d.half_width).sum();
    let sum_var: f64 = dims.iter().map(|d| d.sd * d.sd).sum();
    for d in &mut dims {
        let share = |part: f64, all: f64| if all > 0.0 { part / all } else { 0.0 };
        d.worst_case_share = share(d.half_width, sum_t);
        d.rss_share = share(d.half_width * d.half_width, sum_t2);
        d.variance_share = share(d.sd * d.sd, sum_var);
    }
    let worst_case = [center - sum_t, center + sum_t];
    let rss_half = sum_t2.sqrt();
    let rss = [center - rss_half, center + rss_half];
    // Monte Carlo.
    let draws: Vec<SimDistribution> = dims
        .iter()
        .map(|d| match d.distribution {
            ToleranceDistribution::Normal => SimDistribution::Normal {
                mean: d.center,
                sd: d.sd,
            },
            ToleranceDistribution::Uniform => SimDistribution::Uniform {
                min: d.center - d.half_width,
                max: d.center + d.half_width,
            },
        })
        .collect();
    let mut rng = Rng::new(request.seed);
    let n = request.samples as usize;
    let mut stack: Vec<f64> = (0..n)
        .map(|_| {
            dims.iter()
                .zip(&draws)
                .map(|(d, dist)| {
                    if d.half_width == 0.0 {
                        d.center * d.direction.sign()
                    } else {
                        draw(dist, &mut rng) * d.direction.sign()
                    }
                })
                .sum()
        })
        .collect();
    let nf = n as f64;
    let mean = stack.iter().sum::<f64>() / nf;
    let sd = (stack.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / (nf - 1.0)).sqrt();
    let outside = |x: f64| {
        request.lower_limit.is_some_and(|lo| x < lo) || request.upper_limit.is_some_and(|hi| x > hi)
    };
    let out_count = stack.iter().filter(|x| outside(**x)).count();
    stack.sort_by(f64::total_cmp);
    let mc_range = [quantile(&stack, 0.001_35), quantile(&stack, 0.998_65)];
    let (edges, _, _) = bins(&stack, domain::computed::BinRule::Fixed, Some(BINS));
    let hist = histogram(&stack, &edges);
    // Envelope.
    let mut engine = envelope::solve_engine();
    engine.backend = Some("crest tolerance stack (ChaCha20 Monte Carlo)".into());
    let mut computed = Computed::new(id, "solve.tolerance", "Tolerance stack-up", engine);
    computed.uncertainty_kind = Some(UncertaintyKind::MonteCarlo);
    computed.seed = Some(request.seed);
    computed.rng = Some(RNG_NAME.into());
    computed.inputs.push(domain::computed::InputRef {
        name: "stack".into(),
        role: Some("param".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: Some(dims.len() as u64),
        columns: dims.iter().map(|d| d.name.clone()).collect(),
        filter: None,
    });
    let scale = dims
        .iter()
        .map(|d| d.half_width)
        .filter(|t| *t > 0.0)
        .fold(f64::INFINITY, f64::min);
    let decimals = auto_decimals(if scale.is_finite() { scale } else { 1.0 }) + 1;
    let length = |v: f64| {
        let mut value = number(v, decimals);
        value.unit = request.unit.clone();
        value
    };
    let values = &mut computed.values;
    values.insert("nominal".into(), length(nominal));
    values.insert("center".into(), length(center));
    values.insert("worst_case_min".into(), length(worst_case[0]));
    values.insert("worst_case_max".into(), length(worst_case[1]));
    values.insert("rss_min".into(), length(rss[0]));
    values.insert("rss_max".into(), length(rss[1]));
    let mut mc_mean = length(mean);
    mc_mean.ci = Some(Interval {
        level: 0.95,
        lo: mean - 1.96 * sd / nf.sqrt(),
        hi: mean + 1.96 * sd / nf.sqrt(),
        method: Some("monte_carlo_se".into()),
    });
    values.insert("mc_mean".into(), mc_mean);
    values.insert("mc_sd".into(), length(sd));
    values.insert("mc_low".into(), length(mc_range[0]));
    values.insert("mc_high".into(), length(mc_range[1]));
    let has_limits = request.lower_limit.is_some() || request.upper_limit.is_some();
    if has_limits {
        let p = out_count as f64 / nf;
        let se = (p * (1.0 - p) / nf).sqrt();
        let mut v: ComputedValue = bounded(p, 4);
        v.display = Some(format!("{}%", apa::number(p * 100.0, 3)));
        v.ci = Some(Interval {
            level: 0.95,
            lo: (p - 1.96 * se).max(0.0),
            hi: (p + 1.96 * se).min(1.0),
            method: Some("monte_carlo_se".into()),
        });
        values.insert("out_of_spec".into(), v);
        let fits = |[lo, hi]: [f64; 2]| {
            request.lower_limit.is_none_or(|l| lo >= l)
                && request.upper_limit.is_none_or(|u| hi <= u)
        };
        computed
            .method
            .params
            .insert("worst_case_fits".into(), Value::from(fits(worst_case)));
        computed
            .method
            .params
            .insert("rss_fits".into(), Value::from(fits(rss)));
        let cpk = match (request.lower_limit, request.upper_limit) {
            (Some(lo), Some(hi)) => Some(((hi - mean).min(mean - lo)) / (3.0 * sd)),
            (Some(lo), None) => Some((mean - lo) / (3.0 * sd)),
            (None, Some(hi)) => Some((hi - mean) / (3.0 * sd)),
            (None, None) => None,
        };
        if let Some(cpk) = cpk.filter(|c| c.is_finite()) {
            values.insert("cpk".into(), number(cpk, 2));
        }
        if !fits(worst_case) && fits(rss) {
            computed.warnings.push(warning(
                "worst_case_exceeds",
                WarningSeverity::Info,
                "the worst case falls outside the limits while RSS fits: acceptable only if \
                 the dimensions really vary independently around their centres",
            ));
        } else if !fits(rss) {
            computed.warnings.push(warning(
                "rss_exceeds",
                WarningSeverity::Warn,
                "even the RSS range falls outside the limits",
            ));
        }
    }
    let params = &mut computed.method.params;
    params.insert("sigmas".into(), Value::from(request.sigmas));
    params.insert("samples".into(), Value::from(request.samples));
    computed.method.why = Some(format!(
        "worst case adds the half-widths; RSS adds them in quadrature (each tolerance = {}σ); \
         the Monte Carlo draws {} stacks",
        apa::number(request.sigmas, 1),
        request.samples
    ));
    computed.method.alternatives = Vec::new();
    computed.assumptions.push(Assumption {
        id: "independence".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: Some(
            "RSS and Monte Carlo understate the spread of correlated dimensions".into(),
        ),
        note: Some("dimensions vary independently (no common setup, tool or thermal drift)".into()),
    });
    computed.assumptions.push(Assumption {
        id: "centred".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some(format!(
            "normal dimensions are centred in their bands with the tolerance at {}σ; use \
             `uniform` for a process that wanders within the band",
            apa::number(request.sigmas, 1)
        )),
    });
    computed.assumptions.push(Assumption {
        id: "linear_stack".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some(
            "one-dimensional chain: each dimension adds or subtracts with sensitivity ±1".into(),
        ),
    });
    computed.narrative = Some(Narrative {
        template: "tolerance".into(),
        text_slots: vec![
            "worst_case_min".into(),
            "worst_case_max".into(),
            "rss_min".into(),
            "rss_max".into(),
            "out_of_spec".into(),
        ],
    });
    let body = ToleranceBody {
        unit: request.unit.clone(),
        dimensions: dims,
        nominal,
        center,
        worst_case,
        rss,
        sigmas: request.sigmas,
        samples: request.samples,
        mc_mean: mean,
        mc_sd: sd,
        mc_range,
        histogram: hist,
        lower_limit: request.lower_limit,
        upper_limit: request.upper_limit,
    };
    Ok((computed, ComputedBody::Tolerance(body)))
}
