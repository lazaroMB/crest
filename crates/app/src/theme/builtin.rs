//! The built-in TwoCrest themes: the palette of <https://twocrest.mx>
//! (warm ink and paper, a coral accent and an amber highlight), dark and
//! light.

use std::collections::BTreeMap;

use domain::theme::{Appearance, Color};

use super::spec::{ColorRef, StyleRef, ThemeSpec};

/// Name of the dark TwoCrest theme, the default.
pub const TWOCREST: &str = "twocrest";
/// Name of the light TwoCrest theme.
pub const TWOCREST_LIGHT: &str = "twocrest-light";

/// The brand colors of twocrest.mx, by the names of its CSS variables.
pub mod palette {
    /// `--ink`: the dark background.
    pub const INK: &str = "#0b0b0c";
    /// `--surface`: cards on the dark background.
    pub const SURFACE: &str = "#161618";
    /// `--surface-2`: panels on the dark background.
    pub const SURFACE_2: &str = "#111113";
    /// `--line`: borders on the dark background.
    pub const LINE: &str = "#2a2a2d";
    /// `--line-2`: stronger borders on the dark background.
    pub const LINE_2: &str = "#3a3a3e";
    /// `--paper`: text on the dark background, and the light background.
    pub const PAPER: &str = "#f4f1ec";
    /// `--text-2`: secondary text on the dark background.
    pub const TEXT_2: &str = "#b9b5ae";
    /// `--text-3`: tertiary text on the dark background.
    pub const TEXT_3: &str = "#d6d2cb";
    /// `--muted`: muted text on the dark background.
    pub const MUTED: &str = "#8e8a84";
    /// `--accent`: the coral of the logo.
    pub const ACCENT: &str = "#d9604a";
    /// `--accent-ink`: the accent on paper, and pressed.
    pub const ACCENT_INK: &str = "#ae3a25";
    /// `--amber`: highlights and warnings.
    pub const AMBER: &str = "#ffae42";
    /// `--paper-chip`: chips on paper.
    pub const PAPER_CHIP: &str = "#ece7df";
    /// `--paper-rule`: rules on paper.
    pub const PAPER_RULE: &str = "#e6e1d8";
    /// `--paper-line`: borders on paper.
    pub const PAPER_LINE: &str = "#ddd8cf";
    /// `--paper-text`: text on paper.
    pub const PAPER_TEXT: &str = "#3d3a35";
    /// `--paper-muted`: muted text on paper.
    pub const PAPER_MUTED: &str = "#57534c";
}

use palette::*;

const DARK: &[(&str, &str)] = &[
    ("background", INK),
    ("foreground", PAPER),
    ("accent.bg", ACCENT),
    ("ansi.red", "#f0524a"),
    ("ansi.green", "#8fc27a"),
    ("ansi.yellow", AMBER),
    ("ansi.blue", TEXT_2),
    ("ansi.magenta", ACCENT),
    ("ansi.cyan", TEXT_3),
    ("border", LINE),
    ("muted.bg", SURFACE),
    ("muted.fg", MUTED),
    ("selection", "#d9604a59"),
    ("caret", ACCENT),
    ("overlay", "#0b0b0cb3"),
    ("accent.fg", INK),
    ("accent.hover.bg", "#e2745f"),
    ("accent.active.bg", ACCENT_INK),
    ("button.bg", SURFACE),
    ("button.fg", TEXT_3),
    ("button.hover.bg", "#1d1d20"),
    ("button.hover.fg", PAPER),
    ("button.active.bg", LINE),
    ("input.border", LINE_2),
    ("input.focused.border", ACCENT),
    ("list.bg", INK),
    ("list.even.bg", "#0e0e10"),
    ("list.header.bg", SURFACE_2),
    ("list.header.fg", TEXT_3),
    ("list.hover.bg", SURFACE),
    ("list.selected.bg", "#d9604a26"),
    ("list.selected.border", ACCENT),
    ("popover.bg", SURFACE_2),
    ("popover.fg", PAPER),
    ("title_bar.bg", INK),
    ("title_bar.border", LINE),
    ("status_bar.bg", SURFACE_2),
    ("status_bar.border", LINE),
    ("sidebar.bg", SURFACE_2),
    ("sidebar.fg", TEXT_3),
    ("sidebar.border", LINE),
    ("sidebar.hover.bg", SURFACE),
    ("sidebar.hover.fg", PAPER),
    ("sidebar.selected.bg", ACCENT),
    ("sidebar.selected.fg", INK),
    ("tab.bar.bg", SURFACE_2),
    ("tab.bg", INK),
    ("tab.fg", MUTED),
    ("tab.selected.bg", SURFACE),
    ("tab.selected.fg", PAPER),
    ("scrollbar.thumb.bg", LINE_2),
    ("scrollbar.thumb.hover.bg", PAPER_MUTED),
    ("link.fg", ACCENT),
    ("link.hover.fg", AMBER),
    ("link.active.fg", ACCENT_INK),
    ("info.bg", TEXT_2),
    ("info.fg", INK),
    ("success.fg", INK),
    ("warning.fg", INK),
    ("error.fg", PAPER),
    ("chart.1", ACCENT),
    ("chart.2", AMBER),
    ("chart.3", TEXT_3),
    ("chart.4", ACCENT_INK),
    ("chart.5", MUTED),
];

