//! [`PermissionsView`]: the `/permissions` modal (`<leader>A`), which lists
//! the permission rules in force and removes or promotes them.
//!
//! The rules come grouped as the policy holds them: **Session** ("allow
//! always" rules of this session), **Project** (those of
//! `.crest/permissions.local.toml`) and **Deny** (denials of either
//! scope). Each row shows the rule in its notation
//! (`run_command(cargo test *)`) and its scope. The current permission
//! mode is shown on top, with the key that cycles it.
//!
//! The list has the focus when it opens:
//!
//! - `↑`/`↓` (or `k`/`j`), `PageUp`/`PageDown`, `Home`/`End` move the
//!   selection, which stays in view;
//! - `d` or `Delete` removes the selected rule; a project rule asks first
//!   (`y`, `d` or `Enter` confirm, any other key cancels);
//! - `p` turns a session rule into a project rule;
//! - `m` cycles the permission mode (as `<leader>p` does elsewhere);
//! - `/` goes to the filter box, where typing filters the rows live;
//!   `Enter` or `↓` go back to the list.
//!
//! `Esc` reaches the modal host, which closes the modal.
//!
//! The changes go through the [`SessionController`], which applies them to
//! the shared policy (so the next approval already sees them) and to the
//! project file; the view never touches files itself. When the project
//! file has entries crest cannot read, they are listed and crest does not
//! rewrite the file (removing or promoting a project rule fails).

use std::sync::Arc;

use application::usecases::permissions::RuleListing;
use domain::modal::ModalId;
use domain::permission::{PermissionMode, PermissionRule, RuleDecision, RuleScope};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::session::SessionController;

/// Rows `PageUp`/`PageDown` move.
const PAGE: usize = 10;

/// What the modal shows when there are no rules at all.
pub const EMPTY_TEXT: &str = "No rules yet — answer s or p in an approval to add one";

/// Id of the `/permissions` modal.
pub fn permissions_modal_id() -> ModalId {
    ModalId::new("crest/permissions")
}

/// The group a rule is listed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleGroup {
    /// "Allow always" for this session.
    Session,
    /// "Allow always" in the project file.
    Project,
    /// Denials, of either scope.
    Deny,
}

impl RuleGroup {
    /// The group's title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Project => "Project",
            Self::Deny => "Deny",
        }
    }
}

/// One rule as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRow {
    /// Its group.
    pub group: RuleGroup,
    /// The rule.
    pub rule: PermissionRule,
    /// Its notation (`run_command(cargo test *)`).
    pub text: String,
}

impl RuleRow {
    /// `session` or `project`.
    pub fn scope(&self) -> &'static str {
        scope_name(self.rule.scope)
    }
}

fn scope_name(scope: RuleScope) -> &'static str {
    match scope {
        RuleScope::Session => "session",
        RuleScope::Project => "project",
    }
}

/// The rows of `listing` whose notation, group or scope contain `query`
/// (ignoring case), in group order.
pub fn rule_rows(listing: &RuleListing, query: &str) -> Vec<RuleRow> {
    let query = query.trim().to_lowercase();
    [
        (RuleGroup::Session, &listing.session),
        (RuleGroup::Project, &listing.project),
        (RuleGroup::Deny, &listing.deny),
    ]
    .into_iter()
    .flat_map(|(group, rules)| {
        rules.iter().map(move |rule| RuleRow {
            group,
            rule: rule.clone(),
            text: rule.to_string(),
        })
    })
    .filter(|row| {
        query.is_empty()
            || row.text.to_lowercase().contains(&query)
            || row.group.title().to_lowercase().contains(&query)
            || row.scope().contains(&query)
    })
    .collect()
}

/// What the view asks the session to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionsEvent {
    /// `m`: cycle the permission mode (the session tells the status bar).
    CycleMode,
}

