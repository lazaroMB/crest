//! File and text pickers: `<leader>ff` (`picker.files`) finds a file and
//! `<leader>fg` (`picker.grep`) searches the workspace contents.
//!
//! Both call the same ports and the same default requests as the agent's
//! `find_file` and `grep` tools ([`files_request`], [`grep_request`]), so
//! the user sees what the agent would see. Results follow the query as it
//! is typed: each keystroke restarts a short debounce, and replacing the
//! search task drops the previous search future, which cancels it (the
//! adapters kill their `rg`/`fzf` processes on drop).

use application::keymap::Action;
use application::ports::search::{FileList, FindFileRequest, GrepRequest, GrepResult};
use domain::workspace::WorkspacePath;

/// Action that opens the file picker.
pub const FILES_ACTION: &str = "picker.files";
/// Action that opens the text picker.
pub const GREP_ACTION: &str = "picker.grep";

/// The request the file picker sends for `query`: the defaults of the
/// `find_file` tool (`{"query": …}`).
pub fn files_request(query: &str) -> FindFileRequest {
    FindFileRequest::new(query)
}

/// The request the text picker sends for `query`: the defaults of the
/// `grep` tool (`{"pattern": …}`), so the pattern is a regex.
pub fn grep_request(query: &str) -> GrepRequest {
    GrepRequest::new(query)
}

/// Which search a picker runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    /// `find_file`: paths ranked by fuzzy match.
    Files,
    /// `grep`: matching lines.
    Grep,
}

impl PickerMode {
    /// The mode an action opens, if it is a picker action.
    pub fn for_action(action: &Action) -> Option<Self> {
        match action {
            Action::Builtin(name) if name == FILES_ACTION => Some(Self::Files),
            Action::Builtin(name) if name == GREP_ACTION => Some(Self::Grep),
            _ => None,
        }
    }
}

/// One row of results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    /// File to open.
    pub path: WorkspacePath,
    /// 1-based line, for text matches.
    pub line: Option<u64>,
    /// The matching line, for text matches.
    pub text: Option<String>,
}

/// Rows of a `find_file` result, in its order.
pub fn items_from_files(list: &FileList) -> Vec<PickerItem> {
    list.paths
        .iter()
        .map(|path| PickerItem {
            path: path.clone(),
            line: None,
            text: None,
        })
        .collect()
}

/// Rows of a `grep` result, in its order.
pub fn items_from_grep(result: &GrepResult) -> Vec<PickerItem> {
    result
        .hits
        .iter()
        .map(|hit| PickerItem {
            path: hit.path.clone(),
            line: Some(hit.line_number),
            text: Some(hit.line.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_pick_the_mode() {
        assert_eq!(
            PickerMode::for_action(&Action::builtin("picker.files")),
            Some(PickerMode::Files)
        );
        assert_eq!(
            PickerMode::for_action(&Action::builtin("picker.grep")),
            Some(PickerMode::Grep)
        );
        assert_eq!(PickerMode::for_action(&Action::builtin("block.next")), None);
    }

    #[test]
    fn requests_are_the_tool_defaults() {
        // What the model gets when it calls the tool with only the query.
        let find: FindFileRequest = serde_json::from_str(r#"{"query":"main"}"#).unwrap();
        assert_eq!(files_request("main"), find);
        let grep: GrepRequest = serde_json::from_str(r#"{"pattern":"run"}"#).unwrap();
        assert_eq!(grep_request("run"), grep);
    }
}
