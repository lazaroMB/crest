//! The TwoCrest look: the palette of <https://twocrest.mx> (warm ink and
//! paper, a coral accent and an amber highlight) applied to the
//! gpui-component theme, in a dark and a light variant.
//!
//! The palette is written as a gpui-component theme config and applied
//! with [`Theme::apply_config`], so the component tokens derived from it
//! (buttons, lists, scrollbars) follow too.

use std::rc::Rc;

use gpui::App;
use gpui_component::{Theme, ThemeConfig};
use serde_json::json;

use crate::app::ThemeMode;
use palette::*;

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

impl ThemeMode {
    /// The mode `app.opt.theme` names: `dark` (or `twocrest`) and `light`
    /// (or `twocrest-light`).
    ///
    /// # Errors
    ///
    /// A message naming the accepted values.
    pub fn from_name(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "dark" | "twocrest" | "twocrest-dark" => Ok(Self::Dark),
            "light" | "twocrest-light" => Ok(Self::Light),
            other => Err(format!(
                "unknown app.opt.theme `{other}`; use \"dark\" or \"light\""
            )),
        }
    }
}

/// The dark variant: ink background, paper text, coral accent.
const DARK: &[(&str, &str)] = &[
    ("background", INK),
    ("foreground", PAPER),
    ("border", LINE),
    ("input.border", LINE_2),
    ("ring", ACCENT),
    ("caret", ACCENT),
    ("selection.background", "#d9604a59"),
    ("muted.background", SURFACE),
    ("muted.foreground", MUTED),
    ("primary.background", ACCENT),
    ("primary.foreground", INK),
    ("primary.hover.background", "#e2745f"),
    ("primary.active.background", ACCENT_INK),
    ("secondary.background", SURFACE),
    ("secondary.foreground", TEXT_3),
    ("secondary.hover.background", "#1d1d20"),
    ("secondary.active.background", LINE),
    ("accent.background", "#1d1d20"),
    ("accent.foreground", PAPER),
    ("popover.background", SURFACE_2),
    ("popover.foreground", PAPER),
    ("overlay", "#0b0b0cb3"),
    ("list.background", INK),
    ("list.even.background", "#0e0e10"),
    ("list.hover.background", SURFACE),
    ("list.active.background", "#d9604a26"),
    ("list.active.border", ACCENT),
    ("list.head.background", SURFACE_2),
    ("table.background", INK),
    ("table.head.background", SURFACE_2),
    ("table.head.foreground", TEXT_3),
    ("table.hover.background", SURFACE),
    ("table.active.background", "#d9604a26"),
    ("table.active.border", ACCENT),
    ("table.row.border", LINE),
    ("link", ACCENT),
    ("link.hover", AMBER),
    ("link.active", ACCENT_INK),
    ("sidebar.background", SURFACE_2),
    ("sidebar.foreground", TEXT_3),
    ("sidebar.border", LINE),
    ("sidebar.primary.background", ACCENT),
    ("sidebar.primary.foreground", INK),
    ("sidebar.accent.background", SURFACE),
    ("sidebar.accent.foreground", PAPER),
    ("title_bar.background", INK),
    ("title_bar.border", LINE),
    ("status_bar.background", SURFACE_2),
    ("status_bar.border", LINE),
    ("tab.background", INK),
    ("tab.foreground", MUTED),
    ("tab.active.background", SURFACE),
    ("tab.active.foreground", PAPER),
    ("tab_bar.background", SURFACE_2),
    ("scrollbar.background", "#00000000"),
    ("scrollbar.thumb.background", LINE_2),
    ("scrollbar.thumb.hover.background", PAPER_MUTED),
    ("info.background", TEXT_2),
    ("info.foreground", INK),
    ("success.background", "#8fc27a"),
    ("success.foreground", INK),
    ("warning.background", AMBER),
    ("warning.foreground", INK),
    ("danger.background", "#f0524a"),
    ("danger.foreground", PAPER),
    ("chart.1", ACCENT),
    ("chart.2", AMBER),
    ("chart.3", TEXT_3),
    ("chart.4", ACCENT_INK),
    ("chart.5", MUTED),
    ("chart.grid", LINE),
    ("chart.bullish", "#8fc27a"),
    ("chart.bearish", "#f0524a"),
    ("drag.border", ACCENT),
    ("window.border", LINE),
    ("base.red", "#f0524a"),
    ("base.green", "#8fc27a"),
    ("base.yellow", AMBER),
    ("base.blue", TEXT_2),
    ("base.magenta", ACCENT),
    ("base.cyan", TEXT_3),
];

