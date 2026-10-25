//! The tree table widget: nested rows of a [`TreeSpec`] with typed
//! columns.
//!
//! Like the JSON tree ([`crate::blocks::json`]) it lays out plain rows
//! (so it nests in a scrolling block) and at most [`MAX_ROWS`] of them,
//! saying how many it left out. A row with children opens or closes on a
//! click; the toolbar expands or collapses everything. The label column
//! is indented by depth and carries the badge; text columns are
//! left-aligned, numbers and durations right-aligned in the mono font,
//! and bar columns draw a bar scaled to the column's largest value behind
//! the number.
//!
//! [`view`] gives the entity of an id and [`render`] the whole widget.

use std::collections::HashSet;
use std::rc::Rc;

use domain::block::BlockId;
use domain::widget::tree::{BadgeTone, ColumnKind, TreeSpec};
use gpui::{
    AnyElement, App, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, px, relative,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

/// Rows laid out at most.
pub const MAX_ROWS: usize = 2_000;
/// Indentation per level.
const INDENT: f32 = 14.0;
/// Width of a text column.
const TEXT_WIDTH: f32 = 160.0;
/// Width of a number, duration or bar column.
const NUMBER_WIDTH: f32 = 96.0;
/// Narrowest the label column gets.
const LABEL_MIN_WIDTH: f32 = 160.0;

/// One node, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Nesting level.
    pub depth: usize,
    /// Index just past its subtree, in depth-first order.
    pub end: usize,
    /// Label.
    pub label: SharedString,
    /// Badge and its tone.
    pub badge: Option<(SharedString, BadgeTone)>,
    /// Formatted value of each column.
    pub cells: Vec<SharedString>,
    /// Bar length (0..=1) of each bar column with a number.
    pub bars: Vec<Option<f32>>,
}

impl Row {
    /// Whether the node has children.
    pub fn has_children(&self, index: usize) -> bool {
        self.end > index + 1
    }
}

/// The rows of `spec` in depth-first order.
pub fn rows(spec: &TreeSpec) -> Vec<Row> {
    let maxima = spec.column_max();
    spec.flatten()
        .into_iter()
        .map(|flat| {
            let node = flat.node;
            let cells = spec
                .columns
                .iter()
                .map(|c| c.format(node.values.get(&c.name)).into())
                .collect();
            let bars = spec
                .columns
                .iter()
                .zip(&maxima)
                .map(|(column, &max)| {
                    let value = node.values.get(&column.name)?.as_f64()?;
                    (column.kind == ColumnKind::Bar && max > 0.0)
                        .then(|| (value.abs() / max).clamp(0.0, 1.0) as f32)
                })
                .collect();
            Row {
                depth: flat.depth,
                end: flat.end,
                label: node.label.clone().into(),
                badge: node
                    .badge
                    .clone()
                    .map(|b| (b.into(), node.tone.unwrap_or_default())),
                cells,
                bars,
            }
        })
        .collect()
}

/// Indices of the rows shown with `open` rows open, at most `limit`, and
/// how many more there would be.
pub fn visible(rows: &[Row], open: &HashSet<usize>, limit: usize) -> (Vec<usize>, usize) {
    let mut shown = Vec::new();
    let mut hidden = 0;
    let mut ix = 0;
    while ix < rows.len() {
        if shown.len() < limit {
            shown.push(ix);
        } else {
            hidden += 1;
        }
        ix = if open.contains(&ix) {
            ix + 1
        } else {
            rows[ix].end.max(ix + 1)
        };
    }
    (shown, hidden)
}

/// Per-id state: the spec, its rows and the open ones.
pub struct TreeView {
    spec: Rc<TreeSpec>,
    rows: Rc<Vec<Row>>,
    open: HashSet<usize>,
}

impl TreeView {
    fn new(spec: &TreeSpec) -> Self {
        let rows = rows(spec);
        // `expand_depth` levels are visible: rows above the last one open.
        let depth = spec.expand_depth as usize;
        let open = rows
            .iter()
            .enumerate()
            .filter(|(ix, row)| row.depth + 1 < depth && row.has_children(*ix))
            .map(|(ix, _)| ix)
            .collect();
        Self {
            spec: Rc::new(spec.clone()),
            rows: Rc::new(rows),
            open,
        }
    }

    fn sync(&mut self, spec: &TreeSpec, cx: &mut Context<Self>) {
        if *self.spec != *spec {
            *self = Self::new(spec);
            cx.notify();
        }
    }

    /// The spec drawn.
    pub fn spec(&self) -> &TreeSpec {
        &self.spec
    }

    /// Whether row `ix` (depth-first index) is open.
    pub fn is_open(&self, ix: usize) -> bool {
        self.open.contains(&ix)
    }

    /// Indices of the rows shown (up to [`MAX_ROWS`]).
    pub fn visible(&self) -> Vec<usize> {
        visible(&self.rows, &self.open, MAX_ROWS).0
    }

