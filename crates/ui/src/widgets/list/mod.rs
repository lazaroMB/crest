//! The filterable list widget: records (log lines, requests) in a
//! virtual list.
//!
//! Rows are drawn by `uniform_list`, so only the ones in view are laid
//! out even for hundreds of thousands of items. The filter takes a
//! substring (case-insensitive unless it has an uppercase letter) or a
//! `/regex/` (`/…/i` ignores case) and searches the text, timestamp, level
//! and field values. Chips narrow by level and by the spec's facet fields
//! (values of one facet are or-ed, facets and-ed). A click on a row opens
//! it: its fields follow as key/value lines, one per row, so rows keep one
//! height. Follow keeps the last item in view when items arrive; scrolling
//! up turns it off. Copy puts the visible items on the clipboard as lines.
//!
//! With the rows focused (a click on one, or `↓` from the filter), `↑`/`↓`
//! and `Home`/`End` move the selection, keeping it in view, and `Enter`
//! opens or closes the selected item.
//!
//! [`view`] gives the entity of an id (to subscribe to
//! [`ListEvent::Selected`] or drive it) and [`render`] the widget.

pub mod filter;

use std::collections::HashSet;
use std::rc::Rc;

use domain::block::BlockId;
use domain::widget::list::{LevelClass, ListSpec, value_text};
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, ElementId, Entity, EventEmitter,
    FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Render, ScrollStrategy, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle, Window,
    div, prelude::FluentBuilder as _, px, uniform_list,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use self::filter::{Facet, LEVEL_FACET, Line, Query, Selection, facets, lines};

/// Height of a line.
pub const ROW_HEIGHT: f32 = 22.0;
/// Tallest the list gets before it scrolls.
pub const MAX_HEIGHT: f32 = 320.0;
/// Width of the timestamp column.
const TS_WIDTH: f32 = 150.0;
/// Width of the level column.
const LEVEL_WIDTH: f32 = 56.0;
/// Width of a field column.
const COLUMN_WIDTH: f32 = 110.0;
/// Width of a field name in an opened row.
const KEY_WIDTH: f32 = 160.0;

/// What the list tells its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListEvent {
    /// A click selected an item (index into the spec's items), or the
    /// selection was cleared.
    Selected(Option<usize>),
}

/// Per-id state of a list: the filter, the chips, the opened rows and
/// follow mode.
pub struct ListView {
    spec: Rc<ListSpec>,
    input: Entity<InputState>,
    query: Query,
    query_error: Option<String>,
    selection: Selection,
    facets: Rc<Vec<Facet>>,
    visible: Rc<Vec<usize>>,
    open: HashSet<usize>,
    lines: Rc<Vec<Line>>,
    selected: Option<usize>,
    /// Whether any item has a timestamp (then every row has the column).
    has_ts: bool,
    follow: bool,
    scroll: UniformListScrollHandle,
    /// Focused, the arrows move the selection.
    focus_handle: FocusHandle,
    _input_events: Subscription,
}

impl EventEmitter<ListEvent> for ListView {}

