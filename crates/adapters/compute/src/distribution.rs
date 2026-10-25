//! The distribution card: histogram, Gaussian kernel density, box/violin,
//! normal QQ with a pointwise band and the ECDF with its DKW band.
//!
//! Conventions follow R: Freedman-Diaconis, Sturges and Scott bins over
//! the pooled range (one set of edges for every group), bandwidths
//! `bw.nrd0` (Silverman) and `bw.nrd` (Scott), the density evaluated on
//! 512 points from `min − 3h` to `max + 3h` (`density(cut = 3)`), type-7
//! quartiles and IQR, the box from Tukey's hinges with whiskers at 1.5
//! hinge spreads (`boxplot.stats`), Blom plotting positions for
//! the QQ plot. The QQ band is pointwise: the i-th order statistic of a
//! normal sample with the fitted mean and SD lies between the normal
//! quantiles of the Beta(i, n + 1 − i) quantiles at α/2 and 1 − α/2.

use application::ports::compute::{ComputeError, DistributionRequest};
use domain::computed::{
    Assumption, AssumptionStatus, BandwidthRule, BinRule, BoxSummary, Computed, ComputedBody,
    Curve, DistributionBody, DistributionGroup, EcdfStep, Histogram, Narrative, QqPoint,
    UncertaintyKind, WarningSeverity,
};
use serde_json::Value;
use statrs::function::beta::inv_beta_reg;

use crate::checks::shapiro_wilk;
use crate::data::{self, key_part};
use crate::describe::{mean, quantile, variance};
use crate::envelope::{self, auto_decimals, bounded, count, number, p_value, warning};
use crate::math::qnorm;
use crate::reproduce::{py_col, py_frame, py_str, r_col, r_frame, r_name, r_str};

/// Points of the density curve.
pub const KDE_POINTS: usize = 512;
/// Above this many values the density is computed from a binned sample.
const KDE_EXACT_MAX: usize = 20_000;
/// Bins of the binned density.
const KDE_BINS: usize = 2048;
/// Most points of the QQ plot and of the ECDF; beyond, evenly spaced
/// order statistics are kept.
pub const MAX_CURVE_POINTS: usize = 2000;
/// Most histogram bins.
pub const MAX_BINS: u32 = 200;

/// Smallest group drawn.
const MIN_GROUP: usize = 2;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// The shared bins: (edges, width) over `pooled` (sorted, non-empty).
fn bins(pooled: &[f64], rule: BinRule, fixed: Option<u32>) -> (Vec<f64>, f64, BinRule) {
    let (min, max) = (pooled[0], pooled[pooled.len() - 1]);
    if max == min {
        return (vec![min - 0.5, max + 0.5], 1.0, rule);
    }
    let n = pooled.len() as f64;
    let range = max - min;
    let sturges = n.log2().ceil() + 1.0;
    let (count, rule) = match (fixed, rule) {
        (Some(k), _) => (f64::from(k), BinRule::Fixed),
        (None, BinRule::Sturges | BinRule::Fixed) => (sturges, BinRule::Sturges),
        (None, BinRule::Scott) => {
            let width = 3.49 * variance(pooled).sqrt() / n.cbrt();
            ((range / width).ceil(), BinRule::Scott)
        }
        (None, BinRule::FreedmanDiaconis) => {
            let iqr = quantile(pooled, 0.75) - quantile(pooled, 0.25);
            if iqr > 0.0 {
                (
                    (range / (2.0 * iqr / n.cbrt())).ceil(),
                    BinRule::FreedmanDiaconis,
                )
            } else {
                (sturges, BinRule::Sturges)
            }
        }
    };
    // Clamped to 1..=MAX_BINS before the cast.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = count.clamp(1.0, f64::from(MAX_BINS)) as usize;
    let width = range / count as f64;
    let mut edges: Vec<f64> = (0..=count).map(|i| min + width * i as f64).collect();
    edges[count] = max;
    (edges, width, rule)
}

/// Counts of `xs` in the bins of `edges` (the last bin is closed).
fn histogram(xs: &[f64], edges: &[f64]) -> Histogram {
    let bins = edges.len() - 1;
    let (min, width) = (edges[0], (edges[bins] - edges[0]) / bins as f64);
    let mut counts = vec![0u64; bins];
    for x in xs {
        // The values lie within the edges, so the index is in range.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (((x - min) / width).floor().max(0.0) as usize).min(bins - 1);
        counts[i] += 1;
    }
    Histogram {
        edges: edges.to_vec(),
        counts,
    }
}