    /// Opens or closes row `ix`.
    pub fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.open.remove(&ix) && self.rows.get(ix).is_some_and(|r| r.has_children(ix)) {
            self.open.insert(ix);
        }
        cx.notify();
    }

    /// Opens every row with children.
    pub fn expand_all(&mut self, cx: &mut Context<Self>) {
        self.open = (0..self.rows.len())
            .filter(|&ix| self.rows[ix].has_children(ix))
            .collect();
        cx.notify();
    }

    /// Closes every row.
    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.open.clear();
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let title = self
            .spec
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("Tree");
        h_flex()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(title.to_owned())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} nodes", self.rows.len())),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .child(
                        super::tool_button("tree-expand-all", "expand all", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.expand_all(cx))),
                    )
                    .child(
                        super::tool_button("tree-collapse-all", "collapse all", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx))),
                    ),
            )
            .into_any_element()
    }

    fn tone_color(tone: BadgeTone, cx: &App) -> Hsla {
        let theme = cx.theme();
        match tone {
            BadgeTone::Neutral => theme.muted_foreground,
            BadgeTone::Info => theme.info,
            BadgeTone::Ok => theme.success,
            BadgeTone::Warning => theme.warning,
            BadgeTone::Error => theme.danger,
        }
    }

    fn column_width(kind: ColumnKind) -> f32 {
        if kind == ColumnKind::Text {
            TEXT_WIDTH
        } else {
            NUMBER_WIDTH
        }
    }
}

impl Render for TreeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (shown, hidden) = visible(&self.rows, &self.open, MAX_ROWS);
        let theme = cx.theme();
        let (muted, border, hover, mono) = (
            theme.muted_foreground,
            theme.border,
            theme.secondary_hover,
            theme.mono_font_family.clone(),
        );
        let tones = [
            BadgeTone::Neutral,
            BadgeTone::Info,
            BadgeTone::Ok,
            BadgeTone::Warning,
            BadgeTone::Error,
        ]
        .map(|tone| (tone, Self::tone_color(tone, cx)));
        let kinds: Vec<ColumnKind> = self.spec.columns.iter().map(|c| c.kind).collect();
        let bar_colors: Vec<Hsla> = (0..kinds.len())
            .map(|slot| super::palette(slot, cx).opacity(0.3))
            .collect();
        let head = h_flex()
            .gap_2()
            .px_1()
            .border_b_1()
            .border_color(border)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(muted)
            .child(div().flex_1().min_w(px(LABEL_MIN_WIDTH)).child("name"))
            .children(self.spec.columns.iter().map(|column| {
                div()
                    .flex_none()
                    .w(px(Self::column_width(column.kind)))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(column.kind != ColumnKind::Text, |d| d.text_right())
                    .child(SharedString::from(column.name.clone()))
            }));
        let lines = shown.into_iter().map(|ix| {
            let row = &self.rows[ix];
            let branch = row.has_children(ix);
            let toggle = match (branch, self.open.contains(&ix)) {
                (false, _) => " ",
                (true, true) => "▾",
                (true, false) => "▸",
            };
            let label = h_flex()
                .flex_1()
                .min_w(px(LABEL_MIN_WIDTH))
                .gap_1()
                .pl(px(INDENT * row.depth as f32))
                .overflow_hidden()
                .child(div().flex_none().w(px(10.)).text_color(muted).child(toggle))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(row.label.clone()),
                )
                .children(row.badge.as_ref().map(|(text, tone)| {
                    let color = tones
                        .iter()
                        .find(|(t, _)| t == tone)
                        .map_or(muted, |(_, c)| *c);
                    div()
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .text_xs()
                        .border_1()
                        .border_color(color.opacity(0.6))
                        .text_color(color)
                        .child(text.clone())
                }));
            let cells = row.cells.iter().enumerate().map(|(col, text)| {
                let kind = kinds.get(col).copied().unwrap_or_default();
                let cell = div()
                    .flex_none()
                    .relative()
                    .w(px(Self::column_width(kind)))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis();
                let cell = if kind == ColumnKind::Text {
                    cell.text_color(muted)
                } else {
                    cell.font_family(mono.clone()).text_right()
                };
                let bar = row.bars.get(col).copied().flatten().map(|length| {
                    div()
                        .absolute()
                        .top(px(2.))
                        .bottom(px(2.))
                        .left_0()
                        .w(relative(length))
                        .rounded_sm()
                        .bg(bar_colors.get(col).copied().unwrap_or(muted))
                });
                cell.children(bar)
                    .child(div().relative().child(text.clone()))
            });
            h_flex()
                .id(("tree-row", ix))
                .debug_selector(move || format!("tree-row-{ix}"))
                .gap_2()
                .px_1()
                .text_sm()
                .child(label)
                .children(cells)
                .when(branch, |line| {
                    line.cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle(ix, cx);
                        }))
                })
        });
        let lines: Vec<_> = lines.collect();
        let mut body = v_flex()
            .w_full()
            .debug_selector(|| "tree".to_owned())
            .gap_1()
            .child(self.header(cx))
            .child(head)
            .child(v_flex().children(lines));
        if hidden > 0 {
            body = body.child(div().text_xs().text_color(muted).child(SharedString::from(
                format!("… {hidden} more rows; collapse some to see them"),
            )));
        }
        if self.rows.is_empty() {
            body = body.child(div().text_xs().text_color(muted).child("no rows"));
        }
        body
    }
}

/// The tree entity of `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &TreeSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<TreeView> {
    let view = crate::blocks::block_state(id, window, cx, |_, _| TreeView::new(spec));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// The whole widget (header, column heads, rows) of `id`.
pub fn render(id: BlockId, spec: &TreeSpec, window: &mut Window, cx: &mut App) -> AnyElement {
    view(id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests;
