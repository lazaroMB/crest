//! [`McpView`]: the `/mcp` modal (`<leader>S`, the `+` menu of the prompt,
//! the dock of the sidebar). The servers on the left, grouped by where they
//! are declared; the chosen one on the right with its state, its actions
//! and the Overview, Tools and Log tabs; or the add/edit form.
//!
//! Keys, with the list:
//!
//! - `↑`/`↓` (or `k`/`j`), `PageUp`/`PageDown`, `Home`/`End` choose a
//!   server; `Tab`/`Shift+Tab` go through the tabs;
//! - `a` adds a server, `e` edits the chosen one, `d` deletes it (asks
//!   first: `Enter` confirms, `Esc` keeps it);
//! - `t` tests it, `r` restarts it, `l` logs in (http with OAuth),
//!   `Space` turns it on or off;
//! - `/` goes to the filter, `f` to the tools filter, `o` unfolds the
//!   chosen tool;
//! - `Esc` reaches the modal host, which closes the modal.
//!
//! In the form `Tab`/`Shift+Tab` move between the fields, `Ctrl+Enter`
//! saves and `Esc` leaves the form (the modal stays). Every change goes
//! through the [`SessionController`]; the view touches no file.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use application_core::ports::mcp::McpServerState;
use application_core::usecases::mcp_servers::{McpAction, McpListing, McpServerInfo};
use domain::modal::ModalId;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, Styled as _, Subscription, Task, Window, div,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::form::{Draft, FormMode};
use super::model::{McpTab, ServerRow, server_rows, tone};
use crate::kit::{Space, TokenStyled as _, caption};
use crate::session::SessionController;

/// Rows `PageUp`/`PageDown` move.
const PAGE: usize = 10;
/// How often the spinners turn.
const TICK: Duration = Duration::from_millis(100);

/// Id of the `/mcp` modal.
pub fn mcp_modal_id() -> ModalId {
    ModalId::new("crest/mcp")
}

/// What the view tells the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEvent {
    /// A server was changed, added or removed: list them again.
    Changed,
}

/// What to do once the modal is open: `/mcp <name>`, `/mcp add`, `/mcp
/// tools <name>`, `/mcp restart <name>`...
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpOpen {
    /// The server to choose.
    pub server: Option<String>,
    /// The tab to show.
    pub tab: Option<McpTab>,
    /// Open the add form.
    pub add: bool,
    /// Then do this to the chosen server.
    pub then: Option<McpThen>,
}

/// A follow-up of [`McpOpen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpThen {
    /// Run an action.
    Act(McpAction),
    /// Ask to delete it.
    ConfirmDelete,
}

/// The `/mcp` modal's content.
pub struct McpView {
    pub(super) controller: Arc<dyn SessionController>,
    pub(super) listing: McpListing,
    pub(super) rows: Vec<ServerRow>,
    pub(super) selected: usize,
    pub(super) filter: Entity<InputState>,
    pub(super) tab: McpTab,
    pub(super) tool_filter: Entity<InputState>,
    /// The tool unfolded in the Tools tab.
    pub(super) open_tool: Option<String>,
    /// Secrets shown in the Overview, as `server/key`.
    pub(super) revealed: HashSet<String>,
    /// The action in flight of each server.
    pub(super) pending: HashMap<String, McpAction>,
    /// The last test or login outcome of each server, and when.
    pub(super) results: HashMap<String, (Result<String, String>, SystemTime)>,
    /// `d` was pressed: `Enter` deletes the chosen server.
    pub(super) confirm_delete: bool,
    /// The add/edit form, while it shows.
    pub(super) form: Option<Draft>,
    /// The footer's message.
    pub(super) status: Option<String>,
    /// A server to choose once the next listing has it (just saved).
    select_after: Option<String>,
    /// Keys of `mcp.open`, for the footer.
    pub(super) open_keys: Option<String>,
    pub(super) focus_handle: FocusHandle,
    pub(super) list_scroll: ScrollHandle,
    pub(super) body_scroll: ScrollHandle,
    /// The spinners' frame, moved by `tick` while something spins.
    pub(super) frame: usize,
    tick: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<McpEvent> for McpView {}

impl McpView {
    /// A view over `listing`; `open_keys` names the keys that open it.
    pub fn new(
        controller: Arc<dyn SessionController>,
        listing: McpListing,
        open_keys: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx
            .new(|cx| InputState::new(window, cx).placeholder("Filter servers  (/ to type here)"));
        let tool_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter tools  (f to type here)"));
        let refilter = cx.subscribe(&filter, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.refilter(cx);
            }
        });
        let retools = cx.subscribe(&tool_filter, |_: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let rows = server_rows(&listing, "");
        let mut this = Self {
            controller,
            listing,
            rows,
            selected: 0,
            filter,
            tab: McpTab::Overview,
            tool_filter,
            open_tool: None,
            revealed: HashSet::new(),
            pending: HashMap::new(),
            results: HashMap::new(),
            confirm_delete: false,
            form: None,
            status: None,
            select_after: None,
            open_keys,
            focus_handle: cx.focus_handle(),
            list_scroll: ScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            frame: 0,
            tick: None,
            _subscriptions: vec![refilter, retools],
        };
        this.ensure_tick(cx);
        this
    }

