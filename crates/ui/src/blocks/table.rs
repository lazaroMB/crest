//! Renderer of the `table` block kind: a sortable table whose cells wrap.
//!
//! Each block keeps its [`TableView`] across frames (see
//! [`super::block_state`]), so the sort order, the horizontal scroll and
//! whether every row is shown survive scrolling the timeline. Columns get
//! a width from their contents ([`column_sizes`]) and share the block's
//! width in proportion to it; text cells wrap, so rows have their own
//! heights. The table shows [`COLLAPSED_ROWS`] rows until expanded, and at
//! most [`MAX_RENDERED_ROWS`] after. Clicking a header cycles descending →
//! ascending → original order ([`sort`]), over every row. [`render_spec`]
//! is shared with the chart block's table view.

pub mod sort;

use domain::block::{Align, BlockId, BlockKind, Cell, TableSpec};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use self::sort::Direction;
use super::chart::paint::LabelFont;
use crate::timeline::TimelineEntry;

/// Rows shown until the table is expanded.
pub const COLLAPSED_ROWS: usize = 12;
/// Rows drawn at most once expanded; `y` copies all of them.
pub const MAX_RENDERED_ROWS: usize = 500;
/// Rows sampled to size a column.
const WIDTH_SAMPLE: usize = 200;
/// Horizontal padding of a cell (`px_2` on both sides), plus a little room.
const CELL_PADDING: f32 = 20.0;
const MIN_COL_WIDTH: f32 = 56.0;
/// Widest a column asks to be; wider text wraps.
const MAX_IDEAL_WIDTH: f32 = 480.0;
/// Widest a column's minimum gets; longer words break.
const MAX_MIN_WIDTH: f32 = 200.0;
/// Lines a cell shows before its text is cut.
const MAX_CELL_LINES: usize = 8;

/// Text shown for a cell; nulls are blank.
pub fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Null => String::new(),
        Cell::Bool(b) => b.to_string(),
        Cell::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
        Cell::Number(n) => n.to_string(),
        Cell::Text(t) => t.clone(),
    }
}

/// Whether column `col` holds numbers: every non-null cell is one, and
/// there is at least one.
fn is_numeric(spec: &TableSpec, col: usize) -> bool {
    let mut cells = spec.rows.iter().filter_map(|row| row.get(col));
    let numeric = cells.all(|c| matches!(c, Cell::Number(_) | Cell::Null));
    numeric
        && spec
            .rows
            .iter()
            .any(|row| matches!(row.get(col), Some(Cell::Number(_))))
}

/// Alignment of column `col`: the declared one, else right when every
/// non-null cell is a number.
pub fn column_align(spec: &TableSpec, col: usize) -> Align {
    if let Some(align) = spec.columns.get(col).and_then(|c| c.align) {
        return align;
    }
    if is_numeric(spec, col) {
        Align::Right
    } else {
        Align::Left
    }
}

/// How wide a column wants to be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColSize {
    /// Narrowest it gets before the table scrolls sideways.
    pub min: f32,
    /// Width it asks for; columns share the table's width in proportion
    /// to it.
    pub ideal: f32,
    /// Numbers, which never wrap.
    pub numeric: bool,
}

/// The longest word of `text` by characters.
fn longest_word(text: &str) -> &str {
    text.split_whitespace()
        .max_by_key(|word| word.chars().count())
        .unwrap_or("")
}

/// Width of the widest line of `text`.
fn widest_line(text: &str, measure: &dyn Fn(&str) -> f32) -> f32 {
    text.lines().map(measure).fold(0.0, f32::max)
}

/// Size of each column of `spec`, from its header and a sample of its
/// cells, with text widths from `measure`. The ideal width is the widest
/// of them; the minimum is the longest word, or the ideal for numbers.
pub fn column_sizes(spec: &TableSpec, measure: &dyn Fn(&str) -> f32) -> Vec<ColSize> {
    (0..spec.columns.len())
        .map(|col| {
            // Room for the sort indicator next to the name.
            let header = format!("{} ▲", spec.columns[col].name);
            let cells: Vec<String> = spec
                .rows
                .iter()
                .take(WIDTH_SAMPLE)
                .filter_map(|row| row.get(col))
                .map(cell_text)
                .collect();
            let widest = cells
                .iter()
                .map(|text| widest_line(text, measure))
                .fold(widest_line(&header, measure), f32::max);
            let ideal = (widest + CELL_PADDING).clamp(MIN_COL_WIDTH, MAX_IDEAL_WIDTH);
            let numeric = is_numeric(spec, col);
            let min = if numeric {
                ideal
            } else {
                let word = cells
                    .iter()
                    .map(|text| measure(longest_word(text)))
                    .fold(measure(longest_word(&header)), f32::max);
                (word + CELL_PADDING)
                    .clamp(MIN_COL_WIDTH, MAX_MIN_WIDTH)
                    .min(ideal)
            };
            ColSize {
                min,
                ideal,
                numeric,
            }
        })
        .collect()
}

