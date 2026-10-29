//! Body of the beam card: a straight beam on pins, rollers and fixed
//! supports under point loads, distributed loads and moments, with its
//! reactions, shear and bending-moment diagrams and, given E·I, its
//! deflection.
//!
//! Conventions: x runs from the left end; a load's `value` is positive
//! downward; an applied moment is positive counterclockwise; reactions
//! are positive upward (forces) and counterclockwise (moments). Shear is
//! the sum of the upward forces left of a section; the bending moment is
//! positive when it sags (tension at the bottom). Deflection is positive
//! upward, so a sagging beam deflects negative.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// Length units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LengthUnit {
    /// Metres.
    #[default]
    M,
    /// Millimetres.
    Mm,
    /// Feet.
    Ft,
    /// Inches.
    In,
}

impl LengthUnit {
    /// Symbol.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::M => "m",
            Self::Mm => "mm",
            Self::Ft => "ft",
            Self::In => "in",
        }
    }

    /// Metres in one unit.
    #[must_use]
    pub fn metres(self) -> f64 {
        match self {
            Self::M => 1.0,
            Self::Mm => 1e-3,
            Self::Ft => 0.3048,
            Self::In => 0.0254,
        }
    }

    /// The unit deflections are shown in: mm for metric, in for imperial.
    #[must_use]
    pub fn deflection_unit(self) -> Self {
        match self {
            Self::M | Self::Mm => Self::Mm,
            Self::Ft | Self::In => Self::In,
        }
    }
}

/// Force units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ForceUnit {
    /// Newtons.
    #[serde(rename = "N", alias = "n")]
    N,
    /// Kilonewtons.
    #[default]
    #[serde(rename = "kN", alias = "kn")]
    KN,
    /// Pounds-force.
    #[serde(rename = "lbf")]
    Lbf,
    /// Kips (1000 lbf).
    #[serde(rename = "kip")]
    Kip,
}

impl ForceUnit {
    /// Symbol.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::N => "N",
            Self::KN => "kN",
            Self::Lbf => "lbf",
            Self::Kip => "kip",
        }
    }

    /// Newtons in one unit.
    #[must_use]
    pub fn newtons(self) -> f64 {
        match self {
            Self::N => 1.0,
            Self::KN => 1e3,
            Self::Lbf => 4.448_221_615_260_5,
            Self::Kip => 4_448.221_615_260_5,
        }
    }
}

/// Units of Young's modulus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ModulusUnit {
    /// Pascals.
    #[serde(rename = "Pa", alias = "pa")]
    Pa,
    /// Kilopascals.
    #[serde(rename = "kPa", alias = "kpa")]
    KPa,
    /// Megapascals (N/mm²).
    #[serde(rename = "MPa", alias = "mpa")]
    MPa,
    /// Gigapascals.
    #[default]
    #[serde(rename = "GPa", alias = "gpa")]
    GPa,
    /// Pounds per square inch.
    #[serde(rename = "psi")]
    Psi,
    /// Kips per square inch.
    #[serde(rename = "ksi")]
    Ksi,
}

impl ModulusUnit {
    /// Symbol.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Pa => "Pa",
            Self::KPa => "kPa",
            Self::MPa => "MPa",
            Self::GPa => "GPa",
            Self::Psi => "psi",
            Self::Ksi => "ksi",
        }
    }

    /// Pascals in one unit.
    #[must_use]
    pub fn pascals(self) -> f64 {
        match self {
            Self::Pa => 1.0,
            Self::KPa => 1e3,
            Self::MPa => 1e6,
            Self::GPa => 1e9,
            Self::Psi => 6_894.757_293_168_361,
            Self::Ksi => 6_894_757.293_168_361,
        }
    }
}

/// Units of the second moment of area.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InertiaUnit {
    /// m⁴.
    M4,
    /// cm⁴ (steel tables in Europe).
    Cm4,
    /// mm⁴.
    #[default]
    Mm4,
    /// in⁴ (AISC tables).
    In4,
}

impl InertiaUnit {
    /// Symbol.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::M4 => "m⁴",
            Self::Cm4 => "cm⁴",
            Self::Mm4 => "mm⁴",
            Self::In4 => "in⁴",
        }
    }

    /// m⁴ in one unit.
    #[must_use]
    pub fn metres4(self) -> f64 {
        match self {
            Self::M4 => 1.0,
            Self::Cm4 => 1e-8,
            Self::Mm4 => 1e-12,
            Self::In4 => 4.162_314_256e-7,
        }
    }
}

/// How a support restrains the beam.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SupportKind {
    /// A pin: no vertical movement, free rotation.
    #[default]
    Pin,
    /// A roller: as a pin for a beam without axial load.
    Roller,
    /// Fixed (clamped): no movement, no rotation.
    Fixed,
}

