//! Renderer of the `form` block kind: the questions of `ask_user`.
//!
//! The block is drawn by a [`FormView`] kept per block
//! ([`super::block_state`]), so what the user typed survives scrolling. It
//! shows one input per field: a text field (one line or a text area), a
//! text field for numbers checked as they are submitted, a date picker
//! (its calendar greys out the days outside the bounds), a checkbox for a
//! boolean, radio buttons for one choice and checkboxes for several. Labels
//! mark the required fields with `*` and descriptions sit under them.
//!
//! The form only takes answers while the `ask_user` call waits on it: the
//! request arrives through [`crate::form::UiForm`] and [`attach`] hands it
//! to the view, which takes the focus. Submit checks every field and shows
//! the problems inline; a valid answer goes back to the tool, which marks
//! the block submitted. A prompt sent while a form waits answers it in
//! prose instead ([`reply_to_waiting`]). A submitted form shows its
//! answers read-only, a replied one the words, a cancelled one says so.
//!
//! A click on the card while it waits keeps the focus in the form (the
//! timeline row behind it would take it otherwise, and the next key would
//! land in the prompt).
//!
//! Keys while a field has the focus: `Tab` / `Shift+Tab` move between the
//! inputs, `Enter` in a one-line field submits, `Esc` cancels; `Space`
//! toggles a checkbox or picks a radio button. On a date picker `Enter`
//! opens or closes its calendar instead (the picker's own binding runs
//! first) and `Esc` closes an open calendar before it cancels the form.

use std::collections::BTreeMap;

use application::ports::form::{FormRequest, FormResponse};
use chrono::NaiveDate;
use domain::block::{BlockId, BlockKind};
use domain::form::{FieldKind, FormField, FormOption, FormSpec, FormState};
use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _,
    FontWeight, Global, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::calendar::Matcher;
use gpui_component::checkbox::Checkbox;
use gpui_component::date_picker::{DatePicker, DatePickerEvent, DatePickerState};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::radio::Radio;
use gpui_component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use serde_json::Value;

use crate::form::PendingForm;
use crate::timeline::TimelineEntry;

/// The input of one field.
enum FieldInput {
    /// One line of text, or a number typed as text.
    Line(Entity<InputState>),
    /// Several lines of text.
    Area(Entity<TextareaState>),
    /// A calendar date.
    Date(Entity<DatePickerState>),
    /// A checkbox.
    Toggle(bool),
    /// The index of the chosen option.
    Choice(Option<usize>),
    /// Which options are chosen.
    Choices(Vec<bool>),
}

/// The request being answered.
struct Waiting {
    pending: PendingForm,
    _cancel: Task<()>,
}

/// How a date reads and is written: `YYYY-MM-DD`.
const ISO_DATE: &str = "%Y-%m-%d";

/// The day `text` names, when it is an ISO date.
fn iso_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, ISO_DATE).ok()
}

/// The forms drawn so far, for [`owns_focus`].
#[derive(Default)]
struct Forms(Vec<WeakEntity<FormView>>);

impl Global for Forms {}

/// Whether the focus is in a form that waits for an answer. The session
/// then leaves the keys to the form instead of typing them in the prompt.
pub fn owns_focus(window: &Window, cx: &App) -> bool {
    cx.try_global::<Forms>().is_some_and(|forms| {
        forms.0.iter().filter_map(WeakEntity::upgrade).any(|view| {
            let view = view.read(cx);
            view.waiting.is_some() && view.focus_handle.contains_focused(window, cx)
        })
    })
}

/// The form waiting for an answer, if any.
fn waiting(cx: &App) -> Option<Entity<FormView>> {
    cx.try_global::<Forms>()?
        .0
        .iter()
        .filter_map(WeakEntity::upgrade)
        .find(|view| view.read(cx).waiting.is_some())
}

/// Whether a form waits for an answer.
pub fn is_waiting(cx: &App) -> bool {
    waiting(cx).is_some()
}

/// Answers the waiting form with `text`, typed in the chat instead of its
/// fields. Returns whether a form was waiting.
pub fn reply_to_waiting(text: &str, window: &mut Window, cx: &mut App) -> bool {
    let Some(form) = waiting(cx) else {
        return false;
    };
    form.update(cx, |form, cx| form.reply(text.to_owned(), window, cx));
    true
}

