//! Applies a resolved [`domain::theme::Theme`] to the window.
//!
//! The tokens become a gpui-component theme config (applied with
//! [`Theme::apply_config`], so the component tokens derived from it —
//! buttons, lists, scrollbars — follow too) plus its syntax highlight
//! styles. The theme itself is also kept as a global ([`CrestTheme`]) for
//! the tokens gpui-component has no field for: diff rows, the focused
//! block edge, diagrams and the 3D views.

use std::rc::Rc;
use std::sync::Arc;

use application::theme::ThemeCatalog;
use domain::theme::{Appearance, Color, TextStyle, Theme as CrestThemeData};
use gpui::{App, Global, Hsla, Rgba};
use gpui_component::{Theme, ThemeConfig};
use serde_json::{Map, Value, json};

/// gpui-component keys and the token each one takes.
const KEYS: &[(&str, &str)] = &[
    ("background", "background"),
    ("foreground", "foreground"),
    ("border", "border"),
    ("window.border", "border"),
    ("input.border", "input.border"),
    ("ring", "input.focused.border"),
    ("caret", "caret"),
    ("selection.background", "selection"),
    ("muted.background", "muted.bg"),
    ("muted.foreground", "muted.fg"),
    ("primary.background", "accent.bg"),
    ("primary.foreground", "accent.fg"),
    ("primary.hover.background", "accent.hover.bg"),
    ("primary.active.background", "accent.active.bg"),
    ("drag.border", "accent.bg"),
    ("secondary.background", "button.bg"),
    ("secondary.foreground", "button.fg"),
    ("secondary.hover.background", "button.hover.bg"),
    ("secondary.active.background", "button.active.bg"),
    ("accent.background", "button.hover.bg"),
    ("accent.foreground", "button.hover.fg"),
    ("popover.background", "popover.bg"),
    ("popover.foreground", "popover.fg"),
    ("overlay", "overlay"),
    ("list.background", "list.bg"),
    ("list.even.background", "list.even.bg"),
    ("list.hover.background", "list.hover.bg"),
    ("list.active.background", "list.selected.bg"),
    ("list.active.border", "list.selected.border"),
    ("list.head.background", "list.header.bg"),
    ("table.background", "list.bg"),
    ("table.even.background", "list.even.bg"),
    ("table.head.background", "list.header.bg"),
    ("table.head.foreground", "list.header.fg"),
    ("table.hover.background", "list.hover.bg"),
    ("table.active.background", "list.selected.bg"),
    ("table.active.border", "list.selected.border"),
    ("table.row.border", "border"),
    ("link", "link.fg"),
    ("link.hover", "link.hover.fg"),
    ("link.active", "link.active.fg"),
    ("sidebar.background", "sidebar.bg"),
    ("sidebar.foreground", "sidebar.fg"),
    ("sidebar.border", "sidebar.border"),
    ("sidebar.primary.background", "sidebar.selected.bg"),
    ("sidebar.primary.foreground", "sidebar.selected.fg"),
    ("sidebar.accent.background", "sidebar.hover.bg"),
    ("sidebar.accent.foreground", "sidebar.hover.fg"),
    ("title_bar.background", "title_bar.bg"),
    ("title_bar.border", "title_bar.border"),
    ("status_bar.background", "status_bar.bg"),
    ("status_bar.border", "status_bar.border"),
    ("tab.background", "tab.bg"),
    ("tab.foreground", "tab.fg"),
    ("tab.active.background", "tab.selected.bg"),
    ("tab.active.foreground", "tab.selected.fg"),
    ("tab_bar.background", "tab.bar.bg"),
    ("scrollbar.background", "scrollbar.bg"),
    ("scrollbar.thumb.background", "scrollbar.thumb.bg"),
    (
        "scrollbar.thumb.hover.background",
        "scrollbar.thumb.hover.bg",
    ),
    ("info.background", "info.bg"),
    ("info.foreground", "info.fg"),
    ("success.background", "success.bg"),
    ("success.foreground", "success.fg"),
    ("warning.background", "warning.bg"),
    ("warning.foreground", "warning.fg"),
    ("danger.background", "error.bg"),
    ("danger.foreground", "error.fg"),
    ("chart.1", "chart.1"),
    ("chart.2", "chart.2"),
    ("chart.3", "chart.3"),
    ("chart.4", "chart.4"),
    ("chart.5", "chart.5"),
    ("chart.grid", "chart.grid"),
    ("chart.bullish", "success.bg"),
    ("chart.bearish", "error.bg"),
    ("base.red", "ansi.red"),
    ("base.green", "ansi.green"),
    ("base.yellow", "ansi.yellow"),
    ("base.blue", "ansi.blue"),
    ("base.magenta", "ansi.magenta"),
    ("base.cyan", "ansi.cyan"),
];