impl SupportKind {
    /// Reactions it provides (1 or 2).
    #[must_use]
    pub fn reactions(self) -> u32 {
        match self {
            Self::Pin | Self::Roller => 1,
            Self::Fixed => 2,
        }
    }
}

/// A support.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BeamSupport {
    /// Position from the left end.
    pub at: f64,
    /// Its kind.
    #[serde(default)]
    pub kind: SupportKind,
}

/// What a load is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BeamLoadKind {
    /// A concentrated force at `at` (positive downward).
    #[default]
    Point,
    /// A force per length from `at` to `end`, `value` at `at` and
    /// `end_value` (default the same) at `end` (positive downward).
    Distributed,
    /// A concentrated moment at `at` (positive counterclockwise).
    Moment,
}

/// A load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BeamLoad {
    /// Its kind.
    #[serde(default)]
    pub kind: BeamLoadKind,
    /// Position (start of a distributed load).
    pub at: f64,
    /// End of a distributed load (default the beam's end).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    /// Force, force per length or moment.
    pub value: f64,
    /// Intensity at `end` for a linearly varying load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_value: Option<f64>,
}

/// A support reaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BeamReaction {
    /// Support name (`A`, `B`… from the left).
    pub name: String,
    /// Position.
    pub at: f64,
    /// Kind of support.
    pub kind: SupportKind,
    /// Vertical force, positive upward.
    pub force: f64,
    /// Moment of a fixed support, positive counterclockwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moment: Option<f64>,
}

/// Body of a beam card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BeamBody {
    /// Span.
    pub length: f64,
    /// Unit of positions and the span.
    pub length_unit: LengthUnit,
    /// Unit of forces (moments are force × length).
    pub force_unit: ForceUnit,
    /// The supports, from the left.
    pub supports: Vec<BeamSupport>,
    /// The loads as given.
    pub loads: Vec<BeamLoad>,
    /// One reaction per support, from the left.
    pub reactions: Vec<BeamReaction>,
    /// Section positions; a position repeats where shear or moment jumps
    /// (left value, then right).
    pub x: Vec<f64>,
    /// Shear at each position.
    pub shear: Vec<f64>,
    /// Bending moment at each position.
    pub moment: Vec<f64>,
    /// Deflection at each position, in [`LengthUnit::deflection_unit`];
    /// empty without E·I.
    #[serde(default)]
    pub deflection: Vec<f64>,
    /// E·I in N·m², when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ei: Option<f64>,
    /// Reactions beyond the two statics gives (0: statically
    /// determinate).
    pub indeterminacy: u32,
}

impl BeamBody {
    /// Checks the diagrams' shapes.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for a mismatch.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let n = self.x.len();
        if self.shear.len() != n
            || self.moment.len() != n
            || !(self.deflection.is_empty() || self.deflection.len() == n)
        {
            return Err(ComputedError::Body(
                "shear, moment and deflection need one value per position".into(),
            ));
        }
        if self.reactions.len() != self.supports.len() {
            return Err(ComputedError::Body("one reaction per support".into()));
        }
        Ok(())
    }

    /// `kN·m`, `lbf·ft`.
    #[must_use]
    pub fn moment_unit(&self) -> String {
        format!("{}·{}", self.force_unit.symbol(), self.length_unit.symbol())
    }

    /// The diagrams as CSV: `x,shear,moment[,deflection]`.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let (l, f) = (self.length_unit.symbol(), self.force_unit.symbol());
        let mut out = format!("x_{l},shear_{f},moment_{f}_{l}");
        if !self.deflection.is_empty() {
            out.push_str(&format!(
                ",deflection_{}",
                self.length_unit.deflection_unit().symbol()
            ));
        }
        out.push('\n');
        for i in 0..self.x.len() {
            out.push_str(&format!(
                "{},{},{}",
                self.x[i], self.shear[i], self.moment[i]
            ));
            if let Some(d) = self.deflection.get(i) {
                out.push_str(&format!(",{d}"));
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_convert_and_parse() {
        assert!((ForceUnit::Kip.newtons() / ForceUnit::Lbf.newtons() - 1000.0).abs() < 1e-9);
        assert!((InertiaUnit::In4.metres4() - 0.0254f64.powi(4)).abs() < 1e-18);
        let f: Option<ForceUnit> = serde_json::from_str("\"N\"").ok();
        assert_eq!(f, Some(ForceUnit::N));
        let e: Option<ModulusUnit> = serde_json::from_str("\"MPa\"").ok();
        assert_eq!(e, Some(ModulusUnit::MPa));
        assert_eq!(LengthUnit::Ft.deflection_unit(), LengthUnit::In);
    }
}
