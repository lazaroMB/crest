//! The right pane of the manager for one server: its head with the switch,
//! the banner of its state, the actions, the last outcome and the
//! Overview, Tools and Log tabs.

use std::collections::BTreeMap;

use application_core::ports::mcp::McpServerState;
use application_core::ports::plugin::{ConfigOrigin, McpSource, McpTransport};
use application_core::usecases::mcp_servers::{McpAction, McpServerInfo, McpToolInfo};
use gpui::{
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _,
    px,
};
use gpui_component::input::Input;
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};

use super::model::{McpTab, ago, is_secret, mask, state_text, tone};
use super::view::{McpView, dot, key_hint, tone_color};
use crate::activity::SPINNER;
use crate::kit::{
    ChipKind, Radius, Space, TokenStyled as _, caption, chip, danger_button, quiet_button, title,
    tool_button, toolbar,
};

/// Width of a field's label in the Overview, in pixels.
const LABEL_PX: f32 = 110.0;

impl McpView {
    /// The detail pane of `server`.
    pub(super) fn render_detail(
        &self,
        server: &McpServerInfo,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let body = match self.tab {
            McpTab::Overview => self.render_overview(server, cx),
            McpTab::Tools => self.render_tools(server, cx),
            McpTab::Log => render_log(server, cx),
        };
        v_flex()
            .debug_selector(|| "mcp-detail".to_owned())
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_head(server, cx))
            .children(self.render_banner(server, cx))
            .child(
                if self.confirm_delete.as_deref() == Some(server.spec.name.as_str()) {
                    self.render_confirm(server, cx).into_any_element()
                } else {
                    self.render_actions(server, cx).into_any_element()
                },
            )
            .children(self.render_result(&server.spec.name, cx))
            .child(self.render_tabs(server, cx))
            .child(
                crate::keyscroll::list("mcp-body", &self.body_scroll, vec![body])
                    .flex_1()
                    .w_full()
                    .p_3(),
            )
    }

    fn render_head(&self, server: &McpServerInfo, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let state_color = tone_color(tone(&server.state), cx);
        let transport = match &server.spec.transport {
            McpTransport::Stdio { .. } => "stdio",
            McpTransport::Http { .. } => "http",
        };
        let origin = match server.spec.origin {
            ConfigOrigin::Global => "global",
            ConfigOrigin::Workspace => "workspace",
        };
        let enabled = server.enabled();
        let locked = self.pending.contains_key(&server.spec.name)
            || server.state == McpServerState::Untrusted;
        // The name gives way first; the chips and the switch wrap below it
        // rather than leave the card.
        h_flex()
            .debug_selector(|| "mcp-detail-head".to_owned())
            .flex_wrap()
            .gap_x_2()
            .gap_y_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(120.))
                    .gap_2()
                    .child(dot(&server.state, cx))
                    .child(title(server.spec.name.clone()))
                    .child(
                        caption(state_text(&server.state), cx)
                            .text_color(state_color)
                            .flex_none(),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(chip(transport, muted, ChipKind::Outline, cx))
                    .child(chip(origin, muted, ChipKind::Outline, cx))
                    .when(server.spec.source == McpSource::Lua, |head| {
                        head.child(chip("init.lua", theme.info, ChipKind::Soft, cx))
                    })
                    .child(
                        Switch::new("mcp-enabled")
                            .checked(enabled)
                            .disabled(locked)
                            .label(if enabled { "Enabled" } else { "Disabled" })
                            .on_click(cx.listener(|this, _: &bool, _, cx| this.toggle_enabled(cx))),
                    ),
            )
    }

    /// What the state asks of the user, when it asks something.
    fn render_banner(&self, server: &McpServerInfo, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let name = server.spec.name.clone();
        let pending = self.pending.get(&name).copied();
        let spinner = SPINNER[self.frame % SPINNER.len()];
        let banner = |tint: Hsla, cx: &Context<Self>| {
            h_flex()
                .debug_selector(|| "mcp-banner".to_owned())
                .mx_3()
                .mt_2()
                .tk_p(Space::Md, cx)
                .tk_gap(Space::Md, cx)
                .tk_rounded(Radius::Md, cx)
                .border_1()
                .border_color(tint)
                .bg(tint.opacity(0.08))
                .items_start()
        };
        let text = |strong: &str, note: String, cx: &Context<Self>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(SharedString::from(strong.to_owned())),
                )
                .when(!note.is_empty(), |text| text.child(caption(note, cx)))
        };
        if pending == Some(McpAction::Login) {
            let url = match &server.spec.transport {
                McpTransport::Http { url, .. } => url.clone(),
                McpTransport::Stdio { .. } => String::new(),
            };
            let cancel = name.clone();
            return Some(
                banner(theme.info, cx)
                    .debug_selector(|| "mcp-banner-login".to_owned())
                    .child(div().child(spinner))
                    .child(text(
                        "Waiting for the browser…",
                        format!("crest opened the login page of {url}; finish the login there"),
                        cx,
                    ))
                    .child(
                        quiet_button("mcp-login-cancel", "Cancel", cx).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.pending.remove(&cancel);
                                this.act(&cancel, McpAction::CancelLogin, cx);
                            },
                        )),
                    )
                    .into_any_element(),
            );
        }
        let element = match &server.state {
            McpServerState::Starting => {
                banner(theme.info, cx)
                    .child(div().child(spinner))
                    .child(text(
                        "Starting…",
                        "its tools appear when it is ready".to_owned(),
                        cx,
                    ))
            }
            McpServerState::NeedsLogin { message } => {
                let login = name.clone();
                let note = if server.is_http() && !server.has_oauth() {
                    format!(
                        "{message}. Log in registers crest with the server on its own; no \
                         client to set up."
                    )
                } else {
                    message.clone()
                };
                banner(theme.warning, cx)
                    .child(text("Needs a login", note, cx))
                    .child(tool_button("mcp-login", "Log in", cx).on_click(
                        cx.listener(move |this, _, _, cx| this.act(&login, McpAction::Login, cx)),
                    ))
                    .child(key_hint("l", cx))
            }
            McpServerState::Untrusted => {
                let trust = name.clone();
                let what = match &server.spec.transport {
                    McpTransport::Stdio { command, args, .. } => {
                        let mut line = command.clone();
                        for arg in args {
                            line.push(' ');
                            line.push_str(arg);
                        }
                        line
                    }
                    McpTransport::Http { url, .. } => url.clone(),
                };
                banner(theme.warning, cx)
                    .flex_col()
                    .items_start()
                    .child(text(
                        "Not trusted yet",
                        "it comes with the workspace: starting it runs what whoever wrote the \
                         repository chose"
                            .to_owned(),
                        cx,
                    ))
                    .child(pre("mcp-untrusted-what", what, cx))
                    .child(tool_button("mcp-trust", "Trust and start", cx).on_click(
                        cx.listener(move |this, _, _, cx| this.act(&trust, McpAction::Trust, cx)),
                    ))
            }
            McpServerState::Failed { message } => {
                let retry = name.clone();
                let edit = name.clone();
                let first = message.lines().next().unwrap_or_default().to_owned();
                let mut element = banner(theme.danger, cx)
                    .flex_col()
                    .items_start()
                    .child(text("Failed", first, cx));
                if !server.stderr.is_empty() {
                    element = element.child(pre("mcp-stderr", server.stderr.join("\n"), cx));
                }
                let mut actions =
                    toolbar(cx).child(tool_button("mcp-retry", "Retry", cx).on_click(
                        cx.listener(move |this, _, _, cx| this.act(&retry, McpAction::Restart, cx)),
                    ));
                if server.editable() {
                    actions =
                        actions.child(quiet_button("mcp-edit-config", "Edit config", cx).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.start_form(Some(edit.clone()), window, cx);
                            }),
                        ));
                }
                element.child(actions)
            }
            McpServerState::Disabled => banner(theme.muted_foreground, cx).child(caption(
                "Disabled — the model doesn't see its tools. Turn it on to start it.",
                cx,
            )),
            McpServerState::Ready { .. } | McpServerState::Stopped => return None,
        };
        Some(element.into_any_element())
    }

    fn render_actions(&self, server: &McpServerInfo, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = server.spec.name.clone();
        let pending = self.pending.get(&name).copied();
        let busy = pending.is_some() || server.state == McpServerState::Starting;
        let off = matches!(
            server.state,
            McpServerState::Disabled | McpServerState::Untrusted
        );
        let editable = server.editable();
        let action = |id: &'static str,
                      label: &'static str,
                      action: McpAction,
                      on: bool,
                      cx: &Context<Self>| {
            let target = name.clone();
            let button = quiet_button(id, label, cx);
            if on {
                button
                    .on_click(cx.listener(move |this, _, _, cx| this.act(&target, action, cx)))
                    .into_any_element()
            } else {
                button.opacity(0.5).into_any_element()
            }
        };
        let testing = pending == Some(McpAction::Test);
        let mut left = vec![
            with_hint(
                action(
                    "mcp-test",
                    if testing { "Testing…" } else { "Test" },
                    McpAction::Test,
                    !busy && !off,
                    cx,
                ),
                Some("t"),
                cx,
            ),
            with_hint(
                action(
                    "mcp-restart",
                    "Restart",
                    McpAction::Restart,
                    !busy && !off,
                    cx,
                ),
                Some("r"),
                cx,
            ),
        ];
        if server.is_http() && server.logged_in == Some(true) {
            left.push(with_hint(
                action(
                    "mcp-login-again",
                    "Log in again",
                    McpAction::Login,
                    !busy && !off,
                    cx,
                ),
                Some("l"),
                cx,
            ));
            left.push(with_hint(
                action(
                    "mcp-logout",
                    "Log out",
                    McpAction::Logout,
                    !busy && !off,
                    cx,
                ),
                None,
                cx,
            ));
        }
        let edit = name.clone();
        let edit_button = quiet_button("mcp-edit", "Edit", cx);
        let delete_button = quiet_button("mcp-delete", "Delete", cx).text_color(theme.danger);
        let right = vec![
            with_hint(
                if editable {
                    edit_button
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_form(Some(edit.clone()), window, cx);
                        }))
                        .into_any_element()
                } else {
                    edit_button.opacity(0.5).into_any_element()
                },
                Some("e"),
                cx,
            ),
            with_hint(
                if editable && pending.is_none() {
                    delete_button
                        .on_click(cx.listener(|this, _, _, cx| this.ask_delete(cx)))
                        .into_any_element()
                } else {
                    delete_button.opacity(0.5).into_any_element()
                },
                Some("d"),
                cx,
            ),
        ];
        // Each button keeps its key; the groups wrap onto a second line
        // rather than push Edit and Delete out of the card.
        v_flex()
            .debug_selector(|| "mcp-actions".to_owned())
            .gap_1()
            .px_3()
            .py_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_1()
                    .child(toolbar(cx).flex_wrap().gap_y_1().children(left))
                    .child(div().flex_1())
                    .child(toolbar(cx).flex_wrap().gap_y_1().children(right)),
            )
            .when(!editable, |bar| {
                bar.child(caption("declared in init.lua — edit it there", cx))
            })
    }

    /// The question of `d`: a danger banner in place of the actions. The
    /// text wraps (`min_w_0`: a flex item of text is otherwise as wide as
    /// its unwrapped line) and the buttons wrap below it, so both stay in
    /// the card however narrow it is.
    fn render_confirm(&self, server: &McpServerInfo, cx: &Context<Self>) -> impl IntoElement {
        let tint = cx.theme().danger;
        v_flex()
            .debug_selector(|| "mcp-confirm".to_owned())
            .mx_3()
            .my_2()
            .tk_p(Space::Md, cx)
            .tk_gap(Space::Md, cx)
            .tk_rounded(Radius::Md, cx)
            .border_1()
            .border_color(tint)
            .bg(tint.opacity(0.08))
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .min_w_0()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(SharedString::from(format!("Delete {}?", server.spec.name))),
                    )
                    .child(
                        caption(
                            "Its tools are gone from every chat. It is removed from:",
                            cx,
                        )
                        .min_w_0(),
                    )
                    .child(div().min_w_0().child(mono(server.saved_in.clone(), cx))),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_1()
                    .child(with_hint(
                        danger_button("mcp-confirm-delete", "Delete", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx)))
                            .into_any_element(),
                        Some("↵"),
                        cx,
                    ))
                    .child(with_hint(
                        quiet_button("mcp-confirm-cancel", "Cancel", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.keep_server(cx)))
                            .into_any_element(),
                        Some("Esc"),
                        cx,
                    )),
            )
    }

    fn render_result(&self, name: &str, cx: &Context<Self>) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let (result, at) = self.results.get(name)?;
        let (mark, color, text) = match result {
            Ok(text) => ("✓", theme.success, text.clone()),
            Err(text) => ("✗", theme.danger, text.clone()),
        };
        Some(
            h_flex()
                .debug_selector(|| "mcp-result".to_owned())
                .gap_2()
                .px_3()
                .pb_1()
                .text_sm()
                .child(div().text_color(color).child(mark))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_clamp(2)
                        .child(SharedString::from(text)),
                )
                .child(caption(ago(*at), cx).flex_none()),
        )
    }

    fn render_tabs(&self, server: &McpServerInfo, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (accent, muted) = (theme.primary, theme.muted_foreground);
        let tabs = McpTab::ALL.into_iter().enumerate().map(|(ix, tab)| {
            let label = match tab {
                McpTab::Tools if !server.tools.is_empty() => {
                    format!("Tools {}", server.tools.len())
                }
                other => other.title().to_owned(),
            };
            let on = tab == self.tab;
            let key = tab.key();
            div()
                .id(("mcp-tab", ix))
                .debug_selector(move || format!("mcp-tab-{key}"))
                .px_2()
                .py_1()
                .cursor_pointer()
                .border_b_2()
                .border_color(if on {
                    accent
                } else {
                    gpui::transparent_black()
                })
                .text_color(if on { theme.foreground } else { muted })
                .child(SharedString::from(label))
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
        });
        h_flex()
            .debug_selector(|| "mcp-tabs".to_owned())
            .gap_1()
            .px_3()
            .border_b_1()
            .border_color(theme.border)
            .children(tabs)
    }

    /// `key = value` rows of an environment or of headers, secrets masked.
    fn render_pairs(
        &self,
        server: &str,
        rows: &BTreeMap<String, String>,
        cx: &Context<Self>,
    ) -> AnyElement {
        if rows.is_empty() {
            return caption("none", cx).into_any_element();
        }
        let accent = cx.theme().primary;
        v_flex()
            .gap_0p5()
            .children(rows.iter().enumerate().map(|(ix, (key, value))| {
                let id = format!("{server}/{key}");
                let secret = is_secret(key, value);
                let shown = !secret || self.revealed.contains(&id);
                let text = if shown { value.clone() } else { mask(value) };
                h_flex()
                    .gap_1()
                    .items_start()
                    .child(mono(key.clone(), cx).flex_none())
                    .child(caption("=", cx).flex_none())
                    .child(div().flex_1().min_w_0().child(SharedString::from(text)))
                    .when(secret, |row| {
                        row.child(
                            div()
                                .id(("mcp-reveal", ix))
                                .cursor_pointer()
                                .flex_none()
                                .text_xs()
                                .text_color(accent)
                                .child(if shown { "hide" } else { "show" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.reveal(id.clone(), cx);
                                })),
                        )
                    })
            }))
            .into_any_element()
    }

    fn render_overview(&self, server: &McpServerInfo, cx: &Context<Self>) -> AnyElement {
        let name = server.spec.name.as_str();
        let mut fields: Vec<AnyElement> = Vec::new();
        match &server.spec.transport {
            McpTransport::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                let mut line = command.clone();
                for arg in args {
                    line.push(' ');
                    line.push_str(arg);
                }
                fields.push(field("Command", mono(line, cx), cx));
                fields.push(field("Environment", self.render_pairs(name, env, cx), cx));
                fields.push(field(
                    "Working dir",
                    match cwd {
                        Some(cwd) => div()
                            .child(SharedString::from(cwd.clone()))
                            .into_any_element(),
                        None => caption("the workspace", cx).into_any_element(),
                    },
                    cx,
                ));
            }
            McpTransport::Http {
                url,
                headers,
                oauth,
            } => {
                fields.push(field("URL", mono(url.clone(), cx), cx));
                fields.push(field("Headers", self.render_pairs(name, headers, cx), cx));
                let oauth_text = match oauth {
                    Some(client) => {
                        let mut parts = vec![if client.client_id.is_empty() {
                            "dynamic registration".to_owned()
                        } else {
                            format!("client {}", client.client_id)
                        }];
                        if !client.scopes.is_empty() {
                            parts.push(format!("scopes {}", client.scopes.join(" ")));
                        }
                        parts.push(match server.logged_in {
                            Some(true) => "token in the keyring".to_owned(),
                            _ => "not logged in".to_owned(),
                        });
                        parts.join(" · ")
                    }
                    // A login still works: crest registers itself.
                    None => match server.logged_in {
                        Some(true) => "dynamic registration · token in the keyring".to_owned(),
                        _ => "none set; a login registers crest on its own".to_owned(),
                    },
                };
                fields.push(field(
                    "OAuth",
                    div().child(SharedString::from(oauth_text)),
                    cx,
                ));
            }
        }
        fields.push(field(
            "Effect",
            h_flex()
                .gap_2()
                .child(SharedString::from(
                    format!("{:?}", server.spec.effect).to_lowercase(),
                ))
                .child(caption("how the permission rules treat its tools", cx)),
            cx,
        ));
        fields.push(field(
            "Timeouts",
            div().child(SharedString::from(format!(
                "{} s per call · {} s to start",
                server.timeout_s(),
                server.startup_timeout_s()
            ))),
            cx,
        ));
        fields.push(field(
            "Tool names",
            mono(format!("mcp__{name}__<tool>"), cx),
            cx,
        ));
        fields.push(field(
            "Saved in",
            h_flex()
                .gap_2()
                .child(mono(server.saved_in.clone(), cx))
                .when(!server.editable(), |row| {
                    row.child(caption("read-only here", cx))
                }),
            cx,
        ));
        v_flex()
            .debug_selector(|| "mcp-overview".to_owned())
            .gap_2()
            .children(fields)
            .into_any_element()
    }

    /// The tools of the chosen server that match the tools filter.
    pub(super) fn visible_tools(&self, cx: &App) -> Vec<McpToolInfo> {
        let query = self.tool_query(cx).trim().to_lowercase();
        self.selected_server()
            .map(|server| {
                server
                    .tools
                    .iter()
                    .filter(|t| {
                        query.is_empty()
                            || t.name.to_lowercase().contains(&query)
                            || t.description.to_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn render_tools(&self, server: &McpServerInfo, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        if !matches!(server.state, McpServerState::Ready { .. }) {
            return div()
                .debug_selector(|| "mcp-tools-empty".to_owned())
                .text_color(muted)
                .child("The tools show once the server is ready.")
                .into_any_element();
        }
        let tools = self.visible_tools(cx);
        let prefix = format!("mcp__{}__", server.spec.name);
        let rows = tools.iter().enumerate().map(|(ix, tool)| {
            let open = self.open_tool.as_deref() == Some(tool.name.as_str());
            let name = tool.name.clone();
            let params = open.then(|| {
                let rows: Vec<AnyElement> = if tool.params.is_empty() {
                    vec![caption("no arguments", cx).into_any_element()]
                } else {
                    tool.params
                        .iter()
                        .map(|param| {
                            h_flex()
                                .debug_selector(|| "mcp-param".to_owned())
                                .gap_2()
                                .items_baseline()
                                .child(mono(
                                    format!(
                                        "{}{}",
                                        param.name,
                                        if param.required { "" } else { "?" }
                                    ),
                                    cx,
                                ))
                                .child(
                                    chip(param.kind.clone(), muted, ChipKind::Outline, cx)
                                        .flex_none(),
                                )
                                .child(caption(param.description.clone(), cx).flex_1().min_w_0())
                                .into_any_element()
                        })
                        .collect()
                };
                v_flex().gap_0p5().pl_6().pb_1().children(rows)
            });
            let summary = tool
                .description
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            v_flex()
                .child(
                    h_flex()
                        .id(("mcp-tool", ix))
                        .debug_selector(move || format!("mcp-tool-{ix}"))
                        .gap_2()
                        .py_0p5()
                        .cursor_pointer()
                        .child(div().w(px(12.)).text_color(muted).child(if open {
                            "▾"
                        } else {
                            "▸"
                        }))
                        .child(
                            h_flex()
                                .child(caption(prefix.clone(), cx))
                                .child(SharedString::from(tool.name.clone())),
                        )
                        .child(
                            caption(summary, cx)
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_tool(&name, cx))),
                )
                .children(params)
        });
        v_flex()
            .debug_selector(|| "mcp-tools".to_owned())
            .gap_1()
            .child(Input::new(&self.tool_filter))
            .children(rows)
            .when(tools.is_empty(), |list| {
                list.child(caption("No tool matches", cx))
            })
            .into_any_element()
    }
}

fn render_log(server: &McpServerInfo, cx: &Context<McpView>) -> AnyElement {
    let theme = cx.theme();
    if server.log.is_empty() {
        return div()
            .debug_selector(|| "mcp-log-empty".to_owned())
            .text_color(theme.muted_foreground)
            .child("Nothing yet.")
            .into_any_element();
    }
    v_flex()
        .debug_selector(|| "mcp-log".to_owned())
        .gap_0p5()
        .children(server.log.iter().map(|entry| {
            let color = crate::status::notice_color(entry.level, cx);
            let level = format!("{:?}", entry.level).to_lowercase();
            h_flex()
                .gap_3()
                .items_baseline()
                .child(caption(ago(entry.at), cx).w(px(80.)).flex_none())
                .child(chip(level, color, ChipKind::Soft, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(SharedString::from(entry.text.clone())),
                )
        }))
        .into_any_element()
}

/// A button and the key that does the same, kept on one line.
fn with_hint(button: AnyElement, key: Option<&'static str>, cx: &Context<McpView>) -> AnyElement {
    h_flex()
        .flex_none()
        .child(button)
        .children(key.map(|key| key_hint(key, cx)))
        .into_any_element()
}

/// A labelled value of the Overview.
fn field(label: &'static str, value: impl IntoElement, cx: &Context<McpView>) -> AnyElement {
    h_flex()
        .gap_3()
        .items_start()
        .child(caption(label, cx).w(px(LABEL_PX)).flex_none())
        .child(div().flex_1().min_w_0().whitespace_normal().child(value))
        .into_any_element()
}

/// A bit of code inline.
pub(super) fn mono(text: String, cx: &Context<McpView>) -> gpui::Div {
    let theme = cx.theme();
    div()
        .px_1()
        .rounded_sm()
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .child(SharedString::from(text))
}

/// Tallest a block of output in a banner grows before it scrolls, so a
/// long stderr cannot push the tabs out of the card.
const PRE_MAX_PX: f32 = 110.0;

/// A block of code or output, `id` for its own scroll.
fn pre(id: &'static str, text: String, cx: &Context<McpView>) -> gpui::Stateful<gpui::Div> {
    let theme = cx.theme();
    div()
        .id(id)
        .max_h(px(PRE_MAX_PX))
        .overflow_y_scroll()
        .w_full()
        .p_2()
        .rounded_sm()
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .whitespace_normal()
        .child(SharedString::from(text))
}
