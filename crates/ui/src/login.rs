//! The `/login` dialog: a masked field for an OpenCode API key.
//!
//! The key goes straight from this field to the controller
//! ([`SessionController::login`](crate::session::SessionController::login)):
//! it never passes through the prompt, so it is not added to the prompt
//! history nor shown in the timeline. `Enter` submits, `Esc`, **Cancel** or
//! the close mark closes, also while the controller checks the key (the
//! check is then dropped); meanwhile the field is read-only, and an error
//! (a refused key, no answer in time) keeps the dialog open with the reason.

use domain::model::Account;
use gpui::{
    App, AppContext as _, BoxShadow, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, KeystrokeEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use crate::theme::token;
use crate::tour::{card_glow, sweep};

/// Which key this screen is asking for. OpenRouter is only a choice here:
/// its key is created on another site, and Crest does not store it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderPick {
    OpenCode,
    OpenRouter,
}

impl ProviderPick {
    const fn id(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::OpenRouter => "openrouter",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::OpenCode => "OpenCode API",
            Self::OpenRouter => "OpenRouter API",
        }
    }

    const fn blurb(self) -> &'static str {
        match self {
            Self::OpenCode => "Compatible and flexible",
            Self::OpenRouter => "Many models",
        }
    }

    const fn badge(self) -> &'static str {
        match self {
            Self::OpenCode => ">_",
            Self::OpenRouter => "OR",
        }
    }

    const fn placeholder(self) -> &'static str {
        match self {
            Self::OpenCode => "OpenCode API key",
            Self::OpenRouter => "OpenRouter API key",
        }
    }

    const fn docs_label(self) -> &'static str {
        match self {
            Self::OpenCode => "Get an OpenCode API key",
            Self::OpenRouter => "Get an OpenRouter API key",
        }
    }

    const fn docs_url(self) -> &'static str {
        match self {
            Self::OpenCode => "https://opencode.ai/console",
            Self::OpenRouter => "https://openrouter.ai/keys",
        }
    }

    const fn stored_note(self) -> &'static str {
        match self {
            Self::OpenCode => "Your key is stored in the system keyring.",
            Self::OpenRouter => "OpenRouter uses its own key, separate from OpenCode.",
        }
    }
}

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
    pick: ProviderPick,
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
            pick: ProviderPick::OpenCode,
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

    /// Whether text is selected in the key field. The key is masked, so it
    /// is never copied, but `Ctrl+C` there must not interrupt the turn.
    pub fn has_selection(&self, cx: &App) -> bool {
        self.open && !self.input.read(cx).selected_range().is_empty()
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

    /// Opens the dialog on OpenCode with an empty field and focuses it.
    /// The account argument is ignored: connecting still stores an OpenCode key.
    pub fn show(&mut self, _account: Account, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.pick = ProviderPick::OpenCode;
        self.set_account(Account::OpenCode, window, cx);
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
            Account::OpenCode => self.pick.placeholder(),
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
        if self.pick == ProviderPick::OpenRouter {
            self.error = Some(
                "OpenRouter uses its own key. This screen can open its page; Crest does not store that key yet."
                    .into(),
            );
            cx.notify();
            return;
        }
        if key.is_empty() {
            self.error = Some(application_core::usecases::login::key_prompt(self.account).into());
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
        self.pick = ProviderPick::OpenCode;
        self.set_account(account, window, cx);
        self.focus(window, cx);
    }

    /// Selects OpenRouter. Its key page is different; the typed key is not
    /// sent to OpenCode.
    fn pick_openrouter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open || self.busy {
            return;
        }
        self.pick = ProviderPick::OpenRouter;
        self.error = None;
        self.input.update(cx, |input, cx| {
            input.set_placeholder(ProviderPick::OpenRouter.placeholder(), window, cx);
        });
        self.focus(window, cx);
        cx.notify();
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
        if event.keystroke.key.as_str() == "escape" {
            self.cancel(window, cx);
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
        let accent = token(cx, "accent.bg");
        let popover = token(cx, "popover.bg");
        let (foreground, muted_fg, danger, overlay) = {
            let theme = cx.theme();
            (
                theme.foreground,
                theme.muted_foreground,
                theme.danger,
                theme.overlay,
            )
        };
        let name = self.account.display_name();
        let key_empty = self.input.read(cx).value().trim().is_empty();
        let status = match (&self.error, self.busy) {
            (_, true) => Some((
                SharedString::from(format!("Checking the key with {name}… (Esc cancels)")),
                muted_fg,
            )),
            (Some(error), false) => Some((SharedString::from(error.clone()), danger)),
            (None, false) => None,
        };
        let connect = !key_empty && !self.busy;
        let button_bg = token(cx, "button.bg");
        let button_hover = token(cx, "button.hover.bg");
        let hairline = token(cx, "border");
        let header = self.header(muted_fg);
        let opencode = self.provider_card(Account::OpenCode, accent, cx);
        let openrouter = self.openrouter_card(accent, cx);
        let pick = self.pick;
        let form = v_flex()
            .w_full()
            .gap_3()
            .p(px(28.))
            .text_color(foreground)
            .child(header)
            .child(div().h(px(1.)).w_full().bg(accent.opacity(0.35)))
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_3()
                    .child(opencode)
                    .child(openrouter),
            )
            .child(div().text_size(px(14.)).child("API key"))
            .child(
                Input::new(&self.input)
                    .large()
                    .rounded(px(10.))
                    .mask_toggle()
                    .disabled(self.busy),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted_fg)
                    .child(pick.stored_note()),
            )
            .child(
                h_flex().justify_start().child(
                    Button::new("login-key-docs")
                        .link()
                        .label(pick.docs_label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.open_url(this.pick.docs_url());
                        })),
                ),
            )
            .children(
                status.map(|(text, color)| div().text_size(px(13.)).text_color(color).child(text)),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("login-cancel")
                            .debug_selector(|| "login-cancel".to_owned())
                            .px_4()
                            .py(px(7.))
                            .min_w(px(104.))
                            .flex()
                            .justify_center()
                            .rounded(px(10.))
                            .text_sm()
                            .cursor_pointer()
                            .bg(button_bg)
                            .text_color(foreground)
                            .border_1()
                            .border_color(hairline.opacity(0.7))
                            .hover(move |style| style.bg(button_hover))
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
                            .child("Cancel"),
                    )
                    .child(self.connect_button(connect, cx)),
            );
        let card = div()
            .id("login-card")
            .relative()
            .w_full()
            .max_w(px(560.))
            .max_h_full()
            .rounded(px(18.))
            .shadow(card_glow(accent))
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_h_full()
                    .overflow_hidden()
                    .rounded(px(18.))
                    .border_1()
                    .border_color(accent.opacity(0.85))
                    .bg(popover)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(px(440.))
                            .h(px(360.))
                            .child(sweep(accent)),
                    )
                    .child(
                        div()
                            .id("login-body")
                            .w_full()
                            .max_h_full()
                            .overflow_y_scroll()
                            .child(form),
                    ),
            )
            .child(
                div()
                    .id("login-close")
                    .absolute()
                    .top(px(10.))
                    .right(px(12.))
                    .px_1()
                    .text_sm()
                    .text_color(muted_fg)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
                    .child("×"),
            );
        div()
            .id("login")
            .track_focus(&self.focus_handle)
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(20.))
            .bg(overlay)
            .child(card)
            .into_any_element()
    }
}

