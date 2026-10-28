//! A plugin tool opens a modal with an image, a table and a JSON tree
//! during a turn driven by a scripted model, through the real composition
//! root: the front end gets the modal with its image paths checked, and
//! headless mode prints a one-line notice instead.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use application::ports::events::{DomainEvent, EventSink};
use application::ports::plugin::ConfigSources;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::testing::system::{RecordingSink, ScriptedApproval};
use application::usecases::cancel::CancelToken;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal, run_turn};
use crest::root::{App, Config, Frontend, Overrides, build_app};
use domain::block::{CustomNode, ImageData};
use domain::modal::ModalId;
use domain::permission::PermissionMode;
use serde_json::json;

const PLUGIN: &str = r#"
app.tools.register({
  name = "coverage_report",
  description = "Shows the coverage report in a modal",
  effect = "read",
  handler = function()
    app.modal.open{
      id = "coverage", title = "Coverage", width = "60%", height = "80%",
      position = "top-right",
      content = app.ui.col{
        app.ui.image{ path = "out/plot.png", alt = "plot" },
        app.ui.image{ path = ".env" },
        app.ui.csv("file,lines\nsrc/a.rs,120\n"),
        app.ui.json({ total = 87.5 }),
      },
    }
    return "the report is open"
  end,
})
"#;

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

/// A workspace with the plot and a `.env`, and a config with the plugin.
fn setup(dir: &Path) -> (PathBuf, Overrides, Arc<ScriptedLlm>) {
    let ws = dir.join("ws");
    std::fs::create_dir_all(ws.join("out")).unwrap();
    std::fs::write(ws.join("out/plot.png"), "png").unwrap();
    std::fs::write(ws.join(".env"), "KEY=x").unwrap();
    let plugin_dir = dir.join("config").join("plugins").join("coverage");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("init.lua"), PLUGIN).unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedResponse::tool_use("toolu_1", "coverage_report", json!({})),
        ScriptedResponse::text("The report is on screen."),
    ]));
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources {
            global: Some(dir.join("config").join("init.lua")),
            workspace: None,
        }),
        db_path: Some(dir.join("sessions.db")),
        mcp_trust_path: Some(dir.join("mcp-trust.json")),
        plugin_trust_path: Some(dir.join("plugin-trust.json")),
        ..Overrides::default()
    };
    (ws, overrides, llm)
}

fn config(ws: PathBuf) -> Config {
    Config {
        workspace: ws,
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
        session_id: None,
    }
}

async fn run(app: &mut App) -> Exit {
    run_turn(app, "show me the coverage", CancelToken::new()).await
}

#[tokio::test]
async fn a_plugin_tool_opens_a_modal_with_checked_images() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, overrides, llm) = setup(dir.path());
    let sink = Arc::new(RecordingSink::new());
    let frontend = Frontend {
        events: Arc::clone(&sink) as Arc<dyn EventSink>,
        approval: Arc::new(ScriptedApproval::new()),
        form: Arc::new(application::ports::form::NoForms(
            "no UI in this test".into(),
        )),
    };
    let mut app = build_app(&config(ws.clone()), frontend, overrides)
        .await
        .unwrap();
    assert!(app.config_errors.is_empty(), "{:?}", app.config_errors);

    assert_eq!(run(&mut app).await, Exit::Success);
    let result = format!("{:?}", llm.requests()[1].messages.last().unwrap());
    assert!(result.contains("the report is open"), "{result}");

    let events = sink.events();
    let modals: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            DomainEvent::ModalOpened { spec } => Some(spec),
            _ => None,
        })
        .collect();
    let [spec] = modals.as_slice() else {
        panic!("one modal: {events:?}")
    };
    assert_eq!(spec.id, ModalId::new("coverage/coverage"));
    assert_eq!(spec.position.to_string(), "top-right");
    let CustomNode::Col { children } = &spec.content else {
        panic!("{:?}", spec.content)
    };
    let plot = adapter_fs::guard::canonicalize_blocking(&ws.join("out/plot.png")).unwrap();
    assert!(
        matches!(
            &children[0],
            CustomNode::Image { source: ImageData::Path { path }, .. }
                if Path::new(path) == plot
        ),
        "the plot resolves inside the workspace: {:?}",
        children[0]
    );
    assert!(
        matches!(&children[1], CustomNode::Text { text } if text.starts_with("[image unavailable: .env: ")),
        "a sensitive file is not shown: {:?}",
        children[1]
    );
    assert!(matches!(children[2], CustomNode::Table(_)));
    assert!(matches!(children[3], CustomNode::Json { .. }));
    assert!(
        app.timeline_blocks()
            .iter()
            .all(|block| !format!("{:?}", block.kind).contains("Coverage")),
        "the modal is not part of the saved timeline"
    );
}

#[tokio::test]
async fn headless_mode_announces_the_modal() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, overrides, _llm) = setup(dir.path());
    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let mut app = build_app(&config(ws), frontend(&terminal, approval), overrides)
        .await
        .unwrap();
    assert_eq!(run(&mut app).await, Exit::Success);
    let out = output.text();
    assert!(
        out.contains("modal `Coverage` opened (only visible in the window)"),
        "{out}"
    );
}
