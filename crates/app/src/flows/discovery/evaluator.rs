//! Verdicts: the isolated evaluator model and the reading of a person's
//! answer.

use domain::discovery::{Section, Verdict};
use domain::model::ModelRole;
use serde::{Deserialize, Serialize};

use super::audit::AuditReport;
use super::error::DiscoveryError;
use super::model::Model;
use super::packet::Packet;
use crate::decide::Decider;
use crate::flows::research::gate::{approves, is_approval};

/// The role that evaluates.
pub const EVALUATOR_ROLE: ModelRole = ModelRole::Plan;

/// Autonomous review rounds a section gets before a person decides.
pub const MAX_MODEL_ROUNDS: u32 = 2;

/// What the evaluator answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evaluation {
    /// The verdict.
    pub verdict: Verdict,
    /// Why, or what to fix.
    #[serde(default)]
    pub note: String,
}

/// The system prompt of the evaluator.
pub const EVALUATOR_SYSTEM: &str = "\
You are the isolated evaluator of a discovery document. You see the packet, one draft section \
and its audit. You decide whether the section is good enough to build on. Answer with only a \
JSON object: {\"verdict\": \"approve-as-is\" | \"approve-with-notes\" | \"reject-and-revise\", \
\"note\": \"...\"}. Approve as is when nothing needs fixing; approve with notes when the \
remaining problems are worth recording but do not change what later sections build on; reject \
and revise when a later section would inherit a mistake, saying exactly what to fix. Never \
approve a draft whose audit reports errors.";

/// Asks the evaluator for a verdict on `draft`.
///
/// # Errors
///
/// The model's.
pub async fn evaluate(
    model: &Model,
    packet: &Packet,
    draft: &str,
    audit: &AuditReport,
) -> Result<Evaluation, DiscoveryError> {
    let prompt = format!(
        "{}\n# Draft\n\n{}\n\n# Audit\n\n{}\n",
        packet.render(),
        draft.trim(),
        audit.markdown()
    );
    let (errors, _) = audit.counts();
    model
        .ask_json::<Evaluation>(
            EVALUATOR_ROLE,
            EVALUATOR_SYSTEM,
            &prompt,
            &format!("evaluating {}", packet.section),
            |e| {
                if errors > 0 && e.verdict.approves() {
                    return Err(format!(
                        "the audit reports {errors} error(s); an approval is not allowed"
                    ));
                }
                if e.verdict == Verdict::RejectAndRevise && e.note.trim().is_empty() {
                    return Err("a rejection needs a note saying what to fix".to_owned());
                }
                Ok(())
            },
        )
        .await
}

/// How a person's line reads as a verdict: a plain yes approves as is;
/// a yes followed by more (`yes, but …`, `ok pero …`) approves with the
/// rest as the note; anything else rejects with the line as the
/// correction. Free text the decision model confidently reads as an
/// approval approves as is. A blank or whitespace-only line (a bare
/// Enter) is `None`: not a verdict, even though the gate reads an empty
/// answer as yes.
pub async fn read_human_verdict(
    text: &str,
    decider: &Decider,
) -> Option<(Verdict, Option<String>)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(classify_human_line(trimmed, decider).await)
}

async fn classify_human_line(trimmed: &str, decider: &Decider) -> (Verdict, Option<String>) {
    if is_approval(trimmed) {
        return (Verdict::ApproveAsIs, None);
    }
    if let Some(note) = yes_but(trimmed) {
        return (Verdict::ApproveWithNotes, Some(note));
    }
    if approves(trimmed, decider).await {
        return (Verdict::ApproveAsIs, None);
    }
    (Verdict::RejectAndRevise, Some(trimmed.to_owned()))
}

/// The note of a `yes, but …` line, if it is one.
#[must_use]
pub fn yes_but(text: &str) -> Option<String> {
    // Fold per char so every byte offset below refers to `text` itself:
    // `str::to_lowercase` can change a char's byte length (`İ`).
    let lower = fold_case(text);
    for yes in [
        "yes", "ok", "okay", "sí", "si", "vale", "approved", "aprobado",
    ] {
        let Some(rest) = lower.strip_prefix(yes) else {
            continue;
        };
        let rest_trim = rest.trim_start_matches([',', ';', ':', ' ']);
        if rest_trim.is_empty() {
            continue;
        }
        let connective = ["but ", "pero ", "although ", "aunque ", "with ", "con "]
            .iter()
            .find(|w| rest_trim.starts_with(*w));
        if connective.is_some() || rest.starts_with([',', ';', ':']) {
            let offset = text.len() - rest_trim.len() + connective.map_or(0, |w| w.len());
            return Some(text[offset..].trim().to_owned());
        }
    }
    None
}

/// `text` lowercased char by char, keeping any char whose lowercase would
/// change its byte length, so byte offsets match `text` exactly.
fn fold_case(text: &str) -> String {
    text.chars()
        .map(|c| {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) if l.len_utf8() == c.len_utf8() => l,
                _ => c,
            }
        })
        .collect()
}

/// The prompt shown to a person at the verdict of `section`.
#[must_use]
pub fn human_prompt(section: Section, audit: &AuditReport, escalated: bool) -> String {
    let (errors, warnings) = audit.counts();
    let why = if escalated {
        format!(" The evaluator rejected it {MAX_MODEL_ROUNDS} times; the decision is yours.")
    } else {
        String::new()
    };
    format!(
        "`{section}` is written and audited ({errors} error(s), {warnings} warning(s)).{why} \
         Answer `yes` to approve, `yes, but <note>` to approve with a note, or say what to fix."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yes_but_splits_the_note() {
        assert_eq!(
            yes_but("yes, but cite the RFC"),
            Some("cite the RFC".into())
        );
        assert_eq!(
            yes_but("ok pero quita la segunda frase"),
            Some("quita la segunda frase".into())
        );
        assert_eq!(yes_but("yes"), None);
        assert_eq!(yes_but("yesterday was fine"), None);
        assert_eq!(yes_but("no, rewrite it"), None);
    }

    #[test]
    fn yes_but_survives_unicode_that_changes_length_when_lowercased() {
        // `İ` lowercases to three bytes; the slice must stay on the text.
        assert_eq!(yes_but("İ yes, but keep İstanbul"), None,);
        assert_eq!(
            yes_but("yes, but İstanbul stays"),
            Some("İstanbul stays".into())
        );
        assert_eq!(yes_but("Yes, but Ünïcode Ñ"), Some("Ünïcode Ñ".into()));
    }

    #[tokio::test]
    async fn a_blank_line_is_not_a_verdict() {
        let decider = Decider::none();
        for blank in [
            "", "   ", "	
",
        ] {
            assert_eq!(read_human_verdict(blank, &decider).await, None);
        }
        assert_eq!(
            read_human_verdict("yes", &decider).await,
            Some((Verdict::ApproveAsIs, None))
        );
    }

    #[tokio::test]
    async fn human_lines_map_to_the_three_verdicts() {
        let decider = Decider::none();
        assert_eq!(
            read_human_verdict("yes", &decider).await,
            Some((Verdict::ApproveAsIs, None))
        );
        assert_eq!(
            read_human_verdict("yes, but shorten it", &decider).await,
            Some((Verdict::ApproveWithNotes, Some("shorten it".into())))
        );
        assert_eq!(
            read_human_verdict("the second bullet is wrong", &decider).await,
            Some((
                Verdict::RejectAndRevise,
                Some("the second bullet is wrong".into())
            ))
        );
    }
}
