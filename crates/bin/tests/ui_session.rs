//! The window mode over the real composition root (rg, fzf, LocalFs,
//! SQLite, Lua) with the model replaced by a script: a prompt typed in the
//! input runs a whole turn, the edit is approved in the dialog and the
//! session is saved; `Ctrl+C` interrupts a hanging model.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use adapter_store::SqliteSessionStore;
use application::ports::plugin::ConfigSources;
use application::ports::store::SessionStorePort;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use crest::gui::session_parts;
use crest::root::{Config, Overrides};
use domain::block::{BlockKind, ToolStatus};
use domain::model::{ModelRole, default_spec};
use domain::permission::PermissionMode;
use gpui::{
    App, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div,
};
use serde_json::json;
use ui::app::RootView;
use ui::session::Session;

const LIB: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a - b\n}\n";
const EXPLANATION: &str = "Cambié `a - b` por `a + b` en `add`.";

struct Host(Entity<RootView>);

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

struct Window_ {
    _dir: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    workspace: std::path::PathBuf,
    db: std::path::PathBuf,
    llm: Arc<ScriptedLlm>,
    session: Entity<Session>,
}

fn open(
    script: Vec<ScriptedResponse>,
    cx: &mut TestAppContext,
) -> (Window_, &mut VisualTestContext) {
    open_with(script, None, cx)
}

/// The window over a new workspace whose `.crest/init.lua` is `init`.
fn open_with<'a>(
    script: Vec<ScriptedResponse>,
    init: Option<&str>,
    cx: &'a mut TestAppContext,
) -> (Window_, &'a mut VisualTestContext) {
    cx.executor().allow_parking();
    cx.update(gpui_component::init);
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/lib.rs"), LIB).unwrap();
    let db = dir.path().join("sessions.db");
    let mut sources = ConfigSources::default();
    if let Some(init) = init {
        std::fs::create_dir_all(workspace.join(".crest")).unwrap();
        let path = workspace.join(".crest/init.lua");
        std::fs::write(&path, init).unwrap();
        sources.workspace = Some(path);
    }
    let llm = Arc::new(ScriptedLlm::new(script));
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(sources),
        db_path: Some(db.clone()),
        ..Overrides::default()
    };
    let config = Config {
        workspace: workspace.clone(),
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let parts = session_parts(&config, overrides, runtime.handle()).unwrap();
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let (root, session) = ui::session::open(parts, window, cx);
        *out.borrow_mut() = Some(session);
        Host(root)
    });
    cx.run_until_parked();
    let session = slot.borrow_mut().take().unwrap();
    let w = Window_ {
        _dir: dir,
        runtime,
        workspace,
        db,
        llm,
        session,
    };
    (w, cx)
}

