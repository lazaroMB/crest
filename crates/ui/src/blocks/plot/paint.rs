//! Painting a plot: the grid at the ticks (fainter minor lines on log
//! axes), the zero lines, every layer (curves as GPUI paths, points as
//! ringed dots) clipped to the plot area, and the labels. Positions in the layout are relative to `origin`, the
//! element's top-left corner on screen.

use gpui::{
    App, BorderStyle, Bounds, ContentMask, Hsla, PathBuilder, Pixels, Point, Window, fill, point,
    px, quad, size,
};

use super::axes::PlotLayout;
use super::sample::{Mark, Pt, Scene};
use crate::blocks::chart::layout::Rect;
use crate::blocks::chart::paint::{LabelFont, to_bounds};

/// Width of a curve.
pub const LINE_WIDTH: f32 = 2.0;
/// Diameter of a data point.
pub const POINT_SIZE: f32 = 6.0;
/// Width of the surface-colored ring around a point, when there are few.
pub const POINT_RING: f32 = 1.5;
/// Points beyond which they are drawn without a ring.
pub const MAX_RINGED_POINTS: usize = 500;

/// Grid lines at the ticks, then the axes through zero when it is in view.
pub fn paint_grid(
    layout: &PlotLayout,
    origin: Point<Pixels>,
    grid: Hsla,
    axis: Hsla,
    window: &mut Window,
) {
    let plot = &layout.plot;
    let minor = grid.opacity(0.45);
    for &x in &layout.x_minor {
        let line = Rect::new(x.round(), plot.y, 1.0, plot.h);
        window.paint_quad(fill(to_bounds(&line, origin), minor));
    }
    for &y in &layout.y_minor {
        let line = Rect::new(plot.x, y.round(), plot.w, 1.0);
        window.paint_quad(fill(to_bounds(&line, origin), minor));
    }
    for tick in &layout.x_ticks {
        let line = Rect::new(tick.pos.round(), plot.y, 1.0, plot.h);
        window.paint_quad(fill(to_bounds(&line, origin), grid));
    }
    for tick in &layout.y_ticks {
        let line = Rect::new(plot.x, tick.pos.round(), plot.w, 1.0);
        window.paint_quad(fill(to_bounds(&line, origin), grid));
    }
    // Zero lines on linear axes (0 on a log axis is the value 1).
    let view = layout.view;
    if layout.axes.x.is_linear() && view.x.0 < 0.0 && view.x.1 > 0.0 {
        let x = layout.to_px((0.0, view.y.0)).0.round();
        window.paint_quad(fill(
            to_bounds(&Rect::new(x, plot.y, 1.0, plot.h), origin),
            axis,
        ));
    }
    if layout.axes.y.is_linear() && view.y.0 < 0.0 && view.y.1 > 0.0 {
        let y = layout.to_px((view.x.0, 0.0)).1.round();
        window.paint_quad(fill(
            to_bounds(&Rect::new(plot.x, y, plot.w, 1.0), origin),
            axis,
        ));
    }
    // The bottom edge, so the x labels sit on a line even off zero.
    let bottom = Rect::new(plot.x, plot.bottom().round(), plot.w, 1.0);
    window.paint_quad(fill(to_bounds(&bottom, origin), axis));
}

/// Strokes `lines` as one path in `color`.
fn stroke<'a>(
    lines: impl IntoIterator<Item = &'a [Pt]>,
    layout: &PlotLayout,
    origin: Point<Pixels>,
    color: Hsla,
    window: &mut Window,
) {
    let at = |p: Pt| {
        let (x, y) = layout.to_px(p);
        origin + point(px(x), px(y))
    };
    let mut path = PathBuilder::stroke(px(LINE_WIDTH));
    let mut any = false;
    for line in lines {
        let Some((&first, rest)) = line.split_first() else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        path.move_to(at(first));
        for &p in rest {
            path.line_to(at(p));
        }
        any = true;
    }
    if any && let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Every layer of `scene` in its palette color, clipped to the plot area.
pub fn paint_layers(
    scene: &Scene,
    layout: &PlotLayout,
    origin: Point<Pixels>,
    color: impl Fn(usize) -> Hsla,
    surface: Hsla,
    window: &mut Window,
) {
    let plot = &layout.plot;
    let mask = ContentMask {
        bounds: Bounds::new(
            origin + point(px(plot.x - 1.0), px(plot.y - 1.0)),
            size(px(plot.w + 2.0), px(plot.h + 2.0)),
        ),
    };
    window.with_content_mask(Some(mask), |window| {
        for layer in &scene.layers {
            let color = color(layer.color);
            let color = if layer.faint {
                color.opacity(0.3)
            } else {
                color
            };
            match &layer.mark {
                Mark::Lines(lines) => {
                    stroke(
                        lines.iter().map(Vec::as_slice),
                        layout,
                        origin,
                        color,
                        window,
                    );
                }
                Mark::Segments(segments) => {
                    stroke(
                        segments.iter().map(|s| s.as_slice()),
                        layout,
                        origin,
                        color,
                        window,
                    );
                }
                Mark::Points(points) => {
                    let ring = if points.len() <= MAX_RINGED_POINTS {
                        POINT_RING
                    } else {
                        0.0
                    };
                    let outer = POINT_SIZE + 2.0 * ring;
                    for &p in points {
                        let (x, y) = layout.to_px(p);
                        if !plot.contains(x, y) {
                            continue;
                        }
                        let rect = Rect::new(x - outer / 2.0, y - outer / 2.0, outer, outer);
                        window.paint_quad(quad(
                            to_bounds(&rect, origin),
                            px(outer / 2.0),
                            color,
                            px(ring),
                            surface,
                            BorderStyle::Solid,
                        ));
                    }
                }
            }
        }
    });
}

/// Tick labels and axis titles.
pub fn paint_labels(
    layout: &PlotLayout,
    origin: Point<Pixels>,
    font: &LabelFont,
    muted: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    for label in &layout.labels {
        font.paint(label, muted, origin, window, cx);
    }
}
