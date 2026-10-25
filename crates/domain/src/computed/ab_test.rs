//! Body of the A/B test card: the two arms with their conversion rates and
//! intervals, which test produced the p-value, and the checks around it
//! (sample-ratio mismatch, repeated looks).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// The test behind an A/B card's p-value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AbMethod {
    /// Two-proportion z-test with the pooled rate (R's `prop.test(...,
    /// correct = FALSE)`).
    ZTest,
    /// Fisher's exact test, for small counts.
    FisherExact,
}

impl AbMethod {
    /// Human name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ZTest => "two-proportion z-test",
            Self::FisherExact => "Fisher's exact test",
        }
    }
}

/// One arm of an A/B test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AbArm {
    /// Arm name, e.g. `A`.
    pub name: String,
    /// Units exposed.
    pub visitors: u64,
    /// Units that converted.
    pub conversions: u64,
    /// Conversion rate, a fraction.
    pub rate: f64,
    /// Wilson interval of the rate: lower bound.
    pub rate_lo: f64,
    /// Wilson interval of the rate: upper bound.
    pub rate_hi: f64,
}

/// Body of an A/B test card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AbTestBody {
    /// Control first, then treatment.
    pub arms: Vec<AbArm>,
    /// The share of visitors meant for the treatment, e.g. 0.5.
    pub expected_split: f64,
    /// Level of the intervals.
    pub conf_level: f64,
    /// The test behind the p-value.
    pub method: AbMethod,
    /// How often the results were looked at (1 = once, at the end).
    pub looks: u32,
    /// Whether the visitor split departs from `expected_split` (SRM χ²
    /// p < .001).
    pub sample_ratio_mismatch: bool,
}

impl AbTestBody {
    /// Checks that there are two arms with conversions ≤ visitors.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] otherwise.
    pub fn validate(&self) -> Result<(), ComputedError> {
        if self.arms.len() != 2 || self.arms.iter().any(|a| a.conversions > a.visitors) {
            return Err(ComputedError::Body(
                "an A/B test has two arms with conversions ≤ visitors".into(),
            ));
        }
        Ok(())
    }
}
