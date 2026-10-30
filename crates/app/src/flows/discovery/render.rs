//! `discovery.md`: the document rendered from the state and the stored
//! section texts, with one status line per section.

use std::fmt::Write as _;

use domain::discovery::{DiscoveryState, Logged, RecordVerdict, Section, Status};

use crate::ports::discovery::{DiscoveryId, StoredSource};

/// The artifact name of the text of `section`.
#[must_use]
pub fn section_artifact(section: Section) -> String {
    format!("sections/{}.md", section_key(section))
}

/// The snake_case key of `section` (`solution_proposal`).
#[must_use]
pub fn section_key(section: Section) -> String {
    serde_json::to_value(section)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| section.heading().to_lowercase().replace(' ', "_"))
}

/// The status line of `section` in `state`.
#[must_use]
pub fn section_status(state: &DiscoveryState, section: Section) -> String {
    if let Some(approval) = state.approved.get(&section) {
        let note = approval
            .note
            .as_deref()
            .map(|n| format!(" · note: {n}"))
            .unwrap_or_default();
        return format!(
            "approved by {} at seq {} ({:?}){note}",
            approval.by, approval.seq, approval.choice
        );
    }
    if state.open() == Some(section) {
        return match (&state.written, &state.audit) {
            (Some(_), Some(audit)) => format!(
                "written and audited: {} error(s), {} warning(s){}",
                audit.errors,
                audit.warnings,
                if audit.isolated {
                    ", isolated reviewer ran"
                } else {
                    ""
                }
            ),
            (Some(_), None) => "written, not audited".to_owned(),
            _ => {
                let missing = state.gate_missing(section);
                if missing.is_empty() {
                    "open, not written".to_owned()
                } else {
                    format!("gate closed: {}", missing.join("; "))
                }
            }
        };
    }
    "not written".to_owned()
}

/// Renders the document of `id` from `state`, the `texts` of its sections
/// (by section) and its `sources`.
#[must_use]
pub fn discovery_markdown(
    id: &DiscoveryId,
    state: &DiscoveryState,
    texts: &[(Section, String)],
    sources: &[StoredSource],
    events: &[Logged],
) -> String {
    let mut out = String::new();
    let status = match state.status {
        Status::Open => "open",
        Status::Accepted => "accepted",
        Status::Aborted => "aborted",
    };
    let _ = writeln!(out, "# Discovery {id}\n");
    let _ = writeln!(
        out,
        "_Level: {:?} · status: {status} · {} of {} sections approved · {} events_\n",
        state.level,
        state.approved.len(),
        state.level.sections().len(),
        events.len()
    );
    let _ = writeln!(out, "**Request.** {}\n", state.problem.trim());
    for section in state.level.sections() {
        let _ = writeln!(out, "## {section}\n");
        let _ = writeln!(out, "_{}_\n", section_status(state, *section));
        if let Some((_, text)) = texts.iter().find(|(s, _)| s == section)
            && !text.trim().is_empty()
        {
            let _ = writeln!(out, "{}\n", text.trim());
        }
    }
    if !sources.is_empty() {
        out.push_str("## Sources\n\n");
        for s in sources {
            let title = s.source.title.as_deref().unwrap_or("untitled");
            let _ = writeln!(
                out,
                "- **[{}]** {} · {title} · {} · `{}`",
                s.id,
                s.source.kind,
                s.source.location,
                &s.hash[..s.hash.len().min(12)]
            );
        }
        out.push('\n');
    }
    let confirmed: Vec<String> = state
        .records
        .iter()
        .filter(|(_, r)| r.verdict == RecordVerdict::Confirmed)
        .map(|(r, st)| format!("`{r}` (seq {}, by {})", st.seq, st.by))
        .collect();
    if !confirmed.is_empty() {
        let _ = writeln!(out, "## Records\n\nConfirmed: {}.\n", confirmed.join(", "));
    }
    if !state.spikes.is_empty() {
        out.push_str("## Spikes\n\n");
        for (id, spike) in &state.spikes {
            let score = spike
                .score
                .map(|(s, at)| format!("{s:?} at seq {at}"))
                .unwrap_or_else(|| "not scored".to_owned());
            let _ = writeln!(out, "- {id}: {score} (worktree `{}`)", spike.worktree);
        }
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "## Trail\n\n{} events, {} reopen(s), {} refusal(s). The log is `events.jsonl`.\n",
        events.len(),
        state.reopens,
        state.refusals
    );
    out
}

/// The log as JSON lines.
#[must_use]
pub fn events_jsonl(events: &[Logged]) -> String {
    let mut out = String::new();
    for e in events {
        if let Ok(line) = serde_json::to_string(e) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::discovery::{DiscoveryEvent, Level, replay};

    use super::*;

    #[test]
    fn keys_and_artifacts() {
        assert_eq!(section_key(Section::SolutionProposal), "solution_proposal");
        assert_eq!(section_artifact(Section::Goal), "sections/goal.md");
    }

    #[test]
    fn the_document_lists_every_section_with_its_status() {
        let events = vec![
            Logged::new(
                1,
                1,
                DiscoveryEvent::Started {
                    problem: "slow".into(),
                    level: Level::Research,
                },
            ),
            Logged::new(
                2,
                2,
                DiscoveryEvent::SectionWritten {
                    section: Section::Goal,
                    hash: "h".into(),
                },
            ),
        ];
        let state = replay(&events).unwrap();
        let md = discovery_markdown(
            &DiscoveryId::new("0001-slow"),
            &state,
            &[(Section::Goal, "Enable x.".into())],
            &[],
            &events,
        );
        assert!(md.starts_with("# Discovery 0001-slow"));
        assert!(md.contains("## Goal\n\n_written, not audited_\n\nEnable x."));
        assert!(md.contains("## Background\n\n_not written_"));
        assert!(!md.contains("## Current system"));
        assert!(md.contains("2 events, 0 reopen(s), 0 refusal(s)"));
    }
}