const LIGHT: &[(&str, &str)] = &[
    ("background", PAPER),
    ("foreground", PAPER_TEXT),
    ("accent.bg", ACCENT_INK),
    ("ansi.red", "#c0392b"),
    ("ansi.green", "#3f7d3a"),
    ("ansi.yellow", "#b8740c"),
    ("ansi.blue", PAPER_MUTED),
    ("ansi.magenta", ACCENT_INK),
    ("ansi.cyan", PAPER_TEXT),
    ("border", PAPER_LINE),
    ("muted.bg", PAPER_CHIP),
    ("muted.fg", PAPER_MUTED),
    ("selection", "#d9604a40"),
    ("caret", ACCENT_INK),
    ("overlay", "#0b0b0c40"),
    ("accent.fg", PAPER),
    ("accent.hover.bg", ACCENT),
    ("accent.active.bg", "#8f2e1c"),
    ("button.bg", PAPER_CHIP),
    ("button.fg", PAPER_TEXT),
    ("button.hover.bg", PAPER_RULE),
    ("button.hover.fg", INK),
    ("button.active.bg", PAPER_LINE),
    ("input.border", PAPER_LINE),
    ("input.focused.border", ACCENT),
    ("list.bg", PAPER),
    ("list.even.bg", "#f0ece5"),
    ("list.header.bg", PAPER_CHIP),
    ("list.header.fg", PAPER_TEXT),
    ("list.hover.bg", PAPER_CHIP),
    ("list.selected.bg", "#d9604a22"),
    ("list.selected.border", ACCENT_INK),
    ("popover.bg", "#ffffff"),
    ("popover.fg", PAPER_TEXT),
    ("title_bar.bg", PAPER),
    ("title_bar.border", PAPER_LINE),
    ("status_bar.bg", PAPER_CHIP),
    ("status_bar.border", PAPER_LINE),
    ("sidebar.bg", PAPER_CHIP),
    ("sidebar.fg", PAPER_TEXT),
    ("sidebar.border", PAPER_LINE),
    ("sidebar.hover.bg", PAPER_RULE),
    ("sidebar.hover.fg", INK),
    ("sidebar.selected.bg", ACCENT_INK),
    ("sidebar.selected.fg", PAPER),
    ("tab.bar.bg", PAPER_CHIP),
    ("tab.bg", PAPER),
    ("tab.fg", PAPER_MUTED),
    ("tab.selected.bg", PAPER_CHIP),
    ("tab.selected.fg", INK),
    ("scrollbar.thumb.bg", PAPER_LINE),
    ("scrollbar.thumb.hover.bg", PAPER_MUTED),
    ("link.fg", ACCENT_INK),
    ("link.hover.fg", ACCENT),
    ("link.active.fg", "#8f2e1c"),
    ("info.bg", PAPER_MUTED),
    ("info.fg", PAPER),
    ("success.fg", PAPER),
    ("warning.fg", PAPER),
    ("error.fg", PAPER),
    ("chart.1", ACCENT),
    ("chart.2", "#d08a1a"),
    ("chart.3", PAPER_TEXT),
    ("chart.4", ACCENT_INK),
    ("chart.5", PAPER_MUTED),
];

/// Syntax colors: coral keywords, amber numbers, green strings.
const DARK_SYNTAX: &[(&str, &str)] = &[
    ("keyword", ACCENT),
    ("function", "#f0a08e"),
    ("type", "#e8c07d"),
    ("constant", AMBER),
    ("number", AMBER),
    ("boolean", AMBER),
    ("string", "#8fc27a"),
    ("property", TEXT_3),
    ("tag", ACCENT),
    ("attribute", AMBER),
];

const LIGHT_SYNTAX: &[(&str, &str)] = &[
    ("keyword", ACCENT_INK),
    ("function", "#8f2e1c"),
    ("type", "#8a5a0a"),
    ("constant", "#b8740c"),
    ("number", "#b8740c"),
    ("boolean", "#b8740c"),
    ("string", "#3f7d3a"),
    ("property", PAPER_TEXT),
    ("tag", ACCENT_INK),
    ("attribute", "#b8740c"),
];

/// The built-in theme `name`, if it is one.
#[must_use]
pub fn spec(name: &str) -> Option<ThemeSpec> {
    let (appearance, colors, syntax) = match name {
        TWOCREST => (Appearance::Dark, DARK, DARK_SYNTAX),
        TWOCREST_LIGHT => (Appearance::Light, LIGHT, LIGHT_SYNTAX),
        _ => return None,
    };
    let color = |hex: &str| ColorRef::Hex(Color::parse_hex(hex).unwrap_or(Color::TRANSPARENT));
    Some(ThemeSpec {
        name: Some(name.to_owned()),
        appearance: Some(appearance),
        extends: None,
        palette: BTreeMap::new(),
        colors: colors
            .iter()
            .map(|(path, hex)| ((*path).to_owned(), color(hex)))
            .collect(),
        syntax: syntax
            .iter()
            .map(|(name, hex)| {
                let style = StyleRef {
                    fg: Some(color(hex)),
                    ..StyleRef::default()
                };
                ((*name).to_owned(), style)
            })
            .collect(),
    })
}

/// The names of the built-in themes.
pub const NAMES: [&str; 2] = [TWOCREST, TWOCREST_LIGHT];

#[cfg(test)]
mod tests {
    use domain::theme::{SYNTAX, token_spec};

    use super::*;

    #[test]
    fn every_entry_is_a_known_token_with_a_valid_color() {
        for (path, hex) in DARK.iter().chain(LIGHT) {
            assert!(token_spec(path).is_some(), "{path}");
            assert!(Color::parse_hex(hex).is_ok(), "{path}: {hex}");
        }
        for (name, hex) in DARK_SYNTAX.iter().chain(LIGHT_SYNTAX) {
            assert!(SYNTAX.iter().any(|s| s.name == *name), "{name}");
            assert!(Color::parse_hex(hex).is_ok(), "{name}: {hex}");
        }
    }
}