/// The form of one block.
pub struct FormView {
    id: BlockId,
    spec: FormSpec,
    inputs: Vec<FieldInput>,
    waiting: Option<Waiting>,
    /// The answer was sent and the tool has not marked the block yet.
    sent: bool,
    /// Problems of the last submit, by field name.
    errors: BTreeMap<String, String>,
    /// Why the tool asked again.
    error: Option<String>,
    focus_handle: FocusHandle,
    restore_focus: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

fn label_of(field: &FormField) -> &str {
    field.label.as_deref().unwrap_or(&field.name)
}

fn option_label(option: &FormOption) -> &str {
    option.label.as_deref().unwrap_or(&option.value)
}

/// The answer as text, for the read-only summary.
fn answer_text(field: &FormField, value: Option<&Value>) -> String {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return "—".to_owned();
    };
    let label = |options: &[FormOption], value: &str| {
        options
            .iter()
            .find(|o| o.value == value)
            .map_or(value, option_label)
            .to_owned()
    };
    match (&field.kind, value) {
        (_, Value::String(s)) => match &field.kind {
            FieldKind::Enum { options, .. } => label(options, s),
            _ => s.clone(),
        },
        (_, Value::Bool(b)) => if *b { "Yes" } else { "No" }.to_owned(),
        (FieldKind::MultiEnum { options, .. }, Value::Array(items)) => {
            let picked: Vec<String> = items
                .iter()
                .filter_map(Value::as_str)
                .map(|v| label(options, v))
                .collect();
            if picked.is_empty() {
                "—".to_owned()
            } else {
                picked.join(", ")
            }
        }
        (_, other) => other.to_string(),
    }
}

