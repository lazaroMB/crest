//! Color themes: colors, the tokens a theme sets and how the missing ones
//! are derived.
//!
//! A theme is a set of named color **tokens** (`background`,
//! `list.hover.bg`, `input.focused.border`, ...) and **syntax** styles
//! (`keyword`, `string`, ...). A token path is `element[.state].attribute`:
//! the states are `hover`, `active` (pressed), `focused`, `selected` and
//! `disabled`, and the attributes `bg`, `fg` and `border`.
//!
//! Every token has a [`Rule`] that derives it from tokens listed before it
//! in [`TOKENS`], so a theme only needs the colors it cares about: the
//! three core tokens (`background`, `foreground`, `accent.bg`) and the
//! terminal colors fall back to the TwoCrest values of the appearance.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// An sRGB color with alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha: 0 is transparent, 255 opaque.
    pub a: u8,
}

/// A string that is not a `#rrggbb` or `#rrggbbaa` color.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a color: use #rrggbb or #rrggbbaa")]
pub struct BadColor(pub String);

impl Color {
    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    /// A color from its channels.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parses `#rrggbb` or `#rrggbbaa` (also `#rgb`), case-insensitive.
    ///
    /// # Errors
    ///
    /// [`BadColor`] for anything else.
    pub fn parse_hex(text: &str) -> Result<Self, BadColor> {
        let bad = || BadColor(text.to_owned());
        let hex = text.trim().strip_prefix('#').ok_or_else(bad)?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
        match hex.len() {
            3 => {
                let nibble = |i: usize| {
                    u8::from_str_radix(&hex[i..=i], 16)
                        .map(|n| n * 17)
                        .map_err(|_| bad())
                };
                Ok(Self::rgba(nibble(0)?, nibble(1)?, nibble(2)?, 255))
            }
            6 => Ok(Self::rgba(byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Ok(Self::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => Err(bad()),
        }
    }

    /// `#rrggbb`, or `#rrggbbaa` when it is not opaque.
    #[must_use]
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// The same color with opacity `alpha` (0.0 to 1.0).
    #[must_use]
    pub fn with_alpha(self, alpha: f32) -> Self {
        Self {
            a: unit_to_byte(alpha),
            ..self
        }
    }

    /// `self` moved towards `other` by `t` (0.0 is `self`, 1.0 is
    /// `other`), every channel alpha included.
    #[must_use]
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let channel = |a: u8, b: u8| {
            let value = f32::from(a) + (f32::from(b) - f32::from(a)) * t;
            // Clamped to 0..=255 above.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let value = value.round() as u8;
            value
        };
        Self::rgba(
            channel(self.r, other.r),
            channel(self.g, other.g),
            channel(self.b, other.b),
            channel(self.a, other.a),
        )
    }

    /// Towards white by `amount` (0.0 to 1.0).
    #[must_use]
    pub fn lighten(self, amount: f32) -> Self {
        self.mix(Self::rgba(255, 255, 255, self.a), amount)
    }

    /// Towards black by `amount` (0.0 to 1.0).
    #[must_use]
    pub fn darken(self, amount: f32) -> Self {
        self.mix(Self::rgba(0, 0, 0, self.a), amount)
    }

    /// Relative luminance (WCAG 2), ignoring alpha.
    #[must_use]
    pub fn luminance(self) -> f32 {
        let linear = |c: u8| {
            let c = f32::from(c) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }

    /// WCAG contrast ratio between two opaque colors, from 1 to 21.
    #[must_use]
    pub fn contrast_ratio(self, other: Self) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (light, dark) = if a > b { (a, b) } else { (b, a) };
        (light + 0.05) / (dark + 0.05)
    }

    /// `self` drawn over the opaque `below`.
    #[must_use]
    pub fn over(self, below: Self) -> Self {
        let t = f32::from(self.a) / 255.0;
        Self {
            a: 255,
            ..below.mix(Self { a: 255, ..self }, t)
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn unit_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Whether a theme is dark text on light or the other way round.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// Light text on a dark background.
    #[default]
    Dark,
    /// Dark text on a light background.
    Light,
}

impl Appearance {
    /// Its name in a theme: `dark` or `light`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// `dark` or `light`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

/// How a token gets its color when the theme does not set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule {
    /// The value of another token.
    Ref(&'static str),
    /// Another token with this opacity.
    Alpha(&'static str, f32),
    /// The first token moved towards the second by the fraction.
    Mix(&'static str, &'static str, f32),
    /// `background` or `foreground` of the theme, whichever reads better
    /// on the token (for text drawn on it).
    Contrast(&'static str),
    /// A fixed color for each appearance: `(dark, light)`, as hex.
    Fixed(&'static str, &'static str),
}

/// A color token: its path, how it is derived and what it paints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenSpec {
    /// Path, such as `list.hover.bg`.
    pub path: &'static str,
    /// How it is derived when the theme does not set it.
    pub rule: Rule,
    /// What it paints, for the docs and diagnostics.
    pub doc: &'static str,
}

const fn token(path: &'static str, rule: Rule, doc: &'static str) -> TokenSpec {
    TokenSpec { path, rule, doc }
}

use Rule::{Alpha, Contrast, Fixed, Mix, Ref};

/// Every color token, each after the tokens its rule reads.
pub const TOKENS: &[TokenSpec] = &[
    // Core.
    token("background", Fixed("#0b0b0c", "#f4f1ec"), "the window background"),
    token("foreground", Fixed("#f4f1ec", "#3d3a35"), "the default text"),
    token("accent.bg", Fixed("#d9604a", "#ae3a25"), "primary buttons, focus, highlights"),
    // Terminal colors, the base of the status and syntax defaults.
    token("ansi.red", Fixed("#f0524a", "#c0392b"), "red"),
    token("ansi.green", Fixed("#8fc27a", "#3f7d3a"), "green"),
    token("ansi.yellow", Fixed("#ffae42", "#b8740c"), "yellow"),
    token("ansi.blue", Fixed("#7aa2d6", "#3a6ea5"), "blue"),
    token("ansi.magenta", Fixed("#d9604a", "#ae3a25"), "magenta"),
    token("ansi.cyan", Fixed("#d6d2cb", "#3d3a35"), "cyan"),
    // Surfaces and text.
    token("border", Mix("background", "foreground", 0.15), "borders and rules"),
    token("muted.bg", Mix("background", "foreground", 0.06), "cards, chips, code"),
    token("muted.fg", Mix("foreground", "background", 0.42), "secondary text: hints, labels, timestamps"),
    token("selection", Alpha("accent.bg", 0.3), "selected text"),
    token("caret", Ref("accent.bg"), "the text cursor"),
    token("overlay", Alpha("background", 0.7), "the shade behind dialogs"),
    token("canvas.bg", Ref("background"), "the background of diagrams and 3D views"),
    // Accent (primary buttons).
    token("accent.fg", Contrast("accent.bg"), "text on the accent"),
    token("accent.hover.bg", Mix("accent.bg", "foreground", 0.15), "the accent under the mouse"),
    token("accent.active.bg", Mix("accent.bg", "background", 0.25), "the accent while pressed"),
    // Buttons (secondary).
    token("button.bg", Ref("muted.bg"), "buttons"),
    token("button.fg", Ref("foreground"), "button labels"),
    token("button.hover.bg", Mix("button.bg", "foreground", 0.08), "a button under the mouse; menu items too"),
    token("button.hover.fg", Ref("foreground"), "a button label under the mouse"),
    token("button.active.bg", Mix("button.bg", "foreground", 0.15), "a button while pressed"),
    token("button.disabled.fg", Alpha("foreground", 0.4), "the label of a disabled button"),
    // Inputs.
    token("input.bg", Ref("background"), "the prompt and text fields"),
    token("input.fg", Ref("foreground"), "typed text"),
    token("input.border", Ref("border"), "text field borders"),
    token("input.focused.border", Ref("accent.bg"), "the border (ring) of the focused field"),
    token("input.disabled.fg", Alpha("foreground", 0.4), "text of a disabled field and placeholders"),
    // Lists, tables and timeline rows.
    token("list.bg", Ref("background"), "lists and tables"),
    token("list.even.bg", Mix("background", "foreground", 0.02), "every other row"),
    token("list.header.bg", Mix("background", "foreground", 0.04), "table headers"),
    token("list.header.fg", Ref("foreground"), "table header text"),
    token("list.hover.bg", Ref("muted.bg"), "a row under the mouse"),
    token("list.selected.bg", Alpha("accent.bg", 0.15), "the selected row or timeline block"),
    token("list.selected.fg", Ref("foreground"), "text of the selected row"),
    token("list.selected.border", Ref("accent.bg"), "the edge of the selected row or block"),
    token("list.focused.border", Ref("accent.bg"), "the edge of the focused timeline block"),
    // Popovers and menus.
    token("popover.bg", Mix("background", "foreground", 0.03), "popups, menus, dialogs"),
    token("popover.fg", Ref("foreground"), "popup text"),
    token("popover.border", Ref("border"), "popup borders"),
    // Window chrome.
    token("title_bar.bg", Ref("background"), "the title bar"),
    token("title_bar.border", Ref("border"), "the title bar border"),
    token("status_bar.bg", Ref("popover.bg"), "the status bar"),
    token("status_bar.fg", Ref("muted.fg"), "status bar text"),
    token("status_bar.border", Ref("border"), "the status bar border"),
    token("sidebar.bg", Ref("popover.bg"), "the side panel"),
    token("sidebar.fg", Ref("foreground"), "side panel text"),
    token("sidebar.border", Ref("border"), "the side panel border"),
    token("sidebar.hover.bg", Ref("muted.bg"), "a side panel item under the mouse"),
    token("sidebar.hover.fg", Ref("foreground"), "its text"),
    token("sidebar.selected.bg", Ref("accent.bg"), "the selected side panel item"),
    token("sidebar.selected.fg", Ref("accent.fg"), "its text"),
    token("tab.bar.bg", Ref("popover.bg"), "the tab bar"),
    token("tab.bg", Ref("background"), "tabs"),
    token("tab.fg", Ref("muted.fg"), "tab labels"),
    token("tab.selected.bg", Ref("muted.bg"), "the current tab"),
    token("tab.selected.fg", Ref("foreground"), "the current tab label"),
    token("scrollbar.bg", Fixed("#00000000", "#00000000"), "the scrollbar track"),
    token("scrollbar.thumb.bg", Mix("border", "foreground", 0.1), "the scrollbar thumb"),
    token("scrollbar.thumb.hover.bg", Ref("muted.fg"), "the thumb under the mouse"),
    // Links.
    token("link.fg", Ref("accent.bg"), "links"),
    token("link.hover.fg", Ref("accent.hover.bg"), "a link under the mouse"),
    token("link.active.fg", Ref("accent.active.bg"), "a link while pressed"),
    // Status.
    token("info.bg", Ref("ansi.blue"), "information: notices, running tools"),
    token("info.fg", Contrast("info.bg"), "text on it"),
    token("success.bg", Ref("ansi.green"), "success: finished tools, approvals"),
    token("success.fg", Contrast("success.bg"), "text on it"),
    token("warning.bg", Ref("ansi.yellow"), "warnings"),
    token("warning.fg", Contrast("warning.bg"), "text on it"),
    token("error.bg", Ref("ansi.red"), "errors, failed tools, denials"),
    token("error.fg", Contrast("error.bg"), "text on it"),
    // Diffs.
    token("diff.added.fg", Ref("success.bg"), "added lines"),
    token("diff.added.bg", Alpha("diff.added.fg", 0.12), "the background of added lines"),
    token("diff.removed.fg", Ref("error.bg"), "removed lines"),
    token("diff.removed.bg", Alpha("diff.removed.fg", 0.12), "the background of removed lines"),
    token("diff.hunk.fg", Ref("info.bg"), "hunk headers (`@@ ... @@`)"),
    token("diff.header.fg", Ref("muted.fg"), "file headers"),
    // Code editor.
    token("editor.active_line.bg", Ref("muted.bg"), "the line of the cursor in code views"),
    token("editor.line_number.fg", Ref("muted.fg"), "line numbers"),
    token("editor.active_line_number.fg", Ref("foreground"), "the line number of the cursor"),
    // Charts.
    token("chart.1", Ref("accent.bg"), "the first series"),
    token("chart.2", Ref("warning.bg"), "the second series"),
    token("chart.3", Ref("info.bg"), "the third series"),
    token("chart.4", Ref("success.bg"), "the fourth series"),
    token("chart.5", Ref("ansi.magenta"), "the fifth series"),
    token("chart.grid", Ref("border"), "chart grid lines"),
];

/// How a syntax style is derived when the theme does not set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyntaxSpec {
    /// Highlight name, as tree-sitter queries use it (`keyword`,
    /// `string.escape`).
    pub name: &'static str,
    /// The color token it takes, when it has a color.
    pub color: Option<&'static str>,
    /// Whether it is italic.
    pub italic: bool,
    /// Whether it is bold.
    pub bold: bool,
}

const fn syntax(name: &'static str, color: &'static str) -> SyntaxSpec {
    SyntaxSpec {
        name,
        color: Some(color),
        italic: false,
        bold: false,
    }
}

const fn italic(spec: SyntaxSpec) -> SyntaxSpec {
    SyntaxSpec {
        italic: true,
        ..spec
    }
}

const fn bold(spec: SyntaxSpec) -> SyntaxSpec {
    SyntaxSpec { bold: true, ..spec }
}

/// Every syntax style. A name with a dot that a theme leaves out (and
/// whose default is the same as its prefix's) falls back to the prefix.
pub const SYNTAX: &[SyntaxSpec] = &[
    italic(syntax("comment", "muted.fg")),
    italic(syntax("comment.doc", "muted.fg")),
    syntax("keyword", "ansi.magenta"),
    syntax("string", "ansi.green"),
    syntax("string.escape", "ansi.cyan"),
    syntax("string.regex", "ansi.cyan"),
    syntax("string.special", "ansi.cyan"),
    syntax("string.special.symbol", "ansi.cyan"),
    syntax("number", "ansi.yellow"),
    syntax("boolean", "ansi.yellow"),
    syntax("constant", "ansi.yellow"),
    syntax("function", "ansi.blue"),
    syntax("constructor", "ansi.blue"),
    syntax("type", "ansi.cyan"),
    syntax("enum", "ansi.cyan"),
    syntax("variant", "ansi.cyan"),
    syntax("variable", "foreground"),
    syntax("variable.special", "ansi.red"),
    syntax("property", "foreground"),
    syntax("attribute", "ansi.yellow"),
    syntax("label", "ansi.blue"),
    syntax("tag", "ansi.red"),
    syntax("operator", "foreground"),
    syntax("punctuation", "muted.fg"),
    syntax("punctuation.bracket", "muted.fg"),
    syntax("punctuation.delimiter", "muted.fg"),
    syntax("punctuation.special", "ansi.magenta"),
    syntax("punctuation.list_marker", "accent.bg"),
    syntax("preproc", "ansi.magenta"),
    syntax("embedded", "foreground"),
    bold(syntax("title", "accent.bg")),
    syntax("link_text", "link.fg"),
    italic(syntax("link_uri", "muted.fg")),
    syntax("text.literal", "ansi.green"),
    syntax("text.code.span", "ansi.green"),
    SyntaxSpec {
        name: "emphasis",
        color: None,
        italic: true,
        bold: false,
    },
    SyntaxSpec {
        name: "emphasis.strong",
        color: None,
        italic: false,
        bold: true,
    },
];

/// The spec of a token path, if crest knows it.
#[must_use]
pub fn token_spec(path: &str) -> Option<&'static TokenSpec> {
    TOKENS.iter().find(|spec| spec.path == path)
}

/// The spec of a syntax name, if crest knows it.
#[must_use]
pub fn syntax_spec(name: &str) -> Option<&'static SyntaxSpec> {
    SYNTAX.iter().find(|spec| spec.name == name)
}

/// A syntax style as a theme writes it: every field optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSpec {
    /// The color.
    pub fg: Option<Color>,
    /// Italic.
    pub italic: Option<bool>,
    /// Bold.
    pub bold: Option<bool>,
}

/// A resolved syntax style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStyle {
    /// The color; `None` keeps the color of the surrounding text.
    pub fg: Option<Color>,
    /// Italic.
    pub italic: bool,
    /// Bold.
    pub bold: bool,
}

/// A theme with every token and syntax style set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    /// Its name.
    pub name: String,
    /// Dark or light.
    pub appearance: Appearance,
    colors: BTreeMap<String, Color>,
    syntax: BTreeMap<String, TextStyle>,
}

