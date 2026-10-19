//! The help modal over the real session, with a fake controller whose
//! snapshot has a plugin, its tool and command, an MCP server and a few
//! flags.
//!
//! `cargo run -p ui --example help [--demo]`
//!
//! Press `Esc` then `?` (or `F1` in any mode, or `/help tools`) to open it;
//! `1`–`8` switch sections, `j`/`k` move, `Enter` expands, `y` copies, `/`
//! searches and `Esc` closes.
//!
//! With `--demo` the example types into its own window (through
//! `Window::dispatch_keystroke`, the same path as real keys), logs what the
//! help shows after each step and quits.

#[path = "support/mod.rs"]
mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use application::testing::fs::InMemoryFs;
use application::testing::system::RecordingSink;
use domain::conversation::StopReason;
use domain::model::ModelId;
use domain::permission::PermissionMode;
use domain::workspace::WorkspaceRoot;
use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui::{App, Entity};
use ui::approval::UiApproval;
use ui::help::{
    CliRow, CommandRow, HelpSnapshot, HelpView, McpRow, PathRow, PluginRow, ToolRow, ToolSource,
    help_modal_id,
};
use ui::session::{ModelChoice, Session, SessionController, SessionParts, TurnEnd, TurnRequest};

/// A controller that only answers the help.
struct Fake {
    mode: Mutex<PermissionMode>,
}

impl SessionController for Fake {
    fn run_turn(&self, _request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
        futures::future::ready(TurnEnd::Finished(StopReason::EndTurn)).boxed()
    }

    fn permission_mode(&self) -> PermissionMode {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_permission_mode(&self, mode: PermissionMode) {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner) = mode;
    }

    fn models(&self) -> Vec<ModelChoice> {
        vec![ModelChoice {
            alias: "opus".into(),
            model: ModelId::new("claude-opus-5-5"),
        }]
    }

    fn new_session(&self) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    fn help(&self) -> BoxFuture<'static, HelpSnapshot> {
        futures::future::ready(snapshot()).boxed()
    }
}

fn tool(name: &str, description: &str, source: ToolSource, effect: &str) -> ToolRow {
    ToolRow {
        name: name.into(),
        description: description.into(),
        source,
        effect: effect.into(),
        schema: "{\n  \"type\": \"object\",\n  \"properties\": {\n    \"path\": { \"type\": \"string\" }\n  }\n}".into(),
    }
}

fn snapshot() -> HelpSnapshot {
    HelpSnapshot {
        commands: vec![CommandRow {
            name: "issue".into(),
            usage: "/issue [args]".into(),
            description: "Show a Jira issue".into(),
            source: "jira".into(),
        }],
        tools: vec![
            tool(
                "read_file",
                "Reads a file of the workspace",
                ToolSource::Builtin,
                "read",
            ),
            tool(
                "run_command",
                "Runs a command",
                ToolSource::Builtin,
                "depends on input",
            ),
            tool(
                "jira_issue",
                "Fetches a Jira issue",
                ToolSource::Plugin("jira".into()),
                "read",
            ),
            tool(
                "mcp__echo__echo",
                "Echoes its input",
                ToolSource::Mcp("echo".into()),
                "execute",
            ),
        ],
        plugins: vec![PluginRow {
            name: "jira".into(),
            path: "~/.config/crest/plugins/jira/init.lua".into(),
            layer: "global".into(),
            adds: vec!["tool jira_issue".into(), "command :issue".into()],
            capabilities: vec!["network:jira.example.com".into()],
            trust: "pending".into(),
            approve: Some("/plugin trust jira".into()),
            diagnostics: Vec::new(),
        }],
        mcp: vec![McpRow {
            name: "echo".into(),
            transport: "stdio: echo_server".into(),
            origin: "global".into(),
            state: "ready (2 tools)".into(),
            trust_command: None,
        }],
        cli: vec![CliRow {
            flag: "--model <ALIAS|ID>".into(),
            description: "Model for the turn".into(),
        }],
        paths: vec![PathRow {
            label: "workspace".into(),
            value: "/workspace".into(),
        }],
        ..HelpSnapshot::default()
    }
}

fn main() {
    let demo = std::env::args().any(|arg| arg == "--demo");
    support::run("crest — help", move |window, cx| {
        let root = WorkspaceRoot::new("/workspace").unwrap_or_else(|err| panic!("{err}"));
        let fs = Arc::new(InMemoryFs::new(root).with_file("README.md", "# demo\n"));
        let (_sink, events) = RecordingSink::with_channel();
        let (_approval, approvals) = UiApproval::channel();
        let controller = Arc::new(Fake {
            mode: Mutex::new(PermissionMode::Ask),
        });
        let mut parts = SessionParts::new(controller, events, approvals, fs.clone(), fs);
        parts.workspace_label = "help".into();
        let (root, session) = ui::session::open(parts, window, cx);
        if demo {
            run_demo(window.window_handle(), session, cx);
        }
        root.into()
    });
}

/// Keys typed by `--demo`, each followed by a pause and a log line.
const DEMO_STEPS: &[(&str, u64)] = &[
    ("f1", 400),
    ("3", 300),
    ("j j enter", 300),
    ("y", 300),
    ("4", 300),
    ("/ e c h o", 400),
    ("enter", 300),
    ("escape", 300),
    ("ctrl-space h", 400),
    ("escape", 300),
];

fn run_demo(window: gpui::AnyWindowHandle, session: Entity<Session>, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(500))
            .await;
        for (keys, pause) in DEMO_STEPS {
            for key in keys.split(' ') {
                let Ok(keystroke) = gpui::Keystroke::parse(key) else {
                    continue;
                };
                window
                    .update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))
                    .ok();
            }
            cx.background_executor()
                .timer(Duration::from_millis(*pause))
                .await;
            cx.update(|cx| {
                let modal = session.read(cx).modal().read(cx);
                let view = modal
                    .view(&help_modal_id())
                    .and_then(|view| view.clone().downcast::<HelpView>().ok());
                let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
                match view {
                    Some(view) => {
                        let view = view.read(cx);
                        let rows: Vec<String> =
                            view.rows().iter().map(|row| row.primary.clone()).collect();
                        let bounds = modal
                            .spec(&help_modal_id())
                            .map(|spec| format!("{}×{}%", spec.width.get(), spec.height.get()));
                        eprintln!(
                            "demo [{keys}]: help open {bounds:?} section={:?} selected={} \
                             query={:?} rows={rows:?} clipboard={clipboard:?}",
                            view.section(),
                            view.selected(),
                            view.query(cx),
                        );
                    }
                    None => eprintln!(
                        "demo [{keys}]: help closed; prompt={:?}",
                        session.read(cx).input().read(cx).text(cx)
                    ),
                }
            });
        }
        eprintln!("demo: done");
        cx.update(|cx| cx.quit());
    })
    .detach();
}
