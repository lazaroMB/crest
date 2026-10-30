//! Deterministic checks of a section's text: its [`Format`], its
//! citations and the words it must not use.
//!
//! The writer's retry loop and the audit call the same function, so the
//! producer and the reviewer cannot disagree on what a valid section is.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::section::{Format, Phase, Section};

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Blocks a verdict.
    Error,
    /// Does not.
    Warning,
}

/// One thing the check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// How bad.
    pub severity: Severity,
    /// What, in one sentence.
    pub message: String,
}

impl Finding {
    fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
        }
    }

    fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
        }
    }
}

/// Counts of a list of findings.
#[must_use]
pub fn counts(findings: &[Finding]) -> (u32, u32) {
    let errors = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warnings = findings.len() - errors;
    (
        u32::try_from(errors).unwrap_or(u32::MAX),
        u32::try_from(warnings).unwrap_or(u32::MAX),
    )
}

/// What the text may cite: the ids of the confirmed records. A citation
/// is `[<PREFIX><n>]`: `[E3]` evidence, `[S7]` a source, `[CL2]` a claim,
/// `[SP1]` a spike.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Citable {
    ids: BTreeSet<String>,
}

impl Citable {
    /// Nothing may be cited.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Adds an id, e.g. `S7`.
    #[must_use]
    pub fn with(mut self, id: impl Into<String>) -> Self {
        self.ids.insert(id.into());
        self
    }

    /// Adds every id of `ids`.
    #[must_use]
    pub fn with_all<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.ids.extend(ids.into_iter().map(Into::into));
        self
    }

    /// Whether `id` may be cited.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }
}

/// Words a framing section must not contain (method leaks).
const FRAMING_FORBIDDEN: [&str; 4] = ["CATWOE", "causal loop", "CLD", "records/"];

/// Phrases a problem-phase section must not contain (solution leaks).
const PROBLEM_FORBIDDEN: [&str; 4] = ["we propose", "we should", "the solution", "proposal"];

/// Checks `text` (the body under `## <section>`, without the heading)
/// against the contract of `section`, citing only `citable`.
#[must_use]
pub fn check_section(section: Section, text: &str, citable: &Citable) -> Vec<Finding> {
    let doc = Doc::parse(text);
    let mut out = Vec::new();
    check_format(section.format(), &doc, &mut out);
    check_citations(section, &doc, citable, &mut out);
    check_words(section, text, &mut out);
    out
}

/// A parsed section body.
struct Doc {
    blocks: Vec<Block>,
}

/// A block of the body.
#[derive(Debug, PartialEq, Eq)]
enum Block {
    Paragraph(String),
    Bullets(Vec<String>),
    Numbered(Vec<String>),
    Heading { level: usize, text: String },
    Fence { lang: String },
}

