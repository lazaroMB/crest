//! What the simulator reads from a Spec's `spec.md`: its title, its
//! Mermaid diagrams by kind, its test protocol and the ids it uses.

use crate::flows::research::nodes::spec::render::{SECTIONS, SECTIONS_ES};

/// The parts of a Spec the model is generated from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpecDoc {
    /// Title after `# Spec NNNN:`.
    pub title: String,
    /// `flowchart` / `graph` diagrams (the black box).
    pub black_box: Vec<String>,
    /// `sequenceDiagram` diagrams.
    pub sequences: Vec<String>,
    /// `stateDiagram` diagrams.
    pub states: Vec<String>,
    /// Text of the test protocol section.
    pub test_protocol: String,
    /// The proposal, functional and technical sections, for context.
    pub design: String,
    /// Test-protocol item ids (`T1`...) found in the protocol, in order.
    pub test_items: Vec<String>,
    /// Hypothesis ids (`H1`...) cited anywhere, in order.
    pub hypotheses: Vec<String>,
}

fn heading_is(heading: &str, index: usize) -> bool {
    heading == SECTIONS[index] || heading == SECTIONS_ES[index]
}

/// `## ` sections of `markdown`, outside code fences.
fn sections(markdown: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut fence: Option<String> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if let Some(open) = &fence {
            if trimmed.starts_with(open.as_str())
                && trimmed.trim_start_matches('`').trim().is_empty()
            {
                fence = None;
            }
        } else if trimmed.starts_with("```") {
            let ticks = trimmed.chars().take_while(|c| *c == '`').count();
            fence = Some("`".repeat(ticks));
        } else if let Some(h) = line.strip_prefix("## ") {
            out.push((h.trim().to_owned(), String::new()));
            continue;
        }
        if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

/// Bodies of the ```` ```mermaid ```` blocks of `markdown`.
fn mermaid_blocks(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if let Some((fence, body)) = &mut current {
            if trimmed.starts_with(fence.as_str())
                && trimmed.trim_start_matches('`').trim().is_empty()
            {
                out.push(body.trim().to_owned());
                current = None;
            } else {
                body.push_str(line);
                body.push('\n');
            }
        } else if trimmed.starts_with("```") {
            let ticks = trimmed.chars().take_while(|c| *c == '`').count();
            let lang = trimmed[ticks..].trim();
            if lang == "mermaid" {
                current = Some(("`".repeat(ticks), String::new()));
            }
        }
    }
    out
}

/// Ids made of `prefix` and digits in `text` (`T1`, `T12`), without
/// repeats, in order.
fn ids(text: &str, prefix: char) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        let boundary = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
        if !boundary || chars[i] != prefix {
            continue;
        }
        let digits: String = chars[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let after = chars.get(i + 1 + digits.len());
        if digits.is_empty()
            || digits.starts_with('0')
            || after.is_some_and(|c| c.is_ascii_alphanumeric())
        {
            continue;
        }
        let id = format!("{prefix}{digits}");
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

impl SpecDoc {
    /// Reads `spec.md`.
    #[must_use]
    pub fn parse(markdown: &str) -> Self {
        let title = markdown
            .lines()
            .find_map(|l| l.strip_prefix("# "))
            .map(|t| {
                t.split_once(':')
                    .filter(|(head, _)| head.trim_start().starts_with("Spec"))
                    .map_or(t, |(_, rest)| rest)
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default();
        let mut doc = Self {
            title,
            ..Self::default()
        };
        for block in mermaid_blocks(markdown) {
            let first = block.trim_start();
            if first.starts_with("sequenceDiagram") {
                doc.sequences.push(block);
            } else if first.starts_with("stateDiagram") {
                doc.states.push(block);
            } else if first.starts_with("flowchart") || first.starts_with("graph") {
                doc.black_box.push(block);
            }
        }
        for (heading, body) in sections(markdown) {
            if heading_is(&heading, 7) || heading.to_lowercase().contains("test protocol") {
                doc.test_protocol.push_str(&body);
            } else if heading_is(&heading, 3) || heading_is(&heading, 4) || heading_is(&heading, 5)
            {
                doc.design.push_str(&format!("## {heading}\n\n{body}"));
            }
        }
        doc.test_items = ids(&doc.test_protocol, 'T');
        doc.hypotheses = ids(markdown, 'H');
        doc
    }

    /// Whether there is something to build a model from.
    #[must_use]
    pub fn has_diagrams(&self) -> bool {
        !(self.black_box.is_empty() && self.sequences.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "# Spec 0003: Session cache\n\n## 1. Problem\n\nSlow [E1].\n\n\
## 4. Proposed solution\n\nA cache.\n\n```mermaid\nflowchart LR\n  c[client] --> a[api]\n```\n\n\
## 5. Functional specification\n\nFlows.\n\n### Sequence\n\n```mermaid\nsequenceDiagram\n  c->>a: Req\n```\n\n\
### States\n\n```mermaid\nstateDiagram-v2\n  [*] --> idle\n```\n\n\
## 8. Test protocol\n\n- T1: one request answers within 50 ms (H1).\n- T2: a burst is not rejected.\n- see T1 again, not ET3 nor T03\n\n\
## 10. Sources\n\n- [E1]\n";

    #[test]
    fn reads_title_diagrams_and_protocol() {
        let doc = SpecDoc::parse(SPEC);
        assert_eq!(doc.title, "Session cache");
        assert_eq!(doc.black_box.len(), 1);
        assert!(doc.black_box[0].starts_with("flowchart LR"));
        assert_eq!(doc.sequences.len(), 1);
        assert_eq!(doc.states.len(), 1);
        assert!(doc.test_protocol.contains("burst"));
        assert_eq!(doc.test_items, ["T1", "T2"]);
        assert_eq!(doc.hypotheses, ["H1"]);
        assert!(doc.design.contains("## 4. Proposed solution"));
        assert!(doc.design.contains("## 5. Functional specification"));
        assert!(!doc.design.contains("Test protocol"));
        assert!(doc.has_diagrams());
    }

    #[test]
    fn a_fenced_heading_is_not_a_section() {
        let doc = SpecDoc::parse(
            "# Spec 0001: X\n\n## 8. Test protocol\n\n```\n## 9. Not a heading\n```\n- T4\n",
        );
        assert_eq!(doc.test_items, ["T4"]);
        assert!(!doc.has_diagrams());
    }
}