impl FormView {
    fn new(id: BlockId, spec: FormSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let inputs = spec
            .fields
            .iter()
            .map(|field| {
                let input = Self::input(field, window, cx);
                let name = field.name.clone();
                match &input {
                    FieldInput::Line(state) => {
                        subscriptions.push(cx.subscribe_in(
                            state,
                            window,
                            move |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                                InputEvent::PressEnter { .. } => this.submit(window, cx),
                                InputEvent::Change => this.clear_error(&name, cx),
                                _ => {}
                            },
                        ));
                    }
                    FieldInput::Area(state) => {
                        subscriptions.push(cx.subscribe(
                            state,
                            move |this: &mut Self, _, event: &InputEvent, cx| {
                                if let InputEvent::Change = event {
                                    this.clear_error(&name, cx);
                                }
                            },
                        ));
                    }
                    FieldInput::Date(state) => {
                        // The only event is a change (a pick or a clear).
                        subscriptions.push(cx.subscribe(
                            state,
                            move |this: &mut Self, _, _: &DatePickerEvent, cx| {
                                this.clear_error(&name, cx);
                            },
                        ));
                    }
                    _ => {}
                }
                input
            })
            .collect();
        let this = cx.entity().downgrade();
        let forms = &mut cx.default_global::<Forms>().0;
        forms.retain(|form| form.upgrade().is_some());
        forms.push(this);
        Self {
            id,
            spec,
            inputs,
            waiting: None,
            sent: false,
            errors: BTreeMap::new(),
            error: None,
            focus_handle: cx.focus_handle(),
            restore_focus: None,
            _subscriptions: subscriptions,
        }
    }

    fn input(field: &FormField, window: &mut Window, cx: &mut Context<Self>) -> FieldInput {
        let line = |text: String, window: &mut Window, cx: &mut Context<Self>| {
            FieldInput::Line(cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                if !text.is_empty() {
                    state = state.default_value(text);
                }
                state
            }))
        };
        match &field.kind {
            FieldKind::String { default, .. } => {
                line(default.clone().unwrap_or_default(), window, cx)
            }
            FieldKind::Number { default, .. } => line(
                default.map(|v| v.to_string()).unwrap_or_default(),
                window,
                cx,
            ),
            FieldKind::Integer { default, .. } => line(
                default.map(|v| v.to_string()).unwrap_or_default(),
                window,
                cx,
            ),
            FieldKind::Date { default, min, max } => {
                let (min, max) = (
                    min.as_deref().and_then(iso_date),
                    max.as_deref().and_then(iso_date),
                );
                let default = default.as_deref().and_then(iso_date);
                FieldInput::Date(cx.new(|cx| {
                    let mut state = DatePickerState::new(window, cx).date_format(ISO_DATE);
                    if min.is_some() || max.is_some() {
                        // `interval` greys out the days before `min` and
                        // after `max` (`range` would grey out the inside).
                        state = state.disabled_matcher(Matcher::interval(min, max));
                    }
                    if let Some(day) = default {
                        state.set_date(day, window, cx);
                    }
                    state
                }))
            }
            FieldKind::Multiline { default, .. } => {
                let text = default.clone().unwrap_or_default();
                FieldInput::Area(cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx).auto_grow(3, 10);
                    if !text.is_empty() {
                        state = state.default_value(text);
                    }
                    state
                }))
            }
            FieldKind::Boolean { default } => FieldInput::Toggle(default.unwrap_or(false)),
            FieldKind::Enum { options, default } => FieldInput::Choice(
                default
                    .as_ref()
                    .and_then(|d| options.iter().position(|o| &o.value == d)),
            ),
            FieldKind::MultiEnum {
                options, default, ..
            } => FieldInput::Choices(options.iter().map(|o| default.contains(&o.value)).collect()),
        }
    }

    /// Takes the latest version of the block.
    fn sync(&mut self, spec: &FormSpec, cx: &mut Context<Self>) {
        if &self.spec != spec {
            // The fields never change; only the state and the answer.
            self.spec.state = spec.state;
            self.spec.answer.clone_from(&spec.answer);
            if spec.state != FormState::Pending {
                self.sent = false;
                self.waiting = None;
            }
            cx.notify();
        }
    }

    /// Starts answering `pending` and takes the focus.
    fn attach(&mut self, mut pending: PendingForm, window: &mut Window, cx: &mut Context<Self>) {
        if pending.is_cancelled() {
            return;
        }
        self.error.clone_from(&pending.request().error);
        let cancellation = pending.cancellation();
        let cancel = cx.spawn_in(window, async move |this, cx| {
            cancellation.await;
            this.update(cx, |this, cx| {
                if this
                    .waiting
                    .as_ref()
                    .is_some_and(|w| w.pending.is_cancelled())
                {
                    this.waiting = None;
                    cx.notify();
                }
            })
            .ok();
        });
        self.waiting = Some(Waiting {
            pending,
            _cancel: cancel,
        });
        self.sent = false;
        if self.restore_focus.is_none() {
            self.restore_focus = window.focused(cx);
        }
        self.focus_first(window, cx);
        cx.notify();
    }

    /// Focuses the first input. The card is a tab group, so the next tab
    /// stop after it is its first input; a card not drawn yet has no tab
    /// stops, so that waits for the next frame.
    fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        window.focus_next(cx);
        if !self.focus_handle.contains_focused(window, cx) {
            window.focus(&self.focus_handle, cx);
            cx.on_next_frame(window, |this, window, cx| {
                if this.focus_handle.is_focused(window) {
                    window.focus_next(cx);
                }
            });
        }
    }

    fn clear_error(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.errors.remove(name).is_some() {
            cx.notify();
        }
    }

    /// The request being answered, if any.
    pub fn request(&self) -> Option<&FormRequest> {
        self.waiting.as_ref().map(|w| w.pending.request())
    }

    /// The problems shown under the fields, by field name.
    pub fn errors(&self) -> &BTreeMap<String, String> {
        &self.errors
    }

    /// The value of every field the user filled, or the problems by field.
    fn read(&self, cx: &App) -> Result<BTreeMap<String, Value>, BTreeMap<String, String>> {
        let mut answer = BTreeMap::new();
        let mut errors = BTreeMap::new();
        for (field, input) in self.spec.fields.iter().zip(&self.inputs) {
            match read_field(field, input, cx) {
                Ok(Some(value)) => {
                    answer.insert(field.name.clone(), value);
                }
                Ok(None) if field.required => {
                    errors.insert(field.name.clone(), "Required".to_owned());
                }
                Ok(None) => {}
                Err(problem) => {
                    errors.insert(field.name.clone(), problem);
                }
            }
        }
        if errors.is_empty() {
            Ok(answer)
        } else {
            Err(errors)
        }
    }

    /// Checks the fields and sends the answer, or shows the problems.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting.is_none() {
            return;
        }
        match self.read(cx) {
            Ok(answer) => {
                self.errors.clear();
                self.error = None;
                self.respond(FormResponse::Submitted { answer }, window, cx);
                self.sent = true;
            }
            Err(errors) => {
                self.errors = errors;
                self.focus_first_error(window, cx);
            }
        }
        cx.notify();
    }

    /// Puts the cursor in the first text field with a problem.
    fn focus_first_error(&self, window: &mut Window, cx: &mut Context<Self>) {
        let first = self
            .spec
            .fields
            .iter()
            .zip(&self.inputs)
            .find(|(field, _)| self.errors.contains_key(&field.name));
        let handle = match first {
            Some((_, FieldInput::Line(state))) => state.focus_handle(cx),
            Some((_, FieldInput::Area(state))) => state.focus_handle(cx),
            Some((_, FieldInput::Date(state))) => state.focus_handle(cx),
            _ => return,
        };
        window.focus(&handle, cx);
    }

    /// Answers with `text`, written in the chat instead of the fields.
    pub fn reply(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting.is_some() {
            self.errors.clear();
            self.error = None;
            self.respond(FormResponse::Replied { text }, window, cx);
            self.sent = true;
            cx.notify();
        }
    }

    /// Dismisses the form without answering.
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting.is_some() {
            self.respond(FormResponse::Cancelled { reason: None }, window, cx);
            cx.notify();
        }
    }

    fn respond(&mut self, response: FormResponse, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(waiting) = self.waiting.take() {
            waiting.pending.respond(response);
        }
        if let Some(handle) = self.restore_focus.take() {
            window.focus(&handle, cx);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting.is_none() {
            return;
        }
        let m = &event.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return;
        }
        match event.keystroke.key.as_str() {
            "tab" => {
                if m.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
                // Past either end: back to the first input.
                if !self.focus_handle.contains_focused(window, cx) {
                    self.focus_first(window, cx);
                }
            }
            "enter" if !m.shift => self.submit(window, cx),
            "escape" => self.cancel(window, cx),
            // The timeline would move its selection.
            "up" | "down" | "home" | "end" | "pageup" | "pagedown" | "enter" => {}
            _ => return,
        }
        cx.stop_propagation();
    }

    fn element_id(&self, name: &str, index: usize) -> ElementId {
        ElementId::Name(SharedString::from(format!(
            "form-{name}-{index}-{}",
            self.id
        )))
    }

    fn render_input(
        &self,
        index: usize,
        field: &FormField,
        input: &FieldInput,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled = self.waiting.is_none();
        match (input, &field.kind) {
            (FieldInput::Line(state), _) => Input::new(state).disabled(disabled).into_any_element(),
            (FieldInput::Area(state), _) => {
                Textarea::new(state).disabled(disabled).into_any_element()
            }
            (FieldInput::Date(state), _) => DatePicker::new(state)
                .disabled(disabled)
                .cleanable(true)
                .placeholder("YYYY-MM-DD")
                .into_any_element(),
            (FieldInput::Toggle(checked), _) => div()
                .debug_selector(move || format!("form-option-{index}-0"))
                .child(
                    Checkbox::new(self.element_id("toggle", index))
                        .checked(*checked)
                        .disabled(disabled)
                        .label(SharedString::from(label_of(field).to_owned()))
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if let Some(FieldInput::Toggle(value)) = this.inputs.get_mut(index) {
                                *value = *checked;
                            }
                            this.clear_error_at(index, cx);
                        })),
                )
                .into_any_element(),
            (FieldInput::Choice(selected), FieldKind::Enum { options, .. }) => v_flex()
                .gap_1()
                .children(options.iter().enumerate().map(|(i, option)| {
                    let selector = format!("form-option-{index}-{i}");
                    let radio = Radio::new(self.element_id(&format!("choice{index}"), i))
                        .label(SharedString::from(option_label(option).to_owned()))
                        .checked(*selected == Some(i))
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _: &bool, _, cx| {
                            if let Some(FieldInput::Choice(value)) = this.inputs.get_mut(index) {
                                *value = Some(i);
                            }
                            this.clear_error_at(index, cx);
                        }));
                    div().debug_selector(move || selector.clone()).child(radio)
                }))
                .into_any_element(),
            (FieldInput::Choices(picked), FieldKind::MultiEnum { options, .. }) => v_flex()
                .gap_1()
                .children(options.iter().enumerate().map(|(i, option)| {
                    let selector = format!("form-option-{index}-{i}");
                    let checkbox = Checkbox::new(self.element_id(&format!("choices{index}"), i))
                        .label(SharedString::from(option_label(option).to_owned()))
                        .checked(picked.get(i).copied().unwrap_or(false))
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if let Some(FieldInput::Choices(values)) = this.inputs.get_mut(index)
                                && let Some(value) = values.get_mut(i)
                            {
                                *value = *checked;
                            }
                            this.clear_error_at(index, cx);
                        }));
                    div()
                        .debug_selector(move || selector.clone())
                        .child(checkbox)
                }))
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn clear_error_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(name) = self.spec.fields.get(index).map(|f| f.name.clone()) {
            self.clear_error(&name, cx);
        }
        cx.notify();
    }

    fn render_field(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let field = &self.spec.fields[index];
        let input = self.render_input(index, field, &self.inputs[index], cx);
        let theme = cx.theme();
        let mut column = v_flex().gap_1();
        // A checkbox carries its own label.
        if !matches!(field.kind, FieldKind::Boolean { .. }) {
            let mut label = h_flex().gap_1().child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(label_of(field).to_owned())),
            );
            if field.required {
                label = label.child(div().text_color(theme.danger).child("*"));
            }
            column = column.child(label);
        }
        if let Some(description) = &field.description {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(description.clone())),
            );
        }
        column = column.child(
            div()
                .debug_selector(move || format!("form-input-{index}"))
                .child(input),
        );
        if let Some(problem) = self.errors.get(&field.name) {
            column = column.child(
                div()
                    .debug_selector(|| "form-field-error".to_owned())
                    .text_xs()
                    .text_color(theme.danger)
                    .child(SharedString::from(problem.clone())),
            );
        }
        column.into_any_element()
    }

    fn render_summary(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let answer = self.spec.answer.clone().unwrap_or_default();
        v_flex()
            .debug_selector(|| "form-summary".to_owned())
            .gap_1()
            .children(self.spec.fields.iter().map(|field| {
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .min_w_32()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(label_of(field).to_owned())),
                    )
                    .child(div().flex_1().whitespace_normal().child(SharedString::from(
                        answer_text(field, answer.get(&field.name)),
                    )))
            }))
            .into_any_element()
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let disabled = self.waiting.is_none();
        let note = if self.sent {
            Some("Sent…")
        } else if disabled {
            Some("Not waiting for an answer")
        } else {
            None
        };
        let submit = self
            .spec
            .submit_label
            .clone()
            .unwrap_or_else(|| "Submit".into());
        h_flex()
            .gap_2()
            .items_center()
            .justify_end()
            .children(note.map(|note| {
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(note)
            }))
            .child(
                Button::new(self.element_id("cancel", 0))
                    .debug_selector(|| "form-cancel".to_owned())
                    .label("Cancel")
                    .disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
            )
            .child(
                Button::new(self.element_id("submit", 0))
                    .debug_selector(|| "form-submit".to_owned())
                    .primary()
                    .label(SharedString::from(submit))
                    .disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
            .into_any_element()
    }
}

