//! Criterion's `estimates.json` files into a [`BenchSpec`].
//!
//! The tool collects, per benchmark directory, `new/benchmark.json` (the
//! id), `new/estimates.json`, the baseline's `estimates.json` and
//! `change/estimates.json`; this module only reads them. Times are
//! nanoseconds; changes are fractions. The numbers are copied as
//! criterion wrote them.

use application::ports::formats::BenchRequest;
use domain::devops::bench::{
    Bench, BenchSpec, Estimate, MAX_BENCHES, NOISE_THRESHOLD, RunEstimates, Verdict,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Interval {
    confidence_level: f64,
    lower_bound: f64,
    upper_bound: f64,
}

#[derive(Debug, Deserialize)]
struct RawEstimate {
    confidence_interval: Interval,
    point_estimate: f64,
    #[serde(default)]
    standard_error: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Estimates {
    mean: RawEstimate,
    #[serde(default)]
    median: Option<RawEstimate>,
    #[serde(default)]
    std_dev: Option<RawEstimate>,
    #[serde(default)]
    slope: Option<RawEstimate>,
}

#[derive(Debug, Deserialize)]
struct BenchmarkId {
    #[serde(default)]
    full_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

fn estimate(raw: &RawEstimate) -> Estimate {
    Estimate {
        point: raw.point_estimate,
        lo: raw.confidence_interval.lower_bound,
        hi: raw.confidence_interval.upper_bound,
        confidence: raw.confidence_interval.confidence_level,
        standard_error: raw.standard_error,
    }
}

fn run(bytes: &[u8]) -> Result<RunEstimates, String> {
    let e: Estimates = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(RunEstimates {
        mean: estimate(&e.mean),
        median: e.median.as_ref().map(estimate),
        std_dev: e.std_dev.as_ref().map(estimate),
        slope: e.slope.as_ref().map(estimate),
    })
}

/// Reads the files of every benchmark of `request`.
///
/// # Errors
///
/// A message when no benchmark has a readable `new/estimates.json`.
pub fn parse(request: &BenchRequest) -> Result<BenchSpec, String> {
    let mut benches = Vec::new();
    let mut skipped = Vec::new();
    for files in &request.benches {
        if benches.len() >= MAX_BENCHES {
            skipped.push(files.dir.clone());
            continue;
        }
        let Some(new) = files.new.as_deref().and_then(|b| run(b).ok()) else {
            skipped.push(files.dir.clone());
            continue;
        };
        let base = files.base.as_deref().and_then(|b| run(b).ok());
        let change = files
            .change
            .as_deref()
            .and_then(|b| serde_json::from_slice::<Estimates>(b).ok());
        let id = files
            .benchmark
            .as_deref()
            .and_then(|b| serde_json::from_slice::<BenchmarkId>(b).ok())
            .and_then(|id| id.full_id.or(id.title))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| files.dir.clone());
        let mean_change = change.as_ref().map(|c| estimate(&c.mean));
        let verdict = mean_change.map_or(Verdict::Unknown, |c| Verdict::of(&c, NOISE_THRESHOLD));
        let ratio = match (&mean_change, &base) {
            (None, Some(base)) if base.mean.point > 0.0 => {
                Some(new.mean.point / base.mean.point - 1.0)
            }
            _ => None,
        };
        benches.push(Bench {
            id,
            dir: files.dir.clone(),
            new,
            base,
            change: mean_change,
            median_change: change
                .as_ref()
                .and_then(|c| c.median.as_ref().map(estimate)),
            verdict,
            ratio,
        });
    }
    if benches.is_empty() {
        return Err(format!(
            "`{}` has no criterion results (no `new/estimates.json` under it); run `cargo bench` \
             first",
            request.path
        ));
    }
    benches.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(BenchSpec {
        title: None,
        path: request.path.clone(),
        baseline: request.baseline.clone(),
        noise_threshold: NOISE_THRESHOLD,
        benches,
        skipped,
    })
}
