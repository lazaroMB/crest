//! Body of the power and sample-size card: which design, what was solved
//! for, the four quantities (n, effect, α, power) and the power curve over
//! n at the given effect and α.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Alternative, ComputedError, Curve};

/// The design a power analysis is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PowerTest {
    /// Two-sample t-test, equal groups; effect Cohen's d.
    TwoSampleT,
    /// One-sample t-test; effect Cohen's d.
    OneSampleT,
    /// Paired t-test; effect d of the differences.
    PairedT,
    /// One-way ANOVA with `groups` equal groups; effect Cohen's f.
    Anova,
    /// Two proportions, equal groups; effect Cohen's h.
    TwoProportions,
    /// One proportion against a null value; effect Cohen's h.
    OneProportion,
}

impl PowerTest {
    /// Symbol of the effect size: `d`, `f` or `h`.
    #[must_use]
    pub fn effect_symbol(self) -> &'static str {
        match self {
            Self::TwoSampleT | Self::OneSampleT | Self::PairedT => "d",
            Self::Anova => "f",
            Self::TwoProportions | Self::OneProportion => "h",
        }
    }

    /// Human name, e.g. `two-sample t-test`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TwoSampleT => "two-sample t-test",
            Self::OneSampleT => "one-sample t-test",
            Self::PairedT => "paired t-test",
            Self::Anova => "one-way ANOVA",
            Self::TwoProportions => "test of two proportions",
            Self::OneProportion => "test of one proportion",
        }
    }

    /// Whether `n` counts each group (true) or the whole sample.
    #[must_use]
    pub fn n_per_group(self) -> bool {
        matches!(self, Self::TwoSampleT | Self::Anova | Self::TwoProportions)
    }
}

/// The quantity a power analysis solved for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PowerTarget {
    /// Sample size (per group where the design has groups).
    N,
    /// Power, 1 − β.
    Power,
    /// The smallest effect detectable.
    Effect,
    /// Significance level.
    Alpha,
}

impl PowerTarget {
    /// Human name, e.g. `sample size`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::N => "sample size",
            Self::Power => "power",
            Self::Effect => "detectable effect",
            Self::Alpha => "significance level",
        }
    }
}

/// Body of a power card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PowerBody {
    /// The design.
    pub test: PowerTest,
    /// What was solved for; the other three were given.
    pub solved_for: PowerTarget,
    /// Direction of the alternative.
    pub alternative: Alternative,
    /// Sample size (per group when [`PowerTest::n_per_group`]), unrounded.
    pub n: f64,
    /// Effect size (`d`, `f` or `h`).
    pub effect: f64,
    /// Significance level.
    pub alpha: f64,
    /// Power, 1 − β.
    pub power: f64,
    /// Number of groups (ANOVA).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<u32>,
    /// Power (y) against n (x) at the effect and α above.
    pub curve: Curve,
}

impl PowerBody {
    /// Checks the curve's shape and that power and α are probabilities.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for the first problem.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let unit = |x: f64| (0.0..=1.0).contains(&x);
        if self.curve.x.len() != self.curve.y.len() {
            return Err(ComputedError::Body("power curve x and y differ".into()));
        }
        if !unit(self.power) || !unit(self.alpha) || !self.curve.y.iter().all(|p| unit(*p)) {
            return Err(ComputedError::Body(
                "power and α must lie between 0 and 1".into(),
            ));
        }
        Ok(())
    }
}
