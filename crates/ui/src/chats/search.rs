//! The sidebar's search: a field in the full width that filters the chats
//! by title. The strip and the hidden rows open it through
//! [`ChatsPanel::open_search`], which switches to the full width first.

use gpui::{AppContext as _, Context, Entity, Focusable as _, Window};
use gpui_component::input::{InputEvent, InputState};

use super::{ChatSearch, ChatsPanel, SidebarWidth};

impl ChatsPanel {
    /// A search is filtering the list.
    pub(super) fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Whether the project `root` shows its chats: a search unfolds all.
    pub(super) fn is_open(&self, root: &str) -> bool {
        self.searching() || self.expanded.contains(root)
    }

    /// Opens the search field at the full width and gives it the keyboard.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_width(SidebarWidth::Full, cx);
        let input = match &self.search {
            Some(input) => input.clone(),
            None => self.new_search(window, cx),
        };
        let handle = input.read(cx).focus_handle(cx);
        // The click that opened the field finishes after this handler.
        window.focus(&handle, cx);
        window.defer(cx, move |window, cx| window.focus(&handle, cx));
        cx.notify();
    }

    fn new_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search chats"));
        cx.set_global(ChatSearch(input.downgrade()));
        self._search_events = Some(cx.subscribe_in(&input, window, Self::on_search_event));
        self.search = Some(input.clone());
        input
    }

    /// Closes the field and shows every chat again.
    pub(super) fn close_search(&mut self, cx: &mut Context<Self>) {
        if self.search.take().is_none() {
            return;
        }
        self._search_events = None;
        self.query.clear();
        self.refilter();
        cx.notify();
    }

    fn on_search_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.query = input.read(cx).value().to_string();
                self.refilter();
                cx.notify();
            }
            // Enter hands the keyboard to the filtered list.
            InputEvent::PressEnter { .. } => self.focus(window, cx),
            _ => {}
        }
    }
}
