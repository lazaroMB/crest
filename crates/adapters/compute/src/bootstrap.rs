//! The bootstrap card: B resamples of one statistic from a seeded
//! ChaCha20 stream, with percentile, BCa, basic and normal intervals.
//!
//! Conventions follow R's `boot`: quantiles of the replicates are read at
//! position (B + 1)·p, interpolating on the normal scale between order
//! statistics (`norm.inter`); BCa takes z₀ = Φ⁻¹(#{θ* < θ̂}/B) and the
//! acceleration from the jackknife (Efron 1987). Two-group statistics
//! resample each group on its own (strata); a correlation resamples pairs.
//!
//! The Monte Carlo error of an interval endpoint at probability p is half
//! the distance between the order statistics B·p ± √(B·p·(1 − p)) (the
//! binomial spread of the quantile's rank); that of the SE is
//! SE/√(2(B − 1)). Same seed, same numbers: a recompute matches exactly.

use application::ports::compute::{BootstrapRequest, ComputeError, DataColumn};
use domain::computed::{
    Assumption, AssumptionStatus, BootInterval, BootMethod, BootStatistic, BootstrapBody, Computed,
    ComputedBody, Narrative, UncertaintyKind, WarningSeverity,
};
use serde_json::Value;

use crate::data;
use crate::describe::{mean, variance};
use crate::distribution;
use crate::envelope::{self, auto_decimals, count, number, significant, warning, with_ci};
use crate::math::{pnorm, qnorm};
use crate::reproduce::{py_col, py_frame, r_col, r_frame};
use crate::rng::{RNG_NAME, Rng};

/// Most resamples.
pub const MAX_RESAMPLES: u32 = 100_000;
/// Most values drawn in total (resamples × n).
pub const MAX_DRAWS: f64 = 2e8;
/// Fewest resamples.
const MIN_RESAMPLES: u32 = 100;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// The data a statistic is computed from.
#[derive(Debug, Clone)]
enum Sample {
    /// One variable.
    One(Vec<f64>),
    /// Two groups.
    Two(Vec<f64>, Vec<f64>),
    /// Pairs.
    Pairs(Vec<f64>, Vec<f64>),
}

impl Sample {
    fn len(&self) -> usize {
        match self {
            Self::One(x) | Self::Pairs(x, _) => x.len(),
            Self::Two(a, b) => a.len() + b.len(),
        }
    }
}

fn median_of(xs: &mut [f64]) -> f64 {
    let n = xs.len();
    if n == 0 {
        return f64::NAN;
    }
    let mid = n / 2;
    let (_, &mut upper, _) = xs.select_nth_unstable_by(mid, f64::total_cmp);
    if n % 2 == 1 {
        upper
    } else {
        let lower = xs[..mid].iter().copied().fold(f64::NEG_INFINITY, f64::max);
        0.5 * (lower + upper)
    }
}

/// R's `mean(x, trim)`: drop ⌊n·trim⌋ values from each end.
fn trimmed_mean(xs: &mut [f64], trim: f64) -> f64 {
    xs.sort_by(f64::total_cmp);
    // A share of the length, floored.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let k = (xs.len() as f64 * trim).floor() as usize;
    if 2 * k >= xs.len() {
        return median_of(xs);
    }
    mean(&xs[k..xs.len() - k])
}

