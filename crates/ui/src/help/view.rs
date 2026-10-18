//! [`HelpView`]: the native view of the help modal.
//!
//! A search box on top, the section tabs on the left and the rows of the
//! current section on the right. The list has the focus when it opens:
//!
//! - `1`–`8` jump to a section, `Tab`/`Shift+Tab` go to the next or the
//!   previous one;
//! - `j`/`k` (or the arrows) move, `PageUp`/`PageDown`, `Home`/`End`;
//! - `Enter` expands the selected row (a tool's input schema, a plugin's
//!   details), `y` copies its command or name;
//! - `/` goes to the search box, where typing filters every section live;
//!   there the arrows still move and `Enter` goes back to the list.
//!
//! `Esc` reaches the modal host, which closes the help.

use std::collections::HashSet;

use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::{HelpSnapshot, RowView, Section, filter, section_rows};

/// Rows `PageUp`/`PageDown` move.
const PAGE: usize = 10;

/// The help modal's content.
pub struct HelpView {
    snapshot: HelpSnapshot,
    filtered: HelpSnapshot,
    section: Section,
    selected: usize,
    expanded: HashSet<(Section, String)>,
    search: Entity<InputState>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    status: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl HelpView {
    /// A view over `snapshot`, on `section` (or the first one with rows),
    /// with `query` already in the search box.
    pub fn new(
        snapshot: HelpSnapshot,
        section: Option<Section>,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = query.to_owned();
        let search = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .placeholder("Search commands, keys, tools, plugins…  (/ to type here)");
            state.set_value(query.clone(), window, cx);
            state
        });
        let changed = cx.subscribe(&search, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.refilter(cx);
            }
        });
        let filtered = filter(&snapshot, &query);
        let section = section.unwrap_or_else(|| first_with_rows(&filtered));
        Self {
            snapshot,
            filtered,
            section,
            selected: 0,
            expanded: HashSet::new(),
            search,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            status: None,
            _subscriptions: vec![changed],
        }
    }

    /// The current section.
    pub fn section(&self) -> Section {
        self.section
    }

    /// The rows left by the search.
    pub fn filtered(&self) -> &HelpSnapshot {
        &self.filtered
    }

    /// The whole snapshot.
    pub fn snapshot(&self) -> &HelpSnapshot {
        &self.snapshot
    }

    /// Index of the selected row of the current section.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The search text.
    pub fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().to_string()
    }

    /// The rows of the current section, as shown.
    pub fn rows(&self) -> Vec<RowView> {
        section_rows(&self.filtered, self.section)
    }

    /// Whether the search box has the focus.
    pub fn is_searching(&self, window: &Window, cx: &App) -> bool {
        self.search.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Shows `section`.
    pub fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section != section {
            self.section = section;
            self.selected = 0;
            self.scroll.scroll_to_item(0);
        }
        cx.notify();
    }

    /// Replaces the search text.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.to_owned();
        self.search
            .update(cx, |search, cx| search.set_value(query, window, cx));
        self.refilter(cx);
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.filtered = filter(&self.snapshot, &query);
        // Stay on the section unless the search left it empty.
        if self.filtered.len(self.section) == 0 && !query.trim().is_empty() {
            self.section = first_with_rows(&self.filtered);
        }
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.filtered.len(self.section);
        if len == 0 {
            return;
        }
        let last = len - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn step_section(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = Section::ALL.len();
        let ix = self.section.index();
        let next = if forward {
            (ix + 1) % n
        } else {
            (ix + n - 1) % n
        };
        self.set_section(Section::ALL[next], cx);
    }

    /// Copies the selected row's command or name.
    pub fn copy_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.rows().into_iter().nth(self.selected) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(row.copy.clone()));
        self.status = Some(format!("copied: {}", row.copy));
        cx.notify();
    }

    fn toggle_expanded(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.rows().into_iter().nth(self.selected) else {
            return;
        };
        let key = (self.section, row.primary);
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let m = &keys.modifiers;
        let searching = self.is_searching(window, cx);
        if m.alt || m.platform {
            return;
        }
        if m.control {
            match keys.key.as_str() {
                "n" => self.move_selection(1, cx),
                "p" => self.move_selection(-1, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        match keys.key.as_str() {
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "pagedown" => self.move_selection(PAGE as isize, cx),
            "pageup" => self.move_selection(-(PAGE as isize), cx),
            "tab" => self.step_section(!m.shift, cx),
            "enter" if searching => window.focus(&self.focus_handle, cx),
            _ if searching => return,
            "j" => self.move_selection(1, cx),
            "k" => self.move_selection(-1, cx),
            "home" => self.move_selection(isize::MIN, cx),
            "end" => self.move_selection(isize::MAX, cx),
            "enter" => self.toggle_expanded(cx),
            "y" => self.copy_selected(cx),
            "/" => {
                let handle = self.search.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            key => {
                let Some(section) = key
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|ix| Section::ALL.get(ix).copied())
                else {
                    return;
                };
                self.set_section(section, cx);
            }
        }
        cx.stop_propagation();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (active, muted) = (theme.list_active, theme.muted_foreground);
        let tabs = Section::ALL.iter().enumerate().map(|(ix, section)| {
            let section = *section;
            let count = self.filtered.len(section);
            let mut tab = h_flex()
                .id(("help-tab", ix))
                .gap_2()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .child(
                    div()
                        .text_color(muted)
                        .child(SharedString::from(format!("{}", ix + 1))),
                )
                .child(div().flex_1().child(section.title()))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(count.to_string())),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_section(section, cx)));
            if section == self.section {
                tab = tab.bg(active);
            }
            if count == 0 {
                tab = tab.opacity(0.5);
            }
            tab
        });
        v_flex()
            .id("help-tabs")
            .w(px(200.))
            .flex_none()
            .gap_0p5()
            .p_2()
            .border_r_1()
            .border_color(theme.border)
            .children(tabs)
    }

    fn render_row(&self, ix: usize, row: &RowView, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let selected = ix == self.selected;
        // Schemas are long: a tool shows its own only when expanded.
        let show_details = !row.details.is_empty()
            && (self.expanded.contains(&(self.section, row.primary.clone()))
                || (selected && self.section != Section::Tools));
        let mut line = h_flex()
            .id(("help-row", ix))
            .gap_3()
            .px_2()
            .py_0p5()
            .rounded_sm()
            .child(
                div()
                    .w(px(260.))
                    .flex_none()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(SharedString::from(row.primary.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(muted)
                    .child(SharedString::from(row.secondary.clone())),
            )
            .children(row.tags.iter().filter(|t| !t.is_empty()).map(|tag| {
                div()
                    .flex_none()
                    .px_1()
                    .text_xs()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_sm()
                    .text_color(muted)
                    .child(SharedString::from(tag.clone()))
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = ix;
                cx.notify();
            }));
        if selected {
            line = line.bg(theme.list_active);
        }
        let details = show_details.then(|| {
            v_flex()
                .pl(px(24.))
                .pb_1()
                .text_xs()
                .text_color(muted)
                .children(row.details.iter().map(|d| {
                    div()
                        .whitespace_nowrap()
                        .child(SharedString::from(d.clone()))
                }))
        });
        let group = row.group.clone().map(|title| {
            div()
                .pt_2()
                .pb_0p5()
                .px_2()
                .text_xs()
                .text_color(theme.primary)
                .child(SharedString::from(title))
        });
        v_flex().children(group).child(line).children(details)
    }
}

/// The first section with rows, or Commands.
fn first_with_rows(snapshot: &HelpSnapshot) -> Section {
    Section::ALL
        .into_iter()
        .find(|section| snapshot.len(*section) > 0)
        .unwrap_or(Section::Commands)
}

impl Focusable for HelpView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HelpView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows();
        let elements: Vec<_> = rows
            .iter()
            .enumerate()
            .map(|(ix, row)| self.render_row(ix, row, cx).into_any_element())
            .collect();
        let tabs = self.render_tabs(cx).into_any_element();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let empty = elements.is_empty().then(|| {
            div()
                .p_2()
                .text_color(muted)
                .child(if self.snapshot.len(self.section) == 0 {
                    "nothing here"
                } else {
                    "nothing matches the search"
                })
        });
        let hint = "/ search · 1–8 or Tab sections · j/k move · Enter expand · y copy · Esc close";
        let footer = h_flex()
            .gap_3()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(muted)
            .child(div().flex_1().child(hint))
            .children(
                self.status
                    .clone()
                    .map(|status| div().child(SharedString::from(status))),
            );
        v_flex()
            .id("help")
            .debug_selector(|| "help-view".to_owned())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Input::new(&self.search)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_start()
                    .child(tabs)
                    .child(
                        div()
                            .id("help-rows")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_scroll()
                            .track_scroll(&self.scroll)
                            .p_2()
                            .children(elements)
                            .children(empty)
                            .scrollbar(&self.scroll, ScrollbarAxis::Vertical),
                    ),
            )
            .child(footer)
    }
}