/// The rows of a [`TableSpec`] in the current sort order.
pub struct SpecTable {
    spec: TableSpec,
    /// Index into `spec.rows` of each displayed row.
    order: Vec<usize>,
    sort: Option<(usize, Direction)>,
}

impl SpecTable {
    /// `spec` in its original order.
    pub fn new(spec: TableSpec) -> Self {
        let order = (0..spec.rows.len()).collect();
        Self {
            spec,
            order,
            sort: None,
        }
    }

    /// The data shown.
    pub fn spec(&self) -> &TableSpec {
        &self.spec
    }

    /// Index into the spec rows of each displayed row, top to bottom.
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// The column sorted by and its direction, or `None` in the original
    /// order.
    pub fn sorting(&self) -> Option<(usize, Direction)> {
        self.sort
    }

    /// Sorts the rows by a column (`None` restores the original order).
    pub fn set_sort(&mut self, sort: Option<(usize, Direction)>) {
        self.sort = sort;
        self.order = match sort {
            None => (0..self.spec.rows.len()).collect(),
            Some((col, direction)) => sort::sorted_order(&self.spec.rows, col, direction),
        };
    }

    /// The cell of displayed row `row` in column `col`.
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        let row = *self.order.get(row)?;
        self.spec.rows.get(row)?.get(col)
    }
}

// The `DataTable` adapter that callers still use; it goes once they draw
// a `TableView`.

impl SpecTable {
    /// Sorts the rows by `col` (`ColumnSort::Default` restores the original
    /// order).
    pub fn sort(&mut self, col: usize, sort: ColumnSort) {
        self.set_sort(match sort {
            ColumnSort::Default => None,
            ColumnSort::Ascending => Some((col, Direction::Ascending)),
            ColumnSort::Descending => Some((col, Direction::Descending)),
        });
    }
}

impl TableDelegate for SpecTable {
    fn columns_count(&self, _: &App) -> usize {
        self.spec.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let name = self
            .spec
            .columns
            .get(col_ix)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        Column::new(SharedString::from(format!("c{col_ix}")), name).sortable()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        self.sort(col_ix, sort);
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self.cell(row_ix, col_ix).map(cell_text).unwrap_or_default();
        div().child(SharedString::from(text))
    }
}

/// The `DataTable` state of block `id`, showing `spec`.
pub fn table_state(
    id: BlockId,
    spec: &TableSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<TableState<SpecTable>> {
    let state = super::block_state(id, window, cx, |window, cx| {
        TableState::new(SpecTable::new(spec.clone()), window, cx)
    });
    if state.read(cx).delegate().spec() != spec {
        state.update(cx, |state, cx| {
            *state.delegate_mut() = SpecTable::new(spec.clone());
            state.refresh(cx);
        });
    }
    state
}

/// The table of one block: its data, sort, column sizes and whether every
/// row is shown.
pub struct TableView {
    id: BlockId,
    table: SpecTable,
    /// Measured on the first frame, when the window's font is known.
    sizes: Option<Vec<ColSize>>,
    expanded: bool,
    scroll: ScrollHandle,
}

impl TableView {
    /// The table of block `id` showing `spec`, collapsed and unsorted.
    pub fn new(id: BlockId, spec: TableSpec) -> Self {
        Self {
            id,
            table: SpecTable::new(spec),
            sizes: None,
            expanded: false,
            scroll: ScrollHandle::new(),
        }
    }

    /// The rows and their order.
    pub fn table(&self) -> &SpecTable {
        &self.table
    }

    /// The rows, to sort them; call `cx.notify()` after.
    pub fn table_mut(&mut self) -> &mut SpecTable {
        &mut self.table
    }

    /// Whether every row (up to [`MAX_RENDERED_ROWS`]) is shown.
    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Shows every row, or only the first [`COLLAPSED_ROWS`].
    pub fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.expanded = expanded;
        cx.notify();
    }

    /// How many rows the next frame draws.
    pub fn rendered_rows(&self) -> usize {
        let limit = if self.expanded {
            MAX_RENDERED_ROWS
        } else {
            COLLAPSED_ROWS
        };
        self.table.order().len().min(limit)
    }

    /// Shows `spec`, resetting the sort and sizes when it changed.
    fn sync(&mut self, spec: &TableSpec, cx: &mut Context<Self>) {
        if self.table.spec() != spec {
            self.table = SpecTable::new(spec.clone());
            self.sizes = None;
            cx.notify();
        }
    }

    fn click_header(&mut self, col: usize, cx: &mut Context<Self>) {
        let next = sort::cycle(self.table.sorting(), col);
        self.table.set_sort(next);
        cx.notify();
    }

    fn sizes(&mut self, window: &Window) -> Vec<ColSize> {
        self.sizes
            .get_or_insert_with(|| {
                let font = LabelFont {
                    font: window.text_style().font(),
                    size: window.rem_size() * 0.875,
                };
                column_sizes(self.table.spec(), &|text| font.measure(text, window))
            })
            .clone()
    }

    fn header(&self, sizes: &[ColSize], cx: &mut Context<Self>) -> gpui::Div {
        let theme = cx.theme();
        let (bg, fg) = (theme.table_head, theme.table_head_foreground);
        let sorting = self.table.sorting();
        let cells = self
            .table
            .spec()
            .columns
            .iter()
            .enumerate()
            .map(|(col, c)| {
                let arrow = match sorting {
                    Some((sorted, Direction::Ascending)) if sorted == col => " ▲",
                    Some((sorted, Direction::Descending)) if sorted == col => " ▼",
                    _ => "",
                };
                let label = SharedString::from(format!("{}{arrow}", c.name));
                let align = column_align(self.table.spec(), col);
                cell_box(sizes[col], align)
                    .id(("table-head", col))
                    .cursor_pointer()
                    .whitespace_normal()
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.click_header(col, cx)))
            });
        h_flex()
            .w_full()
            .items_start()
            .bg(bg)
            .text_color(fg)
            .font_weight(FontWeight::SEMIBOLD)
            .border_b_1()
            .border_color(theme.border)
            .children(cells)
    }

    fn row(&self, row: usize, sizes: &[ColSize], cx: &App) -> gpui::Div {
        let theme = cx.theme();
        let columns = self.table.spec().columns.len();
        let cells = (0..columns).map(|col| {
            let text = self.table.cell(row, col).map(cell_text).unwrap_or_default();
            let cell = cell_box(sizes[col], column_align(self.table.spec(), col));
            if sizes[col].numeric {
                cell.whitespace_nowrap().text_ellipsis()
            } else {
                cell.whitespace_normal()
                    .line_clamp(MAX_CELL_LINES)
                    .text_ellipsis()
            }
            .child(SharedString::from(text))
        });
        let id = self.id;
        h_flex()
            .w_full()
            .items_start()
            .debug_selector(move || format!("table-{id}-row-{row}"))
            .when(row % 2 == 1, |this| this.bg(theme.table_even))
            .when(row + 1 < self.rendered_rows(), |this| {
                this.border_b_1().border_color(theme.border)
            })
            .children(cells)
    }

    fn footer(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let total = self.table.order().len();
        if total <= COLLAPSED_ROWS {
            return None;
        }
        let label = if self.expanded {
            "Show fewer".to_owned()
        } else {
            format!("Show all {total} rows")
        };
        let note = (self.expanded && total > MAX_RENDERED_ROWS)
            .then(|| format!("showing {MAX_RENDERED_ROWS} of {total} · y copies all"));
        let expanded = self.expanded;
        let button = Button::new(ElementId::Name(SharedString::from(format!(
            "table-more-{}",
            self.id
        ))))
        .ghost()
        .xsmall()
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| this.set_expanded(!expanded, cx)));
        let muted = cx.theme().muted_foreground;
        Some(
            h_flex()
                .gap_2()
                .items_center()
                .child(button)
                .children(note.map(|note| div().text_xs().text_color(muted).child(note))),
        )
    }
}

