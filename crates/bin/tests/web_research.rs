//! Research on the web from the chat, without network: the real
//! composition root with the model and the web replaced by scripts.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::sync::{Arc, Mutex};

use application::ports::plugin::ConfigSources;
use application::ports::web::{SearchHit, WebPage, WebPort};
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::testing::web::ScriptedWeb;
use application::usecases::cancel::CancelToken;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal, run_turn};
use crest::root::{App, Config, Overrides, build_app};
use domain::permission::PermissionMode;
use serde_json::json;

const POST: &str = "https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/";

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

fn web() -> Arc<ScriptedWeb> {
    Arc::new(
        ScriptedWeb::new()
            .on_search(
                "rust 1.90",
                vec![SearchHit {
                    title: "Announcing Rust 1.90.0".into(),
                    url: POST.into(),
                    snippet: "The Rust team is happy to announce 1.90.0".into(),
                }],
            )
            .on_fetch(
                POST,
                WebPage {
                    url: POST.into(),
                    title: Some("Announcing Rust 1.90.0 | Rust Blog".into()),
                    text: "LLD is now the default linker on x86_64-unknown-linux-gnu.".into(),
                },
            ),
    )
}

/// The app with `init` as its workspace `init.lua`, in plan mode.
async fn app(
    init: &str,
    llm: Arc<ScriptedLlm>,
    web: Arc<ScriptedWeb>,
) -> (tempfile::TempDir, App, Buffer) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(workspace.join(".crest")).unwrap();
    let init_path = workspace.join(".crest/init.lua");
    std::fs::write(&init_path, init).unwrap();
    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let overrides = Overrides {
        llm: Some(llm),
        config_sources: Some(ConfigSources {
            global: None,
            workspace: Some(init_path),
        }),
        db_path: Some(dir.path().join("sessions.db")),
        web: Some(web as Arc<dyn WebPort>),
        ..Overrides::default()
    };
    let config = Config {
        workspace,
        mode: PermissionMode::Plan,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
    };
    let app = build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap();
    (dir, app, output)
}

fn tool_names(llm: &ScriptedLlm) -> Vec<String> {
    llm.requests()[0]
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

#[tokio::test]
async fn the_chat_researches_on_the_web_without_asking() {
    let llm = Arc::new(ScriptedLlm::new([
        ScriptedResponse::tool_use("t1", "web_search", json!({ "query": "rust 1.90" })),
        ScriptedResponse::tool_use("t2", "web_fetch", json!({ "url": POST })),
        ScriptedResponse::text("Rust 1.90 makes LLD the default linker on Linux [1]."),
    ]));
    let web = web();
    let (_dir, mut app, output) = app(
        "local app = require('app')\napp.opt.web = { max_results = 3 }\n",
        llm.clone(),
        web.clone(),
    )
    .await;
    let exit = run_turn(&mut app, "investiga qué trae rust 1.90", CancelToken::new()).await;
    let out = output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    assert_eq!(web.searches(), [("rust 1.90".to_owned(), 3)]);
    assert_eq!(web.fetches(), [POST]);
    assert!(out.contains("web_search"), "{out}");
    assert!(
        out.contains(&format!("[1] Announcing Rust 1.90.0 | Rust Blog — {POST}")),
        "{out}"
    );
    let tools = tool_names(&llm);
    assert!(tools.contains(&"web_search".to_owned()), "{tools:?}");
    let system: String = llm.requests()[0]
        .system
        .iter()
        .map(|block| block.text.clone())
        .collect();
    assert!(system.contains("# Web"), "{system}");
}

#[tokio::test]
async fn disabled_web_has_no_tools() {
    let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text("vale")]));
    let (_dir, mut app, _) = app(
        "local app = require('app')\napp.opt.web = { enabled = false }\n",
        llm.clone(),
        web(),
    )
    .await;
    run_turn(&mut app, "hola", CancelToken::new()).await;
    let tools = tool_names(&llm);
    assert!(!tools.iter().any(|t| t.starts_with("web_")), "{tools:?}");
}
