//! The deterministic part of the critical review: checks that need no
//! model and run first, so a Spec citing unknown evidence is rejected
//! without spending a request.

use domain::plan::Plan;
use domain::research::{ResearchState, Stage, extract_citations};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::super::spec::render::{SECTIONS, SECTIONS_ES};

/// Stage that collects evidence; claims without sources go back there.
pub const EVIDENCE_STAGE: Stage = Stage::Data;

/// Where claims without sources go back to: [`EVIDENCE_STAGE`], or
/// Observation when the graph has no Data.
pub const EVIDENCE_STAGES: [Stage; 2] = [EVIDENCE_STAGE, Stage::Observation];

/// Section of `spec.md` whose paragraphs are factual claims.
const BACKGROUND: usize = 1;

/// Longest claim quoted in a gap.
const CLAIM_CHARS: usize = 200;

/// How much a gap matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Severity {
    /// The Spec cannot be implemented or defended as is.
    #[serde(rename = "major", alias = "grave")]
    Major,
    /// An improvement that does not block the Spec.
    #[serde(rename = "minor", alias = "menor")]
    Minor,
}

impl Severity {
    /// Name shown to the user.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
        }
    }
}

/// A hole found by the review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Gap {
    /// Claim, section or item of the Spec or the plan with the gap (a short verbatim quote).
    pub claim: String,
    /// What is missing or wrong, in one sentence.
    pub problem: String,
    /// The earliest stage of the flow where the information to close the gap is missing.
    pub missing_stage: Stage,
    /// "major" if it prevents implementing or defending the Spec; "minor" if it is an improvement.
    pub severity: Severity,
}

impl Gap {
    pub(crate) fn major(claim: impl Into<String>, problem: impl Into<String>, to: Stage) -> Self {
        Self {
            claim: short(&claim.into()),
            problem: problem.into(),
            missing_stage: to,
            severity: Severity::Major,
        }
    }

    pub(crate) fn minor(claim: impl Into<String>, problem: impl Into<String>, to: Stage) -> Self {
        Self {
            severity: Severity::Minor,
            ..Self::major(claim, problem, to)
        }
    }

    /// Whether it blocks the Spec.
    #[must_use]
    pub fn is_major(&self) -> bool {
        self.severity == Severity::Major
    }
}

/// `text` on one line, cut at [`CLAIM_CHARS`].
fn short(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= CLAIM_CHARS {
        return line;
    }
    let cut: String = line.chars().take(CLAIM_CHARS).collect();
    format!("{cut}…")
}

