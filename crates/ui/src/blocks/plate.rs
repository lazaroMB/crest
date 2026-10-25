//! Renderer of the `plate` block kind (`show_plate`): a 96- or 384-well
//! microplate as a heatmap.
//!
//! Row letters run down the left, column numbers across the top. A well
//! with a value is filled on the plate's scale ([`PlateRange`]):
//! sequential in the theme's first chart colour from faint (the minimum)
//! to solid (the maximum), the same over `log10` of the values, or
//! diverging, the info colour below the centre and the danger colour
//! above it, faint near it. A legend sits underneath. A listed well
//! without a value (or with one a log scale cannot show) is outlined; an
//! unlisted well is muted. Labels sit inside the wells of a 96-well
//! plate, and of a 384-well one in a smaller font when they fit
//! ([`label_fits`]); every well has a tooltip with its name, label and
//! value.

use domain::block::BlockKind;
use domain::science::{PlateRange, PlateScale, PlateSpec, Well, parse_well, plate_dimensions};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::dataset::short_number;
use crate::timeline::TimelineEntry;

/// Steps of the legend's colour ramp.
const LEGEND_STEPS: usize = 12;
/// Opacity of the scale's lowest value.
const FAINT: f32 = 0.12;

/// Side of a well, in pixels, for a plate of `format`.
#[must_use]
pub fn well_size(format: u16) -> f32 {
    if format == 384 { 22.0 } else { 38.0 }
}

/// The wells laid out row by row: `rows × cols` slots, each the listed
/// well at that position, if any.
#[must_use]
pub fn layout(spec: &PlateSpec) -> Vec<Vec<Option<&Well>>> {
    let (rows, cols) = plate_dimensions(spec.format).unwrap_or((8, 12));
    let mut grid = vec![vec![None; usize::from(cols)]; usize::from(rows)];
    for well in &spec.wells {
        if let Some((r, c)) = parse_well(&well.well, spec.format)
            && let Some(slot) = grid
                .get_mut(usize::from(r))
                .and_then(|row| row.get_mut(usize::from(c)))
        {
            *slot = Some(well);
        }
    }
    grid
}

/// Font size of the labels inside the wells of a plate of `format`.
#[must_use]
pub fn label_font(format: u16) -> f32 {
    if format == 384 { 8.0 } else { 12.0 }
}

/// Whether `label` fits in a well `size` pixels wide at `font` pixels
/// (about 0.6 em per character). Labels of 96-well plates are always
/// shown, clipped to the well.
#[must_use]
pub fn label_fits(label: &str, format: u16) -> bool {
    if format != 384 {
        return true;
    }
    #[allow(clippy::cast_precision_loss)]
    let width = label.chars().count() as f32 * label_font(format) * 0.6;
    width <= well_size(format) - 2.0
}

/// The colour of a well at scale position `t` (see
/// [`PlateRange::position`]): the sequential base from faint to solid, or
/// for a diverging scale `low` below the centre and `high` above it.
fn shade(scale: PlateScale, t: f64, base: Hsla, low: Hsla, high: Hsla) -> Hsla {
    #[allow(clippy::cast_possible_truncation)]
    let t = t as f32;
    let ramp = |color: Hsla, t: f32| color.opacity(FAINT + (1.0 - FAINT) * t);
    match scale {
        PlateScale::Diverging if t < 0.0 => ramp(low, -t),
        PlateScale::Diverging => ramp(high, t),
        PlateScale::Sequential | PlateScale::Log => ramp(base, t),
    }
}

/// Whether text on a well at position `t` needs the background colour.
fn dark(t: f64) -> bool {
    t.abs() > 0.55
}

/// The tooltip of a well: name, label and value.
#[must_use]
pub fn tooltip_text(name: &str, well: Option<&Well>, value_label: Option<&str>) -> String {
    let mut text = name.to_owned();
    let Some(well) = well else {
        return format!("{text} · empty");
    };
    if let Some(label) = &well.label {
        text.push_str(&format!(" · {label}"));
    }
    if let Some(value) = well.value {
        let unit = value_label.map(|l| format!(" {l}")).unwrap_or_default();
        text.push_str(&format!(" · {value}{unit}"));
    }
    text
}

