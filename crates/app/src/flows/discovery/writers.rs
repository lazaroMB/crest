//! The section writers: one prompt per section quoting its format
//! contract, retried with the deterministic findings until the contract
//! holds or three attempts are spent.

use domain::discovery::contract::counts;
use domain::discovery::{Finding, Format, Level, Phase, Section, check_section};
use domain::model::ModelRole;

use super::error::DiscoveryError;
use super::model::Model;
use super::packet::Packet;

/// Attempts a writer gets before the engine gives up on a section.
pub const MAX_ATTEMPTS: usize = 3;

/// The role that writes sections.
pub const WRITER_ROLE: ModelRole = ModelRole::Plan;

/// The format contract of `section`, as the writer and the reviewer read
/// it.
#[must_use]
pub fn contract_text(section: Section) -> String {
    let format = match section.format() {
        Format::Prose {
            min_paragraphs,
            max_paragraphs,
        } if min_paragraphs == max_paragraphs => {
            format!("exactly {min_paragraphs} paragraph(s) of prose: no lists, no headings, no code")
        }
        Format::Prose {
            min_paragraphs,
            max_paragraphs,
        } => format!(
            "{min_paragraphs} to {max_paragraphs} paragraphs of prose: no lists, no headings, no code"
        ),
        Format::Bullets { min } => format!("a bullet list of at least {min} items and nothing else"),
        Format::InOut { min_each } => format!(
            "two headings, `### In` and `### Out`, each followed by a bullet list of at least {min_each} item(s)"
        ),
        Format::Categories { min, max } => format!(
            "{min} to {max} `#### <Category>` headings, each followed by bullets; every bullet is one confirmed fact ending with its citation"
        ),
        Format::ParagraphAndMermaid => {
            "exactly one paragraph followed by exactly one ```mermaid fenced block".to_owned()
        }
        Format::Numbered { min } => format!(
            "a numbered list of at least {min} items and nothing else (an optional lead-in line ending in `:` is allowed)"
        ),
        Format::Proposal => "`### Proposed approach` (one paragraph and one ```mermaid block), then one `### <n>. <Title>` subsection per numbered item of the Breakdown, then `### Future work`".to_owned(),
    };
    let what = match section {
        Section::Goal => {
            "One sentence of the form `Enable <who> to <capability>, providing <outcome>.`"
        }
        Section::Background => {
            "What the system does today, what triggers this request and the terms used; no diagnosis, no solution."
        }
        Section::Constraints => {
            "Research, product, licensing and environment limits the work must respect."
        }
        Section::Scope => "What is inside and outside this discovery.",
        Section::Findings => {
            "Confirmed facts only, grouped by category, every one cited; no proposals."
        }
        Section::CurrentSystem => {
            "The system as it is, inside the scope: one paragraph and one diagram built from the confirmed records; no proposals."
        }
        Section::DesiredState => "The outcomes wanted, numbered, measurable where possible.",
        Section::Gap => {
            "What is missing between the current system and the desired state, in one or two paragraphs."
        }
        Section::Breakdown => "Numbered, measurable problem statements, each one a unit of work.",
        Section::SolutionProposal => {
            "The approach, one subsection per Breakdown item citing the records and spikes it rests on, and the future work."
        }
    };
    let phase = match section.phase() {
        Phase::Framing => {
            "Framing: do not mention methods (CATWOE, causal loop, CLD, records/) nor solutions."
        }
        Phase::Problem => {
            "Problem: state facts with citations; phrases such as `we propose`, `we should`, `the solution` or `proposal` do not belong here."
        }
        Phase::Synthesis => {
            "Synthesis: what is wanted and what is missing, derived from the approved sections."
        }
        Phase::Solution => {
            "Solution: every subsection answers one Breakdown item and cites what it rests on."
        }
    };
    format!("{what}\nFormat: {format}.\n{phase}")
}

/// The system prompt of the writer of `section` at `level`.
#[must_use]
pub fn system_prompt(section: Section, level: Level) -> String {
    let level = match level {
        Level::Research => "a research-level discovery: the repository and a few web pages",
        Level::Deep => {
            "a deep discovery: the literature, causal claims, a causal diagram, risks and spikes"
        }
    };
    format!(
        "You write one section of a discovery document for crest, a coding assistant. This is {level}.\n\
         Write only the body of the section `{section}`: no heading, no preface, no closing remark.\n\
         {contract}\n\
         Cite only the ids listed under \"What you may cite\", as `[ID]`; never invent a source, a number or a quote.\n\
         Write in English, plainly, in the present tense.",
        contract = contract_text(section)
    )
}

