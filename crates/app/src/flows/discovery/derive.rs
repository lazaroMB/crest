//! On accept: `spec.md`, `plan.json`, `plan.md`, `review.md` and
//! `sources.json`, derived from the accepted state into the discovery's
//! folder through the spec sink, so `/implement` and `/simulate` read
//! what they read today.

use std::fmt::Write as _;

use domain::discovery::{DiscoveryEvent, DiscoveryState, Logged, Section};
use domain::model::ModelRole;
use domain::plan::Plan;
use domain::research::ConstraintId;

use super::engine::DiscoveryEngine;
use super::error::DiscoveryError;
use super::records::{ClaimRecord, EvidenceRecord};
use crate::flows::research::nodes::spec::render::utc;
use crate::ports::discovery::{DiscoveryId, StoredSource};
use crate::ports::events::NoticeLevel;

/// The texts of the sections of `state`, by section.
async fn texts(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    state: &DiscoveryState,
) -> Result<Vec<(Section, String)>, DiscoveryError> {
    let mut out = Vec::new();
    for s in state.level.sections() {
        if let Some(text) = engine.section_text(id, *s).await? {
            out.push((*s, text));
        }
    }
    Ok(out)
}

fn text_of(texts: &[(Section, String)], section: Section) -> Option<&str> {
    texts
        .iter()
        .find(|(s, _)| *s == section)
        .map(|(_, t)| t.as_str())
}

/// The bullets of a Markdown list (`- ` or `* `).
fn bullets(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let t = l.trim();
            t.strip_prefix("- ")
                .or_else(|| t.strip_prefix("* "))
                .map(str::trim)
                .map(str::to_owned)
        })
        .collect()
}

/// The body under `### <heading>` in `text`, up to the next `### `.
fn subsection(text: &str, heading: &str) -> Option<String> {
    let mut lines = text.lines();
    let wanted = heading.to_lowercase();
    for line in lines.by_ref() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("### ")
            && h.trim().to_lowercase() == wanted
        {
            break;
        }
    }
    let mut body = Vec::new();
    for line in lines {
        if line.trim().starts_with("### ") {
            break;
        }
        body.push(line);
    }
    let body = body.join("\n").trim().to_owned();
    (!body.is_empty()).then_some(body)
}

/// The `### <n>. <Title>` subsections of a solution proposal.
fn numbered_subsections(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("### ") {
            if let Some((title, body)) = current.take() {
                out.push((title, body.join("\n").trim().to_owned()));
            }
            let h = h.trim();
            let numbered = h
                .split_once(". ")
                .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty());
            current = numbered.then(|| (h.to_owned(), Vec::new()));
            continue;
        }
        if let Some((_, body)) = &mut current {
            body.push(line);
        }
    }
    if let Some((title, body)) = current.take() {
        out.push((title, body.join("\n").trim().to_owned()));
    }
    out
}

/// The constraint rows of `spec.md`: `C<n>`, kind, text, test.
fn constraint_rows(constraints: &str) -> Vec<(String, String, String)> {
    bullets(constraints)
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let lower = text.to_lowercase();
            // `ms` as a unit (`20 ms`, `20ms`), not the letters inside `terms`.
            let has_ms = lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| {
                t == "ms"
                    || t.strip_suffix("ms")
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            });
            let kind = if lower.contains("licen") {
                "licensing"
            } else if lower.contains("perform") || lower.contains("latency") || has_ms {
                "performance"
            } else if lower.contains("architect")
                || lower.contains("layer")
                || lower.contains("domain")
            {
                "architecture"
            } else if lower.contains("compat") {
                "compatibility"
            } else {
                "product"
            };
            (format!("C{}", i + 1), kind.to_owned(), text)
        })
        .collect()
}

