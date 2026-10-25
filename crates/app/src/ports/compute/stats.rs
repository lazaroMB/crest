//! Requests of the phase-2 statistics cards: distribution, contingency
//! table, power, A/B test, bootstrap and forecast.

use domain::computed::{BandwidthRule, BinRule};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DataSet, default_conf_level, default_true};

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

/// Which test of independence a contingency table gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContingencyChoice {
    /// Fisher's exact test when an expected count is below 5, else
    /// Pearson's χ²; the card says which and why.
    #[default]
    Auto,
    /// Pearson's χ² (Yates' correction on 2×2 unless `correct = false`).
    ChiSquare,
    /// Fisher's exact test (simulated p-value when the table is too large
    /// to enumerate).
    Fisher,
}

/// Default seed of the simulated tests and resampling.
pub const DEFAULT_SEED: u64 = 20_261_025;

fn default_seed() -> u64 {
    DEFAULT_SEED
}

fn default_simulations() -> u32 {
    10_000
}

/// Input of `contingency`: a two-way table with observed and expected
/// counts, row/column percentages, standardised residuals and a test of
/// independence. Give two categorical columns `row` and `column`, or the
/// counts in `table`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContingencyRequest {
    /// The data; may be empty when `table` gives the counts.
    #[serde(default)]
    pub data: DataSet,
    /// Row variable (a column of the data).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<String>,
    /// Column variable (a column of the data).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Counts, rows × columns, instead of `row` and `column`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<Vec<Vec<f64>>>,
    /// Labels of the rows of `table` (default `R1`, `R2`, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub row_labels: Vec<String>,
    /// Labels of the columns of `table` (default `C1`, `C2`, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub column_labels: Vec<String>,
    /// The test; `auto` picks Fisher when an expected count is below 5.
    #[serde(default)]
    pub test: ContingencyChoice,
    /// Yates' continuity correction for χ² on a 2×2 table (default true,
    /// as R).
    #[serde(default = "default_true")]
    pub correct: bool,
    /// Alternative of Fisher's test on a 2×2 table (the odds ratio
    /// against 1).
    #[serde(default)]
    pub alternative: domain::computed::Alternative,
    /// Level of the odds-ratio interval (default 0.95).
    #[serde(default = "default_conf_level")]
    pub conf_level: f64,
    /// Seed of the simulated p-value, when one is needed.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Tables simulated for that p-value (default 10000).
    #[serde(default = "default_simulations")]
    pub simulations: u32,
}
