//! Renderer of the `table` block kind: a virtualized, sortable `DataTable`.
//!
//! Each block keeps its `TableState` across frames (see
//! [`super::block_state`]), so the sort order and scroll position survive
//! scrolling the timeline. Only the visible rows are laid out, so the cost
//! of a frame does not grow with the row count. Clicking a header's sort
//! icon cycles descending → ascending → original order ([`sort`]).
//! [`render_spec`] is shared with the chart block's table view.

pub mod sort;

use domain::block::{Align, BlockId, BlockKind, Cell, TableSpec};
use gpui::{
    AnyElement, App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    div, px,
};
use gpui_component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_component::{ActiveTheme as _, Sizable as _, Size, h_flex, v_flex};

use crate::timeline::TimelineEntry;

/// Rows shown before the table scrolls inside its block.
const VISIBLE_ROWS: usize = 12;
/// Rows sampled to size a column.
const WIDTH_SAMPLE: usize = 200;
/// Rough advance of a character of the UI font, to size columns.
const CHAR_WIDTH: f32 = 7.5;
const CELL_PADDING: f32 = 28.0;
const MIN_COL_WIDTH: f32 = 56.0;
const MAX_COL_WIDTH: f32 = 360.0;

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

/// Alignment of column `col`: the declared one, else right when every
/// non-null cell is a number.
fn column_align(spec: &TableSpec, col: usize) -> Align {
    if let Some(align) = spec.columns.get(col).and_then(|c| c.align) {
        return align;
    }
    let mut cells = spec.rows.iter().filter_map(|row| row.get(col));
    let numeric = cells.all(|c| matches!(c, Cell::Number(_) | Cell::Null));
    let any = spec
        .rows
        .iter()
        .any(|row| matches!(row.get(col), Some(Cell::Number(_))));
    if numeric && any {
        Align::Right
    } else {
        Align::Left
    }
}

/// Width of column `col` from its header and a sample of its cells.
fn column_width(spec: &TableSpec, col: usize) -> f32 {
    let header = spec.columns.get(col).map_or(0, |c| c.name.chars().count());
    let widest = spec
        .rows
        .iter()
        .take(WIDTH_SAMPLE)
        .filter_map(|row| row.get(col))
        .map(|cell| cell_text(cell).chars().count())
        .fold(header + 2, usize::max);
    (widest as f32 * CHAR_WIDTH + CELL_PADDING).clamp(MIN_COL_WIDTH, MAX_COL_WIDTH)
}

/// `DataTable` delegate over a [`TableSpec`], with the current row order.
pub struct SpecTable {
    spec: TableSpec,
    /// Index into `spec.rows` of each displayed row.
    order: Vec<usize>,
    columns: Vec<Column>,
}

impl SpecTable {
    /// A delegate showing `spec` in its original order.
    pub fn new(spec: TableSpec) -> Self {
        let columns = (0..spec.columns.len())
            .map(|col| {
                let name = spec.columns[col].name.clone();
                let column = Column::new(SharedString::from(format!("c{col}")), name)
                    .width(px(column_width(&spec, col)))
                    .sortable();
                match column_align(&spec, col) {
                    Align::Left => column,
                    Align::Center => column.text_center(),
                    Align::Right => column.text_right(),
                }
            })
            .collect();
        let order = (0..spec.rows.len()).collect();
        Self {
            spec,
            order,
            columns,
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

    /// Sorts the rows by `col` (`ColumnSort::Default` restores the original
    /// order).
    pub fn sort(&mut self, col: usize, sort: ColumnSort) {
        self.order = match sort {
            ColumnSort::Default => (0..self.spec.rows.len()).collect(),
            ColumnSort::Ascending => {
                sort::sorted_order(&self.spec.rows, col, sort::Direction::Ascending)
            }
            ColumnSort::Descending => {
                sort::sorted_order(&self.spec.rows, col, sort::Direction::Descending)
            }
        };
    }

    fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        let row = *self.order.get(row)?;
        self.spec.rows.get(row)?.get(col)
    }
}

impl TableDelegate for SpecTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns.get(col_ix).cloned().unwrap_or_default()
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
        div()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(SharedString::from(text))
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.cell(row_ix, col_ix).map(cell_text).unwrap_or_default()
    }
}

