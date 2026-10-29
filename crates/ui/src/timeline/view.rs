//! GPUI view of the timeline: a virtualized [`list`] over a
//! [`TimelineState`], fed by a channel of domain events.
//!
//! Blocks have variable heights (streamed markdown, expanded tool calls), so
//! the view uses `list` with a [`ListState`] rather than `uniform_list`; only
//! the visible rows (plus overdraw) are laid out each frame. The list follows
//! the tail while new blocks arrive and stops following when the user moves
//! the selection up.
//!
//! With the focus (`<C-Tab>` from the prompt, or a click), `Up`/`Down`
//! select blocks, `Home`/`End` the first and last, `Enter` expands or
//! collapses the selected one and `PageUp`/`PageDown` scroll by a page.
//! Empty, it shows the crest mark and the keys to start with.
//!
//! `Esc` and printable keys typed with the focus in the timeline reach the
//! focused element first (a block widget, the outline menu): the session
//! hands them over as a [`DeferredKey`] and only when nothing in the
//! timeline takes them do they fall back to the prompt (`Esc` focuses it,
//! a letter is typed into it, a vim key moves the selection).

use application::keymap::{Action, Key, KeyCode};
use application::ports::events::DomainEvent;
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    AnyElement, App, ClipboardItem, Context, ElementId, FocusHandle, Focusable, FollowMode,
    InteractiveElement as _, IntoElement, KeyDownEvent, ListAlignment, ListState, MouseButton,
    MouseDownEvent, MouseUpEvent, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, Window, div, list,
    prelude::FluentBuilder as _, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};

use super::column::{COLUMN_GUTTER, block_width};
use super::selection::{self, CopyHint, render_hint};
use super::{TimelineChange, TimelineState, copy_text, render_block};

/// Pixels rendered above and below the viewport to avoid pop-in on scroll.
const OVERDRAW: f32 = 512.0;

/// What runs when a deferred key reaches the timeline unhandled.
type KeyFallback = Box<dyn FnOnce(&mut Window, &mut App)>;

/// A key the session let through to the focused element of the timeline,
/// with what to do when no element takes it.
pub struct DeferredKey {
    key: Key,
    fallback: KeyFallback,
}

impl DeferredKey {
    /// `key`, and `fallback` to run when it reaches the timeline itself
    /// unhandled (and is not one of its vim keys).
    pub fn new(key: Key, fallback: impl FnOnce(&mut Window, &mut App) + 'static) -> Self {
        Self {
            key,
            fallback: Box::new(fallback),
        }
    }
}

/// The timeline area of the main window.
pub struct TimelineView {
    state: TimelineState,
    list: ListState,
    focus_handle: FocusHandle,
    /// The "Copy" hint next to the text selected with the mouse.
    copy_hint: Option<CopyHint>,
    _hint_expiry: Option<Task<()>>,
    /// The reply (by its last block) whose copy button just copied.
    copied_reply: Option<usize>,
    /// The menu of the user's questions is open.
    outline_open: bool,
    /// The scroll of the menu's rows.
    outline_scroll: ScrollHandle,
    /// The menu just opened: scroll its current question into view.
    outline_reveal: bool,
    /// The key being dispatched, when the session deferred it.
    deferred: Option<DeferredKey>,
    _reply_expiry: Option<Task<()>>,
    _events: Option<Task<()>>,
}

