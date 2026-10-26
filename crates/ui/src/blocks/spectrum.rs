//! Renderer of the `spectrum` block kind (`show_spectrum`): a spectrum on
//! the plot painter with its picked peaks.
//!
//! The trace is reduced to two points per pixel column of the visible
//! range (minimum and maximum, so no peak is lost) and drawn as a line;
//! sticks (centroided MS, peak tables) are drawn as bars from zero. The
//! picked peaks are dots over it. IR wavenumbers and NMR shifts run right
//! to left. Zoom in and out around the centre, pan, fit, and a click on a
//! peak in the list zooms to it; y fits the visible points. The header
//! gives the kind, file metadata and the envelope's provenance.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::plot::{PlotRange, PlotSpec, SeriesStyle, XySeries};
use domain::science::spectrum::{PeakSense, SpectrumSpec, decimate, range};
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::computed::provenance_text;
use crate::timeline::TimelineEntry;
use crate::widgets::{readout, tool_button};

/// Pixel columns the trace is reduced to.
pub const COLUMNS: usize = 1_200;
/// Zoom step of the buttons.
const ZOOM: f64 = 2.0;
/// Share of the full range a peak is zoomed to.
const PEAK_SPAN: f64 = 0.04;
/// Peaks listed.
const LISTED: usize = 30;

/// The plot of `spec` over x `view` (low, high), with the peaks when
/// `peaks`.
#[must_use]
pub fn plot_spec(spec: &SpectrumSpec, view: (f64, f64), peaks: bool) -> PlotSpec {
    let (lo, hi) = view;
    let (xs, ys) = if spec.sticks {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for (x, y) in spec.x.iter().zip(&spec.y) {
            if *x >= lo && *x <= hi {
                // A bar from the baseline and back.
                xs.extend([*x, *x, *x]);
                ys.extend([0.0, *y, 0.0]);
            }
        }
        (xs, ys)
    } else {
        decimate(&spec.x, &spec.y, lo, hi, COLUMNS)
    };
    let (mut y_lo, mut y_hi) = range(&ys).unwrap_or((0.0, 1.0));
    if spec.sticks {
        y_lo = y_lo.min(0.0);
    }
    let pad = ((y_hi - y_lo) * 0.06).max(1e-12);
    y_lo -= pad;
    y_hi += pad;
    let mut series = vec![XySeries {
        name: spec.kind.name().to_owned(),
        x: xs,
        y: ys,
        style: SeriesStyle::Line,
    }];
    if peaks {
        let shown: Vec<_> = spec
            .peaks
            .iter()
            .filter(|p| p.x >= lo && p.x <= hi)
            .collect();
        if !shown.is_empty() {
            series.push(XySeries {
                name: "peaks".into(),
                x: shown.iter().map(|p| p.x).collect(),
                y: shown.iter().map(|p| p.y).collect(),
                style: SeriesStyle::Scatter,
            });
        }
    }
    PlotSpec {
        title: None,
        x_range: (hi > lo).then_some(PlotRange { min: lo, max: hi }),
        y_range: (y_hi > y_lo).then_some(PlotRange {
            min: y_lo,
            max: y_hi,
        }),
        x_label: Some(spec.x_label.clone()),
        y_label: Some(spec.y_label.clone()),
        series,
        x_reversed: spec.x_reversed,
        ..PlotSpec::default()
    }
}

/// The header line: kind, format, points and metadata.
#[must_use]
pub fn summary_text(spec: &SpectrumSpec) -> String {
    let mut parts = vec![format!(
        "{} · {} {}",
        spec.kind.name(),
        spec.x.len(),
        if spec.sticks { "sticks" } else { "points" }
    )];
    if spec.blocks > 1 {
        parts.push(format!("block {} of {}", spec.block + 1, spec.blocks));
    }
    for (k, v) in &spec.metadata {
        parts.push(format!("{k}: {v}"));
    }
    parts.join(" · ")
}

/// Per-block state: the visible x range and whether peaks are drawn.
pub struct SpectrumView {
    id: BlockId,
    spec: Rc<SpectrumSpec>,
    full: (f64, f64),
    view: (f64, f64),
    peaks: bool,
}

impl SpectrumView {
    fn new(id: BlockId, spec: &SpectrumSpec) -> Self {
        let full = spec.x_range().unwrap_or((0.0, 1.0));
        Self {
            id,
            spec: Rc::new(spec.clone()),
            full,
            view: full,
            peaks: true,
        }
    }

