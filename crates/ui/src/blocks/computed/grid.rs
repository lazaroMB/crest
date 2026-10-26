//! A small results table for the computed cards: a label column on the
//! left and right-aligned numeric columns in the monospace font, so the
//! digits line up (tabular figures). Cells may carry their own color, for
//! a significant p or a heatmap cell.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

/// Width of a numeric column.
pub const NUMBER_WIDTH: f32 = 88.0;
/// Narrowest the label column gets.
const LABEL_MIN_WIDTH: f32 = 120.0;

/// One cell: its text and an optional background and text color.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridCell {
    /// What it shows.
    pub text: String,
    /// Background, e.g. a heatmap color.
    pub bg: Option<Hsla>,
    /// Text color; the theme's foreground when `None`.
    pub fg: Option<Hsla>,
}

impl From<String> for GridCell {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
}

impl From<&str> for GridCell {
    fn from(text: &str) -> Self {
        Self::from(text.to_owned())
    }
}

/// A table with a header row; the first column is the label, the others
/// are numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Grid {
    /// Column titles, the label column's first.
    pub headers: Vec<String>,
    /// Rows of cells, as many as there are headers.
    pub rows: Vec<Vec<GridCell>>,
}

impl Grid {
    /// An empty grid with `headers`.
    #[must_use]
    pub fn new<S: Into<String>>(headers: impl IntoIterator<Item = S>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    /// Adds a row.
    pub fn push<C: Into<GridCell>>(&mut self, cells: impl IntoIterator<Item = C>) {
        self.rows.push(cells.into_iter().map(Into::into).collect());
    }

    /// Whether it has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Draws the grid; `id` keys its horizontal scroll.
    pub fn render(&self, id: impl Into<SharedString>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let cell = |col: usize| {
            let base = div().px_2().py_0p5().overflow_hidden().whitespace_nowrap();
            if col == 0 {
                base.flex_1().min_w(px(LABEL_MIN_WIDTH))
            } else {
                base.w(px(NUMBER_WIDTH))
                    .flex_none()
                    .text_right()
                    .font_family(mono.clone())
            }
        };
        let header = h_flex()
            .w_full()
            .bg(theme.table_head)
            .text_color(theme.table_head_foreground)
            .font_weight(FontWeight::SEMIBOLD)
            .border_b_1()
            .border_color(theme.border)
            .children(self.headers.iter().enumerate().map(|(col, title)| {
                cell(col)
                    .font_family(theme.font_family.clone())
                    .child(SharedString::from(title.clone()))
            }));
        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            h_flex()
                .w_full()
                .when(ix % 2 == 1, |this| this.bg(theme.table_even))
                .children(row.iter().enumerate().map(|(col, c)| {
                    cell(col)
                        .when_some(c.bg, |this, bg| this.bg(bg))
                        .text_color(c.fg.unwrap_or(theme.foreground))
                        .child(SharedString::from(c.text.clone()))
                }))
        });
        #[allow(clippy::cast_precision_loss)]
        let min_width =
            LABEL_MIN_WIDTH + NUMBER_WIDTH * self.headers.len().saturating_sub(1) as f32;
        div()
            .id(ElementId::Name(id.into()))
            .w_full()
            .overflow_x_scroll()
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .text_sm()
            .child(
                v_flex()
                    .w_full()
                    .min_w(px(min_width))
                    .child(header)
                    .children(rows),
            )
            .into_any_element()
    }
}
