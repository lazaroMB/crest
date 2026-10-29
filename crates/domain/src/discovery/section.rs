//! Sections of a discovery, their phases, the two levels and what each
//! section must look like.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// How deep a discovery goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// `/research`: six sections, the repository and a few web pages.
    Research,
    /// `/research --deep`: ten sections, the literature, claims, the causal
    /// diagram, risks and spikes.
    Deep,
}

impl Level {
    /// The sections of this level, in writing order.
    #[must_use]
    pub fn sections(self) -> &'static [Section] {
        match self {
            Self::Research => &Section::RESEARCH,
            Self::Deep => &Section::ALL,
        }
    }
}

/// The phase a section belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// What we are looking at and within which limits.
    Framing,
    /// What exists today and why it behaves as it does.
    Problem,
    /// What we want instead and what is missing.
    Synthesis,
    /// What we will do.
    Solution,
}

/// A section of the discovery document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    /// `Enable <who> to <capability>, providing <outcome>.`
    Goal,
    /// What the system does today, what triggers this, the terms used.
    Background,
    /// Research, product, licensing and environment limits.
    Constraints,
    /// `### In` and `### Out`.
    Scope,
    /// Confirmed facts, grouped by category, every one cited.
    Findings,
    /// One paragraph and one diagram of the system as it is.
    CurrentSystem,
    /// Numbered outcomes we want.
    DesiredState,
    /// One paragraph: what is missing between 6 and 7.
    Gap,
    /// Numbered, measurable problem statements.
    Breakdown,
    /// The approach, one subsection per breakdown item, future work.
    SolutionProposal,
}

impl Section {
    /// Every section, in writing order (the deep level).
    pub const ALL: [Self; 10] = [
        Self::Goal,
        Self::Background,
        Self::Constraints,
        Self::Scope,
        Self::Findings,
        Self::CurrentSystem,
        Self::DesiredState,
        Self::Gap,
        Self::Breakdown,
        Self::SolutionProposal,
    ];

    /// The sections of the research level, in writing order.
    pub const RESEARCH: [Self; 7] = [
        Self::Goal,
        Self::Background,
        Self::Constraints,
        Self::Scope,
        Self::Findings,
        Self::Breakdown,
        Self::SolutionProposal,
    ];

    /// The heading as written in the document, without `## `.
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::Goal => "Goal",
            Self::Background => "Background",
            Self::Constraints => "Constraints",
            Self::Scope => "Scope",
            Self::Findings => "Findings",
            Self::CurrentSystem => "Current system",
            Self::DesiredState => "Desired state",
            Self::Gap => "Gap",
            Self::Breakdown => "Breakdown",
            Self::SolutionProposal => "Solution proposal",
        }
    }

    /// The phase of the section.
    #[must_use]
    pub const fn phase(self) -> Phase {
        match self {
            Self::Goal | Self::Background | Self::Constraints | Self::Scope => Phase::Framing,
            Self::Findings | Self::CurrentSystem => Phase::Problem,
            Self::DesiredState | Self::Gap | Self::Breakdown => Phase::Synthesis,
            Self::SolutionProposal => Phase::Solution,
        }
    }

    /// Whether the section is written at `level`.
    #[must_use]
    pub fn in_level(self, level: Level) -> bool {
        level.sections().contains(&self)
    }

    /// 0-based position in [`Section::ALL`]; sections compare by it.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The format the section's text must follow.
    #[must_use]
    pub const fn format(self) -> Format {
        match self {
            Self::Goal => Format::Prose {
                min_paragraphs: 1,
                max_paragraphs: 1,
            },
            Self::Background => Format::Prose {
                min_paragraphs: 2,
                max_paragraphs: 6,
            },
            Self::Gap => Format::Prose {
                min_paragraphs: 1,
                max_paragraphs: 2,
            },
            Self::Constraints => Format::Bullets { min: 2 },
            Self::Scope => Format::InOut { min_each: 1 },
            Self::Findings => Format::Categories { min: 1, max: 5 },
            Self::CurrentSystem => Format::ParagraphAndMermaid,
            Self::DesiredState | Self::Breakdown => Format::Numbered { min: 2 },
            Self::SolutionProposal => Format::Proposal,
        }
    }

    /// Who may approve the section at `level`.
    #[must_use]
    pub const fn verdict_policy(self, level: Level) -> VerdictPolicy {
        match (level, self) {
            (Level::Research, Self::Constraints | Self::Findings) => VerdictPolicy::AuditOnly,
            (Level::Research, _) => VerdictPolicy::Human,
            (Level::Deep, Self::Goal | Self::Background | Self::Constraints | Self::Scope) => {
                VerdictPolicy::Human
            }
            (Level::Deep, _) => VerdictPolicy::Evaluator,
        }
    }

    /// The section after `self` at `level`, if any.
    #[must_use]
    pub fn next_in(self, level: Level) -> Option<Self> {
        let sections = level.sections();
        let at = sections.iter().position(|s| *s == self)?;
        sections.get(at + 1).copied()
    }
}