/// The bandwidth of `rule` for sorted `xs` (R's `bw.nrd0` / `bw.nrd`).
pub fn bandwidth(sorted: &[f64], rule: BandwidthRule) -> f64 {
    let n = sorted.len() as f64;
    let sd = variance(sorted).sqrt();
    let iqr = (quantile(sorted, 0.75) - quantile(sorted, 0.25)) / 1.34;
    let mut lo = sd.min(iqr);
    if lo.is_nan() || lo <= 0.0 {
        lo = [sd, sorted[0].abs(), 1.0]
            .into_iter()
            .find(|v| *v > 0.0)
            .unwrap_or(1.0);
    }
    let factor = if rule == BandwidthRule::Scott {
        1.06
    } else {
        0.9
    };
    factor * lo * n.powf(-0.2)
}

const INV_SQRT_2PI: f64 = 0.398_942_280_401_432_7;

/// The Gaussian kernel density of `xs` with bandwidth `h` on
/// [`KDE_POINTS`] points from `min − 3h` to `max + 3h`. Large samples are
/// binned first (linear binning on [`KDE_BINS`] bins); the second value
/// says whether that happened.
fn kde(sorted: &[f64], h: f64) -> (Curve, bool) {
    let (lo, hi) = (sorted[0] - 3.0 * h, sorted[sorted.len() - 1] + 3.0 * h);
    let step = (hi - lo) / (KDE_POINTS - 1) as f64;
    let grid: Vec<f64> = (0..KDE_POINTS).map(|i| lo + step * i as f64).collect();
    let n = sorted.len() as f64;
    let binned = sorted.len() > KDE_EXACT_MAX;
    // Points and their weights: the sample, or bin centres with counts.
    let (points, weights): (Vec<f64>, Vec<f64>) = if binned {
        let width = (hi - lo) / (KDE_BINS - 1) as f64;
        let mut w = vec![0.0; KDE_BINS];
        for x in sorted {
            let pos = (x - lo) / width;
            // `pos` lies in 0..KDE_BINS − 1 by construction of the range.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let i = (pos.floor().max(0.0) as usize).min(KDE_BINS - 2);
            let frac = pos - i as f64;
            w[i] += 1.0 - frac;
            w[i + 1] += frac;
        }
        ((0..KDE_BINS).map(|i| lo + width * i as f64).collect(), w)
    } else {
        (sorted.to_vec(), vec![1.0; sorted.len()])
    };
    let y = grid
        .iter()
        .map(|g| {
            points
                .iter()
                .zip(&weights)
                .map(|(x, w)| {
                    let z = (g - x) / h;
                    w * (-0.5 * z * z).exp()
                })
                .sum::<f64>()
                * INV_SQRT_2PI
                / (n * h)
        })
        .collect();
    (Curve { x: grid, y }, binned)
}

/// Tukey's five numbers of sorted, non-empty `xs`, as R's `fivenum`.
pub fn fivenum(sorted: &[f64]) -> [f64; 5] {
    let n = sorted.len() as f64;
    let n4 = ((n + 3.0) / 2.0).floor() / 2.0;
    [1.0, n4, (n + 1.0) / 2.0, n + 1.0 - n4, n].map(|d| {
        // 1-based positions within 1..=n.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (lo, hi) = (d.floor() as usize - 1, d.ceil() as usize - 1);
        0.5 * (sorted[lo] + sorted[hi])
    })
}

/// Tukey's box of sorted `xs`, as R's `boxplot.stats` (hinges, 1.5 hinge
/// spreads).
pub fn boxplot(sorted: &[f64]) -> BoxSummary {
    let [_, lower_hinge, median, upper_hinge, _] = fivenum(sorted);
    let reach = 1.5 * (upper_hinge - lower_hinge);
    let (lo_fence, hi_fence) = (lower_hinge - reach, upper_hinge + reach);
    let inside = || {
        sorted
            .iter()
            .copied()
            .filter(|x| *x >= lo_fence && *x <= hi_fence)
    };
    BoxSummary {
        lower_hinge,
        median,
        upper_hinge,
        whisker_lo: inside().next().unwrap_or(lower_hinge),
        whisker_hi: inside().next_back().unwrap_or(upper_hinge),
        outliers: sorted
            .iter()
            .copied()
            .filter(|x| *x < lo_fence || *x > hi_fence)
            .collect(),
    }
}