    // ----------------------------------------------------------- reading

    /// The rows shown, after the filter.
    pub fn rows(&self) -> &[ServerRow] {
        &self.rows
    }

    /// The whole listing.
    pub fn listing(&self) -> &McpListing {
        &self.listing
    }

    /// Index of the chosen row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The chosen server.
    pub fn selected_server(&self) -> Option<&McpServerInfo> {
        self.rows
            .get(self.selected)
            .and_then(|row| self.listing.servers.get(row.index))
    }

    /// The tab shown.
    pub fn tab(&self) -> McpTab {
        self.tab
    }

    /// The form, while it shows.
    pub fn form(&self) -> Option<&Draft> {
        self.form.as_ref()
    }

    /// The form's error line.
    pub fn form_error(&self) -> Option<&str> {
        self.form.as_ref().and_then(|form| form.error.as_deref())
    }

    /// Whether `d` waits for `Enter`.
    pub fn confirming(&self) -> bool {
        self.confirm_delete
    }

    /// The footer's message.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// The servers with an action in flight.
    pub fn pending(&self) -> &HashMap<String, McpAction> {
        &self.pending
    }

    /// The last test or login outcome of `name`.
    pub fn result(&self, name: &str) -> Option<&Result<String, String>> {
        self.results.get(name).map(|(result, _)| result)
    }

