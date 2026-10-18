//! The deterministic part of the critical review: checks that need no
//! model and run first, so a Spec citing unknown evidence is rejected
//! without spending a request.

use domain::plan::Plan;
use domain::research::{ResearchState, Stage, extract_citations};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::super::spec::render::SECTIONS;

/// Stage that collects evidence; claims without sources go back there.
pub const EVIDENCE_STAGE: Stage = Stage::Data;

/// Section of `spec.md` whose paragraphs are factual claims.
const BACKGROUND: &str = SECTIONS[1];

/// Longest claim quoted in a gap.
const CLAIM_CHARS: usize = 200;

/// How much a gap matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Severity {
    /// The Spec cannot be implemented or defended as is.
    #[serde(rename = "grave")]
    Major,
    /// An improvement that does not block the Spec.
    #[serde(rename = "menor")]
    Minor,
}

impl Severity {
    /// Name shown to the user.
    #[must_use]
    pub const fn name_es(self) -> &'static str {
        match self {
            Self::Major => "grave",
            Self::Minor => "menor",
        }
    }
}

/// A hole found by the review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Gap {
    /// Afirmación, sección o elemento del Spec o del plan con el hueco (cita textual breve).
    pub claim: String,
    /// Qué falta o qué está mal, en una oración.
    pub problem: String,
    /// La etapa más temprana del flujo donde falta la información para cerrar el hueco.
    pub missing_stage: Stage,
    /// "grave" si impide implementar o sostener el Spec; "menor" si es una mejora.
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
/// constraints without a test.
#[must_use]
pub fn spec_gaps(spec_md: &str, state: &ResearchState) -> Vec<Gap> {
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
                format!("cita la evidencia {id}, que no existe"),
                EVIDENCE_STAGE,
            ));
        }
    }
    let found = sections(spec_md);
    for heading in SECTIONS {
        let body = found.iter().find(|(h, _)| h == heading).map(|(_, b)| b);
        if !body.is_some_and(|b| has_content(b)) {
            gaps.push(Gap::major(
                format!("sección «{heading}»"),
                "la sección obligatoria de la plantilla falta o está vacía",
                Stage::Spec,
            ));
        }
    }
    if let Some((_, body)) = found.iter().find(|(h, _)| h == BACKGROUND) {
        for claim in claims(body) {
            if extract_citations(&claim).is_empty() {
                gaps.push(Gap::major(
                    claim,
                    "la afirmación no cita ninguna evidencia",
                    EVIDENCE_STAGE,
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
                "la restricción no tiene una prueba que la demuestre",
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
            format!("el plan no valida: {e}"),
            Stage::Plan,
        ));
    }
    if plan.hypothesis_tests.is_empty() {
        gaps.push(Gap::major(
            claim.clone(),
            "el plan no tiene pruebas de la hipótesis (hypothesis_tests)",
            Stage::Plan,
        ));
    }
    for c in &state.constraints {
        if !plan.constraint_tests.iter().any(|t| t.constraint == c.id) {
            gaps.push(Gap::major(
                claim.clone(),
                format!(
                    "el plan no prueba la restricción {} (constraint_tests)",
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
        let body = "Uno [E1]\nsigue.\n\n- dos\n- tres [E2]\n\n```\nnada\n```\n| a | b |\n### x\n1. cuatro\n";
        assert_eq!(
            claims(body),
            ["Uno [E1] sigue.", "- dos", "- tres [E2]", "1. cuatro"]
        );
    }

    #[test]
    fn long_claims_are_cut() {
        let claim = Gap::major("a ".repeat(300), "p", Stage::Spec).claim;
        assert_eq!(claim.chars().count(), CLAIM_CHARS + 1);
    }
}
