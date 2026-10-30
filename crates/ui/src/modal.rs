//! Plugin modals: [`ModalHost`] draws the modals that plugins open with
//! `app.modal.open` over the rest of the window.
//!
//! The host is an overlay of the root view, always mounted and empty while
//! no modal is open (the same pattern as the approval dialog). It keeps a
//! stack by id: opening an id that is already open replaces that modal and
//! brings it to the front; updating replaces its content; closing removes
//! it. Each modal is a card placed by the rules of
//! [`domain::modal::modal_bounds`] within the area the host covers (the
//! session, beside the chats sidebar and the tree; it follows resizes),
//! with a header (title, plugin,
//! close button) and a body that scrolls both ways when the content is
//! larger. A dimmed backdrop covers the window behind the cards.
//!
//! `Esc` (or a click on the backdrop) closes the front modal; `Up`/`Down`,
//! `PageUp`/`PageDown` and `Home`/`End` scroll its body. The host
//! takes the focus when the first modal opens and gives it back when the
//! last one closes.
//!
//! Events come through [`ModalHost::post`] (the session forwards the
//! modal events of the controller's channel) and are applied in order on
//! the window's executor.

use domain::block::{BlockId, CustomNode};
use domain::modal::{HorizontalAnchor, MODAL_MARGIN, ModalId, ModalSpec, VerticalAnchor};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div,
    px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_component::text::{TextView, TextViewState};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use uuid::Uuid;

use application::ports::events::DomainEvent;
use domain::modal::{ModalPosition, Percent};
use domain::sysmodel::Board;

use crate::sysboard::SysBoardView;