/// The highlight keys of the code editor and the token each one takes.
const EDITOR_KEYS: &[(&str, &str)] = &[
    ("editor.background", "background"),
    ("editor.foreground", "foreground"),
    ("editor.active_line.background", "editor.active_line.bg"),
    ("editor.line_number", "editor.line_number.fg"),
    ("editor.active_line_number", "editor.active_line_number.fg"),
    ("error", "error.bg"),
    ("warning", "warning.bg"),
    ("info", "info.bg"),
    ("success", "success.bg"),
    ("hint", "muted.fg"),
];

/// The theme shown in the window, for the tokens gpui-component has no
/// field for. Read it with [`crest_theme`].
#[derive(Debug, Clone)]
pub struct CrestTheme(pub Arc<CrestThemeData>);

impl Global for CrestTheme {}

/// The theme in use (the built-in dark one before any is applied).
#[must_use]
pub fn crest_theme(cx: &App) -> Arc<CrestThemeData> {
    cx.try_global::<CrestTheme>()
        .map_or_else(default_theme, |theme| Arc::clone(&theme.0))
}

/// A token of the theme in use, as a GPUI color.
#[must_use]
pub fn token(cx: &App, path: &str) -> Hsla {
    hsla(crest_theme(cx).color(path))
}

/// The built-in dark theme.
#[must_use]
pub fn default_theme() -> Arc<CrestThemeData> {
    ThemeCatalog::new()
        .resolve_or_default(application::theme::TWOCREST, None)
        .theme
}

/// A theme color as a GPUI color.
#[must_use]
pub fn hsla(color: Color) -> Hsla {
    Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0,
    }
    .into()
}

/// The gpui-component mode of an appearance.
#[must_use]
pub fn mode(appearance: Appearance) -> gpui_component::ThemeMode {
    match appearance {
        Appearance::Dark => gpui_component::ThemeMode::Dark,
        Appearance::Light => gpui_component::ThemeMode::Light,
    }
}

fn style_json(style: TextStyle) -> Value {
    let mut out = Map::new();
    if let Some(fg) = style.fg {
        out.insert("color".into(), json!(fg.to_hex()));
    }
    if style.italic {
        out.insert("font_style".into(), json!("italic"));
    }
    if style.bold {
        out.insert("font_weight".into(), json!(700));
    }
    Value::Object(out)
}

/// The gpui-component theme config of `theme`.
#[must_use]
pub fn config(theme: &CrestThemeData) -> ThemeConfig {
    let hex = |path: &str| json!(theme.color(path).to_hex());
    let colors: Map<String, Value> = KEYS
        .iter()
        .map(|(key, path)| ((*key).to_owned(), hex(path)))
        .collect();
    let mut highlight: Map<String, Value> = EDITOR_KEYS
        .iter()
        .map(|(key, path)| ((*key).to_owned(), hex(path)))
        .collect();
    let syntax: Map<String, Value> = theme
        .syntax_styles()
        .map(|(name, style)| (name.to_owned(), style_json(style)))
        .collect();
    highlight.insert("syntax".into(), Value::Object(syntax));
    let value = json!({
        "name": theme.name,
        "mode": theme.appearance.name(),
        "colors": colors,
        "highlight": highlight,
    });
    // Every value above is a valid hex color or style; covered by a test.
    serde_json::from_value(value).unwrap_or_default()
}

/// Applies `theme` to the global gpui-component theme (after
/// `Theme::change` loaded its mode) and keeps it as [`CrestTheme`].
pub fn apply(theme: &Arc<CrestThemeData>, cx: &mut App) {
    let config = Rc::new(config(theme));
    Theme::update(cx, |gpui_theme| gpui_theme.apply_config(&config));
    cx.set_global(CrestTheme(Arc::clone(theme)));
}

#[cfg(test)]
mod tests {
    use domain::theme::token_spec;

    use super::*;

    #[test]
    fn every_key_takes_a_known_token() {
        for (key, path) in KEYS.iter().chain(EDITOR_KEYS) {
            assert!(token_spec(path).is_some(), "{key} takes unknown {path}");
        }
    }

    #[test]
    fn the_config_parses_with_every_color_and_highlight() {
        let theme = default_theme();
        let config = config(&theme);
        assert_eq!(config.name, "twocrest");
        assert!(config.mode.is_dark());
        let background = config.colors.background.expect("background");
        assert_eq!(background.as_ref(), theme.color("background").to_hex());
        let highlight = config.highlight.expect("highlight");
        assert!(highlight.syntax.style("keyword").is_some());
        assert!(highlight.syntax.style("string.escape").is_some());
        assert_eq!(
            highlight.editor_background,
            Some(hsla(theme.color("background")))
        );
    }

    #[test]
    fn hsla_round_trips_hex() {
        let color = Color::parse_hex("#d9604a").unwrap();
        let rgba: Rgba = hsla(color).into();
        assert!((rgba.r - f32::from(0xd9_u8) / 255.0).abs() < 0.01);
        assert!((rgba.a - 1.0).abs() < f32::EPSILON);
    }
}
