//! The add/edit form of a server: the fields `app.mcp.add` takes, without
//! having to know Lua. The text fields are `InputState` entities that keep
//! their text while the type, the OAuth switch or the scope change; what
//! they hold is read into an [`McpDraft`] when the form is saved or tested.

use application_core::ports::plugin::{ConfigOrigin, McpServerSpec};
use application_core::usecases::mcp_servers::{McpDraft, McpKind, McpOAuthDraft};
use domain::permission::ToolEffect;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::model::{EFFECTS, TEMPLATES};
use super::view::McpView;
use crate::activity::SPINNER;
use crate::kit::{caption, quiet_button, title, toggle_chip, tool_button, toolbar};

/// Width of a field's label, in pixels.
const LABEL_PX: f32 = 140.0;
/// Width of a text field, in pixels.
const FIELD_PX: f32 = 280.0;

/// Whether the form adds a server or edits one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormMode {
    /// A new server.
    Add,
    /// This server, as it was.
    Edit(McpServerSpec),
}

/// A list of rows of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rows {
    /// The arguments of a stdio server.
    Args,
    /// Its environment.
    Env,
    /// The headers of an http server.
    Headers,
}

/// The form and its fields.
pub struct Draft {
    /// Adding or editing.
    pub mode: FormMode,
    /// Local command or remote URL.
    pub kind: McpKind,
    /// Which `mcp.json` keeps it.
    pub scope: ConfigOrigin,
    /// Side effect of its tools.
    pub effect: ToolEffect,
    /// Whether it logs in through the browser.
    pub oauth: bool,
    /// Kept off.
    pub disabled: bool,
    /// No workspace: only the global scope.
    pub standalone: bool,
    name: Entity<InputState>,
    command: Entity<InputState>,
    cwd: Entity<InputState>,
    url: Entity<InputState>,
    client_id: Entity<InputState>,
    client_secret: Entity<InputState>,
    scopes: Entity<InputState>,
    callback_port: Entity<InputState>,
    timeout: Entity<InputState>,
    startup_timeout: Entity<InputState>,
    args: Vec<Entity<InputState>>,
    env: Vec<(Entity<InputState>, Entity<InputState>)>,
    headers: Vec<(Entity<InputState>, Entity<InputState>)>,
    /// What is wrong with the form, after a save or a test.
    pub error: Option<String>,
    /// **Test connection** runs.
    pub testing: bool,
    /// What the last test said.
    pub test: Option<Result<String, String>>,
    /// The save runs.
    pub saving: bool,
    _events: Vec<Subscription>,
}

impl Draft {
    /// An empty form, or one filled with the server of `mode`.
    pub fn new(
        mode: FormMode,
        standalone: bool,
        window: &mut Window,
        cx: &mut Context<McpView>,
    ) -> Self {
        let values = match &mode {
            FormMode::Add => McpDraft::new(),
            FormMode::Edit(spec) => McpDraft::from_spec(spec),
        };
        Self::from_values(mode, values, standalone, window, cx)
    }

