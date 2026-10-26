//! Renderer of the `array_file` block kind (`show_array_file`): an HDF5
//! or NetCDF file's structure and bounded previews.
//!
//! A summary line comes first (format, size, group and variable counts).
//! The structure is the tree table ([`crate::widgets::tree`]): groups
//! holding their variables with type, shape (with NetCDF dimension
//! names), chunks and filters, attributes as `@name` rows. The previews
//! follow, one at a time behind chips: a 1D preview is a line on the plot
//! painter, a 2D slice a heatmap (one-hue ramp, missing cells muted,
//! hover reads the cell), a scalar its value, strings a list. Under each
//! preview: what was read (`[0, 0:8, 0:6]`), the CF decoding applied and
//! the engine's statistics, labelled as the preview's only.

use std::cell::Cell;
use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use domain::science::arrays::{ArrayFileSpec, ArrayPreview, PreviewKind};
use domain::widget::tree::TreeSpec;
use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, Hsla, InteractiveElement as _, IntoElement,
    MouseMoveEvent, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, point,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use crate::theme::token;
use crate::timeline::TimelineEntry;
use crate::widgets::{self, palette, readout};

/// Height of the heatmap canvas, in pixels.
pub const HEATMAP_HEIGHT: f32 = 320.0;
/// Notes listed.
const LISTED: usize = 5;

