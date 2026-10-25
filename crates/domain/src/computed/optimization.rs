//! Body of the LP/MILP card: the status and objective, every variable with
//! its value and reduced cost, every constraint with its activity, slack,
//! whether it binds and its dual value (shadow price).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// Direction of the objective.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sense {
    /// Largest objective.
    #[default]
    Maximize,
    /// Smallest objective.
    Minimize,
}

/// How a solve ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationStatus {
    /// Proven optimal.
    Optimal,
    /// A feasible point, optimality not proven (node limit reached).
    Feasible,
    /// No point satisfies every constraint.
    Infeasible,
    /// The objective improves without limit.
    Unbounded,
    /// Stopped before any feasible point was found.
    Stopped,
}

impl OptimizationStatus {
    /// Human name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Optimal => "optimal",
            Self::Feasible => "feasible (not proven optimal)",
            Self::Infeasible => "infeasible",
            Self::Unbounded => "unbounded",
            Self::Stopped => "stopped without a solution",
        }
    }
}

/// A comparison of a linear constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintOp {
    /// `≤`.
    Le,
    /// `≥`.
    Ge,
    /// `=`.
    Eq,
}

impl ConstraintOp {
    /// The symbol, e.g. `≤`.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Le => "≤",
            Self::Ge => "≥",
            Self::Eq => "=",
        }
    }
}

/// One decision variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OptVariable {
    /// Name.
    pub name: String,
    /// Value at the solution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// Lower bound (`None` = −∞).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower: Option<f64>,
    /// Upper bound (`None` = +∞).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper: Option<f64>,
    /// Whether it must be a whole number.
    #[serde(default)]
    pub integer: bool,
    /// Its coefficient in the objective.
    pub objective_coefficient: f64,
    /// Change of the objective per unit the variable is forced up (0 for a
    /// variable strictly between its bounds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduced_cost: Option<f64>,
}

/// One linear constraint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OptConstraint {
    /// Name.
    pub name: String,
    /// The constraint as written.
    pub expr: String,
    /// Coefficient of each variable (in variable order) after moving every
    /// variable term to the left.
    #[serde(default)]
    pub coefficients: Vec<f64>,
    /// Comparison.
    pub op: ConstraintOp,
    /// Right-hand side after moving every variable term to the left.
    pub rhs: f64,
    /// Value of the left-hand side at the solution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<f64>,
    /// How far it is from binding (≥ 0 when satisfied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slack: Option<f64>,
    /// Whether it holds with equality.
    #[serde(default)]
    pub binding: bool,
    /// Shadow price: change of the objective per unit increase of `rhs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dual: Option<f64>,
}

/// Body of an optimisation card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OptimizationBody {
    /// Direction.
    pub sense: Sense,
    /// The objective as written.
    pub objective_expr: String,
    /// How the solve ended.
    pub status: OptimizationStatus,
    /// Objective value at the solution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<f64>,
    /// Whether some variables are integer (a MILP).
    #[serde(default)]
    pub integer: bool,
    /// Relative gap between the solution and the best bound (MILP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mip_gap: Option<f64>,
    /// The variables, in request order.
    pub variables: Vec<OptVariable>,
    /// The constraints, in request order.
    pub constraints: Vec<OptConstraint>,
}

impl OptimizationBody {
    /// Checks that a solved body has a value for every variable.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] otherwise.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let solved = matches!(
            self.status,
            OptimizationStatus::Optimal | OptimizationStatus::Feasible
        );
        if solved && self.variables.iter().any(|v| v.value.is_none()) {
            return Err(ComputedError::Body(
                "a solved problem has a value for every variable".into(),
            ));
        }
        Ok(())
    }
}