fn pearson(x: &[f64], y: &[f64]) -> f64 {
    let (mx, my) = (mean(x), mean(y));
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (a, b) in x.iter().zip(y) {
        let (dx, dy) = (a - mx, b - my);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    sxy / (sxx * syy).sqrt()
}

/// The statistic on `sample` (whose vectors it may reorder).
fn statistic(stat: BootStatistic, trim: f64, sample: &mut Sample) -> f64 {
    match (stat, sample) {
        (BootStatistic::Mean, Sample::One(x)) => mean(x),
        (BootStatistic::Median, Sample::One(x)) => median_of(x),
        (BootStatistic::Sd, Sample::One(x)) => variance(x).sqrt(),
        (BootStatistic::TrimmedMean, Sample::One(x)) => trimmed_mean(x, trim),
        (BootStatistic::MeanDifference, Sample::Two(a, b)) => mean(a) - mean(b),
        (BootStatistic::MedianDifference, Sample::Two(a, b)) => median_of(a) - median_of(b),
        (BootStatistic::Correlation, Sample::Pairs(x, y)) => pearson(x, y),
        _ => f64::NAN,
    }
}

/// One resample of `sample` into `out` (same shape).
fn resample(sample: &Sample, out: &mut Sample, rng: &mut Rng) {
    let draw = |src: &[f64], dst: &mut Vec<f64>, rng: &mut Rng| {
        dst.clear();
        dst.extend((0..src.len()).map(|_| src[rng.below(src.len())]));
    };
    match (sample, out) {
        (Sample::One(x), Sample::One(o)) => draw(x, o, rng),
        (Sample::Two(a, b), Sample::Two(oa, ob)) => {
            draw(a, oa, rng);
            draw(b, ob, rng);
        }
        (Sample::Pairs(x, y), Sample::Pairs(ox, oy)) => {
            ox.clear();
            oy.clear();
            for _ in 0..x.len() {
                let i = rng.below(x.len());
                ox.push(x[i]);
                oy.push(y[i]);
            }
        }
        _ => {}
    }
}

/// The statistic with each unit left out in turn.
fn jackknife(stat: BootStatistic, trim: f64, sample: &Sample) -> Vec<f64> {
    let without = |v: &[f64], i: usize| -> Vec<f64> {
        v.iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, x)| *x)
            .collect()
    };
    let mut out = Vec::with_capacity(sample.len());
    match sample {
        Sample::One(x) => {
            for i in 0..x.len() {
                out.push(statistic(stat, trim, &mut Sample::One(without(x, i))));
            }
        }
        Sample::Pairs(x, y) => {
            for i in 0..x.len() {
                out.push(statistic(
                    stat,
                    trim,
                    &mut Sample::Pairs(without(x, i), without(y, i)),
                ));
            }
        }
        Sample::Two(a, b) => {
            for i in 0..a.len() {
                out.push(statistic(
                    stat,
                    trim,
                    &mut Sample::Two(without(a, i), b.clone()),
                ));
            }
            for i in 0..b.len() {
                out.push(statistic(
                    stat,
                    trim,
                    &mut Sample::Two(a.clone(), without(b, i)),
                ));
            }
        }
    }
    out
}

/// The jackknife acceleration of BCa: Σ L³ / (6 (Σ L²)^{3/2}) with
/// L_i = mean − θ₍ᵢ₎.
pub fn acceleration(jack: &[f64]) -> f64 {
    let m = mean(jack);
    let (s2, s3) = jack.iter().fold((0.0, 0.0), |(s2, s3), t| {
        let l = m - t;
        (s2 + l * l, s3 + l * l * l)
    });
    s3 / (6.0 * s2.powf(1.5))
}

/// `boot`'s `norm.inter`: the replicate at probability `p` of sorted `t`.
pub fn norm_inter(t: &[f64], p: f64) -> f64 {
    let b = t.len();
    let rk = (b + 1) as f64 * p;
    let k = rk.floor();
    if (rk - k).abs() < 1e-9 * rk.max(1.0) {
        // An order statistic exactly; clamp to the sample.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (k as usize).clamp(1, b);
        return t[i - 1];
    }
    if k < 1.0 {
        return t[0];
    }
    if k >= b as f64 {
        return t[b - 1];
    }
    // Between the k-th and (k + 1)-th order statistics.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let i = k as usize;
    let n1 = (b + 1) as f64;
    let lo = qnorm(k / n1);
    let hi = qnorm((k + 1.0) / n1);
    t[i - 1] + (qnorm(p) - lo) / (hi - lo) * (t[i] - t[i - 1])
}