impl LoginView {
    /// The centered mark, title and subtitle. The close mark sits on the card,
    /// so this block stays centered under the corner waves.
    fn header(&self, muted_fg: Hsla) -> gpui::AnyElement {
        v_flex()
            .w_full()
            .items_center()
            .gap(px(10.))
            .pt(px(8.))
            .child(crate::brand::mark(40.0))
            .child(
                div()
                    .w_full()
                    .text_center()
                    .text_size(px(26.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Welcome to Crest"),
            )
            .child(
                div()
                    .w_full()
                    .text_center()
                    .text_size(px(14.))
                    .text_color(muted_fg)
                    .child("Your AI-assisted development workspace"),
            )
            .into_any_element()
    }

    /// The connect control. Empty and in-flight keys do not submit; the coral
    /// is the theme accent, and an empty field uses the darker accent.
    fn connect_button(&self, enabled: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let accent = token(cx, "accent.bg");
        let accent_fg = token(cx, "accent.fg");
        let accent_hover = token(cx, "accent.hover.bg");
        let accent_off = token(cx, "accent.active.bg");
        let muted_fg = cx.theme().muted_foreground;
        let mut button = div()
            .id("login-connect")
            .flex_1()
            .px_4()
            .py(px(7.))
            .min_w(px(104.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(10.))
            .text_sm();
        button = if enabled {
            button
                .cursor_pointer()
                .bg(accent)
                .text_color(accent_fg)
                .shadow(vec![
                    BoxShadow::new(px(0.), px(0.), accent.opacity(0.5)).blur_radius(px(12.)),
                ])
                .hover(move |style| style.bg(accent_hover))
                .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
        } else {
            button
                .bg(accent_off.opacity(0.45))
                .text_color(muted_fg)
                .border_1()
                .border_color(token(cx, "border").opacity(0.7))
        };
        button.child("Connect and continue →").into_any_element()
    }

    /// One provider. The debug selector stays `login-tab-<account>` so a click
    /// still switches the account the session tests.
    fn provider_card(
        &self,
        account: Account,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.choice_card(
            ProviderPick::OpenCode,
            accent,
            cx,
            move |this, window, cx| {
                this.switch_to(account, window, cx);
            },
        )
    }

    /// OpenRouter. Its key is created on another site and is not stored yet.
    fn openrouter_card(&self, accent: Hsla, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.choice_card(ProviderPick::OpenRouter, accent, cx, |this, window, cx| {
            this.pick_openrouter(window, cx);
        })
    }

    fn choice_card(
        &self,
        pick: ProviderPick,
        accent: Hsla,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let selected = self.pick == pick;
        let (border, fill, muted_fg) = {
            let theme = cx.theme();
            (
                if selected { accent } else { theme.border },
                if selected {
                    accent.opacity(0.12)
                } else {
                    theme.muted
                },
                theme.muted_foreground,
            )
        };
        let busy = self.busy;
        let selector = format!("login-tab-{}", pick.id());
        h_flex()
            .id(pick.id())
            .debug_selector(move || selector)
            .min_w(px(220.))
            .flex_1()
            .items_center()
            .gap_3()
            .p(px(15.))
            .rounded(px(12.))
            .bg(fill)
            .border_1()
            .border_color(border)
            .when(!busy, |card| {
                card.cursor_pointer()
                    .hover(move |style| style.border_color(accent.opacity(0.85)))
            })
            .on_click(cx.listener(move |this, _, window, cx| on_pick(this, window, cx)))
            .child(choice_badge(pick.badge(), accent))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .child(pick.title()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(muted_fg)
                            .whitespace_nowrap()
                            .child(pick.blurb()),
                    ),
            )
            .child(div().w(px(16.)).when(selected, |mark| {
                mark.text_color(accent)
                    .child(Icon::new(IconName::Check).small())
            }))
            .into_any_element()
    }
}

/// A small mark for the provider: a prompt for OpenCode, OR for OpenRouter.
fn choice_badge(label: &'static str, accent: Hsla) -> gpui::AnyElement {
    div()
        .w(px(28.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(accent.opacity(0.16))
        .text_color(accent)
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(label)
        .into_any_element()
}
