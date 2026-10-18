//! The JSON node of `Custom` blocks and modals: a collapsible tree.
//!
//! Objects and arrays are rows with a toggle (click to open or close) and
//! their size; scalars are colored by type. The first `expand_depth` levels
//! start open. The view lays out at most [`MAX_ROWS`] rows and says how
//! many it left out, so a huge document stays cheap; open fewer branches
//! to see the rest. It is drawn with plain rows (not a virtualized list)
//! so it nests inside a scrolling modal or block.

use std::collections::HashSet;

use domain::block::BlockId;
use gpui::{
    App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use serde_json::Value;

/// Rows laid out at most.
pub const MAX_ROWS: usize = 2_000;
/// Characters of a string shown before it is cut.
const MAX_STRING: usize = 400;
/// Indentation per level, in pixels.
const INDENT: f32 = 14.0;

/// What a row shows.
#[derive(Debug, Clone, PartialEq)]
pub enum RowKind {
    /// An object or array: whether it is open, its size, and its brackets.
    Branch {
        /// Open (children follow).
        open: bool,
        /// Number of entries.
        len: usize,
        /// `true` for an array.
        array: bool,
    },
    /// A scalar, already formatted.
    Leaf {
        /// Formatted value.
        text: String,
        /// Which color it gets.
        tone: Tone,
    },
}

/// Color class of a scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// A string.
    String,
    /// A number.
    Number,
    /// `true` or `false`.
    Bool,
    /// `null`.
    Null,
}

/// One visible line of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Nesting level, 0 for the root.
    pub depth: usize,
    /// Key in its parent (`"name"` or `[3]`), none for the root.
    pub key: Option<String>,
    /// JSON pointer of the value, the toggle's identity.
    pub path: String,
    /// What it shows.
    pub kind: RowKind,
}

/// The paths open at first: every branch above `depth`.
pub fn initially_open(value: &Value, depth: u32) -> HashSet<String> {
    fn walk(value: &Value, path: String, level: u32, depth: u32, open: &mut HashSet<String>) {
        if level >= depth {
            return;
        }
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    walk(
                        child,
                        format!("{path}/{}", escape(key)),
                        level + 1,
                        depth,
                        open,
                    );
                }
            }
            Value::Array(items) => {
                for (ix, child) in items.iter().enumerate() {
                    walk(child, format!("{path}/{ix}"), level + 1, depth, open);
                }
            }
            _ => return,
        }
        open.insert(path);
    }
    let mut open = HashSet::new();
    walk(value, String::new(), 0, depth, &mut open);
    open
}

/// A key as a JSON pointer segment.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn leaf(value: &Value) -> RowKind {
    let (text, tone) = match value {
        Value::Null => ("null".to_owned(), Tone::Null),
        Value::Bool(b) => (b.to_string(), Tone::Bool),
        Value::Number(n) => (n.to_string(), Tone::Number),
        Value::String(s) => {
            let mut shown: String = s.chars().take(MAX_STRING).collect();
            if s.chars().count() > MAX_STRING {
                shown.push('…');
            }
            (format!("{shown:?}"), Tone::String)
        }
        Value::Array(_) | Value::Object(_) => (String::new(), Tone::Null),
    };
    RowKind::Leaf { text, tone }
}

/// The visible rows of `value` with `open` branches, at most `limit`, and
/// how many more there would be.
pub fn rows(value: &Value, open: &HashSet<String>, limit: usize) -> (Vec<Row>, usize) {
    struct Walk<'a> {
        open: &'a HashSet<String>,
        limit: usize,
        rows: Vec<Row>,
        hidden: usize,
    }
    impl Walk<'_> {
        fn push(&mut self, row: Row) {
            if self.rows.len() < self.limit {
                self.rows.push(row);
            } else {
                self.hidden += 1;
            }
        }
        fn visit(&mut self, value: &Value, key: Option<String>, path: String, depth: usize) {
            let children: Vec<(String, String, &Value)> = match value {
                Value::Object(map) => map
                    .iter()
                    .map(|(k, v)| (k.clone(), format!("{path}/{}", escape(k)), v))
                    .collect(),
                Value::Array(items) => items
                    .iter()
                    .enumerate()
                    .map(|(ix, v)| (format!("[{ix}]"), format!("{path}/{ix}"), v))
                    .collect(),
                scalar => {
                    self.push(Row {
                        depth,
                        key,
                        path,
                        kind: leaf(scalar),
                    });
                    return;
                }
            };
            let is_open = self.open.contains(&path);
            self.push(Row {
                depth,
                key,
                path,
                kind: RowKind::Branch {
                    open: is_open,
                    len: children.len(),
                    array: value.is_array(),
                },
            });
            if is_open {
                for (key, path, child) in children {
                    self.visit(child, Some(key), path, depth + 1);
                }
            }
        }
    }
    let mut walk = Walk {
        open,
        limit,
        rows: Vec::new(),
        hidden: 0,
    };
    walk.visit(value, None, String::new(), 0);
    (walk.rows, walk.hidden)
}

/// Per-node state: the value and the open branches.
pub struct JsonView {
    value: Value,
    depth: u32,
    open: HashSet<String>,
}

impl JsonView {
    fn new(value: &Value, depth: u32) -> Self {
        Self {
            value: value.clone(),
            depth,
            open: initially_open(value, depth),
        }
    }