/// A cell of a column sized `size`, the same in the header and every row
/// so the columns line up and share the width.
fn cell_box(size: ColSize, align: Align) -> gpui::Div {
    let cell = div()
        .flex_basis(px(size.ideal))
        .flex_grow(size.ideal)
        .flex_shrink_1()
        .min_w(px(size.min))
        .overflow_hidden()
        .px_2()
        .py_1();
    match align {
        Align::Left => cell,
        Align::Center => cell.text_center(),
        Align::Right => cell.text_right(),
    }
}

impl Render for TableView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sizes = self.sizes(window);
        let min_width: f32 = sizes.iter().map(|s| s.min).sum();
        let header = self.header(&sizes, cx);
        let footer = self.footer(cx);
        let rows: Vec<_> = (0..self.rendered_rows())
            .map(|row| self.row(row, &sizes, cx))
            .collect();
        let theme = cx.theme();
        let id = self.id;
        v_flex()
            .w_full()
            .gap_1()
            .child(
                div()
                    .id(ElementId::Name(SharedString::from(format!("table-{id}"))))
                    .debug_selector(move || format!("table-{id}"))
                    .w_full()
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .track_scroll(&self.scroll)
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
                    ),
            )
            .children(footer)
    }
}

/// The table view of block `id`, in sync with `spec`.
pub fn table_view(
    id: BlockId,
    spec: &TableSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<TableView> {
    let view = super::block_state(id, window, cx, |_, _| TableView::new(id, spec.clone()));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws `spec` as the table of block `id`, with a caption line.
pub fn render_spec(id: BlockId, spec: &TableSpec, window: &mut Window, cx: &mut App) -> AnyElement {
    let view = table_view(id, spec, window, cx);
    let rows = spec.rows.len();
    let caption = format!(
        "{rows} {} × {} {}",
        if rows == 1 { "row" } else { "rows" },
        spec.columns.len(),
        if spec.columns.len() == 1 {
            "column"
        } else {
            "columns"
        }
    );
    v_flex()
        .w_full()
        .gap_1()
        .child(
            h_flex()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(caption),
        )
        .child(view)
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Table(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    render_spec(entry.block.id, spec, window, cx)
}

#[cfg(test)]
mod tests;
