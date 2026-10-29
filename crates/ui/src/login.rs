//! The `/login` dialog: a masked field for the API key of one account
//! (Anthropic or OpenCode); `Tab` or a click on its tab switches the account.
//!
//! The key goes straight from this field to the controller
//! ([`SessionController::login`](crate::session::SessionController::login)):
//! it never passes through the prompt, so it is not added to the prompt
//! history nor shown in the timeline. `Enter` submits, `Esc` or **Cancel**
//! closes, also while the controller checks the key (the check is then
//! dropped); meanwhile the field is read-only, and an error (a refused key,
//! no answer in time) keeps the dialog open with the reason.

use domain::model::Account;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeystrokeEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

/// What the dialog asks the session to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    /// Check and store this key for this account.
    Submit {
        /// Whose key it is.
        account: Account,
        /// The key.
        key: String,
    },
    /// The dialog was closed without a key.
    Cancelled,
}

/// The dialog; drawn only while open.
pub struct LoginView {
    input: Entity<InputState>,
    account: Account,
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
            account: Account::Anthropic,
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

    /// The account the key is for.
    pub fn account(&self) -> Account {
        self.account
    }

    /// Whether a key is being checked.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// The error shown under the field, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Opens the dialog for `account` with an empty field and focuses it.
    pub fn show(&mut self, account: Account, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.set_account(account, window, cx);
        self.busy = false;
        self.error = None;
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.focus(window, cx);
        cx.notify();
    }

    /// Asks for the key of `account` instead.
    pub fn set_account(&mut self, account: Account, window: &mut Window, cx: &mut Context<Self>) {
        self.account = account;
        self.error = None;
        let placeholder = match account {
            Account::Anthropic => "sk-ant-…",
            Account::OpenCode => "OpenCode API key",
        };
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx)
        });
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

    /// Closes without a key, dropping a check in flight.
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.close(window, cx);
        cx.emit(LoginEvent::Cancelled);
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
            self.error = Some(application::usecases::login::key_prompt(self.account).into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.emit(LoginEvent::Submit {
            account: self.account,
            key,
        });
        cx.notify();
    }

    /// Puts `key` in the field, as if it were pasted.
    pub fn set_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(key, window, cx));
    }

    /// A click on the tab of `account`: ask for its key, keeping the field
    /// focused so the key can be pasted right away.
    fn switch_to(&mut self, account: Account, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open || self.busy {
            return;
        }
        self.set_account(account, window, cx);
        self.focus(window, cx);
    }

    fn on_keystroke(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Only the dialog with the focus takes keys: another session's
        // dialog may be open out of sight (parked).
        let focused = self.focus_handle.contains_focused(window, cx)
            || self.input.read(cx).focus_handle(cx).is_focused(window);
        if !self.open || !focused {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.cancel(window, cx);
                cx.stop_propagation();
            }
            "tab" if !self.busy => {
                let next = match self.account {
                    Account::Anthropic => Account::OpenCode,
                    Account::OpenCode => Account::Anthropic,
                };
                self.set_account(next, window, cx);
                cx.stop_propagation();
            }
            _ => {}
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
        let name = self.account.display_name();
        let status = match (&self.error, self.busy) {
            (_, true) => Some((
                SharedString::from(format!("Checking the key with {name}… (Esc cancels)")),
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
            .child(h_flex().gap_3().children(Account::ALL.map(|account| {
                let active = account == self.account;
                div()
                    .id(account.as_str())
                    .debug_selector(move || format!("login-tab-{account}"))
                    .when(!self.busy, |tab| tab.cursor_pointer())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.switch_to(account, window, cx);
                    }))
                    .font_weight(if active {
                        gpui::FontWeight::BOLD
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(if active {
                        theme.popover_foreground
                    } else {
                        theme.muted_foreground
                    })
                    .when(active, |tab| tab.border_b_1().border_color(theme.primary))
                    .child(format!("{} API key", account.display_name()))
            })))
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(match self.account {
                        Account::Anthropic => {
                            "Paste your key (console.anthropic.com → API keys). It is checked with \
                     Anthropic and stored in the system keyring, never in a file or the history."
                        }
                        Account::OpenCode => {
                            "Paste your key (opencode.ai → console, Go or Zen). Use its models as \
                     opencode-go:<model> or opencode-zen:<model>. It is stored in the system \
                     keyring, never in a file or the history."
                        }
                    }),
            )
            .child(Input::new(&self.input).disabled(self.busy));
        if let Some((text, color)) = status {
            card = card.child(div().text_color(color).child(text));
        }
        card = card.child(
            h_flex()
                .gap_3()
                .justify_between()
                .child(div().text_color(theme.muted_foreground).child(
                    "Enter saves · Tab or click switches account · Esc closes · /logout deletes it",
                ))
                .child(
                    div().debug_selector(|| "login-cancel".to_owned()).child(
                        Button::new("login-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                    ),
                ),
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