    /// A form holding `values`.
    pub fn from_values(
        mode: FormMode,
        values: McpDraft,
        standalone: bool,
        window: &mut Window,
        cx: &mut Context<McpView>,
    ) -> Self {
        let mut events = Vec::new();
        let mut field = |value: &str, placeholder: &'static str, masked: bool| {
            let (input, event) = text_field(value, placeholder, masked, window, cx);
            events.push(event);
            input
        };
        let oauth = values.oauth.clone().unwrap_or_default();
        let name = field(&values.name, "github", false);
        let command = field(&values.command, "npx", false);
        let cwd = field(&values.cwd, "empty: the workspace", false);
        let url = field(&values.url, "https://example.com/mcp", false);
        let client_id = field(&oauth.client_id, "optional", false);
        let client_secret = field(&oauth.client_secret, "optional", true);
        let scopes = field(&oauth.scopes, "space separated", false);
        let callback_port = field(&oauth.callback_port, "any free port", false);
        let timeout = field(&values.timeout_s, "120", false);
        let startup_timeout = field(&values.startup_timeout_s, "120", false);
        let args = values
            .args
            .iter()
            .map(|arg| field(arg, "argument", false))
            .collect();
        let mut pairs = |rows: &[(String, String)], key: &'static str, value: &'static str| {
            rows.iter()
                .map(|(k, v)| (field(k, key, false), field(v, value, false)))
                .collect::<Vec<_>>()
        };
        let env = pairs(&values.env, "NAME", "value or ${VAR}");
        let headers = pairs(&values.headers, "Header", "value or ${TOKEN}");
        Self {
            mode,
            kind: values.kind,
            scope: if standalone {
                ConfigOrigin::Global
            } else {
                values.scope
            },
            effect: values.effect,
            oauth: values.oauth.is_some(),
            disabled: values.disabled,
            standalone,
            name,
            command,
            cwd,
            url,
            client_id,
            client_secret,
            scopes,
            callback_port,
            timeout,
            startup_timeout,
            args,
            env,
            headers,
            error: None,
            testing: false,
            test: None,
            saving: false,
            _events: events,
        }
    }

    /// Whether the form adds a server.
    pub fn is_add(&self) -> bool {
        self.mode == FormMode::Add
    }

    /// The name field's text.
    pub fn name(&self, cx: &App) -> String {
        self.name.read(cx).value().to_string()
    }

    /// The text fields in `Tab` order, for the kind and the OAuth choice
    /// shown.
    pub fn fields(&self) -> Vec<Entity<InputState>> {
        let mut fields = vec![self.name.clone()];
        match self.kind {
            McpKind::Stdio => {
                fields.push(self.command.clone());
                fields.extend(self.args.iter().cloned());
                for (k, v) in &self.env {
                    fields.push(k.clone());
                    fields.push(v.clone());
                }
                fields.push(self.cwd.clone());
            }
            McpKind::Http => {
                fields.push(self.url.clone());
                for (k, v) in &self.headers {
                    fields.push(k.clone());
                    fields.push(v.clone());
                }
                if self.oauth {
                    fields.extend([
                        self.client_id.clone(),
                        self.client_secret.clone(),
                        self.scopes.clone(),
                        self.callback_port.clone(),
                    ]);
                }
            }
        }
        fields.push(self.timeout.clone());
        fields.push(self.startup_timeout.clone());
        fields
    }

    /// Gives the name field the focus.
    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        let handle = self.name.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// What the fields hold.
    pub fn to_draft(&self, cx: &App) -> McpDraft {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let pairs = |rows: &[(Entity<InputState>, Entity<InputState>)]| {
            rows.iter().map(|(k, v)| (text(k), text(v))).collect()
        };
        McpDraft {
            name: text(&self.name),
            kind: self.kind,
            scope: self.scope,
            command: text(&self.command),
            args: self.args.iter().map(text).collect(),
            env: pairs(&self.env),
            cwd: text(&self.cwd),
            url: text(&self.url),
            headers: pairs(&self.headers),
            oauth: self.oauth.then(|| McpOAuthDraft {
                client_id: text(&self.client_id),
                client_secret: text(&self.client_secret),
                scopes: text(&self.scopes),
                callback_port: text(&self.callback_port),
            }),
            effect: self.effect,
            timeout_s: text(&self.timeout),
            startup_timeout_s: text(&self.startup_timeout),
            disabled: self.disabled,
        }
    }

    fn rows_mut(&mut self, rows: Rows) -> usize {
        match rows {
            Rows::Args => self.args.len(),
            Rows::Env => self.env.len(),
            Rows::Headers => self.headers.len(),
        }
    }
}

/// A text field holding `value`; `Enter` in it moves to the next field,
/// `Ctrl+Enter` (or `Cmd+Enter`) saves.
fn text_field(
    value: &str,
    placeholder: &'static str,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<McpView>,
) -> (Entity<InputState>, Subscription) {
    let value = value.to_owned();
    let input = cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        if masked {
            state = state.masked(true);
        }
        state.set_value(value, window, cx);
        state
    });
    let event = cx.subscribe_in(
        &input,
        window,
        |this: &mut McpView, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter {
                secondary: true, ..
            } => this.save_form(cx),
            InputEvent::PressEnter { .. } => this.focus_field(true, window, cx),
            _ => {}
        },
    );
    (input, event)
}

