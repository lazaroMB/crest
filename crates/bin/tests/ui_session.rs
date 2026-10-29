//! The window mode over the real composition root (rg, fzf, LocalFs,
//! SQLite, Lua) with the model replaced by a script: a prompt typed in the
//! input runs a whole turn, the edit is approved in the dialog and the
//! session is saved; `Ctrl+C` interrupts a hanging model.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used, clippy::expect_used)]

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
        trust_plugins: Vec::new(),
        resume: false,
        session_id: None,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let parts = session_parts(&config, overrides, runtime.handle()).unwrap();
    // The pickers' preview reads through the app's file system, on the
    // runtime, from any executor.
    let fs = parts.fs.clone().expect("the previews have a file system");
    let root = domain::workspace::WorkspacePath::root();
    assert_eq!(futures::executor::block_on(fs.exists(&root)), Ok(true));
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
    let root = adapter_fs::guard::canonicalize_blocking(&w.workspace).unwrap();
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

/// The example of the plan: `<leader>x` clears the session and `/review`
/// asks the `plan` role.
const KEYMAPS_INIT: &str = r#"
local app = require("app")
app.keymap.set("<leader>x", function() app.cmd("clear") end)
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

    cx.simulate_keystrokes("ctrl-g x");
    wait_until(&w, cx, "<leader>x to clear the session", |s, cx| {
        s.timeline().read(cx).state().is_empty()
    });

    cx.simulate_input("/review");
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

#[cfg(unix)]
fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A git repository in the workspace with the Spec `0001-suma` and its
/// one-task plan, committed on `main`. Its check runs `grep`: Unix only.
#[cfg(unix)]
fn spec_repository(workspace: &std::path::Path) {
    let spec = workspace.join(".crest/specs/0001-suma");
    std::fs::create_dir_all(&spec).unwrap();
    std::fs::write(spec.join("spec.md"), "# Suma\n").unwrap();
    let plan = json!({
        "spec": "0001-suma",
        "tasks": [{
            "id": "T1",
            "title": "Corregir add",
            "description": "`add` resta.",
            "depends_on": [],
            "files": ["src/lib.rs"],
            "acceptance": [{ "kind": "test", "ref": "grep -q 'a + b' src/lib.rs" }],
            "risk": "low"
        }]
    });
    std::fs::write(spec.join("plan.json"), plan.to_string()).unwrap();
    git(workspace, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "crest test"),
        ("user.email", "crest@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(workspace, &["config", key, value]);
    }
    git(workspace, &["add", "-A"]);
    git(workspace, &["commit", "-q", "-m", "inicial"]);
}

fn has_text(w: &Window_, cx: &mut VisualTestContext, needle: &str) -> bool {
    texts(&blocks(w, cx)).iter().any(|t| t.contains(needle))
}

#[cfg(unix)]
#[gpui::test]
fn implement_runs_the_flow_and_the_chat_answers_its_gates(cx: &mut TestAppContext) {
    let script = vec![
        ScriptedResponse::tool_use(
            "toolu_1",
            "edit_file",
            json!({ "path": "src/lib.rs", "old": "a - b", "new": "a + b" }),
        ),
        ScriptedResponse::text("Corregido.\nSTATUS: done"),
    ];
    let (w, cx) = open(script, cx);
    spec_repository(&w.workspace);

    cx.simulate_input("/implement 0001-suma");
    cx.simulate_keystrokes("enter");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !has_text(&w, cx, "Answer \"yes\" to create the branch") {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the plan gate"
        );
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }

    // The answer goes to the flow, not to the model.
    cx.simulate_input("sí");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the approval of the edit", |s, cx| {
        s.approval().read(cx).current().is_some()
    });
    cx.simulate_keystrokes("y");
    wait_until(&w, cx, "the report gate", |s, _| !s.is_running());
    assert!(
        has_text(&w, cx, "report.md"),
        "{:?}",
        texts(&blocks(&w, cx))
    );

    cx.simulate_input("sí");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the end of the flow", |s, _| !s.is_running());

    assert_eq!(w.llm.remaining(), 0);
    let log = git(
        &w.workspace,
        &["log", "--format=%B", "main..spec/0001-suma"],
    );
    assert!(log.starts_with("fix(src): Corregir add\n"), "{log}");
    assert!(log.contains("Task: T1"), "{log}");
    assert!(
        w.workspace
            .join(".crest/specs/0001-suma/report.md")
            .exists()
    );
    let kinds = blocks(&w, cx);
    assert!(
        texts(&kinds).contains(&"> sí\n".to_owned()),
        "{:?}",
        texts(&kinds)
    );
}

