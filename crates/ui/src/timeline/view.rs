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
//! a letter is typed into it, a vim key moves the selection). `Esc` that a
//! block widget leaves gives the focus back to the timeline first
//! ([`TimelineView::focus_widget`] is the way in).

use application_core::keymap::{Action, Key, KeyCode, Keymap, default_keymap, parse_keys};
use application_core::ports::events::DomainEvent;
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    AnyElement, App, ClipboardItem, Context, ElementId, EventEmitter, FocusHandle, Focusable,
    FollowMode, InteractiveElement as _, IntoElement, KeyDownEvent, ListAlignment, ListState,
    MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, Window, div,
    list, prelude::FluentBuilder as _, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};

use domain::doc::DocId;

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

/// Which document an "Add to doc" goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocTarget {
    /// This document of the chat (a row of the block menu).
    Doc(DocId),
    /// A new document.
    New,
    /// The document in the panel, else the chat's latest, else a new one
    /// (`/doc add`, the keyboard selection, the selection bar).
    Default,
}

/// A document of the chat, as the block menu lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRef {
    /// Its id.
    pub id: DocId,
    /// Its title.
    pub title: String,
    /// Last change, milliseconds since the Unix epoch.
    pub updated_at: u64,
}

/// What a [`TimelineView`] tells the session.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineEvent {
    /// The person asked to put `markdown` in a document: a block's
    /// markdown (block menu, `/doc add`) or the selected text (selection
    /// bar). `target` says which document. `image` is a picture to save
    /// beside the doc as `(file name, PNG bytes)`: the picture of a chart
    /// block (`markdown` refers to it as `assets/<name>`); `None` for
    /// everything else, and for a chart that cannot be drawn (it goes in as
    /// its table).
    AddToDoc {
        /// What to append.
        markdown: String,
        /// The document it goes to.
        target: DocTarget,
        /// A picture the markdown refers to (`assets/<name>`).
        image: Option<(String, Vec<u8>)>,
    },
}

impl EventEmitter<TimelineEvent> for TimelineView {}

/// Where the popup is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAt {
    /// Under the split button of this block.
    Block(domain::block::BlockId),
    /// Under the "Add to doc" of the reply ending at this entry.
    Reply(usize),
}

/// The timeline area of the main window.
pub struct TimelineView {
    state: TimelineState,
    list: ListState,
    focus_handle: FocusHandle,
    /// The "Copy" hint next to the text selected with the mouse.
    copy_hint: Option<CopyHint>,
    _hint_expiry: Option<Task<()>>,
    /// Whether the selection bar's "Add to doc" popup is open.
    hint_menu: bool,
    /// The reply (by its last block) whose copy button just copied.
    copied_reply: Option<usize>,
    /// The chat's documents, most recently changed first.
    docs: std::rc::Rc<[DocRef]>,
    /// The open "copy / add to doc" popup.
    menu: Option<MenuAt>,
    /// The row under the pointer: its block bar shows without a selection.
    hovered: Option<usize>,
    /// The menu of the user's questions is open.
    outline_open: bool,
    /// The menu's rows, rebuilt when the entries or the selection change
    /// rather than on every frame (it walks every entry).
    outline: super::outline::Outline,
    /// `(entries revision, selection)` `outline` was built for.
    outline_key: Option<(u64, Option<usize>)>,
    /// Bumped whenever an event changed the entries.
    revision: u64,
    /// The scroll of the menu's rows.
    outline_scroll: ScrollHandle,
    /// The menu just opened: scroll its current question into view.
    outline_reveal: bool,
    /// The key being dispatched, when the session deferred it.
    deferred: Option<DeferredKey>,
    /// The keys the welcome screen suggests, from the keymap in use.
    welcome: Vec<(String, String)>,
    /// The line a branched chat starts with, above its blocks.
    pub(crate) banner: super::banner::Banner,
    _reply_expiry: Option<Task<()>>,
    _events: Option<Task<()>>,
}

