//! Body of the BOM costing card: each line's quantity × unit cost in
//! exact decimal arithmetic, the totals per category and overall, and
//! the lines with no price or no manufacturer part number.
//!
//! Money and quantities are decimal strings (`"0.0125"`), written by the
//! engine with `rust_decimal`, so nothing is rounded until it is shown.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ComputedError;

/// One line of the bill of materials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BomLine {
    /// Row in the source (1 = the first data row).
    pub row: u32,
    /// Part name or number.
    pub part: String,
    /// Reference designators (`R1, R2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Manufacturer part number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Category (`uncategorized` when none).
    pub category: String,
    /// Quantity per unit built, decimal.
    pub quantity: String,
    /// Unit cost, decimal; `None` when the source has no price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_cost: Option<String>,
    /// Quantity × unit cost, decimal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extended: Option<String>,
    /// Whether the quantity was counted from the reference designators.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quantity_from_references: bool,
}

/// Totals of one category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BomCategory {
    /// Name.
    pub name: String,
    /// Lines in it.
    pub lines: u32,
    /// Σ quantity, decimal.
    pub quantity: String,
    /// Σ extended cost of the priced lines, decimal.
    pub total: String,
    /// Lines without a price.
    pub missing_prices: u32,
}

/// Body of a BOM costing card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BomBody {
    /// Currency of the costs, shown only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Units built, decimal (the totals are per unit and × this).
    pub builds: String,
    /// The lines in source order.
    pub lines: Vec<BomLine>,
    /// Totals per category, largest first.
    pub categories: Vec<BomCategory>,
    /// Cost of one unit (priced lines only), decimal.
    pub total: String,
    /// Cost of all the units built, decimal.
    pub total_builds: String,
    /// Whether the source had an MPN column (so missing MPNs mean
    /// something).
    #[serde(default)]
    pub has_mpn: bool,
}

impl BomBody {
    /// Checks there is at least one line and that categories cover them.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] otherwise.
    pub fn validate(&self) -> Result<(), ComputedError> {
        if self.lines.is_empty() {
            return Err(ComputedError::Body("a BOM needs at least one line".into()));
        }
        let counted: u32 = self.categories.iter().map(|c| c.lines).sum();
        if counted as usize != self.lines.len() {
            return Err(ComputedError::Body(
                "category line counts must add up to the lines".into(),
            ));
        }
        Ok(())
    }

    /// Lines without a price.
    #[must_use]
    pub fn missing_prices(&self) -> Vec<&BomLine> {
        self.lines
            .iter()
            .filter(|l| l.unit_cost.is_none())
            .collect()
    }

    /// Lines without an MPN (empty when the source has no MPN column).
    #[must_use]
    pub fn missing_mpns(&self) -> Vec<&BomLine> {
        if self.has_mpn {
            self.lines.iter().filter(|l| l.mpn.is_none()).collect()
        } else {
            Vec::new()
        }
    }

    /// The costed BOM as CSV.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let field = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let mut out = String::from("row,part,reference,mpn,category,quantity,unit_cost,extended\n");
        for l in &self.lines {
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                l.row,
                field(&l.part),
                field(l.reference.as_deref().unwrap_or("")),
                field(l.mpn.as_deref().unwrap_or("")),
                field(&l.category),
                l.quantity,
                l.unit_cost.as_deref().unwrap_or(""),
                l.extended.as_deref().unwrap_or("")
            ));
        }
        out
    }
}
