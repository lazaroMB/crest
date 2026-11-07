//! The welcome tour drawn over the window.

use gpui::{
    AnyElement, Bounds, BoxShadow, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, PathBuilder, Pixels, Point, Size, Styled as _,
    Window, canvas, div, hsla, point, px,
};
use gpui_component::{h_flex, v_flex};

use super::anchors::Frames;
use super::state::{Action, Box2, Side, Step, place, shade};
use crate::theme::token;

/// Something that can answer a button on the tour card.
pub trait Host: Sized + 'static {
    /// `action` was chosen.
    fn tour_act(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>);
}

/// Colors of one card, taken from the theme.
struct Ink {
    accent: Hsla,
    accent_fg: Hsla,
    popover: Hsla,
    foreground: Hsla,
    muted: Hsla,
    button: Hsla,
    border: Hsla,
}

const CARD_W: f32 = 400.0;
const CARD_H: f32 = 300.0;
const CENTER_W: f32 = 440.0;
const CENTER_H: f32 = 360.0;

/// The tour for `step`, using the regions in `frames`.
pub fn layer<T: Host>(
    step: Step,
    frames: &Frames,
    viewport: Size<Pixels>,
    cx: &mut Context<T>,
) -> AnyElement {
    let window = Box2 {
        x: 0.0,
        y: 0.0,
        w: f32::from(viewport.width),
        h: f32::from(viewport.height),
    };
    let hole = step
        .anchor()
        .and_then(|anchor| frames.get(anchor).map(box2));
    let (card_w, card_h) = if step.anchor().is_none() {
        (CENTER_W, CENTER_H)
    } else {
        (CARD_W, CARD_H)
    };
    let card = place(window, hole, step.side(), card_w, card_h);
    let ink = Ink {
        accent: token(cx, "accent.bg"),
        accent_fg: token(cx, "accent.fg"),
        popover: token(cx, "popover.bg"),
        foreground: token(cx, "foreground"),
        muted: token(cx, "muted.fg"),
        button: token(cx, "button.bg"),
        border: token(cx, "border"),
    };
    let dim = token(cx, "background").opacity(0.72);
    let copy = step.copy();
    let mut root = div()
        .id("tour")
        .debug_selector(|| "tour".to_owned())
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .size_full();
    if let Some(hole) = hole {
        for band in shade(window, hole) {
            root = root.child(dim_band(band, dim));
        }
        root = root.child(ring(hole, ink.accent)).child(catcher(hole, cx));
    } else {
        root = root.child(dim_band(window, dim));
    }
    let mut root = root.child(card_view(step, card, copy.title, copy.body, &ink, cx));
    if step.side() == Side::Above {
        root = root.child(
            div()
                .absolute()
                .left(px(card.x + card.w / 2.0 - 8.0))
                .top(px(card.y + card.h - 2.0))
                .text_color(ink.accent)
                .child("▼"),
        );
    }
    root.into_any_element()
}

fn box2(bounds: Bounds<Pixels>) -> Box2 {
    Box2 {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        w: f32::from(bounds.size.width),
        h: f32::from(bounds.size.height),
    }
}

fn at(box2: Box2) -> gpui::Div {
    div()
        .absolute()
        .left(px(box2.x))
        .top(px(box2.y))
        .w(px(box2.w))
        .h(px(box2.h))
}

fn dim_band(band: Box2, color: gpui::Hsla) -> AnyElement {
    at(band)
        .bg(color)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .into_any_element()
}

fn ring(hole: Box2, accent: Hsla) -> AnyElement {
    // The glow is a stroke around the rim. A shadow on the whole rectangle
    // fills the panel; the center has to stay clear.
    const PAD: f32 = 24.0;
    let frame = Box2 {
        x: hole.x - PAD,
        y: hole.y - PAD,
        w: hole.w + PAD * 2.0,
        h: hole.h + PAD * 2.0,
    };
    at(frame)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| glow_rim(bounds, PAD, accent, window),
            )
            .size_full(),
        )
        .into_any_element()
}

fn glow_rim(bounds: Bounds<Pixels>, inset: f32, accent: Hsla, window: &mut gpui::Window) {
    let layers = [(22.0, 0.08), (12.0, 0.16), (6.0, 0.38), (1.6, 0.92)];
    for (width, alpha) in layers {
        let mut path = PathBuilder::stroke(px(width));
        round_rect(&mut path, bounds, inset, 12.0);
        if let Ok(path) = path.build() {
            window.paint_path(path, accent.opacity(alpha));
        }
    }
}