/// Up to [`MAX_CURVE_POINTS`] evenly spaced indices of `0..n`, first and
/// last included.
fn thin(n: usize) -> Vec<usize> {
    if n <= MAX_CURVE_POINTS {
        return (0..n).collect();
    }
    let last = (n - 1) as f64;
    let steps = (MAX_CURVE_POINTS - 1) as f64;
    // Rounded indices within 0..n.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut ix: Vec<usize> = (0..MAX_CURVE_POINTS)
        .map(|i| (last * i as f64 / steps).round() as usize)
        .collect();
    ix.dedup();
    ix
}

/// The normal QQ points of sorted `xs` with the pointwise band at `level`.
pub fn qq(sorted: &[f64], level: f64) -> Vec<QqPoint> {
    let n = sorted.len();
    let nf = n as f64;
    let (m, sd) = (mean(sorted), variance(sorted).sqrt());
    let alpha = 1.0 - level;
    thin(n)
        .into_iter()
        .map(|i| {
            let rank = (i + 1) as f64;
            let theoretical = qnorm((rank - 0.375) / (nf + 0.25));
            let (a, b) = (rank, nf + 1.0 - rank);
            let lo_p = inv_beta_reg(a, b, alpha / 2.0);
            let hi_p = inv_beta_reg(a, b, 1.0 - alpha / 2.0);
            QqPoint {
                theoretical,
                sample: sorted[i],
                lo: m + sd * qnorm(lo_p),
                hi: m + sd * qnorm(hi_p),
            }
        })
        .collect()
}

/// The ECDF steps of sorted `xs` (one per distinct value).
pub fn ecdf(sorted: &[f64]) -> Vec<EcdfStep> {
    let n = sorted.len() as f64;
    let mut steps: Vec<EcdfStep> = Vec::new();
    for (i, &x) in sorted.iter().enumerate() {
        let p = (i + 1) as f64 / n;
        match steps.last_mut() {
            Some(last) if last.x == x => last.p = p,
            _ => steps.push(EcdfStep { x, p }),
        }
    }
    thin(steps.len()).into_iter().map(|i| steps[i]).collect()
}

/// Half-width of the DKW band of an ECDF of `n` values at `level`.
pub fn dkw_epsilon(n: usize, level: f64) -> f64 {
    ((2.0 / (1.0 - level)).ln() / (2.0 * n as f64)).sqrt()
}

