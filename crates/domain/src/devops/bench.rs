//! Criterion benchmark results, baseline against new (`show_benchmarks`).
//!
//! Every number here is read from criterion's own files under
//! `target/criterion/<id>/`: `new/estimates.json` (the last run), the
//! baseline's `estimates.json` and `change/estimates.json` (the relative
//! change of the mean and median with its bootstrap confidence interval).
//! Nothing is re-estimated. The only judgement is the [`Verdict`], from
//! the change's interval against criterion's noise threshold
//! ([`NOISE_THRESHOLD`]), since criterion does not store its p-value.
//! Without a change file, [`Bench::ratio`] is the plain ratio of the two
//! point estimates, with no interval.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::block::BlockError;
use crate::widget::timeline::{TimeUnit, format_duration};

/// Benchmarks a block keeps.
pub const MAX_BENCHES: usize = 2_000;
/// Criterion's default noise threshold: changes within ±1% are noise.
pub const NOISE_THRESHOLD: f64 = 0.01;

const BLOCK: &str = "benchmarks";

/// A point estimate with its confidence interval.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Estimate {
    /// The point estimate.
    pub point: f64,
    /// Lower bound of the interval.
    pub lo: f64,
    /// Upper bound of the interval.
    pub hi: f64,
    /// Confidence level of the interval (0.95).
    pub confidence: f64,
    /// Standard error, when the file gives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_error: Option<f64>,
}

/// The estimates of one run, in nanoseconds per iteration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunEstimates {
    /// Mean time.
    pub mean: Estimate,
    /// Median time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median: Option<Estimate>,
    /// Standard deviation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub std_dev: Option<Estimate>,
    /// Slope of the linear regression (iteration count against time), when
    /// criterion used linear sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slope: Option<Estimate>,
}

/// What the change of the mean says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The whole interval is above +noise: slower.
    Regressed,
    /// The whole interval is below −noise: faster.
    Improved,
    /// The interval excludes 0 but not the noise band.
    WithinNoise,
    /// The interval includes 0.
    NoChange,
    /// No change file: nothing to judge.
    Unknown,
}

impl Verdict {
    /// The verdict of a relative change (fractions, 0.05 = +5%).
    #[must_use]
    pub fn of(change: &Estimate, noise: f64) -> Self {
        if change.lo > noise {
            Self::Regressed
        } else if change.hi < -noise {
            Self::Improved
        } else if change.lo > 0.0 || change.hi < 0.0 {
            Self::WithinNoise
        } else {
            Self::NoChange
        }
    }

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Regressed => "regressed",
            Self::Improved => "improved",
            Self::WithinNoise => "within noise",
            Self::NoChange => "no change",
            Self::Unknown => "no baseline",
        }
    }
}

/// One benchmark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Bench {
    /// Criterion's id (`group/function/value`).
    pub id: String,
    /// Directory under the criterion root.
    pub dir: String,
    /// The new run.
    pub new: RunEstimates,
    /// The baseline run, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<RunEstimates>,
    /// Relative change of the mean (fractions), from the change file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Estimate>,
    /// Relative change of the median, from the change file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median_change: Option<Estimate>,
    /// The verdict on [`Self::change`].
    pub verdict: Verdict,
    /// `new mean / base mean − 1` when there is a baseline but no change
    /// file (no interval).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
}

/// The benchmarks of a criterion directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BenchSpec {
    /// Caption; the directory when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path of the criterion directory.
    pub path: String,
    /// Name of the baseline compared against (`base`, or a
    /// `--save-baseline` name).
    pub baseline: String,
    /// Noise threshold of the verdicts.
    pub noise_threshold: f64,
    /// Benchmarks, by id.
    pub benches: Vec<Bench>,
    /// Directories that had estimates for neither run, or did not parse.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

/// Nanoseconds as a readable time.
#[must_use]
pub fn format_ns(ns: f64) -> String {
    format_duration(ns, TimeUnit::Ns)
}

/// A fraction as a signed percentage (`+4.2%`).
#[must_use]
pub fn format_change(fraction: f64) -> String {
    let pct = fraction * 100.0;
    let digits = if pct.abs() >= 100.0 { 0 } else { 1 };
    format!("{}{pct:.digits$}%", if pct >= 0.0 { "+" } else { "" })
}