impl TimelineView {
    /// An empty timeline that only changes through [`Self::apply`].
    pub fn new(cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(OVERDRAW));
        list.set_follow_mode(FollowMode::Tail);
        // The wheel moves the outline's current question (see `follow_scroll`).
        let this = cx.weak_entity();
        list.set_scroll_handler(move |_, _, app| {
            let this = this.clone();
            app.defer(move |cx| {
                this.update(cx, |this, cx| this.follow_scroll(cx)).ok();
            });
        });
        Self {
            state: TimelineState::new(),
            list,
            focus_handle: cx.focus_handle(),
            copy_hint: None,
            _hint_expiry: None,
            hint_menu: false,
            copied_reply: None,
            docs: std::rc::Rc::from(Vec::new()),
            menu: None,
            hovered: None,
            outline_open: false,
            outline: super::outline::Outline { groups: Vec::new() },
            outline_key: None,
            revision: 0,
            outline_scroll: ScrollHandle::new(),
            outline_reveal: false,
            deferred: None,
            welcome: welcome_keys(&default_keymap()),
            banner: super::banner::Banner::default(),
            _reply_expiry: None,
            _events: None,
        }
    }

    /// Takes the chat's documents for the block menu's "Add to" rows
    /// (newest change first; the menu shows the first three).
    pub fn set_docs(&mut self, mut docs: Vec<DocRef>, cx: &mut Context<Self>) {
        docs.sort_by_key(|doc| std::cmp::Reverse(doc.updated_at));
        if *self.docs != *docs {
            self.docs = docs.into();
            cx.notify();
        }
    }

    /// The documents the menu offers, newest change first.
    #[must_use]
    pub fn docs(&self) -> &[DocRef] {
        &self.docs
    }

    /// The keys the welcome screen suggests: keys and what they do.
    pub fn welcome_keys(&self) -> &[(String, String)] {
        &self.welcome
    }

    /// Makes the welcome screen suggest the keys of `keymap` (the one the
    /// prompt runs, with the person's mappings).
    pub fn set_welcome_keys(&mut self, keymap: &Keymap<Action>, cx: &mut Context<Self>) {
        self.welcome = welcome_keys(keymap);
        cx.notify();
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
            self.hint_menu = false;
            if self.copy_hint.take().is_some() {
                cx.notify();
            }
            return;
        }
        window.focus(&self.focus_handle, cx);
        self.hint_menu = false;
        self.copy_hint = Some(CopyHint {
            at: event.position,
            text,
            copied: false,
        });
        cx.notify();
    }

    /// Selects the first block of kind `tag` and opens its copy / add-to-doc
    /// menu (the showcase's `doc-block-menu` scene). Returns whether there
    /// was one.
    pub fn open_menu_of(&mut self, tag: domain::block::BlockTag, cx: &mut Context<Self>) -> bool {
        let found = (0..self.state.len()).find_map(|ix| {
            let entry = self.state.get(ix)?;
            (entry.block.kind.tag() == tag).then_some((ix, entry.block.id))
        });
        let Some((ix, id)) = found else {
            return false;
        };
        let before = self.state.selected();
        self.state.select(ix);
        self.remeasure_selection(before, Some(ix));
        self.menu = Some(MenuAt::Block(id));
        cx.notify();
        true
    }

    /// What adding the block the keyboard selection is on puts in a
    /// document: its markdown and picture (see [`crate::blocks::doc_content`]).
    pub fn selected_block_doc_content(&self) -> Option<crate::blocks::DocContent> {
        let entry = self.state.selected_entry()?;
        crate::blocks::doc_content(&entry.block.kind, entry.block.id)
    }

    /// Emits [`TimelineEvent::AddToDoc`] for the block `id` (the copy row's
    /// entries). Returns whether the block can be added.
    pub fn add_block_to_doc(
        &mut self,
        id: domain::block::BlockId,
        target: DocTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((markdown, image)) = self
            .state
            .position(id)
            .and_then(|ix| self.state.get(ix))
            .and_then(|entry| crate::blocks::doc_content(&entry.block.kind, entry.block.id))
        else {
            return false;
        };
        self.menu = None;
        cx.emit(TimelineEvent::AddToDoc {
            markdown,
            target,
            image,
        });
        cx.notify();
        true
    }

    /// Adds the text selected in the timeline to the chat's document
    /// (the selection bar's "Add to doc"; bind it to `selection.add_to_doc`).
    /// Hides the bar and clears the selection. Returns whether there was
    /// any selected text.
    pub fn add_selection_to_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let live = selection::selected_text(window, cx);
        let text = if live.is_empty() {
            self.copy_hint
                .as_ref()
                .map(|hint| hint.text.clone())
                .unwrap_or_default()
        } else {
            live
        };
        self.add_text_to_doc(text, DocTarget::Default, window, cx)
    }

    fn add_text_to_doc(
        &mut self,
        text: String,
        target: DocTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if text.is_empty() {
            return false;
        }
        self.copy_hint = None;
        self.hint_menu = false;
        gpui_base::TextSelection::clear(window, cx);
        cx.emit(TimelineEvent::AddToDoc {
            markdown: text,
            target,
            image: None,
        });
        cx.notify();
        true
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
        self.hint_menu = false;
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
        let changed = changed || !touched.is_empty();
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    /// Rebuilds `outline` when the entries or the selection changed.
    fn refresh_outline(&mut self) {
        let key = (self.revision, self.state.selected());
        if self.outline_key != Some(key) {
            self.outline =
                super::outline::chat_outline(self.state.entries(), self.state.selected());
            self.outline_key = Some(key);
        }
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
        // `Esc` in a block widget gives the keys back to the timeline;
        // only from the timeline itself does it go on to the prompt.
        if deferred.key == Key::plain(KeyCode::Esc) && !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return;
        }
        let fallback = deferred.fallback;
        window.defer(cx, fallback);
    }

    /// Keys while the timeline has the focus.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if event.keystroke.key == "escape" && self.copy_hint.take().is_some() {
            self.hint_menu = false;
            gpui_base::TextSelection::clear(window, cx);
            self.deferred = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" && self.menu.take().is_some() {
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
        let key = event.keystroke.key.as_str();
        let arrow = arrow_action(key, self.outline_open);
        if arrow.starts_with("outline.") {
            self.refresh_outline();
            if let Some(block) = self.outline.step(arrow == "outline.next") {
                self.choose_outline(block, cx);
            }
            cx.stop_propagation();
            return;
        }
        let action = match key {
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

    /// `block.focus_widget`: the widget of the selected block (the last
    /// one when none is) takes the keys; `Esc` gives them back to the
    /// timeline. Returns false when the block has no keys of its own.
    pub fn focus_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.select_last_if_none(cx);
        let Some(entry) = self.state.selected_entry() else {
            return false;
        };
        let Some(handle) = crate::blocks::block_focus(entry.block.id, cx) else {
            return false;
        };
        window.focus(&handle, cx);
        cx.notify();
        true
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

    /// Selects the menu row. A block already on screen stays where it is;
    /// any other scrolls to the top of the chat.
    fn choose_outline(&mut self, ix: usize, cx: &mut Context<Self>) {
        let before = self.state.selected();
        self.menu = None;
        if self.state.select(ix).is_some() {
            self.remeasure_selection(before, Some(ix));
            if !super::spy::is_on_screen(&self.list, ix) {
                self.list.pause_following_tail();
                self.list.scroll_to(gpui::ListOffset {
                    item_ix: ix,
                    offset_in_item: px(0.),
                });
            }
        }
        cx.notify();
    }

    /// The chat scrolled by the wheel or the scrollbar: the block at the
    /// top becomes the selected one, so the outline's current question
    /// follows the scroll and not only the keys.
    fn follow_scroll(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = super::spy::block_at_top(&self.list) else {
            return;
        };
        let before = self.state.selected();
        if before != Some(ix) && self.state.select(ix).is_some() {
            self.remeasure_selection(before, Some(ix));
            cx.notify();
        }
    }

    /// The chat menu: a rail of marks, and the questions when it is open.
    fn outline_layer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.refresh_outline();
        let outline = &self.outline;
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
                let title = SharedString::from(mark.title.to_owned());
                div()
                    .id(("chat-outline-mark", ix))
                    .debug_selector(move || format!("chat-outline-mark-{ix}"))
                    .w_full()
                    .h(px(12.))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .when(!title.is_empty(), |mark| {
                        mark.tooltip(move |window, cx| {
                            Tooltip::new(title.clone()).build(window, cx)
                        })
                    })
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
                self.outline_reveal =
                    active_row.is_some() && !crate::keyscroll::laid_out(&self.outline_scroll);
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
                        .child("↑↓ questions · Esc closes")
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
    fn reply_footer(&self, ix: usize, view: WeakEntity<Self>, cx: &App) -> Option<AnyElement> {
        if !self.state.is_reply_end(ix) {
            return None;
        }
        let copied = self.copied_reply == Some(ix);
        let copy_view = view.clone();
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
                copy_view
                    .update(cx, |this, cx| this.copy_reply(ix, cx))
                    .ok();
            });
        // This adds the whole reply; the block bar adds one block.
        let open = self.menu == Some(MenuAt::Reply(ix));
        let toggle_view = view.clone();
        let add = div()
            .relative()
            .debug_selector(|| "reply-add".to_owned())
            .child(
                Button::new(("reply-add-doc", ix))
                    .ghost()
                    .xsmall()
                    .label("Add to doc ▾")
                    .tooltip("Add this reply to a document")
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        toggle_view
                            .update(cx, |this, cx| {
                                this.menu = (this.menu != Some(MenuAt::Reply(ix)))
                                    .then_some(MenuAt::Reply(ix));
                                cx.notify();
                            })
                            .ok();
                    }),
            );
        let add = if open {
            let pick_view = view.clone();
            let pick: crate::blocks::copy::PickDoc = std::rc::Rc::new(move |target, cx| {
                pick_view
                    .update(cx, |this, cx| this.add_reply_to_doc(ix, target, cx))
                    .ok();
            });
            add.child(crate::blocks::copy::doc_menu(
                &self.docs,
                "reply-",
                &ix.to_string(),
                &pick,
                cx,
            ))
        } else {
            add
        };
        Some(
            h_flex()
                .pt_1()
                .gap_1()
                .child(button)
                .child(add)
                .into_any_element(),
        )
    }

    /// Emits [`TimelineEvent::AddToDoc`] with the reply that ends at block
    /// `ix` (the reply footer's buttons). Returns whether there was a reply.
    pub fn add_reply_to_doc(
        &mut self,
        ix: usize,
        target: DocTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(markdown) = self.state.reply_to_copy(ix) else {
            return false;
        };
        self.menu = None;
        cx.emit(TimelineEvent::AddToDoc {
            markdown,
            target,
            image: None,
        });
        cx.notify();
        true
    }

    /// Copies the reply that ends at block `ix`; the text is built now,
    /// not on every frame the button is drawn.
    fn copy_reply(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(text) = self.state.reply_to_copy(ix) else {
            return;
        };
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
        let selected = self.state.selected() == Some(ix);
        let id = entry.block.id;
        let open = self.menu == Some(MenuAt::Block(id));
        // The menu's popup sits outside the row: while it is open its bar
        // stays even if the pointer leaves the block.
        let copy_bar = (selected || self.hovered == Some(ix) || open)
            .then(|| {
                let toggle_view = view.clone();
                let toggle: std::rc::Rc<dyn Fn(&mut App)> = std::rc::Rc::new(move |cx| {
                    toggle_view
                        .update(cx, |this, cx| {
                            this.menu =
                                (this.menu != Some(MenuAt::Block(id))).then_some(MenuAt::Block(id));
                            cx.notify();
                        })
                        .ok();
                });
                let weak = view.clone();
                let pick: crate::blocks::copy::PickDoc = std::rc::Rc::new(move |target, cx| {
                    weak.update(cx, |this, cx| this.add_block_to_doc(id, target, cx))
                        .ok();
                });
                let bar = crate::blocks::copy::Bar {
                    open,
                    toggle,
                    docs: self.docs.clone(),
                    pick,
                    text_of: copy_text_of(view.clone()),
                };
                crate::blocks::copy::bar(entry, bar, window, cx)
            })
            .flatten();
        let theme = cx.theme();
        let width = block_width(&entry.block.kind);
        let footer = self.reply_footer(ix, view.clone(), cx);
        // `min_w_0`: content that does not wrap on its own (a form's
        // labels) must not widen the block past the window.
        let block = div()
            .w_full()
            .min_w_0()
            .max_w(width.max_width())
            .px_2()
            .py_1()
            .border_l_2()
            .relative()
            .debug_selector(move || format!("block-{ix}"))
            .child(content)
            .children(copy_bar)
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
            .on_hover({
                let view = view.clone();
                move |hovered, _, cx| {
                    view.update(cx, |this, cx| {
                        let next = if *hovered {
                            Some(ix)
                        } else if this.hovered == Some(ix) {
                            None
                        } else {
                            this.hovered
                        };
                        if this.hovered != next {
                            this.hovered = next;
                            cx.notify();
                        }
                    })
                    .ok();
                }
            })
            // Capture phase: a child that stops the mouse down (the diagram
            // canvas) still selects its row. The click toggles as before.
            .capture_any_mouse_down({
                let view = view.clone();
                move |event, _, cx| {
                    if event.button != gpui::MouseButton::Left {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        let before = this.state.selected();
                        if before != Some(ix) && this.state.select(ix).is_some() {
                            this.remeasure_selection(before, Some(ix));
                            cx.notify();
                        }
                    })
                    .ok();
                }
            })
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