/// The value of `field`: `None` when left empty, or what is wrong.
fn read_field(field: &FormField, input: &FieldInput, cx: &App) -> Result<Option<Value>, String> {
    let value = match (input, &field.kind) {
        (FieldInput::Line(state), FieldKind::Number { .. }) => {
            let text = state.read(cx).value().trim().to_owned();
            if text.is_empty() {
                return Ok(None);
            }
            let number = text
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .ok_or_else(|| field.kind.hint())?;
            Value::Number(number)
        }
        (FieldInput::Line(state), FieldKind::Integer { .. }) => {
            let text = state.read(cx).value().trim().to_owned();
            if text.is_empty() {
                return Ok(None);
            }
            Value::from(text.parse::<i64>().map_err(|_| field.kind.hint())?)
        }
        (FieldInput::Line(state), _) => {
            let text = state.read(cx).value().to_string();
            if text.trim().is_empty() {
                return Ok(None);
            }
            Value::String(text)
        }
        (FieldInput::Area(state), _) => {
            let text = state.read(cx).value().to_string();
            if text.trim().is_empty() {
                return Ok(None);
            }
            Value::String(text)
        }
        (FieldInput::Date(state), _) => match state.read(cx).date().start() {
            Some(day) => Value::String(day.format(ISO_DATE).to_string()),
            None => return Ok(None),
        },
        (FieldInput::Toggle(checked), _) => Value::Bool(*checked),
        (FieldInput::Choice(selected), FieldKind::Enum { options, .. }) => {
            match selected.and_then(|i| options.get(i)) {
                Some(option) => Value::String(option.value.clone()),
                None => return Ok(None),
            }
        }
        (FieldInput::Choices(picked), FieldKind::MultiEnum { options, .. }) => {
            let values: Vec<Value> = options
                .iter()
                .zip(picked)
                .filter(|(_, picked)| **picked)
                .map(|(option, _)| Value::String(option.value.clone()))
                .collect();
            if values.is_empty() && field.required {
                return Ok(None);
            }
            Value::Array(values)
        }
        _ => return Ok(None),
    };
    if field.kind.accepts(&value) {
        Ok(Some(value))
    } else {
        Err(field.kind.hint())
    }
}

