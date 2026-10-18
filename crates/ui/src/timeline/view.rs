//! GPUI view of the timeline: a virtualized [`list`] over a
//! [`TimelineState`], fed by a channel of domain events.
//!
//! Blocks have variable heights (streamed markdown, expanded tool calls), so
//! the view uses `list` with a [`ListState`] rather than `uniform_list`; only
//! the visible rows (plus overdraw) are laid out each frame. The list follows
//! the tail while new blocks arrive and stops following when the user moves
//! the selection up.

use application::keymap::Action;
use application::ports::events::DomainEvent;
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    AnyElement, App, ClipboardItem, Context, ElementId, FocusHandle, Focusable, FollowMode,
    InteractiveElement as _, IntoElement, ListAlignment, ListState, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, Window, div, list, px,
};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, h_flex};

use super::{TimelineChange, TimelineState, copy_text, render_block};

/// Pixels rendered above and below the viewport to avoid pop-in on scroll.
const OVERDRAW: f32 = 512.0;

/// The timeline area of the main window.
pub struct TimelineView {
    state: TimelineState,
    list: ListState,
    focus_handle: FocusHandle,
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
            _events: None,
        }
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
        match self.state.apply(event) {
            TimelineChange::None => false,
            TimelineChange::Inserted(ix) => {
                self.list.splice(ix..ix, 1);
                true
            }
            TimelineChange::Updated(ix) => {
                self.list.remeasure_items(ix..ix + 1);
                true
            }
        }
    }

    /// Runs a normal-mode timeline action (`block.next`, `block.toggle`...).
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
            // Charts are the only blocks with an alternate view (T3.8).
            "block.toggle_view" => {}
            _ => return false,
        }
        true
    }

    fn select_with(
        &mut self,
        select: impl FnOnce(&mut TimelineState) -> Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = select(&mut self.state) else {
            return;
        };
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

    fn click(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.state.select(ix).is_some() {
            if let Some(ix) = self.state.toggle_selected() {
                self.list.remeasure_items(ix..ix + 1);
            }
            cx.notify();
        }
    }

    fn render_item(&self, ix: usize, view: WeakEntity<Self>, cx: &App) -> AnyElement {
        let Some(entry) = self.state.get(ix) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let selected = self.state.selected() == Some(ix);
        let row = div()
            .id(ElementId::Integer(ix as u64))
            .w_full()
            .px_2()
            .py_1()
            .border_l_2()
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.click(ix, cx)).ok();
            })
            .child(render_block(entry, cx));
        if selected {
            row.border_color(theme.list_active_border)
                .bg(theme.list_active)
                .into_any_element()
        } else {
            row.border_color(gpui::transparent_black())
                .into_any_element()
        }
    }
}

impl Focusable for TimelineView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TimelineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.state.is_empty() {
            return h_flex()
                .size_full()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("No messages yet")
                .into_any_element();
        }
        let view = cx.entity().downgrade();
        div()
            .id("timeline-list")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .child(
                list(self.list.clone(), move |ix, _window, cx| {
                    let Some(entity) = view.upgrade() else {
                        return div().into_any_element();
                    };
                    entity.read(cx).render_item(ix, view.clone(), cx)
                })
                .size_full(),
            )
            .vertical_scrollbar(&self.list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::ports::events::EventSink as _;
    use application::testing::system::RecordingSink;
    use domain::block::{Block, BlockKind};
    use gpui::{Entity, TestAppContext};

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
}
