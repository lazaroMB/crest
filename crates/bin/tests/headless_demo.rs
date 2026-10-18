//! The phase 3 demo, without network: the real composition root (rg, fzf,
//! LocalFs, LocalShell, SQLite, Lua) with the model replaced by a script.
//! The agent finds a function, reads it, edits it after a "y" on the
//! simulated stdin and explains the change.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use adapter_store::SqliteSessionStore;
use application::ports::plugin::ConfigSources;
use application::ports::store::SessionStorePort;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::usecases::cancel::CancelToken;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal, run_turn};
use crest::root::{App, Config, Overrides, build_app};
use domain::block::BlockKind;
use domain::permission::{PermissionMode, RuleDecision, RuleMatcher};
use serde_json::json;

const LIB: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a - b\n}\n";
const EXPLANATION: &str = "Cambié `a - b` por `a + b` en `add`: restaba en vez de sumar.";

/// Captured terminal output.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// find_file + grep → read_file → edit_file → explanation.
fn demo_script() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::text("Busco la función `add`.")
            .then_tool_use("toolu_1", "find_file", json!({ "query": "lib" }))
            .then_tool_use("toolu_2", "grep", json!({ "pattern": "fn add" })),
        ScriptedResponse::tool_use("toolu_3", "read_file", json!({ "path": "src/lib.rs" })),
        ScriptedResponse::tool_use(
            "toolu_4",
            "edit_file",
            json!({ "path": "src/lib.rs", "old": "a - b", "new": "a + b" }),
        ),
        ScriptedResponse::text(EXPLANATION),
    ]
}

struct Demo {
    _dir: tempfile::TempDir,
    workspace: std::path::PathBuf,
    db: std::path::PathBuf,
    llm: Arc<ScriptedLlm>,
    output: Buffer,
    app: App,
}

async fn demo(answers: &str, mode: PermissionMode) -> Demo {
    demo_with(answers, mode, demo_script()).await
}

async fn demo_with(answers: &str, mode: PermissionMode, script: Vec<ScriptedResponse>) -> Demo {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(workspace.join("README.md"), "demo\n").unwrap();
    let db = dir.path().join("sessions.db");

    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    let input = Box::new(Cursor::new(answers.as_bytes().to_vec()));
    let approval = StdinApproval::new(terminal.clone(), input, false);
    let llm = Arc::new(ScriptedLlm::new(script));
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources::default()),
        db_path: Some(db.clone()),
        ..Overrides::default()
    };
    let config = Config {
        workspace: workspace.clone(),
        mode,
        model: None,
    };
    let app = build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap();
    Demo {
        _dir: dir,
        workspace,
        db,
        llm,
        output,
        app,
    }
}

fn lib(workspace: &Path) -> String {
    std::fs::read_to_string(workspace.join("src/lib.rs")).unwrap()
}

