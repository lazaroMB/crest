//! Where theme files come from: `<config dir>/themes/<name>.lua`.
//!
//! A source only lists and reads themes, as the JSON form of the table each
//! file returns; [`crate::theme`] parses and resolves them. Theme files
//! are small and local, so the port is synchronous: callers off the UI
//! thread may call it directly.

use serde_json::Value;

/// Why a theme cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThemeSourceError {
    /// No theme by that name.
    #[error("no theme `{0}`")]
    NotFound(String),
    /// The file exists but failed: syntax error, runtime error, not a
    /// table.
    #[error("{0}")]
    Broken(String),
}

/// A theme read from a source.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedTheme {
    /// What the diagnostics name: usually the file path.
    pub origin: String,
    /// The table the file returned, as JSON.
    pub table: Value,
}

/// Lists and reads theme files.
pub trait ThemeSourcePort: Send + Sync {
    /// The names of the themes it has (file names without `.lua`), sorted.
    fn list(&self) -> Vec<String>;

    /// Reads the theme `name`.
    ///
    /// # Errors
    ///
    /// [`ThemeSourceError`] when it does not exist or fails.
    fn load(&self, name: &str) -> Result<LoadedTheme, ThemeSourceError>;
}
