//! Body of the decision-matrix card: criteria with normalised weights,
//! options with their normalised scores, weighted totals and ranks, and
//! how far each weight can move before the first place changes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// Whether more of a criterion is better.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CriterionDirection {
    /// Higher scores are better.
    #[default]
    Benefit,
    /// Lower scores are better (price, risk).
    Cost,
}

/// How raw scores are put on a common scale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Normalization {
    /// (x − min)/(max − min), reversed for a cost: the best option 1, the
    /// worst 0.
    #[default]
    MinMax,
    /// x/max, or min/x for a cost (positive scores).
    Max,
    /// The raw scores (already on one scale); a cost counts negatively.
    None,
}

impl Normalization {
    /// Human name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::MinMax => "min-max",
            Self::Max => "ratio to the best",
            Self::None => "raw scores",
        }
    }
}

/// One criterion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionCriterion {
    /// Name.
    pub name: String,
    /// The weight as given.
    pub raw_weight: f64,
    /// The weight normalised to sum 1 over the criteria.
    pub weight: f64,
    /// Benefit or cost.
    pub direction: CriterionDirection,
}

/// One option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionOption {
    /// Name.
    pub name: String,
    /// Raw score per criterion, in criterion order.
    pub scores: Vec<f64>,
    /// Normalised score per criterion.
    pub normalized: Vec<f64>,
    /// Σ weight × normalised score.
    pub total: f64,
    /// 1 for the best; tied options share a rank.
    pub rank: u32,
}

/// A weight at which another option overtakes the leader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WeightThreshold {
    /// The criterion's normalised weight there (the others rescaled in
    /// proportion).
    pub weight: f64,
    /// The option that takes first place.
    pub new_leader: String,
}

/// How far one criterion's weight can move before the leader changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WeightSensitivity {
    /// The criterion.
    pub criterion: String,
    /// Its current normalised weight.
    pub weight: f64,
    /// The nearest threshold below, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower: Option<WeightThreshold>,
    /// The nearest threshold above, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper: Option<WeightThreshold>,
}

/// Body of a decision-matrix card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionMatrixBody {
    /// How scores were normalised.
    pub normalization: Normalization,
    /// The criteria, in request order.
    pub criteria: Vec<DecisionCriterion>,
    /// The options, in request order.
    pub options: Vec<DecisionOption>,
    /// One entry per criterion.
    #[serde(default)]
    pub sensitivity: Vec<WeightSensitivity>,
}

impl DecisionMatrixBody {
    /// Checks that every option scores every criterion.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] otherwise.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let k = self.criteria.len();
        if self
            .options
            .iter()
            .all(|o| o.scores.len() == k && o.normalized.len() == k)
        {
            Ok(())
        } else {
            Err(ComputedError::Body(
                "every option needs one score per criterion".into(),
            ))
        }
    }
}
