//! A default provider without built-in models (OpenCode): nothing is sent
//! until a model is selected, and the saved pick then serves the turns.
//! The real composition root with a scripted model and a preferences file
//! in a temporary directory.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use application::ports::plugin::ConfigSources;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::usecases::cancel::CancelToken;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal, run_turn};
use crest::preferences::{self, Preferences};
use crest::root::{App, Config, Overrides, build_app};
use domain::model::{ModelId, ProviderId};
use domain::permission::PermissionMode;

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    global: PathBuf,
    preferences: PathBuf,
    db: PathBuf,
}

/// A workspace whose global config makes OpenCode Go the default provider.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let global = dir.path().join("config").join("init.lua");
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::write(
        &global,
        "app.opt.providers = { default = \"opencode-go\" }\n",
    )
    .unwrap();
    Fixture {
        preferences: dir.path().join("data").join("preferences.json"),
        db: dir.path().join("sessions.db"),
        _dir: dir,
        workspace,
        global,
    }
}

async fn app(fx: &Fixture, llm: &Arc<ScriptedLlm>) -> App {
    let terminal = Terminal::new(Box::new(Buffer::default()), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let config = Config {
        workspace: fx.workspace.clone(),
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
    };
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources {
            global: Some(fx.global.clone()),
            workspace: None,
        }),
        db_path: Some(fx.db.clone()),
        preferences_path: Some(fx.preferences.clone()),
        ..Overrides::default()
    };
    build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap()
}

#[tokio::test]
async fn nothing_is_sent_until_a_model_is_selected() {
    let fx = fixture();
    let llm = Arc::new(ScriptedLlm::new(vec![ScriptedResponse::text("hola")]));

    let mut first = app(&fx, &llm).await;
    assert_eq!(first.default_provider, ProviderId::OpencodeGo);
    assert!(first.model_policy().needs_selection());
    assert!(
        first.warnings.iter().any(|w| w.contains("/model")),
        "{:?}",
        first.warnings
    );
    assert_eq!(
        run_turn(&mut first, "hola", CancelToken::new()).await,
        Exit::Config
    );
    assert!(llm.requests().is_empty(), "no provider was called");

    // What the picker saves; the window then reopens the workspace.
    preferences::update(Some(&fx.preferences), |p| {
        p.models
            .insert(ProviderId::OpencodeGo, ModelId::new("opencode-go:glm-5.3"));
    })
    .unwrap();
    let mut second = app(&fx, &llm).await;
    assert!(!second.model_policy().needs_selection());
    assert_eq!(
        run_turn(&mut second, "hola", CancelToken::new()).await,
        Exit::Success
    );
    assert_eq!(
        llm.last_request().unwrap().model.spec.id.as_str(),
        "opencode-go:glm-5.3"
    );
    let saved = Preferences::load(&fx.preferences).unwrap();
    assert_eq!(saved.models.len(), 1);
}
