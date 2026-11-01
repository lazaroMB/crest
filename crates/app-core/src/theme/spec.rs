//! Reading a theme table (the JSON form of what a theme file returns, of
//! `app.theme.define{...}` or of `app.opt.colors`).
//!
//! Each field is read on its own: a malformed one is skipped with a
//! diagnostic and the rest still apply, as with `app.opt`.

use std::collections::BTreeMap;

use domain::theme::{Appearance, Color, StyleSpec, syntax_spec, token_spec};
use serde_json::{Map, Value};

/// A color as a theme writes it: hex, or the name of a palette entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorRef {
    /// `#rrggbb` or `#rrggbbaa`.
    Hex(Color),
    /// A palette entry, resolved once the palette of the whole `extends`
    /// chain is known.
    Name(String),
}

/// A syntax style as a theme writes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleRef {
    /// The color.
    pub fg: Option<ColorRef>,
    /// Italic.
    pub italic: Option<bool>,
    /// Bold.
    pub bold: Option<bool>,
}

/// A theme as written, before `extends` and palette names are resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeSpec {
    /// `name`.
    pub name: Option<String>,
    /// `appearance`.
    pub appearance: Option<Appearance>,
    /// `extends`: the theme it starts from.
    pub extends: Option<String>,
    /// `palette`: named colors.
    pub palette: BTreeMap<String, Color>,
    /// `ui`, flattened to token paths.
    pub colors: BTreeMap<String, ColorRef>,
    /// `syntax`.
    pub syntax: BTreeMap<String, StyleRef>,
}

const FIELDS: [&str; 6] = ["name", "appearance", "extends", "palette", "ui", "syntax"];
const STYLE_FIELDS: [&str; 3] = ["fg", "italic", "bold"];

/// Reads a theme table. Problems are returned as messages prefixed with
/// `origin`.
#[must_use]
pub fn parse(value: &Value, origin: &str) -> (ThemeSpec, Vec<String>) {
    let mut report = Report {
        origin,
        messages: Vec::new(),
    };
    let mut spec = ThemeSpec::default();
    let Some(table) = as_table(value) else {
        report.push("", "a theme must be a table");
        return (spec, report.messages);
    };
    for (key, value) in table {
        match key.as_str() {
            "name" => match value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                Some(name) => spec.name = Some(name.to_owned()),
                None => report.push("name", "must be a non-empty string"),
            },
            "appearance" => match value.as_str().and_then(Appearance::from_name) {
                Some(appearance) => spec.appearance = Some(appearance),
                None => report.push("appearance", "must be \"dark\" or \"light\""),
            },
            "extends" => match value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                Some(base) => spec.extends = Some(base.to_owned()),
                None => report.push("extends", "must be the name of a theme"),
            },
            "palette" => spec.palette = palette(&value, &mut report),
            "ui" => flatten("", &value, &mut spec.colors, &mut report),
            "syntax" => spec.syntax = syntax(&value, &mut report),
            other => report.push(
                other,
                &format!("unknown field (known: {})", FIELDS.join(", ")),
            ),
        }
    }
    (spec, report.messages)
}

/// Reads `app.opt.colors`: a table with `ui` and `syntax` (and an
/// optional `palette`), applied over the chosen theme.
#[must_use]
pub fn parse_overrides(value: &Value) -> (ThemeSpec, Vec<String>) {
    let (spec, mut messages) = parse(value, "app.opt.colors");
    for (field, set) in [
        ("name", spec.name.is_some()),
        ("appearance", spec.appearance.is_some()),
        ("extends", spec.extends.is_some()),
    ] {
        if set {
            messages.push(format!(
                "app.opt.colors: {field} is ignored here; set it in a theme"
            ));
        }
    }
    (spec, messages)
}

struct Report<'a> {
    origin: &'a str,
    messages: Vec<String>,
}

impl Report<'_> {
    fn push(&mut self, path: &str, problem: &str) {
        if path.is_empty() {
            self.messages.push(format!("{}: {problem}", self.origin));
        } else {
            self.messages
                .push(format!("{}: {path}: {problem}", self.origin));
        }
    }
}

/// A table, as Lua tables arrive: an object, an array (a list, or `{}`).
fn as_table(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map.clone()),
        Value::Array(items) => Some(
            items
                .iter()
                .enumerate()
                .map(|(i, v)| ((i + 1).to_string(), v.clone()))
                .collect(),
        ),
        _ => None,
    }
}

