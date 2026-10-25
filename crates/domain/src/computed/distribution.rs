//! Body of the distribution card: histogram, kernel density, box/violin,
//! normal QQ with a pointwise band and the ECDF with its DKW band, per
//! group of one numeric variable.
//!
//! Every coordinate is computed by the engine; the card only draws it. The
//! histogram of all groups shares one set of edges so they compare bin by
//! bin.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ComputedError, Histogram};

/// How the histogram bins were chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BinRule {
    /// Freedman-Diaconis: width 2·IQR·n^(−1/3) (Sturges when the IQR is 0).
    #[default]
    FreedmanDiaconis,
    /// Sturges: ⌈log₂ n⌉ + 1 bins.
    Sturges,
    /// Scott: width 3.49·SD·n^(−1/3).
    Scott,
    /// A bin count given in the request.
    Fixed,
}

impl BinRule {
    /// Human name, e.g. `Freedman-Diaconis`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::FreedmanDiaconis => "Freedman-Diaconis",
            Self::Sturges => "Sturges",
            Self::Scott => "Scott",
            Self::Fixed => "fixed count",
        }
    }
}

/// How the kernel bandwidth was chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BandwidthRule {
    /// Silverman's rule of thumb, 0.9·min(SD, IQR/1.34)·n^(−1/5) (R's
    /// `bw.nrd0`).
    #[default]
    Silverman,
    /// Scott's rule, 1.06·min(SD, IQR/1.34)·n^(−1/5) (R's `bw.nrd`).
    Scott,
    /// A bandwidth given in the request.
    Fixed,
}

impl BandwidthRule {
    /// Human name, e.g. `Silverman`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Silverman => "Silverman",
            Self::Scott => "Scott",
            Self::Fixed => "fixed",
        }
    }
}

/// Body of a distribution card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DistributionBody {
    /// The variable described.
    pub variable: String,
    /// Its unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Grouping column, when split by group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// How the bins were chosen.
    pub bin_rule: BinRule,
    /// Width of every bin.
    pub bin_width: f64,
    /// How the kernel bandwidths were chosen.
    pub bandwidth_rule: BandwidthRule,
    /// Coverage of the QQ and ECDF bands, e.g. `0.95`.
    pub conf_level: f64,
    /// One entry per group (one in all when there is no `by`).
    pub groups: Vec<DistributionGroup>,
}

impl DistributionBody {
    /// Checks every group's shapes and that the histograms share edges.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for the first inconsistency.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let bad = |what: &str| Err(ComputedError::Body(format!("distribution: {what}")));
        if self.groups.is_empty() {
            return bad("no groups");
        }
        let edges = &self.groups[0].histogram.edges;
        for group in &self.groups {
            group.histogram.validate()?;
            if &group.histogram.edges != edges {
                return bad("groups must share the histogram edges");
            }
            if group.kde.x.len() != group.kde.y.len() {
                return bad("kde x and y differ in length");
            }
            if group.qq.iter().any(|p| p.lo > p.hi) {
                return bad("a QQ band has lo > hi");
            }
            if group
                .ecdf
                .windows(2)
                .any(|w| w[0].x > w[1].x || w[0].p > w[1].p)
            {
                return bad("the ECDF must ascend");
            }
        }
        Ok(())
    }
}

/// The distribution of one group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DistributionGroup {
    /// Group value; `None` without `by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Non-missing values.
    pub n: u64,
    /// Binned counts.
    pub histogram: Histogram,
    /// Gaussian kernel density estimate.
    pub kde: Curve,
    /// Kernel bandwidth.
    pub bandwidth: f64,
    /// Box-plot summary.
    pub boxplot: BoxSummary,
    /// Normal QQ points, in ascending order of the sample.
    #[serde(default)]
    pub qq: Vec<QqPoint>,
    /// Steps of the empirical CDF: at `x` it rises to `p`.
    #[serde(default)]
    pub ecdf: Vec<EcdfStep>,
    /// Half-width of the Dvoretzky-Kiefer-Wolfowitz band around the ECDF.
    pub dkw_epsilon: f64,
}

/// A curve as matching x and y coordinates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Curve {
    /// Abscissas, ascending.
    pub x: Vec<f64>,
    /// Ordinates, one per abscissa.
    pub y: Vec<f64>,
}

/// The five numbers of a Tukey box plot (as R's `boxplot.stats`), plus
/// the points outside.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BoxSummary {
    /// Lower hinge (Tukey's, as `fivenum`).
    pub lower_hinge: f64,
    /// Median.
    pub median: f64,
    /// Upper hinge.
    pub upper_hinge: f64,
    /// Smallest value within 1.5 hinge spreads below the lower hinge.
    pub whisker_lo: f64,
    /// Largest value within 1.5 hinge spreads above the upper hinge.
    pub whisker_hi: f64,
    /// Values beyond the whiskers, ascending.
    #[serde(default)]
    pub outliers: Vec<f64>,
}

/// One point of a normal QQ plot with its pointwise band.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QqPoint {
    /// Standard normal quantile of the plotting position.
    pub theoretical: f64,
    /// The sample value (an order statistic).
    pub sample: f64,
    /// Lower band bound for this order statistic under the fitted normal.
    pub lo: f64,
    /// Upper band bound.
    pub hi: f64,
}

/// One step of an empirical CDF.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EcdfStep {
    /// A distinct sample value.
    pub x: f64,
    /// Share of values ≤ `x`.
    pub p: f64,
}