/// `spec.md` from the accepted state.
#[must_use]
pub fn spec_markdown(
    id: &DiscoveryId,
    state: &DiscoveryState,
    texts: &[(Section, String)],
    sources: &[StoredSource],
    evidence: &[EvidenceRecord],
    claims: &[ClaimRecord],
    records: &[(domain::discovery::RecordRef, String)],
) -> String {
    let rows = constraint_rows(text_of(texts, Section::Constraints).unwrap_or("").trim());
    render_spec(id, state, texts, sources, evidence, claims, records, &rows)
}

/// `spec.md` with the constraint rows computed once by the caller, so the
/// table and `plan.json`'s constraint ids come from the same rows.
#[allow(clippy::too_many_arguments)]
fn render_spec(
    id: &DiscoveryId,
    state: &DiscoveryState,
    texts: &[(Section, String)],
    sources: &[StoredSource],
    evidence: &[EvidenceRecord],
    claims: &[ClaimRecord],
    records: &[(domain::discovery::RecordRef, String)],
    rows: &[(String, String, String)],
) -> String {
    let t = |s: Section| text_of(texts, s).unwrap_or("").trim().to_owned();
    let mut out = String::new();
    let _ = writeln!(out, "# Spec {}\n", id);
    let _ = writeln!(
        out,
        "Derived from discovery `{id}` ({:?} level, {} sections approved). The discovery document is `discovery.md`; the log is `events.jsonl`.\n",
        state.level,
        state.approved.len()
    );
    // 1. Problem
    out.push_str("## 1. Problem\n\n");
    let gap = t(Section::Gap);
    if gap.is_empty() {
        let _ = writeln!(out, "{}\n", t(Section::Goal));
    } else {
        let _ = writeln!(out, "{gap}\n");
    }
    let breakdown = t(Section::Breakdown);
    if !breakdown.is_empty() {
        let _ = writeln!(out, "The problem breaks down into:\n\n{breakdown}\n");
    }
    // 2. Background
    out.push_str("## 2. Background\n\n");
    let _ = writeln!(out, "{}\n", t(Section::Background));
    let findings = t(Section::Findings);
    if !findings.is_empty() {
        let _ = writeln!(out, "### Findings\n\n{findings}\n");
    }
    let current = t(Section::CurrentSystem);
    if !current.is_empty() {
        let _ = writeln!(out, "### Current system\n\n{current}\n");
    }
    // 3. Hypothesis
    out.push_str("## 3. Hypothesis\n\n");
    let desired = t(Section::DesiredState);
    let approach = text_of(texts, Section::SolutionProposal)
        .and_then(|s| subsection(s, "Proposed approach"))
        .unwrap_or_default();
    let approach_paragraph = approach
        .split("\n\n")
        .find(|p| !p.trim_start().starts_with("```"))
        .unwrap_or("")
        .trim()
        .to_owned();
    if desired.is_empty() {
        let _ = writeln!(
            out,
            "If the approach below is implemented, every item of the breakdown is resolved: {}\n",
            approach_paragraph
        );
    } else {
        let _ = writeln!(
            out,
            "If the approach below is implemented, the desired state holds:\n\n{desired}\n"
        );
    }
    // 4. Proposed solution
    out.push_str("## 4. Proposed solution\n\n");
    let _ = writeln!(
        out,
        "{}\n",
        if approach.is_empty() {
            t(Section::SolutionProposal)
        } else {
            approach
        }
    );
    // 5. Functional specification
    out.push_str("## 5. Functional specification\n\n### Sequence\n\n");
    let _ = writeln!(
        out,
        "{}\n",
        if breakdown.is_empty() {
            t(Section::Goal)
        } else {
            breakdown.clone()
        }
    );
    out.push_str("### States\n\n");
    if current.is_empty() {
        out.push_str("The states of the system are described in the Current system of the discovery when the deep level is used; at the research level the solution's subsections below carry them.\n\n");
    } else {
        let _ = writeln!(out, "{current}\n");
    }
    // 6. Technical specification
    out.push_str("## 6. Technical specification\n\n");
    let subs = text_of(texts, Section::SolutionProposal)
        .map(numbered_subsections)
        .unwrap_or_default();
    if subs.is_empty() {
        let _ = writeln!(out, "{}\n", t(Section::SolutionProposal));
    } else {
        for (title, body) in &subs {
            let _ = writeln!(out, "### {title}\n\n{body}\n");
        }
    }
    // 7. Constraints
    out.push_str(
        "## 7. Constraints\n\n| Id | Kind | Constraint | Test |\n| --- | --- | --- | --- |\n",
    );
    for (cid, kind, text) in rows {
        let _ = writeln!(
            out,
            "| {cid} | {kind} | {} | see plan.json constraint_tests |",
            text.replace('|', "\\|")
        );
    }
    out.push('\n');
    // 8. Test protocol
    out.push_str("## 8. Test protocol\n\n");
    out.push_str("- Each task of `plan.json` carries its acceptance (a test or a command); `hypothesis_tests` check the hypothesis above and `constraint_tests` each constraint of section 7.\n");
    if !state.spikes.is_empty() {
        for (sid, spike) in &state.spikes {
            let score = spike
                .score
                .map(|(s, _)| format!("{s:?}"))
                .unwrap_or_else(|| "not scored".to_owned());
            let _ = writeln!(
                out,
                "- Spike {sid} ran before the solution with frozen criteria: {score} (`spikes/{sid}.md`)."
            );
        }
    }
    out.push('\n');
    // 9. Out of scope and risks
    out.push_str("## 9. Out of scope and risks\n\n### Out of scope\n\n");
    let scope = t(Section::Scope);
    match subsection(&scope, "Out") {
        Some(body) => {
            let _ = writeln!(out, "{body}\n");
        }
        None => out.push_str("Nothing recorded.\n\n"),
    }
    out.push_str("### Risks\n\n");
    match records
        .iter()
        .find(|(r, _)| *r == domain::discovery::RecordRef::Risks)
    {
        Some((_, risks)) => {
            let _ = writeln!(out, "{}\n", risks.trim());
        }
        None => {
            let future = text_of(texts, Section::SolutionProposal)
                .and_then(|s| subsection(s, "Future work"))
                .unwrap_or_else(|| {
                    "No risks record at this level; see Future work of the solution.".to_owned()
                });
            let _ = writeln!(out, "{future}\n");
        }
    }
    // 10. Sources: every id the sections cite, then the sources themselves.
    out.push_str("## 10. Sources\n\n");
    for e in evidence {
        let _ = writeln!(
            out,
            "- **[E{}]** {} — from [{}], span: \"{}\"",
            e.id, e.claim, e.source, e.span
        );
    }
    for c in claims {
        let _ = writeln!(
            out,
            "- **[{}]** {} — from [{}], span: \"{}\"",
            c.claim.id,
            c.summary(),
            c.claim.source,
            c.claim.span
        );
    }
    for s in sources {
        let title = s.source.title.as_deref().unwrap_or("untitled");
        let _ = writeln!(
            out,
            "- **[{}]** {} · {title} · {} · fetched {} · `sources/{}.txt`",
            s.id,
            s.source.kind,
            s.source.location,
            utc(s.source.fetched_at),
            s.id
        );
    }
    if evidence.is_empty() && sources.is_empty() {
        out.push_str("No external sources were stored.\n");
    }
    out
}