    /// Whether some server spins: starting, or with an action in flight.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
            || self
                .listing
                .servers
                .iter()
                .any(|s| s.state == McpServerState::Starting)
            || self.form.as_ref().is_some_and(|f| f.testing || f.saving)
    }

    /// Whether the server filter has the focus.
    pub fn is_filtering(&self, window: &Window, cx: &App) -> bool {
        self.filter.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Whether a text field of the view has the focus (the filters or a
    /// form field), rather than the view itself.
    pub fn field_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx) && !self.focus_handle.is_focused(window)
    }

    /// The filter text.
    pub fn query(&self, cx: &App) -> String {
        self.filter.read(cx).value().to_string()
    }

    /// The tools filter text.
    pub(super) fn tool_query(&self, cx: &App) -> String {
        self.tool_filter.read(cx).value().to_string()
    }

    pub(super) fn selected_name(&self) -> Option<String> {
        self.selected_server().map(|s| s.spec.name.clone())
    }

    // ---------------------------------------------------------- changing

    /// Replaces the listing, keeping the filter and the chosen server.
    pub fn set_listing(&mut self, listing: McpListing, cx: &mut Context<Self>) {
        let chosen = self.selected_name();
        self.listing = listing;
        self.rows = server_rows(&self.listing, &self.query(cx));
        let wanted = self.select_after.take();
        if !self.select_name(wanted.as_deref()) {
            self.select_name(chosen.as_deref());
        }
        self.ensure_tick(cx);
        cx.notify();
    }

    /// Chooses the row of server `name`, or keeps a valid row.
    fn select_name(&mut self, name: Option<&str>) -> bool {
        let found = name.and_then(|name| {
            self.rows
                .iter()
                .position(|row| self.listing.servers[row.index].spec.name == name)
        });
        self.selected = found.unwrap_or(self.selected);
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        crate::keyscroll::ensure_visible(&self.list_scroll, self.selected);
        found.is_some()
    }

    /// Does what `/mcp <args>` asked once the modal shows.
    pub fn apply_target(&mut self, target: McpOpen, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_delete = false;
        if let Some(name) = &target.server
            && !self.select_name(Some(name))
        {
            self.status = Some(format!("no MCP server named {name}"));
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return;
        }
        if let Some(tab) = target.tab {
            self.tab = tab;
        }
        if target.add {
            self.start_form(None, window, cx);
            return;
        }
        self.cancel_form(cx);
        match target.then {
            Some(McpThen::Act(action)) => {
                if let Some(name) = self.selected_name() {
                    self.act(&name, action, cx);
                }
            }
            Some(McpThen::ConfirmDelete) => self.ask_delete(cx),
            None => {}
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.rows = server_rows(&self.listing, &self.query(cx));
        self.selected = 0;
        self.confirm_delete = false;
        self.list_scroll.scroll_to_item(0);
        cx.notify();
    }

    pub(super) fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix != self.selected {
            self.open_tool = None;
            self.confirm_delete = false;
        }
        self.selected = ix.min(self.rows.len().saturating_sub(1));
        crate::keyscroll::ensure_visible(&self.list_scroll, self.selected);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let next = self.selected.saturating_add_signed(delta).min(last);
        self.select(next, cx);
    }

    pub(super) fn set_tab(&mut self, tab: McpTab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.body_scroll.scroll_to_item(0);
        cx.notify();
    }

    pub(super) fn toggle_tool(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.open_tool.as_deref() == Some(name) {
            self.open_tool = None;
        } else {
            self.open_tool = Some(name.to_owned());
        }
        cx.notify();
    }

    pub(super) fn reveal(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.revealed.remove(&key) {
            self.revealed.insert(key);
        }
        cx.notify();
    }

    /// Runs `action` on `name`; the outcome goes to the result line (a
    /// test, a login) or the footer, and the servers are listed again.
    pub fn act(&mut self, name: &str, action: McpAction, cx: &mut Context<Self>) {
        if self.pending.contains_key(name) {
            return;
        }
        let server = self.listing.get(name);
        let hint = match action {
            McpAction::Login if !server.is_some_and(McpServerInfo::has_oauth) => {
                Some("only http servers with OAuth log in")
            }
            McpAction::Test | McpAction::Restart
                if server.is_some_and(|s| {
                    matches!(
                        s.state,
                        McpServerState::Disabled | McpServerState::Untrusted
                    )
                }) =>
            {
                Some("turn the server on (or trust it) first")
            }
            _ => None,
        };
        if let Some(hint) = hint {
            self.status = Some(hint.to_owned());
            cx.notify();
            return;
        }
        self.status = None;
        self.pending.insert(name.to_owned(), action);
        self.ensure_tick(cx);
        cx.notify();
        let outcome = self.controller.mcp_act(name.to_owned(), action);
        let name = name.to_owned();
        cx.spawn(async move |this, cx| {
            let result = outcome.await;
            this.update(cx, |this, cx| {
                this.pending.remove(&name);
                match action {
                    McpAction::Test | McpAction::Login => {
                        this.results
                            .insert(name.clone(), (result, SystemTime::now()));
                    }
                    _ => {
                        this.status = Some(match &result {
                            Ok(message) | Err(message) => message.clone(),
                        });
                    }
                }
                cx.emit(McpEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `Space` and the switch: off when on, on when off.
    pub fn toggle_enabled(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.selected_server() else {
            return;
        };
        if server.state == McpServerState::Untrusted {
            self.status = Some("trust the server first".to_owned());
            cx.notify();
            return;
        }
        let action = if server.enabled() {
            McpAction::Disable
        } else {
            McpAction::Enable
        };
        let name = server.spec.name.clone();
        self.act(&name, action, cx);
    }

    /// `d`: asks before deleting the chosen server.
    pub fn ask_delete(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.selected_server() else {
            return;
        };
        if !server.editable() {
            self.status = Some(format!(
                "`{}` is declared in {}; remove it there",
                server.spec.name, server.saved_in
            ));
        } else {
            self.status = None;
            self.confirm_delete = true;
        }
        cx.notify();
    }

    /// `Enter` after `d`: deletes the chosen server.
    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        self.confirm_delete = false;
        let Some(name) = self.selected_name() else {
            return;
        };
        self.pending.insert(name.clone(), McpAction::Disable);
        let removing = self.controller.mcp_remove(name.clone());
        cx.spawn(async move |this, cx| {
            let result = removing.await;
            this.update(cx, |this, cx| {
                this.pending.remove(&name);
                this.status = Some(match result {
                    Ok(message) | Err(message) => message,
                });
                cx.emit(McpEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// `a` (no name) or `e` (the chosen server): shows the form.
    pub fn start_form(
        &mut self,
        edit: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.listing.can_edit {
            self.status = Some("this session cannot manage MCP servers".to_owned());
            cx.notify();
            return;
        }
        let mode = match edit {
            None => FormMode::Add,
            Some(name) => {
                let Some(server) = self.listing.get(&name) else {
                    return;
                };
                if !server.editable() {
                    self.status = Some(format!(
                        "`{name}` is declared in {}; edit it there",
                        server.saved_in
                    ));
                    cx.notify();
                    return;
                }
                FormMode::Edit(server.spec.clone())
            }
        };
        self.confirm_delete = false;
        self.status = None;
        let standalone = self.listing.workspace_file.is_none();
        let draft = Draft::new(mode, standalone, window, cx);
        draft.focus_first(window, cx);
        self.form = Some(draft);
        cx.notify();
    }

    /// `Esc` in the form: back to the detail.
    pub fn cancel_form(&mut self, cx: &mut Context<Self>) {
        if self.form.take().is_some() {
            cx.notify();
        }
    }

    /// `Ctrl+Enter`, **Add and start** / **Save and restart**.
    pub fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if form.saving {
            return;
        }
        let previous = match &form.mode {
            FormMode::Add => None,
            FormMode::Edit(spec) => Some(spec.name.clone()),
        };
        let draft = form.to_draft(cx);
        let taken: Vec<String> = self
            .listing
            .names()
            .into_iter()
            .filter(|n| previous.as_deref() != Some(n.as_str()))
            .collect();
        if let Err(problem) = draft.validate(&taken) {
            form.error = Some(problem);
            cx.notify();
            return;
        }
        form.error = None;
        form.saving = true;
        let name = draft.name.trim().to_owned();
        let saving = self.controller.mcp_save(previous, draft);
        self.ensure_tick(cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = saving.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(message) => {
                        this.form = None;
                        this.status = Some(message);
                        if !this.select_name(Some(&name)) {
                            this.select_after = Some(name);
                        }
                    }
                    Err(problem) => {
                        if let Some(form) = &mut this.form {
                            form.saving = false;
                            form.error = Some(problem);
                        }
                    }
                }
                cx.emit(McpEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// **Test connection**: connects the form's server without saving it.
    pub fn test_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if form.testing {
            return;
        }
        let draft = form.to_draft(cx);
        if let Err(problem) = draft.validate(&[]) {
            form.error = Some(problem);
            cx.notify();
            return;
        }
        form.error = None;
        form.testing = true;
        form.test = None;
        let testing = self.controller.mcp_test(draft);
        self.ensure_tick(cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = testing.await;
            this.update(cx, |this, cx| {
                if let Some(form) = &mut this.form {
                    form.testing = false;
                    form.test = Some(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Keeps the spinners turning while something spins.
    fn ensure_tick(&mut self, cx: &mut Context<Self>) {
        if self.tick.is_some() || !self.has_pending() {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let again = this
                    .update(cx, |this, cx| {
                        this.frame = this.frame.wrapping_add(1);
                        cx.notify();
                        if this.has_pending() {
                            true
                        } else {
                            this.tick = None;
                            false
                        }
                    })
                    .unwrap_or(false);
                if !again {
                    return;
                }
            }
        }));
    }

    // -------------------------------------------------------------- keys

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let m = &keys.modifiers;
        let key = keys.key.as_str();
        if m.alt || m.platform {
            return;
        }
        if m.control {
            if key == "enter" && self.form.is_some() {
                self.save_form(cx);
                cx.stop_propagation();
            }
            return;
        }
        if self.field_focused(window, cx) {
            let filtering = self.is_filtering(window, cx);
            match key {
                "escape" => window.focus(&self.focus_handle, cx),
                "tab" if self.form.is_some() => self.focus_field(!m.shift, window, cx),
                "enter" | "down" if filtering => window.focus(&self.focus_handle, cx),
                "up" if filtering => self.move_selection(-1, cx),
                "enter" | "tab" => window.focus(&self.focus_handle, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if self.form.is_some() {
            match key {
                "escape" => self.cancel_form(cx),
                "tab" => self.focus_field(!m.shift, window, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if self.confirm_delete {
            if matches!(key, "enter" | "y" | "d") {
                self.delete_selected(cx);
            } else {
                self.confirm_delete = false;
                self.status = Some("kept the server".to_owned());
                cx.notify();
            }
            // Even `Esc` only cancels the question.
            cx.stop_propagation();
            return;
        }
        let name = self.selected_name();
        match key {
            "down" | "j" => self.move_selection(1, cx),
            "up" | "k" => self.move_selection(-1, cx),
            "pagedown" => self.move_selection(PAGE as isize, cx),
            "pageup" => self.move_selection(-(PAGE as isize), cx),
            "home" => self.move_selection(isize::MIN, cx),
            "end" => self.move_selection(isize::MAX, cx),
            "tab" if m.shift => self.set_tab(self.tab.prev(), cx),
            "tab" => self.set_tab(self.tab.next(), cx),
            "a" => self.start_form(None, window, cx),
            "e" => {
                if let Some(name) = name {
                    self.start_form(Some(name), window, cx);
                }
            }
            "d" | "delete" => self.ask_delete(cx),
            "t" => {
                if let Some(name) = name {
                    self.act(&name, McpAction::Test, cx);
                }
            }
            "r" => {
                if let Some(name) = name {
                    self.act(&name, McpAction::Restart, cx);
                }
            }
            "l" => {
                if let Some(name) = name {
                    self.act(&name, McpAction::Login, cx);
                }
            }
            "space" => self.toggle_enabled(cx),
            "/" => {
                let handle = self.filter.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            "f" if self.tab == McpTab::Tools => {
                let handle = self.tool_filter.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            "o" if self.tab == McpTab::Tools => {
                let first = self.visible_tools(cx).first().map(|t| t.name.clone());
                if let Some(tool) = self.open_tool.clone().or(first) {
                    self.toggle_tool(&tool, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// `Tab` in the form: the next (or previous) field, wrapping around.
    pub(super) fn focus_field(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &self.form else {
            return;
        };
        let fields = form.fields();
        if fields.is_empty() {
            return;
        }
        let at = fields
            .iter()
            .position(|field| field.read(cx).focus_handle(cx).is_focused(window));
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => fields.len() - 1,
            (Some(at), true) => (at + 1) % fields.len(),
            (Some(at), false) => (at + fields.len() - 1) % fields.len(),
        };
        let handle = fields[next].read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    // ------------------------------------------------------------ render

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let keys = if self.form.is_some() {
            "Tab next field · Ctrl+Enter save · Esc cancel".to_owned()
        } else if self.confirm_delete {
            "Enter delete · Esc keep".to_owned()
        } else {
            let open = self
                .open_keys
                .as_deref()
                .map(|keys| format!(" · {keys} opens this"))
                .unwrap_or_default();
            format!(
                "↑↓ server · Tab tabs · a add · e edit · d delete · t test · r restart · l log \
                 in · Space on/off · / filter · Esc close{open}"
            )
        };
        let status = self.status.clone().map(|status| {
            div()
                .debug_selector(|| "mcp-status".to_owned())
                .child(SharedString::from(status))
        });
        v_flex()
            .gap_0p5()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .children(status)
            .child(
                div()
                    .debug_selector(|| "mcp-keys".to_owned())
                    .child(SharedString::from(keys)),
            )
    }
}

impl Focusable for McpView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for McpView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self.render_list(cx).into_any_element();
        let right = if self.form.is_some() {
            self.render_form(cx).into_any_element()
        } else if let Some(server) = self.selected_server().cloned() {
            self.render_detail(&server, cx).into_any_element()
        } else {
            div()
                .debug_selector(|| "mcp-empty-detail".to_owned())
                .flex_1()
                .p_3()
                .text_color(cx.theme().muted_foreground)
                .child(if self.listing.servers.is_empty() {
                    "No MCP servers yet. Add one, or declare it in init.lua."
                } else {
                    "Pick a server, or add one."
                })
                .into_any_element()
        };
        let footer = self.render_footer(cx).into_any_element();
        v_flex()
            .id("mcp")
            .debug_selector(|| "mcp-view".to_owned())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(list)
                    .child(right),
            )
            .child(footer)
    }
}

/// The color of a tone.
pub(super) fn tone_color(tone: super::model::Tone, cx: &App) -> gpui::Hsla {
    let theme = cx.theme();
    match tone {
        super::model::Tone::Ok => theme.success,
        super::model::Tone::Warn => theme.warning,
        super::model::Tone::Error => theme.danger,
        super::model::Tone::Busy => theme.info,
        super::model::Tone::Off => theme.muted_foreground,
    }
}

/// A state's dot.
pub(super) fn dot(state: &McpServerState, cx: &App) -> gpui::Div {
    div()
        .flex_none()
        .text_color(tone_color(tone(state), cx))
        .child("●")
}

/// A small note after a control: the key that does the same.
pub(super) fn key_hint(key: &'static str, cx: &App) -> gpui::Div {
    caption(key, cx).tk_px(Space::Xxs, cx)
}
