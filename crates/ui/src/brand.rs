//! The crest mark: the two peaks of the TwoCrest logo, painted as vector
//! paths so it needs no image assets and follows the theme (the back peak
//! takes the foreground color, the front one the brand accent).

use gpui::{
    App, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Styled as _, Window, canvas, point, px,
    rgb,
};
use gpui_component::ActiveTheme as _;

/// The brand accent of the front peak (`#D9604A`), the same in both themes.
pub const ACCENT: u32 = 0xD9604A;

/// The logo's drawing box, in the units of its SVG: x 2..126, y 2..78.
const VIEW: (f32, f32, f32, f32) = (2.0, 2.0, 124.0, 76.0);
/// The back peak.
const BACK: [(f32, f32); 3] = [(4.0, 74.0), (37.0, 24.0), (70.0, 74.0)];
/// The front peak, cut out of the back one by a gap of [`GAP`] units.
const FRONT: [(f32, f32); 3] = [(46.0, 74.0), (86.0, 6.0), (124.0, 74.0)];
/// Width of the gap between the peaks.
const GAP: f32 = 6.0;

/// The mark, `height` pixels tall.
pub fn mark(height: f32) -> impl IntoElement {
    let scale = height / VIEW.3;
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            paint(bounds, scale, window, cx);
        },
    )
    .w(px(VIEW.2 * scale))
    .h(px(height))
}

fn paint(bounds: Bounds<Pixels>, scale: f32, window: &mut Window, cx: &mut App) {
    let theme = cx.theme();
    let (back, gap, front): (Hsla, Hsla, Hsla) =
        (theme.foreground, theme.background, rgb(ACCENT).into());
    let at = |(x, y): (f32, f32)| {
        point(
            bounds.origin.x + px((x - VIEW.0) * scale),
            bounds.origin.y + px((y - VIEW.1) * scale),
        )
    };
    let triangle = |mut builder: PathBuilder, points: &[(f32, f32); 3]| {
        builder.move_to(at(points[0]));
        builder.line_to(at(points[1]));
        builder.line_to(at(points[2]));
        builder.close();
        builder.build().ok()
    };
    if let Some(path) = triangle(PathBuilder::fill(), &BACK) {
        window.paint_path(path, back);
    }
    if let Some(path) = triangle(PathBuilder::stroke(px(GAP * scale)), &FRONT) {
        window.paint_path(path, gap);
    }
    if let Some(path) = triangle(PathBuilder::fill(), &FRONT) {
        window.paint_path(path, front);
    }
}