impl Doc {
    fn parse(text: &str) -> Self {
        let mut blocks = Vec::new();
        let mut para: Vec<String> = Vec::new();
        let mut bullets: Vec<String> = Vec::new();
        let mut numbered: Vec<String> = Vec::new();
        let mut fence: Option<String> = None;

        fn flush(
            blocks: &mut Vec<Block>,
            para: &mut Vec<String>,
            bullets: &mut Vec<String>,
            numbered: &mut Vec<String>,
        ) {
            if !para.is_empty() {
                blocks.push(Block::Paragraph(para.join(" ")));
                para.clear();
            }
            if !bullets.is_empty() {
                blocks.push(Block::Bullets(std::mem::take(bullets)));
            }
            if !numbered.is_empty() {
                blocks.push(Block::Numbered(std::mem::take(numbered)));
            }
        }

        for raw in text.lines() {
            let line = raw.trim_end();
            if let Some(lang) = fence.take() {
                if line.trim_start().starts_with("```") {
                    blocks.push(Block::Fence { lang });
                } else {
                    fence = Some(lang);
                }
                continue;
            }
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("```") {
                flush(&mut blocks, &mut para, &mut bullets, &mut numbered);
                fence = Some(rest.trim().to_owned());
                continue;
            }
            if trimmed.is_empty() {
                flush(&mut blocks, &mut para, &mut bullets, &mut numbered);
                continue;
            }
            if trimmed.starts_with('#') {
                flush(&mut blocks, &mut para, &mut bullets, &mut numbered);
                let level = trimmed.bytes().take_while(|b| *b == b'#').count();
                blocks.push(Block::Heading {
                    level,
                    text: trimmed.trim_start_matches('#').trim().to_owned(),
                });
                continue;
            }
            if let Some(item) = bullet_item(trimmed) {
                if !para.is_empty() || !numbered.is_empty() {
                    flush(&mut blocks, &mut para, &mut bullets, &mut numbered);
                }
                bullets.push(item.to_owned());
                continue;
            }
            if let Some(item) = numbered_item(trimmed) {
                if !para.is_empty() || !bullets.is_empty() {
                    flush(&mut blocks, &mut para, &mut bullets, &mut numbered);
                }
                numbered.push(item.to_owned());
                continue;
            }
            // A continuation line of a list item, or prose.
            if raw.starts_with(' ') || raw.starts_with('\t') {
                if let Some(last) = bullets.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                    continue;
                }
                if let Some(last) = numbered.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                    continue;
                }
            }
            if !bullets.is_empty() || !numbered.is_empty() {
                flush(&mut blocks, &mut para, &mut bullets, &mut numbered);
            }
            para.push(trimmed.to_owned());
        }
        if let Some(lang) = fence {
            blocks.push(Block::Fence { lang });
        }
        flush(&mut blocks, &mut para, &mut bullets, &mut numbered);
        Self { blocks }
    }

    fn paragraphs(&self) -> usize {
        self.blocks
            .iter()
            .filter(|b| matches!(b, Block::Paragraph(_)))
            .count()
    }

    fn mermaid_fences(&self) -> usize {
        self.blocks
            .iter()
            .filter(|b| matches!(b, Block::Fence { lang } if lang == "mermaid"))
            .count()
    }

    fn has(&self, pred: impl Fn(&Block) -> bool) -> bool {
        self.blocks.iter().any(pred)
    }

    /// Every piece of prose that should carry a citation: paragraphs and
    /// list items.
    fn sentences_units(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for b in &self.blocks {
            match b {
                Block::Paragraph(p) => out.push(p.as_str()),
                Block::Bullets(items) | Block::Numbered(items) => {
                    out.extend(items.iter().map(String::as_str));
                }
                Block::Heading { .. } | Block::Fence { .. } => {}
            }
        }
        out
    }
}

fn bullet_item(line: &str) -> Option<&str> {
    line.strip_prefix("* ")
        .or_else(|| line.strip_prefix("- "))
        .or_else(|| line.strip_prefix("+ "))
}

fn numbered_item(line: &str) -> Option<&str> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = line.get(digits..)?;
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

fn check_format(format: Format, doc: &Doc, out: &mut Vec<Finding>) {
    let any_list = doc.has(|b| matches!(b, Block::Bullets(_) | Block::Numbered(_)));
    let any_heading = doc.has(|b| matches!(b, Block::Heading { .. }));
    let any_fence = doc.has(|b| matches!(b, Block::Fence { .. }));
    match format {
        Format::Prose {
            min_paragraphs,
            max_paragraphs,
        } => {
            if any_list {
                out.push(Finding::error("prose only: no lists"));
            }
            if any_heading {
                out.push(Finding::error("prose only: no sub-headings"));
            }
            if any_fence {
                out.push(Finding::error("prose only: no code or diagrams"));
            }
            let n = doc.paragraphs();
            if n < min_paragraphs || n > max_paragraphs {
                out.push(Finding::error(format!(
                    "{n} paragraph(s); expected between {min_paragraphs} and {max_paragraphs}"
                )));
            }
        }
        Format::Bullets { min } => {
            let bullets: usize = doc
                .blocks
                .iter()
                .map(|b| match b {
                    Block::Bullets(items) => items.len(),
                    _ => 0,
                })
                .sum();
            if doc.has(|b| !matches!(b, Block::Bullets(_))) {
                out.push(Finding::error("a bullet list and nothing else"));
            }
            if bullets < min {
                out.push(Finding::error(format!(
                    "{bullets} bullet(s); expected at least {min}"
                )));
            }
        }
        Format::InOut { min_each } => check_in_out(doc, min_each, out),
        Format::Categories { min, max } => check_categories(doc, min, max, out),
        Format::ParagraphAndMermaid => {
            if doc.paragraphs() != 1 {
                out.push(Finding::error(format!(
                    "{} paragraph(s); expected exactly one",
                    doc.paragraphs()
                )));
            }
            if doc.mermaid_fences() != 1 {
                out.push(Finding::error(format!(
                    "{} mermaid block(s); expected exactly one",
                    doc.mermaid_fences()
                )));
            }
            if any_list || any_heading {
                out.push(Finding::error(
                    "one paragraph and one diagram, nothing else",
                ));
            }
        }
        Format::Numbered { min } => {
            let mut items = 0;
            for (i, b) in doc.blocks.iter().enumerate() {
                match b {
                    Block::Numbered(list) => items += list.len(),
                    // One lead-in line is allowed, first and ending in `:`.
                    Block::Paragraph(p) if i == 0 && p.trim_end().ends_with(':') => {}
                    _ => out.push(Finding::error(
                        "a numbered list, with at most one lead-in line ending in `:`",
                    )),
                }
            }
            if items < min {
                out.push(Finding::error(format!(
                    "{items} item(s); expected at least {min}"
                )));
            }
        }
        Format::Proposal => check_proposal(doc, out),
    }
}

