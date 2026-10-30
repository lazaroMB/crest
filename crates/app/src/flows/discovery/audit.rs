//! The audit of a written section: the deterministic contract, plus an
//! isolated reviewer that sees only the packet and the draft.

use domain::discovery::contract::counts;
use domain::discovery::{Actor, Finding, Section, Severity, check_section};
use domain::model::ModelRole;
use serde::{Deserialize, Serialize};

use super::error::DiscoveryError;
use super::model::Model;
use super::packet::Packet;
use super::writers::{contract_text, findings_text};

/// The role that reviews.
pub const REVIEWER_ROLE: ModelRole = ModelRole::Explore;

/// What the isolated reviewer answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerReport {
    /// Problems that must be fixed before a verdict.
    #[serde(default)]
    pub errors: Vec<String>,
    /// Problems worth noting that do not block.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// The audit of one draft, as stored next to the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReport {
    /// The section.
    pub section: Section,
    /// The contract's findings.
    pub findings: Vec<Finding>,
    /// The reviewer's report, when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<ReviewerReport>,
    /// Who audited: the engine, or the reviewer model.
    pub by: Actor,
}

impl AuditReport {
    /// Blocking and non-blocking counts, contract and reviewer together.
    #[must_use]
    pub fn counts(&self) -> (u32, u32) {
        let (mut errors, mut warnings) = counts(&self.findings);
        if let Some(r) = &self.reviewer {
            errors = errors.saturating_add(u32::try_from(r.errors.len()).unwrap_or(u32::MAX));
            warnings = warnings.saturating_add(u32::try_from(r.warnings.len()).unwrap_or(u32::MAX));
        }
        (errors, warnings)
    }

    /// Whether the reviewer ran.
    #[must_use]
    pub fn isolated(&self) -> bool {
        self.reviewer.is_some()
    }

    /// Everything found, as the writer reads it back.
    #[must_use]
    pub fn correction(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        if !self.findings.is_empty() {
            lines.push(findings_text(&self.findings));
        }
        if let Some(r) = &self.reviewer {
            lines.extend(r.errors.iter().map(|e| format!("- Error (reviewer): {e}")));
            lines.extend(
                r.warnings
                    .iter()
                    .map(|w| format!("- Warning (reviewer): {w}")),
            );
        }
        lines.join("\n")
    }

    /// The report as a short Markdown block for the document.
    #[must_use]
    pub fn markdown(&self) -> String {
        let (errors, warnings) = self.counts();
        let mut out = format!(
            "Audit of {}: {errors} error(s), {warnings} warning(s), by {}{}.\n",
            self.section,
            self.by,
            if self.isolated() {
                " (isolated reviewer ran)"
            } else {
                ""
            }
        );
        if !self.findings.is_empty() {
            out.push_str(&findings_text(&self.findings));
            out.push('\n');
        }
        if let Some(r) = &self.reviewer {
            for e in &r.errors {
                out.push_str(&format!("- Error (reviewer): {e}\n"));
            }
            for w in &r.warnings {
                out.push_str(&format!("- Warning (reviewer): {w}\n"));
            }
        }
        out
    }
}

/// The system prompt of the isolated reviewer.
pub const REVIEWER_SYSTEM: &str = "\
You are the isolated reviewer of a discovery document. You see only the packet (the request, \
the approved sections, what may be cited) and one draft section. You did not write it and you \
know nothing else. Check that every statement is supported by what the packet allows to cite, \
that every citation is used for what its excerpt says (a qualifier not in the span is an error), \
that vendor claims are marked as such, that nothing is overstated, and that the draft follows \
its contract. Answer with only a JSON object: {\"errors\": [\"...\"], \"warnings\": [\"...\"]}. \
An error must be fixed before a verdict; a warning is worth noting. Be specific: quote the words \
you object to. Do not rewrite the section.";

/// Asks the isolated reviewer about `draft`.
///
/// # Errors
///
/// The model's.
pub async fn review(
    model: &Model,
    packet: &Packet,
    draft: &str,
) -> Result<ReviewerReport, DiscoveryError> {
    let prompt = format!(
        "{}\n\nContract of the section:\n{}\n\n# Draft\n\n{}\n",
        packet.render(),
        contract_text(packet.section),
        draft.trim()
    );
    model
        .ask_json::<ReviewerReport>(
            REVIEWER_ROLE,
            REVIEWER_SYSTEM,
            &prompt,
            &format!("reviewing {}", packet.section),
            |r| {
                if r.errors
                    .iter()
                    .chain(&r.warnings)
                    .any(|s| s.trim().is_empty())
                {
                    return Err("empty entries are not allowed".to_owned());
                }
                Ok(())
            },
        )
        .await
}

/// Audits `draft` for `packet.section`: the contract always, the isolated
/// reviewer when `isolated`.
///
/// # Errors
///
/// The model's.
pub async fn audit(
    model: &Model,
    packet: &Packet,
    draft: &str,
    isolated: bool,
) -> Result<AuditReport, DiscoveryError> {
    let findings = check_section(packet.section, draft, &packet.citable_ids());
    let reviewer = if isolated {
        Some(review(model, packet, draft).await?)
    } else {
        None
    };
    let by = if isolated {
        Actor::Model(model.id_of(REVIEWER_ROLE))
    } else {
        Actor::Process("audit".into())
    };
    Ok(AuditReport {
        section: packet.section,
        findings,
        reviewer,
        by,
    })
}

/// Only the blocking findings of `findings`.
#[must_use]
pub fn errors_of(findings: &[Finding]) -> Vec<&Finding> {
    findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .collect()
}