    fn sync(&mut self, spec: &SpectrumSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The visible x range.
    pub fn view(&self) -> (f64, f64) {
        self.view
    }

    /// Shows `lo..hi`, kept inside the data.
    pub fn set_view(&mut self, lo: f64, hi: f64, cx: &mut Context<Self>) {
        let (a, b) = self.full;
        let span = (hi - lo).clamp((b - a) * 1e-6, b - a);
        let lo = lo.clamp(a, b - span);
        self.view = (lo, lo + span);
        cx.notify();
    }

    /// Zooms by `factor` (above 1 zooms in) around the centre.
    pub fn zoom(&mut self, factor: f64, cx: &mut Context<Self>) {
        let (lo, hi) = self.view;
        let centre = (lo + hi) / 2.0;
        let half = (hi - lo) / factor / 2.0;
        self.set_view(centre - half, centre + half, cx);
    }

    /// Moves the view by `share` of its width towards the screen's right
    /// (`share` > 0) or left, whichever way the axis runs.
    pub fn pan(&mut self, share: f64, cx: &mut Context<Self>) {
        let (lo, hi) = self.view;
        let step = (hi - lo) * share * if self.spec.x_reversed { -1.0 } else { 1.0 };
        self.set_view(lo + step, hi + step, cx);
    }

    /// The whole spectrum again.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.view = self.full;
        cx.notify();
    }

    /// Zooms to peak `ix`.
    pub fn focus_peak(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(p) = self.spec.peaks.get(ix) {
            let half = (self.full.1 - self.full.0) * PEAK_SPAN / 2.0;
            self.set_view(p.x - half, p.x + half, cx);
        }
    }

    fn peak_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let rows: Vec<AnyElement> = self
            .spec
            .peaks
            .iter()
            .enumerate()
            .take(LISTED)
            .map(|(i, p)| {
                let prominence = p
                    .prominence
                    .map(|v| format!(" · prominence {v:.4}"))
                    .unwrap_or_default();
                h_flex()
                    .id(("spectrum-peak", i))
                    .debug_selector(move || format!("spectrum-peak-{i}"))
                    .gap_2()
                    .px_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.focus_peak(i, cx);
                    }))
                    .child(div().w_6().text_color(muted).child(format!("{}.", i + 1)))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{:.4}", p.x).trim_end_matches('0').trim_end_matches('.').to_owned()),
                    )
                    .child(div().text_color(muted).child(format!("y {:.4}{prominence}", p.y)))
                    .into_any_element()
            })
            .collect();
        let more = self.spec.peaks.len().saturating_sub(LISTED);
        v_flex()
            .debug_selector(|| "spectrum-peaks".to_owned())
            .text_xs()
            .border_t_1()
            .border_color(border)
            .pt_1()
            .children(rows)
            .when(more > 0, |d| d.child(div().text_color(muted).child(format!("… {more} more"))))
    }
}

impl Render for SpectrumView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = Rc::clone(&self.spec);
        let plot = plot_spec(&spec, self.view, self.peaks);
        let canvas = super::plot::static_plot(&plot, &format!("spectrum-{}", self.id), window, cx);
        let peak_list = self.peak_list(cx);
        let theme = cx.theme();
        let (muted, warning) = (theme.muted_foreground, theme.warning);
        let zoomed = self.view != self.full;
        let what = match (spec.sticks, spec.sense) {
            (true, _) => "strongest sticks",
            (false, PeakSense::Minima) => "minima",
            (false, PeakSense::Maxima) => "maxima",
        };
        let (lo, hi) = self.view;
        let toolbar = h_flex()
            .gap_1()
            .text_xs()
            .child(
                tool_button("spectrum-zoom-out", "−", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.0 / ZOOM, cx))),
            )
            .child(
                tool_button("spectrum-zoom-in", "+", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(ZOOM, cx))),
            )
            .child(
                tool_button("spectrum-pan-left", "◀", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.pan(-0.25, cx))),
            )
            .child(
                tool_button("spectrum-pan-right", "▶", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.pan(0.25, cx))),
            )
            .child(
                tool_button("spectrum-fit", "fit", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
            .child(
                tool_button(
                    "spectrum-peaks-toggle",
                    if self.peaks { "peaks: on" } else { "peaks: off" },
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.peaks = !this.peaks;
                    cx.notify();
                })),
            )
            .when(zoomed, |row| {
                row.child(
                    readout(format!("{lo:.4} – {hi:.4}"), cx)
                        .debug_selector(|| "spectrum-range".to_owned()),
                )
            });
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "spectrum".to_owned())
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(spec.display_title())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "spectrum-summary".to_owned())
                    .child(SharedString::from(summary_text(&spec))),
            )
            .child(toolbar)
            .child(
                div()
                    .w_full()
                    .debug_selector(|| "spectrum-canvas".to_owned())
                    .child(canvas),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "spectrum-provenance".to_owned())
                    .child(SharedString::from(format!(
                        "{} peaks ({what}) · {} · {}",
                        spec.peaks.len(),
                        spec.picking.method.name,
                        provenance_text(&spec.picking)
                    ))),
            )
            .when(!spec.peaks.is_empty(), |d| d.child(peak_list))
            .when(!spec.warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(warning)
                        .children(spec.warnings.iter().take(6).map(|w| div().child(w.clone()))),
                )
            })
    }
}