    /// Opens or closes the branch at `path`.
    pub fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.open.remove(path) {
            self.open.insert(path.to_owned());
        }
        cx.notify();
    }

    /// Whether the branch at `path` is open.
    pub fn is_open(&self, path: &str) -> bool {
        self.open.contains(path)
    }

    fn tone_color(tone: Tone, cx: &App) -> Hsla {
        let theme = cx.theme();
        match tone {
            Tone::String => theme.success,
            Tone::Number => theme.info,
            Tone::Bool => theme.warning,
            Tone::Null => theme.muted_foreground,
        }
    }
}

impl Render for JsonView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (rows, hidden) = rows(&self.value, &self.open, MAX_ROWS);
        let theme = cx.theme();
        let (muted, key_color, mono) = (
            theme.muted_foreground,
            theme.foreground,
            theme.mono_font_family.clone(),
        );
        let lines = rows.into_iter().enumerate().map(|(ix, row)| {
            let mut line = h_flex()
                .id(("json-row", ix))
                .gap_1()
                .pl(px(INDENT * row.depth as f32))
                .whitespace_nowrap();
            let toggle = match &row.kind {
                RowKind::Branch { open, .. } => {
                    if *open {
                        "▾"
                    } else {
                        "▸"
                    }
                }
                RowKind::Leaf { .. } => " ",
            };
            line = line.child(div().w(px(10.)).text_color(muted).child(toggle));
            if let Some(key) = &row.key {
                line = line.child(
                    div()
                        .text_color(key_color)
                        .child(SharedString::from(format!("{key}:"))),
                );
            }
            match &row.kind {
                RowKind::Branch { open, len, array } => {
                    let (l, r, unit) = if *array {
                        ("[", "]", if *len == 1 { "item" } else { "items" })
                    } else {
                        ("{", "}", if *len == 1 { "key" } else { "keys" })
                    };
                    let summary = if *open {
                        format!("{l} {len} {unit}")
                    } else {
                        format!("{l}…{r} {len} {unit}")
                    };
                    let path = row.path.clone();
                    line.child(div().text_color(muted).child(SharedString::from(summary)))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.secondary_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle(&path, cx);
                        }))
                }
                RowKind::Leaf { text, tone } => line.child(
                    div()
                        .text_color(Self::tone_color(*tone, cx))
                        .child(SharedString::from(text.clone())),
                ),
            }
        });
        let mut body = v_flex().font_family(mono).text_sm().children(lines);
        if hidden > 0 {
            body = body.child(div().text_color(muted).child(SharedString::from(format!(
                "… {hidden} more rows; close some branches to see them"
            ))));
        }
        body
    }
}

/// The view of the JSON node `id`, in sync with `value`.
pub fn json_view(
    id: BlockId,
    value: &Value,
    depth: u32,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<JsonView> {
    let view = super::block_state(id, window, cx, |_, _| JsonView::new(value, depth));
    view.update(cx, |view, cx| {
        if &view.value != value || view.depth != depth {
            *view = JsonView::new(value, depth);
            cx.notify();
        }
    });
    view
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sample() -> Value {
        json!({ "name": "crest", "tags": ["a", "b"], "meta": { "ok": true, "n": 3, "x": null } })
    }

    fn summary(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let key = row.key.clone().unwrap_or_else(|| "$".into());
                let kind = match &row.kind {
                    RowKind::Branch { open, len, .. } => {
                        format!("{}{len}", if *open { "-" } else { "+" })
                    }
                    RowKind::Leaf { text, .. } => text.clone(),
                };
                format!("{}{key}={kind}", "  ".repeat(row.depth))
            })
            .collect()
    }

    #[test]
    fn the_first_levels_start_open() {
        let value = sample();
        assert!(initially_open(&value, 0).is_empty());
        assert_eq!(initially_open(&value, 1), HashSet::from([String::new()]));
        let two = initially_open(&value, 2);
        assert!(two.contains("") && two.contains("/tags") && two.contains("/meta"));
        let (rows, hidden) = rows(&value, &initially_open(&value, 1), MAX_ROWS);
        assert_eq!(hidden, 0);
        assert_eq!(
            summary(&rows),
            ["$=-3", "  name=\"crest\"", "  tags=+2", "  meta=+3"]
        );
    }

    #[test]
    fn opening_a_branch_shows_its_children_typed() {
        let value = sample();
        let mut open = initially_open(&value, 1);
        open.insert("/meta".into());
        let (rows, _) = rows(&value, &open, MAX_ROWS);
        assert_eq!(
            summary(&rows),
            [
                "$=-3",
                "  name=\"crest\"",
                "  tags=+2",
                "  meta=-3",
                "    ok=true",
                "    n=3",
                "    x=null"
            ]
        );
        let tones: Vec<_> = rows
            .iter()
            .filter_map(|r| match r.kind {
                RowKind::Leaf { tone, .. } => Some(tone),
                RowKind::Branch { .. } => None,
            })
            .collect();
        assert_eq!(tones, [Tone::String, Tone::Bool, Tone::Number, Tone::Null]);
    }

    #[test]
    fn rows_are_capped_and_keys_escaped() {
        let big = Value::Array((0..50).map(Value::from).collect());
        let (rows, hidden) = rows(&big, &initially_open(&big, 1), 10);
        assert_eq!((rows.len(), hidden), (10, 41));
        let odd = json!({ "a/b": { "c": 1 } });
        assert!(initially_open(&odd, 2).contains("/a~1b"));
    }
}