fn palette(value: &Value, report: &mut Report<'_>) -> BTreeMap<String, Color> {
    let mut out = BTreeMap::new();
    let Some(table) = as_table(value) else {
        report.push("palette", "must be a table of colors");
        return out;
    };
    for (name, value) in table {
        let path = format!("palette.{name}");
        match value.as_str().map(Color::parse_hex) {
            Some(Ok(color)) => {
                out.insert(name, color);
            }
            Some(Err(err)) => report.push(&path, &err.to_string()),
            None => report.push(&path, "must be a color: #rrggbb or #rrggbbaa"),
        }
    }
    out
}

fn color_ref(value: &Value) -> Option<Result<ColorRef, String>> {
    let text = value.as_str()?.trim();
    Some(if text.starts_with('#') {
        Color::parse_hex(text)
            .map(ColorRef::Hex)
            .map_err(|err| err.to_string())
    } else if text.is_empty() {
        Err("an empty color".to_owned())
    } else {
        Ok(ColorRef::Name(text.to_owned()))
    })
}

/// Flattens `ui` into token paths: nested tables join with dots, and a
/// color on a path that is not a token goes to its `bg` (else its `fg`).
fn flatten(
    prefix: &str,
    value: &Value,
    out: &mut BTreeMap<String, ColorRef>,
    report: &mut Report<'_>,
) {
    let shown = |path: &str| format!("ui.{path}");
    if let Some(color) = color_ref(value) {
        let path = [
            prefix.to_owned(),
            format!("{prefix}.bg"),
            format!("{prefix}.fg"),
        ]
        .into_iter()
        .find(|path| token_spec(path).is_some());
        match (path, color) {
            (Some(path), Ok(color)) => {
                out.insert(path, color);
            }
            (Some(path), Err(problem)) => report.push(&shown(&path), &problem),
            (None, _) => report.push(&shown(prefix), "unknown token"),
        }
        return;
    }
    match as_table(value) {
        Some(table) if !prefix.is_empty() || !table.is_empty() => {
            for (key, value) in table {
                let path = if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&path, &value, out, report);
            }
        }
        Some(_) => {}
        None if prefix.is_empty() => report.push("ui", "must be a table"),
        None => report.push(&shown(prefix), "must be a color or a table"),
    }
}

fn syntax(value: &Value, report: &mut Report<'_>) -> BTreeMap<String, StyleRef> {
    let mut out = BTreeMap::new();
    let Some(table) = as_table(value) else {
        report.push("syntax", "must be a table");
        return out;
    };
    for (name, value) in table {
        let path = format!("syntax.{name}");
        if syntax_spec(&name).is_none() {
            report.push(&path, "unknown syntax name");
            continue;
        }
        if let Some(color) = color_ref(&value) {
            match color {
                Ok(color) => {
                    out.insert(
                        name,
                        StyleRef {
                            fg: Some(color),
                            ..StyleRef::default()
                        },
                    );
                }
                Err(problem) => report.push(&path, &problem),
            }
            continue;
        }
        let Some(fields) = as_table(&value) else {
            report.push(&path, "must be a color or { fg, italic, bold }");
            continue;
        };
        let mut style = StyleRef::default();
        for (field, value) in fields {
            let field_path = format!("{path}.{field}");
            match field.as_str() {
                "fg" => match color_ref(&value) {
                    Some(Ok(color)) => style.fg = Some(color),
                    Some(Err(problem)) => report.push(&field_path, &problem),
                    None => report.push(&field_path, "must be a color"),
                },
                "italic" | "bold" => match value.as_bool() {
                    Some(flag) if field == "italic" => style.italic = Some(flag),
                    Some(flag) => style.bold = Some(flag),
                    None => report.push(&field_path, "must be true or false"),
                },
                _ => report.push(
                    &field_path,
                    &format!("unknown field (known: {})", STYLE_FIELDS.join(", ")),
                ),
            }
        }
        out.insert(name, style);
    }
    out
}

/// Resolves a [`ColorRef`] against `palette`.
pub(super) fn lookup(color: &ColorRef, palette: &BTreeMap<String, Color>) -> Option<Color> {
    match color {
        ColorRef::Hex(color) => Some(*color),
        ColorRef::Name(name) => palette.get(name).copied(),
    }
}