impl McpView {
    /// A template of the add form: the fields take its values.
    pub fn apply_template(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(form), Some(template)) = (&self.form, TEMPLATES.get(ix)) else {
            return;
        };
        let values = template.apply(&form.to_draft(cx));
        let standalone = form.standalone;
        let draft = Draft::from_values(FormMode::Add, values, standalone, window, cx);
        draft.focus_first(window, cx);
        self.form = Some(draft);
        cx.notify();
    }

    fn set_kind(&mut self, kind: McpKind, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.kind = kind;
            cx.notify();
        }
    }

    fn set_scope(&mut self, scope: ConfigOrigin, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.scope = scope;
            cx.notify();
        }
    }

    fn set_effect(&mut self, effect: ToolEffect, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.effect = effect;
            cx.notify();
        }
    }

    fn set_oauth(&mut self, oauth: bool, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.oauth = oauth;
            cx.notify();
        }
    }

    /// `+ argument`, `+ variable`, `+ header`: an empty row, focused.
    pub fn add_row(&mut self, rows: Rows, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_none() {
            return;
        }
        let (key_hint, value_hint) = match rows {
            Rows::Args => ("argument", ""),
            Rows::Env => ("NAME", "value or ${VAR}"),
            Rows::Headers => ("Header", "value or ${TOKEN}"),
        };
        let (first, first_event) = text_field("", key_hint, false, window, cx);
        let second = (rows != Rows::Args).then(|| text_field("", value_hint, false, window, cx));
        let Some(form) = &mut self.form else {
            return;
        };
        form._events.push(first_event);
        match (rows, second) {
            (Rows::Env, Some((value, event))) => {
                form._events.push(event);
                form.env.push((first.clone(), value));
            }
            (Rows::Headers, Some((value, event))) => {
                form._events.push(event);
                form.headers.push((first.clone(), value));
            }
            _ => form.args.push(first.clone()),
        }
        let handle = first.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// `×` of a row.
    pub fn remove_row(&mut self, rows: Rows, ix: usize, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if ix >= form.rows_mut(rows) {
            return;
        }
        match rows {
            Rows::Args => {
                form.args.remove(ix);
            }
            Rows::Env => {
                form.env.remove(ix);
            }
            Rows::Headers => {
                form.headers.remove(ix);
            }
        }
        cx.notify();
    }

    /// The form pane.
    pub(super) fn render_form(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let Some(form) = &self.form else {
            return div().into_any_element();
        };
        let heading = match &form.mode {
            FormMode::Add => "Add an MCP server".to_owned(),
            FormMode::Edit(spec) => format!("Edit {}", spec.name),
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        if form.is_add() {
            rows.push(
                h_flex()
                    .debug_selector(|| "mcp-templates".to_owned())
                    .gap_1()
                    .items_center()
                    .child(caption("Start from", cx))
                    .children(TEMPLATES.iter().enumerate().map(|(ix, template)| {
                        toggle_chip(template.label, false, cx)
                            .id(("mcp-template", ix))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_template(ix, window, cx);
                            }))
                    }))
                    .into_any_element(),
            );
        }
        rows.push(form_row(
            "Name",
            Some("letters, digits, - and _"),
            input(&form.name, FIELD_PX),
            cx,
        ));
        let stdio = form.kind == McpKind::Stdio;
        rows.push(form_row(
            "Type",
            Some(if stdio {
                "a local program crest starts"
            } else {
                "a remote server crest connects to"
            }),
            segmented(
                "mcp-type",
                vec![
                    ("Local command (stdio)", stdio, McpKind::Stdio),
                    ("Remote URL (http)", !stdio, McpKind::Http),
                ],
                |this: &mut Self, kind, cx| this.set_kind(kind, cx),
                cx,
            ),
            cx,
        ));
        if stdio {
            rows.push(form_row(
                "Command",
                None,
                input(&form.command, FIELD_PX),
                cx,
            ));
            rows.push(form_row(
                "Arguments",
                Some("one per row"),
                self.render_rows(Rows::Args, cx),
                cx,
            ));
            rows.push(form_row(
                "Environment",
                Some("${VAR} and ${VAR:-default} expand"),
                self.render_rows(Rows::Env, cx),
                cx,
            ));
            rows.push(form_row(
                "Working dir",
                Some("empty: the workspace"),
                input(&form.cwd, FIELD_PX),
                cx,
            ));
        } else {
            rows.push(form_row("URL", None, input(&form.url, FIELD_PX), cx));
            rows.push(form_row(
                "Headers",
                Some("e.g. Authorization"),
                self.render_rows(Rows::Headers, cx),
                cx,
            ));
            let oauth = v_flex()
                .gap_2()
                .child(segmented(
                    "mcp-oauth",
                    vec![("No", !form.oauth, false), ("Yes", form.oauth, true)],
                    |this: &mut Self, on, cx| this.set_oauth(on, cx),
                    cx,
                ))
                .when(form.oauth, |oauth| {
                    oauth.child(
                        v_flex()
                            .gap_1()
                            .pl_2()
                            .border_l_1()
                            .border_color(theme.border)
                            .child(caption(
                                "Leave the client empty to register crest on its own (dynamic \
                                 registration).",
                                cx,
                            ))
                            .child(form_row(
                                "Client id",
                                None,
                                input(&form.client_id, FIELD_PX),
                                cx,
                            ))
                            .child(form_row(
                                "Client secret",
                                None,
                                input(&form.client_secret, FIELD_PX),
                                cx,
                            ))
                            .child(form_row("Scopes", None, input(&form.scopes, FIELD_PX), cx))
                            .child(form_row(
                                "Callback port",
                                None,
                                input(&form.callback_port, 120.0),
                                cx,
                            )),
                    )
                });
            rows.push(form_row(
                "OAuth",
                Some("log in through the browser"),
                oauth,
                cx,
            ));
        }
        if !form.standalone {
            let global = form.scope == ConfigOrigin::Global;
            rows.push(form_row(
                "Available in",
                Some(if global {
                    "~/.config/crest/mcp.json"
                } else {
                    ".crest/mcp.json"
                }),
                segmented(
                    "mcp-scope",
                    vec![
                        ("Every project", global, ConfigOrigin::Global),
                        ("This workspace", !global, ConfigOrigin::Workspace),
                    ],
                    |this: &mut Self, scope, cx| this.set_scope(scope, cx),
                    cx,
                ),
                cx,
            ));
        }
        let effect_hint = EFFECTS
            .iter()
            .find(|(effect, _, _)| *effect == form.effect)
            .map(|(_, _, hint)| *hint);
        rows.push(form_row(
            "Effect",
            effect_hint,
            segmented(
                "mcp-effect",
                EFFECTS
                    .iter()
                    .map(|(effect, label, _)| (*label, *effect == form.effect, *effect))
                    .collect(),
                |this: &mut Self, effect, cx| this.set_effect(effect, cx),
                cx,
            ),
            cx,
        ));
        rows.push(form_row(
            "Timeouts",
            Some("seconds"),
            h_flex()
                .gap_2()
                .items_center()
                .child(input(&form.timeout, 80.0))
                .child(caption("per call", cx))
                .child(input(&form.startup_timeout, 80.0))
                .child(caption("to start", cx)),
            cx,
        ));
        let body = v_flex().gap_3().children(rows).into_any_element();
        let spinner = SPINNER[self.frame % SPINNER.len()];
        let error = form.error.clone().map(|error| {
            h_flex()
                .debug_selector(|| "mcp-form-error".to_owned())
                .gap_2()
                .px_3()
                .text_sm()
                .child(div().text_color(theme.danger).child("✗"))
                .child(SharedString::from(error))
        });
        let testing = form.testing.then(|| {
            h_flex()
                .gap_2()
                .px_3()
                .text_sm()
                .child(div().child(spinner))
                .child("Connecting…")
        });
        let test = form.test.clone().map(|test| {
            let (mark, color, text) = match test {
                Ok(text) => ("✓", theme.success, text),
                Err(text) => ("✗", theme.danger, text),
            };
            h_flex()
                .debug_selector(|| "mcp-form-test".to_owned())
                .gap_2()
                .px_3()
                .text_sm()
                .child(div().text_color(color).child(mark))
                .child(SharedString::from(text))
        });
        let save_label = if form.is_add() {
            "Add and start"
        } else {
            "Save and restart"
        };
        let saving = form.saving;
        v_flex()
            .debug_selector(|| "mcp-form".to_owned())
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(title(heading))
                    .child(caption("saving starts it at once", cx)),
            )
            .child(
                crate::keyscroll::list("mcp-form-body", &self.body_scroll, vec![body])
                    .flex_1()
                    .w_full()
                    .p_3(),
            )
            .children(error)
            .children(testing)
            .children(test)
            .child(
                toolbar(cx)
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        quiet_button("mcp-form-test", "Test connection", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.test_form(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        quiet_button("mcp-form-cancel", "Cancel", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_form(cx))),
                    )
                    .child(if saving {
                        tool_button("mcp-save", "Saving…", cx)
                            .opacity(0.5)
                            .into_any_element()
                    } else {
                        tool_button("mcp-save", save_label, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.save_form(cx)))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// The rows of arguments, variables or headers with their `×` and the
    /// link that adds one.
    fn render_rows(&self, rows: Rows, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(form) = &self.form else {
            return div().into_any_element();
        };
        let (add_label, selector) = match rows {
            Rows::Args => ("+ argument", "mcp-arg"),
            Rows::Env => ("+ variable", "mcp-env"),
            Rows::Headers => ("+ header", "mcp-header"),
        };
        let remove = |ix: usize, cx: &Context<Self>| {
            div()
                .id((selector, ix))
                .cursor_pointer()
                .text_color(theme.muted_foreground)
                .child("×")
                .on_click(cx.listener(move |this, _, _, cx| this.remove_row(rows, ix, cx)))
        };
        let mut list = v_flex().gap_1();
        match rows {
            Rows::Args => {
                for (ix, arg) in form.args.iter().enumerate() {
                    list = list.child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(input(arg, FIELD_PX))
                            .child(remove(ix, cx)),
                    );
                }
            }
            Rows::Env | Rows::Headers => {
                let pairs = if rows == Rows::Env {
                    &form.env
                } else {
                    &form.headers
                };
                let sep = if rows == Rows::Env { "=" } else { ":" };
                for (ix, (key, value)) in pairs.iter().enumerate() {
                    list = list.child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(input(key, 120.0))
                            .child(caption(sep, cx))
                            .child(input(value, FIELD_PX - 130.0))
                            .child(remove(ix, cx)),
                    );
                }
            }
        }
        list.child(
            div()
                .id((selector, usize::MAX))
                .debug_selector(move || format!("{selector}-add"))
                .cursor_pointer()
                .text_xs()
                .text_color(theme.primary)
                .child(add_label)
                .on_click(cx.listener(move |this, _, window, cx| this.add_row(rows, window, cx))),
        )
        .into_any_element()
    }
}