impl ListView {
    fn new(spec: &ListSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("filter · /regex/"));
        super::register_input(&input, cx);
        let subscription =
            cx.subscribe(&input, |this: &mut Self, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let text = input.read(cx).value().to_string();
                    this.set_query(&text, cx);
                }
            });
        let mut view = Self {
            spec: Rc::new(spec.clone()),
            input,
            query: Query::All,
            query_error: None,
            selection: Selection::new(),
            facets: Rc::new(facets(spec)),
            visible: Rc::default(),
            open: HashSet::new(),
            lines: Rc::default(),
            selected: None,
            has_ts: spec.items.iter().any(|i| i.ts.is_some()),
            follow: spec.follow,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _input_events: subscription,
        };
        view.refilter();
        view
    }

    /// Takes a new spec: keeps the filter, chips and follow mode, drops
    /// opened rows that no longer exist.
    fn sync(&mut self, spec: &ListSpec, cx: &mut Context<Self>) {
        if *self.spec == *spec {
            return;
        }
        self.spec = Rc::new(spec.clone());
        self.facets = Rc::new(facets(spec));
        self.has_ts = spec.items.iter().any(|i| i.ts.is_some());
        let len = spec.items.len();
        self.open.retain(|ix| *ix < len);
        self.selected = self.selected.filter(|ix| *ix < len);
        self.refilter();
        cx.notify();
    }

    fn refilter(&mut self) {
        self.visible = Rc::new(filter::filter(&self.spec, &self.query, &self.selection));
        self.relines();
    }

    fn relines(&mut self) {
        self.lines = Rc::new(lines(&self.spec, &self.visible, &self.open));
        if self.follow && !self.lines.is_empty() {
            self.scroll
                .scroll_to_item(self.lines.len() - 1, ScrollStrategy::Bottom);
        }
    }

    /// The spec shown.
    pub fn spec(&self) -> &ListSpec {
        &self.spec
    }

    /// Indices of the items that pass the filter, in order.
    pub fn visible(&self) -> &[usize] {
        &self.visible
    }

    /// The lines drawn (items and fields of opened items).
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The error of an invalid `/regex/`, if the filter is one.
    pub fn query_error(&self) -> Option<&str> {
        self.query_error.as_deref()
    }

    /// The selected item.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Whether follow mode is on.
    pub fn follow(&self) -> bool {
        self.follow
    }

    /// The scroll handle of the rows.
    pub fn scroll_handle(&self) -> &UniformListScrollHandle {
        &self.scroll
    }

    /// The focus of the rows: focused, the arrows move the selection.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Selects visible item `ix` (an index into the spec's items), scrolls
    /// it into view and tells the parent. Follow mode stops.
    pub fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.follow = false;
        if let Some(line) = self.lines.iter().position(|line| *line == Line::Item(ix)) {
            self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
        }
        if self.selected != Some(ix) {
            self.selected = Some(ix);
            cx.emit(ListEvent::Selected(Some(ix)));
        }
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if m.control || m.alt || m.platform || m.shift {
            return;
        }
        let key = event.keystroke.key.as_str();
        if !self.focus_handle.is_focused(window) {
            // `↓` in the filter goes down to the rows.
            let in_filter =
                gpui::Focusable::focus_handle(self.input.read(cx), cx).is_focused(window);
            if in_filter && key == "down" {
                window.focus(&self.focus_handle, cx);
                if let Some(ix) = step(&self.visible, self.selected, Step::First) {
                    self.select(ix, cx);
                }
                cx.stop_propagation();
            }
            return;
        }
        let step_by = match key {
            "up" => Step::Prev,
            "down" => Step::Next,
            "home" => Step::First,
            "end" => Step::Last,
            "enter" => {
                if let Some(ix) = self.selected.filter(|ix| self.visible.contains(ix)) {
                    self.toggle_item(ix, cx);
                }
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        if let Some(ix) = step(&self.visible, self.selected, step_by) {
            self.select(ix, cx);
        }
        cx.stop_propagation();
    }

    /// The visible items as lines, for the clipboard.
    pub fn visible_text(&self) -> String {
        self.spec.to_text(self.visible.iter().copied())
    }

    /// Filters by `text` (see [`Query::parse`]). An invalid regex keeps
    /// the previous result and shows its error.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        match Query::parse(text) {
            Ok(query) => {
                self.query = query;
                self.query_error = None;
                self.refilter();
            }
            Err(error) => self.query_error = Some(error),
        }
        cx.notify();
    }

    /// Adds or removes the chip `value` of `facet`.
    pub fn toggle_chip(&mut self, facet: &str, value: &str, cx: &mut Context<Self>) {
        let chosen = self.selection.entry(facet.to_owned()).or_default();
        if !chosen.remove(value) {
            chosen.insert(value.to_owned());
        }
        if chosen.is_empty() {
            self.selection.remove(facet);
        }
        self.refilter();
        cx.notify();
    }

    /// Whether chip `value` of `facet` is chosen.
    pub fn is_chosen(&self, facet: &str, value: &str) -> bool {
        self.selection.get(facet).is_some_and(|c| c.contains(value))
    }

    /// Opens or closes item `ix`, selects it and tells the parent.
    pub fn toggle_item(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.open.remove(&ix) {
            self.open.insert(ix);
        }
        if self.selected != Some(ix) {
            self.selected = Some(ix);
            cx.emit(ListEvent::Selected(Some(ix)));
        }
        let follow = std::mem::replace(&mut self.follow, false);
        self.relines();
        self.follow = follow;
        cx.notify();
    }

    /// Whether item `ix` is open.
    pub fn is_open(&self, ix: usize) -> bool {
        self.open.contains(&ix)
    }

    /// Turns follow mode on (scrolling to the last item) or off.
    pub fn set_follow(&mut self, follow: bool, cx: &mut Context<Self>) {
        self.follow = follow;
        if follow && !self.lines.is_empty() {
            self.scroll
                .scroll_to_item(self.lines.len() - 1, ScrollStrategy::Bottom);
        }
        cx.notify();
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Scrolling back up stops following the tail.
        if self.follow && f32::from(event.delta.pixel_delta(px(ROW_HEIGHT)).y) > 0.0 {
            self.follow = false;
            cx.notify();
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let title = self
            .spec
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("List");
        let count = if self.visible.len() == self.spec.items.len() {
            format!("{} items", self.spec.items.len())
        } else {
            format!("{} of {} items", self.visible.len(), self.spec.items.len())
        };
        let follow = if self.follow { "following" } else { "follow" };
        h_flex()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(title.to_owned())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .debug_selector(|| "filter-list-count".to_owned())
                    .child(count),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .child(
                        super::tool_button("filter-list-follow", follow, cx).on_click(cx.listener(
                            |this, _, _, cx| {
                                cx.stop_propagation();
                                let follow = !this.follow;
                                this.set_follow(follow, cx);
                            },
                        )),
                    )
                    .child(super::tool_button("filter-list-copy", "copy", cx).on_click(
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            cx.write_to_clipboard(ClipboardItem::new_string(this.visible_text()));
                        }),
                    )),
            )
            .into_any_element()
    }

    fn chips(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let facets = self.facets.clone();
        facets
            .iter()
            .filter(|facet| !facet.values.is_empty())
            .map(|facet| {
                let theme = cx.theme();
                let (muted, border, active_bg, hover) = (
                    theme.muted_foreground,
                    theme.border,
                    theme.secondary_active,
                    theme.secondary_hover,
                );
                let mut row = h_flex()
                    .flex_wrap()
                    .gap_1()
                    .text_xs()
                    .child(div().text_color(muted).child(format!("{}:", facet.name)));
                for (value, count) in &facet.values {
                    let chosen = self.is_chosen(&facet.name, value);
                    let color = if facet.name == LEVEL_FACET {
                        level_color(value, cx)
                    } else {
                        cx.theme().foreground
                    };
                    let (name, value_owned) = (facet.name.clone(), value.clone());
                    let selector = format!("filter-list-chip-{}-{value}", facet.name);
                    row = row.child(
                        div()
                            .id(ElementId::Name(SharedString::from(selector.clone())))
                            .debug_selector(move || selector.clone())
                            .px_1p5()
                            .rounded_sm()
                            .border_1()
                            .border_color(border)
                            .when(chosen, |d| d.bg(active_bg))
                            .hover(move |s| s.bg(hover))
                            .cursor_pointer()
                            .text_color(color)
                            .child(format!("{value} {count}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_chip(&name, &value_owned, cx);
                            })),
                    );
                }
                if facet.more > 0 {
                    row = row.child(
                        div()
                            .text_color(muted)
                            .child(format!("+{} more", facet.more)),
                    );
                }
                row.into_any_element()
            })
            .collect()
    }

    fn render_lines(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let lines = self.lines.clone();
        let spec = self.spec.clone();
        range
            .filter_map(|ix| lines.get(ix).copied())
            .map(|line| match line {
                Line::Item(item) => self.item_row(&spec, item, cx),
                Line::Field(item, field) => field_row(&spec, item, field, cx),
            })
            .collect()
    }

    fn item_row(&self, spec: &ListSpec, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = spec.items.get(ix) else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let theme = cx.theme();
        let (muted, hover, selected_bg, mono) = (
            theme.muted_foreground,
            theme.secondary_hover,
            theme.secondary,
            theme.mono_font_family.clone(),
        );
        let chevron = match (item.fields.is_empty(), self.open.contains(&ix)) {
            (true, _) => " ",
            (false, true) => "▾",
            (false, false) => "▸",
        };
        let mut row = h_flex()
            .id(("filter-list-item", ix))
            .debug_selector(move || format!("filter-list-item-{ix}"))
            .h(px(ROW_HEIGHT))
            .w_full()
            .gap_2()
            .px_1()
            .whitespace_nowrap()
            .cursor_pointer()
            .when(self.selected == Some(ix), |d| d.bg(selected_bg))
            .hover(move |s| s.bg(hover))
            .child(div().w(px(10.)).text_color(muted).child(chevron));
        if self.has_ts {
            row = row.child(
                div()
                    .w(px(TS_WIDTH))
                    .flex_none()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(mono.clone())
                    .text_color(muted)
                    .child(SharedString::from(item.ts.clone().unwrap_or_default())),
            );
        }
        if let Some(level) = &item.level {
            row = row.child(
                div()
                    .w(px(LEVEL_WIDTH))
                    .flex_none()
                    .overflow_hidden()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(level_color(level, cx))
                    .child(SharedString::from(level.to_ascii_uppercase())),
            );
        }
        for column in &spec.columns {
            row = row.child(
                div()
                    .w(px(COLUMN_WIDTH))
                    .flex_none()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(muted)
                    .child(SharedString::from(
                        item.field_text(column).unwrap_or_default(),
                    )),
            );
        }
        row.child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(SharedString::from(first_line(&item.text))),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            window.focus(&this.focus_handle, cx);
            this.toggle_item(ix, cx);
        }))
        .into_any_element()
    }
}