/// Runs a distribution request.
pub fn run(
    request: &DistributionRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    if !(request.conf_level > 0.0 && request.conf_level < 1.0) {
        return Err(invalid("`conf_level` must be between 0 and 1"));
    }
    if request.bins.is_some_and(|k| !(1..=MAX_BINS).contains(&k)) {
        return Err(invalid(format!("`bins` must be between 1 and {MAX_BINS}")));
    }
    if request
        .bandwidth
        .is_some_and(|h| !(h.is_finite() && h > 0.0))
    {
        return Err(invalid("`bandwidth` must be a positive number"));
    }
    let values = data::numeric(&request.data, &request.variable)?;
    let (labels, levels) = match &request.by {
        Some(by) => {
            let (rows, levels) = data::labels(&request.data, by)?;
            (Some(rows), levels)
        }
        None => (None, Vec::new()),
    };
    let mut dropped = 0u64;
    let mut groups: Vec<(Option<String>, Vec<f64>)> = match &labels {
        None => vec![(None, data::present(&values))],
        Some(_) => levels
            .iter()
            .map(|level| (Some(level.clone()), Vec::new()))
            .collect(),
    };
    if let Some(rows) = &labels {
        for (value, label) in values.iter().zip(rows) {
            match (value, label) {
                (Some(x), Some(label)) => {
                    if let Some(group) = groups.iter_mut().find(|(n, _)| n.as_ref() == Some(label))
                    {
                        group.1.push(*x);
                    }
                }
                _ => dropped += 1,
            }
        }
    } else {
        dropped = data::missing(&values);
    }

    let name = format!("Distribution of {}", request.variable);
    let mut computed = Computed::new(id, "stats.distribution", name, envelope::engine());
    computed.uncertainty_kind = Some(UncertaintyKind::FrequentistCi);
    let params = &mut computed.method.params;
    params.insert("conf_level".into(), Value::from(request.conf_level));
    params.insert("quantile_type".into(), Value::from(7));
    params.insert(
        "box".into(),
        Value::from("Tukey hinges, whiskers at 1.5 hinge spreads"),
    );
    let mut names: Vec<&str> = vec![request.variable.as_str()];
    if let Some(by) = &request.by {
        names.push(by);
    }
    let used = data::used_columns(&request.data, &names);
    computed
        .inputs
        .push(envelope::input(&request.data, &used, None));
    if dropped > 0 {
        computed.warnings.push(warning(
            "missing_dropped",
            WarningSeverity::Info,
            format!("{dropped} rows with missing values were left out"),
        ));
    }
    let small: Vec<String> = groups
        .iter()
        .filter(|(_, xs)| xs.len() < MIN_GROUP)
        .map(|(n, _)| n.clone().unwrap_or_default())
        .collect();
    groups.retain(|(_, xs)| xs.len() >= MIN_GROUP);
    if !small.is_empty() && request.by.is_some() {
        computed.warnings.push(warning(
            "group_too_small",
            WarningSeverity::Info,
            format!(
                "groups with fewer than {MIN_GROUP} values left out: {}",
                small.join(", ")
            ),
        ));
    }
    let unit = request
        .data
        .column(&request.variable)
        .and_then(|c| c.unit.clone());
    let mut body = DistributionBody {
        variable: request.variable.clone(),
        unit,
        by: request.by.clone(),
        bin_rule: request.bin_rule,
        bin_width: 0.0,
        bandwidth_rule: if request.bandwidth.is_some() {
            BandwidthRule::Fixed
        } else {
            request.bandwidth_rule
        },
        conf_level: request.conf_level,
        groups: Vec::new(),
    };
    reproduce(&mut computed, request, &used);
    if groups.is_empty() {
        envelope::fail(
            &mut computed,
            format!(
                "{} needs at least {MIN_GROUP} non-missing values",
                request.variable
            ),
        );
        return Ok((computed, ComputedBody::Distribution(body)));
    }
    for (_, xs) in &mut groups {
        xs.sort_by(f64::total_cmp);
    }
    let mut pooled: Vec<f64> = groups
        .iter()
        .flat_map(|(_, xs)| xs.iter().copied())
        .collect();
    pooled.sort_by(f64::total_cmp);
    let (edges, width, rule) = bins(&pooled, request.bin_rule, request.bins);
    body.bin_rule = rule;
    body.bin_width = width;
    let decimals = auto_decimals(variance(&pooled).sqrt());
    let bin_count = edges.len() - 1;
    computed
        .values
        .insert("n".into(), count(pooled.len() as u64));
    computed
        .values
        .insert("bins".into(), count(bin_count as u64));
    computed
        .values
        .insert("bin_width".into(), number(width, decimals + 1));
    let mut any_binned = false;
    let grouped = request.by.is_some();
    for (name, xs) in &groups {
        let h = request
            .bandwidth
            .unwrap_or_else(|| bandwidth(xs, request.bandwidth_rule));
        let (curve, binned) = kde(xs, h);
        any_binned |= binned;
        let boxplot = boxplot(xs);
        let prefix = match (grouped, name) {
            (true, Some(name)) => format!("{}.", key_part(name)),
            _ => String::new(),
        };
        let key = |k: &str| format!("{prefix}{k}");
        let values = &mut computed.values;
        if grouped {
            values.insert(key("n"), count(xs.len() as u64));
        }
        let sd = variance(xs).sqrt();
        values.insert(key("mean"), number(mean(xs), decimals));
        values.insert(key("sd"), number(sd, decimals));
        let iqr = quantile(xs, 0.75) - quantile(xs, 0.25);
        values.insert(key("median"), number(boxplot.median, decimals));
        values.insert(key("q1"), number(quantile(xs, 0.25), decimals));
        values.insert(key("q3"), number(quantile(xs, 0.75), decimals));
        values.insert(key("iqr"), number(iqr, decimals));
        values.insert(key("lower_hinge"), number(boxplot.lower_hinge, decimals));
        values.insert(key("upper_hinge"), number(boxplot.upper_hinge, decimals));
        values.insert(key("min"), number(xs[0], decimals));
        values.insert(key("max"), number(xs[xs.len() - 1], decimals));
        values.insert(key("outliers"), count(boxplot.outliers.len() as u64));
        values.insert(key("bandwidth"), number(h, decimals + 1));
        let epsilon = dkw_epsilon(xs.len(), request.conf_level);
        values.insert(key("dkw_epsilon"), bounded(epsilon, 3));
        let label = name.clone().unwrap_or_else(|| request.variable.clone());
        let normality = match shapiro_wilk(xs) {
            Some(check) => {
                values.insert(key("shapiro_w"), bounded(check.stat, 3));
                values.insert(key("shapiro_p"), p_value(check.p));
                Assumption {
                    id: format!("normality_{}", key_part(&label)),
                    check: Some("Shapiro-Wilk".into()),
                    stat: Some(check.stat),
                    p: Some(check.p),
                    status: if check.p < 0.05 {
                        AssumptionStatus::Warn
                    } else {
                        AssumptionStatus::Pass
                    },
                    consequence: (check.p < 0.05).then(|| {
                        "not normal at α = .05; see where the QQ points leave the band".into()
                    }),
                    note: None,
                }
            }
            None => Assumption {
                id: format!("normality_{}", key_part(&label)),
                check: None,
                stat: None,
                p: None,
                status: AssumptionStatus::NotChecked,
                consequence: None,
                note: Some("Shapiro-Wilk needs 3 to 5000 non-constant values".into()),
            },
        };
        computed.assumptions.push(normality);
        if xs.len() < 10 {
            computed.warnings.push(warning(
                "small_n",
                WarningSeverity::Warn,
                format!(
                    "{label} has only {} values: the histogram and the density are rough",
                    xs.len()
                ),
            ));
        }
        body.groups.push(DistributionGroup {
            name: name.clone(),
            n: xs.len() as u64,
            histogram: histogram(xs, &edges),
            kde: curve,
            bandwidth: h,
            boxplot,
            qq: qq(xs, request.conf_level),
            ecdf: ecdf(xs),
            dkw_epsilon: epsilon,
        });
    }
    let params = &mut computed.method.params;
    params.insert("bin_rule".into(), Value::from(body.bin_rule.label()));
    params.insert(
        "bandwidth_rule".into(),
        Value::from(body.bandwidth_rule.label()),
    );
    if any_binned {
        params.insert(
            "kde".into(),
            Value::from(format!("binned on {KDE_BINS} points (n > {KDE_EXACT_MAX})")),
        );
    }
    computed.method.why = Some(format!(
        "bins by {}, kernel bandwidth by {}",
        body.bin_rule.label(),
        body.bandwidth_rule.label()
    ));
    let slots = if grouped {
        vec!["n", "bins", "bin_width"]
    } else {
        vec![
            "n",
            "mean",
            "sd",
            "median",
            "iqr",
            "outliers",
            "bins",
            "bin_width",
            "bandwidth",
        ]
    };
    computed.narrative = Some(Narrative {
        template: "distribution".into(),
        text_slots: slots.into_iter().map(str::to_owned).collect(),
    });
    Ok((computed, ComputedBody::Distribution(body)))
}

