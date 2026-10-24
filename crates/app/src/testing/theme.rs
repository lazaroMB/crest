//! An in-memory [`ThemeSourcePort`].

use std::collections::BTreeMap;

use serde_json::Value;

use crate::ports::theme::{LoadedTheme, ThemeSourceError, ThemeSourcePort};

/// Themes kept in memory, by name.
#[derive(Debug, Clone, Default)]
pub struct FakeThemes {
    themes: BTreeMap<String, Value>,
}

impl FakeThemes {
    /// No themes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the theme `name` with the table `table`.
    #[must_use]
    pub fn with(mut self, name: &str, table: Value) -> Self {
        self.themes.insert(name.to_owned(), table);
        self
    }
}

impl ThemeSourcePort for FakeThemes {
    fn list(&self) -> Vec<String> {
        self.themes.keys().cloned().collect()
    }

    fn load(&self, name: &str) -> Result<LoadedTheme, ThemeSourceError> {
        self.themes
            .get(name)
            .map(|table| LoadedTheme {
                origin: format!("themes/{name}.lua"),
                table: table.clone(),
            })
            .ok_or_else(|| ThemeSourceError::NotFound(name.to_owned()))
    }
}
