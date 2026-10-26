//! Requests of the engineering cards: Bode / Nyquist plots of a transfer
//! function, Karnaugh maps, beams and tolerance stack-ups.

use domain::computed::{
    BeamLoad, BeamSupport, Complex, ForceUnit, FrequencyUnit, InertiaUnit, LengthUnit, ModulusUnit,
    StackDirection, ToleranceDistribution,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_points() -> u32 {
    400
}

/// Input of `bode`: a transfer function as polynomials in s, or as
/// zeros, poles and a gain, with an optional dead time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BodeRequest {
    /// Numerator coefficients, highest power of s first: `[10]` for 10,
    /// `[1, 2]` for s + 2. Give `numerator` and `denominator`, or
    /// `zeros`, `poles` and `gain`.
    #[serde(default)]
    pub numerator: Vec<f64>,
    /// Denominator coefficients, highest power of s first: `[1, 3, 2]`
    /// for s² + 3s + 2.
    #[serde(default)]
    pub denominator: Vec<f64>,
    /// Zeros as `{re, im}`; complex ones in conjugate pairs.
    #[serde(default)]
    pub zeros: Vec<Complex>,
    /// Poles as `{re, im}`; complex ones in conjugate pairs.
    #[serde(default)]
    pub poles: Vec<Complex>,
    /// Gain k of k·Π(s − z)/Π(s − p) (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f64>,
    /// Dead time in seconds: G(s)·e^(−s·delay).
    #[serde(default)]
    pub delay: f64,
    /// Lowest frequency of the sweep, in `unit`; chosen from the poles
    /// and zeros when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w_min: Option<f64>,
    /// Highest frequency of the sweep, in `unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w_max: Option<f64>,
    /// Points of the sweep, log-spaced (default 400).
    #[serde(default = "default_points")]
    pub points: u32,
    /// `rad_per_s` (default) or `hz`, for `w_min`/`w_max` and the card.
    #[serde(default)]
    pub unit: FrequencyUnit,
}

/// Input of `karnaugh`: a Boolean function of up to six variables, as
/// minterms, maxterms or an expression, with optional don't-cares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KarnaughRequest {
    /// Variable names, most significant first (`["A", "B", "C"]`); taken
    /// from the expression in alphabetical order when absent, else
    /// A, B, C… for as many as the terms need.
    #[serde(default)]
    pub variables: Vec<String>,
    /// Minterms where F = 1 (`[1, 3, 5, 7]`). Give these, `maxterms` or
    /// `expression`.
    #[serde(default)]
    pub minterms: Vec<u32>,
    /// Maxterms where F = 0; every other term (not a don't-care) is 1.
    #[serde(default)]
    pub maxterms: Vec<u32>,
    /// Don't-care terms.
    #[serde(default)]
    pub dont_cares: Vec<u32>,
    /// F as an expression: `'` or `!`/`~` for NOT, `&`, `*`, `·` or
    /// juxtaposition for AND, `+` or `|` for OR, `^` for XOR,
    /// parentheses, 0 and 1 (`AB' + C(A ^ D)`). Without `variables`, a
    /// name is one letter with optional digits (`x1x2` is x1·x2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
}

fn default_segments() -> u32 {
    200
}

/// Input of `beam`: a straight beam, its supports and loads, and
/// optionally E and I for the deflection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BeamRequest {
    /// Span, in `length_unit`.
    pub length: f64,
    /// `m` (default), `mm`, `ft` or `in`; for the span and every position.
    #[serde(default)]
    pub length_unit: LengthUnit,
    /// `kN` (default), `N`, `lbf` or `kip`; moments are this × length and
    /// distributed loads this / length.
    #[serde(default)]
    pub force_unit: ForceUnit,
    /// Supports: `{at, kind}` with `kind` `pin` (default), `roller` or
    /// `fixed`. A cantilever is one `fixed` support.
    pub supports: Vec<BeamSupport>,
    /// Loads: `{kind: "point", at, value}` (downward positive),
    /// `{kind: "distributed", at, end, value, end_value}` (per length,
    /// downward positive, linear from `value` to `end_value`) or
    /// `{kind: "moment", at, value}` (counterclockwise positive).
    pub loads: Vec<BeamLoad>,
    /// Young's modulus, in `e_unit`; with `i`, the deflection is computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e: Option<f64>,
    /// `GPa` (default), `MPa`, `kPa`, `Pa`, `psi` or `ksi`.
    #[serde(default)]
    pub e_unit: ModulusUnit,
    /// Second moment of area, in `i_unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i: Option<f64>,
    /// `mm4` (default), `cm4`, `m4` or `in4`.
    #[serde(default)]
    pub i_unit: InertiaUnit,
    /// Elements of the stiffness model between the key points (default
    /// 200); nodes sit at every support, load point and load end too.
    #[serde(default = "default_segments")]
    pub segments: u32,
}

/// One dimension of a tolerance stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StackDimensionInput {
    /// Name (`housing bore`, `A`).
    pub name: String,
    /// Nominal value.
    pub nominal: f64,
    /// Symmetric tolerance ±`tolerance`; or give `plus` and `minus`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    /// Upper deviation (e.g. 0.05 for +0.05).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plus: Option<f64>,
    /// Lower deviation as a magnitude (0.02 for −0.02).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minus: Option<f64>,
    /// `add` (default) or `subtract` in the chain.
    #[serde(default)]
    pub direction: StackDirection,
    /// `normal` (default) or `uniform` for the Monte Carlo.
    #[serde(default)]
    pub distribution: ToleranceDistribution,
}

fn default_sigmas() -> f64 {
    3.0
}

fn default_stack_seed() -> u64 {
    super::DEFAULT_SEED
}

fn default_stack_samples() -> u32 {
    100_000
}

/// Input of `tolerance`: a one-dimensional tolerance chain and optional
/// specification limits on the result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToleranceRequest {
    /// The dimensions in chain order.
    pub dimensions: Vec<StackDimensionInput>,
    /// Lowest acceptable result (a minimum gap, say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_limit: Option<f64>,
    /// Highest acceptable result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper_limit: Option<f64>,
    /// Unit of every length (`mm`), shown only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Standard deviations a tolerance stands for (default 3, Cp = 1).
    #[serde(default = "default_sigmas")]
    pub sigmas: f64,
    /// Monte Carlo draws (default 100 000).
    #[serde(default = "default_stack_samples")]
    pub samples: u32,
    /// Seed of the draws.
    #[serde(default = "default_stack_seed")]
    pub seed: u64,
}
