//! A GHS hazard card (`show_ghs`): pictograms, signal word, hazard and
//! precautionary statements of a compound as PubChem gives them, each
//! source with its name and link, and where and when they were fetched.
//!
//! Everything comes from the fetched record: nothing here (or in the
//! tool) writes a hazard. A compound PubChem does not know, or one
//! without a GHS classification, is a card that says so.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::BlockError;

/// The nine GHS pictograms: code and name, as the UN GHS names them.
pub const PICTOGRAMS: [(&str, &str); 9] = [
    ("GHS01", "Exploding bomb"),
    ("GHS02", "Flame"),
    ("GHS03", "Flame over circle"),
    ("GHS04", "Gas cylinder"),
    ("GHS05", "Corrosion"),
    ("GHS06", "Skull and crossbones"),
    ("GHS07", "Exclamation mark"),
    ("GHS08", "Health hazard"),
    ("GHS09", "Environment"),
];

/// A pictogram of a classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Pictogram {
    /// `GHS01` … `GHS09`.
    pub code: String,
    /// The hazard it stands for, as the source labels it (`Flammable`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Pictogram {
    /// The UN name of the symbol (`Flame`), when the code is one of the
    /// nine.
    #[must_use]
    pub fn symbol_name(&self) -> Option<&'static str> {
        PICTOGRAMS
            .iter()
            .find(|(code, _)| *code == self.code)
            .map(|(_, name)| *name)
    }
}

/// A hazard statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HazardStatement {
    /// `H225`, `EUH066`.
    pub code: String,
    /// Its text as given, with the class in brackets when the source
    /// adds it.
    pub text: String,
    /// The share of notifications that report it (`> 99.9%`), for
    /// aggregated sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<String>,
}

/// One source's classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GhsClassification {
    /// The source (`European Chemicals Agency (ECHA)`).
    pub source: String,
    /// What the source calls the substance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub substance: Option<String>,
    /// The source's page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The source's licence note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Pictograms.
    #[serde(default)]
    pub pictograms: Vec<Pictogram>,
    /// `Danger` or `Warning`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    /// Hazard statements.
    #[serde(default)]
    pub hazards: Vec<HazardStatement>,
    /// Precautionary statement codes (`P210`, `P305+P351+P338`); PubChem
    /// gives the codes, not their texts.
    #[serde(default)]
    pub precautionary: Vec<String>,
    /// The source's notes (share of non-classified reports).
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A GHS card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GhsSpec {
    /// What was looked up (name, CAS number or CID).
    pub query: String,
    /// PubChem's compound id, when the compound was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<u64>,
    /// PubChem's name of the compound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The classifications, one per source.
    #[serde(default)]
    pub classifications: Vec<GhsClassification>,
    /// The PubChem page to check (the compound's GHS section).
    pub source_url: String,
    /// The URL fetched.
    pub fetched_url: String,
    /// When it was fetched (UTC date, `YYYY-MM-DD`).
    pub retrieved: String,
    /// Why there is nothing to show (not found, no classification).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl GhsSpec {
    /// Checks there is a query, a source and a date.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] describing the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        let invalid = |reason: &str| BlockError::Invalid {
            kind: "ghs",
            reason: reason.to_owned(),
        };
        if self.query.trim().is_empty() {
            return Err(invalid("the query is empty"));
        }
        if self.source_url.trim().is_empty() || self.retrieved.trim().is_empty() {
            return Err(invalid(
                "a GHS card needs its source URL and retrieval date",
            ));
        }
        if self.classifications.is_empty() && self.message.is_none() {
            return Err(invalid("a card without classifications must say why"));
        }
        Ok(())
    }

    /// Whether any classification was found.
    #[must_use]
    pub fn found(&self) -> bool {
        !self.classifications.is_empty()
    }

    /// The pictogram codes over all sources, in GHS order, once each.
    #[must_use]
    pub fn all_pictograms(&self) -> Vec<String> {
        let mut codes: Vec<String> = self
            .classifications
            .iter()
            .flat_map(|c| c.pictograms.iter().map(|p| p.code.clone()))
            .collect();
        codes.sort();
        codes.dedup();
        codes
    }

    /// The card as text: the compound, then each source with its
    /// signal word, pictograms and statements, then where it is from.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "{}{}\n",
            self.name.as_deref().unwrap_or(&self.query),
            self.cid
                .map(|c| format!(" (PubChem CID {c})"))
                .unwrap_or_default()
        );
        if let Some(m) = &self.message {
            out.push_str(m);
            out.push('\n');
        }
        for c in &self.classifications {
            out.push_str(&format!("\nSource: {}\n", c.source));
            if let Some(s) = &c.signal {
                out.push_str(&format!("Signal word: {s}\n"));
            }
            if !c.pictograms.is_empty() {
                let p: Vec<String> = c
                    .pictograms
                    .iter()
                    .map(|p| match &p.label {
                        Some(l) => format!("{} ({l})", p.code),
                        None => p.code.clone(),
                    })
                    .collect();
                out.push_str(&format!("Pictograms: {}\n", p.join(", ")));
            }
            for h in &c.hazards {
                out.push_str(&format!("{}\n", h.text));
            }
            if !c.precautionary.is_empty() {
                out.push_str(&format!("Precautionary: {}\n", c.precautionary.join(", ")));
            }
        }
        out.push_str(&format!(
            "\nFrom PubChem, {} (retrieved {})\n",
            self.source_url, self.retrieved
        ));
        out
    }
}

/// The UTC date (`YYYY-MM-DD`) of `ms` milliseconds since the Unix epoch.
#[must_use]
pub fn utc_date(ms: u64) -> String {
    // Howard Hinnant's days-to-civil.
    let days = i64::try_from(ms / 86_400_000).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_symbols() {
        assert_eq!(utc_date(0), "1970-01-01");
        // 2026-10-25T12:00:00Z.
        assert_eq!(utc_date(1_792_929_600_000), "2026-10-25");
        assert_eq!(utc_date(951_782_400_000), "2000-02-29");
        let p = Pictogram {
            code: "GHS02".into(),
            label: Some("Flammable".into()),
        };
        assert_eq!(p.symbol_name(), Some("Flame"));
        let empty = GhsSpec {
            query: "x".into(),
            cid: None,
            name: None,
            classifications: Vec::new(),
            source_url: "https://pubchem.ncbi.nlm.nih.gov".into(),
            fetched_url: "https://pubchem.ncbi.nlm.nih.gov/rest".into(),
            retrieved: "2026-10-25".into(),
            message: None,
        };
        assert!(empty.validate().is_err());
        let said = GhsSpec {
            message: Some("not found".into()),
            ..empty
        };
        assert!(said.validate().is_ok());
        assert!(!said.found());
        assert!(said.to_text().contains("retrieved 2026-10-25"));
    }
}
