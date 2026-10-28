//! Renderer of the `dataset` block kind (`show_dataset`): a data file's
//! profile and first rows.
//!
//! - The header: the path, the format, rows × columns and a `sampled`
//!   badge when the statistics cover only the first rows.
//! - The column profile: one row per column with its name, a type chip, a
//!   bar of the missing share, the distinct count (`≥n` when counting
//!   stopped at a cap), min, max and a sparkline of numeric columns
//!   ([`super::computed::descriptive::sparkline`]).
//! - The preview table, drawn by the table block's [`super::table`] view
//!   (sortable, expandable).
//!
//! At most [`MAX_PROFILE_ROWS`] columns are profiled on screen; copy-as
//! has all of them.

use domain::block::{BlockKind, Cell};
use domain::computed::apa::significant;
use domain::dataset::{DatasetColumn, DatasetFormat, DatasetSpec};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::computed::descriptive::sparkline;
use crate::timeline::TimelineEntry;

/// Most columns drawn in the profile; the rest are counted.
pub const MAX_PROFILE_ROWS: usize = 100;

/// Width of the missing-share bar.
const NULL_BAR_WIDTH: f32 = 44.0;

/// Lower-case name of a dataset format.
#[must_use]
pub fn format_name(format: DatasetFormat) -> &'static str {
    match format {
        DatasetFormat::Csv => "CSV",
        DatasetFormat::Tsv => "TSV",
        DatasetFormat::Parquet => "Parquet",
        DatasetFormat::Arrow => "Arrow",
        DatasetFormat::Jsonl => "JSON lines",
    }
}

/// A number in a few significant digits: whole numbers as they are,
/// others to 4 significant digits without trailing zeros.
#[must_use]
pub fn short_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", n as i64);
    }
    let text = significant(n, 4);
    if text.contains('.') && !text.contains('e') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

/// A min or max as shown in the profile.
#[must_use]
pub fn extreme_text(cell: Option<&Cell>) -> String {
    match cell {
        None | Some(Cell::Null) => String::new(),
        Some(Cell::Number(n)) => short_number(*n),
        Some(Cell::Bool(b)) => b.to_string(),
        Some(Cell::Text(t)) => t.clone(),
    }
}

/// The distinct count as shown: `≥n` when counting stopped at a cap.
#[must_use]
pub fn distinct_text(column: &DatasetColumn) -> String {
    match column.distinct {
        Some(n) if column.distinct_capped => format!("≥{n}"),
        Some(n) => n.to_string(),
        None => String::new(),
    }
}

/// `rows × columns` of the header; `?` rows when the file was not read to
/// the end.
#[must_use]
pub fn shape_text(spec: &DatasetSpec) -> String {
    let rows = spec.rows.map_or_else(|| "?".to_owned(), |n| n.to_string());
    format!("{rows} rows × {} columns", spec.columns.len())
}

/// A null share with one decimal, `0%` and `100%` without.
#[must_use]
pub fn percent_text(p: f64) -> String {
    if p == 0.0 || p == 100.0 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}

