//! What the information mark draws: the mark, a toast, the panel and its
//! rows. Free functions over the view's context, so the view itself stays
//! the model and its events.

use application_core::ports::events::NoticeLevel;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};

use super::{InfoView, Row, Toast};
use crate::kit::quiet_button;
use crate::status::notice_color;

/// Tallest the panel grows before it scrolls.
pub const PANEL_MAX_HEIGHT: f32 = 320.0;
/// Widest the toast and the panel get.
const WIDTH: f32 = 360.0;
/// The icon of each level.
const ICONS: [(NoticeLevel, IconName); 3] = [
    (NoticeLevel::Info, IconName::Info),
    (NoticeLevel::Warning, IconName::TriangleAlert),
    (NoticeLevel::Error, IconName::CircleX),
];

fn icon(level: NoticeLevel) -> Icon {
    let found = ICONS.iter().find(|(known, _)| *known == level);
    Icon::new(found.map_or(IconName::Info, |(_, name)| name.clone())).size_3()
}

/// The text of a line, cut with an ellipsis instead of wrapping.
fn one_line(text: &str) -> impl gpui::IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .child(SharedString::from(text.to_owned()))
}

/// `el` as a floating card: the popover surface with a border, in small text.
fn card<E: gpui::Styled>(el: E, cx: &App) -> E {
    let theme = cx.theme();
    el.rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_sm()
}

/// The mark: the number of kept notices in the color of the worst, or the
/// info icon, quiet, when only short steps wait.
pub fn mark(rows: &[Row], cx: &mut Context<InfoView>) -> AnyElement {
    let kept: Vec<NoticeLevel> = rows
        .iter()
        .filter(|row| row.notice.is_some())
        .map(|row| row.level)
        .collect();
    let worst = if kept.contains(&NoticeLevel::Error) {
        Some(NoticeLevel::Error)
    } else {
        kept.first().copied()
    };
    let theme = cx.theme();
    let (border, fill, hover) = (theme.border, theme.popover, theme.list_active);
    let color = worst.map_or(theme.muted_foreground, |level| notice_color(level, cx));
    let face = match kept.len() {
        0 => icon(NoticeLevel::Info).into_any_element(),
        count => div().text_xs().child(count.to_string()).into_any_element(),
    };
    div()
        .id("info-mark")
        .debug_selector(|| "info-mark".to_owned())
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(border)
        .bg(fill)
        .text_color(color)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx)))
        .child(face)
        .into_any_element()
}

/// One line of the panel: a notice has its own `Dismiss`, a step none.
fn row((ix, entry): (usize, &Row), cx: &mut Context<InfoView>) -> AnyElement {
    let hover = cx.theme().list_active;
    let line = h_flex()
        .id(("info-row", ix))
        .gap_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .hover(move |style| style.bg(hover))
        .text_color(notice_color(entry.level, cx))
        .child(icon(entry.level))
        .child(one_line(&entry.text));
    let Some(id) = entry.notice else {
        return line.into_any_element();
    };
    line.child(dismiss_button(id, cx)).into_any_element()
}

/// The `Dismiss` of the kept notice `id`: only that one goes.
fn dismiss_button(id: u64, cx: &mut Context<InfoView>) -> impl gpui::IntoElement {
    let click = cx.listener(move |this, _, window, cx| {
        this.dismiss(id, cx);
        this.settle(window, cx);
    });
    quiet_button("info-dismiss", "Dismiss", cx).on_click(click)
}

/// The panel over `rows`, newest first, scrolling past its height.
pub fn panel(rows: &[Row], cx: &mut Context<InfoView>) -> AnyElement {
    let lines: Vec<AnyElement> = rows.iter().enumerate().map(|pair| row(pair, cx)).collect();
    let panel = v_flex()
        .w(px(WIDTH))
        .max_h(px(PANEL_MAX_HEIGHT))
        .py_1()
        .id("info-panel")
        .overflow_y_scroll()
        .debug_selector(|| "info-panel".to_owned())
        .children(lines);
    card(panel, cx).into_any_element()
}

/// The toast: one new entry.
pub fn toast(toast: &Toast, cx: &App) -> AnyElement {
    let line = h_flex()
        .id("info-toast")
        .debug_selector(|| "info-toast".to_owned())
        .gap_2()
        .px_2()
        .py_1()
        .max_w(px(WIDTH))
        .text_color(notice_color(toast.level, cx))
        .child(icon(toast.level))
        .child(one_line(&toast.text));
    card(line, cx).into_any_element()
}