/// A byte count for people: `12.3 KiB`.
fn bytes_text(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let v = n as f64;
    if v >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GiB", v / (1024.0 * 1024.0 * 1024.0))
    } else if v >= 1024.0 * 1024.0 {
        format!("{:.1} MiB", v / (1024.0 * 1024.0))
    } else if v >= 1024.0 {
        format!("{:.1} KiB", v / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// A statistic for people: up to 6 significant digits.
fn number(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return v.to_string();
    }
    let digits = (5 - v.abs().log10().floor() as i32).clamp(0, 10) as usize;
    let s = format!("{v:.digits$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// The summary text: format, size and counts.
#[must_use]
pub fn summary_text(spec: &ArrayFileSpec) -> String {
    let groups = spec.nodes.len() - spec.variables().count();
    let mut text = format!(
        "{} · {} · {groups} groups · {} variables · {} previews",
        spec.format.name(),
        bytes_text(spec.size),
        spec.variables().count(),
        spec.previews.len()
    );
    if spec.truncated {
        text.push_str(" · listing cut");
    }
    text
}

/// The statistics line of a preview, from its envelope.
#[must_use]
pub fn stats_text(preview: &ArrayPreview) -> String {
    let v = &preview.stats.values;
    let parts: Vec<String> = ["n", "missing", "min", "max", "mean", "sd"]
        .into_iter()
        .filter_map(|k| v.get(k).map(|x| format!("{k} {}", number(x.v))))
        .collect();
    format!(
        "Of this preview only ({}): {}",
        preview.selection,
        parts.join(" · ")
    )
}

/// The plot of a 1D preview: the values against their index.
#[must_use]
pub fn line_plot(preview: &ArrayPreview, units: Option<&str>) -> PlotSpec {
    #[allow(clippy::cast_precision_loss)]
    let (x, y): (Vec<f64>, Vec<f64>) = preview
        .values
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|v| (i as f64, v)))
        .unzip();
    PlotSpec {
        x_label: Some("index".into()),
        y_label: units.map(str::to_owned),
        series: vec![XySeries {
            name: preview.path.clone(),
            x,
            y,
            style: SeriesStyle::Line,
        }],
        ..PlotSpec::default()
    }
}

/// The ramp colour of position `t` (0 low, 1 high).
fn ramp(base: Hsla, t: f64, dark: bool) -> Hsla {
    let (lo, hi) = if dark { (0.2, 0.8) } else { (0.9, 0.25) };
    #[allow(clippy::cast_possible_truncation)]
    let t = t.clamp(0.0, 1.0) as f32;
    Hsla {
        l: lo + (hi - lo) * t,
        s: base.s.max(0.5),
        ..base
    }
}

/// Per-block state: the spec, its tree, the preview shown and the
/// hovered heatmap cell.
pub struct ArrayFileView {
    id: BlockId,
    spec: Rc<ArrayFileSpec>,
    tree: Rc<TreeSpec>,
    shown: usize,
    hover: Option<(usize, usize)>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl ArrayFileView {
    fn new(id: BlockId, spec: &ArrayFileSpec) -> Self {
        Self {
            id,
            tree: Rc::new(spec.to_tree()),
            spec: Rc::new(spec.clone()),
            shown: 0,
            hover: None,
            bounds: Rc::default(),
        }
    }

    fn sync(&mut self, spec: &ArrayFileSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The structure tree.
    pub fn tree(&self) -> &TreeSpec {
        &self.tree
    }

    /// Index of the preview shown.
    pub fn shown(&self) -> usize {
        self.shown
    }

    /// Shows preview `ix`.
    pub fn show(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.spec.previews.len() {
            self.shown = ix;
            self.hover = None;
            cx.notify();
        }
    }

    /// The hovered heatmap cell (row, column).
    pub fn hover(&self) -> Option<(usize, usize)> {
        self.hover
    }

    fn heat_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.spec.previews.get(self.shown) else {
            return;
        };
        let b = self.bounds.get();
        let cell = if b.contains(&event.position) && p.rows > 0 && p.cols > 0 {
            let x = f32::from(event.position.x - b.origin.x) / f32::from(b.size.width);
            let y = f32::from(event.position.y - b.origin.y) / f32::from(b.size.height);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let at = (
                ((y * p.rows as f32) as usize).min(p.rows - 1),
                ((x * p.cols as f32) as usize).min(p.cols - 1),
            );
            Some(at)
        } else {
            None
        };
        if cell != self.hover {
            self.hover = cell;
            cx.notify();
        }
    }

    fn heatmap(&self, preview: &ArrayPreview, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (base, missing, dark) = (palette(0, cx), theme.muted, theme.background.l < 0.5);
        let p = preview.clone();
        let range = p.range();
        let bounds_cell = Rc::clone(&self.bounds);
        let cells = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, (), window, _| {
                let (rows, cols) = (p.rows.max(1), p.cols.max(1));
                #[allow(clippy::cast_precision_loss)]
                let (w, h) = (
                    f32::from(bounds.size.width) / cols as f32,
                    f32::from(bounds.size.height) / rows as f32,
                );
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for r in 0..rows {
                        for c in 0..cols {
                            let color = match (p.at(r, c), range) {
                                (Some(v), Some((lo, hi))) => ramp(
                                    base,
                                    if hi > lo { (v - lo) / (hi - lo) } else { 0.5 },
                                    dark,
                                ),
                                _ => missing,
                            };
                            #[allow(clippy::cast_precision_loss)]
                            let cell = Bounds::new(
                                bounds.origin + point(px(c as f32 * w), px(r as f32 * h)),
                                gpui::size(px(w.ceil()), px(h.ceil())),
                            );
                            window.paint_quad(gpui::fill(cell, color));
                        }
                    }
                });
            },
        )
        .size_full();
        let readout_text = self.hover.map(|(r, c)| {
            let value = preview
                .at(r, c)
                .map_or_else(|| "missing".to_owned(), number);
            format!("row {r} · col {c} · {value}")
        });
        let scale = range.map(|(lo, hi)| {
            h_flex()
                .gap_1()
                .text_xs()
                .child(SharedString::from(number(lo)))
                .children((0..6).map(|i| {
                    div()
                        .size_3()
                        .rounded_sm()
                        .bg(ramp(base, f64::from(i) / 5.0, dark))
                }))
                .child(SharedString::from(number(hi)))
        });
        v_flex()
            .gap_1()
            .child(
                div()
                    .id("array-heatmap")
                    .debug_selector(|| "array-heatmap".to_owned())
                    .relative()
                    .w_full()
                    .h(px(HEATMAP_HEIGHT))
                    .border_1()
                    .border_color(token(cx, "accent.bg"))
                    .on_mouse_move(cx.listener(Self::heat_move))
                    .child(cells)
                    .when_some(readout_text, |d, text| {
                        d.child(
                            div()
                                .absolute()
                                .left_1()
                                .bottom_1()
                                .text_xs()
                                .debug_selector(|| "array-cell".to_owned())
                                .child(readout(text, cx)),
                        )
                    }),
            )
            .children(scale)
            .into_any_element()
    }

    fn preview(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.spec.previews.get(self.shown)?.clone();
        let muted = cx.theme().muted_foreground;
        let units = self.spec.node(&p.path).and_then(|n| n.attr("units"));
        let body = match p.kind {
            PreviewKind::Heatmap => self.heatmap(&p, cx),
            PreviewKind::Line => super::plot::static_plot(
                &line_plot(&p, units),
                &format!("array-{}-{}", self.id, self.shown),
                window,
                cx,
            ),
            PreviewKind::Scalar => div()
                .text_sm()
                .child(SharedString::from(
                    p.values
                        .first()
                        .copied()
                        .flatten()
                        .map_or_else(|| "missing".to_owned(), number),
                ))
                .into_any_element(),
            PreviewKind::Text => v_flex()
                .text_xs()
                .children(
                    p.texts
                        .iter()
                        .map(|t| div().child(SharedString::from(t.clone()))),
                )
                .into_any_element(),
        };
        Some(
            v_flex()
                .gap_1()
                .debug_selector(|| "array-preview".to_owned())
                .child(body)
                .when_some(p.decoding.clone(), |d, text| {
                    d.child(div().text_xs().text_color(muted).child(text))
                })
                .when(p.kind != PreviewKind::Text, |d| {
                    d.child(
                        div()
                            .text_xs()
                            .debug_selector(|| "array-stats".to_owned())
                            .child(SharedString::from(stats_text(&p))),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(super::computed::provenance_text(&p.stats)),
                )
                .into_any_element(),
        )
    }
}

impl Render for ArrayFileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = Rc::clone(&self.tree);
        let tree = widgets::tree::render(node_id(self.id, 0), &tree, window, cx);
        let preview = self.preview(window, cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let shown = self.shown;
        let chips: Vec<AnyElement> = self
            .spec
            .previews
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let on = i == shown;
                div()
                    .id(("array-preview-chip", i))
                    .debug_selector(move || format!("array-preview-{i}"))
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(token(cx, "accent.bg"))
                            .text_color(token(cx, "accent.fg"))
                    })
                    .when(!on, |d| d.text_color(muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.show(i, cx);
                    }))
                    .child(SharedString::from(format!("{} {}", p.path, p.selection)))
                    .into_any_element()
            })
            .collect();
        let warnings = self.spec.warnings.clone();
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "array-file".to_owned())
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "array-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .child(tree)
            .when(!chips.is_empty(), |d| {
                d.child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .text_xs()
                        .child(div().text_color(muted).child("Preview"))
                        .children(chips),
                )
            })
            .children(preview)
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "array-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone()))),
                )
            })
    }
}