/// `review.md`: the trail of verdicts, audits, reopens and refusals.
#[must_use]
pub fn review_markdown(state: &DiscoveryState, events: &[Logged]) -> String {
    let mut out = String::from("# Review trail\n\n");
    let _ = writeln!(
        out,
        "{} events · {} reopen(s) · {} refusal(s) · {} source(s) · {} record(s) · {} spike(s)\n",
        events.len(),
        state.reopens,
        state.refusals,
        state.sources.len(),
        state.records.len(),
        state.spikes.len()
    );
    out.push_str(
        "## Verdicts\n\n| Seq | Section | Choice | By | Note |\n| --- | --- | --- | --- | --- |\n",
    );
    for e in events {
        if let DiscoveryEvent::Verdict {
            section,
            choice,
            by,
            note,
        } = &e.event
        {
            let _ = writeln!(
                out,
                "| {} | {section} | {choice:?} | {by} | {} |",
                e.seq,
                note.as_deref()
                    .unwrap_or("")
                    .replace('|', "\\|")
                    .replace('\n', " ")
            );
        }
    }
    out.push_str("\n## Audits\n\n| Seq | Section | Errors | Warnings | Isolated | By |\n| --- | --- | --- | --- | --- | --- |\n");
    for e in events {
        if let DiscoveryEvent::Audited {
            section,
            errors,
            warnings,
            isolated,
            by,
        } = &e.event
        {
            let _ = writeln!(
                out,
                "| {} | {section} | {errors} | {warnings} | {isolated} | {by} |",
                e.seq
            );
        }
    }
    let other: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            DiscoveryEvent::Reopened { section, reason } => {
                Some(format!("- seq {}: reopened `{section}`: {reason}", e.seq))
            }
            DiscoveryEvent::Refused { what, why } => {
                Some(format!("- seq {}: refused `{what}`: {why}", e.seq))
            }
            DiscoveryEvent::RecordChecked {
                record,
                verdict,
                by,
            } => Some(format!(
                "- seq {}: record `{record}` {verdict:?} by {by}",
                e.seq
            )),
            DiscoveryEvent::SpikeScored { spike, score } => {
                Some(format!("- seq {}: spike {spike} {score:?}", e.seq))
            }
            _ => None,
        })
        .collect();
    if !other.is_empty() {
        let _ = writeln!(
            out,
            "\n## Records, spikes, reopens and refusals\n\n{}",
            other.join("\n")
        );
    }
    out
}

