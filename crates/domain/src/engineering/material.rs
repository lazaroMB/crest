//! A material data card (`show_material`): properties of one material,
//! each with its value, unit, condition and the source it was taken from.
//!
//! crest bundles no material database: every value comes from the call
//! (the user's message, a datasheet or standard the model read, a tool
//! result), and a value without a source is shown as unverified rather
//! than given an authority it does not have.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::block::BlockError;

/// Properties a card keeps.
pub const MAX_PROPERTIES: usize = 200;

const BLOCK: &str = "material";

/// One property value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MaterialProperty {
    /// Name (`Young's modulus`, `yield strength`, `density`).
    pub name: String,
    /// Symbol (`E`, `σy`, `ρ`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// The value as given.
    pub value: f64,
    /// Its unit (`GPa`, `MPa`, `kg/m³`, `1/K`).
    #[serde(default)]
    pub unit: String,
    /// Condition or basis (`20 °C`, `minimum`, `typical`, `T6 temper`,
    /// `A-basis`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Where the value comes from: a datasheet, standard, handbook (with
    /// edition and table), a URL, or `user` for a value the user gave.
    /// Empty: unverified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl MaterialProperty {
    /// Whether the value has a source.
    #[must_use]
    pub fn is_cited(&self) -> bool {
        self.source.as_deref().is_some_and(|s| !s.trim().is_empty())
    }

    /// The value as plain text (the shortest form that reads back as the
    /// same number).
    #[must_use]
    pub fn value_text(&self) -> String {
        self.value.to_string()
    }
}

/// A material card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MaterialSpec {
    /// Material name (`aluminium alloy`, `PEEK`).
    pub name: String,
    /// Designation or grade (`6061-T6`, `EN AW-6061`, `S355JR`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designation: Option<String>,
    /// Form or condition of the whole card (`plate, 6–12 mm`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// The properties.
    pub properties: Vec<MaterialProperty>,
    /// Notes shown under the table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl MaterialSpec {
    /// Checks the name, the count and that values are finite.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.name.trim().is_empty() {
            return Err(invalid(BLOCK, "the material needs a name"));
        }
        if self.properties.is_empty() || self.properties.len() > MAX_PROPERTIES {
            return Err(invalid(
                BLOCK,
                format!("give 1 to {MAX_PROPERTIES} properties"),
            ));
        }
        if let Some(p) = self
            .properties
            .iter()
            .find(|p| p.name.trim().is_empty() || !p.value.is_finite())
        {
            return Err(invalid(
                BLOCK,
                format!("property `{}` needs a name and a finite value", p.name),
            ));
        }
        Ok(())
    }

    /// The name with its designation.
    #[must_use]
    pub fn full_name(&self) -> String {
        match &self.designation {
            Some(d) => format!("{} {d}", self.name),
            None => self.name.clone(),
        }
    }

    /// Properties without a source.
    #[must_use]
    pub fn unverified(&self) -> Vec<&MaterialProperty> {
        self.properties.iter().filter(|p| !p.is_cited()).collect()
    }

    /// The table as CSV: `property,symbol,value,unit,condition,source`
    /// (`unverified` when there is none).
    #[must_use]
    pub fn to_csv(&self) -> String {
        let field = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let mut out = String::from("property,symbol,value,unit,condition,source\n");
        for p in &self.properties {
            out.push_str(&format!(
                "{},{},{},{},{},{}\n",
                field(&p.name),
                field(p.symbol.as_deref().unwrap_or("")),
                p.value_text(),
                field(&p.unit),
                field(p.condition.as_deref().unwrap_or("")),
                field(if p.is_cited() {
                    p.source.as_deref().unwrap_or("")
                } else {
                    "unverified"
                })
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn property(name: &str, value: f64, unit: &str, source: Option<&str>) -> MaterialProperty {
        MaterialProperty {
            name: name.into(),
            symbol: None,
            value,
            unit: unit.into(),
            condition: None,
            source: source.map(str::to_owned),
        }
    }

    #[test]
    fn sources_mark_values_cited_or_unverified() {
        let spec = MaterialSpec {
            name: "aluminium alloy".into(),
            designation: Some("6061-T6".into()),
            condition: None,
            properties: vec![
                property("density", 2700.0, "kg/m³", Some("user")),
                property("Young's modulus", 68.9, "GPa", Some(" ")),
            ],
            notes: Vec::new(),
        };
        assert!(spec.validate().is_ok());
        assert_eq!(spec.full_name(), "aluminium alloy 6061-T6");
        assert_eq!(spec.unverified().len(), 1);
        assert!(
            spec.to_csv()
                .ends_with("Young's modulus,,68.9,GPa,,unverified\n")
        );
        let empty = MaterialSpec {
            properties: Vec::new(),
            ..spec
        };
        assert!(empty.validate().is_err());
    }
}
