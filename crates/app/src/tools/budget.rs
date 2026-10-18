//! Truncation of tool results to a token budget, so one search or file
//! read cannot flood the model's context.
//!
//! Tokens are estimated at [`BYTES_PER_TOKEN`] bytes each, the usual rule
//! of thumb for English and code with Claude's tokenizer; it is a rough
//! guard, not an exact count. The explanatory note added after a cut is
//! not counted against the budget.

use domain::conversation::ToolResultContent;

use super::registry::ToolOutput;

/// Estimated bytes per token.
pub const BYTES_PER_TOKEN: usize = 4;

/// Default budget of one tool result, in tokens (about 32 KB of text).
pub const DEFAULT_RESULT_TOKENS: usize = 8_000;

/// Estimated number of tokens of `text`.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(BYTES_PER_TOKEN)
}

/// The line that ends a list cut by the budget.
#[must_use]
pub fn more_results_line(omitted: usize) -> String {
    format!("{omitted} more results; refine the search")
}

/// Token budget of a tool result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultBudget {
    /// Maximum tokens of text in one result.
    pub max_tokens: usize,
}

impl Default for ResultBudget {
    fn default() -> Self {
        Self::new(DEFAULT_RESULT_TOKENS)
    }
}

impl ResultBudget {
    /// A budget of `max_tokens` tokens.
    #[must_use]
    pub const fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }

    /// The budget in bytes.
    #[must_use]
    pub const fn max_bytes(self) -> usize {
        self.max_tokens.saturating_mul(BYTES_PER_TOKEN)
    }

    /// Joins `items` with newlines, keeping only whole items that fit,
    /// and ends with "N more results; refine the search" when some
    /// were left out.
    #[must_use]
    pub fn truncate_list<S: AsRef<str>>(self, items: &[S]) -> String {
        let max = self.max_bytes();
        let mut out = String::new();
        let mut kept = 0;
        for item in items {
            let item = item.as_ref();
            let sep = usize::from(kept > 0);
            if out.len() + sep + item.len() > max {
                break;
            }
            if sep == 1 {
                out.push('\n');
            }
            out.push_str(item);
            kept += 1;
        }
        let omitted = items.len() - kept;
        if omitted > 0 {
            if kept > 0 {
                out.push('\n');
            }
            out.push_str(&more_results_line(omitted));
        }
        out
    }

    /// Cuts free text to the budget at a line boundary (or inside the
    /// first line if even that does not fit) and adds a note with the
    /// number of lines left out.
    #[must_use]
    pub fn truncate_text(self, text: &str) -> String {
        truncate_text_to(text, self.max_bytes())
    }

    /// Applies the budget to the text items of `output`, shared between
    /// them in order; items left without budget are dropped and counted
    /// in a final note. Images are kept and not counted.
    #[must_use]
    pub fn apply(self, mut output: ToolOutput) -> ToolOutput {
        let mut remaining = self.max_bytes();
        let mut dropped = 0;
        let mut content = Vec::with_capacity(output.content.len());
        for item in output.content {
            match item {
                ToolResultContent::Text { text } if text.len() <= remaining => {
                    remaining -= text.len();
                    content.push(ToolResultContent::Text { text });
                }
                ToolResultContent::Text { .. } if remaining == 0 => dropped += 1,
                ToolResultContent::Text { text } => {
                    let text = truncate_text_to(&text, remaining);
                    remaining = 0;
                    content.push(ToolResultContent::Text { text });
                }
                image @ ToolResultContent::Image { .. } => content.push(image),
            }
        }
        if dropped > 0 {
            content.push(ToolResultContent::text(more_results_line(dropped)));
        }
        output.content = content;
        output
    }
}

fn truncate_text_to(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut cut = floor_char_boundary(text, max);
    if let Some(newline) = text[..cut].rfind('\n') {
        cut = newline;
    }
    let (kept, rest) = text.split_at(cut);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let omitted = rest.lines().count();
    let mut out = kept.to_owned();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!(
        "[{omitted} more lines omitted by the context budget; \
         refine the search or ask for a smaller range]"
    ));
    out
}

/// The largest char boundary of `text` not after `index`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A budget of exactly `bytes` bytes.
    fn bytes(bytes: usize) -> ResultBudget {
        assert_eq!(bytes % BYTES_PER_TOKEN, 0);
        ResultBudget::new(bytes / BYTES_PER_TOKEN)
    }

    #[test]
    fn estimates_four_bytes_per_token() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(ResultBudget::new(10).max_bytes(), 40);
    }

    #[test]
    fn list_that_fits_is_unchanged() {
        assert_eq!(bytes(16).truncate_list(&["a.rs", "b.rs"]), "a.rs\nb.rs");
    }

    #[test]
    fn list_is_cut_by_whole_items_with_the_more_results_line() {
        let items = ["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"];
        // "src/a.rs\nsrc/b.rs" is 17 bytes; a third item would need 26.
        assert_eq!(
            bytes(20).truncate_list(&items),
            "src/a.rs\nsrc/b.rs\n2 more results; refine the search"
        );
    }

    #[test]
    fn list_with_no_room_keeps_only_the_line() {
        assert_eq!(
            bytes(4).truncate_list(&["demasiado largo"]),
            "1 more results; refine the search"
        );
    }

    #[test]
    fn text_is_cut_at_a_line_boundary() {
        let text = "uno\ndos\ntres\ncuatro";
        assert_eq!(
            bytes(12).truncate_text(text),
            "uno\ndos\n[2 more lines omitted by the context budget; \
             refine the search or ask for a smaller range]"
        );
        assert_eq!(bytes(20).truncate_text(text), text);
    }

    #[test]
    fn a_long_first_line_is_cut_on_a_char_boundary() {
        let text = "ñññññññññ";
        let out = bytes(4).truncate_text(text);
        assert!(out.starts_with("ññ\n[1 more lines"), "{out}");
    }

    #[test]
    fn apply_shares_the_budget_between_text_items() {
        let output = ToolOutput {
            content: vec![
                ToolResultContent::text("abcd"),
                ToolResultContent::Image {
                    media_type: "image/png".into(),
                    data: "x".repeat(100),
                },
                ToolResultContent::text("efgh\nijkl"),
                ToolResultContent::text("mnop"),
            ],
            is_error: false,
            block_update: None,
        };
        let out = bytes(8).apply(output);
        assert_eq!(out.content.len(), 4);
        assert_eq!(out.content[0], ToolResultContent::text("abcd"));
        assert!(matches!(out.content[1], ToolResultContent::Image { .. }));
        assert!(out.text_content().contains("efgh\n[1 more lines"));
        assert_eq!(
            out.content[3],
            ToolResultContent::text("1 more results; refine the search")
        );
    }
}