/// [`tooltip_text`], noting a value a log scale does not colour.
fn well_tooltip(name: &str, well: Option<&Well>, spec: &PlateSpec) -> String {
    let mut text = tooltip_text(name, well, spec.value_label.as_deref());
    if spec.scale == PlateScale::Log && well.and_then(|w| w.value).is_some_and(|v| v <= 0.0) {
        text.push_str(" (not shown on the log scale)");
    }
    text
}

/// The wells as CSV, `well,label,value`, row by row.
#[must_use]
pub fn plate_csv(spec: &PlateSpec) -> String {
    use super::copy::csv_field;
    let mut out = String::from("well,label,value\n");
    for well in layout(spec).iter().flatten().flatten() {
        let label = well.label.as_deref().map(csv_field).unwrap_or_default();
        let value = well.value.map(|v| v.to_string()).unwrap_or_default();
        out.push_str(&format!(
            "{},{label},{value}\n",
            well.well.to_ascii_uppercase()
        ));
    }
    out
}

fn row_letter(row: usize) -> char {
    u8::try_from(row)
        .ok()
        .and_then(|r| b'A'.checked_add(r))
        .map_or('?', char::from)
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, _window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Plate(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let theme = cx.theme();
    let size = well_size(spec.format);
    let range = PlateRange::of(spec);
    let (base, low, high) = (theme.chart_1, theme.info, theme.danger);
    let font = label_font(spec.format);
    let block = entry.block.id;
    let grid = layout(spec);
    let cols = grid.first().map_or(0, Vec::len);
    let label_cell = |text: String| {
        div()
            .w(px(size))
            .flex_none()
            .text_xs()
            .text_center()
            .text_color(theme.muted_foreground)
            .child(text)
    };
    let header = h_flex()
        .gap(px(2.))
        .child(div().w(px(18.)).flex_none())
        .children((1..=cols).map(|c| label_cell(c.to_string())));
    let rows = grid.iter().enumerate().map(|(r, row)| {
        let letter = row_letter(r);
        h_flex()
            .gap(px(2.))
            .child(
                div()
                    .w(px(18.))
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(letter.to_string()),
            )
            .children(row.iter().enumerate().map(|(c, well)| {
                let name = format!("{letter}{}", c + 1);
                let tip = well_tooltip(&name, *well, spec);
                let selector = format!("plate-well-{name}");
                let mut cell = div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "plate-{block}-{name}"
                    ))))
                    .debug_selector(move || selector.clone())
                    .w(px(size))
                    .h(px(size))
                    .flex_none()
                    .rounded_sm()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
                match well {
                    None => cell = cell.bg(theme.muted),
                    Some(well) => {
                        let t = well.value.zip(range).and_then(|(v, r)| r.position(v));
                        cell = match t {
                            Some(t) => cell.bg(shade(spec.scale, t, base, low, high)).text_color(
                                if dark(t) {
                                    theme.background
                                } else {
                                    theme.foreground
                                },
                            ),
                            None => cell.border_1().border_color(theme.border),
                        };
                        if let Some(label) = &well.label
                            && label_fits(label, spec.format)
                        {
                            cell = cell
                                .text_size(px(font))
                                .child(SharedString::from(label.clone()));
                        }
                    }
                }
                cell
            }))
    });
    let legend = range.map(|range| {
        let diverging = range.scale == PlateScale::Diverging;
        #[allow(clippy::cast_precision_loss)]
        let swatches = (0..LEGEND_STEPS).map(|i| {
            let u = i as f64 / (LEGEND_STEPS - 1) as f64;
            let t = if diverging { 2.0 * u - 1.0 } else { u };
            div()
                .w(px(14.))
                .h(px(10.))
                .bg(shade(range.scale, t, base, low, high))
        });
        // A diverging legend runs from the centre minus the reach to the
        // centre plus it.
        let reach = (range.hi - range.center)
            .abs()
            .max((range.center - range.lo).abs());
        let (from, to) = if diverging {
            (range.center - reach, range.center + reach)
        } else {
            (range.lo, range.hi)
        };
        let note = match range.scale {
            PlateScale::Sequential => None,
            PlateScale::Log => Some("log scale".to_owned()),
            PlateScale::Diverging => Some(format!("centre {}", short_number(range.center))),
        };
        h_flex()
            .gap_1()
            .text_xs()
            .text_color(theme.muted_foreground)
            .debug_selector(|| "plate-legend".to_owned())
            .child(short_number(from))
            .child(h_flex().children(swatches))
            .child(short_number(to))
            .when_some(spec.value_label.clone(), |this, label| this.child(label))
            .when_some(note, |this, note| this.child(note))
    });
    let title = spec.title.clone().unwrap_or_else(|| {
        let (r, c) = plate_dimensions(spec.format).unwrap_or_default();
        format!("{}-well plate ({r} × {c})", spec.format)
    });
    v_flex()
        .gap_1()
        .debug_selector(|| "plate".to_owned())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "plate-{block}"
                ))))
                .max_w_full()
                .overflow_x_scroll()
                .child(v_flex().gap(px(2.)).child(header).children(rows)),
        )
        .children(legend)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{TestAppContext, point, size};

    use super::*;
    use crate::timeline::TimelineView;

    fn well(name: &str, label: Option<&str>, value: Option<f64>) -> Well {
        Well {
            well: name.into(),
            label: label.map(str::to_owned),
            value,
        }
    }

    fn spec() -> PlateSpec {
        PlateSpec {
            title: None,
            format: 96,
            value_label: Some("OD600".into()),
            wells: vec![
                well("h12", Some("ctrl+"), Some(1.2)),
                well("A1", Some("blank, 1"), Some(0.04)),
                well("B2", None, None),
            ],
            scale: PlateScale::Sequential,
            center: None,
        }
    }

    #[test]
    fn layout_scale_and_texts() {
        let spec = spec();
        let grid = layout(&spec);
        assert_eq!((grid.len(), grid[0].len()), (8, 12));
        assert_eq!(grid[7][11].map(|w| w.value), Some(Some(1.2)));
        assert!(grid[0][1].is_none());
        let range = PlateRange::of(&spec).unwrap();
        assert_eq!((range.lo, range.hi), (0.04, 1.2));
        assert_eq!(range.position(1.2), Some(1.0));
        // 384-well labels show when short enough.
        assert!(label_fits("c+", 384) && !label_fits("blank, 1", 384));
        assert!(label_fits("blank, 1", 96));
        let mut log = spec.clone();
        log.scale = PlateScale::Log;
        log.wells.push(well("C3", None, Some(0.0)));
        assert!(
            well_tooltip("C3", log.wells.last(), &log).ends_with("(not shown on the log scale)")
        );
        assert_eq!(
            tooltip_text("A1", grid[0][0], Some("OD600")),
            "A1 · blank, 1 · 0.04 OD600"
        );
        assert_eq!(tooltip_text("C3", None, None), "C3 · empty");
        assert_eq!(
            plate_csv(&spec),
            "well,label,value\nA1,\"blank, 1\",0.04\nB2,,\nH12,ctrl+,1.2\n"
        );
    }

    #[gpui::test]
    fn draws_wells_and_legend(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let mut diverging = spec();
        diverging.format = 384;
        diverging.scale = PlateScale::Diverging;
        diverging.wells.push(well("P24", Some("x"), Some(-3.0)));
        let mut log = spec();
        log.scale = PlateScale::Log;
        view.update(cx, |view, cx| {
            view.apply_all(
                [spec(), diverging, log]
                    .into_iter()
                    .map(|spec| DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Plate(spec)),
                    }),
                cx,
            );
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(900.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "plate",
            "plate-well-A1",
            "plate-well-H12",
            "plate-well-D6",
            "plate-legend",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
