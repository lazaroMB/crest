//! Body of the Bode / Nyquist card: the frequency response of a transfer
//! function G(s) = N(s)/D(s)·e^(−sT), its poles and zeros, crossover
//! frequencies with gain and phase margins, and the closed-loop poles
//! under unity negative feedback.
//!
//! Frequencies are stored in rad/s; [`FrequencyUnit`] only says how the
//! card and the values show them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// How frequencies are shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrequencyUnit {
    /// Radians per second.
    #[default]
    RadPerS,
    /// Hertz (ω / 2π).
    Hz,
}

impl FrequencyUnit {
    /// Unit symbol.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::RadPerS => "rad/s",
            Self::Hz => "Hz",
        }
    }

    /// `w` (rad/s) in this unit.
    #[must_use]
    pub fn from_rad(self, w: f64) -> f64 {
        match self {
            Self::RadPerS => w,
            Self::Hz => w / std::f64::consts::TAU,
        }
    }

    /// A frequency in this unit, in rad/s.
    #[must_use]
    pub fn to_rad(self, f: f64) -> f64 {
        match self {
            Self::RadPerS => f,
            Self::Hz => f * std::f64::consts::TAU,
        }
    }
}

/// A complex number (a pole or zero).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    #[serde(default)]
    pub im: f64,
}

impl Complex {
    /// `re + j·im` as text, e.g. `-0.5 ± 0.866j` is two of these.
    #[must_use]
    pub fn text(&self, decimals: usize) -> String {
        let re = format!("{:.decimals$}", self.re);
        if self.im.abs() < 1e-12 {
            re
        } else {
            let sign = if self.im < 0.0 { '−' } else { '+' };
            format!("{re} {sign} {:.decimals$}j", self.im.abs())
        }
    }
}

/// One frequency of the response.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BodePoint {
    /// Frequency, rad/s.
    pub w: f64,
    /// 20·log10 |G(jω)|.
    pub magnitude_db: f64,
    /// arg G(jω) in degrees, unwrapped along the sweep.
    pub phase_deg: f64,
    /// Re G(jω).
    pub re: f64,
    /// Im G(jω).
    pub im: f64,
}

/// A crossover: where |G| = 1 (gain) or the phase is −180° + k·360°
/// (phase), with the margin there.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Crossover {
    /// Frequency, rad/s.
    pub w: f64,
    /// Phase margin in degrees (gain crossover) or gain margin in dB
    /// (phase crossover).
    pub margin: f64,
}

/// Body of a Bode / Nyquist card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BodeBody {
    /// Numerator coefficients, highest power of s first (gain included).
    pub numerator: Vec<f64>,
    /// Denominator coefficients, highest power of s first.
    pub denominator: Vec<f64>,
    /// Dead time T in seconds (0 for none).
    #[serde(default)]
    pub delay: f64,
    /// How frequencies are shown.
    #[serde(default)]
    pub unit: FrequencyUnit,
    /// The sweep, ascending in ω.
    pub points: Vec<BodePoint>,
    /// Zeros of N(s).
    #[serde(default)]
    pub zeros: Vec<Complex>,
    /// Poles (zeros of D(s)).
    #[serde(default)]
    pub poles: Vec<Complex>,
    /// Gain crossovers with the phase margin at each.
    #[serde(default)]
    pub gain_crossovers: Vec<Crossover>,
    /// Phase crossovers with the gain margin at each.
    #[serde(default)]
    pub phase_crossovers: Vec<Crossover>,
    /// Poles of G/(1+G) (roots of D + N), when there is no delay.
    #[serde(default)]
    pub closed_loop_poles: Vec<Complex>,
}

impl BodeBody {
    /// Checks the sweep ascends and holds finite numbers.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] otherwise.
    pub fn validate(&self) -> Result<(), ComputedError> {
        if self.denominator.is_empty() {
            return Err(ComputedError::Body("the denominator is empty".into()));
        }
        if self.points.windows(2).any(|w| w[0].w >= w[1].w) {
            return Err(ComputedError::Body(
                "the frequency sweep must ascend".into(),
            ));
        }
        Ok(())
    }

    /// The transfer function as text: `(s + 2) / (s^2 + 3s + 2)`.
    #[must_use]
    pub fn transfer_text(&self) -> String {
        let num = polynomial_text(&self.numerator);
        let den = polynomial_text(&self.denominator);
        let mut text = if self.denominator.len() == 1 && (self.denominator[0] - 1.0).abs() < 1e-15 {
            num
        } else {
            format!("({num}) / ({den})")
        };
        if self.delay > 0.0 {
            text.push_str(&format!(" · e^(−{}s)", trim(self.delay)));
        }
        text
    }
}

fn trim(x: f64) -> String {
    let text = format!("{x:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// `coefficients` (highest power first) as `2s^2 + 3s + 1`.
#[must_use]
pub fn polynomial_text(coefficients: &[f64]) -> String {
    let n = coefficients.len();
    let mut out = String::new();
    for (i, c) in coefficients.iter().enumerate() {
        if *c == 0.0 {
            continue;
        }
        let power = n - 1 - i;
        let magnitude = c.abs();
        let sign = if *c < 0.0 { "−" } else { "+" };
        if out.is_empty() {
            if *c < 0.0 {
                out.push('−');
            }
        } else {
            out.push_str(&format!(" {sign} "));
        }
        let coef = if (magnitude - 1.0).abs() < 1e-15 && power > 0 {
            String::new()
        } else {
            trim(magnitude)
        };
        out.push_str(&match power {
            0 => trim(magnitude),
            1 => format!("{coef}s"),
            p => format!("{coef}s^{p}"),
        });
    }
    if out.is_empty() { "0".to_owned() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polynomials_and_units_read_naturally() {
        assert_eq!(polynomial_text(&[1.0, 3.0, 2.0]), "s^2 + 3s + 2");
        assert_eq!(polynomial_text(&[-2.0, 0.0, 0.5]), "−2s^2 + 0.5");
        assert_eq!(polynomial_text(&[0.0, 0.0]), "0");
        assert!((FrequencyUnit::Hz.from_rad(std::f64::consts::TAU) - 1.0).abs() < 1e-12);
        assert_eq!(
            Complex {
                re: -0.5,
                im: -0.866
            }
            .text(3),
            "-0.500 − 0.866j"
        );
    }
}