/// The `/permissions` modal's content.
pub struct PermissionsView {
    controller: Arc<dyn SessionController>,
    listing: RuleListing,
    rows: Vec<RuleRow>,
    selected: usize,
    mode: PermissionMode,
    /// Keys of `permission.cycle` (`Ctrl+G p`), if bound.
    mode_keys: Option<String>,
    /// A project rule waiting for the delete to be confirmed.
    confirm: Option<PermissionRule>,
    status: Option<String>,
    search: Entity<InputState>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PermissionsEvent> for PermissionsView {}

impl PermissionsView {
    /// A view over `listing`; `mode_keys` names the keys that cycle the
    /// mode.
    pub fn new(
        controller: Arc<dyn SessionController>,
        listing: RuleListing,
        mode_keys: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter the rules…  (/ to type here)")
        });
        let changed = cx.subscribe(&search, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.refilter(cx);
            }
        });
        let mode = controller.permission_mode();
        let rows = rule_rows(&listing, "");
        Self {
            controller,
            listing,
            rows,
            selected: 0,
            mode,
            mode_keys,
            confirm: None,
            status: None,
            search,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _subscriptions: vec![changed],
        }
    }

    /// The rules shown, after the filter.
    pub fn rows(&self) -> &[RuleRow] {
        &self.rows
    }

    /// The whole listing.
    pub fn listing(&self) -> &RuleListing {
        &self.listing
    }

    /// Index of the selected row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The mode shown on top.
    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// The last message of the footer (removed, promoted, why not).
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// The project rule whose delete waits for a confirmation.
    pub fn confirming(&self) -> Option<&PermissionRule> {
        self.confirm.as_ref()
    }

    /// The scroll of the rows.
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// Whether the filter box has the focus.
    pub fn is_filtering(&self, window: &Window, cx: &App) -> bool {
        self.search.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The filter text.
    pub fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().to_string()
    }

    /// Replaces the filter text.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.to_owned();
        self.search
            .update(cx, |search, cx| search.set_value(query, window, cx));
        self.refilter(cx);
    }

    /// Shows `mode` on top (it changed elsewhere).
    pub fn set_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    /// Replaces the listing, keeping the filter and (clamped) the
    /// selection.
    pub fn set_listing(&mut self, listing: RuleListing, cx: &mut Context<Self>) {
        self.listing = listing;
        self.rows = rule_rows(&self.listing, &self.query(cx));
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    /// Asks the controller for the rules again.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let rules = self.controller.permission_rules();
        cx.spawn(async move |this, cx| {
            let listing = rules.await;
            this.update(cx, |this, cx| this.set_listing(listing, cx))
                .ok();
        })
        .detach();
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.rows = rule_rows(&self.listing, &self.query(cx));
        self.selected = 0;
        self.confirm = None;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        crate::keyscroll::ensure_visible(&self.scroll, self.selected);
        cx.notify();
    }

    fn selected_row(&self) -> Option<&RuleRow> {
        self.rows.get(self.selected)
    }

    /// `d`: removes the selected rule; a project rule asks first.
    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.rule.scope == RuleScope::Project && self.confirm.as_ref() != Some(&row.rule) {
            self.status = None;
            self.confirm = Some(row.rule);
            cx.notify();
            return;
        }
        self.confirm = None;
        let text = row.text;
        let done = match row.rule.scope {
            RuleScope::Session => format!("removed `{text}` from this session"),
            RuleScope::Project => format!("removed `{text}` from the project file"),
        };
        let removing = self.controller.remove_permission_rule(row.rule);
        self.finish(removing, done, format!("`{text}` was not removed"), cx);
    }

    /// `p`: makes the selected session rule a project rule.
    pub fn promote_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        self.confirm = None;
        if row.rule.scope == RuleScope::Project {
            self.status = Some(format!("`{}` is already a project rule", row.text));
            cx.notify();
            return;
        }
        let text = row.text;
        let promoting = self.controller.promote_permission_rule(row.rule);
        self.finish(
            promoting,
            format!("`{text}` now applies to the project"),
            format!("`{text}` stays a session rule"),
            cx,
        );
    }

    /// Waits for a change, then says how it went and lists the rules
    /// again.
    fn finish(
        &mut self,
        change: futures::future::BoxFuture<'static, Result<(), String>>,
        done: String,
        failed: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = change.await;
            this.update(cx, |this, cx| {
                this.status = Some(match result {
                    Ok(()) => done,
                    Err(err) => format!("{failed}: {err}"),
                });
                this.reload(cx);
            })
            .ok();
        })
        .detach();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let m = &keys.modifiers;
        if m.control || m.alt || m.platform {
            return;
        }
        let key = keys.key.as_str();
        if self.is_filtering(window, cx) {
            match key {
                "enter" | "down" => window.focus(&self.focus_handle, cx),
                "up" => self.move_selection(-1, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if self.confirm.is_some() {
            if matches!(key, "y" | "d" | "delete" | "enter") {
                self.delete_selected(cx);
            } else {
                self.confirm = None;
                self.status = Some("kept the rule".into());
                cx.notify();
            }
            // Even `Esc` only cancels the question.
            cx.stop_propagation();
            return;
        }
        match key {
            "down" | "j" => self.move_selection(1, cx),
            "up" | "k" => self.move_selection(-1, cx),
            "pagedown" => self.move_selection(PAGE as isize, cx),
            "pageup" => self.move_selection(-(PAGE as isize), cx),
            "home" => self.move_selection(isize::MIN, cx),
            "end" => self.move_selection(isize::MAX, cx),
            "d" | "delete" => self.delete_selected(cx),
            "p" => self.promote_selected(cx),
            "m" => cx.emit(PermissionsEvent::CycleMode),
            "/" => {
                let handle = self.search.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let keys = self.mode_keys.as_deref().map_or_else(
            || "m cycles it".to_owned(),
            |keys| format!("{keys} cycles it (m here)"),
        );
        let file = self.listing.project_file.as_ref().map(|file| {
            div()
                .debug_selector(|| "permissions-file".to_owned())
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(format!(
                    "Project rules live in {}",
                    file.display()
                )))
        });
        let warnings = (!self.listing.warnings.is_empty()).then(|| {
            v_flex()
                .debug_selector(|| "permissions-warnings".to_owned())
                .text_xs()
                .text_color(theme.warning)
                .child(
                    "The project file has entries crest cannot read, so crest will not \
                     rewrite it (removing or promoting project rules fails) until they are \
                     fixed:",
                )
                .children(
                    self.listing
                        .warnings
                        .iter()
                        .map(|warning| div().pl_2().child(SharedString::from(warning.clone()))),
                )
        });
        v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .debug_selector(|| "permissions-mode".to_owned())
                    .gap_2()
                    .child("Mode:")
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(SharedString::from(self.mode.to_string())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(keys)),
                    ),
            )
            .children(file)
            .children(warnings)
            .child(Input::new(&self.search))
    }

    fn render_row(&self, ix: usize, row: &RuleRow, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let first_of_group = ix == 0 || self.rows[ix - 1].group != row.group;
        let group = first_of_group.then(|| {
            div()
                .pt_2()
                .pb_0p5()
                .px_2()
                .text_xs()
                .text_color(theme.primary)
                .child(row.group.title())
        });
        let mut tags = vec![row.scope()];
        if row.rule.decision == RuleDecision::Deny {
            tags.push("deny");
        }
        let mut line = h_flex()
            .id(("permissions-row", ix))
            .debug_selector(move || format!("permissions-row-{ix}"))
            .gap_3()
            .px_2()
            .py_0p5()
            .rounded_sm()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(SharedString::from(row.text.clone())),
            )
            .children(tags.into_iter().map(|tag| {
                div()
                    .flex_none()
                    .px_1()
                    .text_xs()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_sm()
                    .text_color(muted)
                    .child(tag)
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = ix;
                this.confirm = None;
                cx.notify();
            }));
        if ix == self.selected {
            line = line.bg(theme.list_active);
        }
        v_flex().children(group).child(line)
    }
}

