//! Body of the contingency-table card: observed and expected counts, row
//! and column percentages, adjusted standardised residuals and which test
//! the engine ran on them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// The test of independence the engine ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContingencyTest {
    /// Pearson's χ² (with Yates' correction on a 2×2 table when asked).
    ChiSquare,
    /// Fisher's exact test, by full enumeration of the tables with the
    /// observed margins.
    FisherExact,
    /// Fisher's test with a Monte Carlo p-value (too many tables to
    /// enumerate), as R's `simulate.p.value`.
    FisherSimulated,
}

impl ContingencyTest {
    /// Human name, e.g. `Fisher's exact test`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ChiSquare => "Pearson's χ² test",
            Self::FisherExact => "Fisher's exact test",
            Self::FisherSimulated => "Fisher's test (simulated p-value)",
        }
    }
}

/// Body of a contingency-table card. Every matrix is rows × columns in the
/// order of the level lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContingencyBody {
    /// Row variable, when built from data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_variable: Option<String>,
    /// Column variable, when built from data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_variable: Option<String>,
    /// Row labels.
    pub row_levels: Vec<String>,
    /// Column labels.
    pub column_levels: Vec<String>,
    /// Observed counts.
    pub observed: Vec<Vec<f64>>,
    /// Expected counts under independence.
    pub expected: Vec<Vec<f64>>,
    /// Each cell as a percentage of its row, 0–100.
    pub row_percent: Vec<Vec<f64>>,
    /// Each cell as a percentage of its column, 0–100.
    pub column_percent: Vec<Vec<f64>>,
    /// Adjusted standardised residuals (R's `stdres`): about N(0, 1)
    /// under independence, so |r| > 2 marks a cell that departs from it.
    pub std_residuals: Vec<Vec<f64>>,
    /// The test that produced the p-value.
    pub test: ContingencyTest,
}

impl ContingencyBody {
    /// Checks that every matrix has the shape of the level lists.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for a mismatch.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let (rows, cols) = (self.row_levels.len(), self.column_levels.len());
        let shaped = |m: &[Vec<f64>]| m.len() == rows && m.iter().all(|r| r.len() == cols);
        if [
            &self.observed,
            &self.expected,
            &self.row_percent,
            &self.column_percent,
            &self.std_residuals,
        ]
        .iter()
        .all(|m| shaped(m))
        {
            Ok(())
        } else {
            Err(ComputedError::Body(format!(
                "contingency matrices must be {rows}×{cols}"
            )))
        }
    }
}