/// The inspector entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &ArrayFileSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<ArrayFileView> {
    let view = super::block_state(id, window, cx, |_, _| ArrayFileView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::ArrayFile(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::computed::{Computed, ComputedValue, Engine};
    use domain::science::arrays::{ArrayFormat, ArrayNode, ArrayNodeKind};
    use gpui::{Modifiers, TestAppContext, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn preview(path: &str, kind: PreviewKind, rows: usize, cols: usize) -> ArrayPreview {
        #[allow(clippy::cast_precision_loss)]
        let values = (0..rows * cols)
            .map(|i| (i != 2).then_some(i as f64))
            .collect();
        let mut stats = Computed::new(
            "ar_1",
            "array.preview_stats",
            "stats",
            Engine {
                name: "crest-array".into(),
                version: "0".into(),
                backend: None,
                platform: None,
            },
        );
        stats.values.insert("n".into(), ComputedValue::new(5.0));
        stats.values.insert("mean".into(), ComputedValue::new(2.5));
        ArrayPreview {
            path: path.into(),
            kind,
            selection: format!("[0:{rows}, 0:{cols}]"),
            rows,
            cols,
            values,
            texts: Vec::new(),
            decoding: Some("fill -1 left out".into()),
            stats,
        }
    }

    fn spec() -> ArrayFileSpec {
        let node = |path: &str, depth, kind, shape: Vec<u64>| ArrayNode {
            path: path.into(),
            depth,
            kind,
            dtype: (kind == ArrayNodeKind::Variable).then(|| "f64".into()),
            shape,
            dims: Vec::new(),
            chunks: None,
            filters: Vec::new(),
            attrs: Vec::new(),
        };
        ArrayFileSpec {
            title: None,
            path: "data/run.h5".into(),
            format: ArrayFormat::Hdf5,
            size: 12_335,
            nodes: vec![
                node("/", 0, ArrayNodeKind::Group, vec![]),
                node("/field", 1, ArrayNodeKind::Variable, vec![2, 3]),
                node("/series", 1, ArrayNodeKind::Variable, vec![6]),
            ],
            truncated: false,
            previews: vec![
                preview("/field", PreviewKind::Heatmap, 2, 3),
                preview("/series", PreviewKind::Line, 1, 6),
            ],
            warnings: vec!["note".into()],
        }
    }

    #[test]
    fn texts_and_line_plot() {
        assert_eq!(
            summary_text(&spec()),
            "HDF5 · 12.0 KiB · 1 groups · 2 variables · 2 previews"
        );
        assert_eq!(
            stats_text(&spec().previews[0]),
            "Of this preview only ([0:2, 0:3]): n 5 · mean 2.5"
        );
        let plot = line_plot(&spec().previews[1], Some("K"));
        assert_eq!(
            plot.series[0].x,
            vec![0.0, 1.0, 3.0, 4.0, 5.0],
            "missing left out"
        );
        assert_eq!(plot.y_label.as_deref(), Some("K"));
        plot.validate().unwrap();
    }

    #[gpui::test]
    fn draws_tree_heatmap_and_switches_previews(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (timeline, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::ArrayFile(Box::new(spec())));
        let id = block.id;
        timeline.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1400.)), |_, _| {
            div().size_full().child(timeline.clone())
        });
        cx.run_until_parked();
        for selector in [
            "array-file",
            "array-summary",
            "tree",
            "array-preview-0",
            "array-preview-1",
            "array-heatmap",
            "array-stats",
            "array-warnings",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let view = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        let heat = cx.debug_bounds("array-heatmap").unwrap();
        cx.simulate_mouse_move(
            heat.origin + point(px(5.0), px(5.0)),
            None,
            Modifiers::none(),
        );
        assert_eq!(view.read_with(cx, |v, _| v.hover()), Some((0, 0)));
        assert!(cx.debug_bounds("array-cell").is_some());
        let chip = cx.debug_bounds("array-preview-1").unwrap().center();
        cx.simulate_click(chip, Modifiers::none());
        assert_eq!(view.read_with(cx, |v, _| v.shown()), 1);
        assert_eq!(view.read_with(cx, |v, _| v.tree().nodes.len()), 1);
    }
}