/// A modal on screen.
struct Open {
    spec: ModalSpec,
    /// Parent id of its stateful nodes (tables, images...).
    node_id: BlockId,
    scroll: ScrollHandle,
    /// A native view shown instead of `spec.content` (the simulation
    /// board), filling the card.
    view: Option<AnyView>,
    /// The parsed document when the content is one markdown or code node
    /// (a file opened with `/open`). Drawn as a virtualized `TextView`
    /// that lays out only the visible part: the body of a 20,000-line
    /// file is not laid out again on every tick of the wheel. Owned here,
    /// not in `block_state`, so it goes with the modal.
    doc: Option<Entity<TextViewState>>,
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

/// Id of the board modal of Spec `spec`.
pub fn board_modal_id(spec: &str) -> ModalId {
    ModalId::new(format!("crest/simulate/{spec}"))
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
            DomainEvent::SimulationReady { board } => self.open_board(*board, window, cx),
            _ => {}
        }
    }

    fn open(&mut self, spec: ModalSpec, window: &mut Window, cx: &mut Context<Self>) {
        self.push(spec, None, window, cx);
    }

    fn push(
        &mut self,
        spec: ModalSpec,
        view: Option<AnyView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (scroll, previous_doc) =
            match self.stack.iter().position(|open| open.spec.id == spec.id) {
                Some(ix) => {
                    let open = self.stack.remove(ix);
                    (open.scroll, open.doc)
                }
                None => (ScrollHandle::new(), None),
            };
        let doc = match (
            crate::blocks::custom::document_markdown(&spec.content),
            previous_doc,
        ) {
            (Some(markdown), Some(doc)) => {
                // An update keeps the scroll position of the document.
                doc.update(cx, |doc, cx| doc.set_text(&markdown, cx));
                Some(doc)
            }
            (Some(markdown), None) => Some(cx.new(|cx| TextViewState::markdown(&markdown, cx))),
            (None, _) => None,
        };
        if self.stack.is_empty() {
            self.restore_focus = window.focused(cx);
        }
        self.stack.push(Open {
            node_id: modal_node_id(&spec.id),
            spec,
            scroll,
            view,
            doc,
        });
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Opens `view` in a modal described by `spec` (its content is
    /// ignored); the view fills the card.
    pub fn open_view(
        &mut self,
        spec: ModalSpec,
        view: AnyView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.push(spec, Some(view), window, cx);
    }

    /// Opens the simulation board of `board` at full size, replacing the
    /// board of the same Spec.
    pub fn open_board(&mut self, board: Board, window: &mut Window, cx: &mut Context<Self>) {
        let spec = ModalSpec {
            id: board_modal_id(&board.spec),
            plugin: "crest".into(),
            title: format!("Simulation · {} · {}", board.spec, board.title),
            width: Percent::FULL,
            height: Percent::FULL,
            position: ModalPosition::CENTER,
            content: CustomNode::Text {
                text: String::new(),
            },
        };
        let view = cx.new(|cx| SysBoardView::new(board, window, cx));
        let focus = view.focus_handle(cx);
        self.open_view(spec, view.into(), window, cx);
        // Keys reach the board first; Esc still bubbles up to the host.
        window.focus(&focus, cx);
    }

    /// The native view of modal `id`, if it shows one.
    pub fn view(&self, id: &ModalId) -> Option<&AnyView> {
        self.stack
            .iter()
            .find(|open| &open.spec.id == id)
            .and_then(|open| open.view.as_ref())
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

    /// The document of modal `id`, when its content is one markdown or
    /// code node drawn virtualized (see [`Open::doc`]).
    pub fn document(&self, id: &ModalId) -> Option<&Entity<TextViewState>> {
        self.stack
            .iter()
            .find(|open| &open.spec.id == id)
            .and_then(|open| open.doc.as_ref())
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
        if m.control || m.alt || m.platform || m.shift {
            return;
        }
        if keys.key == "escape" && self.close_top(window, cx) {
            cx.stop_propagation();
            return;
        }
        // The body of a plugin modal scrolls with the keyboard too; a
        // view (the help) scrolls itself.
        let Some(top) = self.stack.last().filter(|open| open.view.is_none()) else {
            return;
        };
        let scrolled = match &top.doc {
            Some(doc) => crate::keyscroll::scroll_list_key(doc.read(cx).list_state(), &keys.key),
            None => crate::keyscroll::scroll_key(&top.scroll, &keys.key),
        };
        if scrolled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn render_card(
        &self,
        ix: usize,
        open: &Open,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = open.view.clone();
        let body = match (&view, &open.doc) {
            (Some(_), _) => None,
            (None, Some(doc)) => {
                // The document scrolls itself, virtualized.
                let text = crate::blocks::math::with_math(TextView::new(doc))
                    .scrollable(true)
                    .on_link_click(crate::blocks::custom::open_link);
                Some(div().size_full().p_3().child(text).into_any_element())
            }
            (None, None) => Some(crate::blocks::custom::render_node(
                open.node_id,
                &open.spec.content,
                window,
                cx,
            )),
        };
        let virtualized = open.doc.is_some();
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
        let position = open.spec.position;
        let card = v_flex()
            .id(("modal", ix))
            .debug_selector(|| "modal-card".to_owned())
            .occlude()
            .flex_none()
            .w(relative(open.spec.width.fraction()))
            .h(relative(open.spec.height.fraction()))
            .overflow_hidden()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .shadow_lg()
            .child(header)
            .children(view.map(|view| {
                div()
                    .id(("modal-view", ix))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(view)
            }))
            .children(body.map(|body| {
                let body_el = div().id(("modal-body", ix)).flex_1().min_h_0().w_full();
                if virtualized {
                    body_el.child(body)
                } else {
                    body_el
                        .overflow_scroll()
                        .track_scroll(&open.scroll)
                        .child(div().p_3().min_w_0().child(body))
                        .scrollbar(&open.scroll, ScrollbarAxis::Both)
                }
            }));
        // The card is sized against this layer, not the window: the host
        // fills the session area, which the chats sidebar and the tree
        // panel make narrower than the window. The padding is the margin
        // and the percentages apply to what is left, as in `modal_bounds`.
        let layer = div()
            .absolute()
            .inset_0()
            .p(px(MODAL_MARGIN))
            .flex()
            .flex_row();
        let layer = match position.horizontal {
            HorizontalAnchor::Left => layer.justify_start(),
            HorizontalAnchor::Center => layer.justify_center(),
            HorizontalAnchor::Right => layer.justify_end(),
        };
        let layer = match position.vertical {
            VerticalAnchor::Top => layer.items_start(),
            VerticalAnchor::Center => layer.items_center(),
            VerticalAnchor::Bottom => layer.items_end(),
        };
        layer.child(card)
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
