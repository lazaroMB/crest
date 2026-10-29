//! The Monte Carlo card: independent uncertain inputs drawn from a seeded
//! ChaCha20 stream, pushed through a model written as a plot expression,
//! and summarised by mean ± MCSE, the 5th, 50th and 95th percentiles, a
//! histogram and the convergence of the running mean.
//!
//! The tornado moves one input at a time to its 10th and 90th percentiles
//! with the others at their medians (exact quantiles of each input, not
//! draws) and sorts the inputs by the swing of the output. Without a model
//! the output is the product of the inputs: a Fermi estimate. When every
//! factor is lognormal (or a positive constant) the product is lognormal
//! too, so its percentiles are also given exactly next to the simulated
//! ones.
//!
//! Draws are by inversion (uniform, normal, lognormal, triangular) and,
//! for PERT, as a beta from two Marsaglia-Tsang gamma draws.

use application::ports::compute::{ComputeError, SimDistribution, SimInput, SimulateRequest};
use domain::computed::{
    Assumption, AssumptionStatus, Computed, ComputedBody, Curve, Narrative, SimInputSummary,
    SimulationBody, TornadoBar, UncertaintyKind, WarningSeverity, apa,
};
use domain::plot::expr::Expr;
use serde_json::Value;

use crate::data::key_part;
use crate::describe::{mean, quantile, variance};
use crate::distribution;
use crate::envelope::{self, auto_decimals, count, number, significant, warning, with_ci};
use crate::exprs::{check_names, parse};
use crate::math::{inv_beta, qnorm};
use crate::rng::{RNG_NAME, Rng};

/// Most draws.
pub const MAX_SAMPLES: u32 = 1_000_000;
/// Fewest draws.
const MIN_SAMPLES: u32 = 100;
/// Most inputs.
pub const MAX_INPUTS: usize = 100;
/// Points of the convergence trace.
const TRACE_POINTS: usize = 60;
/// z of the 95th percentile, for the 90% ranges of lognormal inputs.
const Z95: f64 = 1.644_853_626_951_472_2;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// Checks a distribution's parameters.
fn check(input: &SimInput) -> Result<(), String> {
    let name = &input.name;
    let finite = |xs: &[f64]| xs.iter().all(|x| x.is_finite());
    let ok = match input.distribution {
        SimDistribution::Fixed { value } => finite(&[value]),
        SimDistribution::Uniform { min, max } => finite(&[min, max]) && min < max,
        SimDistribution::Normal { mean, sd } => finite(&[mean, sd]) && sd > 0.0,
        SimDistribution::Lognormal { low, high } => finite(&[low, high]) && low > 0.0 && low < high,
        SimDistribution::Triangular { min, mode, max }
        | SimDistribution::Pert { min, mode, max } => {
            finite(&[min, mode, max]) && min <= mode && mode <= max && min < max
        }
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "input {name}: bad parameters (need min < max, mode within them, sd > 0, 0 < low < high)"
        ))
    }
}

/// The log-scale parameters of a lognormal with 90% in [low, high].
fn log_params(low: f64, high: f64) -> (f64, f64) {
    let (a, b) = (low.ln(), high.ln());
    (0.5 * (a + b), (b - a) / (2.0 * Z95))
}

/// The shapes of a PERT distribution.
fn pert_shapes(min: f64, mode: f64, max: f64) -> (f64, f64) {
    let range = max - min;
    (
        1.0 + 4.0 * (mode - min) / range,
        1.0 + 4.0 * (max - mode) / range,
    )
}

/// The exact `p` quantile of a distribution.
pub fn quantile_of(d: &SimDistribution, p: f64) -> f64 {
    match *d {
        SimDistribution::Fixed { value } => value,
        SimDistribution::Uniform { min, max } => min + p * (max - min),
        SimDistribution::Normal { mean, sd } => mean + sd * qnorm(p),
        SimDistribution::Lognormal { low, high } => {
            let (mu, sigma) = log_params(low, high);
            (mu + sigma * qnorm(p)).exp()
        }
        SimDistribution::Triangular { min, mode, max } => {
            let range = max - min;
            let fc = (mode - min) / range;
            if p < fc {
                min + (p * range * (mode - min)).sqrt()
            } else {
                max - ((1.0 - p) * range * (max - mode)).sqrt()
            }
        }
        SimDistribution::Pert { min, mode, max } => {
            let (a, b) = pert_shapes(min, mode, max);
            min + (max - min) * inv_beta(a, b, p)
        }
    }
}

/// A gamma(`shape`, 1) draw, Marsaglia and Tsang (2000).
fn gamma(rng: &mut Rng, shape: f64) -> f64 {
    if shape < 1.0 {
        let u = rng.uniform();
        return gamma(rng, shape + 1.0) * u.powf(1.0 / shape);
    }
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        let x = rng.normal();
        let v = (1.0 + c * x).powi(3);
        if v <= 0.0 {
            continue;
        }
        let u = rng.uniform();
        if u.ln() < 0.5 * x * x + d - d * v + d * v.ln() {
            return d * v;
        }
    }
}

