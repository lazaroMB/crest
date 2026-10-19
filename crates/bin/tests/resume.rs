//! Closing and reopening crest resumes the latest session of the
//! workspace: the real composition root over the same SQLite file brings
//! back the history (sent whole to the model on the next turn), the
//! timeline and the "allow always" rules.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use application::ports::plugin::ConfigSources;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::usecases::approval::lock_permissions;
use application::usecases::cancel::CancelToken;
use crest::gui::session_parts;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal, run_turn};
use crest::root::{App, Config, Overrides, build_app};
use domain::conversation::Role;
use domain::permission::PermissionMode;
use serde_json::json;

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

struct Ws {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    db: PathBuf,
}

fn workspace() -> Ws {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/lib.rs"), "pub fn add() {}\n").unwrap();
    let db = dir.path().join("sessions.db");
    Ws {
        _dir: dir,
        workspace,
        db,
    }
}

fn config(workspace: &Path, resume: bool) -> Config {
    Config {
        workspace: workspace.to_path_buf(),
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume,
    }
}

fn overrides(ws: &Ws, llm: &Arc<ScriptedLlm>) -> Overrides {
    Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources::default()),
        db_path: Some(ws.db.clone()),
        ..Overrides::default()
    }
}

/// A headless app over `ws` answering approvals with `answers`.
async fn app(ws: &Ws, resume: bool, answers: &str, llm: &Arc<ScriptedLlm>) -> (App, Buffer) {
    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    let input = Box::new(Cursor::new(answers.as_bytes().to_vec()));
    let approval = StdinApproval::new(terminal.clone(), input, false);
    let app = build_app(
        &config(&ws.workspace, resume),
        frontend(&terminal, approval),
        overrides(ws, llm),
    )
    .await
    .unwrap();
    (app, output)
}

#[tokio::test]
async fn a_reopened_app_resumes_history_timeline_and_rules() {
    let ws = workspace();
    let first_llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedResponse::tool_use(
            "toolu_1",
            "edit_file",
            json!({ "path": "src/lib.rs", "old": "add", "new": "sum" }),
        ),
        ScriptedResponse::text("Renombrada."),
    ]));
    let (mut first, _) = app(&ws, true, "a\n", &first_llm).await;
    // What the UI records as the prompt is sent.
    first.prompt_history().push("renombra add".into());
    assert_eq!(
        run_turn(&mut first, "renombra add", CancelToken::new()).await,
        Exit::Success
    );
    let saved = first.stored_session();
    assert_eq!(saved.session.turns.len(), 1);
    assert_eq!(
        saved.permission_rules.len(),
        1,
        "the edit was allowed always"
    );
    drop(first);

    let second_llm = Arc::new(ScriptedLlm::new(vec![ScriptedResponse::text("Hecho.")]));
    let (mut second, output) = app(&ws, true, "", &second_llm).await;
    assert_eq!(second.session, saved.session);
    assert_eq!(second.timeline_blocks(), saved.timeline);
    assert_eq!(*second.prompt_history(), ["renombra add"]);
    assert_eq!(
        lock_permissions(second.permissions()).rules,
        saved.permission_rules
    );
    assert!(
        output.text().contains("previous session resumed"),
        "{}",
        output.text()
    );

    assert_eq!(
        run_turn(&mut second, "y ahora?", CancelToken::new()).await,
        Exit::Success
    );
    // The whole history went to the model: the first turn (prompt, tool
    // use, tool result, answer) and the new prompt.
    let request = second_llm.last_request().unwrap();
    let previous: usize = saved.session.turns.iter().map(|t| t.messages.len()).sum();
    assert_eq!(
        request.messages.len(),
        previous + 1,
        "{:?}",
        request.messages
    );
    assert_eq!(request.messages[0].role, Role::User);
    assert!(format!("{:?}", request.messages[0]).contains("renombra add"));
    assert_eq!(
        second.session.id, saved.session.id,
        "the same session goes on"
    );
    assert_eq!(second.session.turns.len(), 2);
}

#[tokio::test]
async fn without_resume_a_new_session_starts() {
    let ws = workspace();
    let llm = Arc::new(ScriptedLlm::new(vec![ScriptedResponse::text("hola")]));
    let (mut first, _) = app(&ws, true, "", &llm).await;
    assert_eq!(
        run_turn(&mut first, "hola", CancelToken::new()).await,
        Exit::Success
    );
    let saved = first.stored_session();

    let (fresh, _) = app(&ws, false, "", &llm).await;
    assert_ne!(fresh.session.id, saved.session.id);
    assert!(fresh.session.turns.is_empty());
    assert!(fresh.timeline_blocks().is_empty());
    assert!(fresh.prompt_history().is_empty());
}

#[test]
fn the_window_opens_with_the_resumed_timeline() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let ws = workspace();
    let llm = Arc::new(ScriptedLlm::new(vec![ScriptedResponse::text("hola")]));
    let saved = runtime.block_on(async {
        let (mut first, _) = app(&ws, true, "", &llm).await;
        run_turn(&mut first, "hola", CancelToken::new()).await;
        first.stored_session()
    });
    assert!(!saved.timeline.is_empty());

    let parts = session_parts(
        &config(&ws.workspace, true),
        overrides(&ws, &llm),
        runtime.handle(),
    )
    .unwrap();
    assert_eq!(parts.history, saved.timeline);
    let fresh = session_parts(
        &config(&ws.workspace, false),
        overrides(&ws, &llm),
        runtime.handle(),
    )
    .unwrap();
    assert!(fresh.history.is_empty());
}
