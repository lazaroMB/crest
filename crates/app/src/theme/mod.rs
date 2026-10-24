//! Color themes: which ones exist and how a name becomes a [`Theme`].
//!
//! ```lua
//! app.opt.theme  = "tokyonight-night"                    -- ~/.config/crest/themes/tokyonight-night.lua
//! app.opt.colors = { ui = { caret = "#ff9e64" } }       -- over the chosen theme
//! app.theme.define{ name = "mine", extends = "twocrest", ui = { accent = "#7aa2f7" } }
//! ```
//!
//! A name is looked up in this order: the themes defined in the config
//! (`app.theme.define`), the theme files ([`ThemeSourcePort`]) and the
//! built-in TwoCrest themes. A theme may start from another one with
//! `extends`; what neither sets is derived (see [`domain::theme`]).

mod builtin;
mod spec;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use domain::theme::{Appearance, Color, Theme};
use serde_json::Value;

use crate::ports::plugin::AppOptions;
use crate::ports::theme::{ThemeSourceError, ThemeSourcePort};

pub use builtin::{NAMES as BUILTIN_NAMES, TWOCREST, TWOCREST_LIGHT, palette};
pub use spec::{ColorRef, StyleRef, ThemeSpec, parse, parse_overrides};

/// Key of the color overrides in `app.opt`.
pub const COLORS_OPTION: &str = "colors";

/// Longest `extends` chain.
pub const MAX_EXTENDS: usize = 8;

/// Why a theme cannot be resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    /// No theme by that name.
    #[error("unknown theme `{0}`")]
    NotFound(String),
    /// Its file failed to load.
    #[error("theme `{name}`: {reason}")]
    Broken {
        /// The theme.
        name: String,
        /// What went wrong.
        reason: String,
    },
    /// `extends` loops back.
    #[error("theme `{0}`: `extends` loops back to it")]
    Cycle(String),
    /// `extends` is longer than [`MAX_EXTENDS`].
    #[error("theme `{0}`: `extends` is nested more than {MAX_EXTENDS} deep")]
    TooDeep(String),
}

/// A resolved theme and the problems found on the way (unknown tokens,
/// bad colors), each `origin: path: problem`.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The theme.
    pub theme: Arc<Theme>,
    /// Problems that were skipped.
    pub warnings: Vec<String>,
}

/// Where a theme comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThemeOrigin {
    /// Built in (TwoCrest).
    Builtin,
    /// A theme file.
    File,
    /// `app.theme.define` in the config.
    Config,
}

/// A theme name with where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeEntry {
    /// The name.
    pub name: String,
    /// Where it comes from.
    pub origin: ThemeOrigin,
}

/// The canonical name of `name`: `dark` and `twocrest-dark` are
/// `twocrest`, `light` is `twocrest-light`; other names are trimmed and
/// lowercased.
#[must_use]
pub fn canonical_name(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    match name.as_str() {
        "dark" | "twocrest-dark" => TWOCREST.to_owned(),
        "light" => TWOCREST_LIGHT.to_owned(),
        _ => name,
    }
}

/// Every theme crest can resolve.
#[derive(Clone, Default)]
pub struct ThemeCatalog {
    defined: BTreeMap<String, (String, Value)>,
    files: Option<Arc<dyn ThemeSourcePort>>,
}

impl std::fmt::Debug for ThemeCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemeCatalog")
            .field("defined", &self.defined.keys().collect::<Vec<_>>())
            .field("files", &self.files.is_some())
            .finish()
    }
}

impl ThemeCatalog {
    /// Only the built-in themes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// With the theme files of `source`.
    #[must_use]
    pub fn with_files(mut self, source: Arc<dyn ThemeSourcePort>) -> Self {
        self.files = Some(source);
        self
    }