impl fmt::Display for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.heading())
    }
}

/// Error of [`Section::from_str`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown section `{0}`")]
pub struct UnknownSection(pub String);

impl FromStr for Section {
    type Err = UnknownSection;

    /// Parses a heading, case-insensitively, with a few aliases
    /// (`current state`, `problem space`, `desired`, `proposal`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key: String = s
            .trim()
            .trim_start_matches('#')
            .trim()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let section = match key.as_str() {
            "goal" => Self::Goal,
            "background" => Self::Background,
            "constraints" => Self::Constraints,
            "scope" => Self::Scope,
            "findings" | "problem space" => Self::Findings,
            "current system" | "current state" => Self::CurrentSystem,
            "desired state" | "desired" => Self::DesiredState,
            "gap" => Self::Gap,
            "breakdown" => Self::Breakdown,
            "solution proposal" | "solution" | "proposal" => Self::SolutionProposal,
            _ => return Err(UnknownSection(s.to_owned())),
        };
        Ok(section)
    }
}

/// What a section's text must look like. Checked by
/// [`check_section`](super::contract::check_section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Paragraphs only: no lists, no headings, no code.
    Prose {
        /// Fewest paragraphs.
        min_paragraphs: usize,
        /// Most paragraphs.
        max_paragraphs: usize,
    },
    /// A bullet list and nothing else.
    Bullets {
        /// Fewest bullets.
        min: usize,
    },
    /// `### In` and `### Out`, each a bullet list.
    InOut {
        /// Fewest bullets under each.
        min_each: usize,
    },
    /// `#### <Category>` headings, each followed by bullets; a bare bullet
    /// list counts as one category.
    Categories {
        /// Fewest categories.
        min: usize,
        /// Most categories.
        max: usize,
    },
    /// Exactly one paragraph and one `mermaid` fenced block.
    ParagraphAndMermaid,
    /// A numbered list and nothing else, except an optional lead-in line
    /// ending in `:`.
    Numbered {
        /// Fewest items.
        min: usize,
    },
    /// `### Proposed approach` (paragraph plus `mermaid`), `### <n>. <Title>`
    /// subsections and `### Future work`.
    Proposal,
}

/// Who may give a section its approving verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictPolicy {
    /// A person, always.
    Human,
    /// A clean audit is enough; the engine records the approval itself.
    AuditOnly,
    /// An isolated evaluator model may approve after a clean, isolated
    /// audit; a person always may.
    Evaluator,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_level_is_a_subsequence_of_deep() {
        let deep = Level::Deep.sections();
        let mut at = 0;
        for s in Level::Research.sections() {
            let pos = deep.iter().position(|d| d == s).expect("in deep");
            assert!(pos >= at, "{s} out of order");
            at = pos;
        }
    }

    #[test]
    fn next_in_skips_sections_the_level_lacks() {
        assert_eq!(
            Section::Findings.next_in(Level::Research),
            Some(Section::Breakdown)
        );
        assert_eq!(
            Section::Findings.next_in(Level::Deep),
            Some(Section::CurrentSystem)
        );
        assert_eq!(Section::SolutionProposal.next_in(Level::Deep), None);
        assert_eq!(Section::CurrentSystem.next_in(Level::Research), None);
    }

    #[test]
    fn headings_parse_back_with_aliases() {
        for s in Section::ALL {
            assert_eq!(s.heading().parse::<Section>(), Ok(s));
            assert_eq!(format!("## {}", s.heading()).parse::<Section>(), Ok(s));
        }
        assert_eq!(
            "Current State".parse::<Section>(),
            Ok(Section::CurrentSystem)
        );
        assert_eq!("proposal".parse::<Section>(), Ok(Section::SolutionProposal));
        assert!("Guardrails".parse::<Section>().is_err());
    }

    #[test]
    fn framing_is_always_human_except_research_constraints() {
        for s in [Section::Goal, Section::Background, Section::Scope] {
            assert_eq!(s.verdict_policy(Level::Research), VerdictPolicy::Human);
            assert_eq!(s.verdict_policy(Level::Deep), VerdictPolicy::Human);
        }
        assert_eq!(
            Section::Constraints.verdict_policy(Level::Research),
            VerdictPolicy::AuditOnly
        );
        assert_eq!(
            Section::Findings.verdict_policy(Level::Deep),
            VerdictPolicy::Evaluator
        );
        assert_eq!(
            Section::SolutionProposal.verdict_policy(Level::Research),
            VerdictPolicy::Human
        );
    }
}