#[gpui::test]
fn research_takes_the_chat_until_it_is_aborted(cx: &mut TestAppContext) {
    let intake = json!({
        "kind": "bug",
        "restated": "`add` resta en vez de sumar.",
        "questions": []
    });
    let script = vec![
        ScriptedResponse::text(intake.to_string()),
        ScriptedResponse::text("Respuesta del chat."),
    ];
    let (w, cx) = open(script, cx);

    cx.simulate_input("/research --deep add(2, 2) devuelve 0");
    cx.simulate_keystrokes("enter");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !has_text(&w, cx, "go on to Observation") {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the Intake gate: {:?}",
            texts(&blocks(&w, cx))
        );
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(has_text(&w, cx, "`add` resta en vez de sumar."));

    // /abort goes to the flow, not to the model.
    cx.simulate_input("/abort");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the abort", |s, _| !s.is_busy());
    assert_eq!(w.llm.requests().len(), 1);

    // Once aborted, the chat answers turns again.
    cx.simulate_input("y ahora?");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the chat turn", |s, _| !s.is_running());
    assert!(
        has_text(&w, cx, "Respuesta del chat."),
        "{:?}",
        texts(&blocks(&w, cx))
    );
    assert_eq!(w.llm.remaining(), 0);
}

fn notice(w: &Window_, cx: &mut VisualTestContext) -> Option<String> {
    w.session.read_with(cx, |s, cx| {
        let now = cx.background_executor().now();
        s.status()
            .read(cx)
            .state()
            .notice(now)
            .map(|n| n.message.clone())
    })
}

fn modal_ids(w: &Window_, cx: &mut VisualTestContext) -> Vec<String> {
    let modal = w.session.read_with(cx, |s, _| s.modal().clone());
    modal.read_with(cx, |m, _| m.ids().into_iter().map(|id| id.0).collect())
}

#[gpui::test]
fn open_shows_workspace_files_and_refuses_the_rest(cx: &mut TestAppContext) {
    let (w, cx) = open(Vec::new(), cx);
    std::fs::write(w.workspace.join("README.md"), "# crest\n").unwrap();
    std::fs::write(w.workspace.join(".env"), "KEY=1\n").unwrap();
    let outside = w.workspace.parent().unwrap().join("outside.txt");
    std::fs::write(&outside, "secret\n").unwrap();
    // Creating symlinks on Windows needs a privilege the CI may not have.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, w.workspace.join("link.txt")).unwrap();

    cx.simulate_input("/open @README.md");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the modal", |s, cx| s.modal().read(cx).is_open());
    assert_eq!(modal_ids(&w, cx), ["open:README.md"]);
    let modal = w.session.read_with(cx, |s, _| s.modal().clone());
    modal.read_with(cx, |m, _| {
        let spec = m
            .spec(&domain::modal::ModalId::new("open:README.md"))
            .unwrap();
        assert_eq!(
            spec.content,
            domain::block::CustomNode::Markdown {
                source: "# crest".into()
            }
        );
    });
    assert!(blocks(&w, cx).is_empty(), "nothing goes to the timeline");

    for (line, needle) in [
        ("/open @link.txt", "outside the workspace"),
        ("/open @../outside.txt", "outside the workspace"),
        ("/open @.env", "sensitive path pattern"),
        ("/open @nope.md", "does not exist"),
        ("/open @src", "is a directory"),
        ("/open README.md", "usage: /open @<path>"),
    ]
    .into_iter()
    .filter(|(line, _)| cfg!(unix) || !line.contains("link.txt"))
    {
        // The open modal has the focus: run the line as the input would.
        let command = line.strip_prefix('/').unwrap();
        w.session
            .update_in(cx, |s, window, cx| s.command(command, window, cx));
        wait_until(&w, cx, line, |s, cx| {
            let now = cx.background_executor().now();
            s.status()
                .read(cx)
                .state()
                .notice(now)
                .is_some_and(|n| n.message.contains(needle))
        });
        assert!(notice(&w, cx).unwrap().contains(needle), "{line}");
    }
    assert_eq!(modal_ids(&w, cx), ["open:README.md"]);
    assert!(w.llm.requests().is_empty());
}

