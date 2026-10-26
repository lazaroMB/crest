//! Requests of the engineering cards: Bode / Nyquist plots of a transfer
//! function, and Karnaugh maps.

use domain::computed::{Complex, FrequencyUnit};
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
