//! The packet: everything a writer, a reviewer or an evaluator may see
//! for one section. Built from the state and the stores; never a whole
//! source, only excerpts around the spans the records cite.

use std::fmt::Write as _;

use domain::discovery::{
    Citable, DiscoveryState, Level, RecordRef, RecordVerdict, Section, SourceId,
};

use crate::ports::discovery::StoredSource;

/// Characters of source text shown around a cited span.
pub const EXCERPT_RADIUS: usize = 300;
/// Characters of a source shown when nothing cites it.
pub const EXCERPT_HEAD: usize = 400;
/// Characters of a record shown in the packet.
pub const RECORD_LIMIT: usize = 4_000;

/// Something the section may cite, with what the writer needs to cite it
/// well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitableItem {
    /// The id as cited: `S3`, `E2`, `CL4`, `SP1`, `structure`, ...
    pub id: String,
    /// One line about it.
    pub summary: String,
    /// Text the writer may rely on (an excerpt, a span, a record).
    pub excerpt: Option<String>,
}

/// A piece of evidence or a claim as the packet shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Source it comes from.
    pub source: SourceId,
    /// The verbatim span.
    pub span: String,
}

/// What one section's writer may see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// The user's request.
    pub problem: String,
    /// The level.
    pub level: Level,
    /// The section to write.
    pub section: Section,
    /// The approved sections before it, in order, with their text.
    pub approved: Vec<(Section, String)>,
    /// What may be cited.
    pub citable: Vec<CitableItem>,
    /// A correction to apply: the note of a rejection, or the findings of
    /// an audit.
    pub correction: Option<String>,
}

impl Packet {
    /// The ids the section may cite.
    #[must_use]
    pub fn citable_ids(&self) -> Citable {
        Citable::none().with_all(self.citable.iter().map(|c| c.id.clone()))
    }

    /// The packet as the writer reads it.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Request\n\n{}\n", self.problem.trim());
        if !self.approved.is_empty() {
            out.push_str("# Approved sections\n\n");
            for (section, text) in &self.approved {
                let _ = writeln!(out, "## {section}\n\n{}\n", text.trim());
            }
        }
        if self.citable.is_empty() {
            out.push_str("# What you may cite\n\nNothing: this section cites no record.\n\n");
        } else {
            out.push_str("# What you may cite\n\nCite with the id in square brackets, e.g. `[S1]`. Cite nothing else.\n\n");
            for item in &self.citable {
                let _ = writeln!(out, "- [{}] {}", item.id, item.summary.trim());
                if let Some(excerpt) = &item.excerpt {
                    for line in excerpt.trim().lines() {
                        let _ = writeln!(out, "    {line}");
                    }
                }
            }
            out.push('\n');
        }
        if let Some(correction) = &self.correction {
            let _ = writeln!(
                out,
                "# Correction\n\nThe previous draft was not accepted. Fix exactly this:\n\n{}\n",
                correction.trim()
            );
        }
        let _ = writeln!(out, "# Section to write\n\n## {}", self.section);
        out
    }
}