fn round_rect(path: &mut PathBuilder, bounds: Bounds<Pixels>, inset: f32, radius: f32) {
    let x = f32::from(bounds.origin.x) + inset;
    let y = f32::from(bounds.origin.y) + inset;
    let w = (f32::from(bounds.size.width) - inset * 2.0).max(0.0);
    let h = (f32::from(bounds.size.height) - inset * 2.0).max(0.0);
    if w < 2.0 || h < 2.0 {
        return;
    }
    let r = radius.min(w / 2.0).min(h / 2.0);
    let p = |x: f32, y: f32| point(px(x), px(y));
    path.move_to(p(x + r, y));
    path.line_to(p(x + w - r, y));
    path.arc_to(p(r, r), px(0.), false, true, p(x + w, y + r));
    path.line_to(p(x + w, y + h - r));
    path.arc_to(p(r, r), px(0.), false, true, p(x + w - r, y + h));
    path.line_to(p(x + r, y + h));
    path.arc_to(p(r, r), px(0.), false, true, p(x, y + h - r));
    path.line_to(p(x, y + r));
    path.arc_to(p(r, r), px(0.), false, true, p(x + r, y));
    path.close();
}

fn catcher<T: Host>(hole: Box2, cx: &mut Context<T>) -> AnyElement {
    at(hole)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_: &mut T, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
        )
        .into_any_element()
}

fn card_view<T: Host>(
    step: Step,
    card: Box2,
    title: &str,
    body: &str,
    ink: &Ink,
    cx: &mut Context<T>,
) -> AnyElement {
    let mut column = v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .px_5()
        .py_4()
        .text_center();
    if let Some((n, total)) = step.progress() {
        column = column.child(progress_pill(n, total, ink.accent));
    }
    if matches!(step, Step::Welcome | Step::Done) {
        column = column.child(mark(step, ink));
    }
    column = column
        .child(
            div()
                .w_full()
                .text_xl()
                .text_center()
                .font_weight(FontWeight::BOLD)
                .text_color(ink.foreground)
                .child(title.to_owned()),
        )
        .child(
            div()
                .w_full()
                .text_sm()
                .text_center()
                .text_color(ink.muted)
                .child(body.to_owned()),
        )
        .child(buttons(step, ink, cx));
    if let Some(footer) = step.footer() {
        column = column.child(
            div()
                .w_full()
                .text_xs()
                .text_center()
                .text_color(ink.muted)
                .child(footer.to_owned()),
        );
    }
    at(card)
        .id("tour-card")
        .rounded(px(18.))
        .shadow(card_glow(ink.accent))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .size_full()
                .rounded(px(18.))
                .overflow_hidden()
                .border_1()
                .border_color(ink.accent.opacity(0.85))
                .bg(ink.popover)
                .child(sweep(ink.accent))
                .child(column),
        )
        .child(close_button(ink.muted, cx))
        .into_any_element()
}

fn card_glow(accent: Hsla) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(0.), accent.opacity(0.45))
            .blur_radius(px(18.))
            .spread_radius(px(1.)),
        BoxShadow::new(px(0.), px(18.), hsla(0.0, 0.0, 0.0, 0.45)).blur_radius(px(28.)),
    ]
}

fn sweep(accent: Hsla) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint_waves(bounds, accent, window),
    )
    .absolute()
    .top(px(0.))
    .left(px(0.))
    .size_full()
    .into_any_element()
}

/// Three nested waves in the top-left, each with its own edge, so the
/// corner reads as stacked curves instead of one blob.
fn paint_waves(bounds: Bounds<Pixels>, accent: Hsla, window: &mut gpui::Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let waves = [
        (0.22, 0.42, 0.20, 0.32, 0.02, 0.08, 0.22),
        (0.48, 0.28, 0.12, 0.20, 0.0, 0.05, 0.13),
        (0.72, 0.15, 0.06, 0.11, 0.0, 0.03, 0.07),
    ];
    for (alpha, top, drop, cx, cy, dx, dy) in waves {
        fill_wave(
            window,
            bounds.origin,
            accent.opacity(alpha),
            w * top,
            h * drop,
            (w * cx, h * cy),
            (w * dx, h * dy),
        );
        stroke_wave(
            window,
            bounds.origin,
            accent.opacity((alpha + 0.28).min(0.9)),
            w * top,
            h * drop,
            (w * cx, h * cy),
            (w * dx, h * dy),
        );
    }
}