impl Render for FormView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, background) = (theme.border, theme.secondary);
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let mut card = v_flex()
            .id(self.element_id("card", 0))
            .debug_selector(|| "form-card".to_owned())
            .track_focus(&self.focus_handle)
            .tab_group()
            .on_key_down(cx.listener(Self::on_key))
            // A click anywhere on a waiting card (a label, the padding)
            // brings the focus into the form; a click on an input has
            // already focused it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.waiting.is_some() && !this.focus_handle.contains_focused(window, cx) {
                        this.focus_first(window, cx);
                    }
                }),
            )
            .gap_3()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(background)
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::from(self.spec.title.clone())),
            );
        if let Some(description) = &self.spec.description {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(SharedString::from(description.clone())),
            );
        }
        match self.spec.state {
            FormState::Submitted => return card.child(self.render_summary(cx)).into_any_element(),
            FormState::Replied => {
                let reply = self.spec.reply.clone().unwrap_or_default();
                return card
                    .child(
                        div()
                            .debug_selector(|| "form-replied".to_owned())
                            .text_sm()
                            .text_color(muted)
                            .child("Answered in the chat:"),
                    )
                    .child(div().text_sm().child(SharedString::from(reply)))
                    .into_any_element();
            }
            FormState::Cancelled => {
                return card
                    .child(
                        div()
                            .debug_selector(|| "form-cancelled".to_owned())
                            .text_sm()
                            .text_color(muted)
                            .child("Cancelled: the form was closed without an answer."),
                    )
                    .into_any_element();
            }
            FormState::Pending => {}
        }
        if let Some(error) = &self.error {
            card = card.child(div().text_sm().text_color(danger).child(SharedString::from(
                format!("The answer was rejected: {error}"),
            )));
        }
        for index in 0..self.spec.fields.len() {
            card = card.child(self.render_field(index, cx));
        }
        card.child(self.render_footer(cx)).into_any_element()
    }
}

/// The view of block `id`, created from `spec` on first use.
fn view(id: BlockId, spec: &FormSpec, window: &mut Window, cx: &mut App) -> Entity<FormView> {
    super::block_state(id, window, cx, |window, cx| {
        FormView::new(id, spec.clone(), window, cx)
    })
}

/// Hands `pending` to the form of its block, which takes the focus.
pub fn attach(pending: PendingForm, window: &mut Window, cx: &mut App) {
    let request = pending.request();
    let form = view(request.block_id, &request.spec, window, cx);
    form.update(cx, |form, cx| form.attach(pending, window, cx));
}

/// The view of block `id`, if it was drawn or attached.
pub fn existing(id: BlockId, cx: &mut App) -> Option<Entity<FormView>> {
    super::existing_block_state::<FormView>(id, cx)
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Form(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let form = view(entry.block.id, spec, window, cx);
    form.update(cx, |form, cx| form.sync(spec, cx));
    form.into_any_element()
}

#[cfg(test)]
mod tests;