impl Theme {
    /// Builds a theme from the colors and syntax styles it sets, deriving
    /// the rest. Paths and names crest does not know are ignored (the
    /// caller reports them).
    #[must_use]
    pub fn derive(
        name: impl Into<String>,
        appearance: Appearance,
        set: &BTreeMap<String, Color>,
        set_syntax: &BTreeMap<String, StyleSpec>,
    ) -> Self {
        let mut colors: BTreeMap<String, Color> = BTreeMap::new();
        for spec in TOKENS {
            let color = set
                .get(spec.path)
                .copied()
                .unwrap_or_else(|| apply(spec.rule, appearance, &colors));
            colors.insert(spec.path.to_owned(), color);
        }
        let mut syntax = BTreeMap::new();
        for spec in SYNTAX {
            let given = set_syntax.get(spec.name).copied().unwrap_or_default();
            // A dotted name the theme leaves out follows its prefix when
            // the theme set the prefix (`string.escape` after `string`).
            let inherited = spec
                .name
                .rsplit_once('.')
                .filter(|_| !set_syntax.contains_key(spec.name))
                .and_then(|(prefix, _)| set_syntax.get(prefix).copied());
            let given = inherited.map_or(given, |parent| StyleSpec {
                fg: given.fg.or(parent.fg),
                italic: given.italic.or(parent.italic),
                bold: given.bold.or(parent.bold),
            });
            let style = TextStyle {
                fg: given
                    .fg
                    .or_else(|| spec.color.and_then(|token| colors.get(token).copied())),
                italic: given.italic.unwrap_or(spec.italic),
                bold: given.bold.unwrap_or(spec.bold),
            };
            syntax.insert(spec.name.to_owned(), style);
        }
        Self {
            name: name.into(),
            appearance,
            colors,
            syntax,
        }
    }