    /// With the themes defined in the config (`app.theme.define`), each a
    /// table with a `name`. Returns the catalog and the tables that were
    /// skipped for having no name.
    #[must_use]
    pub fn with_defined(mut self, tables: &[Value]) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        for table in tables {
            match table.get("name").and_then(Value::as_str) {
                Some(name) if !name.trim().is_empty() => {
                    self.defined.insert(
                        canonical_name(name),
                        (format!("app.theme.define `{name}`"), table.clone()),
                    );
                }
                _ => warnings.push("app.theme.define: a theme needs a `name`".to_owned()),
            }
        }
        (self, warnings)
    }

    /// Every theme name, sorted; a name defined in several places is
    /// listed once, with the origin that wins.
    #[must_use]
    pub fn entries(&self) -> Vec<ThemeEntry> {
        let mut entries: BTreeMap<String, ThemeOrigin> = BTreeMap::new();
        for name in BUILTIN_NAMES {
            entries.insert(name.to_owned(), ThemeOrigin::Builtin);
        }
        if let Some(files) = &self.files {
            for name in files.list() {
                entries.insert(canonical_name(&name), ThemeOrigin::File);
            }
        }
        for name in self.defined.keys() {
            entries.insert(name.clone(), ThemeOrigin::Config);
        }
        entries
            .into_iter()
            .map(|(name, origin)| ThemeEntry { name, origin })
            .collect()
    }

    /// The spec of `name` as written, with the origin its warnings name.
    fn spec(&self, name: &str) -> Result<(ThemeSpec, Vec<String>), ThemeError> {
        if let Some((origin, table)) = self.defined.get(name) {
            return Ok(parse(table, origin));
        }
        if let Some(files) = &self.files {
            match files.load(name) {
                Ok(loaded) => return Ok(parse(&loaded.table, &loaded.origin)),
                Err(ThemeSourceError::NotFound(_)) => {}
                Err(ThemeSourceError::Broken(reason)) => {
                    return Err(ThemeError::Broken {
                        name: name.to_owned(),
                        reason,
                    });
                }
            }
        }
        builtin::spec(name)
            .map(|spec| (spec, Vec::new()))
            .ok_or_else(|| ThemeError::NotFound(name.to_owned()))
    }

    /// Resolves the theme `name`, then applies `overrides`
    /// (`app.opt.colors`) over it.
    ///
    /// # Errors
    ///
    /// [`ThemeError`] when the theme or one it extends is missing or
    /// broken, or `extends` loops.
    pub fn resolve(
        &self,
        name: &str,
        overrides: Option<&ThemeSpec>,
    ) -> Result<Resolved, ThemeError> {
        let name = canonical_name(name);
        let mut chain = Vec::new();
        let mut warnings = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = name.clone();
        loop {
            if !seen.insert(current.clone()) {
                return Err(ThemeError::Cycle(name));
            }
            if chain.len() == MAX_EXTENDS {
                return Err(ThemeError::TooDeep(name));
            }
            let (spec, problems) = self.spec(&current)?;
            warnings.extend(problems);
            let next = spec.extends.as_deref().map(canonical_name);
            chain.push(spec);
            match next {
                Some(next) => current = next,
                None => break,
            }
        }
        // From the base to the theme, then the overrides.
        chain.reverse();
        if let Some(overrides) = overrides {
            chain.push(overrides.clone());
        }
        let appearance = chain
            .iter()
            .rev()
            .find_map(|spec| spec.appearance)
            .unwrap_or_default();
        let mut palette = BTreeMap::new();
        let mut colors = BTreeMap::new();
        let mut styles = BTreeMap::new();
        for spec in &chain {
            palette.extend(spec.palette.clone());
            colors.extend(spec.colors.clone());
            styles.extend(spec.syntax.clone());
        }
        let mut set: BTreeMap<String, Color> = BTreeMap::new();
        for (path, color) in colors {
            match spec::lookup(&color, &palette) {
                Some(color) => {
                    set.insert(path, color);
                }
                None => warnings.push(format!(
                    "theme `{name}`: ui.{path}: unknown color `{}` (not in the palette)",
                    spec::name_of(&color)
                )),
            }
        }
        let mut set_syntax = BTreeMap::new();
        for (syntax_name, style) in styles {
            match spec::style(&style, &palette) {
                Ok(style) => {
                    set_syntax.insert(syntax_name, style);
                }
                Err(color) => warnings.push(format!(
                    "theme `{name}`: syntax.{syntax_name}: unknown color `{color}` (not in the palette)"
                )),
            }
        }
        Ok(Resolved {
            theme: Arc::new(Theme::derive(name, appearance, &set, &set_syntax)),
            warnings,
        })
    }

    /// The theme `name`, or the built-in theme of the same appearance
    /// (dark when unknown) with the error as a warning.
    #[must_use]
    pub fn resolve_or_default(&self, name: &str, overrides: Option<&ThemeSpec>) -> Resolved {
        match self.resolve(name, overrides) {
            Ok(resolved) => resolved,
            Err(err) => {
                let mut fallback = self
                    .resolve(TWOCREST, overrides)
                    .unwrap_or_else(|_| default_resolved());
                fallback
                    .warnings
                    .insert(0, format!("{err}; using the {TWOCREST} theme"));
                fallback
            }
        }
    }
}

fn default_resolved() -> Resolved {
    Resolved {
        theme: Arc::new(Theme::derive(
            TWOCREST,
            Appearance::Dark,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )),
        warnings: Vec::new(),
    }
}