/// Builds the citable items of `state` for `section` out of the stored
/// `sources`, the evidence `spans` (by evidence id) and the confirmed
/// `claims` (by claim id, with the span), plus the text of the method
/// `records` (by name).
#[must_use]
pub fn citable_items(
    state: &DiscoveryState,
    section: Section,
    sources: &[StoredSource],
    evidence: &[(u32, String, Span)],
    claims: &[(u32, String, Span)],
    records: &[(RecordRef, String)],
) -> Vec<CitableItem> {
    // Framing sections cite nothing; the rest cite what is confirmed.
    if section.phase() == domain::discovery::Phase::Framing {
        return Vec::new();
    }
    let confirmed = |record: RecordRef| {
        state
            .records
            .get(&record)
            .is_some_and(|r| r.verdict == RecordVerdict::Confirmed)
    };
    let mut items = Vec::new();
    let mut cited_spans: Vec<(SourceId, &str)> = Vec::new();
    for (id, claim, span) in evidence {
        if confirmed(RecordRef::Evidence { id: *id }) {
            cited_spans.push((span.source, &span.span));
            items.push(CitableItem {
                id: format!("E{id}"),
                summary: format!("evidence (from [{}]): {claim}", span.source),
                excerpt: Some(format!("\"{}\"", span.span.trim())),
            });
        }
    }
    for (id, claim, span) in claims {
        if confirmed(RecordRef::Claim { id: *id }) {
            cited_spans.push((span.source, &span.span));
            items.push(CitableItem {
                id: format!("CL{id}"),
                summary: format!("claim (from [{}]): {claim}", span.source),
                excerpt: Some(format!("\"{}\"", span.span.trim())),
            });
        }
    }
    for source in sources {
        if !state.sources.contains_key(&source.id) {
            continue;
        }
        let title = source.source.title.as_deref().unwrap_or("untitled");
        items.push(CitableItem {
            id: source.id.to_string(),
            summary: format!(
                "{} source, {title}: {}",
                source.source.kind, source.source.location
            ),
            excerpt: Some(excerpt_of(&source.source.text, source.id, &cited_spans)),
        });
    }
    for (record, text) in records {
        if confirmed(*record) {
            items.push(CitableItem {
                id: record.to_string(),
                summary: format!("record `{record}`"),
                excerpt: Some(truncate(text, RECORD_LIMIT)),
            });
        }
    }
    for (id, spike) in &state.spikes {
        if let Some((score, _)) = spike.score {
            items.push(CitableItem {
                id: id.to_string(),
                summary: format!("spike {id}: {score:?}"),
                excerpt: None,
            });
        }
    }
    items
}

/// The excerpt of `text` the packet shows for source `id`: windows around
/// every span that cites it, else its head.
#[must_use]
pub fn excerpt_of(text: &str, id: SourceId, spans: &[(SourceId, &str)]) -> String {
    let mut windows: Vec<String> = Vec::new();
    for (_, span) in spans.iter().filter(|(s, _)| *s == id) {
        let span = span.trim();
        if span.is_empty() {
            continue;
        }
        if let Some(at) = text.find(span) {
            let start = floor_char(text, at.saturating_sub(EXCERPT_RADIUS));
            let end = ceil_char(text, (at + span.len() + EXCERPT_RADIUS).min(text.len()));
            windows.push(format!("…{}…", text[start..end].trim()));
        }
    }
    if windows.is_empty() {
        truncate(text, EXCERPT_HEAD)
    } else {
        windows.join("\n")
    }
}

/// `text` cut to `limit` characters with an ellipsis.
#[must_use]
pub fn truncate(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit).collect();
    format!("{}…", cut.trim_end())
}

fn floor_char(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_char(text: &str, mut at: usize) -> usize {
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_windows_around_spans_or_shows_the_head() {
        let text = "a".repeat(1000) + " the span here " + &"b".repeat(1000);
        let excerpt = excerpt_of(&text, SourceId(1), &[(SourceId(1), "the span here")]);
        assert!(excerpt.contains("the span here"));
        assert!(excerpt.len() < 700, "{}", excerpt.len());
        assert!(excerpt.starts_with('…') && excerpt.ends_with('…'));
        let head = excerpt_of(&text, SourceId(1), &[(SourceId(2), "the span here")]);
        assert_eq!(head.chars().count(), EXCERPT_HEAD + 1);
    }

    #[test]
    fn render_lists_sections_citables_and_the_correction() {
        let packet = Packet {
            problem: "slow".into(),
            level: Level::Research,
            section: Section::Findings,
            approved: vec![(Section::Goal, "Enable x.".into())],
            citable: vec![CitableItem {
                id: "E1".into(),
                summary: "evidence".into(),
                excerpt: Some("\"span\"".into()),
            }],
            correction: Some("cite E1".into()),
        };
        let text = packet.render();
        assert!(text.contains("## Goal\n\nEnable x."));
        assert!(text.contains("- [E1] evidence\n    \"span\""));
        assert!(text.contains("# Correction"));
        assert!(text.ends_with("## Findings\n"));
        assert!(packet.citable_ids().contains("E1"));
    }
}
