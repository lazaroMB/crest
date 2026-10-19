//! The web sources an agent used, read back from its `web_search` and
//! `web_fetch` calls.
//!
//! The web tools return text in a fixed shape (see
//! [`tools::web`](crate::tools::web)); this module pairs each successful
//! call with its result and turns them into [`Citation`]s: the pages the
//! agent **read** (with an excerpt of their text) and the search results it
//! **found** (with their snippet). The chat shows the read pages as a
//! `Sources` block; the research flow records both as evidence.

use std::collections::BTreeMap;

use domain::block::SourceRef;
use domain::conversation::{ContentPart, Message, ToolResultContent};

use crate::ports::llm::Citation;
use crate::tools::web::{SOURCE_PREFIX, TITLE_PREFIX, URL_PREFIX, WEB_FETCH_TOOL, WEB_SEARCH_TOOL};

/// Characters of a page kept as the excerpt of its citation.
const EXCERPT_CHARS: usize = 400;

/// The web sources of some messages, each list without repeated URLs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebSources {
    /// Pages read with `web_fetch`, in order.
    pub read: Vec<Citation>,
    /// Results of `web_search` calls, in order (pages also read included).
    pub found: Vec<Citation>,
}

impl WebSources {
    /// Whether no web tool returned anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.found.is_empty()
    }

    /// The read pages as timeline sources.
    #[must_use]
    pub fn read_refs(&self) -> Vec<SourceRef> {
        self.read
            .iter()
            .map(|c| SourceRef::Url {
                url: c.url.clone(),
                title: c.title.clone(),
            })
            .collect()
    }

    /// Read pages first, then the other results, without repeated URLs.
    #[must_use]
    pub fn all(&self) -> Vec<Citation> {
        let mut out = self.read.clone();
        for citation in &self.found {
            if !out.iter().any(|c| c.url == citation.url) {
                out.push(citation.clone());
            }
        }
        out
    }
}

/// The sources of the web calls in `messages`.
pub fn web_sources<'a>(messages: impl IntoIterator<Item = &'a Message>) -> WebSources {
    let mut calls: BTreeMap<String, &str> = BTreeMap::new();
    let mut sources = WebSources::default();
    for message in messages {
        for part in &message.content {
            match part {
                ContentPart::ToolUse { id, name, .. } => {
                    if name == WEB_SEARCH_TOOL || name == WEB_FETCH_TOOL {
                        calls.insert(id.to_string(), name.as_str());
                    }
                }
                ContentPart::ToolResult {
                    tool_use_id,
                    content,
                    is_error: false,
                } => {
                    let text = text_of(content);
                    match calls.get(&tool_use_id.to_string()).copied() {
                        Some(WEB_FETCH_TOOL) => push(&mut sources.read, parse_page(&text)),
                        Some(WEB_SEARCH_TOOL) => {
                            for hit in parse_hits(&text) {
                                push(&mut sources.found, Some(hit));
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    sources
}

fn push(list: &mut Vec<Citation>, citation: Option<Citation>) {
    if let Some(citation) = citation
        && !list.iter().any(|c| c.url == citation.url)
    {
        list.push(citation);
    }
}

fn text_of(content: &[ToolResultContent]) -> String {
    content
        .iter()
        .filter_map(|item| match item {
            ToolResultContent::Text { text } => Some(text.as_str()),
            ToolResultContent::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `web_fetch` result: `Source:` and `Title:` lines, a blank line, text.
fn parse_page(text: &str) -> Option<Citation> {
    let mut lines = text.lines();
    let url = lines.next()?.strip_prefix(SOURCE_PREFIX)?.trim().to_owned();
    let mut title = None;
    let mut body = Vec::new();
    for line in lines.by_ref() {
        if let Some(t) = line.strip_prefix(TITLE_PREFIX) {
            title = Some(t.trim().to_owned());
        } else if line.trim().is_empty() {
            break;
        } else {
            body.push(line);
        }
    }
    body.extend(lines);
    let excerpt: String = body
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(EXCERPT_CHARS)
        .collect();
    Some(Citation {
        url,
        title,
        cited_text: excerpt,
    })
}

/// The results of a `web_search` result: `[n] title`, `URL: …`, snippet.
fn parse_hits(text: &str) -> Vec<Citation> {
    let mut hits: Vec<Citation> = Vec::new();
    let mut title: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('[')
            && let Some((n, t)) = rest.split_once("] ")
            && n.chars().all(|c| c.is_ascii_digit())
        {
            title = Some(t.trim().to_owned());
        } else if let Some(url) = line.strip_prefix(URL_PREFIX) {
            hits.push(Citation {
                url: url.trim().to_owned(),
                title: title.take(),
                cited_text: String::new(),
            });
        } else if let Some(last) = hits.last_mut()
            && !line.trim().is_empty()
        {
            if !last.cited_text.is_empty() {
                last.cited_text.push(' ');
            }
            last.cited_text.push_str(line.trim());
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use domain::conversation::{Role, ToolUseId};
    use serde_json::json;

    use super::*;

    fn call(id: &str, name: &str) -> ContentPart {
        ContentPart::ToolUse {
            id: ToolUseId::new(id),
            name: name.into(),
            input: json!({}),
        }
    }

    fn result(id: &str, text: &str, is_error: bool) -> ContentPart {
        ContentPart::ToolResult {
            tool_use_id: ToolUseId::new(id),
            content: vec![ToolResultContent::text(text)],
            is_error,
        }
    }

    #[test]
    fn reads_pages_and_results_back() {
        let messages = [
            Message::new(
                Role::Assistant,
                vec![call("s1", WEB_SEARCH_TOOL), call("f1", WEB_FETCH_TOOL)],
            ),
            Message::new(
                Role::User,
                vec![
                    result(
                        "s1",
                        "[1] Rust 1.99\nURL: https://blog.rust-lang.org/1.99\nThe release\nnotes.\n\n\
                         [2] Reddit\nURL: https://reddit.com/r/rust\n",
                        false,
                    ),
                    result(
                        "f1",
                        "Source: https://blog.rust-lang.org/1.99\nTitle: Announcing 1.99\n\n\
                         Rust 1.99   is out.\nIt adds things.",
                        false,
                    ),
                ],
            ),
        ];
        let sources = web_sources(&messages);
        assert_eq!(
            sources.read,
            [Citation {
                url: "https://blog.rust-lang.org/1.99".into(),
                title: Some("Announcing 1.99".into()),
                cited_text: "Rust 1.99 is out. It adds things.".into(),
            }]
        );
        assert_eq!(sources.found.len(), 2);
        assert_eq!(sources.found[0].cited_text, "The release notes.");
        assert_eq!(sources.found[1].title.as_deref(), Some("Reddit"));
        let all: Vec<_> = sources.all().into_iter().map(|c| c.url).collect();
        assert_eq!(
            all,
            [
                "https://blog.rust-lang.org/1.99",
                "https://reddit.com/r/rust"
            ]
        );
    }

    #[test]
    fn errors_and_other_tools_are_ignored() {
        let messages = [
            Message::new(
                Role::Assistant,
                vec![call("f1", WEB_FETCH_TOOL), call("g1", "grep")],
            ),
            Message::new(
                Role::User,
                vec![
                    result("f1", "cannot read https://x: HTTP 404", true),
                    result("g1", "Source: https://not-a-page\n", false),
                ],
            ),
        ];
        assert!(web_sources(&messages).is_empty());
    }
}