#[gpui::test]
fn chat_only_images_are_saved_without_a_model_call(cx: &mut TestAppContext) {
    let (w, cx) = open(Vec::new(), cx);
    // A 1×1 transparent PNG.
    let png: Vec<u8> = vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 96, 248, 95, 15,
        0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let input = w.session.read_with(cx, |s, _| s.input().clone());
    input.update(cx, |input, cx| {
        input.attach_images(
            vec![gpui::Image::from_bytes(gpui::ImageFormat::Png, png)],
            cx,
        );
        input.toggle_image_mode(cx);
    });
    cx.simulate_input("the screenshot");
    cx.simulate_keystrokes("enter");
    wait_until(&w, cx, "the saved notice", |s, cx| {
        let now = cx.background_executor().now();
        s.status()
            .read(cx)
            .state()
            .notice(now)
            .is_some_and(|n| n.message.contains("not sent to the model"))
    });
    assert!(w.llm.requests().is_empty());
    let store = SqliteSessionStore::open(&w.db).unwrap();
    let root = adapter_fs::guard::canonicalize_blocking(&w.workspace).unwrap();
    let saved = w
        .runtime
        .block_on(store.latest_for_workspace(&root.display().to_string()))
        .unwrap()
        .unwrap();
    assert!(saved.session.turns.is_empty());
    let kinds: Vec<_> = saved.timeline.into_iter().map(|b| b.kind).collect();
    assert_eq!(texts(&kinds), ["> the screenshot\n"]);
    assert!(kinds.iter().any(|kind| matches!(
        kind,
        BlockKind::Image { media_type, .. } if media_type == "image/png"
    )));
}

/// Hosts a project workbench as the window's root.
struct WorkbenchHost(Entity<ui::workbench::Workbench>);

impl Render for WorkbenchHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

#[gpui::test]
fn the_picker_opens_a_project_and_slash_project_goes_back(cx: &mut TestAppContext) {
    use clap::Parser as _;

    cx.executor().allow_parking();
    cx.update(gpui_component::init);
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(project.join("README.md"), "# proj\n").unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let overrides = Overrides {
        llm: Some(Arc::new(ScriptedLlm::new(Vec::new()))),
        config_sources: Some(ConfigSources::default()),
        db_path: Some(dir.path().join("sessions.db")),
        ..Overrides::default()
    };
    let args = crest::args::Args::try_parse_from(["crest", "--pick"]).unwrap();
    let launcher = Arc::new(crest::gui::Launcher::new(
        args,
        overrides,
        runtime.handle().clone(),
    ));
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let workbench = ui::workbench::Workbench::new_picker(launcher, window, cx);
        *out.borrow_mut() = Some(workbench.clone());
        WorkbenchHost(workbench)
    });
    cx.run_until_parked();
    let workbench = slot.borrow_mut().take().unwrap();
    workbench.read_with(cx, |w, _| assert!(w.project_picker().is_some()));

    // The picker lists the real disk through the port.
    let picker = workbench.read_with(cx, |w, _| w.project_picker().unwrap().clone());
    picker.update_in(cx, |p, window, cx| p.show_dir(dir.path(), window, cx));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        let names: Vec<String> = picker.read_with(cx, |p, _| {
            p.listing()
                .map(|l| l.dirs.iter().map(|d| d.name.clone()).collect())
                .unwrap_or_default()
        });
        if names == ["proj"] {
            break;
        }
        assert!(Instant::now() < deadline, "listing never came: {names:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
    cx.simulate_keystrokes("enter");

    let deadline = Instant::now() + Duration::from_secs(20);
    let session = loop {
        cx.run_until_parked();
        if let Some(session) = workbench.read_with(cx, |w, _| w.session().cloned()) {
            break session;
        }
        assert!(Instant::now() < deadline, "the project never opened");
        std::thread::sleep(Duration::from_millis(10));
    };
    let weak = session.downgrade();
    let to_picker = |cx: &mut gpui::VisualTestContext| {
        let session = weak.upgrade().expect("the session is alive");
        session.update_in(cx, |s, window, cx| s.command("project", window, cx));
        cx.run_until_parked();
        workbench.read_with(cx, |w, cx| {
            let picker = w.project_picker().expect("back to the picker");
            let shown = adapter_fs::guard::canonicalize_blocking(dir.path()).unwrap();
            let expected = format!("{}{}", shown.display(), std::path::MAIN_SEPARATOR);
            assert_eq!(picker.read(cx).text(cx), expected);
        });
    };
    drop(session);
    to_picker(cx);
    assert!(
        weak.upgrade().is_some(),
        "the session is kept until another project is chosen"
    );

    // Esc returns to it.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    workbench.read_with(cx, |w, _| {
        let shown = w.session().expect("the session is back");
        assert!(Some(shown.clone()) == weak.upgrade(), "the same session");
    });

    // Choosing a project drops it.
    to_picker(cx);
    workbench.update_in(cx, |w, window, cx| w.open(project.clone(), window, cx));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if workbench.read_with(cx, |w, _| w.session().is_some()) {
            break;
        }
        assert!(Instant::now() < deadline, "the project never opened");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(weak.upgrade().is_none(), "the session was dropped");
}