/// The spectrum entity of block `id`, in sync with `spec`.
pub fn view(id: BlockId, spec: &SpectrumSpec, window: &mut Window, cx: &mut App) -> Entity<SpectrumView> {
    let view = super::block_state(id, window, cx, |_, _| SpectrumView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Spectrum(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::computed::{Computed, Engine};
    use domain::science::spectrum::{Peak, SpectrumFormat, SpectrumKind};
    use gpui::{Modifiers, TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> SpectrumSpec {
        let x: Vec<f64> = (0..=1800).map(|i| 4000.0 - 2.0 * f64::from(i)).collect();
        let y: Vec<f64> = x
            .iter()
            .map(|x| 0.95 - 0.7 / (1.0 + ((x - 1716.0) / 7.5).powi(2)))
            .collect();
        SpectrumSpec {
            title: None,
            path: "spectra/ketone.jdx".into(),
            format: SpectrumFormat::Jcamp,
            kind: SpectrumKind::Infrared,
            name: Some("ketone".into()),
            x_label: "wavenumber (cm⁻¹)".into(),
            y_label: "transmittance".into(),
            x_reversed: true,
            x,
            y,
            sticks: false,
            peaks: vec![Peak {
                x: 1716.0,
                y: 0.25,
                prominence: Some(0.7),
            }],
            sense: PeakSense::Minima,
            picking: Computed::new(
                "ch_1",
                "chem.peak_picking",
                "Peak picking by prominence",
                Engine {
                    name: "crest-chem".into(),
                    version: "0.14.0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            metadata: vec![("origin".into(), "test".into())],
            blocks: 1,
            block: 0,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn the_plot_is_decimated_reversed_and_has_peaks() {
        let s = spec();
        let plot = plot_spec(&s, (400.0, 4000.0), true);
        plot.validate().unwrap();
        assert!(plot.x_reversed);
        assert!(plot.series[0].x.len() <= 2 * COLUMNS + 2);
        assert_eq!(plot.series[1].x, [1716.0]);
        let zoomed = plot_spec(&s, (3000.0, 3500.0), true);
        assert_eq!(zoomed.series.len(), 1, "the peak is out of view");
        let mut sticks = s.clone();
        sticks.sticks = true;
        sticks.x = vec![43.0, 58.0];
        sticks.y = vec![100.0, 64.0];
        let bars = plot_spec(&sticks, (40.0, 60.0), false);
        assert_eq!(bars.series[0].y, [0.0, 100.0, 0.0, 0.0, 64.0, 0.0]);
        assert_eq!(summary_text(&s), "IR · 1801 points · origin: test");
    }

    #[gpui::test]
    fn draws_zooms_and_focuses_a_peak(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Spectrum(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1200.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        };
        draw(cx);
        for selector in [
            "spectrum",
            "spectrum-summary",
            "spectrum-provenance",
            "spectrum-peaks",
            "spectrum-peak-0",
            "spectrum-canvas",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let peak = cx.debug_bounds("spectrum-peak-0").unwrap().center();
        cx.simulate_click(peak, Modifiers::none());
        let s = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        let (lo, hi) = s.read_with(cx, |v, _| v.view());
        assert!(lo < 1716.0 && hi > 1716.0 && hi - lo < 200.0, "{lo} {hi}");
        s.update(cx, |v, cx| v.fit(cx));
        s.update(cx, |v, cx| v.zoom(2.0, cx));
        let (lo, hi) = s.read_with(cx, |v, _| v.view());
        assert!((hi - lo - 1800.0).abs() < 1e-9);
        // Panning right on a reversed axis goes to lower wavenumbers.
        s.update(cx, |v, cx| v.pan(0.25, cx));
        assert!(s.read_with(cx, |v, _| v.view()).0 < lo);
        draw(cx);
        assert!(cx.debug_bounds("spectrum-range").is_some());
    }
}
