//! Instructions port: the `AGENTS.md` and `CLAUDE.md` files a project (and
//! the user) keeps for coding agents.
//!
//! The user's and the workspace root's files go into the system prompt
//! ([`InstructionsPort::base`]); the files of subdirectories are attached
//! to the result of the first tool call that touches their subtree
//! ([`InstructionsPort::nested`]). The adapter reads the files, follows
//! their `@path` imports and drops duplicates; it reloads them when they
//! change, so the next request sees the new text.

use std::sync::Arc;

/// Where an instructions file applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionScope {
    /// The user's own file, for every project.
    User,
    /// A file at the workspace root.
    Project,
    /// A file in a subdirectory: it applies to that subtree.
    Nested,
}

impl InstructionScope {
    /// The word shown for the scope.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Nested => "nested",
        }
    }
}

/// One instructions file, with its imports already inlined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    /// The path shown to the model and the user: relative to the workspace
    /// root for project and nested files, `~/…` or absolute for the user's.
    pub path: String,
    /// Where it applies.
    pub scope: InstructionScope,
    /// Its text, with the files it imports appended.
    pub text: String,
    /// The files it imports (shown like `path`).
    pub imports: Vec<String>,
}

/// Reads the instructions files of the user and the workspace.
///
/// Calls are synchronous: the files are small, and the adapter only reads
/// them again when their modification time changes.
pub trait InstructionsPort: Send + Sync {
    /// The system prompt section with the user's and the root's files, or
    /// an empty text when there are none. The same files give the same
    /// text, so the prompt cache holds until one of them changes.
    fn base(&self) -> Arc<str>;

    /// The files of the directories between the workspace root (excluded)
    /// and `path` (a file or a directory, relative to the root), outermost
    /// first. Empty for a path outside the workspace.
    fn nested(&self, path: &str) -> Vec<InstructionFile>;

    /// The files in [`Self::base`], in their order, for `/help`.
    fn loaded(&self) -> Vec<InstructionFile>;

    /// Problems found reading the files (unreadable, truncated, an import
    /// that cannot be followed), as "path: problem".
    fn warnings(&self) -> Vec<String> {
        Vec::new()
    }
}