impl TimelineView {
    /// An empty timeline that only changes through [`Self::apply`].
    pub fn new(cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(OVERDRAW));
        list.set_follow_mode(FollowMode::Tail);
        Self {
            state: TimelineState::new(),
            list,
            focus_handle: cx.focus_handle(),
            copy_hint: None,
            _hint_expiry: None,
            copied_reply: None,
            outline_open: false,
            outline_scroll: ScrollHandle::new(),
            outline_reveal: false,
            deferred: None,
            _reply_expiry: None,
            _events: None,
        }
    }

    /// The hint offering to copy the selected text, if shown.
    pub fn copy_hint(&self) -> Option<&CopyHint> {
        self.copy_hint.as_ref()
    }

    /// The mouse was released over the timeline: with text selected, the
    /// timeline takes the focus (so Ctrl+C copies instead of interrupting)
    /// and shows the hint.
    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let text = selection::selected_text(window, cx);
        if text.is_empty() {
            if self.copy_hint.take().is_some() {
                cx.notify();
            }
            return;
        }
        window.focus(&self.focus_handle, cx);
        self.copy_hint = Some(CopyHint {
            at: event.position,
            text,
            copied: false,
        });
        cx.notify();
    }

    /// Copies the text selected in the timeline. Returns whether there was
    /// any.
    pub fn copy_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = selection::selected_text(window, cx);
        if text.is_empty() {
            return false;
        }
        self.copy(text, cx);
        true
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let Some(hint) = &mut self.copy_hint else {
            return;
        };
        hint.copied = true;
        self._hint_expiry = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(selection::COPIED_TTL).await;
            this.update(cx, |this, cx| {
                if this.copy_hint.as_ref().is_some_and(|h| h.copied) {
                    this.copy_hint = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// A timeline that applies every event received on `events`, on the GPUI
    /// foreground executor.
    pub fn with_events(events: UnboundedReceiver<DomainEvent>, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(cx);
        this.listen(events, cx);
        this
    }

    /// Starts consuming `events`, replacing any previous receiver. Events
    /// that are already queued are applied in one batch, with one repaint.
    pub fn listen(&mut self, mut events: UnboundedReceiver<DomainEvent>, cx: &mut Context<Self>) {
        self._events = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                if this
                    .update(cx, |this, cx| this.apply_all(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// The model behind the view.
    pub fn state(&self) -> &TimelineState {
        &self.state
    }

    /// Applies one event and repaints if it changed the timeline.
    pub fn apply(&mut self, event: &DomainEvent, cx: &mut Context<Self>) {
        if self.apply_quietly(event) {
            cx.notify();
        }
    }

    /// Applies events in order with at most one repaint.
    pub fn apply_all(
        &mut self,
        events: impl IntoIterator<Item = DomainEvent>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for event in events {
            changed |= self.apply_quietly(&event);
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_quietly(&mut self, event: &DomainEvent) -> bool {
        let changed = match self.state.apply(event) {
            TimelineChange::None => false,
            TimelineChange::Inserted(ix) => {
                self.list.splice(ix..ix, 1);
                true
            }
            TimelineChange::Updated(ix) => {
                self.list.remeasure_items(ix..ix + 1);
                true
            }
        };
        // Texts whose slots or lint changed because of another block.
        let touched = self.state.take_touched();
        for ix in &touched {
            self.list.remeasure_items(*ix..*ix + 1);
        }
        changed || !touched.is_empty()
    }

    /// Selects the last block when none is selected (on taking the focus).
    pub fn select_last_if_none(&mut self, cx: &mut Context<Self>) {
        if self.state.selected().is_none() {
            self.select_with(TimelineState::select_last, cx);
        }
    }

    /// Hands the key about to be dispatched to the focused element of the
    /// timeline; [`DeferredKey::fallback`] runs if nothing takes it.
    pub fn defer_key(&mut self, deferred: DeferredKey) {
        self.deferred = Some(deferred);
    }

    /// Forgets a deferred key that an element took (it stopped the key
    /// before it reached the timeline).
    pub fn clear_deferred_key(&mut self) {
        self.deferred = None;
    }

    /// Whether a deferred key waits.
    pub fn has_deferred_key(&self) -> bool {
        self.deferred.is_some()
    }

    /// A deferred key reached the timeline unhandled: a vim key acts on
    /// it, any other runs its fallback once this dispatch is over.
    fn fall_through(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(deferred) = self.deferred.take() else {
            return;
        };
        cx.stop_propagation();
        if let Some(action) = Self::vim_action(&deferred.key) {
            self.handle_action(&Action::builtin(action), cx);
            return;
        }
        let fallback = deferred.fallback;
        window.defer(cx, fallback);
    }

    /// Keys while the timeline has the focus.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if event.keystroke.key == "escape" && self.copy_hint.take().is_some() {
            gpui_base::TextSelection::clear(window, cx);
            self.deferred = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" && self.outline_open {
            self.outline_open = false;
            self.deferred = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        // Arrows and Enter typed in a widget's filter stay there.
        if m.control || m.alt || m.platform || crate::widgets::owns_focus(window, cx) {
            return;
        }
        let action = match event.keystroke.key.as_str() {
            "up" => "block.prev",
            "down" => "block.next",
            "home" => "block.first",
            "end" => "block.last",
            "enter" => "block.toggle",
            "pageup" | "pagedown" => {
                let page = self.list.viewport_bounds().size.height * 0.9;
                let up = event.keystroke.key == "pageup";
                self.list.pause_following_tail();
                self.list.scroll_by(if up { -page } else { page });
                cx.notify();
                cx.stop_propagation();
                return;
            }
            _ => {
                self.fall_through(window, cx);
                return;
            }
        };
        self.handle_action(&Action::builtin(action), cx);
        cx.stop_propagation();
    }

    /// The action of a plain vim key while the timeline has the focus:
    /// `j`/`k` next/previous, `g`/`G` first/last, `o` expand, `t` view,
    /// `y` copy. The session asks before typing the key into the prompt.
    pub fn vim_action(key: &Key) -> Option<&'static str> {
        if key.mods.ctrl || key.mods.alt || key.mods.cmd || key.mods.shift {
            return None;
        }
        Some(match key.code {
            KeyCode::Char('j') => "block.next",
            KeyCode::Char('k') => "block.prev",
            KeyCode::Char('g') => "block.first",
            KeyCode::Char('G') => "block.last",
            KeyCode::Char('o') => "block.toggle",
            KeyCode::Char('t') => "block.toggle_view",
            KeyCode::Char('y') => "block.yank",
            _ => return None,
        })
    }

    /// Runs a timeline action (`block.next`, `block.toggle`...).
    /// Returns whether the action belongs to the timeline.
    pub fn handle_action(&mut self, action: &Action, cx: &mut Context<Self>) -> bool {
        let Action::Builtin(name) = action else {
            return false;
        };
        match name.as_str() {
            "block.next" => self.select_with(TimelineState::select_next, cx),
            "block.prev" => self.select_with(TimelineState::select_prev, cx),
            "block.first" => self.select_with(TimelineState::select_first, cx),
            "block.last" => self.select_with(TimelineState::select_last, cx),
            "block.toggle" => {
                if let Some(ix) = self.state.toggle_selected() {
                    self.list.remeasure_items(ix..ix + 1);
                    cx.notify();
                }
            }
            "block.yank" => {
                if let Some(entry) = self.state.selected_entry() {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_text(&entry.block.kind)));
                }
            }
            "block.toggle_view" => {
                if let Some(ix) = self.state.toggle_view_selected() {
                    self.list.remeasure_items(ix..ix + 1);
                    cx.notify();
                }
            }
            _ => return false,
        }
        true
    }

    fn select_with(
        &mut self,
        select: impl FnOnce(&mut TimelineState) -> Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let before = self.state.selected();
        let Some(ix) = select(&mut self.state) else {
            return;
        };
        // The selected block grows a copy-as row.
        self.remeasure_selection(before, Some(ix));
        self.reveal(ix);
        cx.notify();
    }

    /// Scrolls `ix` into view; the last block re-enables tail following.
    fn reveal(&self, ix: usize) {
        if ix + 1 == self.state.len() {
            self.list.set_follow_mode(FollowMode::Tail);
        } else {
            self.list.pause_following_tail();
            self.list.scroll_to_reveal_item(ix);
        }
    }

    /// Remeasures the blocks that lost and gained the selection.
    fn remeasure_selection(&self, before: Option<usize>, after: Option<usize>) {
        if before == after {
            return;
        }
        for ix in [before, after].into_iter().flatten() {
            if ix < self.state.len() {
                self.list.remeasure_items(ix..ix + 1);
            }
        }
    }

    /// Whether the menu of the user's questions is open.
    pub fn outline_open(&self) -> bool {
        self.outline_open
    }

    /// Opens or closes the menu of the user's questions. Open, the
    /// timeline takes the focus so `Esc` closes it.
    pub fn toggle_outline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.outline_open = !self.outline_open;
        if self.outline_open {
            self.outline_reveal = true;
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    /// Closes the menu of the user's questions.
    pub fn close_outline(&mut self, cx: &mut Context<Self>) {
        if self.outline_open {
            self.outline_open = false;
            cx.notify();
        }
    }

    /// Selects the menu row and scrolls that block to the top of the chat.
    fn choose_outline(&mut self, ix: usize, cx: &mut Context<Self>) {
        let before = self.state.selected();
        if self.state.select(ix).is_some() {
            self.remeasure_selection(before, Some(ix));
            self.list.pause_following_tail();
            self.list.scroll_to(gpui::ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }

    /// The chat menu: a rail of marks, and the questions when it is open.
    fn outline_layer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let outline = super::outline::chat_outline(self.state.entries(), self.state.selected());
        if outline.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (popover, border, muted, foreground, link) = (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            crate::theme::token(cx, "link.fg"),
        );
        let marks = outline.marks();
        let rail = v_flex()
            .id("chat-outline-rail")
            .w(px(18.))
            .flex_none()
            .py_1()
            .children(marks.into_iter().enumerate().map(|(ix, mark)| {
                let block = mark.block;
                div()
                    .id(("chat-outline-mark", ix))
                    .debug_selector(move || format!("chat-outline-mark-{ix}"))
                    .w_full()
                    .h(px(12.))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.toggle_outline(window, cx);
                            this.choose_outline(block, cx);
                        }),
                    )
                    .child(
                        div()
                            .h(px(2.))
                            .w(if mark.active { px(16.) } else { px(10.) })
                            .rounded_full()
                            .bg(if mark.active { foreground } else { muted }),
                    )
            }));
        let menu = self.outline_open.then(|| {
            let mut rows = Vec::new();
            let mut active_row = None;
            for (group_ix, group) in outline.groups.iter().enumerate() {
                if !group.title.is_empty() {
                    rows.push(
                        div()
                            .id(("chat-outline-group", group_ix))
                            .pt(if group_ix == 0 { px(0.) } else { px(6.) })
                            .text_sm()
                            .text_color(muted)
                            .child(group.title.clone())
                            .into_any_element(),
                    );
                }
                for item in &group.items {
                    let block = item.block;
                    let color = if item.active { link } else { foreground };
                    if item.active {
                        active_row = Some(rows.len());
                    }
                    rows.push(
                        div()
                            .id(("chat-outline-item", item.block))
                            .debug_selector(move || format!("chat-outline-item-{block}"))
                            .w_full()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(color)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.choose_outline(block, cx);
                                    this.close_outline(cx);
                                }),
                            )
                            .child(format!("{}. {}", item.number, item.label))
                            .into_any_element(),
                    );
                }
            }
            if self.outline_reveal {
                if let Some(row) = active_row {
                    crate::keyscroll::ensure_visible(&self.outline_scroll, row);
                }
                // Asked again until the menu has been laid out once.
                self.outline_reveal = active_row.is_some()
                    && !crate::keyscroll::laid_out(&self.outline_scroll);
                if self.outline_reveal {
                    cx.notify();
                }
            }
            v_flex()
                .id("chat-outline")
                .w(px(220.))
                .max_h(px(320.))
                .flex_none()
                .py_2()
                .px_3()
                .gap_1()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .bg(popover)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    h_flex()
                        .flex_none()
                        .justify_between()
                        .text_xs()
                        .text_color(muted)
                        .child("Esc to close")
                        .child(
                            div()
                                .debug_selector(|| "chat-outline-close".to_owned())
                                .child(
                                    Button::new("chat-outline-close")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("Close")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.close_outline(cx);
                                        })),
                                ),
                        ),
                )
                .child(
                    crate::keyscroll::list("chat-outline-rows", &self.outline_scroll, rows)
                        .flex_shrink(1.)
                        .gap_1(),
                )
        });
        Some(
            h_flex()
                .id("chat-outline-layer")
                .absolute()
                .top(px(8.))
                .right(px(10.))
                .items_start()
                .gap_1()
                // A click anywhere else closes the menu.
                .when(self.outline_open, |layer| {
                    layer.on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_outline(cx);
                    }))
                })
                .children(menu)
                .child(rail)
                .into_any_element(),
        )
    }

    /// A click on a row selects it and expands or collapses it, unless the
    /// click ended a text selection: then the row keeps its state.
    fn click(&mut self, ix: usize, text_selected: bool, cx: &mut Context<Self>) {
        let before = self.state.selected();
        if self.state.select(ix).is_some() {
            self.remeasure_selection(before, Some(ix));
            if !text_selected && let Some(ix) = self.state.toggle_selected() {
                self.list.remeasure_items(ix..ix + 1);
            }
        }
        cx.notify();
    }

    /// The "Copy" button at the end of a complete reply.
    fn reply_footer(&self, ix: usize, view: WeakEntity<Self>) -> Option<AnyElement> {
        let text = self.state.reply_to_copy(ix)?;
        let copied = self.copied_reply == Some(ix);
        let button = Button::new(("reply-copy", ix))
            .ghost()
            .xsmall()
            .icon(if copied {
                IconName::Check
            } else {
                IconName::Copy
            })
            .tooltip("Copy Message")
            .on_click(move |_, _, cx| {
                // The row's click would select and expand the block.
                cx.stop_propagation();
                let text = text.clone();
                view.update(cx, |this, cx| this.copy_reply(ix, text, cx))
                    .ok();
            });
        Some(h_flex().pt_1().child(button).into_any_element())
    }

    fn copy_reply(&mut self, ix: usize, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied_reply = Some(ix);
        self._reply_expiry = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(selection::COPIED_TTL).await;
            this.update(cx, |this, cx| {
                if this.copied_reply == Some(ix) {
                    this.copied_reply = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_item(
        &self,
        ix: usize,
        view: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(entry) = self.state.get(ix) else {
            return div().into_any_element();
        };
        let content = render_block(entry, window, cx);
        let theme = cx.theme();
        let selected = self.state.selected() == Some(ix);
        let width = block_width(&entry.block.kind);
        let footer = self.reply_footer(ix, view.clone());
        // `min_w_0`: content that does not wrap on its own (a form's
        // labels) must not widen the block past the window.
        let block = div()
            .w_full()
            .min_w_0()
            .max_w(width.max_width())
            .px_2()
            .py_1()
            .border_l_2()
            .debug_selector(move || format!("block-{ix}"))
            .child(content)
            .when_some(
                selected
                    .then(|| crate::blocks::copy::row(entry, cx))
                    .flatten(),
                |block, row| block.child(row),
            )
            .children(footer);
        let block = if selected {
            block
                .border_color(theme.list_active_border)
                .bg(theme.list_active)
        } else {
            block.border_color(gpui::transparent_black())
        };
        div()
            .id(ElementId::Integer(ix as u64))
            .w_full()
            .flex()
            .justify_center()
            .px(px(COLUMN_GUTTER))
            .on_click(move |_, window, cx| {
                // The click landed in a form being answered (one of its
                // inputs took the focus on mouse down): the form keeps the
                // focus and its row stays as it is.
                if crate::blocks::form::owns_focus(window, cx) {
                    return;
                }
                let text_selected = !selection::selected_text(window, cx).is_empty();
                view.update(cx, |this, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.click(ix, text_selected, cx);
                })
                .ok();
            })
            .child(block)
            .into_any_element()
    }
}

impl Focusable for TimelineView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The keys the welcome screen suggests.
const WELCOME_KEYS: &[(&str, &str)] = &[
    ("/", "commands: /research, /implement, /!cmd, /help…"),
    (
        "Ctrl+G",
        "shortcuts: e tree · ff files · fg grep · m model · j/k blocks · h help",
    ),
    ("Ctrl+Tab", "move the focus: prompt → timeline → tree"),
    ("Esc", "back to the prompt"),
    ("Ctrl+C", "clear the prompt or interrupt the turn"),
    (
        "Ctrl+Shift+C",
        "copy the selected text, from the prompt or the chat",
    ),
];

/// The empty timeline: the crest mark and the keys to start with.
fn welcome(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let rows = WELCOME_KEYS.iter().map(|(keys, what)| {
        h_flex()
            .gap_3()
            .child(
                div()
                    .w(px(110.))
                    .text_color(theme.primary)
                    .child(SharedString::from(*keys)),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(*what)),
            )
    });
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_4()
        .child(
            h_flex()
                .gap_3()
                .items_end()
                .child(crate::brand::mark(48.0))
                .child(
                    div()
                        .text_3xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("crest"),
                ),
        )
        .child(v_flex().gap_1().text_sm().children(rows))
}

impl Render for TimelineView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let edge = if focused {
            crate::theme::token(cx, "list.focused.border")
        } else {
            gpui::transparent_black()
        };
        if self.state.is_empty() {
            return div()
                .id("timeline-empty")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::on_key))
                .size_full()
                .border_t_1()
                .border_color(edge)
                .child(welcome(cx))
                .into_any_element();
        }
        let view = cx.entity().downgrade();
        // The hint outlives the selection only while it says "Copied".
        if let Some(hint) = &self.copy_hint
            && !hint.copied
            && selection::selected_text(window, cx).is_empty()
        {
            self.copy_hint = None;
        }
        let hint = self.copy_hint.as_ref().map(|hint| {
            let weak = view.clone();
            let text = hint.text.clone();
            render_hint(
                hint,
                move |_window, cx| {
                    weak.update(cx, |this, cx| this.copy(text.clone(), cx)).ok();
                },
                cx,
            )
        });
        let outline = self.outline_layer(cx);
        div()
            .id("timeline-list")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.mouse_up(event, window, cx);
                }),
            )
            .relative()
            .size_full()
            .border_t_1()
            .border_color(edge)
            .child(
                list(self.list.clone(), move |ix, window, cx| {
                    let Some(entity) = view.upgrade() else {
                        return div().into_any_element();
                    };
                    // `update` rather than `read`: block renderers need a
                    // mutable App to keep their per-block state.
                    entity.update(cx, |this, cx| {
                        this.render_item(ix, view.clone(), window, cx)
                    })
                })
                .size_full(),
            )
            .vertical_scrollbar(&self.list)
            .children(hint)
            .children(outline)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{PROSE_MAX_WIDTH, WIDE_MAX_WIDTH};
    use application::ports::events::EventSink as _;
    use application::testing::system::RecordingSink;
    use domain::block::{Block, BlockKind};
    use gpui::{Entity, TestAppContext, VisualTestContext};

    fn text(markdown: &str) -> Block {
        Block::new(BlockKind::Text {
            markdown: markdown.into(),
        })
    }

    fn timeline(
        cx: &mut TestAppContext,
        rx: UnboundedReceiver<DomainEvent>,
    ) -> Entity<TimelineView> {
        cx.update(gpui_component::init);
        let window = cx.add_window(|_, cx| TimelineView::with_events(rx, cx));
        window.root(cx).unwrap_or_else(|err| panic!("{err}"))
    }

    #[gpui::test]
    fn consumes_events_from_the_sink(cx: &mut TestAppContext) {
        let (sink, rx) = RecordingSink::with_channel();
        let view = timeline(cx, rx);
        let block = text("hola");
        sink.emit(DomainEvent::BlockAdded {
            block: block.clone(),
        });
        sink.emit(DomainEvent::TextDelta {
            block_id: block.id,
            text: " mundo".into(),
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.state().len(), 1);
            assert_eq!(view.list.item_count(), 1);
            assert!(matches!(
                &view.state().entries()[0].block.kind,
                BlockKind::Text { markdown } if markdown == "hola mundo"
            ));
        });
    }

    #[gpui::test]
    fn the_outline_menu_closes_every_way(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        view.update(cx, |view, cx| {
            view.apply_all(
                [text("> uno"), text("reply"), text("> dos"), text("reply")]
                    .map(|block| DomainEvent::BlockAdded { block }),
                cx,
            );
        });
        let draw = |cx: &mut VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        let click = |cx: &mut VisualTestContext, selector: &'static str| {
            let at = cx
                .debug_bounds(selector)
                .map(|b| b.center())
                .unwrap_or_else(|| panic!("no {selector}"));
            cx.simulate_click(at, gpui::Modifiers::none());
            cx.run_until_parked();
        };
        let open = |cx: &mut VisualTestContext| view.read_with(cx, |v, _| v.outline_open());
        draw(cx);
        // The rail toggles the menu.
        click(cx, "chat-outline-mark-0");
        assert!(open(cx));
        draw(cx);
        click(cx, "chat-outline-mark-0");
        assert!(!open(cx), "a second click on the rail closes it");
        // Choosing a question closes it.
        draw(cx);
        click(cx, "chat-outline-mark-0");
        draw(cx);
        click(cx, "chat-outline-item-2");
        assert!(!open(cx), "choosing closes it");
        assert_eq!(view.read_with(cx, |v, _| v.state().selected()), Some(2));
        // The close button.
        draw(cx);
        click(cx, "chat-outline-mark-0");
        draw(cx);
        click(cx, "chat-outline-close");
        assert!(!open(cx), "the close button closes it");
        // A click elsewhere.
        draw(cx);
        click(cx, "chat-outline-mark-0");
        draw(cx);
        cx.simulate_click(gpui::point(px(40.), px(600.)), gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(!open(cx), "a click outside closes it");
    }

    #[gpui::test]
    fn the_outline_scrolls_to_the_current_question(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        view.update(cx, |view, cx| {
            let blocks: Vec<_> = (0..40)
                .flat_map(|n| [text(&format!("> question {n}")), text("reply")])
                .map(|block| DomainEvent::BlockAdded { block })
                .collect();
            view.apply_all(blocks, cx);
        });
        // The last question is the current one.
        view.update(cx, |view, cx| {
            view.choose_outline(78, cx);
        });
        view.update_in(cx, |view, window, cx| view.toggle_outline(window, cx));
        for _ in 0..2 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        view.read_with(cx, |view, _| {
            let scroll = &view.outline_scroll;
            assert!(scroll.max_offset().y > px(0.), "the menu scrolls");
            assert!(scroll.offset().y < px(0.), "down to the current question");
            assert!(crate::keyscroll::is_visible(scroll, 39), "which is in view");
        });
    }

    #[gpui::test]
    fn draws_every_block_kind(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let tool = Block::new(BlockKind::ToolCall {
            tool_use_id: "toolu_1".into(),
            name: "read_file".into(),
            summary: "src/lib.rs".into(),
            input: serde_json::json!({"path": "src/lib.rs"}),
            status: domain::block::ToolStatus::Succeeded,
            output: Some("40 lines".into()),
        });
        let blocks = [
            text("# Title\n\n- item `code`"),
            tool,
            Block::new(BlockKind::Error {
                message: "boom".into(),
                hint: Some("retry".into()),
            }),
            Block::new(BlockKind::Thinking {
                text: "hmm".into(),
                collapsed: true,
            }),
        ];
        view.update(cx, |view, cx| {
            view.apply_all(blocks.map(|block| DomainEvent::BlockAdded { block }), cx);
            view.handle_action(&Action::builtin("block.first"), cx);
            view.handle_action(&Action::builtin("block.next"), cx);
            view.handle_action(&Action::builtin("block.toggle"), cx);
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(600.)),
            |_, _| div().size_full().child(view.clone()),
        );
        view.read_with(cx, |view, _| {
            assert!(view.state().entries()[1].expanded);
            assert!(
                view.list.bounds_for_item(3).is_some(),
                "all four rows laid out"
            );
        });
    }

    #[gpui::test]
    fn blocks_sit_in_a_centered_column_of_two_widths(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let chart = Block::new(BlockKind::Chart(domain::block::ChartSpec {
            title: "sizes".into(),
            kind: domain::block::ChartKind::Bar,
            labels: vec!["a".into(), "b".into()],
            series: vec![domain::block::Series {
                name: "s".into(),
                values: vec![1.0, 2.0],
            }],
        }));
        view.update(cx, |view, cx| {
            view.apply_all(
                [text("hola"), chart].map(|block| DomainEvent::BlockAdded { block }),
                cx,
            );
        });
        let window = 2400.0;
        cx.simulate_resize(gpui::size(px(window), px(900.)));
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(window), px(900.)),
            |_, _| div().size_full().child(view.clone()),
        );
        for (selector, max) in [("block-0", PROSE_MAX_WIDTH), ("block-1", WIDE_MAX_WIDTH)] {
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(bounds.size.width <= px(max), "{selector} is capped");
            let left = bounds.origin.x;
            let right = px(window) - (bounds.origin.x + bounds.size.width);
            assert!(
                (left - right).abs() <= px(2.),
                "{selector} is centered: {bounds:?}"
            );
        }
        cx.simulate_resize(gpui::size(px(600.), px(900.)));
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(600.), px(900.)),
            |_, _| div().size_full().child(view.clone()),
        );
        for selector in ["block-0", "block-1"] {
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(
                bounds.size.width >= px(600. - 2. * COLUMN_GUTTER - 1.),
                "{selector} takes the whole narrow window"
            );
        }
    }

    #[gpui::test]
    fn actions_move_selection_and_toggle(cx: &mut TestAppContext) {
        let (sink, rx) = RecordingSink::with_channel();
        let view = timeline(cx, rx);
        for i in 0..3 {
            sink.emit(DomainEvent::BlockAdded {
                block: text(&i.to_string()),
            });
        }
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            assert!(view.handle_action(&Action::builtin("block.first"), cx));
            assert!(view.handle_action(&Action::builtin("block.next"), cx));
            assert_eq!(view.state().selected(), Some(1));
            assert!(view.handle_action(&Action::builtin("block.toggle"), cx));
            assert!(!view.state().entries()[1].expanded);
            assert!(view.handle_action(&Action::builtin("block.last"), cx));
            assert_eq!(view.state().selected(), Some(2));
            assert!(!view.handle_action(&Action::builtin("picker.files"), cx));
        });
    }

    #[gpui::test]
    fn toggle_view_action_flips_the_selected_block(cx: &mut TestAppContext) {
        let (sink, rx) = RecordingSink::with_channel();
        let view = timeline(cx, rx);
        sink.emit(DomainEvent::BlockAdded {
            block: Block::new(BlockKind::Markdown {
                title: Some("README.md".into()),
                source: "# crest".into(),
            }),
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            assert!(view.handle_action(&Action::builtin("block.toggle_view"), cx));
            assert!(!view.state().entries()[0].alt_view, "nothing selected");
            view.handle_action(&Action::builtin("block.last"), cx);
            view.handle_action(&Action::builtin("block.toggle_view"), cx);
            assert!(view.state().entries()[0].alt_view);
        });
    }
}