fn check_in_out(doc: &Doc, min_each: usize, out: &mut Vec<Finding>) {
    let mut current: Option<&str> = None;
    let mut counts = [0usize; 2];
    let mut other = false;
    for b in &doc.blocks {
        match b {
            Block::Heading { level: 3, text } if text.eq_ignore_ascii_case("in") => {
                current = Some("in");
            }
            Block::Heading { level: 3, text } if text.eq_ignore_ascii_case("out") => {
                current = Some("out");
            }
            Block::Bullets(items) => match current {
                Some("in") => counts[0] += items.len(),
                Some("out") => counts[1] += items.len(),
                _ => other = true,
            },
            _ => other = true,
        }
    }
    if other {
        out.push(Finding::error(
            "`### In` and `### Out`, each a bullet list, and nothing else",
        ));
    }
    for (name, n) in [("In", counts[0]), ("Out", counts[1])] {
        if n < min_each {
            out.push(Finding::error(format!(
                "`### {name}` has {n} bullet(s); expected at least {min_each}"
            )));
        }
    }
}

fn check_categories(doc: &Doc, min: usize, max: usize, out: &mut Vec<Finding>) {
    let mut categories = 0usize;
    let mut bare_bullets = 0usize;
    let mut expecting_bullets = false;
    for b in &doc.blocks {
        match b {
            Block::Heading { level: 4, .. } => {
                if expecting_bullets {
                    out.push(Finding::error("a category without facts under it"));
                }
                categories += 1;
                expecting_bullets = true;
            }
            Block::Bullets(items) => {
                if expecting_bullets {
                    expecting_bullets = false;
                } else {
                    bare_bullets += items.len();
                }
            }
            _ => out.push(Finding::error(
                "`#### <Category>` headings, each followed by a bullet list of facts",
            )),
        }
    }
    if expecting_bullets {
        out.push(Finding::error("a category without facts under it"));
    }
    if bare_bullets > 0 && categories == 0 {
        categories = 1;
    } else if bare_bullets > 0 {
        out.push(Finding::error("bullets outside any category"));
    }
    if categories < min || categories > max {
        out.push(Finding::error(format!(
            "{categories} categor(ies); expected between {min} and {max}"
        )));
    }
}

fn check_proposal(doc: &Doc, out: &mut Vec<Finding>) {
    let heading = |want: &str| {
        doc.has(
            |b| matches!(b, Block::Heading { level: 3, text } if text.eq_ignore_ascii_case(want)),
        )
    };
    if !heading("Proposed approach") {
        out.push(Finding::error("missing `### Proposed approach`"));
    }
    if !heading("Future work") {
        out.push(Finding::error("missing `### Future work`"));
    }
    let numbered_subsections = doc
        .blocks
        .iter()
        .filter(|b| matches!(b, Block::Heading { level: 3, text } if numbered_item(text).is_some()))
        .count();
    if numbered_subsections == 0 {
        out.push(Finding::error(
            "no `### <n>. <Title>` subsection; one per breakdown item",
        ));
    }
    if doc.mermaid_fences() == 0 {
        out.push(Finding::error(
            "`### Proposed approach` needs one mermaid diagram of the target workflow",
        ));
    }
}

