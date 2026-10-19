//! The `/login` dialog: a masked field for the Anthropic API key.
//!
//! The key goes straight from this field to the controller
//! ([`SessionController::login`](crate::session::SessionController::login)):
//! it never passes through the prompt, so it is not added to the prompt
//! history nor shown in the timeline. `Enter` submits, `Esc` closes; while
//! the controller checks the key the field is read-only, and an error keeps
//! the dialog open with the reason.

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeystrokeEvent, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, v_flex};

/// What the dialog asks the session to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    /// Check and store this key.
    Submit(String),
    /// The dialog was closed without a key.
    Cancelled,
}

/// The dialog; drawn only while open.
pub struct LoginView {
    input: Entity<InputState>,
    open: bool,
    busy: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LoginEvent> for LoginView {}

impl LoginView {
    /// A closed dialog.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("sk-ant-…")
        });
        let enter = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(cx);
            }
        });
        // Before the field and the session see it: the field would keep
        // `Esc` and the session would send it to the prompt.
        let weak = cx.entity().downgrade();
        let escape = cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
            weak.update(cx, |this, cx| this.on_keystroke(event, window, cx))
                .ok();
        });
        Self {
            input,
            open: false,
            busy: false,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: vec![enter, escape],
        }
    }

    /// Whether the dialog shows.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether a key is being checked.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// The error shown under the field, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Opens the dialog with an empty field and focuses it.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.busy = false;
        self.error = None;
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.focus(window, cx);
        cx.notify();
    }

    /// Focuses the field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Closes the dialog and forgets the typed key.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.busy = false;
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// The controller refused the key: show why and let the user retry.
    pub fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        self.busy = false;
        self.error = Some(error);
        cx.notify();
    }

    /// Submits what the field holds (tests type into it directly).
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.open || self.busy {
            return;
        }
        let key = self.input.read(cx).value().trim().to_owned();
        if key.is_empty() {
            self.error = Some("paste your API key (it starts with sk-ant-)".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.emit(LoginEvent::Submit(key));
        cx.notify();
    }

    /// Puts `key` in the field, as if it were pasted.
    pub fn set_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(key, window, cx));
    }

    fn on_keystroke(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open && !self.busy && event.keystroke.key == "escape" {
            self.close(window, cx);
            cx.emit(LoginEvent::Cancelled);
            cx.stop_propagation();
        }
    }
}

impl Focusable for LoginView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().id("login").into_any_element();
        }
        let theme = cx.theme();
        let status = match (&self.error, self.busy) {
            (_, true) => Some((
                SharedString::from("Checking the key with Anthropic…"),
                theme.muted_foreground,
            )),
            (Some(error), false) => Some((SharedString::from(error.clone()), theme.danger)),
            (None, false) => None,
        };
        let mut card = v_flex()
            .w(px(560.))
            .max_w_full()
            .gap_3()
            .p_4()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Anthropic API key"),
            )
            .child(div().text_color(theme.muted_foreground).child(
                "Paste your key (console.anthropic.com → API keys). It is checked with \
                 Anthropic and stored in the system keyring, never in a file or the history.",
            ))
            .child(Input::new(&self.input).disabled(self.busy));
        if let Some((text, color)) = status {
            card = card.child(div().text_color(color).child(text));
        }
        card = card.child(
            div()
                .text_color(theme.muted_foreground)
                .child("Enter saves · Esc closes · /logout deletes it"),
        );
        div()
            .id("login")
            .track_focus(&self.focus_handle)
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.overlay)
            .child(card)
            .into_any_element()
    }
}
