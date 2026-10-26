//! Requests of the engineering cards: Bode / Nyquist plots of a transfer
//! function.

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
