//! Body of the equation-solver card: the solution, how it was found, and
//! the plug-back check of every equation at it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ComputedError, Curve};

/// How the solver searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SolveMethod {
    /// Damped Newton with a finite-difference Jacobian, from a guess.
    Newton,
    /// Sign changes scanned on a grid over a range, each refined by
    /// bisection (one unknown).
    BracketScan,
}

impl SolveMethod {
    /// Human name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Newton => "damped Newton (finite-difference Jacobian)",
            Self::BracketScan => "sign-change scan with bisection",
        }
    }
}

/// One unknown at the solution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SolvedUnknown {
    /// Name.
    pub name: String,
    /// Value (the first root of a scan).
    pub value: f64,
}

/// One equation evaluated at the solution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlugBack {
    /// The equation as written.
    pub equation: String,
    /// Its left-hand side at the solution.
    pub lhs: f64,
    /// Its right-hand side at the solution.
    pub rhs: f64,
    /// `lhs − rhs`.
    pub residual: f64,
}

/// Body of an equation-solver card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EquationsBody {
    /// How the solver searched.
    pub method: SolveMethod,
    /// Whether it met the tolerance.
    pub converged: bool,
    /// The unknowns at the solution.
    pub unknowns: Vec<SolvedUnknown>,
    /// Every root found in the range (bracket scan).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<f64>,
    /// The equations at the solution.
    pub plug_back: Vec<PlugBack>,
    /// Largest |residual|.
    pub residual_norm: f64,
    /// Iterations used (Newton steps or bisection halvings in total).
    pub iterations: u32,
    /// Tolerance on the residuals.
    pub tolerance: f64,
    /// Condition number of the Jacobian at the solution (Newton).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_number: Option<f64>,
    /// `lhs − rhs` over the range, for one unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<Curve>,
}

impl EquationsBody {
    /// Checks the curve's shape.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] when x and y differ in length.
    pub fn validate(&self) -> Result<(), ComputedError> {
        match &self.curve {
            Some(c) if c.x.len() != c.y.len() => {
                Err(ComputedError::Body("solver curve x and y differ".into()))
            }
            _ => Ok(()),
        }
    }
}