fn fill_wave(
    window: &mut gpui::Window,
    origin: Point<Pixels>,
    color: Hsla,
    top: f32,
    drop: f32,
    ctrl_a: (f32, f32),
    ctrl_b: (f32, f32),
) {
    let p = |x: f32, y: f32| origin + point(px(x), px(y));
    let mut path = PathBuilder::fill();
    path.move_to(p(0.0, 0.0));
    path.line_to(p(top, 0.0));
    path.cubic_bezier_to(p(0.0, drop), p(ctrl_a.0, ctrl_a.1), p(ctrl_b.0, ctrl_b.1));
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn stroke_wave(
    window: &mut gpui::Window,
    origin: Point<Pixels>,
    color: Hsla,
    top: f32,
    drop: f32,
    ctrl_a: (f32, f32),
    ctrl_b: (f32, f32),
) {
    let p = |x: f32, y: f32| origin + point(px(x), px(y));
    let mut path = PathBuilder::stroke(px(1.5));
    path.move_to(p(top, 0.0));
    path.cubic_bezier_to(p(0.0, drop), p(ctrl_a.0, ctrl_a.1), p(ctrl_b.0, ctrl_b.1));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn progress_pill(n: u8, total: u8, accent: Hsla) -> AnyElement {
    div()
        .px_2()
        .py(px(2.))
        .rounded_full()
        .border_1()
        .border_color(accent.opacity(0.7))
        .bg(accent.opacity(0.16))
        .text_xs()
        .text_color(accent)
        .child(format!("Step {n} of {total}"))
        .into_any_element()
}

fn mark(step: Step, ink: &Ink) -> AnyElement {
    let symbol = if step == Step::Done { "✓" } else { ">_" };
    let dots = step != Step::Done;
    let mut face = v_flex()
        .items_center()
        .justify_center()
        .gap_1()
        .w(px(78.))
        .h(px(58.))
        .rounded(px(12.))
        .border_1()
        .border_color(ink.accent)
        .bg(ink.popover)
        .shadow(vec![
            BoxShadow::new(px(0.), px(0.), ink.accent.opacity(0.7)).blur_radius(px(16.)),
        ]);
    if dots {
        face = face.child(
            h_flex()
                .gap_1()
                .child(dot(ink.accent))
                .child(dot(hsla(0.13, 0.85, 0.58, 1.)))
                .child(dot(hsla(0.35, 0.55, 0.48, 1.))),
        );
    }
    div()
        .flex()
        .justify_center()
        .child(
            face.child(
                div()
                    .text_color(ink.foreground)
                    .font_weight(FontWeight::BOLD)
                    .child(symbol),
            ),
        )
        .into_any_element()
}

fn dot(color: Hsla) -> AnyElement {
    div()
        .size(px(6.))
        .rounded_full()
        .bg(color)
        .into_any_element()
}

fn close_button<T: Host>(muted: Hsla, cx: &mut Context<T>) -> AnyElement {
    div()
        .id("tour-close")
        .debug_selector(|| "tour-close".to_owned())
        .absolute()
        .top(px(10.))
        .right(px(12.))
        .px_1()
        .text_sm()
        .text_color(muted)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this: &mut T, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.tour_act(Action::Exit, window, cx);
            }),
        )
        .child("×")
        .into_any_element()
}

fn buttons<T: Host>(step: Step, ink: &Ink, cx: &mut Context<T>) -> AnyElement {
    let specs: &[(&str, &str, bool, Action)] = match step {
        Step::Welcome => &[
            ("tour-exit", "Exit", false, Action::Exit),
            ("tour-next", "Next →", true, Action::Next),
        ],
        Step::NewChat | Step::Chats | Step::Timeline | Step::Prompt => &[
            ("tour-exit", "Exit", false, Action::Exit),
            ("tour-back", "Back", false, Action::Back),
            ("tour-next", "Next →", true, Action::Next),
        ],
        Step::Commands => &[
            ("tour-exit", "Exit", false, Action::Exit),
            ("tour-back", "Back", false, Action::Back),
            ("tour-next", "Finish ✓", true, Action::Next),
        ],
        Step::Done => &[
            ("tour-again", "See again", false, Action::Again),
            ("tour-begin", "Get started →", true, Action::Begin),
        ],
    };
    let mut row = h_flex().w_full().flex_wrap().gap_2().justify_center();
    for (id, label, primary, action) in specs {
        row = row.child(action_button(id, label, *primary, ink, *action, cx));
    }
    row.into_any_element()
}

fn action_button<T: Host>(
    id: &'static str,
    label: &'static str,
    primary: bool,
    ink: &Ink,
    action: Action,
    cx: &mut Context<T>,
) -> AnyElement {
    let mut button = div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .px_4()
        .py(px(7.))
        .min_w(px(104.))
        .flex()
        .flex_shrink_0()
        .justify_center()
        .rounded(px(10.))
        .text_sm()
        .cursor_pointer();
    button = if primary {
        button.bg(ink.accent).text_color(ink.accent_fg).shadow(vec![
            BoxShadow::new(px(0.), px(0.), ink.accent.opacity(0.5)).blur_radius(px(12.)),
        ])
    } else {
        button
            .bg(ink.button)
            .text_color(ink.foreground)
            .border_1()
            .border_color(ink.border.opacity(0.7))
    };
    button
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this: &mut T, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.tour_act(action, window, cx);
            }),
        )
        .child(label)
        .into_any_element()
}
