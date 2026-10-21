//! Painting a [`ChartLayout`]: the grid, the baseline, the bars and the
//! labels. Positions in the layout are relative to `origin`, the chart's
//! top-left corner on screen.

use gpui::{
    App, Bounds, Corners, Font, Hsla, Pixels, Point, SharedString, TextAlign, TextRun, Window,
    fill, point, px, size,
};

use super::layout::{Align, BarMark, ChartLayout, LABEL_HEIGHT, Label, Orientation, Rect};

/// Corner radius at the data end of a bar.
pub const BAR_RADIUS: f32 = 4.0;

/// The font labels are drawn and measured with, captured when the chart is
/// rendered.
#[derive(Clone)]
pub struct LabelFont {
    pub font: Font,
    pub size: Pixels,
}

impl LabelFont {
    /// Width in pixels of `text` in this font.
    pub fn measure(&self, text: &str, window: &Window) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        self.shape(text, gpui::black(), window).width().as_f32()
    }

    fn shape(&self, text: &str, color: Hsla, window: &Window) -> gpui::ShapedLine {
        let run = TextRun {
            len: text.len(),
            font: self.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window.text_system().shape_line(
            SharedString::from(text.to_owned()),
            self.size,
            &[run],
            None,
        )
    }

    /// Draws `label` in `color`, positions relative to `origin`.
    pub fn paint(
        &self,
        label: &Label,
        color: Hsla,
        origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if label.text.is_empty() {
            return;
        }
        let line = self.shape(&label.text, color, window);
        let w = line.width().as_f32();
        let left = match label.align {
            Align::Left => label.x,
            Align::Center => label.x - w / 2.0,
            Align::Right => label.x - w,
        };
        let at = origin + point(px(left), px(label.y));
        // A failed glyph lookup leaves the label out; nothing else to do.
        line.paint(at, px(LABEL_HEIGHT), TextAlign::Left, None, window, cx)
            .ok();
    }
}

/// `rect` on screen, relative to `origin`.
pub fn to_bounds(rect: &Rect, origin: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        origin + point(px(rect.x), px(rect.y)),
        size(px(rect.w), px(rect.h)),
    )
}

/// The grid lines at every tick and the baseline at zero.
pub fn paint_grid(
    layout: &ChartLayout,
    origin: Point<Pixels>,
    grid: Hsla,
    baseline: Hsla,
    window: &mut Window,
) {
    let plot = &layout.plot;
    let line = |pos: f32| match layout.orientation {
        Orientation::Vertical => Rect::new(plot.x, pos.round(), plot.w, 1.0),
        Orientation::Horizontal => Rect::new(pos.round(), plot.y, 1.0, plot.h),
    };
    for tick in &layout.ticks {
        window.paint_quad(fill(to_bounds(&line(tick.pos), origin), grid));
    }
    if let Some(base) = layout.baseline {
        window.paint_quad(fill(to_bounds(&line(base), origin), baseline));
    }
}

/// Corner radii of a bar: rounded only at its data end, flat at the base.
pub fn bar_corners(bar: &BarMark, orientation: Orientation) -> Corners<Pixels> {
    let r = px(BAR_RADIUS
        .min(bar.rect.w / 2.0)
        .min(bar.rect.h / 2.0)
        .max(0.0));
    let zero = px(0.);
    let mut corners = Corners::all(zero);
    match (orientation, bar.negative) {
        (Orientation::Vertical, false) => {
            corners.top_left = r;
            corners.top_right = r;
        }
        (Orientation::Vertical, true) => {
            corners.bottom_left = r;
            corners.bottom_right = r;
        }
        (Orientation::Horizontal, false) => {
            corners.top_right = r;
            corners.bottom_right = r;
        }
        (Orientation::Horizontal, true) => {
            corners.top_left = r;
            corners.bottom_left = r;
        }
    }
    corners
}

/// Every bar in its series color.
pub fn paint_bars(
    layout: &ChartLayout,
    origin: Point<Pixels>,
    color: impl Fn(usize) -> Hsla,
    window: &mut Window,
) {
    for bar in &layout.bars {
        if bar.rect.w <= 0.0 || bar.rect.h <= 0.0 {
            continue;
        }
        let quad = fill(to_bounds(&bar.rect, origin), color(bar.series))
            .corner_radii(bar_corners(bar, layout.orientation));
        window.paint_quad(quad);
    }
}

/// Tick, category and value labels.
pub fn paint_labels(
    layout: &ChartLayout,
    origin: Point<Pixels>,
    font: &LabelFont,
    muted: Hsla,
    strong: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    for label in layout.tick_labels.iter().chain(&layout.category_labels) {
        font.paint(label, muted, origin, window, cx);
    }
    for label in &layout.value_labels {
        font.paint(label, strong, origin, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(negative: bool) -> BarMark {
        BarMark {
            label: 0,
            series: 0,
            value: if negative { -1.0 } else { 1.0 },
            rect: Rect::new(0.0, 0.0, 20.0, 50.0),
            negative,
        }
    }

    #[test]
    fn only_the_data_end_is_rounded() {
        let up = bar_corners(&bar(false), Orientation::Vertical);
        assert_eq!(up.top_left, px(BAR_RADIUS));
        assert_eq!(up.bottom_left, px(0.));
        let down = bar_corners(&bar(true), Orientation::Vertical);
        assert_eq!(down.bottom_right, px(BAR_RADIUS));
        assert_eq!(down.top_right, px(0.));
        let left = bar_corners(&bar(true), Orientation::Horizontal);
        assert_eq!(left.top_left, px(BAR_RADIUS));
        assert_eq!(left.top_right, px(0.));
    }
}