/// Makes the copy-as text of a block of `view` on click, from the block as
/// it is then.
fn copy_text_of(view: WeakEntity<TimelineView>) -> crate::blocks::copy::CopyText {
    std::rc::Rc::new(move |id, key, cx| {
        let view = view.upgrade()?;
        let state = &view.read(cx).state;
        let entry = state.get(state.position(id)?)?;
        crate::blocks::copy::text(&entry.block.kind, key)
    })
}

/// Where an arrow key goes: the menu of questions takes Up and Down only
/// while it is open, any other time they move blocks.
fn arrow_action(key: &str, outline_open: bool) -> &'static str {
    match (key, outline_open) {
        ("up", true) => "outline.prev",
        ("down", true) => "outline.next",
        ("up", false) => "block.prev",
        ("down", false) => "block.next",
        _ => "none",
    }
}

impl Focusable for TimelineView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The keys of `action` in `keymap`: `preferred` (notation) when it is one
/// of them, else the shortest.
fn keys_of(keymap: &Keymap<Action>, action: &str, preferred: Option<&str>) -> Option<String> {
    let wanted = Action::builtin(action);
    preferred
        .and_then(|notation| parse_keys(notation, keymap.leader()).ok())
        .filter(|keys| keymap.lookup(keys).exact == Some(&wanted))
        .map(|keys| crate::help::keys_label(&keys))
        .or_else(|| crate::help::action_keys(keymap, action))
}

