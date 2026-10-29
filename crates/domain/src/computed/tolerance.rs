//! Body of the tolerance stack-up card: a one-dimensional chain of
//! dimensions, each added or subtracted, with its worst-case and
//! root-sum-square (RSS) limits, a seeded Monte Carlo of the stack, and
//! each dimension's share of the spread.
//!
//! A dimension `nominal +plus/−minus` is centred at `nominal + (plus −
//! minus)/2` with half-width `(plus + minus)/2`. Worst case adds the
//! half-widths; RSS adds them in quadrature, which assumes each tolerance
//! is `sigmas` standard deviations of an independent, centred process.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ComputedError, Histogram};

/// Whether a dimension adds to the stack or is taken away.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StackDirection {
    /// Adds (+).
    #[default]
    Add,
    /// Subtracts (−).
    Subtract,
}

impl StackDirection {
    /// +1 or −1.
    #[must_use]
    pub fn sign(self) -> f64 {
        match self {
            Self::Add => 1.0,
            Self::Subtract => -1.0,
        }
    }
}

/// How a dimension varies within its tolerance in the Monte Carlo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToleranceDistribution {
    /// Normal, centred, with the half-width = `sigmas` standard
    /// deviations.
    #[default]
    Normal,
    /// Uniform over the tolerance band (a process that is not centred).
    Uniform,
}

/// One dimension of the chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StackDimension {
    /// Name.
    pub name: String,
    /// Nominal value.
    pub nominal: f64,
    /// Upper deviation (≥ 0).
    pub plus: f64,
    /// Lower deviation as a magnitude (≥ 0).
    pub minus: f64,
    /// Added or subtracted.
    pub direction: StackDirection,
    /// Monte Carlo distribution.
    pub distribution: ToleranceDistribution,
    /// Centre of the band.
    pub center: f64,
    /// Half-width of the band.
    pub half_width: f64,
    /// Standard deviation used in the Monte Carlo.
    pub sd: f64,
    /// Share of the worst-case spread (half-width / Σ half-widths).
    pub worst_case_share: f64,
    /// Share of the RSS variance (half-width² / Σ half-width²).
    pub rss_share: f64,
    /// Share of the Monte Carlo variance (sd² / Σ sd²).
    pub variance_share: f64,
}

/// Body of a tolerance stack-up card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToleranceBody {
    /// Unit of every length, shown only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The chain in order.
    pub dimensions: Vec<StackDimension>,
    /// Σ ± nominal.
    pub nominal: f64,
    /// Σ ± centre.
    pub center: f64,
    /// Worst-case lower and upper limits.
    pub worst_case: [f64; 2],
    /// RSS lower and upper limits.
    pub rss: [f64; 2],
    /// Standard deviations a tolerance stands for (3 by default).
    pub sigmas: f64,
    /// Monte Carlo draws.
    pub samples: u32,
    /// Mean and standard deviation of the simulated stack.
    pub mc_mean: f64,
    /// Standard deviation of the simulated stack.
    pub mc_sd: f64,
    /// The simulated 0.135% and 99.865% quantiles (±3σ of a normal).
    pub mc_range: [f64; 2],
    /// Histogram of the simulated stack.
    pub histogram: Histogram,
    /// Lower specification limit, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_limit: Option<f64>,
    /// Upper specification limit, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper_limit: Option<f64>,
}

impl ToleranceBody {
    /// Checks the shares and histogram.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for a malformed histogram or limits that
    /// cross.
    pub fn validate(&self) -> Result<(), ComputedError> {
        self.histogram.validate()?;
        if let (Some(lo), Some(hi)) = (self.lower_limit, self.upper_limit)
            && lo >= hi
        {
            return Err(ComputedError::Body(
                "the lower limit must be below the upper".into(),
            ));
        }
        Ok(())
    }

    /// The chain as CSV with the contributions.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "dimension,direction,nominal,plus,minus,distribution,sd,worst_case_share,rss_share,variance_share\n",
        );
        for d in &self.dimensions {
            let name = if d.name.contains([',', '"']) {
                format!("\"{}\"", d.name.replace('"', "\"\""))
            } else {
                d.name.clone()
            };
            out.push_str(&format!(
                "{name},{},{},{},{},{},{},{},{},{}\n",
                match d.direction {
                    StackDirection::Add => "+",
                    StackDirection::Subtract => "-",
                },
                d.nominal,
                d.plus,
                d.minus,
                match d.distribution {
                    ToleranceDistribution::Normal => "normal",
                    ToleranceDistribution::Uniform => "uniform",
                },
                d.sd,
                d.worst_case_share,
                d.rss_share,
                d.variance_share
            ));
        }
        out
    }
}