/// The table state of block `id`, showing `spec`. The data is replaced
/// (and the sort reset) only when the block's spec changed.
pub fn table_state(
    id: BlockId,
    spec: &TableSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<TableState<SpecTable>> {
    let state = super::block_state(id, window, cx, |window, cx| {
        TableState::new(SpecTable::new(spec.clone()), window, cx)
            .col_movable(false)
            .loop_selection(false)
    });
    if state.read(cx).delegate().spec() != spec {
        state.update(cx, |state, cx| {
            *state.delegate_mut() = SpecTable::new(spec.clone());
            state.refresh(cx);
        });
    }
    state
}

/// Draws `spec` as the table of block `id`, with a caption line.
pub fn render_spec(id: BlockId, spec: &TableSpec, window: &mut Window, cx: &mut App) -> AnyElement {
    let state = table_state(id, spec, window, cx);
    let size = Size::Small;
    let rows = spec.rows.len();
    let visible = rows.clamp(3, VISIBLE_ROWS) as f32;
    // Header plus visible rows, the border and the horizontal scrollbar.
    let height = size.table_row_height() * (visible + 1.0) + px(12.0);
    let theme = cx.theme();
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
        .gap_1()
        .child(
            h_flex()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(caption),
        )
        .child(
            div()
                .w_full()
                .h(height)
                .child(DataTable::new(&state).with_size(size).stripe(true)),
        )
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
mod tests {
    use super::*;
    use application::keymap::Action;
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{TestAppContext, point, size};

    use crate::timeline::TimelineView;

    fn spec(rows: usize) -> TableSpec {
        TableSpec {
            columns: vec![
                domain::block::Column::new("file"),
                domain::block::Column::new("bytes"),
            ],
            rows: (0..rows)
                .map(|i| {
                    let bytes = if i % 7 == 3 {
                        Cell::Null
                    } else {
                        Cell::Number(((i * 7919) % 1000) as f64)
                    };
                    vec![Cell::Text(format!("src/f{i}.rs")), bytes]
                })
                .collect(),
        }
    }

    #[test]
    fn cells_render_as_plain_text() {
        assert_eq!(cell_text(&Cell::Number(42.0)), "42");
        assert_eq!(cell_text(&Cell::Number(-0.25)), "-0.25");
        assert_eq!(cell_text(&Cell::Null), "");
        assert_eq!(cell_text(&Cell::Bool(true)), "true");
    }

    #[test]
    fn numeric_columns_align_right() {
        let spec = spec(10);
        assert_eq!(column_align(&spec, 0), Align::Left);
        assert_eq!(column_align(&spec, 1), Align::Right);
    }

    #[test]
    fn delegate_sorts_and_restores_order() {
        let mut table = SpecTable::new(spec(20));
        table.sort(1, ColumnSort::Ascending);
        let values: Vec<_> = table
            .order()
            .iter()
            .map(|&ix| table.spec().rows[ix][1].clone())
            .collect();
        let numbers: Vec<f64> = values
            .iter()
            .filter_map(|c| match c {
                Cell::Number(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert!(numbers.is_sorted());
        assert_eq!(values.last(), Some(&Cell::Null));
        table.sort(1, ColumnSort::Default);
        assert_eq!(table.order(), (0..20).collect::<Vec<_>>());
    }

    #[gpui::test]
    fn draws_a_large_table_in_the_timeline(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = Block::new(BlockKind::Table(spec(10_000)));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply(&DomainEvent::BlockAdded { block }, cx);
            view.handle_action(&Action::builtin("block.last"), cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(800.), px(600.)), |_, _| {
            div().size_full().child(view.clone())
        });
        let state = cx.update(|window, cx| table_state(id, &spec(10_000), window, cx));
        state.read_with(cx, |state, cx| {
            assert_eq!(state.delegate().rows_count(cx), 10_000);
            let visible = state.visible_range().rows().clone();
            assert!(
                !visible.is_empty() && visible.len() < 50,
                "only visible rows are laid out: {visible:?}"
            );
        });
    }
}
