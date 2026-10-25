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

/// Input of `power`: a power or sample-size calculation. Give three of
/// `n`, `effect` (or the proportions `p1`, `p2`), `power` and `alpha`;
/// the missing one is solved for (or name it in `solve_for`). `alpha`
/// defaults to 0.05 unless it is the one solved for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PowerRequest {
    /// The design: `two_sample_t`, `one_sample_t`, `paired_t`, `anova`,
    /// `two_proportions` or `one_proportion`.
    pub test: domain::computed::PowerTest,
    /// Sample size: per group for two-sample t, ANOVA and two
    /// proportions, else in total (pairs for paired t).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<f64>,
    /// Effect size: Cohen's d (t-tests), f (ANOVA) or h (proportions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<f64>,
    /// Proportions instead of `effect`: the two group proportions, or the
    /// expected proportion and (in `p2`) the null value for
    /// `one_proportion`; h = 2·asin√p1 − 2·asin√p2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p1: Option<f64>,
    /// Second proportion; see `p1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2: Option<f64>,
    /// Significance level (default 0.05).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
    /// Power, 1 − β, e.g. 0.8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<f64>,
    /// Number of groups (ANOVA, at least 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<u32>,
    /// Direction of the alternative (default two_sided).
    #[serde(default)]
    pub alternative: domain::computed::Alternative,
    /// What to solve for, when it is not clear from what is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solve_for: Option<domain::computed::PowerTarget>,
}