/// The color overrides of `app.opt.colors`, with their warnings.
#[must_use]
pub fn overrides_from_options(options: &AppOptions) -> (Option<ThemeSpec>, Vec<String>) {
    match options.extra.get(COLORS_OPTION) {
        None | Some(Value::Null) => (None, Vec::new()),
        Some(value) => {
            let (spec, warnings) = parse_overrides(value);
            (Some(spec), warnings)
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::theme::FakeThemes;

    fn hex(text: &str) -> Color {
        Color::parse_hex(text).unwrap()
    }

    fn catalog() -> ThemeCatalog {
        let files = FakeThemes::new()
            .with(
                "night",
                json!({
                    "appearance": "dark",
                    "palette": { "bg": "#1a1b26", "fg": "#c0caf5", "blue": "#7aa2f7" },
                    "ui": { "background": "bg", "foreground": "fg", "accent": "blue" },
                    "syntax": { "keyword": "blue" },
                }),
            )
            .with(
                "night-orange",
                json!({
                    "extends": "night",
                    "palette": { "orange": "#ff9e64" },
                    "ui": { "accent": "orange", "caret": "blue" },
                }),
            )
            .with("loop-a", json!({ "extends": "loop-b" }))
            .with("loop-b", json!({ "extends": "loop-a" }))
            .with("typo", json!({ "ui": { "accent": "bleu", "carret": "#000000" } }));
        ThemeCatalog::new().with_files(Arc::new(files))
    }

    #[test]
    fn builtin_twocrest_keeps_the_brand_colors() {
        let resolved = ThemeCatalog::new().resolve("dark", None).unwrap();
        let theme = resolved.theme;
        assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
        assert_eq!(theme.name, TWOCREST);
        assert_eq!(theme.color("background"), hex(palette::INK));
        assert_eq!(theme.color("accent.bg"), hex(palette::ACCENT));
        assert_eq!(theme.color("error.bg"), hex("#f0524a"));
        let light = ThemeCatalog::new().resolve("light", None).unwrap().theme;
        assert_eq!(light.appearance, Appearance::Light);
        assert_eq!(light.color("background"), hex(palette::PAPER));
    }

    #[test]
    fn resolves_palette_names_and_extends() {
        let theme = catalog().resolve("night-orange", None).unwrap();
        assert!(theme.warnings.is_empty(), "{:?}", theme.warnings);
        let theme = theme.theme;
        assert_eq!(theme.name, "night-orange");
        assert_eq!(theme.appearance, Appearance::Dark);
        assert_eq!(theme.color("background"), hex("#1a1b26"));
        assert_eq!(theme.color("accent.bg"), hex("#ff9e64"));
        // The base's palette serves the child.
        assert_eq!(theme.color("caret"), hex("#7aa2f7"));
        assert_eq!(theme.syntax("keyword").unwrap().fg, Some(hex("#7aa2f7")));
    }

    #[test]
    fn overrides_go_on_top() {
        let (overrides, warnings) = parse_overrides(&json!({ "ui": { "caret": "#ff0000" } }));
        assert!(warnings.is_empty());
        let theme = catalog().resolve("night", Some(&overrides)).unwrap().theme;
        assert_eq!(theme.color("caret"), hex("#ff0000"));
        assert_eq!(theme.color("accent.bg"), hex("#7aa2f7"));
    }

    #[test]
    fn problems() {
        assert_eq!(
            catalog().resolve("loop-a", None),
            Err(ThemeError::Cycle("loop-a".into()))
        );
        assert_eq!(
            catalog().resolve("nope", None),
            Err(ThemeError::NotFound("nope".into()))
        );
        let typo = catalog().resolve("typo", None).unwrap();
        let all = typo.warnings.join("\n");
        assert!(all.contains("ui.carret: unknown token"), "{all}");
        assert!(all.contains("ui.accent.bg: unknown color `bleu`"), "{all}");
        // The accent falls back to its default.
        assert_eq!(typo.theme.color("accent.bg"), hex(palette::ACCENT));

        let fallback = catalog().resolve_or_default("nope", None);
        assert_eq!(fallback.theme.name, TWOCREST);
        assert!(fallback.warnings[0].starts_with("unknown theme `nope`"));
    }

    #[test]
    fn defined_themes_win_and_need_a_name() {
        let (catalog, warnings) = catalog().with_defined(&[
            json!({ "name": "Night", "ui": { "background": "#000000" } }),
            json!({ "ui": {} }),
        ]);
        assert_eq!(warnings, ["app.theme.define: a theme needs a `name`"]);
        let theme = catalog.resolve("night", None).unwrap().theme;
        assert_eq!(theme.color("background"), hex("#000000"));
        let entries = catalog.entries();
        assert!(entries.contains(&ThemeEntry {
            name: "night".into(),
            origin: ThemeOrigin::Config
        }));
        assert!(entries.contains(&ThemeEntry {
            name: TWOCREST.into(),
            origin: ThemeOrigin::Builtin
        }));
    }
}