impl Focusable for PermissionsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PermissionsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| self.render_row(ix, row, cx).into_any_element())
            .collect();
        let header = self.render_header(cx).into_any_element();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let listing = &self.listing;
        let none =
            listing.session.is_empty() && listing.project.is_empty() && listing.deny.is_empty();
        let empty = rows.is_empty().then(|| {
            div()
                .debug_selector(|| "permissions-empty".to_owned())
                .p_3()
                .text_color(muted)
                .child(if none {
                    EMPTY_TEXT
                } else {
                    "No rule matches the filter"
                })
        });
        let confirm = self.confirm.as_ref().map(|rule| {
            let file = listing.project_file.as_ref().map_or_else(
                || "the project file".to_owned(),
                |f| f.display().to_string(),
            );
            div()
                .debug_selector(|| "permissions-confirm".to_owned())
                .text_color(theme.warning)
                .child(SharedString::from(format!(
                    "Delete `{rule}` from {file}?  y / d yes · any other key no"
                )))
        });
        let status = self.status.clone().map(|status| {
            div()
                .debug_selector(|| "permissions-status".to_owned())
                .child(SharedString::from(status))
        });
        let footer = v_flex()
            .gap_0p5()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(muted)
            .children(confirm)
            .children(status)
            .child(
                div()
                    .debug_selector(|| "permissions-keys".to_owned())
                    .child(
                        "↑↓ move · / filter · d delete · p move to project · m cycle mode · \
                         Esc close",
                    ),
            );
        v_flex()
            .id("permissions")
            .debug_selector(|| "permissions-view".to_owned())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .child(header)
            .child(
                crate::keyscroll::list("permissions-rows", &self.scroll, rows)
                    .flex_1()
                    .w_full()
                    .p_1(),
            )
            .children(empty)
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(text: &str) -> PermissionRule {
        text.parse().unwrap()
    }

    #[test]
    fn rows_follow_the_groups_and_the_filter() {
        let listing = RuleListing {
            session: vec![rule("run_command(cargo test *)")],
            project: vec![rule("edit_file(src/*)").with_scope(RuleScope::Project)],
            deny: vec![
                PermissionRule::parse(
                    "run_command(git push *)",
                    RuleDecision::Deny,
                    RuleScope::Project,
                )
                .unwrap(),
            ],
            ..RuleListing::default()
        };
        let all = rule_rows(&listing, "");
        let groups: Vec<_> = all.iter().map(|row| row.group).collect();
        assert_eq!(
            groups,
            [RuleGroup::Session, RuleGroup::Project, RuleGroup::Deny]
        );
        assert_eq!(all[0].text, "run_command(cargo test *)");
        assert_eq!(all[2].scope(), "project");
        let texts = |q: &str| -> Vec<String> {
            rule_rows(&listing, q)
                .into_iter()
                .map(|row| row.text)
                .collect()
        };
        assert_eq!(texts("CARGO"), ["run_command(cargo test *)"]);
        assert_eq!(texts("deny"), ["run_command(git push *)"]);
        assert_eq!(texts("project").len(), 2, "scope matches too");
        assert!(texts("nothing").is_empty());
    }
}