/// One draw of a distribution.
pub(crate) fn draw(d: &SimDistribution, rng: &mut Rng) -> f64 {
    match *d {
        SimDistribution::Fixed { value } => value,
        SimDistribution::Normal { mean, sd } => mean + sd * rng.normal(),
        SimDistribution::Lognormal { low, high } => {
            let (mu, sigma) = log_params(low, high);
            (mu + sigma * rng.normal()).exp()
        }
        SimDistribution::Pert { min, mode, max } => {
            let (a, b) = pert_shapes(min, mode, max);
            let (x, y) = (gamma(rng, a), gamma(rng, b));
            min + (max - min) * x / (x + y)
        }
        SimDistribution::Uniform { .. } | SimDistribution::Triangular { .. } => {
            quantile_of(d, rng.uniform())
        }
    }
}

/// The distribution in words.
fn describe(d: &SimDistribution) -> String {
    let n = |x: f64| apa::significant(x, 4);
    match *d {
        SimDistribution::Fixed { value } => format!("fixed at {}", n(value)),
        SimDistribution::Uniform { min, max } => format!("uniform on [{}, {}]", n(min), n(max)),
        SimDistribution::Normal { mean, sd } => format!("normal, mean {}, SD {}", n(mean), n(sd)),
        SimDistribution::Lognormal { low, high } => {
            format!("lognormal, 90% in [{}, {}]", n(low), n(high))
        }
        SimDistribution::Triangular { min, mode, max } => {
            format!("triangular [{}, {}, {}]", n(min), n(mode), n(max))
        }
        SimDistribution::Pert { min, mode, max } => {
            format!("PERT [{}, {}, {}]", n(min), n(mode), n(max))
        }
    }
}

/// The model: an expression of the inputs, or their product.
enum Model {
    Expr(Expr),
    Product,
}

impl Model {
    fn eval(&self, v: &[f64]) -> f64 {
        match self {
            Self::Expr(e) => e.eval(v),
            Self::Product => v.iter().product(),
        }
    }
}

