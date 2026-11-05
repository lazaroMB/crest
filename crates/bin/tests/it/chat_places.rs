//! Where a chat lives: in a directory's project, in a project the person
//! chose, or in none and with no directory. The real composition root over
//! one SQLite file.

#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use adapter_store::SqliteSessionStore;
use application::ports::plugin::ConfigSources;
use application::ports::store::{ProjectStorePort as _, SessionStorePort as _, StoredSession};
use application::usecases::cancel::CancelToken;
use application::usecases::sidebar::sidebar;
use application_testing::llm::{ScriptedLlm, ScriptedResponse};
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal, run_turn};
use crest::root::{App, Config, Overrides, build_app};
use domain::conversation::{ProjectId, Session};
use domain::permission::PermissionMode;
use serde_json::json;

struct Dirs {
    _tmp: tempfile::TempDir,
    api: PathBuf,
    web: PathBuf,
    db: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let make = |name: &str| {
        let dir = tmp.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.rs"), "pub fn add() {}\n").unwrap();
        std::fs::canonicalize(dir).unwrap()
    };
    Dirs {
        api: make("api"),
        web: make("web"),
        db: tmp.path().join("sessions.db"),
        _tmp: tmp,
    }
}

fn config(workspace: &Path, project: Option<ProjectId>, session: Option<String>) -> Config {
    Config {
        workspace: workspace.to_path_buf(),
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
        session_id: session,
        project: project.map(|id| id.to_string()),
    }
}

struct Quiet;

impl Write for Quiet {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

async fn open(dirs: &Dirs, config: &Config, llm: Arc<ScriptedLlm>) -> App {
    let terminal = Terminal::new(Box::new(Quiet), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let overrides = Overrides {
        llm: Some(llm),
        config_sources: Some(ConfigSources::default()),
        db_path: Some(dirs.db.clone()),
        ..Overrides::default()
    };
    build_app(config, frontend(&terminal, approval), overrides)
        .await
        .unwrap()
}

fn silent() -> Arc<ScriptedLlm> {
    Arc::new(ScriptedLlm::new(Vec::new()))
}

#[tokio::test]
async fn a_chat_opened_in_a_directory_joins_its_project_and_the_next_one_reuses_it() {
    let dirs = dirs();
    let first = open(&dirs, &config(&dirs.api, None, None), silent()).await;
    let second = open(&dirs, &config(&dirs.api, None, None), silent()).await;
    let store = SqliteSessionStore::open(&dirs.db).unwrap();
    let projects = store.projects().await.unwrap();
    assert_eq!(projects.len(), 1, "the second chat found the project");
    assert_eq!(projects[0].roots, [dirs.api.display().to_string()]);
    assert_eq!(projects[0].name, "api");
    assert_eq!(first.session.project_id, Some(projects[0].id));
    assert_eq!(second.session.project_id, Some(projects[0].id));
}

#[tokio::test]
async fn a_new_chat_joins_the_project_it_was_asked_for_and_sees_all_its_directories() {
    let dirs = dirs();
    let store = SqliteSessionStore::open(&dirs.db).unwrap();
    let both = [
        dirs.api.display().to_string(),
        dirs.web.display().to_string(),
    ];
    let multi = store.create_project("multi", &both).await.unwrap();
    // Another project over the same first directory must not be picked.
    store.create_project("api alone", &both[..1]).await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedResponse::tool_use(
            "t1",
            "read_file",
            json!({ "path": dirs.web.join("lib.rs").display().to_string() }),
        ),
        ScriptedResponse::text("ok"),
    ]));
    let mut app = open(&dirs, &config(&dirs.api, Some(multi.id), None), llm).await;
    assert_eq!(app.session.project_id, Some(multi.id));
    assert_eq!(
        run_turn(&mut app, "read", CancelToken::new()).await,
        Exit::Success
    );
    let seen = format!("{:?}", app.stored_session().timeline);
    assert!(
        seen.contains("pub fn add"),
        "the second directory is readable: {seen}"
    );
}

#[tokio::test]
async fn a_chat_saved_without_a_project_is_put_in_its_directory_s_when_opened() {
    let dirs = dirs();
    let store = SqliteSessionStore::open(&dirs.db).unwrap();
    let old = Session::new(dirs.api.display().to_string(), 5);
    let id = old.id;
    store.save(StoredSession::new(old)).await.unwrap();
    let app = open(
        &dirs,
        &config(&dirs.api, None, Some(id.to_string())),
        silent(),
    )
    .await;
    assert_eq!(app.session.id, id);
    let project = app.session.project_id.expect("adopted");
    assert_eq!(
        store.load(id).await.unwrap().session.project_id,
        Some(project)
    );
}

#[tokio::test]
async fn a_chat_with_no_directory_runs_answers_without_file_tools_and_is_listed_alone() {
    let dirs = dirs();
    let llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedResponse::tool_use("t1", "read_file", json!({ "path": "lib.rs" })),
        ScriptedResponse::tool_use("t2", "grep", json!({ "pattern": "add" })),
        ScriptedResponse::text("Necesito un directorio."),
    ]));
    let mut app = open(&dirs, &config(Path::new(""), None, None), llm).await;
    assert_eq!(app.session.workspace_root, "");
    assert_eq!(app.session.project_id, None);
    assert!(app.fs.roots().is_empty(), "no directory is a workspace");
    assert_eq!(
        run_turn(&mut app, "lee lib.rs", CancelToken::new()).await,
        Exit::Success
    );
    let saved = app.stored_session();
    let seen = format!("{:?}", saved.timeline);
    assert!(!seen.contains("pub fn add"), "nothing was read: {seen}");
    assert_eq!(
        seen.matches("this chat has no directory").count(),
        2,
        "the read and the search both asked for a directory: {seen}"
    );
    // The chat's own folder is not a workspace, and no project was made.
    let store = SqliteSessionStore::open(&dirs.db).unwrap();
    assert!(store.projects().await.unwrap().is_empty());
    let listed = sidebar(&store).await.unwrap();
    assert_eq!(listed.chats.len(), 1);
    assert_eq!(listed.chats[0].id, app.session.id);
    assert!(listed.projects.is_empty());
}