/// The keys the welcome screen suggests, from `keymap`: the leader with
/// what follows it, then the focus, the prompt, cancelling and copying.
/// An action without keys is left out.
pub fn welcome_keys(keymap: &Keymap<Action>) -> Vec<(String, String)> {
    let mut rows = vec![(
        "/".to_owned(),
        "commands: /research, /implement, /!cmd, /help…".to_owned(),
    )];
    let leader = parse_keys("<leader>", keymap.leader()).unwrap_or_default();
    let entries = keymap.entries();
    // What follows the leader for `action`, if it is under the leader.
    let after_leader = |action: &str| {
        entries
            .iter()
            .filter(|e| matches!(&e.action, Action::Builtin(name) if name == action))
            .filter_map(|e| e.keys.strip_prefix(leader.as_slice()))
            .filter(|rest| !rest.is_empty())
            .min_by_key(|rest| rest.len())
            .map(crate::help::keys_label)
    };
    let mut shortcuts: Vec<String> = Vec::new();
    for (action, what) in [
        ("panel.tree", "tree"),
        ("picker.files", "files"),
        ("picker.grep", "grep"),
        ("model.pick", "model"),
    ] {
        if let Some(keys) = after_leader(action) {
            shortcuts.push(format!("{keys} {what}"));
        }
    }
    match (after_leader("block.next"), after_leader("block.prev")) {
        (Some(next), Some(prev)) => shortcuts.push(format!("{next}/{prev} blocks")),
        (Some(keys), None) | (None, Some(keys)) => shortcuts.push(format!("{keys} blocks")),
        (None, None) => {}
    }
    if let Some(keys) = after_leader("help.open") {
        shortcuts.push(format!("{keys} help"));
    }
    if !leader.is_empty() && !shortcuts.is_empty() {
        rows.push((
            crate::help::keys_label(&leader),
            format!("shortcuts: {}", shortcuts.join(" · ")),
        ));
    }
    for (action, preferred, what) in [
        (
            "focus.next",
            Some("<C-Tab>"),
            "move the focus: prompt → timeline → tree",
        ),
        ("focus.input", None, "back to the prompt"),
        (
            "input.cancel",
            None,
            "clear the prompt or interrupt the turn",
        ),
        (
            "selection.copy",
            Some("<C-S-c>"),
            "copy the selected text, from the prompt or the chat",
        ),
    ] {
        if let Some(keys) = keys_of(keymap, action, preferred) {
            rows.push((keys, what.to_owned()));
        }
    }
    rows
}

