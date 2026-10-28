//! Body of the Monte Carlo card: the inputs with their distributions, the
//! output's distribution and convergence, and the tornado of one-at-a-time
//! swings. A Fermi estimate is the same card with the output the product
//! of the inputs, each a 90% range with its source.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ComputedError, Curve, Histogram};

/// One input as simulated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SimInputSummary {
    /// Name.
    pub name: String,
    /// Its distribution in words, e.g. `lognormal, 90% in [2, 8]`.
    pub distribution: String,
    /// 10th percentile of the distribution.
    pub p10: f64,
    /// Median.
    pub p50: f64,
    /// 90th percentile.
    pub p90: f64,
    /// Where the estimate comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// One bar of the tornado: the output with one input at its 10th and 90th
/// percentiles, the others at their medians.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TornadoBar {
    /// The input moved.
    pub input: String,
    /// Output at the input's 10th percentile.
    pub at_p10: f64,
    /// Output at its 90th percentile.
    pub at_p90: f64,
    /// |at_p90 − at_p10|.
    pub swing: f64,
}

/// Body of a Monte Carlo card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SimulationBody {
    /// Name of the output.
    pub output: String,
    /// Its unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The model, e.g. `price * volume - cost`.
    pub model: String,
    /// Whether the output is the product of the inputs (a Fermi estimate).
    #[serde(default)]
    pub fermi: bool,
    /// Draws.
    pub samples: u32,
    /// The inputs.
    pub inputs: Vec<SimInputSummary>,
    /// Distribution of the output.
    pub histogram: Histogram,
    /// The running mean (y) after n draws (x).
    pub convergence: Curve,
    /// Monte Carlo SE of the running mean at each point of `convergence`.
    #[serde(default)]
    pub convergence_mcse: Vec<f64>,
    /// The output with every input at its median.
    pub base: f64,
    /// Tornado bars, largest swing first.
    #[serde(default)]
    pub tornado: Vec<TornadoBar>,
}

impl SimulationBody {
    /// Checks the histogram and the convergence trace.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for a shape mismatch.
    pub fn validate(&self) -> Result<(), ComputedError> {
        self.histogram.validate()?;
        let n = self.convergence.x.len();
        if self.convergence.y.len() != n
            || !(self.convergence_mcse.is_empty() || self.convergence_mcse.len() == n)
        {
            return Err(ComputedError::Body(
                "convergence trace shapes differ".into(),
            ));
        }
        Ok(())
    }
}