/// What a write attempt produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// The text.
    pub text: String,
    /// The findings of the deterministic contract on it.
    pub findings: Vec<Finding>,
    /// Attempts spent.
    pub attempts: u32,
}

impl Draft {
    /// Whether the contract holds (no errors).
    #[must_use]
    pub fn passes(&self) -> bool {
        counts(&self.findings).0 == 0
    }
}

/// The findings as the writer reads them back.
#[must_use]
pub fn findings_text(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| format!("- {:?}: {}", f.severity, f.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Writes `packet.section` with `model`: up to [`MAX_ATTEMPTS`] drafts,
/// each checked with the contract and, when it fails, sent back with the
/// findings. Returns the last draft either way; the caller decides what a
/// failing one means.
///
/// # Errors
///
/// The model's.
pub async fn write_section(model: &Model, packet: &Packet) -> Result<Draft, DiscoveryError> {
    let system = system_prompt(packet.section, packet.level);
    let citable = packet.citable_ids();
    let mut packet = packet.clone();
    // The human's rejection note or the previous audit's findings stay in
    // every retry prompt; only the latest contract findings are appended.
    let original_correction = packet.correction.clone();
    let mut last: Option<Draft> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let text = model
            .ask(
                WRITER_ROLE,
                &system,
                &packet.render(),
                &format!("writing {} (attempt {attempt})", packet.section),
            )
            .await?;
        let text = strip_heading(&text, packet.section);
        let findings = check_section(packet.section, &text, &citable);
        let draft = Draft {
            text,
            findings,
            attempts: u32::try_from(attempt).unwrap_or(u32::MAX),
        };
        if draft.passes() {
            return Ok(draft);
        }
        packet.correction = Some(retry_correction(
            original_correction.as_deref(),
            &findings_text(&draft.findings),
        ));
        last = Some(draft);
    }
    Ok(last.unwrap_or_else(|| Draft {
        text: String::new(),
        findings: Vec::new(),
        attempts: 0,
    }))
}

/// The correction for a retry: the original note (if any), then the
/// latest contract findings.
fn retry_correction(original: Option<&str>, findings: &str) -> String {
    let contract = format!(
        "The deterministic contract found:\n{findings}\n\nRewrite the whole section so none of these remain."
    );
    match original.map(str::trim).filter(|o| !o.is_empty()) {
        Some(note) => format!("{note}\n\n{contract}"),
        None => contract,
    }
}

/// Removes a leading `## <section>` heading (or any `## ` heading) the
/// model may have repeated, and surrounding blank lines.
#[must_use]
pub fn strip_heading(text: &str, section: Section) -> String {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    let heading = first.trim();
    let is_heading = heading.starts_with("## ")
        && !heading.starts_with("### ")
        && (heading[3..].trim().eq_ignore_ascii_case(section.heading())
            || heading[3..].trim().parse::<Section>() == Ok(section));
    if is_heading {
        lines.collect::<Vec<_>>().join("\n").trim().to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_text_names_the_format_and_the_phase() {
        let goal = contract_text(Section::Goal);
        assert!(goal.contains("exactly 1 paragraph"));
        assert!(goal.contains("Framing"));
        let findings = contract_text(Section::Findings);
        assert!(findings.contains("1 to 5 `#### <Category>`"));
        assert!(findings.contains("Problem"));
        assert!(contract_text(Section::SolutionProposal).contains("### Future work"));
    }

    #[test]
    fn a_retry_keeps_the_original_correction_and_only_the_latest_findings() {
        let note = "name the developers, not the users";
        let first = retry_correction(Some(note), "- one");
        assert!(first.starts_with(&format!("{note}\n\n")));
        assert!(first.contains("- one"));
        let second = retry_correction(Some(note), "- two");
        assert!(second.contains(note));
        assert!(second.contains("- two") && !second.contains("- one"));
        assert!(retry_correction(None, "- x").starts_with("The deterministic contract"));
    }

    #[test]
    fn a_repeated_heading_is_stripped() {
        assert_eq!(
            strip_heading("## Goal\n\nEnable x.", Section::Goal),
            "Enable x."
        );
        assert_eq!(
            strip_heading(
                "## Solution proposal\n### Proposed approach\nx",
                Section::SolutionProposal
            ),
            "### Proposed approach\nx"
        );
        assert_eq!(strip_heading("### In\n- a", Section::Scope), "### In\n- a");
        assert_eq!(
            strip_heading("## Scope\n- a", Section::Goal),
            "## Scope\n- a"
        );
    }
}