impl BenchSpec {
    /// Checks the size and that the numbers are finite and the intervals
    /// ordered.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.benches.len() > MAX_BENCHES {
            return Err(invalid(
                BLOCK,
                format!("{} benchmarks; at most {MAX_BENCHES}", self.benches.len()),
            ));
        }
        let ok = |e: &Estimate| {
            [e.point, e.lo, e.hi, e.confidence]
                .iter()
                .all(|n| n.is_finite())
                && e.lo <= e.hi
        };
        for b in &self.benches {
            let mut all = vec![b.new.mean];
            all.extend(b.new.median);
            if let Some(base) = &b.base {
                all.push(base.mean);
                all.extend(base.median);
            }
            all.extend(b.change);
            all.extend(b.median_change);
            if !all.iter().all(ok) {
                return Err(invalid(
                    BLOCK,
                    format!(
                        "{}: an estimate is not finite or its interval is reversed",
                        b.id
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Indices of the benchmarks with `verdict`.
    #[must_use]
    pub fn with_verdict(&self, verdict: Verdict) -> Vec<usize> {
        (0..self.benches.len())
            .filter(|&ix| self.benches[ix].verdict == verdict)
            .collect()
    }

    /// The largest |bound| of the mean changes, for a symmetric axis.
    #[must_use]
    pub fn change_extent(&self) -> f64 {
        self.benches
            .iter()
            .filter_map(|b| b.change)
            .flat_map(|c| [c.lo.abs(), c.hi.abs()])
            .chain(self.benches.iter().filter_map(|b| b.ratio.map(f64::abs)))
            .fold(self.noise_threshold * 2.0, f64::max)
    }

    /// One CSV line per benchmark: means and medians with intervals (ns)
    /// and changes (fractions), as read.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "id,new_mean_ns,new_mean_lo,new_mean_hi,new_median_ns,base_mean_ns,base_mean_lo,\
             base_mean_hi,change_mean,change_lo,change_hi,change_median,verdict\n",
        );
        let f = |v: Option<f64>| v.map(|v| format!("{v}")).unwrap_or_default();
        for b in &self.benches {
            let id = if b.id.contains([',', '"']) {
                format!("\"{}\"", b.id.replace('"', "\"\""))
            } else {
                b.id.clone()
            };
            out.push_str(&format!(
                "{id},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                b.new.mean.point,
                b.new.mean.lo,
                b.new.mean.hi,
                f(b.new.median.map(|m| m.point)),
                f(b.base.map(|m| m.mean.point)),
                f(b.base.map(|m| m.mean.lo)),
                f(b.base.map(|m| m.mean.hi)),
                f(b.change.map(|c| c.point).or(b.ratio)),
                f(b.change.map(|c| c.lo)),
                f(b.change.map(|c| c.hi)),
                f(b.median_change.map(|c| c.point)),
                b.verdict.name()
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn est(point: f64, lo: f64, hi: f64) -> Estimate {
        Estimate {
            point,
            lo,
            hi,
            confidence: 0.95,
            standard_error: None,
        }
    }

    #[test]
    fn verdicts_compare_the_interval_with_the_noise_band() {
        assert_eq!(Verdict::of(&est(0.1, 0.05, 0.15), 0.01), Verdict::Regressed);
        assert_eq!(
            Verdict::of(&est(-0.1, -0.15, -0.05), 0.01),
            Verdict::Improved
        );
        assert_eq!(
            Verdict::of(&est(0.006, 0.002, 0.011), 0.01),
            Verdict::WithinNoise
        );
        assert_eq!(
            Verdict::of(&est(0.01, -0.02, 0.04), 0.01),
            Verdict::NoChange
        );
        assert_eq!(format_change(0.1423), "+14.2%");
        assert_eq!(format_change(-0.05), "-5.0%");
        assert_eq!(format_change(1.5), "+150%");
        assert_eq!(format_ns(25_100.0), "25.1 µs");
    }

    #[test]
    fn csv_and_extent() {
        let run = RunEstimates {
            mean: est(100.0, 90.0, 110.0),
            median: None,
            std_dev: None,
            slope: None,
        };
        let spec = BenchSpec {
            title: None,
            path: "target/criterion".into(),
            baseline: "base".into(),
            noise_threshold: NOISE_THRESHOLD,
            benches: vec![Bench {
                id: "fib/20".into(),
                dir: "fib/20".into(),
                new: run,
                base: Some(run),
                change: Some(est(0.1, 0.05, 0.3)),
                median_change: None,
                verdict: Verdict::Regressed,
                ratio: None,
            }],
            skipped: Vec::new(),
        };
        spec.validate().unwrap();
        assert!((spec.change_extent() - 0.3).abs() < 1e-12);
        assert_eq!(spec.with_verdict(Verdict::Regressed), [0]);
        let csv = spec.to_csv();
        assert!(
            csv.lines()
                .nth(1)
                .is_some_and(|l| l == "fib/20,100,90,110,,100,90,110,0.1,0.05,0.3,,regressed"),
            "{csv}"
        );
    }
}