/// The light variant: paper background, ink text, the accent darkened for contrast.
const LIGHT: &[(&str, &str)] = &[
    ("background", PAPER),
    ("foreground", PAPER_TEXT),
    ("border", PAPER_LINE),
    ("input.border", PAPER_LINE),
    ("ring", ACCENT),
    ("caret", ACCENT_INK),
    ("selection.background", "#d9604a40"),
    ("muted.background", PAPER_CHIP),
    ("muted.foreground", PAPER_MUTED),
    ("primary.background", ACCENT_INK),
    ("primary.foreground", PAPER),
    ("primary.hover.background", ACCENT),
    ("primary.active.background", "#8f2e1c"),
    ("secondary.background", PAPER_CHIP),
    ("secondary.foreground", PAPER_TEXT),
    ("secondary.hover.background", PAPER_RULE),
    ("secondary.active.background", PAPER_LINE),
    ("accent.background", PAPER_RULE),
    ("accent.foreground", INK),
    ("popover.background", "#ffffff"),
    ("popover.foreground", PAPER_TEXT),
    ("overlay", "#0b0b0c40"),
    ("list.background", PAPER),
    ("list.even.background", "#f0ece5"),
    ("list.hover.background", PAPER_CHIP),
    ("list.active.background", "#d9604a22"),
    ("list.active.border", ACCENT_INK),
    ("list.head.background", PAPER_CHIP),
    ("table.background", PAPER),
    ("table.head.background", PAPER_CHIP),
    ("table.head.foreground", PAPER_TEXT),
    ("table.hover.background", PAPER_CHIP),
    ("table.active.background", "#d9604a22"),
    ("table.active.border", ACCENT_INK),
    ("table.row.border", PAPER_RULE),
    ("link", ACCENT_INK),
    ("link.hover", ACCENT),
    ("link.active", "#8f2e1c"),
    ("sidebar.background", PAPER_CHIP),
    ("sidebar.foreground", PAPER_TEXT),
    ("sidebar.border", PAPER_LINE),
    ("sidebar.primary.background", ACCENT_INK),
    ("sidebar.primary.foreground", PAPER),
    ("sidebar.accent.background", PAPER_RULE),
    ("sidebar.accent.foreground", INK),
    ("title_bar.background", PAPER),
    ("title_bar.border", PAPER_LINE),
    ("status_bar.background", PAPER_CHIP),
    ("status_bar.border", PAPER_LINE),
    ("tab.background", PAPER),
    ("tab.foreground", PAPER_MUTED),
    ("tab.active.background", PAPER_CHIP),
    ("tab.active.foreground", INK),
    ("tab_bar.background", PAPER_CHIP),
    ("scrollbar.background", "#00000000"),
    ("scrollbar.thumb.background", PAPER_LINE),
    ("scrollbar.thumb.hover.background", PAPER_MUTED),
    ("info.background", PAPER_MUTED),
    ("info.foreground", PAPER),
    ("success.background", "#3f7d3a"),
    ("success.foreground", PAPER),
    ("warning.background", "#b8740c"),
    ("warning.foreground", PAPER),
    ("danger.background", "#c0392b"),
    ("danger.foreground", PAPER),
    ("chart.1", ACCENT),
    ("chart.2", "#d08a1a"),
    ("chart.3", PAPER_TEXT),
    ("chart.4", ACCENT_INK),
    ("chart.5", PAPER_MUTED),
    ("chart.grid", PAPER_RULE),
    ("chart.bullish", "#3f7d3a"),
    ("chart.bearish", "#c0392b"),
    ("drag.border", ACCENT_INK),
    ("window.border", PAPER_LINE),
    ("base.red", "#c0392b"),
    ("base.green", "#3f7d3a"),
    ("base.yellow", "#b8740c"),
    ("base.blue", PAPER_MUTED),
    ("base.magenta", ACCENT_INK),
    ("base.cyan", PAPER_TEXT),
];

/// The theme config of `mode` in the TwoCrest palette.
pub fn config(mode: ThemeMode) -> ThemeConfig {
    let (name, mode_name, colors) = match mode {
        ThemeMode::Dark => ("TwoCrest", "dark", DARK),
        ThemeMode::Light => ("TwoCrest Light", "light", LIGHT),
    };
    let colors: serde_json::Map<String, serde_json::Value> = colors
        .iter()
        .map(|(key, value)| ((*key).to_owned(), json!(value)))
        .collect();
    let value = json!({ "name": name, "mode": mode_name, "colors": colors });
    // The tables above are static and covered by a test.
    serde_json::from_value(value).unwrap_or_default()
}

/// Applies the TwoCrest palette of `mode` to the global theme (after
/// `Theme::change` loaded that mode).
pub fn apply(mode: ThemeMode, cx: &mut App) {
    let config = Rc::new(config(mode));
    Theme::update(cx, |theme| theme.apply_config(&config));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_configs_parse_with_the_brand_accent() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let config = config(mode);
            assert!(config.name.starts_with("TwoCrest"), "{mode:?}");
            assert_eq!(config.mode.is_dark(), mode == ThemeMode::Dark);
        }
    }

    #[test]
    fn theme_names() {
        assert_eq!(ThemeMode::from_name("dark"), Ok(ThemeMode::Dark));
        assert_eq!(ThemeMode::from_name(" TwoCrest "), Ok(ThemeMode::Dark));
        assert_eq!(ThemeMode::from_name("light"), Ok(ThemeMode::Light));
        assert!(ThemeMode::from_name("tokyonight").is_err());
    }
}
