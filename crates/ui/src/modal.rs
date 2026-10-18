//! Plugin modals: [`ModalHost`] draws the modals that plugins open with
//! `app.modal.open` over the rest of the window.
//!
//! The host is an overlay of the root view, always mounted and empty while
//! no modal is open (the same pattern as the approval dialog). It keeps a
//! stack by id: opening an id that is already open replaces that modal and
//! brings it to the front; updating replaces its content; closing removes
//! it. Each modal is a card placed at [`modal_bounds`] for the current
//! window size (so it follows resizes), with a header (title, plugin,
//! close button) and a body that scrolls both ways when the content is
//! larger. A dimmed backdrop covers the window behind the cards.
//!
//! `Esc` (or a click on the backdrop) closes the front modal. The host
//! takes the focus when the first modal opens and gives it back when the
//! last one closes.
//!
//! Events come through [`ModalHost::post`] (the session forwards the
//! modal events of the controller's channel) and are applied in order on
//! the window's executor.

use domain::block::{BlockId, CustomNode};
use domain::modal::{ModalId, ModalSpec, modal_bounds};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, Context, FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use uuid::Uuid;

use application::ports::events::DomainEvent;

/// A modal on screen.
struct Open {
    spec: ModalSpec,
    /// Parent id of its stateful nodes (tables, images...).
    node_id: BlockId,
    scroll: ScrollHandle,
}

/// Id of the stateful nodes of modal `id`: stable for the same id, so an
/// update keeps the state of nodes that did not change.
pub fn modal_node_id(id: &ModalId) -> BlockId {
    // FNV-1a, 128 bits.
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let hash = id.as_str().bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u128::from(byte)).wrapping_mul(PRIME)
    });
    BlockId(Uuid::from_u128(hash))
}

/// The overlay that shows plugin modals.
pub struct ModalHost {
    focus_handle: FocusHandle,
    stack: Vec<Open>,
    restore_focus: Option<FocusHandle>,
    inbox: mpsc::UnboundedSender<DomainEvent>,
    _inbox: Task<()>,
}

impl ModalHost {
    /// An empty host.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<DomainEvent>();
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.apply(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            stack: Vec::new(),
            restore_focus: None,
            inbox: tx,
            _inbox: task,
        }
    }

    /// Queues a modal event; it is applied on the window's executor, in
    /// order. Other events are ignored.
    pub fn post(&self, event: DomainEvent) {
        if event.is_modal() {
            // The task ends only with the host, and then nothing is shown.
            self.inbox.unbounded_send(event).ok();
        }
    }

    /// Applies a modal event now.
    pub fn apply(&mut self, event: DomainEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            DomainEvent::ModalOpened { spec } => self.open(spec, window, cx),
            DomainEvent::ModalUpdated { id, content } => self.update(&id, content, cx),
            DomainEvent::ModalClosed { id } => self.close(&id, window, cx),
            _ => {}
        }
    }

    fn open(&mut self, spec: ModalSpec, window: &mut Window, cx: &mut Context<Self>) {
        let scroll = match self.stack.iter().position(|open| open.spec.id == spec.id) {
            Some(ix) => self.stack.remove(ix).scroll,
            None => ScrollHandle::new(),
        };
        if self.stack.is_empty() {
            self.restore_focus = window.focused(cx);
        }
        self.stack.push(Open {
            node_id: modal_node_id(&spec.id),
            spec,
            scroll,
        });
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn update(&mut self, id: &ModalId, content: CustomNode, cx: &mut Context<Self>) {
        if let Some(open) = self.stack.iter_mut().find(|open| &open.spec.id == id) {
            open.spec.content = content;
            cx.notify();
        }
    }

    /// Closes the modal `id`, if open.
    pub fn close(&mut self, id: &ModalId, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.stack.len();
        self.stack.retain(|open| &open.spec.id != id);
        if self.stack.len() != before {
            self.closed(window, cx);
        }
    }

    /// Closes the front modal; `false` when none is open.
    pub fn close_top(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.stack.pop().is_none() {
            return false;
        }
        self.closed(window, cx);
        true
    }

    fn closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stack.is_empty() {
            if let Some(handle) = self.restore_focus.take() {
                window.focus(&handle, cx);
            }
        } else {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    /// Whether any modal is open.
    pub fn is_open(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Ids of the open modals, back to front.
    pub fn ids(&self) -> Vec<ModalId> {
        self.stack.iter().map(|open| open.spec.id.clone()).collect()
    }

    /// The spec (with its current content) of modal `id`.
    pub fn spec(&self, id: &ModalId) -> Option<&ModalSpec> {
        self.stack
            .iter()
            .find(|open| &open.spec.id == id)
            .map(|open| &open.spec)
    }

    /// The scroll position of modal `id`'s body.
    pub fn scroll_handle(&self, id: &ModalId) -> Option<&ScrollHandle> {
        self.stack
            .iter()
            .find(|open| &open.spec.id == id)
            .map(|open| &open.scroll)
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let m = &keys.modifiers;
        if keys.key == "escape"
            && !(m.control || m.alt || m.platform || m.shift)
            && self.close_top(window, cx)
        {
            cx.stop_propagation();
        }
    }

    fn render_card(
        &self,
        ix: usize,
        open: &Open,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let viewport = window.viewport_size();
        let rect = modal_bounds(
            f32::from(viewport.width),
            f32::from(viewport.height),
            &open.spec,
        );
        let body = crate::blocks::custom::render_node(open.node_id, &open.spec.content, window, cx);
        let theme = cx.theme();
        let id = open.spec.id.clone();
        let subtitle = if open.spec.title == open.spec.plugin {
            None
        } else {
            Some(open.spec.plugin.clone())
        };
        let header = h_flex()
            .gap_2()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(open.spec.title.clone())),
            )
            .children(subtitle.map(|plugin| {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(plugin))
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Esc"),
            )
            .child(
                Button::new(("modal-close", ix))
                    .ghost()
                    .small()
                    .label("✕")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close(&id, window, cx);
                    })),
            );
        v_flex()
            .id(("modal", ix))
            .debug_selector(|| "modal-card".to_owned())
            .occlude()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .overflow_hidden()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .shadow_lg()
            .child(header)
            .child(
                div()
                    .id(("modal-body", ix))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_scroll()
                    .track_scroll(&open.scroll)
                    .child(div().p_3().min_w_0().child(body))
                    .scrollbar(&open.scroll, ScrollbarAxis::Both),
            )
    }
}

impl Focusable for ModalHost {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ModalHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.stack.is_empty() {
            return div().id("modal-host").into_any_element();
        }
        let cards: Vec<_> = self
            .stack
            .iter()
            .enumerate()
            .map(|(ix, open)| self.render_card(ix, open, window, cx).into_any_element())
            .collect();
        div()
            .id("modal-host")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key))
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("modal-backdrop")
                    .absolute()
                    .inset_0()
                    .bg(cx.theme().overlay)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.close_top(window, cx);
                        }),
                    ),
            )
            .children(cards)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