/// A move of the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The visible item before (the last one with none selected).
    Prev,
    /// The visible item after (the first one with none selected).
    Next,
    /// The first visible item.
    First,
    /// The last visible item.
    Last,
}

/// The item `step` selects among the `visible` ones from `selected`. It
/// stops at the ends; a selection the filter hid starts over.
pub fn step(visible: &[usize], selected: Option<usize>, step: Step) -> Option<usize> {
    let at = selected.and_then(|ix| visible.iter().position(|v| *v == ix));
    let pos = match (step, at) {
        (Step::First, _) | (Step::Next, None) => 0,
        (Step::Last, _) | (Step::Prev, None) => visible.len().checked_sub(1)?,
        (Step::Next, Some(at)) => (at + 1).min(visible.len().checked_sub(1)?),
        (Step::Prev, Some(at)) => at.saturating_sub(1),
    };
    visible.get(pos).copied()
}

/// The first line of `text`, with a marker when there are more.
fn first_line(text: &str) -> String {
    match text.split_once('\n') {
        Some((first, _)) => format!("{first} …"),
        None => text.to_owned(),
    }
}

fn field_row(spec: &ListSpec, ix: usize, field: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let entry = spec
        .items
        .get(ix)
        .and_then(|item| item.fields.iter().nth(field));
    let (key, value) = entry
        .map(|(k, v)| (k.clone(), value_text(v)))
        .unwrap_or_default();
    h_flex()
        .debug_selector(move || format!("filter-list-field-{ix}-{field}"))
        .h(px(ROW_HEIGHT))
        .w_full()
        .gap_2()
        .pl(px(28.))
        .whitespace_nowrap()
        .text_sm()
        .child(
            div()
                .w(px(KEY_WIDTH))
                .flex_none()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(key)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .font_family(theme.mono_font_family.clone())
                .child(SharedString::from(value.replace('\n', " ⏎ "))),
        )
        .into_any_element()
}