fn wait_until(
    w: &Window_,
    cx: &mut VisualTestContext,
    what: &str,
    done: impl Fn(&Session, &App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if w.session.read_with(cx, |s, cx| done(s, cx)) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn blocks(w: &Window_, cx: &mut VisualTestContext) -> Vec<BlockKind> {
    w.session.read_with(cx, |s, cx| {
        s.timeline()
            .read(cx)
            .state()
            .entries()
            .iter()
            .map(|e| e.block.kind.clone())
            .collect()
    })
}

fn texts(kinds: &[BlockKind]) -> Vec<String> {
    kinds
        .iter()
        .filter_map(|kind| match kind {
            BlockKind::Text { markdown } => Some(markdown.clone()),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn a_turn_from_the_input_edits_after_approval_and_is_saved(cx: &mut TestAppContext) {
    let script = vec![
        ScriptedResponse::text("Busco `add`.").then_tool_use(
            "toolu_1",
            "grep",
            json!({ "pattern": "fn add" }),
        ),
        ScriptedResponse::tool_use("toolu_2", "read_file", json!({ "path": "src/lib.rs" })),
        ScriptedResponse::tool_use(
            "toolu_3",
            "edit_file",
            json!({ "path": "src/lib.rs", "old": "a - b", "new": "a + b" }),
        ),
        ScriptedResponse::text(EXPLANATION),
    ];
    let (w, cx) = open(script, cx);
    cx.simulate_input("arregla add");
    cx.simulate_keystrokes("enter");

    wait_until(&w, cx, "the approval dialog", |s, cx| {
        s.approval().read(cx).current().is_some()
    });
    let tool = w.session.read_with(cx, |s, cx| {
        s.approval().read(cx).current().map(|r| r.tool.clone())
    });
    assert_eq!(tool.as_deref(), Some("edit_file"));
    cx.simulate_keystrokes("y");
    wait_until(&w, cx, "the end of the turn", |s, _| !s.is_running());

    let fixed = std::fs::read_to_string(w.workspace.join("src/lib.rs")).unwrap();
    assert_eq!(fixed, LIB.replace("a - b", "a + b"));
    let kinds = blocks(&w, cx);
    assert_eq!(
        texts(&kinds),
        ["> arregla add\n", "Busco `add`.", EXPLANATION]
    );
    for name in ["grep", "read_file", "edit_file"] {
        assert!(
            kinds.iter().any(|kind| matches!(
                kind,
                BlockKind::ToolCall { name: n, status: ToolStatus::Succeeded, .. } if n == name
            )),
            "{name}: {kinds:?}"
        );
    }
    assert!(kinds.iter().any(|kind| matches!(kind, BlockKind::Diff(_))));
    assert_eq!(w.llm.remaining(), 0);

    // Saved at the end of the turn, with the prompt in the timeline.
    let store = SqliteSessionStore::open(&w.db).unwrap();
    let root = std::fs::canonicalize(&w.workspace).unwrap();
    let saved = w
        .runtime
        .block_on(store.latest_for_workspace(&root.display().to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(saved.session.turns.len(), 1);
    let saved: Vec<_> = saved.timeline.into_iter().map(|b| b.kind).collect();
    assert_eq!(texts(&saved)[0], "> arregla add\n");
}

#[gpui::test]
fn ctrl_c_interrupts_the_real_agent(cx: &mut TestAppContext) {
    let script = vec![ScriptedResponse::text("hola mundo cruel").hang_after(4)];
    let (w, cx) = open(script, cx);
    cx.simulate_input("saluda");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the partial answer", |s, cx| {
        s.timeline()
            .read(cx)
            .state()
            .entries()
            .iter()
            .any(|e| matches!(&e.block.kind, BlockKind::Text { markdown } if markdown.starts_with("hola")))
    });
    cx.simulate_keystrokes("ctrl-c");
    wait_until(&w, cx, "the interrupted turn", |s, _| !s.is_running());

    w.llm.push(ScriptedResponse::text("de nuevo"));
    cx.simulate_input("otra vez");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the second turn", |s, _| !s.is_running());
    let texts = texts(&blocks(&w, cx));
    assert_eq!(
        texts.last().map(String::as_str),
        Some("de nuevo"),
        "{texts:?}"
    );
}

#[gpui::test]
fn a_broken_init_lua_opens_with_an_error_block(cx: &mut TestAppContext) {
    let (w, cx) = open_with(
        vec![ScriptedResponse::text("hola")],
        Some("local app = require('app')\nesto no es lua\n"),
        cx,
    );
    let path = w.workspace.join(".crest/init.lua");
    let prefix = format!("{}:2:", path.display());
    let kinds = blocks(&w, cx);
    assert!(
        kinds.iter().any(
            |kind| matches!(kind, BlockKind::Error { message, .. } if message.starts_with(&prefix))
        ),
        "{kinds:?}"
    );
    // The agent still answers, with the default model.
    w.session.read_with(cx, |s, cx| {
        let model = s.status().read(cx).state().model().cloned();
        assert_eq!(model, Some(default_spec(ModelRole::Main).id));
    });
    cx.simulate_input("hola");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the turn", |s, _| !s.is_running());
    assert_eq!(
        texts(&blocks(&w, cx)).last().map(String::as_str),
        Some("hola")
    );
}

/// The example of the plan: `<leader>x` clears the session and `:review`
/// asks the `plan` role.
const KEYMAPS_INIT: &str = r#"
local app = require("app")
app.keymap.set("n", "<leader>x", function() app.cmd("clear") end)
app.commands.register("review", function(args)
  app.send("Revisa los cambios sin commitear y lista riesgos", { role = "plan" })
end)
"#;

#[gpui::test]
fn keymaps_and_commands_of_init_lua_run_their_actions(cx: &mut TestAppContext) {
    let script = vec![
        ScriptedResponse::text("hola"),
        ScriptedResponse::text("Sin riesgos."),
    ];
    let (w, cx) = open_with(script, Some(KEYMAPS_INIT), cx);
    assert!(
        !blocks(&w, cx)
            .iter()
            .any(|kind| matches!(kind, BlockKind::Error { .. })),
        "{:?}",
        blocks(&w, cx)
    );
    cx.simulate_input("hola");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the first turn", |s, _| !s.is_running());
    assert!(!texts(&blocks(&w, cx)).is_empty());

    cx.simulate_keystrokes("escape space x");
    wait_until(&w, cx, "<leader>x to clear the session", |s, cx| {
        s.timeline().read(cx).state().is_empty()
    });

    cx.simulate_keystrokes(":");
    cx.simulate_input("review");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the review turn", |s, _| {
        !s.is_running() && w.llm.remaining() == 0
    });
    let request = w.llm.last_request().unwrap();
    assert_eq!(request.model.role, ModelRole::Plan);
    // The new session only sends the review prompt.
    assert_eq!(request.messages.len(), 1, "{:?}", request.messages);
    assert_eq!(
        texts(&blocks(&w, cx)),
        [
            "> Revisa los cambios sin commitear y lista riesgos\n",
            "Sin riesgos."
        ]
    );
}
