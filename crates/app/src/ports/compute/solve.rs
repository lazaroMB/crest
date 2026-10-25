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

/// One unknown of an equation system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Unknown {
    /// Name, as the equations use it.
    pub name: String,
    /// Starting value of Newton's method (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guess: Option<f64>,
}

fn default_tolerance() -> f64 {
    1e-10
}

fn default_iterations() -> u32 {
    100
}

/// Input of `solve`: equations in as many unknowns, solved numerically.
/// With one unknown and a `range`, every root in the range is found (sign
/// changes on a fine grid, refined by bisection); otherwise damped Newton
/// starts from the guesses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SolveRequest {
    /// The equations, `lhs = rhs` (`x^2 + y^2 = 4`, `cos(x) = x`); an
    /// expression without `=` means `= 0`.
    pub equations: Vec<String>,
    /// The unknowns, as many as equations.
    pub unknowns: Vec<Unknown>,
    /// For one unknown: `[low, high]` to search for every root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// Largest |lhs − rhs| accepted (default 1e-10).
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Most Newton iterations (default 100).
    #[serde(default = "default_iterations")]
    pub max_iterations: u32,
}

/// The distribution of a Monte Carlo input, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimDistribution {
    /// Always `value`.
    Fixed {
        /// The value.
        value: f64,
    },
    /// Uniform on [`min`, `max`].
    Uniform {
        /// Lower end.
        min: f64,
        /// Upper end.
        max: f64,
    },
    /// Normal with `mean` and `sd`.
    Normal {
        /// Mean.
        mean: f64,
        /// Standard deviation.
        sd: f64,
    },
    /// Lognormal with 90% of its mass in [`low`, `high`] (both > 0): the
    /// usual Fermi range "between low and high".
    Lognormal {
        /// 5th percentile.
        low: f64,
        /// 95th percentile.
        high: f64,
    },
    /// Triangular on [`min`, `max`] with peak at `mode`.
    Triangular {
        /// Lower end.
        min: f64,
        /// Most likely value.
        mode: f64,
        /// Upper end.
        max: f64,
    },
    /// PERT (a scaled beta) on [`min`, `max`] with peak at `mode`.
    Pert {
        /// Lower end.
        min: f64,
        /// Most likely value.
        mode: f64,
        /// Upper end.
        max: f64,
    },
}

/// One input of a Monte Carlo model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SimInput {
    /// Name, as the model uses it.
    pub name: String,
    /// Its distribution.
    pub distribution: SimDistribution,
    /// Where the estimate comes from (a reference, "guess"), shown on the
    /// card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Unit, shown only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

fn default_samples() -> u32 {
    10_000
}

fn default_sim_seed() -> u64 {
    super::DEFAULT_SEED
}

/// Input of `simulate`: a Monte Carlo model, its tornado of one-at-a-time
/// swings, and (without `model`) a Fermi estimate: the product of the
/// inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SimulateRequest {
    /// The uncertain inputs, assumed independent.
    pub inputs: Vec<SimInput>,
    /// The output as an expression of the inputs (`price * volume -
    /// cost`); without it the output is the product of the inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Name of the output (default `result`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Unit of the output, shown only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Draws (default 10000, at most 1000000).
    #[serde(default = "default_samples")]
    pub samples: u32,
    /// Seed of the generator.
    #[serde(default = "default_sim_seed")]
    pub seed: u64,
}

/// Input of `cash_flow`: net cash flows per period, the first at period 0
/// (not discounted). The engine computes in decimal arithmetic: NPV at
/// `rate`, every IRR, MIRR, simple and discounted payback, and the NPV
/// curve over the rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CashFlowRequest {
    /// Net cash flow of each period, from period 0: `[-1000, 300, 400,
    /// 500]` (outflows negative).
    pub flows: Vec<f64>,
    /// Discount rate per period as a fraction, e.g. 0.08.
    pub rate: f64,
    /// Finance rate of the MIRR (default `rate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finance_rate: Option<f64>,
    /// Reinvestment rate of the MIRR (default `rate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reinvest_rate: Option<f64>,
    /// Currency of the amounts, shown only, e.g. `EUR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Name of a period, e.g. `year` (default `period`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
}
