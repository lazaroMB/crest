use super::*;
use application::ports::events::DomainEvent;
use domain::block::Block;
use gpui::{Entity, TestAppContext, VisualTestContext, size};

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

/// A text measure of 7px per character.
fn by_chars(text: &str) -> f32 {
    text.chars().count() as f32 * 7.0
}

/// Draws a table block showing `spec` in a timeline `width` wide; real
/// window frames rather than `cx.draw`, like the chart tests.
fn draw_table(
    cx: &mut TestAppContext,
    spec: TableSpec,
    width: f32,
) -> (BlockId, Entity<TimelineView>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let block = Block::new(BlockKind::Table(spec));
    let id = block.id;
    view.update(cx, |view, cx| {
        view.apply(&DomainEvent::BlockAdded { block }, cx)
    });
    cx.simulate_resize(size(px(width), px(5000.)));
    cx.run_until_parked();
    (id, view, cx)
}

fn table_of(id: BlockId, cx: &mut VisualTestContext) -> Entity<TableView> {
    cx.update(|window, cx| {
        crate::blocks::block_state::<TableView>(id, window, cx, |_, _| {
            panic!("the table was not drawn")
        })
    })
}

fn bounds(cx: &mut VisualTestContext, selector: String) -> Option<gpui::Bounds<gpui::Pixels>> {
    // `debug_bounds` takes a static selector.
    cx.debug_bounds(selector.leak())
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
fn table_sorts_and_restores_order() {
    let mut table = SpecTable::new(spec(20));
    table.set_sort(Some((1, Direction::Ascending)));
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
    table.set_sort(None);
    assert_eq!(table.order(), (0..20).collect::<Vec<_>>());
}

#[test]
fn numeric_columns_never_shrink_below_their_ideal() {
    let sizes = column_sizes(&spec(50), &by_chars);
    assert!(!sizes[0].numeric);
    assert!(sizes[1].numeric);
    assert_eq!(sizes[1].min, sizes[1].ideal);
}

#[test]
fn long_text_columns_may_shrink_to_their_longest_word() {
    let words = "a fairly long sentence that should wrap inside its cell ".repeat(4);
    let spec = TableSpec {
        columns: vec![domain::block::Column::new("note")],
        rows: vec![vec![Cell::Text(words)]],
    };
    let [size] = column_sizes(&spec, &by_chars)[..] else {
        panic!("one column");
    };
    assert_eq!(size.ideal, MAX_IDEAL_WIDTH);
    assert!(size.min < size.ideal);
    assert!(size.min >= MIN_COL_WIDTH);
    // "sentence" (8 chars) plus the padding, under the floor.
    assert_eq!(size.min, MIN_COL_WIDTH.max(8.0 * 7.0 + CELL_PADDING));
}

#[test]
fn short_columns_get_the_minimum_width() {
    let spec = TableSpec {
        columns: vec![domain::block::Column::new("x")],
        rows: vec![vec![Cell::Text("a".into())]],
    };
    let sizes = column_sizes(&spec, &by_chars);
    assert_eq!(sizes[0].ideal, MIN_COL_WIDTH);
    assert_eq!(sizes[0].min, MIN_COL_WIDTH);
}

#[gpui::test]
fn a_large_table_draws_at_most_the_row_limit(cx: &mut TestAppContext) {
    let (id, _view, cx) = draw_table(cx, spec(10_000), 800.);
    let table = table_of(id, cx);
    table.read_with(cx, |table, _| {
        assert_eq!(table.table().order().len(), 10_000);
        assert_eq!(table.rendered_rows(), COLLAPSED_ROWS);
    });
    assert!(bounds(cx, format!("table-{id}-row-{}", COLLAPSED_ROWS - 1)).is_some());
    assert!(bounds(cx, format!("table-{id}-row-{COLLAPSED_ROWS}")).is_none());

    table.update(cx, |table, cx| table.set_expanded(true, cx));
    cx.run_until_parked();
    table.read_with(cx, |table, _| {
        assert_eq!(table.rendered_rows(), MAX_RENDERED_ROWS);
    });
    assert!(bounds(cx, format!("table-{id}-row-{}", MAX_RENDERED_ROWS - 1)).is_some());
    assert!(bounds(cx, format!("table-{id}-row-{MAX_RENDERED_ROWS}")).is_none());
}

#[gpui::test]
fn sorting_applies_to_every_row(cx: &mut TestAppContext) {
    let (id, _view, cx) = draw_table(cx, spec(10_000), 800.);
    let table = table_of(id, cx);
    table.update(cx, |table, cx| table.click_header(1, cx));
    cx.run_until_parked();
    table.read_with(cx, |table, _| {
        assert_eq!(table.table().sorting(), Some((1, Direction::Descending)));
        // 999 is the largest value in the whole table, wherever it was.
        assert_eq!(table.table().cell(0, 1), Some(&Cell::Number(999.0)));
    });
}

#[gpui::test]
fn a_narrow_table_fills_the_block_width(cx: &mut TestAppContext) {
    let (id, _view, cx) = draw_table(cx, spec(5), 1000.);
    let container = bounds(cx, format!("table-{id}")).expect("table drawn");
    let row = bounds(cx, format!("table-{id}-row-0")).expect("row drawn");
    assert!(container.size.width > px(900.), "{container:?}");
    // The rows sit inside the 1px border.
    assert!(
        (container.size.width - row.size.width).abs() <= px(2.),
        "row {row:?} in {container:?}"
    );
}

#[gpui::test]
fn long_text_wraps_into_a_taller_row(cx: &mut TestAppContext) {
    let long = "a long description that keeps going well past the width of its column ".repeat(6);
    let spec = TableSpec {
        columns: vec![
            domain::block::Column::new("name"),
            domain::block::Column::new("description"),
        ],
        rows: vec![
            vec![Cell::Text("short".into()), Cell::Text("brief".into())],
            vec![Cell::Text("long".into()), Cell::Text(long)],
        ],
    };
    let (id, _view, cx) = draw_table(cx, spec, 1000.);
    let short = bounds(cx, format!("table-{id}-row-0")).expect("row drawn");
    let tall = bounds(cx, format!("table-{id}-row-1")).expect("row drawn");
    assert!(
        tall.size.height > short.size.height * 1.5,
        "{short:?} vs {tall:?}"
    );
    let container = bounds(cx, format!("table-{id}")).expect("table drawn");
    assert!(tall.size.width <= container.size.width);
}