/// Theme color of a level name.
pub fn level_color(level: &str, cx: &App) -> Hsla {
    let theme = cx.theme();
    match LevelClass::of(level) {
        LevelClass::Fatal | LevelClass::Error => theme.danger,
        LevelClass::Warn => theme.warning,
        LevelClass::Info => theme.info,
        LevelClass::Debug | LevelClass::Trace | LevelClass::Other => theme.muted_foreground,
    }
}

impl Render for ListView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.lines.len();
        let height = (count as f32 * ROW_HEIGHT).clamp(ROW_HEIGHT, MAX_HEIGHT);
        let theme = cx.theme();
        let (border, radius, danger, muted) = (
            theme.border,
            theme.radius,
            theme.danger,
            theme.muted_foreground,
        );
        let list = uniform_list(
            "filter-list-rows",
            count,
            cx.processor(|this, range, _, cx| this.render_lines(range, cx)),
        )
        .track_scroll(&self.scroll)
        .size_full();
        let body = div()
            .id("filter-list-body")
            .debug_selector(|| "filter-list-body".to_owned())
            .w_full()
            .h(px(height + 2.0))
            .rounded(radius)
            .border_1()
            .border_color(border)
            .text_sm()
            .track_focus(&self.focus_handle)
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .when(count == 0, |d| {
                d.child(
                    div()
                        .px_2()
                        .text_color(muted)
                        .child(if self.spec.items.is_empty() {
                            "no items"
                        } else {
                            "no items match"
                        }),
                )
            })
            .when(count > 0, |d| {
                d.child(list)
                    .scrollbar(&self.scroll, ScrollbarAxis::Vertical)
            });
        let filter_row = h_flex()
            .gap_2()
            .child(
                div()
                    .w(px(280.))
                    .debug_selector(|| "filter-list-input".to_owned())
                    .child(Input::new(&self.input).small()),
            )
            .children(self.query_error.clone().map(|error| {
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .text_color(danger)
                    .child(SharedString::from(error.replace('\n', " ")))
            }));
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "filter-list".to_owned())
            // A press in the widget (the filter, a chip, a row) is not a
            // click on the timeline row, which would fold the block.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(Self::on_key))
            .child(self.header(cx))
            .child(filter_row)
            .children(self.chips(cx))
            .child(body)
    }
}

/// The list entity of `id`, in sync with `spec`.
pub fn view(id: BlockId, spec: &ListSpec, window: &mut Window, cx: &mut App) -> Entity<ListView> {
    let view =
        crate::blocks::block_state(id, window, cx, |window, cx| ListView::new(spec, window, cx));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// The whole widget (header, filter, chips, rows) of `id`.
pub fn render(id: BlockId, spec: &ListSpec, window: &mut Window, cx: &mut App) -> AnyElement {
    view(id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests;
