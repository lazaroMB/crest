//! Requests of the phase-2 statistics cards: distribution, contingency
//! table, power, A/B test, bootstrap and forecast.

use domain::computed::{BandwidthRule, BinRule};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DataSet, default_conf_level};

/// Input of `distribution`: histogram, kernel density, box/violin, normal
/// QQ and ECDF of one numeric variable, per group of `by`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DistributionRequest {
    /// The data.
    pub data: DataSet,
    /// Numeric column to describe.
    pub variable: String,
    /// Grouping column: one distribution per level, drawn side by side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Binning rule (default freedman_diaconis); `bins` overrides it.
    #[serde(default)]
    pub bin_rule: BinRule,
    /// Number of histogram bins (1–200), instead of the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bins: Option<u32>,
    /// Kernel bandwidth rule (default silverman); `bandwidth` overrides it.
    #[serde(default)]
    pub bandwidth_rule: BandwidthRule,
    /// Kernel bandwidth in the variable's unit, instead of the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<f64>,
    /// Coverage of the QQ and ECDF bands (default 0.95).
    #[serde(default = "default_conf_level")]
    pub conf_level: f64,
}
