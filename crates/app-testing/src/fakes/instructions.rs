//! [`FixedInstructions`]: an [`InstructionsPort`] with files set by the test.

use std::sync::{Arc, Mutex, PoisonError};

use crate::ports::instructions::{InstructionFile, InstructionScope, InstructionsPort};

/// Instructions files held in memory: a base text the test can change and
/// nested files by directory. Nested lookups are recorded.
#[derive(Debug, Default)]
pub struct FixedInstructions {
    base: Mutex<String>,
    nested: Vec<(String, String)>,
    lookups: Mutex<Vec<String>>,
}

impl FixedInstructions {
    /// No files.
    pub fn new() -> Self {
        Self::default()
    }

    /// The base text is `text`.
    #[must_use]
    pub fn with_base(self, text: &str) -> Self {
        self.set_base(text);
        self
    }

    /// `dir/AGENTS.md` holds `text`.
    #[must_use]
    pub fn with_nested(mut self, dir: &str, text: &str) -> Self {
        self.nested.push((dir.to_owned(), text.to_owned()));
        self
    }

    /// Replaces the base text, as if a file changed.
    pub fn set_base(&self, text: &str) {
        *self.base.lock().unwrap_or_else(PoisonError::into_inner) = text.to_owned();
    }

    /// The paths asked for with [`InstructionsPort::nested`].
    pub fn lookups(&self) -> Vec<String> {
        self.lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl InstructionsPort for FixedInstructions {
    fn base(&self) -> Arc<str> {
        self.base
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_str()
            .into()
    }

    fn nested(&self, path: &str) -> Vec<InstructionFile> {
        self.lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(path.to_owned());
        self.nested
            .iter()
            .filter(|(dir, _)| path.starts_with(&format!("{dir}/")))
            .map(|(dir, text)| InstructionFile {
                path: format!("{dir}/AGENTS.md"),
                scope: InstructionScope::Nested,
                text: text.clone(),
                imports: Vec::new(),
            })
            .collect()
    }

    fn loaded(&self) -> Vec<InstructionFile> {
        Vec::new()
    }
}