/// The `## ` sections of `markdown` as (heading, body), in order.
fn sections(markdown: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in markdown.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            out.push((heading.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

/// Whether `body` has any line that is not blank or a heading.
fn has_content(body: &str) -> bool {
    body.lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#'))
}

/// Paragraphs and list items of `body`, outside code fences, headings
/// and tables.
fn claims(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut fenced = false;
    let flush = |current: &mut String, out: &mut Vec<String>| {
        if !current.trim().is_empty() {
            out.push(current.trim().to_owned());
        }
        current.clear();
    };
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut current, &mut out);
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let item = trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('|') || item {
            flush(&mut current, &mut out);
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('|') {
            continue;
        }
        current.push_str(trimmed);
        current.push(' ');
    }
    flush(&mut current, &mut out);
    out
}

/// Gaps of `spec_md` against the evidence of `state`: unknown citations,
/// empty sections of the template, uncited background claims and
/// constraints without a test. Missing evidence is asked to
/// `evidence_stage` (one of [`EVIDENCE_STAGES`]).
#[must_use]
pub fn spec_gaps(spec_md: &str, state: &ResearchState, evidence_stage: Stage) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let cited = extract_citations(spec_md);
    if let Err(unknown) = state.cites_only_known_evidence(&cited) {
        for id in unknown {
            let claim = spec_md
                .lines()
                .find(|l| extract_citations(l).contains(&id))
                .unwrap_or_default();
            gaps.push(Gap::major(
                claim,
                format!("cites evidence {id}, which does not exist"),
                evidence_stage,
            ));
        }
    }
    let found = sections(spec_md);
    // A section is found by its English heading or by the Spanish one of
    // the Specs written by earlier versions.
    let section = |i: usize| {
        found
            .iter()
            .find(|(h, _)| h == SECTIONS[i] || h == SECTIONS_ES[i])
            .map(|(_, b)| b)
    };
    for (i, heading) in SECTIONS.iter().enumerate() {
        if !section(i).is_some_and(|b| has_content(b)) {
            gaps.push(Gap::major(
                format!("section \"{heading}\""),
                "the required section of the template is missing or empty",
                Stage::Spec,
            ));
        }
    }
    if let Some(body) = section(BACKGROUND) {
        for claim in claims(body) {
            if extract_citations(&claim).is_empty() {
                gaps.push(Gap::major(
                    claim,
                    "the claim cites no evidence",
                    evidence_stage,
                ));
            }
        }
    }
    for c in &state.constraints {
        if c.test
            .as_ref()
            .is_none_or(|t| t.reference.trim().is_empty())
        {
            gaps.push(Gap::major(
                format!("{}: {}", c.id, c.statement),
                "the constraint has no test that proves it",
                Stage::Definition,
            ));
        }
    }
    gaps
}

/// Gaps of the `plan` of `folder`: it validates and covers the
/// hypothesis and every constraint.
#[must_use]
pub fn plan_gaps(folder: &str, plan: &Plan, state: &ResearchState) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let claim = format!("{folder}/plan.json");
    let known: Vec<_> = state.constraints.iter().map(|c| c.id).collect();
    let errors = plan
        .validate()
        .err()
        .into_iter()
        .chain(plan.check_constraints(&known).err())
        .flatten();
    for e in errors {
        gaps.push(Gap::major(
            claim.clone(),
            format!("the plan does not validate: {e}"),
            Stage::Plan,
        ));
    }
    if plan.hypothesis_tests.is_empty() {
        gaps.push(Gap::major(
            claim.clone(),
            "the plan has no hypothesis tests (hypothesis_tests)",
            Stage::Plan,
        ));
    }
    for c in &state.constraints {
        if !plan.constraint_tests.iter().any(|t| t.constraint == c.id) {
            gaps.push(Gap::major(
                claim.clone(),
                format!(
                    "the plan does not test constraint {} (constraint_tests)",
                    c.id
                ),
                Stage::Plan,
            ));
        }
    }
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_split_paragraphs_and_items_and_skip_code() {
        let body = "One [E1]\ngoes on.\n\n- two\n- three [E2]\n\n```\nnothing\n```\n| a | b |\n### x\n1. four\n";
        assert_eq!(
            claims(body),
            ["One [E1] goes on.", "- two", "- three [E2]", "1. four"]
        );
    }

    #[test]
    fn severities_read_their_spanish_names_too() {
        for (name, severity) in [
            ("major", Severity::Major),
            ("grave", Severity::Major),
            ("minor", Severity::Minor),
            ("menor", Severity::Minor),
        ] {
            let read: Severity = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(read, severity);
        }
    }

    #[test]
    fn spanish_section_headings_still_count() {
        let state = ResearchState::default();
        let english: String = SECTIONS
            .iter()
            .map(|h| format!("## {h}\n\n| x |\n\n"))
            .collect();
        let spanish: String = SECTIONS_ES
            .iter()
            .map(|h| format!("## {h}\n\n| x |\n\n"))
            .collect();
        assert!(spec_gaps(&english, &state, Stage::Data).is_empty());
        assert!(spec_gaps(&spanish, &state, Stage::Data).is_empty());
        let gaps = spec_gaps("## 1. Problem\n\nx\n", &state, Stage::Data);
        assert_eq!(gaps.len(), 9);
        assert!(gaps[0].problem.contains("missing or empty"), "{gaps:?}");
    }

    #[test]
    fn long_claims_are_cut() {
        let claim = Gap::major("a ".repeat(300), "p", Stage::Spec).claim;
        assert_eq!(claim.chars().count(), CLAIM_CHARS + 1);
    }
}
