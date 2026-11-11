//! The instructions files of subdirectories, attached to tool results.
//!
//! A nested `AGENTS.md` (or `CLAUDE.md`) applies to its subtree. Rather than
//! putting every one of them in the system prompt, the agent loop attaches
//! a file to the result of the first tool call that touches its subtree,
//! wrapped in an [`open_tag`] the history can be searched for. The history
//! is the record of what was attached: a file already in it is not
//! attached again, and once compaction replaces the turns that had it, the
//! next call in its subtree attaches it again.

use std::collections::BTreeSet;

use domain::conversation::{ContentPart, Message, ToolResultContent, Turn};
use serde_json::Value;

use crate::ports::instructions::InstructionFile;

/// The tools whose `path` input says which subtree they work in.
pub const PATH_TOOLS: &[&str] = &["read_file", "edit_file", "write_file", "tree", "grep"];

const OPEN: &str = "<instructions path=\"";
const CLOSE: &str = "</instructions>";

/// The tag that opens the attachment of the file at `path`.
#[must_use]
pub fn open_tag(path: &str) -> String {
    format!("{OPEN}{path}\">")
}

/// The path a call of `tool` with `input` works in, if it is one of the
/// [`PATH_TOOLS`] and names one.
#[must_use]
pub fn call_path<'a>(tool: &str, input: &'a Value) -> Option<&'a str> {
    if !PATH_TOOLS.contains(&tool) {
        return None;
    }
    input
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
}

/// The text attached to a tool result for `files`.
#[must_use]
pub fn attachment(files: &[InstructionFile]) -> String {
    let mut out = String::from(
        "The directories of this path have their own instructions; follow them for files \
         under them (they add to and take precedence over the project's):",
    );
    for file in files {
        out.push_str("\n\n");
        out.push_str(&open_tag(&file.path));
        out.push('\n');
        out.push_str(file.text.trim());
        out.push('\n');
        out.push_str(CLOSE);
    }
    out
}

/// The paths of the files attached in `messages`.
pub fn attached<'a>(messages: impl IntoIterator<Item = &'a Message>) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for message in messages {
        for part in &message.content {
            let ContentPart::ToolResult { content, .. } = part else {
                continue;
            };
            for item in content {
                if let ToolResultContent::Text { text } = item {
                    paths.extend(tagged_paths(text));
                }
            }
        }
    }
    paths
}

/// The paths of the files attached in `turns`.
pub fn attached_in_turns<'a>(turns: impl IntoIterator<Item = &'a Turn>) -> BTreeSet<String> {
    attached(turns.into_iter().flat_map(|turn| turn.messages.iter()))
}

/// The paths in the opening tags of `text`.
fn tagged_paths(text: &str) -> impl Iterator<Item = String> + '_ {
    text.match_indices(OPEN).filter_map(move |(at, _)| {
        let rest = &text[at + OPEN.len()..];
        let end = rest.find("\">")?;
        let path = &rest[..end];
        (!path.is_empty() && !path.contains('\n')).then(|| path.to_owned())
    })
}

/// What the summary of a compaction adds when it replaced turns that had
/// nested files attached: their paths, so the model knows they exist
/// before a call attaches them again.
#[must_use]
pub fn compaction_reminder(paths: &BTreeSet<String>) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let list = paths
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Subdirectories with their own instructions (attached again when you next work in \
         them): {list}."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::instructions::InstructionScope;
    use domain::conversation::ToolUseId;
    use serde_json::json;

    fn file(path: &str, text: &str) -> InstructionFile {
        InstructionFile {
            path: path.into(),
            scope: InstructionScope::Nested,
            text: text.into(),
            imports: Vec::new(),
        }
    }

    #[test]
    fn the_path_of_a_call_comes_from_the_path_tools_only() {
        let input = json!({ "path": " pkg/a.rs " });
        assert_eq!(call_path("read_file", &input), Some("pkg/a.rs"));
        assert_eq!(call_path("tree", &json!({})), None);
        assert_eq!(call_path("run_command", &input), None);
        assert_eq!(call_path("grep", &json!({ "path": "" })), None);
    }

    #[test]
    fn attached_files_are_found_again_in_the_history() {
        let text = attachment(&[
            file("pkg/AGENTS.md", "Use tabs."),
            file("pkg/api/CLAUDE.md", "x"),
        ]);
        assert!(text.contains("<instructions path=\"pkg/AGENTS.md\">\nUse tabs.\n</instructions>"));
        let mut message = Message::user(String::new(), Vec::new());
        message.content = vec![ContentPart::ToolResult {
            tool_use_id: ToolUseId::from("t1"),
            content: vec![
                ToolResultContent::text("fn main() {}"),
                ToolResultContent::text(text),
            ],
            is_error: false,
        }];
        let paths = attached([&message]);
        assert_eq!(
            paths.into_iter().collect::<Vec<_>>(),
            ["pkg/AGENTS.md", "pkg/api/CLAUDE.md"]
        );
    }

    #[test]
    fn the_reminder_names_the_files_or_is_absent() {
        assert_eq!(compaction_reminder(&BTreeSet::new()), None);
        let paths = BTreeSet::from(["a/AGENTS.md".to_owned(), "b/CLAUDE.md".to_owned()]);
        let reminder = compaction_reminder(&paths).unwrap();
        assert!(
            reminder.ends_with("a/AGENTS.md, b/CLAUDE.md."),
            "{reminder}"
        );
    }
}