#[tokio::test]
async fn finds_edits_after_approval_and_explains() {
    let mut demo = demo("y\n", PermissionMode::Ask).await;
    let exit = run_turn(&mut demo.app, "arregla add", CancelToken::new()).await;
    let out = demo.output.text();
    assert_eq!(exit, Exit::Success, "{out}");

    assert_eq!(lib(&demo.workspace), LIB.replace("a - b", "a + b"));
    assert!(out.contains("→ find_file"), "{out}");
    assert!(out.contains("✓ grep"), "{out}");
    assert!(out.contains("→ read_file src/lib.rs"), "{out}");
    assert!(out.contains("  -    a - b\n  +    a + b\n"), "{out}");
    assert!(
        out.contains("¿Permitir edit_file src/lib.rs? [y]es / [a]lways / [n]o: y\n"),
        "{out}"
    );
    assert!(out.contains("✓ edit_file"), "{out}");
    assert!(out.trim_end().contains(EXPLANATION), "{out}");
    assert!(out.contains("tokens:"), "{out}");
    println!("{out}");

    // The real tools ran: grep found the function in the read file.
    let requests = demo.llm.requests();
    assert_eq!(requests.len(), 4);
    let results = format!("{:?}", requests[1].messages.last().unwrap());
    assert!(results.contains("src/lib.rs"), "{results}");
    let read = format!("{:?}", requests[2].messages.last().unwrap());
    assert!(read.contains("a - b"), "{read}");

    // Every tool is offered and the system prompt names the workspace.
    let names: Vec<_> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    for tool in [
        "find_file",
        "grep",
        "read_file",
        "edit_file",
        "run_command",
        "show_chart",
        "task",
    ] {
        assert!(names.contains(&tool), "{names:?}");
    }
    let root = demo.app.workspace.as_path().display().to_string();
    assert!(requests[0].system[0].text.contains(&root));

    // The session was saved with its timeline.
    let store = SqliteSessionStore::open(&demo.db).unwrap();
    let saved = store.latest_for_workspace(&root).await.unwrap().unwrap();
    assert_eq!(saved.session.turns.len(), 1);
    assert!(
        saved
            .timeline
            .iter()
            .any(|b| matches!(&b.kind, BlockKind::Diff(d) if d.path == "src/lib.rs"))
    );
    assert!(saved.permission_rules.is_empty());
}

#[tokio::test]
async fn always_is_saved_with_the_session() {
    let mut demo = demo("a\n", PermissionMode::Ask).await;
    let exit = run_turn(&mut demo.app, "arregla add", CancelToken::new()).await;
    assert_eq!(exit, Exit::Success, "{}", demo.output.text());
    let root = demo.app.workspace.as_path().display().to_string();
    let store = SqliteSessionStore::open(&demo.db).unwrap();
    let saved = store.latest_for_workspace(&root).await.unwrap().unwrap();
    assert_eq!(saved.permission_rules.len(), 1);
    let rule = &saved.permission_rules[0];
    assert_eq!(rule.tool, "edit_file");
    assert_eq!(rule.matcher, RuleMatcher::Exact("src/lib.rs".into()));
    assert_eq!(rule.decision, RuleDecision::AllowAlways);
}

#[tokio::test]
async fn a_rejected_edit_leaves_the_file_alone() {
    let mut demo = demo("n\n", PermissionMode::Ask).await;
    let exit = run_turn(&mut demo.app, "arregla add", CancelToken::new()).await;
    let out = demo.output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    assert_eq!(lib(&demo.workspace), LIB);
    assert!(out.contains("⊘ edit_file: denied by user"), "{out}");
}

#[tokio::test]
async fn auto_edit_does_not_ask() {
    let mut demo = demo("", PermissionMode::AutoEdit).await;
    let exit = run_turn(&mut demo.app, "arregla add", CancelToken::new()).await;
    let out = demo.output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    assert_eq!(lib(&demo.workspace), LIB.replace("a - b", "a + b"));
    assert!(!out.contains("¿Permitir"), "{out}");
    assert!(out.contains("  +    a + b\n"), "{out}");
}

#[tokio::test]
async fn an_interrupted_turn_exits_with_130() {
    let mut demo = demo("", PermissionMode::Ask).await;
    let cancel = CancelToken::new();
    cancel.cancel();
    let exit = run_turn(&mut demo.app, "arregla add", cancel).await;
    assert_eq!(exit, Exit::Interrupted);
    assert_eq!(lib(&demo.workspace), LIB);
    assert!(demo.output.text().contains("(interrumpido)"));
}

#[tokio::test]
async fn a_failing_model_exits_with_1() {
    // With no scripted answer left the model call fails.
    let mut demo = demo_with("", PermissionMode::Ask, Vec::new()).await;
    let exit = run_turn(&mut demo.app, "hola", CancelToken::new()).await;
    let out = demo.output.text();
    assert_eq!(exit, Exit::TurnFailed, "{out}");
    assert!(out.contains("error:"), "{out}");
}
