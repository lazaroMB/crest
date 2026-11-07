//! A chat without a directory.
//!
//! It has no workspace root: the file port, the searches and the shell all
//! refuse and ask for a directory. The parts of the app that still want a
//! folder (config files, flow state) get a small one of crest's own, next to
//! the session database; no tool can reach it.

use std::path::{Path, PathBuf};

use adapter_search::RipgrepSearch;
use adapter_shell::LocalShell;
use domain::roots::WorkspaceRoots;
use domain::workspace::WorkspaceRoot;

use super::{Config, prompt_around, system_prompt};

/// What the system prompt says to a chat with no directory.
const NO_DIRECTORY: &str = "You have no workspace directory: file, search and command tools \
     are unavailable and answer that the chat has no directory. Answer from what you know, \
     and tell the user they can add the chat to a project, or open crest on a directory, \
     when they want you to work on files.";

/// crest's own folder for chats without a directory, created if missing:
/// `chats` beside the session database (`db`, or the default one).
pub(crate) fn home(db: Option<&Path>) -> PathBuf {
    let base = db
        .map(Path::to_path_buf)
        .or_else(adapter_store::path::default_db_path)
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("chats");
    // A folder that cannot be made fails the open with its own message.
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The search of a chat in `root`, or the one that finds nothing.
pub(crate) fn search(standalone: bool, root: &WorkspaceRoot) -> RipgrepSearch {
    if standalone {
        RipgrepSearch::standalone()
    } else {
        RipgrepSearch::new(root.clone())
    }
}

/// The directories a chat starts with: its `root`, or none.
pub(crate) fn roots(standalone: bool, root: &WorkspaceRoot) -> WorkspaceRoots {
    if standalone {
        WorkspaceRoots::default()
    } else {
        WorkspaceRoots::new([root.clone()])
    }
}

/// The directory a chat opens on: the one of `config`, or crest's own folder
/// for a chat with none.
pub(crate) fn workspace(config: &Config, db: Option<&Path>) -> PathBuf {
    if config.is_standalone() {
        home(db)
    } else {
        config.workspace.clone()
    }
}

/// The shell of a chat: in `root`, or with none for a standalone chat.
pub(super) fn shell(standalone: bool, root: &WorkspaceRoot) -> LocalShell {
    if standalone {
        LocalShell::standalone()
    } else {
        LocalShell::new(root.clone())
    }
}

/// The system prompt of a chat in `root`, or the one for no directory.
pub(super) fn prompt(standalone: bool, root: &WorkspaceRoot) -> String {
    if standalone {
        prompt_around(NO_DIRECTORY)
    } else {
        system_prompt(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_without_a_directory_has_its_own_folder_and_a_prompt_that_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = home(Some(&tmp.path().join("sessions.db")));
        assert_eq!(dir, tmp.path().join("chats"));
        assert!(dir.is_dir());
        let root = WorkspaceRoot::new(&dir).unwrap();
        let alone = prompt(true, &root);
        assert!(alone.contains("no workspace directory"), "{alone}");
        assert!(!alone.contains("Workspace root:"));
        let in_project = prompt(false, &root);
        assert!(in_project.contains("Workspace root:"));
        assert!(shell(true, &root).root().is_none());
        assert!(shell(false, &root).root().is_some());
    }
}