/// The profile as CSV: one line per column.
#[must_use]
pub fn profile_csv(spec: &DatasetSpec) -> String {
    use super::copy::csv_field;
    let mut out = String::from("column,dtype,null_percent,distinct,min,max\n");
    for c in &spec.columns {
        let fields = [
            csv_field(&c.name),
            csv_field(&c.dtype),
            format!("{}", c.null_percent),
            distinct_text(c),
            csv_field(&extreme_text(c.min.as_ref())),
            csv_field(&extreme_text(c.max.as_ref())),
        ];
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

/// Widths of the profile's fixed columns: type, missing, distinct, min,
/// max, distribution. The name column takes the rest.
const WIDTHS: [f32; 6] = [84.0, 96.0, 72.0, 96.0, 96.0, 96.0];
const HEADERS: [&str; 7] = [
    "Column",
    "Type",
    "Missing",
    "Distinct",
    "Min",
    "Max",
    "Distribution",
];
const NAME_MIN_WIDTH: f32 = 120.0;

fn profile(id: &str, spec: &DatasetSpec, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    let cell = |col: usize| {
        let base = div().px_2().py_0p5().overflow_hidden().whitespace_nowrap();
        if col == 0 {
            base.flex_1().min_w(px(NAME_MIN_WIDTH))
        } else {
            base.w(px(WIDTHS[col - 1])).flex_none()
        }
    };
    let header = h_flex()
        .w_full()
        .bg(theme.table_head)
        .text_color(theme.table_head_foreground)
        .font_weight(FontWeight::SEMIBOLD)
        .border_b_1()
        .border_color(theme.border)
        .children(
            HEADERS
                .iter()
                .enumerate()
                .map(|(col, title)| cell(col).child(*title)),
        );
    let rows = spec
        .columns
        .iter()
        .take(MAX_PROFILE_ROWS)
        .enumerate()
        .map(|(ix, c)| {
            let null_fill = (c.null_percent / 100.0).clamp(0.0, 1.0);
            #[allow(clippy::cast_possible_truncation)]
            let fill_width = NULL_BAR_WIDTH * null_fill as f32;
            let bar_color = if c.null_percent > 0.0 {
                theme.warning
            } else {
                theme.success
            };
            let spark = c.histogram.as_ref().map(sparkline).unwrap_or_default();
            h_flex()
                .w_full()
                .debug_selector(move || format!("dataset-column-{ix}"))
                .when(ix % 2 == 1, |this| this.bg(theme.table_even))
                .child(
                    cell(0)
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(c.name.clone())),
                )
                .child(
                    cell(1).child(
                        div()
                            .px_1()
                            .rounded_sm()
                            .bg(theme.secondary)
                            .text_xs()
                            .font_family(mono.clone())
                            .text_color(theme.secondary_foreground)
                            .child(SharedString::from(c.dtype.clone())),
                    ),
                )
                .child(
                    cell(2).child(
                        h_flex()
                            .gap_1()
                            .child(
                                div()
                                    .w(px(NULL_BAR_WIDTH))
                                    .h(px(6.))
                                    .rounded_sm()
                                    .bg(theme.muted)
                                    .child(
                                        div().w(px(fill_width)).h_full().rounded_sm().bg(bar_color),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(percent_text(c.null_percent)),
                            ),
                    ),
                )
                .child(
                    cell(3)
                        .text_right()
                        .font_family(mono.clone())
                        .child(distinct_text(c)),
                )
                .child(
                    cell(4)
                        .font_family(mono.clone())
                        .child(extreme_text(c.min.as_ref())),
                )
                .child(
                    cell(5)
                        .font_family(mono.clone())
                        .child(extreme_text(c.max.as_ref())),
                )
                .child(
                    cell(6)
                        .font_family(mono.clone())
                        .text_color(theme.chart_1)
                        .child(spark),
                )
        });
    let hidden = spec.columns.len().saturating_sub(MAX_PROFILE_ROWS);
    let min_width = NAME_MIN_WIDTH + WIDTHS.iter().sum::<f32>();
    div()
        .id(ElementId::Name(SharedString::from(format!("{id}-profile"))))
        .debug_selector(|| "dataset-profile".to_owned())
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
                .children(rows)
                .when(hidden > 0, |this| {
                    this.child(
                        div()
                            .px_2()
                            .py_0p5()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("… and {hidden} more columns")),
                    )
                }),
        )
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Dataset(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let id = format!("dataset-{}", entry.block.id);
    let table = super::table::table_view(entry.block.id, &spec.preview, window, cx);
    let theme = cx.theme();
    let header = h_flex()
        .gap_2()
        .flex_wrap()
        .items_center()
        .debug_selector(|| "dataset-header".to_owned())
        .child(
            div()
                .font_family(theme.mono_font_family.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(spec.path.clone())),
        )
        .child(
            div()
                .px_1()
                .rounded_sm()
                .bg(theme.secondary)
                .text_xs()
                .text_color(theme.secondary_foreground)
                .child(format_name(spec.format)),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(shape_text(spec)),
        )
        .when(spec.sampled, |this| {
            this.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!("{id}-sampled"))))
                    .debug_selector(|| "dataset-sampled".to_owned())
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme.warning)
                    .text_xs()
                    .text_color(theme.warning)
                    .child("sampled")
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new(
                            "Statistics cover only the first rows of the file",
                        )
                        .build(window, cx)
                    }),
            )
        });
    let rows = spec.preview.rows.len();
    let caption = format!("first {rows} {}", if rows == 1 { "row" } else { "rows" });
    v_flex()
        .w_full()
        .gap_2()
        .child(header)
        .child(profile(&id, spec, cx))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(caption),
        )
        .child(table)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::{Block, Column, TableSpec};
    use domain::computed::Histogram;
    use gpui::{TestAppContext, point, size};

    use super::*;
    use crate::timeline::TimelineView;

    fn column(name: &str, dtype: &str) -> DatasetColumn {
        DatasetColumn {
            name: name.into(),
            dtype: dtype.into(),
            null_percent: 0.0,
            distinct: Some(3),
            distinct_capped: false,
            min: None,
            max: None,
            histogram: None,
        }
    }

    fn spec() -> DatasetSpec {
        let mut mpg = column("mpg", "float");
        mpg.null_percent = 12.5;
        mpg.min = Some(Cell::Number(10.4));
        mpg.max = Some(Cell::Number(33.9));
        mpg.histogram = Some(Histogram {
            edges: vec![10.0, 20.0, 34.0],
            counts: vec![14, 18],
        });
        let mut id = column("id", "integer");
        id.distinct = Some(10_000);
        id.distinct_capped = true;
        DatasetSpec {
            path: "data/cars.csv".into(),
            format: DatasetFormat::Csv,
            rows: None,
            sampled: true,
            columns: vec![mpg, id, column("model, name", "text")],
            preview: TableSpec {
                columns: vec![
                    Column::new("mpg"),
                    Column::new("id"),
                    Column::new("model, name"),
                ],
                rows: vec![vec![
                    Cell::Number(21.0),
                    Cell::Number(1.0),
                    Cell::Text("Mazda".into()),
                ]],
            },
        }
    }

    #[test]
    fn profile_texts() {
        let spec = spec();
        assert_eq!(shape_text(&spec), "? rows × 3 columns");
        assert_eq!(distinct_text(&spec.columns[1]), "≥10000");
        assert_eq!(extreme_text(spec.columns[0].min.as_ref()), "10.4");
        assert_eq!(short_number(0.300_000_000_000_000_04), "0.3");
        assert_eq!(short_number(1234.5678), "1235");
        assert_eq!(short_number(-2.0), "-2");
        assert_eq!(percent_text(12.5), "12.5%");
        assert_eq!(percent_text(0.0), "0%");
        assert_eq!(
            profile_csv(&spec),
            "column,dtype,null_percent,distinct,min,max\n\
             mpg,float,12.5,3,10.4,33.9\n\
             id,integer,0,≥10000,,\n\
             \"model, name\",text,0,3,,\n"
        );
    }

    #[gpui::test]
    fn draws_header_profile_and_preview(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = Block::new(BlockKind::Dataset(spec()));
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(900.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "dataset-header",
            "dataset-sampled",
            "dataset-profile",
            "dataset-column-0",
            "dataset-column-2",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