/// The Monte Carlo error of the quantile at `p` of sorted `t`.
pub fn quantile_mcse(t: &[f64], p: f64) -> f64 {
    let b = t.len() as f64;
    let spread = (b * p * (1.0 - p)).sqrt();
    let at = |r: f64| {
        // A rank within 1..=B.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (r.round().clamp(1.0, b) as usize) - 1;
        t[i]
    };
    (at(b * p + spread) - at(b * p - spread)) / 2.0
}

/// The sample of the request and a description of what it compares.
fn sample_of(
    request: &BootstrapRequest,
    dropped: &mut u64,
) -> Result<(Sample, Option<String>, Vec<DataColumn>), ComputeError> {
    let y = data::numeric(&request.data, &request.y)?;
    let mut names = vec![request.y.as_str()];
    let stat = request.statistic;
    let sample = if stat.two_groups() {
        let group = request.group.as_deref().ok_or_else(|| {
            invalid(format!(
                "`{}` needs `group`, a column with two levels",
                stat.label()
            ))
        })?;
        names.push(group);
        let (rows, mut levels) = data::labels(&request.data, group)?;
        if !request.levels.is_empty() {
            levels.clone_from(&request.levels);
        }
        if levels.len() != 2 {
            return Err(invalid(format!(
                "`{group}` has {} levels; name the two to compare in `levels`",
                levels.len()
            )));
        }
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for (v, l) in y.iter().zip(&rows) {
            match (v, l) {
                (Some(v), Some(l)) if *l == levels[0] => a.push(*v),
                (Some(v), Some(l)) if *l == levels[1] => b.push(*v),
                (Some(_), Some(_)) => {}
                _ => *dropped += 1,
            }
        }
        let versus = Some(format!("{group}: {} − {}", levels[0], levels[1]));
        (Sample::Two(a, b), versus)
    } else if stat == BootStatistic::Correlation {
        let y2 = request
            .y2
            .as_deref()
            .ok_or_else(|| invalid("`correlation` needs `y2`, a second numeric column"))?;
        names.push(y2);
        let other = data::numeric(&request.data, y2)?;
        let (mut x, mut z) = (Vec::new(), Vec::new());
        for (a, b) in y.iter().zip(&other) {
            match (a, b) {
                (Some(a), Some(b)) => {
                    x.push(*a);
                    z.push(*b);
                }
                _ => *dropped += 1,
            }
        }
        (Sample::Pairs(x, z), Some(y2.to_owned()))
    } else {
        *dropped = data::missing(&y);
        (Sample::One(data::present(&y)), None)
    };
    let used = data::used_columns(&request.data, &names);
    Ok((sample.0, sample.1, used))
}