/// The empty timeline: the crest mark and the keys to start with.
fn welcome(keys: &[(String, String)], cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let rows = keys.iter().map(|(keys, what)| {
        h_flex()
            .gap_3()
            .child(
                div()
                    .w(px(110.))
                    .text_color(theme.primary)
                    .child(SharedString::from(keys.clone())),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(what.clone())),
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

/// The blocks: the list, its scrollbar, the "Copy" hint and the outline.
fn blocks_layer(
    this: &mut TimelineView,
    window: &mut Window,
    cx: &mut Context<TimelineView>,
) -> AnyElement {
    let view = cx.entity().downgrade();
    // The hint outlives the selection only while it says "Copied".
    // While its popup is open the bar stays (the text is already captured).
    if let Some(hint) = &this.copy_hint
        && !hint.copied
        && !this.hint_menu
        && selection::selected_text(window, cx).is_empty()
    {
        this.copy_hint = None;
    }
    if this.copy_hint.is_none() {
        this.hint_menu = false;
    }
    let hint = this.copy_hint.as_ref().map(|hint| {
        let weak = view.clone();
        let text = hint.text.clone();
        let toggle_weak = view.clone();
        let menu = this.hint_menu.then(|| {
            let pick_view = view.clone();
            let pick_text = hint.text.clone();
            let handle = window.window_handle();
            let pick: crate::blocks::copy::PickDoc = std::rc::Rc::new(move |target, cx| {
                let (view, text) = (pick_view.clone(), pick_text.clone());
                // The click is being dispatched in this window: clear the
                // selection once it is done.
                cx.defer(move |cx| {
                    handle
                        .update(cx, move |_, window, cx| {
                            view.update(cx, |this, cx| {
                                this.add_text_to_doc(text, target, window, cx);
                            })
                            .ok();
                        })
                        .ok();
                });
            });
            crate::blocks::copy::doc_popup(&this.docs, "hint-", "selection", &pick, cx)
                .into_any_element()
        });
        render_hint(
            hint,
            move |_window, cx| {
                weak.update(cx, |this, cx| this.copy(text.clone(), cx)).ok();
            },
            move |_window, cx| {
                toggle_weak
                    .update(cx, |this, cx| {
                        this.hint_menu = !this.hint_menu;
                        cx.notify();
                    })
                    .ok();
            },
            menu,
            cx,
        )
    });
    // The scrollbar thumb moves no wheel event: follow it here.
    if this.list.is_scrollbar_dragging() {
        this.follow_scroll(cx);
    }
    let outline = this.outline_layer(cx);
    div()
        .id("timeline-body")
        .debug_selector(|| "timeline-body".to_owned())
        .relative()
        .w_full()
        .flex_1()
        .min_h_0()
        .child(
            list(this.list.clone(), move |ix, window, cx| {
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
        .vertical_scrollbar(&this.list)
        .children(hint)
        .children(outline)
        .into_any_element()
}

impl Render for TimelineView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let edge = if focused {
            crate::theme::token(cx, "list.focused.border")
        } else {
            gpui::transparent_black()
        };
        let body = if self.state.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .child(welcome(&self.welcome, cx))
                .into_any_element()
        } else {
            blocks_layer(self, window, cx)
        };
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
            .size_full()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(edge)
            .children(self.banner.element(cx))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{PROSE_MAX_WIDTH, WIDE_MAX_WIDTH};
    use application_core::ports::events::EventSink as _;
    use application_testing::system::RecordingSink;
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
        // Open, the arrows walk the questions (not the blocks); after Esc
        // they move blocks again.
        assert_eq!(arrow_action("up", false), "block.prev");
        let steps = [("up", 76, true), ("down", 78, true), ("down", 78, true)]
            .into_iter()
            .chain([("escape", 78, false), ("up", 77, false)]);
        for (key, selected, open) in steps {
            cx.simulate_keystrokes(key);
            let held = view.read_with(cx, |v, _| (v.state().selected(), v.outline_open()));
            assert_eq!(held, (Some(selected), open), "after {key}");
        }
    }

    /// A chat of `n` questions with a reply each, laid out once.
    fn long_chat(
        cx: &mut TestAppContext,
        n: usize,
    ) -> (Entity<TimelineView>, &mut VisualTestContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        view.update(cx, |view, cx| {
            let blocks: Vec<_> = (0..n)
                .flat_map(|n| [text(&format!("> question {n}")), text("reply")])
                .map(|block| DomainEvent::BlockAdded { block })
                .collect();
            view.apply_all(blocks, cx);
        });
        for _ in 0..2 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        (view, cx)
    }

    #[gpui::test]
    fn the_outline_follows_the_scroll(cx: &mut TestAppContext) {
        let (view, cx) = long_chat(cx, 40);
        let selected = |cx: &mut VisualTestContext| view.read_with(cx, |v, _| v.state().selected());
        let at_top = |cx: &mut VisualTestContext| {
            view.read_with(cx, |v, _| super::super::spy::block_at_top(&v.list))
        };
        // The wheel, up and then down: each stop selects the block at the top.
        for wheel in [900., 900., -300., -90000., -90000., -90000.] {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: gpui::point(px(400.), px(300.)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(wheel))),
                ..Default::default()
            });
            // Twice: the selected block's copy row changes its height, and
            // the list settles on the frame after.
            for _ in 0..6 {
                cx.update(|window, _| window.refresh());
                cx.run_until_parked();
            }
            assert_eq!(selected(cx), at_top(cx), "after a wheel of {wheel}");
        }
        assert_eq!(selected(cx), Some(79), "the bottom is the last block");
        // A block on screen is chosen where it stands; a far one scrolls.
        view.update(cx, |v, cx| v.choose_outline(78, cx));
        let top = |cx: &mut VisualTestContext| {
            view.read_with(cx, |v, _| v.list.logical_scroll_top().item_ix)
        };
        assert_ne!(top(cx), 78, "78 is on screen: no scroll");
        view.update(cx, |v, cx| v.choose_outline(0, cx));
        assert_eq!(top(cx), 0, "0 is far: it goes to the top");
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

    /// Records every [`TimelineEvent`] `view` emits.
    fn record_events(
        view: &Entity<TimelineView>,
        cx: &mut VisualTestContext,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<TimelineEvent>>> {
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = events.clone();
        cx.update(|_, app| {
            app.subscribe(view, move |_, event: &TimelineEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
        });
        events
    }

    #[gpui::test]
    fn add_to_doc_fires_from_the_copy_row_and_the_keyboard_selection(cx: &mut TestAppContext) {
        use application_core::keymap::Action;
        use gpui::{Modifiers, point, size};

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        let kind = BlockKind::Table(domain::block::TableSpec {
            columns: vec![domain::block::Column::new("a")],
            rows: vec![vec![domain::block::Cell::Number(1.0)]],
        });
        let expected = domain::doc::block_markdown(&kind, None).unwrap_or_default();
        let table = Block::new(kind);
        view.update(cx, |view, cx| {
            view.apply_all([table].map(|block| DomainEvent::BlockAdded { block }), cx);
            view.handle_action(&Action::builtin("block.last"), cx);
        });
        assert_eq!(
            view.read_with(cx, |view, _| view
                .selected_block_doc_content()
                .map(|(markdown, _)| markdown)),
            Some(expected.clone())
        );
        cx.draw(point(px(0.), px(0.)), size(px(800.), px(600.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        let doc = doc_ref("Plan", 1);
        let chosen = doc.id;
        // Without docs one row adds to the default; with one, rows name it.
        let mut rounds = vec![("add-to-doc", DocTarget::Default)];
        rounds.extend([
            ("add-to-doc-0", DocTarget::Doc(chosen)),
            ("add-to-new-doc", DocTarget::New),
        ]);
        for (n, (selector, target)) in rounds.into_iter().enumerate() {
            if n == 1 {
                view.update(cx, |view, cx| view.set_docs(vec![doc.clone()], cx));
            }
            let more = cx
                .debug_bounds("copy-more")
                .unwrap_or_else(|| panic!("no chevron"));
            cx.simulate_click(more.center(), Modifiers::none());
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            let button = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("no {selector}"));
            cx.simulate_click(button.center(), Modifiers::none());
            assert_eq!(
                events.borrow().last(),
                Some(&TimelineEvent::AddToDoc {
                    markdown: expected.clone(),
                    target,
                    image: None,
                })
            );
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        assert_eq!(events.borrow().len(), 3);
    }

    fn redraw(cx: &mut VisualTestContext, view: &Entity<TimelineView>) {
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(900.)),
            |_, _| div().size_full().child(view.clone()),
        );
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_block_bar_shows_while_the_pointer_is_over_the_row(cx: &mut TestAppContext) {
        use gpui::Modifiers;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let table = Block::new(BlockKind::Table(domain::block::TableSpec {
            columns: vec![domain::block::Column::new("a")],
            rows: vec![vec![domain::block::Cell::Number(1.0)]],
        }));
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
                [table, chart].map(|block| DomainEvent::BlockAdded { block }),
                cx,
            );
        });
        redraw(cx, &view);
        assert!(view.read_with(cx, |v, _| v.state().selected()).is_none());
        assert!(cx.debug_bounds("copy-row").is_none());
        for (ix, selector) in [(0, "block-0"), (1, "block-1")] {
            let row = cx.debug_bounds(selector).expect("row");
            cx.simulate_mouse_move(row.center(), None, Modifiers::none());
            redraw(cx, &view);
            assert!(cx.debug_bounds("copy-row").is_some(), "no bar on {ix}");
            cx.simulate_mouse_move(gpui::point(px(799.), px(899.)), None, Modifiers::none());
            redraw(cx, &view);
            assert!(cx.debug_bounds("copy-row").is_none(), "bar stuck on {ix}");
        }
    }

    #[gpui::test]
    fn a_mouse_down_on_a_diagram_selects_its_block(cx: &mut TestAppContext) {
        use gpui::Modifiers;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let diagram = Block::new(BlockKind::Mermaid(domain::block::MermaidSpec {
            source: "graph TD; A-->B".into(),
            title: None,
        }));
        view.update(cx, |view, cx| {
            view.apply_all([diagram].map(|block| DomainEvent::BlockAdded { block }), cx);
        });
        redraw(cx, &view);
        let row = cx.debug_bounds("block-0").expect("row");
        cx.simulate_click(row.center(), Modifiers::none());
        redraw(cx, &view);
        assert_eq!(view.read_with(cx, |v, _| v.state().selected()), Some(0));
        assert!(cx.debug_bounds("copy-row").is_some());
    }

    #[gpui::test]
    fn a_text_block_adds_itself_to_a_doc(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        let block = Block::new(BlockKind::Text {
            markdown: "Some **reply**".into(),
        });
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([block].map(|block| DomainEvent::BlockAdded { block }), cx);
        });
        let added = view.update(cx, |view, cx| {
            view.add_block_to_doc(id, DocTarget::Default, cx)
        });
        assert!(added);
        assert_eq!(
            events.borrow().last(),
            Some(&TimelineEvent::AddToDoc {
                markdown: "Some **reply**".into(),
                target: DocTarget::Default,
                image: None,
            })
        );
    }

    fn doc_ref(title: &str, updated_at: u64) -> DocRef {
        DocRef {
            id: DocId::new(),
            title: title.into(),
            updated_at,
        }
    }

    /// Opens the menu of the selected table and lists its "Add to" rows
    /// (as `(selector, id)`: the debug selectors found, in order).
    fn menu_rows(cx: &mut gpui::VisualTestContext) -> Vec<String> {
        use gpui::Modifiers;
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        draw(cx);
        // The menu stays open between calls: open it only when it is closed.
        if cx.debug_bounds("copy-csv").is_none() {
            let more = cx
                .debug_bounds("copy-more")
                .unwrap_or_else(|| panic!("no chevron"));
            cx.simulate_click(more.center(), Modifiers::none());
            draw(cx);
        }
        let mut found = Vec::new();
        for selector in [
            "add-to-doc-0",
            "add-to-doc-1",
            "add-to-doc-2",
            "add-to-doc-3",
        ]
        .into_iter()
        .chain(["add-to-new-doc", "add-to-doc"])
        {
            if cx.debug_bounds(selector).is_some() {
                found.push(selector.to_owned());
            }
        }
        found
    }

    #[gpui::test]
    fn the_menu_lists_the_three_newest_docs_then_a_new_one(cx: &mut TestAppContext) {
        use application_core::keymap::Action;
        use gpui::Modifiers;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        let table = Block::new(BlockKind::Table(domain::block::TableSpec {
            columns: vec![domain::block::Column::new("a")],
            rows: vec![vec![domain::block::Cell::Number(1.0)]],
        }));
        view.update(cx, |view, cx| {
            view.apply_all([table].map(|block| DomainEvent::BlockAdded { block }), cx);
            view.handle_action(&Action::builtin("block.last"), cx);
        });
        // No docs: one row that makes the first.
        assert_eq!(menu_rows(cx), ["add-to-doc"]);

        let docs = [
            doc_ref("old", 10),
            doc_ref("newest", 40),
            doc_ref("middle", 30),
            doc_ref("second", 35),
        ];
        let ids = docs.clone().map(|doc| doc.id);
        // Three docs, set out of order: the newest change first.
        view.update(cx, |view, cx| view.set_docs(docs[..3].to_vec(), cx));
        assert_eq!(
            view.read_with(cx, |view, _| view
                .docs()
                .iter()
                .map(|doc| doc.title.clone())
                .collect::<Vec<_>>()),
            ["newest", "middle", "old"]
        );
        assert_eq!(
            menu_rows(cx),
            [
                "add-to-doc-0",
                "add-to-doc-1",
                "add-to-doc-2",
                "add-to-new-doc"
            ]
        );
        let label = |cx: &mut gpui::VisualTestContext, doc: &DocRef| {
            let _ = cx;
            crate::blocks::copy::add_label(doc)
        };
        assert_eq!(label(cx, &docs[1]), "Add to \"newest\"");

        // Four docs: only the three newest.
        view.update(cx, |view, cx| view.set_docs(docs.to_vec(), cx));
        assert_eq!(
            menu_rows(cx),
            [
                "add-to-doc-0",
                "add-to-doc-1",
                "add-to-doc-2",
                "add-to-new-doc"
            ]
        );
        // The second row is the second newest change.
        let second = cx
            .debug_bounds("add-to-doc-1")
            .unwrap_or_else(|| panic!("no second row"));
        cx.simulate_click(second.center(), Modifiers::none());
        assert!(matches!(
            events.borrow().last(),
            Some(TimelineEvent::AddToDoc { target, .. }) if *target == DocTarget::Doc(ids[3])
        ));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(cx.debug_bounds("add-to-doc-1").is_none(), "the menu closes");
    }

    #[gpui::test]
    fn blocks_without_markdown_cannot_be_added(cx: &mut TestAppContext) {
        use application_core::keymap::Action;
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        let shell = Block::new(BlockKind::ToolCall {
            tool_use_id: "t".into(),
            name: "read_file".into(),
            summary: "x".into(),
            input: serde_json::json!({}),
            status: domain::block::ToolStatus::Succeeded,
            output: None,
        });
        let id = shell.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block: shell }], cx);
            view.handle_action(&Action::builtin("block.last"), cx);
            assert!(!view.add_block_to_doc(id, DocTarget::New, cx));
        });
        assert_eq!(
            view.read_with(cx, |view, _| view
                .selected_block_doc_content()
                .map(|(markdown, _)| markdown)),
            None
        );
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn the_selection_bar_adds_the_selected_text(cx: &mut TestAppContext) {
        use gpui::point;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        view.update(cx, |view, cx| {
            view.apply_all(
                [text("pick this")].map(|block| DomainEvent::BlockAdded { block }),
                cx,
            );
        });
        view.update_in(cx, |view, window, cx| {
            // The bar as the mouse-up handler leaves it (set here: a redraw
            // would drop it, there being no live selection in the test).
            view.copy_hint = Some(CopyHint {
                at: point(px(40.), px(40.)),
                text: "pick this".into(),
                copied: false,
            });
            assert!(view.add_selection_to_doc(window, cx));
            assert!(view.copy_hint().is_none(), "the bar hides");
            assert!(!view.add_selection_to_doc(window, cx), "nothing left");
        });
        assert_eq!(
            events.borrow().as_slice(),
            [TimelineEvent::AddToDoc {
                markdown: "pick this".into(),
                target: DocTarget::Default,
                image: None,
            }]
        );
    }

    /// Shows the selection bar with its popup open, as a click on "Add to
    /// doc" leaves it. The test window has no live text selection, so the
    /// open popup is what keeps the bar through a redraw.
    fn open_hint_menu(cx: &mut VisualTestContext, view: &Entity<TimelineView>) {
        view.update(cx, |view, cx| {
            view.copy_hint = Some(CopyHint {
                at: gpui::point(px(40.), px(40.)),
                text: "pick this".into(),
                copied: false,
            });
            view.hint_menu = true;
            cx.notify();
        });
        // `cx.draw` of a bar (a deferred, anchored element) trips GPUI's
        // frame arena in tests; a refresh of the drawn window does not.
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_selection_bar_asks_which_document(cx: &mut TestAppContext) {
        use gpui::Modifiers;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        let doc = doc_ref("Plan", 1);
        let chosen = doc.id;
        view.update(cx, |view, cx| {
            view.set_docs(vec![doc.clone()], cx);
            view.apply_all(
                [text("pick this")].map(|block| DomainEvent::BlockAdded { block }),
                cx,
            );
        });
        redraw(cx, &view);
        open_hint_menu(cx, &view);
        let picked = String::from("pick this");
        let chip = cx.debug_bounds("add-to-doc-hint").expect("chip");
        assert!(cx.debug_bounds("hint-add-to-doc-0").is_some());
        assert!(cx.debug_bounds("hint-add-to-new-doc").is_some());
        assert!(events.borrow().is_empty(), "opening adds nothing");
        // A click on the chip closes it, adding nothing.
        cx.simulate_click(chip.center(), Modifiers::none());
        assert!(!view.read_with(cx, |v, _| v.hint_menu));
        assert!(events.borrow().is_empty());
        open_hint_menu(cx, &view);
        let row = cx.debug_bounds("hint-add-to-doc-0").expect("row");
        cx.simulate_click(row.center(), Modifiers::none());
        assert_eq!(
            events.borrow().as_slice(),
            [TimelineEvent::AddToDoc {
                markdown: picked,
                target: DocTarget::Doc(chosen),
                image: None,
            }]
        );
        assert!(view.read_with(cx, |v, _| v.copy_hint().is_none()));
        assert!(!view.read_with(cx, |v, _| v.hint_menu));
    }

    #[gpui::test]
    fn the_selection_bar_without_docs_adds_to_the_default(cx: &mut TestAppContext) {
        use gpui::Modifiers;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        view.update(cx, |view, cx| {
            view.apply_all(
                [text("pick this")].map(|block| DomainEvent::BlockAdded { block }),
                cx,
            );
        });
        redraw(cx, &view);
        open_hint_menu(cx, &view);
        let row = cx.debug_bounds("hint-add-to-doc").expect("row");
        cx.simulate_click(row.center(), Modifiers::none());
        assert!(matches!(
            events.borrow().as_slice(),
            [TimelineEvent::AddToDoc {
                target: DocTarget::Default,
                ..
            }]
        ));
    }

    #[gpui::test]
    fn adding_a_chart_to_a_doc_sends_its_picture(cx: &mut TestAppContext) {
        use domain::block::{ChartKind, ChartSpec, Series};

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        let block = Block::new(BlockKind::Chart(ChartSpec {
            title: "Sales by month".into(),
            kind: ChartKind::Line,
            labels: vec!["jan".into(), "feb".into(), "mar".into()],
            series: vec![Series {
                name: "units".into(),
                values: vec![3.0, 5.0, 4.0],
            }],
        }));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([block].map(|block| DomainEvent::BlockAdded { block }), cx);
            assert!(view.add_block_to_doc(id, DocTarget::Default, cx));
        });
        let events = events.borrow();
        let [
            TimelineEvent::AddToDoc {
                markdown, image, ..
            },
        ] = events.as_slice()
        else {
            panic!("one event expected: {events:?}");
        };
        let (name, png) = image.as_ref().expect("a picture");
        assert!(
            name.starts_with("sales-by-month-") && name.ends_with(".png"),
            "{name}"
        );
        assert_eq!(&png[..4], b"\x89PNG");
        assert_eq!(markdown, &format!("![Sales by month](assets/{name})"));
    }

    #[gpui::test]
    fn a_reply_footer_adds_the_reply_to_a_doc(cx: &mut TestAppContext) {
        use gpui::{Modifiers, point, size};

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let events = record_events(&view, cx);
        view.update(cx, |view, cx| {
            view.apply_all(
                [text("> question"), text("the reply")]
                    .map(|block| DomainEvent::BlockAdded { block }),
                cx,
            );
        });
        cx.draw(point(px(0.), px(0.)), size(px(800.), px(600.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| view.set_docs(vec![doc_ref("Plan", 5)], cx));
        for (open, selector, target) in [
            ("reply-add-doc", "reply-add-to-doc-0", None),
            (
                "reply-add-doc",
                "reply-add-to-new-doc",
                Some(DocTarget::New),
            ),
        ] {
            let draw = |cx: &mut gpui::VisualTestContext| {
                cx.update(|window, _| window.refresh());
                cx.run_until_parked();
            };
            draw(cx);
            let more = cx
                .debug_bounds("reply-add")
                .unwrap_or_else(|| panic!("no {open}"));
            cx.simulate_click(more.center(), Modifiers::none());
            draw(cx);
            let button = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("no {selector}"));
            cx.simulate_click(button.center(), Modifiers::none());
            let doc = view.read_with(cx, |view, _| view.docs()[0].id);
            assert_eq!(
                events.borrow().last(),
                Some(&TimelineEvent::AddToDoc {
                    markdown: "the reply".into(),
                    target: target.unwrap_or(DocTarget::Doc(doc)),
                    image: None,
                })
            );
        }
    }
}