/// `sources.json`.
fn sources_json(sources: &[StoredSource], evidence: &[EvidenceRecord]) -> String {
    let value = serde_json::json!({
        "sources": sources.iter().map(|s| serde_json::json!({
            "id": s.id.to_string(),
            "kind": s.source.kind.as_str(),
            "location": s.source.location,
            "title": s.source.title,
            "hash": s.hash,
            "fetched_at": s.source.fetched_at,
        })).collect::<Vec<_>>(),
        "evidence": evidence.iter().map(|e| serde_json::json!({
            "id": format!("E{}", e.id),
            "claim": e.claim,
            "source": e.source.to_string(),
            "span": e.span,
        })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned())
}

/// The planner's instructions.
pub const PLANNER_SYSTEM: &str = "\
You turn an accepted Spec into plan.json for crest's implementation flow. Answer with only the \
JSON object, no text around it: {\"spec\": \"<folder>\", \"tasks\": [{\"id\": \"T1\", \"title\": \
\"...\", \"description\": \"...\", \"depends_on\": [], \"files\": [\"path/\"], \"acceptance\": \
[{\"kind\": \"test\"|\"command\", \"ref\": \"<command line>\"}], \"role\": \"main\"|\"plan\"|\
\"explore\", \"risk\": \"low\"|\"medium\"|\"high\"}], \"hypothesis_tests\": [{\"kind\": \"test\"|\
\"command\", \"ref\": \"...\", \"expect\": \"...\"}], \"constraint_tests\": [{\"constraint\": \
\"C1\", \"kind\": \"test\"|\"command\", \"ref\": \"...\"}]}. One task per subsection of the \
technical specification, in dependency order, each with at least one file and one acceptance; \
ids unique; no cycles; every constraint of section 7 gets a constraint test. Unknown fields are \
rejected.";

/// Derives the Spec folder of `id` from its accepted state.
pub(crate) async fn derive(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
) -> Result<(), DiscoveryError> {
    let loaded = engine.load(id).await?;
    let texts = texts(engine, id, &loaded.state).await?;
    let sources = engine.deps().sources.sources(id).await?;
    let (evidence, claims, records) = engine.records(id).await?;
    let rows = constraint_rows(text_of(&texts, Section::Constraints).unwrap_or("").trim());
    let spec = render_spec(
        id,
        &loaded.state,
        &texts,
        &sources,
        &evidence,
        &claims,
        &records,
        &rows,
    );
    let review = review_markdown(&loaded.state, &loaded.events);
    let sources_json = sources_json(&sources, &evidence);
    let constraint_ids: Vec<ConstraintId> = (1..=rows.len())
        .filter_map(|n| ConstraintId::new(u32::try_from(n).ok()?).ok())
        .collect();
    let folder = id.as_str().to_owned();
    let plan = plan_from_spec(engine, &folder, &spec, &constraint_ids).await;
    let mut files: Vec<(&str, String)> = vec![
        ("spec.md", spec),
        ("review.md", review),
        ("sources.json", sources_json),
    ];
    match plan {
        Ok(plan) => {
            let json = serde_json::to_string_pretty(&plan)
                .map_err(|e| DiscoveryError::Files(format!("plan.json: {e}")))?;
            files.push(("plan.json", json));
            files.push(("plan.md", plan.to_markdown()));
        }
        Err(err) => {
            engine.notice(
                NoticeLevel::Warning,
                format!("plan.json could not be derived: {err}; spec.md is written, `/implement` needs a plan"),
            );
        }
    }
    for (name, content) in &files {
        engine
            .deps()
            .log
            .put_artifact(id, &format!("derived/{name}"), content)
            .await?;
        if let Some(sink) = &engine.deps().sink {
            sink.write(&folder, name, content)
                .await
                .map_err(|e| DiscoveryError::Files(e.to_string()))?;
        }
    }
    Ok(())
}

/// Asks the planner for a valid `plan.json`.
async fn plan_from_spec(
    engine: &DiscoveryEngine,
    folder: &str,
    spec: &str,
    constraints: &[ConstraintId],
) -> Result<Plan, DiscoveryError> {
    let known = constraints.to_vec();
    let folder_owned = folder.to_owned();
    let plan: Plan = engine
        .model()
        .ask_json(
            ModelRole::Plan,
            PLANNER_SYSTEM,
            &format!("Folder: `{folder}`\n\n{spec}"),
            "deriving plan.json",
            move |plan: &Plan| {
                if plan.spec != folder_owned {
                    return Err(format!("`spec` must be `{folder_owned}`"));
                }
                plan.validate().map_err(|errors| {
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                })?;
                plan.check_constraints(&known).map_err(|errors| {
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                })?;
                Ok(())
            },
        )
        .await?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsections_and_bullets_are_found() {
        let text = "### Proposed approach\n\nA paragraph.\n\n```mermaid\nflowchart LR\n```\n\n### 1. First\n\nbody one\n\n### 2. Second\n\nbody two\n\n### Future work\n\nlater";
        assert_eq!(
            subsection(text, "Proposed approach")
                .unwrap()
                .lines()
                .next(),
            Some("A paragraph.")
        );
        assert_eq!(subsection(text, "Future work").as_deref(), Some("later"));
        assert_eq!(
            numbered_subsections(text),
            vec![
                ("1. First".to_owned(), "body one".to_owned()),
                ("2. Second".to_owned(), "body two".to_owned())
            ]
        );
        assert_eq!(bullets("- a\n* b\nc"), vec!["a", "b"]);
        let rows = constraint_rows(
            "- MIT license only\n- the domain gains no I/O\n- p95 under 20 ms\n- Must work for teams and their forms",
        );
        assert_eq!(rows[0].0, "C1");
        assert_eq!(rows[0].1, "licensing");
        assert_eq!(rows[1].1, "architecture");
        assert_eq!(rows[2].1, "performance");
        assert_eq!(rows[3].1, "product", "`teams` and `forms` are not a unit");
    }
}