/// A text field `width` pixels wide.
fn input(state: &Entity<InputState>, width: f32) -> gpui::Div {
    div().w(px(width)).child(Input::new(state))
}

/// A row of the form: the label and its hint on the left, the control on
/// the right.
fn form_row(
    label: &'static str,
    hint: Option<&'static str>,
    control: impl IntoElement,
    cx: &Context<McpView>,
) -> AnyElement {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            v_flex()
                .w(px(LABEL_PX))
                .flex_none()
                .child(label)
                .children(hint.map(|hint| caption(hint, cx))),
        )
        .child(div().flex_1().min_w_0().child(control))
        .into_any_element()
}

/// A row of choices, one on: `(label, on, value)`.
fn segmented<V: Copy + 'static>(
    id: &'static str,
    options: Vec<(&'static str, bool, V)>,
    pick: impl Fn(&mut McpView, V, &mut Context<McpView>) + Copy + 'static,
    cx: &Context<McpView>,
) -> gpui::Div {
    h_flex()
        .gap_1()
        .flex_wrap()
        .children(
            options
                .into_iter()
                .enumerate()
                .map(|(ix, (label, on, value))| {
                    toggle_chip(label, on, cx)
                        .id((id, ix))
                        .on_click(cx.listener(move |this, _, _, cx| pick(this, value, cx)))
                }),
        )
}