/// Runs a simulation request.
pub fn run(
    request: &SimulateRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    if request.inputs.is_empty() || request.inputs.len() > MAX_INPUTS {
        return Err(invalid(format!("give 1 to {MAX_INPUTS} inputs")));
    }
    if !(MIN_SAMPLES..=MAX_SAMPLES).contains(&request.samples) {
        return Err(invalid(format!(
            "`samples` must be between {MIN_SAMPLES} and {MAX_SAMPLES}"
        )));
    }
    let names: Vec<&str> = request.inputs.iter().map(|i| i.name.as_str()).collect();
    check_names(names.iter().copied()).map_err(invalid)?;
    for input in &request.inputs {
        check(input).map_err(invalid)?;
    }
    let model = match &request.model {
        Some(text) => Model::Expr(parse(text, &names).map_err(invalid)?),
        None => Model::Product,
    };
    let fermi = request.model.is_none();
    let model_text = request.model.clone().unwrap_or_else(|| names.join(" × "));
    let output = request.output.clone().unwrap_or_else(|| "result".into());
    let mut engine = envelope::solve_engine();
    engine.backend = Some("crest Monte Carlo (ChaCha20)".into());
    let (kind, name) = if fermi {
        ("solve.fermi", "Fermi estimate")
    } else {
        ("solve.monte_carlo", "Monte Carlo simulation")
    };
    let mut computed = Computed::new(id, kind, name, engine);
    computed.uncertainty_kind = Some(UncertaintyKind::MonteCarlo);
    computed.seed = Some(request.seed);
    computed.rng = Some(RNG_NAME.into());
    computed.inputs.push(domain::computed::InputRef {
        name: "model".into(),
        role: Some("param".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: None,
        columns: names.iter().map(|n| (*n).to_owned()).collect(),
        filter: None,
    });
    // Draws.
    let mut rng = Rng::new(request.seed);
    let k = request.inputs.len();
    let mut point = vec![0.0; k];
    let mut out: Vec<f64> = Vec::with_capacity(request.samples as usize);
    let mut undefined = 0u64;
    for _ in 0..request.samples {
        for (slot, input) in point.iter_mut().zip(&request.inputs) {
            *slot = draw(&input.distribution, &mut rng);
        }
        let y = model.eval(&point);
        if y.is_finite() {
            out.push(y);
        } else {
            undefined += 1;
        }
    }
    let medians: Vec<f64> = request
        .inputs
        .iter()
        .map(|i| quantile_of(&i.distribution, 0.5))
        .collect();
    let base = model.eval(&medians);
    let summaries: Vec<SimInputSummary> = request
        .inputs
        .iter()
        .map(|i| SimInputSummary {
            name: i.name.clone(),
            distribution: describe(&i.distribution),
            p10: quantile_of(&i.distribution, 0.1),
            p50: quantile_of(&i.distribution, 0.5),
            p90: quantile_of(&i.distribution, 0.9),
            source: i.source.clone(),
            unit: i.unit.clone(),
        })
        .collect();
    let mut tornado: Vec<TornadoBar> = summaries
        .iter()
        .enumerate()
        .filter(|(_, s)| s.p10 != s.p90)
        .map(|(j, s)| {
            let at = |v: f64| {
                let mut x = medians.clone();
                x[j] = v;
                model.eval(&x)
            };
            let (lo, hi) = (at(s.p10), at(s.p90));
            TornadoBar {
                input: s.name.clone(),
                at_p10: lo,
                at_p90: hi,
                swing: (hi - lo).abs(),
            }
        })
        .filter(|b| b.swing.is_finite())
        .collect();
    tornado.sort_by(|a, b| b.swing.total_cmp(&a.swing));
    let mut body = SimulationBody {
        output: output.clone(),
        unit: request.unit.clone(),
        model: model_text,
        fermi,
        samples: request.samples,
        inputs: summaries,
        histogram: domain::computed::Histogram {
            edges: vec![0.0, 1.0],
            counts: vec![0],
        },
        convergence: Curve::default(),
        convergence_mcse: Vec::new(),
        base,
        tornado,
    };
    reproduce(&mut computed, request, &names);
    if out.len() < MIN_SAMPLES as usize {
        envelope::fail(
            &mut computed,
            "the model is undefined (NaN or infinite) for most draws",
        );
        return Ok((computed, ComputedBody::Simulation(body)));
    }
    if undefined > 0 {
        computed.warnings.push(warning(
            "undefined_draws",
            WarningSeverity::Warn,
            format!("{undefined} draws gave no finite output and were left out"),
        ));
    }
    // Convergence of the running mean (Welford), at log-spaced counts.
    let total = out.len();
    let mut marks: Vec<usize> = (0..TRACE_POINTS)
        .map(|i| {
            let f = (total as f64).powf((i + 1) as f64 / TRACE_POINTS as f64);
            // Rounded counts within 1..=total.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let m = f.round() as usize;
            m.clamp(1, total)
        })
        .filter(|m| *m >= 10)
        .collect();
    marks.dedup();
    let (mut m, mut s2) = (0.0, 0.0);
    let mut next = marks.iter().peekable();
    for (i, &y) in out.iter().enumerate() {
        let n = (i + 1) as f64;
        let d = y - m;
        m += d / n;
        s2 += d * (y - m);
        if next.peek().is_some_and(|&&mark| mark == i + 1) {
            next.next();
            body.convergence.x.push(n);
            body.convergence.y.push(m);
            body.convergence_mcse
                .push((s2 / (n - 1.0)).sqrt() / n.sqrt());
        }
    }
    let mut sorted = out.clone();
    sorted.sort_by(f64::total_cmp);
    let (edges, _, _) =
        distribution::bins(&sorted, domain::computed::BinRule::FreedmanDiaconis, None);
    body.histogram = distribution::histogram(&sorted, &edges);
    let (mu, sd) = (mean(&out), variance(&out).sqrt());
    let mcse = sd / (total as f64).sqrt();
    let decimals = auto_decimals(sd);
    let values = &mut computed.values;
    values.insert(
        "mean".into(),
        with_ci(
            number(mu, decimals),
            0.95,
            mu - 1.96 * mcse,
            mu + 1.96 * mcse,
            "monte_carlo_se",
        ),
    );
    values.insert("sd".into(), number(sd, decimals));
    values.insert("mcse".into(), significant(mcse, 2));
    for (key, p) in [
        ("p5", 0.05),
        ("p10", 0.1),
        ("p50", 0.5),
        ("p90", 0.9),
        ("p95", 0.95),
    ] {
        values.insert(key.into(), number(quantile(&sorted, p), decimals));
    }
    values.insert("base".into(), number(base, decimals));
    values.insert("samples".into(), count(total as u64));
    for bar in &body.tornado {
        values.insert(
            format!("swing.{}", key_part(&bar.input)),
            number(bar.swing, decimals),
        );
    }
    // A product of lognormals is lognormal: exact percentiles.
    let lognormal = fermi
        && request.inputs.iter().all(|i| match i.distribution {
            SimDistribution::Lognormal { .. } => true,
            SimDistribution::Fixed { value } => value > 0.0,
            _ => false,
        });
    if lognormal {
        let (mut mu_log, mut var_log) = (0.0, 0.0);
        for input in &request.inputs {
            match input.distribution {
                SimDistribution::Lognormal { low, high } => {
                    let (m, s) = log_params(low, high);
                    mu_log += m;
                    var_log += s * s;
                }
                SimDistribution::Fixed { value } => mu_log += value.ln(),
                _ => {}
            }
        }
        let s = var_log.sqrt();
        for (key, z) in [
            ("analytic_p5", -Z95),
            ("analytic_p50", 0.0),
            ("analytic_p95", Z95),
        ] {
            values.insert(key.into(), number((mu_log + z * s).exp(), decimals));
        }
        values.insert(
            "analytic_mean".into(),
            number((mu_log + 0.5 * var_log).exp(), decimals),
        );
        computed.method.params.insert(
            "analytic".into(),
            Value::from("a product of lognormal factors is lognormal; exact percentiles given"),
        );
    }
    let params = &mut computed.method.params;
    params.insert("samples".into(), Value::from(request.samples));
    params.insert("model".into(), Value::from(body.model.clone()));
    params.insert(
        "tornado".into(),
        Value::from("each input at its 10th and 90th percentile, the others at their medians"),
    );
    computed.method.why = Some(if fermi {
        "no model given: the output is the product of the inputs (a Fermi estimate)".into()
    } else {
        "the inputs are drawn independently and pushed through the model".into()
    });
    computed.assumptions.push(Assumption {
        id: "independent_inputs".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some(
            "the inputs are drawn independently; correlated inputs widen or narrow the output"
                .into(),
        ),
    });
    if fermi && request.inputs.iter().any(|i| i.source.is_none()) {
        let unsourced: Vec<&str> = request
            .inputs
            .iter()
            .filter(|i| i.source.is_none())
            .map(|i| i.name.as_str())
            .collect();
        computed.warnings.push(warning(
            "unsourced_estimates",
            WarningSeverity::Info,
            format!("no source for: {}", unsourced.join(", ")),
        ));
    }
    computed.narrative = Some(Narrative {
        template: "simulation".into(),
        text_slots: ["mean", "mcse", "p5", "p50", "p95"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    });
    Ok((computed, ComputedBody::Simulation(body)))
}

fn reproduce(computed: &mut Computed, request: &SimulateRequest, names: &[&str]) {
    let n = request.samples;
    let draws: Vec<String> = request
        .inputs
        .iter()
        .map(|i| {
            let draw = match i.distribution {
                SimDistribution::Fixed { value } => format!("np.full({n}, {value})"),
                SimDistribution::Uniform { min, max } => format!("rng.uniform({min}, {max}, {n})"),
                SimDistribution::Normal { mean, sd } => format!("rng.normal({mean}, {sd}, {n})"),
                SimDistribution::Lognormal { low, high } => {
                    let (m, s) = log_params(low, high);
                    format!("rng.lognormal({m}, {s}, {n})")
                }
                SimDistribution::Triangular { min, mode, max } => {
                    format!("rng.triangular({min}, {mode}, {max}, {n})")
                }
                SimDistribution::Pert { min, mode, max } => {
                    let (a, b) = pert_shapes(min, mode, max);
                    format!("{min} + ({max} - {min}) * rng.beta({a}, {b}, {n})")
                }
            };
            format!("{} = {draw}", i.name)
        })
        .collect();
    let model = request
        .model
        .clone()
        .map(|m| m.replace('^', "**"))
        .unwrap_or_else(|| names.join(" * "));
    computed.reproduce.python = Some(format!(
        "import numpy as np; rng = np.random.default_rng({}); {}; y = {model}; \
         y.mean(), np.percentile(y, [5, 50, 95])  # another random stream: values agree within \
         their MCSE",
        request.seed,
        draws.join("; ")
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_of_the_distributions() {
        let tri = SimDistribution::Triangular {
            min: 0.0,
            mode: 1.0,
            max: 2.0,
        };
        assert!((quantile_of(&tri, 0.5) - 1.0).abs() < 1e-12);
        let ln = SimDistribution::Lognormal {
            low: 2.0,
            high: 8.0,
        };
        assert!((quantile_of(&ln, 0.05) - 2.0).abs() < 1e-9);
        assert!((quantile_of(&ln, 0.5) - 4.0).abs() < 1e-9);
        let pert = SimDistribution::Pert {
            min: 0.0,
            mode: 5.0,
            max: 10.0,
        };
        assert!((quantile_of(&pert, 0.5) - 5.0).abs() < 1e-9);
        // Gamma draws have mean equal to the shape.
        let mut rng = Rng::new(3);
        let m = (0..20_000).map(|_| gamma(&mut rng, 2.5)).sum::<f64>() / 20_000.0;
        assert!((m - 2.5).abs() < 0.05, "{m}");
    }
}
