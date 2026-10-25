//! Body of the bootstrap card: the statistic, its resampling distribution
//! and the intervals computed from it. The seed and generator are in the
//! envelope (`seed`, `rng`), so a recompute draws the same resamples.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ComputedError, Histogram};

/// The statistic a bootstrap resamples.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BootStatistic {
    /// Mean of `y`.
    #[default]
    Mean,
    /// Median of `y`.
    Median,
    /// Standard deviation of `y`.
    Sd,
    /// Mean of `y` without the lowest and highest `trim` share.
    TrimmedMean,
    /// Mean of the first group minus the mean of the second.
    MeanDifference,
    /// Median of the first group minus the median of the second.
    MedianDifference,
    /// Pearson correlation of `y` and `y2` (pairs resampled).
    Correlation,
}

impl BootStatistic {
    /// Human name, e.g. `median`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Mean => "mean",
            Self::Median => "median",
            Self::Sd => "standard deviation",
            Self::TrimmedMean => "trimmed mean",
            Self::MeanDifference => "difference of means",
            Self::MedianDifference => "difference of medians",
            Self::Correlation => "correlation",
        }
    }

    /// Whether it compares two groups.
    #[must_use]
    pub fn two_groups(self) -> bool {
        matches!(self, Self::MeanDifference | Self::MedianDifference)
    }
}

/// How a bootstrap interval was formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BootMethod {
    /// Quantiles of the replicates.
    Percentile,
    /// Bias-corrected and accelerated quantiles (Efron 1987).
    Bca,
    /// Basic (reverse percentile): 2θ̂ minus the quantiles.
    Basic,
    /// θ̂ − bias ± z·SE.
    Normal,
}

impl BootMethod {
    /// Human name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Percentile => "percentile",
            Self::Bca => "BCa",
            Self::Basic => "basic",
            Self::Normal => "normal",
        }
    }
}

/// One bootstrap interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BootInterval {
    /// How it was formed.
    pub method: BootMethod,
    /// Lower bound.
    pub lo: f64,
    /// Upper bound.
    pub hi: f64,
    /// Monte Carlo SE of the lower bound, when the method has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lo_mcse: Option<f64>,
    /// Monte Carlo SE of the upper bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hi_mcse: Option<f64>,
}

/// Body of a bootstrap card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BootstrapBody {
    /// What was resampled.
    pub statistic: BootStatistic,
    /// The variable (and the second variable or the groups).
    pub variable: String,
    /// Second variable or the two groups, e.g. `am: 1 − 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versus: Option<String>,
    /// The statistic on the data.
    pub observed: f64,
    /// Number of resamples.
    pub resamples: u32,
    /// Level of the intervals.
    pub conf_level: f64,
    /// Distribution of the replicates.
    pub histogram: Histogram,
    /// The intervals, percentile and BCa first.
    pub intervals: Vec<BootInterval>,
    /// BCa bias correction z₀.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z0: Option<f64>,
    /// BCa acceleration a (jackknife).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceleration: Option<f64>,
}

impl BootstrapBody {
    /// Checks the histogram and that every interval is ordered.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for the first problem.
    pub fn validate(&self) -> Result<(), ComputedError> {
        self.histogram.validate()?;
        if self.intervals.iter().any(|i| i.lo > i.hi) {
            return Err(ComputedError::Body(
                "a bootstrap interval has lo > hi".into(),
            ));
        }
        Ok(())
    }
}