/// Runs a bootstrap request.
pub fn run(
    request: &BootstrapRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    if !(MIN_RESAMPLES..=MAX_RESAMPLES).contains(&request.resamples) {
        return Err(invalid(format!(
            "`resamples` must be between {MIN_RESAMPLES} and {MAX_RESAMPLES}"
        )));
    }
    if !(request.conf_level > 0.0 && request.conf_level < 1.0) {
        return Err(invalid("`conf_level` must be between 0 and 1"));
    }
    if !(0.0..0.5).contains(&request.trim) {
        return Err(invalid("`trim` must be at least 0 and below 0.5"));
    }
    let mut dropped = 0;
    let (sample, versus, used) = sample_of(request, &mut dropped)?;
    let stat = request.statistic;
    let name = format!("Bootstrap of the {}", stat.label());
    let mut computed = Computed::new(id, "stats.bootstrap", name, envelope::engine());
    computed.uncertainty_kind = Some(UncertaintyKind::BootstrapCi);
    computed.seed = Some(request.seed);
    computed.rng = Some(RNG_NAME.into());
    computed
        .inputs
        .push(envelope::input(&request.data, &used, None));
    reproduce(&mut computed, request, &used);
    let b = request.resamples;
    let mut body = BootstrapBody {
        statistic: stat,
        variable: request.y.clone(),
        versus,
        observed: f64::NAN,
        resamples: b,
        conf_level: request.conf_level,
        histogram: domain::computed::Histogram {
            edges: vec![0.0, 1.0],
            counts: vec![0],
        },
        intervals: Vec::new(),
        z0: None,
        acceleration: None,
    };
    if dropped > 0 {
        computed.warnings.push(warning(
            "missing_dropped",
            WarningSeverity::Info,
            format!("{dropped} rows with missing values were left out"),
        ));
    }
    let too_small = match &sample {
        Sample::One(x) | Sample::Pairs(x, _) => x.len() < 3,
        Sample::Two(a, b) => a.len() < 2 || b.len() < 2,
    };
    if too_small {
        envelope::fail(&mut computed, "too few values to resample");
        return Ok((computed, ComputedBody::Bootstrap(body)));
    }
    let n = sample.len();
    if f64::from(b) * n as f64 > MAX_DRAWS {
        return Err(invalid(format!(
            "{b} resamples of {n} values is too many draws; use fewer resamples"
        )));
    }
    let observed = statistic(stat, request.trim, &mut sample.clone());
    body.observed = observed;
    if !observed.is_finite() {
        envelope::fail(
            &mut computed,
            format!("the {} cannot be computed on these data", stat.label()),
        );
        return Ok((computed, ComputedBody::Bootstrap(body)));
    }
    let mut rng = Rng::new(request.seed);
    let mut buffer = sample.clone();
    let mut t: Vec<f64> = (0..b)
        .map(|_| {
            resample(&sample, &mut buffer, &mut rng);
            statistic(stat, request.trim, &mut buffer)
        })
        .filter(|x| x.is_finite())
        .collect();
    let lost = b as usize - t.len();
    if lost > 0 {
        computed.warnings.push(warning(
            "undefined_replicates",
            WarningSeverity::Warn,
            format!("{lost} resamples gave no value (e.g. a constant resample) and were left out"),
        ));
    }
    if t.len() < MIN_RESAMPLES as usize {
        envelope::fail(&mut computed, "too few resamples gave a value");
        return Ok((computed, ComputedBody::Bootstrap(body)));
    }
    t.sort_by(f64::total_cmp);
    let level = request.conf_level;
    let alpha = 1.0 - level;
    let (pl, ph) = (alpha / 2.0, 1.0 - alpha / 2.0);
    let se = variance(&t).sqrt();
    let bias = mean(&t) - observed;
    let z = qnorm(ph);
    let below = t.iter().filter(|x| **x < observed).count() as f64;
    let z0 = qnorm(below / t.len() as f64);
    let a = acceleration(&jackknife(stat, request.trim, &sample));
    let percentile = BootInterval {
        method: BootMethod::Percentile,
        lo: norm_inter(&t, pl),
        hi: norm_inter(&t, ph),
        lo_mcse: Some(quantile_mcse(&t, pl)),
        hi_mcse: Some(quantile_mcse(&t, ph)),
    };
    let bca = (z0.is_finite() && a.is_finite()).then(|| {
        let adjust = |zq: f64| pnorm(z0 + (z0 + zq) / (1.0 - a * (z0 + zq)));
        let (ql, qh) = (adjust(qnorm(pl)), adjust(qnorm(ph)));
        BootInterval {
            method: BootMethod::Bca,
            lo: norm_inter(&t, ql),
            hi: norm_inter(&t, qh),
            lo_mcse: Some(quantile_mcse(&t, ql)),
            hi_mcse: Some(quantile_mcse(&t, qh)),
        }
    });
    let basic = BootInterval {
        method: BootMethod::Basic,
        lo: 2.0 * observed - percentile.hi,
        hi: 2.0 * observed - percentile.lo,
        lo_mcse: percentile.hi_mcse,
        hi_mcse: percentile.lo_mcse,
    };
    let normal = BootInterval {
        method: BootMethod::Normal,
        lo: observed - bias - z * se,
        hi: observed - bias + z * se,
        lo_mcse: None,
        hi_mcse: None,
    };
    let headline = bca.clone().unwrap_or_else(|| percentile.clone());
    let decimals = auto_decimals(se).max(2);
    let value = |interval: &BootInterval| {
        with_ci(
            number(observed, decimals),
            level,
            interval.lo,
            interval.hi,
            interval.method.label(),
        )
    };
    let values = &mut computed.values;
    values.insert("estimate".into(), value(&headline));
    values.insert("ci_percentile".into(), value(&percentile));
    if let Some(bca) = &bca {
        values.insert("ci_bca".into(), value(bca));
    }
    values.insert("ci_basic".into(), value(&basic));
    values.insert("ci_normal".into(), value(&normal));
    values.insert("se".into(), number(se, decimals));
    values.insert("bias".into(), number(bias, decimals));
    values.insert(
        "se_mcse".into(),
        significant(se / (2.0 * (t.len() as f64 - 1.0)).sqrt(), 2),
    );
    if let (Some(lo), Some(hi)) = (headline.lo_mcse, headline.hi_mcse) {
        values.insert("lo_mcse".into(), significant(lo, 2));
        values.insert("hi_mcse".into(), significant(hi, 2));
    }
    values.insert("resamples".into(), count(t.len() as u64));
    values.insert("n".into(), count(n as u64));
    if z0.is_finite() {
        values.insert("z0".into(), number(z0, 3));
    }
    if a.is_finite() {
        values.insert("acceleration".into(), number(a, 4));
    }
    let (edges, _, _) = distribution::bins(&t, domain::computed::BinRule::FreedmanDiaconis, None);
    body.histogram = distribution::histogram(&t, &edges);
    body.z0 = z0.is_finite().then_some(z0);
    body.acceleration = a.is_finite().then_some(a);
    body.intervals = [Some(percentile), bca.clone(), Some(basic), Some(normal)]
        .into_iter()
        .flatten()
        .collect();
    let params = &mut computed.method.params;
    params.insert("statistic".into(), Value::from(stat.label()));
    params.insert("resamples".into(), Value::from(b));
    params.insert("conf_level".into(), Value::from(level));
    params.insert(
        "strata".into(),
        Value::from(if stat.two_groups() { "groups" } else { "none" }),
    );
    computed.method.why = Some(if bca.is_some() {
        "BCa corrects the percentile interval for bias and skew".into()
    } else {
        "BCa is undefined here (z₀ or the acceleration is infinite); percentile shown".into()
    });
    computed.method.alternatives = vec!["percentile".into(), "basic".into(), "normal".into()];
    computed.assumptions = vec![Assumption {
        id: "independence".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some(
            "independent, identically distributed units; resample clusters or blocks otherwise"
                .into(),
        ),
    }];
    if b < 1000 {
        computed.warnings.push(warning(
            "few_resamples",
            WarningSeverity::Warn,
            format!("{b} resamples: interval endpoints need at least 1000 (see their MCSE)"),
        ));
    }
    if n < 10 {
        computed.warnings.push(warning(
            "small_n",
            WarningSeverity::Warn,
            format!(
                "only {n} values: the bootstrap distribution is coarse and intervals too narrow"
            ),
        ));
    }
    if matches!(
        stat,
        BootStatistic::Median | BootStatistic::MedianDifference
    ) {
        computed.warnings.push(warning(
            "discrete_statistic",
            WarningSeverity::Info,
            "the median takes few distinct values under resampling: endpoints move in steps",
        ));
    }
    computed.narrative = Some(Narrative {
        template: "bootstrap".into(),
        text_slots: ["estimate", "se", "bias", "resamples"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    });
    Ok((computed, ComputedBody::Bootstrap(body)))
}

fn reproduce(computed: &mut Computed, request: &BootstrapRequest, used: &[DataColumn]) {
    let stat_r = match request.statistic {
        BootStatistic::Mean => "function(d, i) mean(d$y[i])".to_owned(),
        BootStatistic::Median => "function(d, i) median(d$y[i])".to_owned(),
        BootStatistic::Sd => "function(d, i) sd(d$y[i])".to_owned(),
        BootStatistic::TrimmedMean => {
            format!("function(d, i) mean(d$y[i], trim = {})", request.trim)
        }
        BootStatistic::MeanDifference => {
            "function(d, i) { s <- d[i, ]; diff(rev(tapply(s$y, s$g, mean))) }".to_owned()
        }
        BootStatistic::MedianDifference => {
            "function(d, i) { s <- d[i, ]; diff(rev(tapply(s$y, s$g, median))) }".to_owned()
        }
        BootStatistic::Correlation => "function(d, i) cor(d$y[i], d$y2[i])".to_owned(),
    };
    let mut frame = format!("d <- data.frame(y = {}", r_col(&request.y));
    if let Some(g) = &request.group {
        frame.push_str(&format!(", g = {}", r_col(g)));
    }
    if let Some(y2) = &request.y2 {
        frame.push_str(&format!(", y2 = {}", r_col(y2)));
    }
    frame.push(')');
    let strata = if request.statistic.two_groups() {
        ", strata = d$g"
    } else {
        ""
    };
    computed.reproduce.r = Some(format!(
        "{}; library(boot); {frame}; set.seed({}); b <- boot(na.omit(d), {stat_r}, R = {}{strata}); \
         boot.ci(b, conf = {}, type = c(\"perc\", \"bca\", \"basic\", \"norm\"))  # another \
         random stream: endpoints differ within their MCSE",
        r_frame(&request.data, used),
        request.seed,
        request.resamples,
        request.conf_level
    ));
    let py_stat = match request.statistic {
        BootStatistic::Mean => Some("np.mean"),
        BootStatistic::Median => Some("np.median"),
        BootStatistic::Sd => Some("lambda x, axis: np.std(x, ddof=1, axis=axis)"),
        _ => None,
    };
    if let Some(py_stat) = py_stat {
        computed.reproduce.python = Some(format!(
            "{}; import numpy as np, scipy.stats as st; x = {}.dropna().to_numpy(); \
             st.bootstrap((x,), {py_stat}, n_resamples={}, confidence_level={}, method='BCa', \
             rng={})",
            py_frame(&request.data, used),
            py_col(&request.y),
            request.resamples,
            request.conf_level,
            request.seed
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_acceleration_of_the_mean_is_its_skewness_term() {
        // For the mean, L_i = (x_i − x̄)/(n − 1): a = Σd³ / (6 (Σd²)^{3/2}).
        let xs = [1.0, 2.0, 2.0, 3.0, 7.0, 11.0];
        let jack = jackknife(BootStatistic::Mean, 0.0, &Sample::One(xs.to_vec()));
        let m = mean(&xs);
        let (s2, s3) = xs.iter().fold((0.0, 0.0), |(a, b), x| {
            (a + (x - m).powi(2), b + (x - m).powi(3))
        });
        let expected = s3 / (6.0 * s2.powf(1.5));
        assert!((acceleration(&jack) - expected).abs() < 1e-12);
    }

    #[test]
    fn norm_inter_reads_order_statistics() {
        let t: Vec<f64> = (1..=1999).map(f64::from).collect();
        // (B + 1)·0.025 = 50: the 50th order statistic exactly.
        assert_eq!(norm_inter(&t, 0.025), 50.0);
        assert_eq!(norm_inter(&t, 0.975), 1950.0);
        let between = norm_inter(&t, 0.0251);
        assert!(between > 50.0 && between < 51.0);
        assert!(quantile_mcse(&t, 0.5) > 0.0);
        let mut odd = [3.0, 1.0, 2.0];
        assert_eq!(median_of(&mut odd), 2.0);
        let mut even = [4.0, 1.0, 3.0, 2.0];
        assert_eq!(median_of(&mut even), 2.5);
        let mut trim = [1.0, 2.0, 3.0, 4.0, 100.0];
        assert_eq!(trimmed_mean(&mut trim, 0.2), 3.0);
    }
}