/// Scans citations: `[<LETTERS><DIGITS>]` such as `[E3]`, `[S7]`, `[CL2]`,
/// and `[<lowercase word>]` such as `[structure]` or `[cld]` (the names
/// method records display with). A bracket followed by `(` is a Markdown
/// link, not a citation.
fn citations(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                j += 1;
            }
            let letters = j - start;
            let all_upper = text
                .get(start..j)
                .is_some_and(|s| s.bytes().all(|b| b.is_ascii_uppercase()));
            let all_lower = text
                .get(start..j)
                .is_some_and(|s| s.bytes().all(|b| b.is_ascii_lowercase()));
            let digits_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let digits = j - digits_start;
            let closes = j < bytes.len() && bytes[j] == b']';
            let is_link = closes && bytes.get(j + 1) == Some(&b'(');
            let id_shape = (all_upper && digits > 0) || (all_lower && digits == 0);
            if letters > 0 && closes && !is_link && id_shape {
                if let Some(id) = text.get(start..j) {
                    out.push(id);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn check_citations(section: Section, doc: &Doc, citable: &Citable, out: &mut Vec<Finding>) {
    let units = doc.sentences_units();
    let mut unknown = BTreeSet::new();
    for unit in &units {
        for id in citations(unit) {
            if !citable.contains(id) {
                unknown.insert(id.to_owned());
            }
        }
    }
    for id in unknown {
        out.push(Finding::error(format!(
            "`[{id}]` is not a confirmed record"
        )));
    }
    let needs_citation = match section {
        Section::Findings | Section::CurrentSystem => Some(Severity::Error),
        Section::Background => Some(Severity::Warning),
        _ => None,
    };
    if let Some(severity) = needs_citation {
        for unit in units {
            if citations(unit).is_empty() {
                let message = format!("no citation in: \"{}\"", short(unit));
                out.push(match severity {
                    Severity::Error => Finding::error(message),
                    Severity::Warning => Finding::warning(message),
                });
            }
        }
    }
}

fn check_words(section: Section, text: &str, out: &mut Vec<Finding>) {
    match section.phase() {
        Phase::Framing => {
            for word in FRAMING_FORBIDDEN {
                if text.contains(word) {
                    out.push(Finding::error(format!("framing must not mention `{word}`")));
                }
            }
        }
        Phase::Problem => {
            let lower = text.to_lowercase();
            for phrase in PROBLEM_FORBIDDEN {
                if lower.contains(phrase) {
                    out.push(Finding::warning(format!(
                        "the problem phase describes what exists; `{phrase}` sounds like a solution"
                    )));
                }
            }
        }
        Phase::Synthesis | Phase::Solution => {}
    }
}

fn short(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(60).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(findings: &[Finding]) -> Vec<&str> {
        findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .map(|f| f.message.as_str())
            .collect()
    }

    #[test]
    fn a_cited_finding_with_numbers_raises_nothing() {
        let findings = check_section(
            Section::Findings,
            "- The parser handles 12 formats in 40 ms [E1].",
            &Citable::none().with("E1"),
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn goal_is_one_paragraph_of_prose() {
        let ok = check_section(
            Section::Goal,
            "Enable crest users to run a discovery, providing a spec they can trust.",
            &Citable::none(),
        );
        assert!(ok.is_empty(), "{ok:?}");

        let bad = check_section(
            Section::Goal,
            "Enable users to do X.\n\n* and also\n* this list\n\nSecond paragraph.",
            &Citable::none(),
        );
        let e = errors(&bad);
        assert!(e.iter().any(|m| m.contains("no lists")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("2 paragraph")), "{e:?}");
    }

    #[test]
    fn framing_must_not_leak_the_method() {
        let bad = check_section(
            Section::Goal,
            "Enable the team to build a CLD of the cache.",
            &Citable::none(),
        );
        assert!(errors(&bad).iter().any(|m| m.contains("`CLD`")));
    }

    #[test]
    fn constraints_are_bullets_only() {
        assert!(check_section(Section::Constraints, "* a\n* b\n* c", &Citable::none()).is_empty());
        let bad = check_section(Section::Constraints, "Some prose.\n\n* a", &Citable::none());
        let e = errors(&bad);
        assert!(e.iter().any(|m| m.contains("nothing else")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("at least 2")), "{e:?}");
    }

    #[test]
    fn scope_has_in_and_out() {
        let ok = "### In\n\n* the session cache\n* its eviction\n\n### Out\n\n* the UI";
        assert!(check_section(Section::Scope, ok, &Citable::none()).is_empty());
        let bad = "### In\n\n* a\n\nSome prose.";
        let f = check_section(Section::Scope, bad, &Citable::none());
        let e = errors(&f);
        assert!(e.iter().any(|m| m.contains("nothing else")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("`### Out` has 0")), "{e:?}");
    }

    #[test]
    fn findings_need_categories_and_citations_that_resolve() {
        let citable = Citable::none().with("E1").with("S2");
        let ok = "#### Cache\n\n* Entries are never evicted [E1].\n* The docs say they should be [S2].\n\n#### History\n\n* The eviction was removed in 2024 [E1].";
        let f = check_section(Section::Findings, ok, &citable);
        assert!(f.is_empty(), "{f:?}");

        let bad = "#### Cache\n\n* Entries are never evicted.\n* Unknown ref [E9].\n\nA paragraph.";
        let f = check_section(Section::Findings, bad, &citable);
        let e = errors(&f);
        assert!(e.iter().any(|m| m.contains("no citation in")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("`[E9]` is not")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("each followed by")), "{e:?}");
    }

    #[test]
    fn bare_bullets_count_as_one_category() {
        let citable = Citable::none().with("E1");
        let f = check_section(Section::Findings, "* a fact [E1]\n* another [E1]", &citable);
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn the_problem_phase_flags_solution_language_as_a_warning() {
        let citable = Citable::none().with("E1");
        let f = check_section(
            Section::Findings,
            "* We propose a new cache [E1].",
            &citable,
        );
        assert_eq!(counts(&f), (0, 1), "{f:?}");
    }

    #[test]
    fn current_system_is_one_paragraph_and_one_mermaid() {
        let citable = Citable::none().with("E1");
        let ok =
            "The cache fills and never drains [E1].\n\n```mermaid\nflowchart LR\n  a --> b\n```";
        assert!(check_section(Section::CurrentSystem, ok, &citable).is_empty());
        let bad = "The cache fills [E1].\n\nAnd drains [E1].\n\n```rust\nfn x() {}\n```";
        let f = check_section(Section::CurrentSystem, bad, &citable);
        let e = errors(&f);
        assert!(e.iter().any(|m| m.contains("2 paragraph")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("0 mermaid")), "{e:?}");
    }

    #[test]
    fn desired_state_and_breakdown_are_numbered_lists() {
        let ok = "We want the cache to:\n\n1. **Bounded**: never exceed 10k entries.\n2. **Fresh**: evict after an hour.";
        assert!(check_section(Section::DesiredState, ok, &Citable::none()).is_empty());
        let bad = "1. only one\n\n* and a bullet";
        let f = check_section(Section::Breakdown, bad, &Citable::none());
        let e = errors(&f);
        assert!(e.iter().any(|m| m.contains("at least 2")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("numbered list")), "{e:?}");
    }

    #[test]
    fn background_without_citations_only_warns() {
        let f = check_section(
            Section::Background,
            "The cache was added in 2023.\n\nIt grew.",
            &Citable::none(),
        );
        assert_eq!(counts(&f), (0, 2), "{f:?}");
    }

    #[test]
    fn the_proposal_needs_its_three_parts() {
        let citable = Citable::none().with("SP1");
        let ok = "### Proposed approach\n\nBound the cache [SP1].\n\n```mermaid\nflowchart LR\n a --> b\n```\n\n### 1. Bound it\n\nAn LRU of 10k [SP1].\n\n### Future work\n\nSharding.";
        let f = check_section(Section::SolutionProposal, ok, &citable);
        assert!(f.is_empty(), "{f:?}");
        let f = check_section(Section::SolutionProposal, "Just do it.", &citable);
        let e = errors(&f);
        assert_eq!(e.len(), 4, "{e:?}");
    }

    #[test]
    fn citations_are_scanned_precisely() {
        assert_eq!(
            citations("see [E1], [S12] and [CL3]; not [x1] or [E] or [12]"),
            vec!["E1", "S12", "CL3"]
        );
    }

    #[test]
    fn continuation_lines_join_their_item() {
        let doc = Doc::parse("* first line\n  continues here\n* second");
        assert_eq!(
            doc.blocks,
            vec![Block::Bullets(vec![
                "first line continues here".into(),
                "second".into()
            ])]
        );
    }
}