/// `StyleSpec` of a [`StyleRef`], with its color resolved (`Err` names a
/// missing palette entry).
pub(super) fn style(
    style: &StyleRef,
    palette: &BTreeMap<String, Color>,
) -> Result<StyleSpec, String> {
    let fg = match &style.fg {
        None => None,
        Some(color) => Some(lookup(color, palette).ok_or_else(|| name_of(color))?),
    };
    Ok(StyleSpec {
        fg,
        italic: style.italic,
        bold: style.bold,
    })
}

pub(super) fn name_of(color: &ColorRef) -> String {
    match color {
        ColorRef::Hex(color) => color.to_hex(),
        ColorRef::Name(name) => name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn hex(text: &str) -> ColorRef {
        ColorRef::Hex(Color::parse_hex(text).unwrap())
    }

    #[test]
    fn flattens_ui_and_reads_shortcuts() {
        let (spec, messages) = parse(
            &json!({
                "name": "t",
                "appearance": "light",
                "palette": { "blue": "#7aa2f7" },
                "ui": {
                    "background": "#ffffff",
                    "accent": "blue",
                    "link": "#112233",
                    "list": { "hover": "#010203", "selected": { "border": "blue" } },
                    "chart": ["#aaaaaa", "blue"],
                },
                "syntax": {
                    "keyword": "blue",
                    "comment": { "fg": "#555555", "italic": true },
                },
            }),
            "t.lua",
        );
        assert!(messages.is_empty(), "{messages:?}");
        assert_eq!(spec.appearance, Some(Appearance::Light));
        assert_eq!(spec.colors["background"], hex("#ffffff"));
        assert_eq!(spec.colors["accent.bg"], ColorRef::Name("blue".into()));
        assert_eq!(spec.colors["link.fg"], hex("#112233"));
        assert_eq!(spec.colors["list.hover.bg"], hex("#010203"));
        assert_eq!(
            spec.colors["list.selected.border"],
            ColorRef::Name("blue".into())
        );
        assert_eq!(spec.colors["chart.1"], hex("#aaaaaa"));
        assert_eq!(spec.colors["chart.2"], ColorRef::Name("blue".into()));
        assert_eq!(spec.syntax["comment"].italic, Some(true));
        assert_eq!(
            spec.syntax["keyword"].fg,
            Some(ColorRef::Name("blue".into()))
        );
    }

    #[test]
    fn reports_each_problem_and_keeps_the_rest() {
        let (spec, messages) = parse(
            &json!({
                "appearance": "dim",
                "palette": { "bad": "blue", "ok": "#000000" },
                "ui": { "list": { "hovr": "#000000" }, "caret": "#12", "border": "#123456" },
                "syntax": { "kw": "#000000", "comment": { "italic": "yes", "under": true } },
                "colours": {},
            }),
            "t.lua",
        );
        assert_eq!(spec.colors.len(), 1);
        assert_eq!(spec.palette.len(), 1);
        let all = messages.join("\n");
        for expected in [
            "t.lua: appearance: must be \"dark\" or \"light\"",
            "t.lua: palette.bad: `blue` is not a color",
            "t.lua: ui.list.hovr: unknown token",
            "t.lua: ui.caret: `#12` is not a color",
            "t.lua: syntax.kw: unknown syntax name",
            "t.lua: syntax.comment.italic: must be true or false",
            "t.lua: syntax.comment.under: unknown field",
            "t.lua: colours: unknown field",
        ] {
            assert!(all.contains(expected), "missing {expected:?} in\n{all}");
        }
    }

    #[test]
    fn overrides_ignore_identity_fields() {
        let (spec, messages) =
            parse_overrides(&json!({ "name": "x", "ui": { "caret": "#ff0000" } }));
        assert_eq!(spec.colors["caret"], hex("#ff0000"));
        assert_eq!(
            messages,
            ["app.opt.colors: name is ignored here; set it in a theme"]
        );
    }

    #[test]
    fn empty_lua_tables_are_fine() {
        let (spec, messages) = parse(&json!({ "ui": [], "palette": [], "syntax": [] }), "t");
        assert!(messages.is_empty(), "{messages:?}");
        assert_eq!(spec, ThemeSpec::default());
    }
}