fn reproduce(
    computed: &mut Computed,
    request: &DistributionRequest,
    used: &[application::ports::compute::DataColumn],
) {
    let var = &request.variable;
    let r_breaks = match (request.bins, request.bin_rule) {
        (Some(k), _) => k.to_string(),
        (None, BinRule::Sturges | BinRule::Fixed) => r_str("Sturges"),
        (None, BinRule::Scott) => r_str("Scott"),
        (None, BinRule::FreedmanDiaconis) => r_str("FD"),
    };
    let r_bw = match (request.bandwidth, request.bandwidth_rule) {
        (Some(h), _) => format!("{h}"),
        (None, BandwidthRule::Scott) => r_str("nrd"),
        (None, _) => r_str("nrd0"),
    };
    let py_bins = match (request.bins, request.bin_rule) {
        (Some(k), _) => k.to_string(),
        (None, BinRule::Sturges | BinRule::Fixed) => py_str("sturges"),
        (None, BinRule::Scott) => py_str("scott"),
        (None, BinRule::FreedmanDiaconis) => py_str("fd"),
    };
    let r_box = match &request.by {
        Some(by) => format!("boxplot({} ~ {}, data = df)", r_name(var), r_name(by)),
        None => "boxplot(x)".to_owned(),
    };
    computed.reproduce.r = Some(format!(
        "{}; x <- na.omit({}); hist(x, breaks = {r_breaks}); plot(density(x, bw = {r_bw})); \
         {r_box}; qqnorm(x); qqline(x); plot(ecdf(x))",
        r_frame(&request.data, used),
        r_col(var)
    ));
    computed.reproduce.python = Some(format!(
        "{}; import numpy as np, scipy.stats as st; x = {}.dropna(); np.histogram(x, \
         bins={py_bins}); st.gaussian_kde(x, bw_method='silverman'); st.probplot(x); \
         st.ecdf(x)",
        py_frame(&request.data, used),
        py_col(var)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bandwidths_follow_r() {
        // bw.nrd0(c(1, 2, 3, 4, 10)) = 0.9 * min(sd, IQR/1.34) * 5^-0.2.
        let xs = [1.0, 2.0, 3.0, 4.0, 10.0];
        let sd = variance(&xs).sqrt();
        let expected = 0.9 * sd.min(2.0 / 1.34) * 5f64.powf(-0.2);
        assert!((bandwidth(&xs, BandwidthRule::Silverman) - expected).abs() < 1e-12);
        // A constant sample falls back to |x|.
        let constant = [3.0, 3.0, 3.0];
        assert!(
            (bandwidth(&constant, BandwidthRule::Silverman) - 0.9 * 3.0 * 3f64.powf(-0.2)).abs()
                < 1e-12
        );
    }

    #[test]
    fn the_density_integrates_to_one_exact_or_binned() {
        let xs: Vec<f64> = (0..50)
            .map(|i| f64::from(i % 7) + 0.1 * f64::from(i))
            .collect();
        let mut sorted = xs.clone();
        sorted.sort_by(f64::total_cmp);
        let h = bandwidth(&sorted, BandwidthRule::Silverman);
        let (curve, binned) = kde(&sorted, h);
        assert!(!binned);
        let dx = curve.x[1] - curve.x[0];
        let area: f64 = curve.y.iter().sum::<f64>() * dx;
        assert!((area - 1.0).abs() < 0.01, "{area}");

        let big: Vec<f64> = (0..30_000).map(|i| f64::from(i % 1000) / 10.0).collect();
        let mut big_sorted = big.clone();
        big_sorted.sort_by(f64::total_cmp);
        let (curve, binned) = kde(&big_sorted, 2.0);
        assert!(binned);
        let dx = curve.x[1] - curve.x[0];
        let area: f64 = curve.y.iter().sum::<f64>() * dx;
        assert!((area - 1.0).abs() < 0.01, "{area}");
    }

    #[test]
    fn boxes_ecdf_and_bands() {
        let xs = [1.0, 2.0, 2.0, 3.0, 4.0, 5.0, 30.0];
        let b = boxplot(&xs);
        assert_eq!((b.lower_hinge, b.median, b.upper_hinge), (2.0, 3.0, 4.5));
        assert_eq!(b.whisker_hi, 5.0);
        assert_eq!(b.outliers, [30.0]);
        let steps = ecdf(&xs);
        assert_eq!(steps.len(), 6);
        assert_eq!(
            steps[1],
            EcdfStep {
                x: 2.0,
                p: 3.0 / 7.0
            }
        );
        assert_eq!(steps[5].p, 1.0);
        // DKW at 95%: sqrt(ln(40) / (2n)).
        assert!((dkw_epsilon(100, 0.95) - (40f64.ln() / 200.0).sqrt()).abs() < 1e-15);
        let points = qq(&xs, 0.95);
        assert_eq!(points.len(), 7);
        assert!(points.iter().all(|p| p.lo < p.hi));
        assert!(points[0].theoretical < 0.0 && points[6].theoretical > 0.0);
        assert_eq!(thin(10_000).len(), MAX_CURVE_POINTS);
    }
}
