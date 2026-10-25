//! Requests of the phase-2 solver cards: linear and mixed-integer
//! programs, equation systems, Monte Carlo models, cash flows and decision
//! matrices. They take parameters only, no data set.

use domain::computed::Sense;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A decision variable of a linear program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LpVariable {
    /// Name, as the expressions use it (letters, digits, `_`).
    pub name: String,
    /// Lower bound (default 0; `null` for none).
    #[serde(default = "zero", deserialize_with = "nullable")]
    pub lower: Option<f64>,
    /// Upper bound (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper: Option<f64>,
    /// Whether it must take whole values.
    #[serde(default)]
    pub integer: bool,
    /// Whether it is 0 or 1 (overrides the bounds).
    #[serde(default)]
    pub binary: bool,
}

#[allow(clippy::unnecessary_wraps)]
fn zero() -> Option<f64> {
    Some(0.0)
}

/// Reads an explicit `null` as `None` (rather than the field default).
fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Option::<f64>::deserialize(d)
}

/// One constraint of a linear program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LpConstraint {
    /// Name (default `c1`, `c2`, …), e.g. `labor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The constraint, linear in the variables: `2*x + 3*y <= 120`,
    /// `x - y >= 0`, `a + b = 1` (`<=`, `>=`, `=`).
    pub expr: String,
}

/// Input of `optimize`: a linear program, or a mixed-integer one when
/// some variables are integer or binary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OptimizeRequest {
    /// `maximize` (default) or `minimize`.
    #[serde(default)]
    pub sense: Sense,
    /// The objective, linear in the variables: `3*x + 5*y`.
    pub objective: String,
    /// The variables.
    pub variables: Vec<LpVariable>,
    /// The constraints.
    #[serde(default)]
    pub constraints: Vec<LpConstraint>,
}
