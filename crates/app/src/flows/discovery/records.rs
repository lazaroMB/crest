//! The records of a deep discovery (evidence, claims, the structural map,
//! the variables, the causal loop diagram, the risks): their stored forms
//! and how the engine produces and checks them to open a gate.

use domain::discovery::{Claim, SourceId};
use serde::{Deserialize, Serialize};

/// A piece of evidence: a statement with a verbatim span of a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    /// Its id (`E3`).
    pub id: u32,
    /// The statement.
    pub claim: String,
    /// The source the span is from.
    pub source: SourceId,
    /// The verbatim span.
    pub span: String,
}

/// A causal claim as stored: the claim and the human-readable summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRecord {
    /// The claim.
    #[serde(flatten)]
    pub claim: Claim,
}

impl ClaimRecord {
    /// `origin → destination (sign)`.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} → {} ({}, {})",
            self.claim.origin,
            self.claim.destination,
            self.claim.direction,
            self.claim.stated_sign.symbol()
        )
    }
}

/// Artifact names of the records.
pub mod names {
    /// The evidence list, JSON.
    pub const EVIDENCE: &str = "records/evidence.json";
    /// The claims list, JSON.
    pub const CLAIMS: &str = "records/claims.json";
    /// The structural map, Markdown.
    pub const STRUCTURE: &str = "records/structure.md";
    /// The variables, Markdown (one `- name: direction` per line).
    pub const VARIABLES: &str = "records/variables.md";
    /// The computed CLD, Markdown with Mermaid.
    pub const CLD: &str = "records/cld.md";
    /// The risks, Markdown.
    pub const RISKS: &str = "records/risks.md";
}

/// The variable names of a `variables.md` record: the text after `- ` up
/// to the first `:` or `(` of each bullet.
#[must_use]
pub fn variable_names(variables_md: &str) -> Vec<String> {
    variables_md
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("- ")
                .or_else(|| l.trim().strip_prefix("* "))
        })
        .map(|rest| {
            let end = rest.find([':', '(']).unwrap_or(rest.len());
            rest[..end].trim().trim_matches('`').to_owned()
        })
        .filter(|n| !n.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_names_come_from_bullets() {
        let md = "A sentence.\n\n- prompts per hour: more prompts, more habituation\n- `attention to each prompt` (falls)\n* unsafe approvals\nnot a bullet";
        assert_eq!(
            variable_names(md),
            vec![
                "prompts per hour",
                "attention to each prompt",
                "unsafe approvals"
            ]
        );
    }
}