    /// The color of a token from [`TOKENS`]; transparent for an unknown
    /// path (a bug: every known token is set).
    #[must_use]
    pub fn color(&self, path: &str) -> Color {
        debug_assert!(token_spec(path).is_some(), "unknown theme token {path}");
        self.colors.get(path).copied().unwrap_or(Color::TRANSPARENT)
    }

    /// The style of a syntax name from [`SYNTAX`].
    #[must_use]
    pub fn syntax(&self, name: &str) -> Option<TextStyle> {
        self.syntax.get(name).copied()
    }

    /// Every token and its color, in [`TOKENS`] order.
    pub fn colors(&self) -> impl Iterator<Item = (&'static str, Color)> + '_ {
        TOKENS.iter().map(|spec| (spec.path, self.color(spec.path)))
    }

    /// Every syntax style, in [`SYNTAX`] order.
    pub fn syntax_styles(&self) -> impl Iterator<Item = (&'static str, TextStyle)> + '_ {
        SYNTAX.iter().filter_map(|spec| Some((spec.name, self.syntax(spec.name)?)))
    }
}

fn apply(rule: Rule, appearance: Appearance, colors: &BTreeMap<String, Color>) -> Color {
    let get = |path: &str| colors.get(path).copied().unwrap_or(Color::TRANSPARENT);
    match rule {
        Rule::Ref(path) => get(path),
        Rule::Alpha(path, alpha) => {
            let color = get(path);
            color.with_alpha(alpha * f32::from(color.a) / 255.0)
        }
        Rule::Mix(from, to, t) => get(from).mix(get(to), t),
        Rule::Contrast(path) => {
            let below = get(path).over(get("background"));
            let (bg, fg) = (get("background"), get("foreground"));
            if below.contrast_ratio(bg) >= below.contrast_ratio(fg) {
                bg
            } else {
                fg
            }
        }
        Rule::Fixed(dark, light) => {
            let hex = match appearance {
                Appearance::Dark => dark,
                Appearance::Light => light,
            };
            Color::parse_hex(hex).unwrap_or(Color::TRANSPARENT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Color {
        Color::parse_hex(text).unwrap()
    }

    #[test]
    fn parses_and_prints_hex() {
        assert_eq!(hex("#d9604a"), Color::rgba(0xd9, 0x60, 0x4a, 255));
        assert_eq!(hex("#D9604A59").a, 0x59);
        assert_eq!(hex("#fff"), Color::rgba(255, 255, 255, 255));
        assert_eq!(hex("#d9604a").to_hex(), "#d9604a");
        assert_eq!(hex("#d9604a59").to_hex(), "#d9604a59");
        for bad in ["d9604a", "#d9604", "#zzzzzz", "", "#", "blue"] {
            assert!(Color::parse_hex(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mixes_and_fades() {
        let black = hex("#000000");
        let white = hex("#ffffff");
        assert_eq!(black.mix(white, 0.5), hex("#808080"));
        assert_eq!(black.mix(white, 0.0), black);
        assert_eq!(black.mix(white, 2.0), white);
        assert_eq!(white.with_alpha(0.5).a, 128);
        assert_eq!(black.lighten(1.0), white);
        assert_eq!(white.darken(1.0), black);
        assert_eq!(white.with_alpha(0.5).over(black), hex("#808080"));
    }

    #[test]
    fn contrast_ratio_matches_wcag() {
        let ratio = hex("#000000").contrast_ratio(hex("#ffffff"));
        assert!((ratio - 21.0).abs() < 0.01, "{ratio}");
        assert!((hex("#777777").contrast_ratio(hex("#777777")) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn every_rule_reads_tokens_listed_before_it() {
        let mut seen: Vec<&str> = Vec::new();
        for spec in TOKENS {
            let reads: &[&str] = match spec.rule {
                Rule::Ref(a) | Rule::Alpha(a, _) => &[a],
                Rule::Mix(a, b, _) => &[a, b],
                Rule::Contrast(a) => &[a, "background", "foreground"],
                Rule::Fixed(dark, light) => {
                    assert!(Color::parse_hex(dark).is_ok() && Color::parse_hex(light).is_ok());
                    &[]
                }
            };
            for read in reads {
                assert!(seen.contains(read), "{} reads {read} before it is set", spec.path);
            }
            assert!(!seen.contains(&spec.path), "{} listed twice", spec.path);
            seen.push(spec.path);
        }
        for spec in SYNTAX {
            if let Some(color) = spec.color {
                assert!(token_spec(color).is_some(), "{} uses {color}", spec.name);
            }
        }
    }

    #[test]
    fn derives_what_is_not_set() {
        let set = BTreeMap::from([
            ("background".to_owned(), hex("#1a1b26")),
            ("foreground".to_owned(), hex("#c0caf5")),
            ("accent.bg".to_owned(), hex("#7aa2f7")),
            ("list.hover.bg".to_owned(), hex("#292e42")),
        ]);
        let syntax = BTreeMap::from([(
            "string".to_owned(),
            StyleSpec {
                fg: Some(hex("#9ece6a")),
                italic: Some(true),
                bold: None,
            },
        )]);
        let theme = Theme::derive("t", Appearance::Dark, &set, &syntax);
        assert_eq!(theme.color("list.hover.bg"), hex("#292e42"));
        assert_eq!(theme.color("caret"), hex("#7aa2f7"));
        assert_eq!(theme.color("selection"), hex("#7aa2f7").with_alpha(0.3));
        // Dark text reads better on a light blue accent.
        assert_eq!(theme.color("accent.fg"), hex("#1a1b26"));
        assert_eq!(theme.color("link.fg"), hex("#7aa2f7"));
        let escape = theme.syntax("string.escape").unwrap();
        assert_eq!(escape.fg, Some(hex("#9ece6a")));
        assert!(escape.italic);
        assert_eq!(theme.syntax("comment").unwrap().fg, Some(theme.color("muted.fg")));
        assert_eq!(theme.colors().count(), TOKENS.len());
    }

    #[test]
    fn light_defaults_differ() {
        let empty = BTreeMap::new();
        let dark = Theme::derive("d", Appearance::Dark, &empty, &BTreeMap::new());
        let light = Theme::derive("l", Appearance::Light, &empty, &BTreeMap::new());
        assert!(dark.color("background").luminance() < light.color("background").luminance());
        assert!(dark.color("foreground").contrast_ratio(dark.color("background")) > 4.5);
        assert!(light.color("foreground").contrast_ratio(light.color("background")) > 4.5);
    }
}
