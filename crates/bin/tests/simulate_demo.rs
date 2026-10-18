//! `crest --headless --simulate`, without network: the real composition
//! root (rg, LocalFs, SQLite, Lua, the HTML exporter) with the model
//! replaced by a script, over a workspace that already has a Spec folder.
//!
//! The script answers the two calls of the use case: the frozen
//! assertions first, then a small client → api → db model. `one_request`
//! passes; `burst` fails (the api holds two requests while the test
//! protocol allows one), so the run exits with 1. The Spec folder gets the
//! model, the traces, `board.html` and one evidence record per scenario.
//!
//! `CREST_KEEP_BOARD=<file>` copies the generated `board.html` there (to
//! open it in a browser).

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use application::ports::plugin::ConfigSources;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::testing::sim::{self, FOLDER, SOURCES_JSON, SPEC_MD};
use crest::headless::run::frontend;
use crest::headless::simulate::run_simulation;
use crest::headless::{Exit, StdinApproval, Terminal};
use crest::root::{App, Config, Overrides, build_app};
use domain::permission::PermissionMode;
use domain::research::Evidence;
use domain::sysmodel::{ScenarioRun, SysModel};
use serde_json::json;

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

struct Workspace {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
}

fn workspace() -> Workspace {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    let spec = root.join(".crest/specs").join(FOLDER);
    std::fs::create_dir_all(&spec).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/orders.rs"), "pub fn order() {}\n").unwrap();
    std::fs::write(spec.join("spec.md"), SPEC_MD).unwrap();
    std::fs::write(spec.join("sources.json"), SOURCES_JSON).unwrap();
    std::fs::write(
        spec.join("journal.md"),
        "# Research journal\n\n## 12. Spec\n\n- `2026-10-01 10:00` Spec written.\n",
    )
    .unwrap();
    let db = dir.path().join("crest.db");
    Workspace {
        _dir: dir,
        root,
        db,
    }
}

async fn open(
    ws: &Workspace,
    script: Vec<ScriptedResponse>,
) -> (App, Arc<ScriptedLlm>, Terminal, Buffer) {
    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let llm = Arc::new(ScriptedLlm::new(script));
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources::default()),
        db_path: Some(ws.db.clone()),
        ..Overrides::default()
    };
    let config = Config {
        workspace: ws.root.clone(),
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
    };
    let app = build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap();
    (app, llm, terminal, output)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spec_is_simulated_with_one_failing_scenario() {
    let ws = workspace();
    let (app, llm, terminal, output) = open(&ws, sim::script()).await;
    let exit = run_simulation(&app, "1", &terminal).await;
    let out = output.text();
    assert_eq!(exit, Exit::TurnFailed, "{out}");
    assert_eq!(exit.code(), 1);
    assert!(
        out.contains("Simulation of 0001-orders-api: 1 of 2 scenarios passed"),
        "{out}"
    );
    assert!(
        out.contains("PASS one_request (2/2 checks, seed 7)"),
        "{out}"
    );
    assert!(out.contains("FAIL burst"), "{out}");
    assert!(
        out.contains("✗ always api.inflight <= 1: false after event"),
        "{out}"
    );
    assert!(out.contains("evidence recorded: E2, E3"), "{out}");

    // Assertions first, in their own call, then the model.
    let requests = llm.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tools[0].name, "submit_assertions");
    assert_eq!(requests[1].tools[0].name, "submit_sysmodel");

    let folder = ws.root.join(".crest/specs").join(FOLDER);
    let sim_dir = folder.join("simulation");
    let model = SysModel::from_json(&read(&sim_dir.join("sysmodel.json"))).unwrap();
    assert_eq!(model.components.len(), 3);
    for (scenario, passed) in [("one_request", true), ("burst", false)] {
        let run: ScenarioRun =
            serde_json::from_str(&read(&sim_dir.join(format!("traces/{scenario}.json")))).unwrap();
        assert_eq!(run.passed, passed, "{scenario}");
        assert_eq!(run.trace.model_hash, model.hash());
    }
    assert!(read(&sim_dir.join("assertions.json")).contains("api.inflight <= 1"));

    let html_path = sim_dir.join("board.html");
    let html = read(&html_path);
    assert!(html.contains("window.THREE={"));
    assert!(html.contains("\"id\":\"burst\""));
    assert!(!html.contains("<script src"), "no external scripts");
    let canonical = html_path.canonicalize().unwrap();
    let shown = out.lines().find_map(|l| l.strip_prefix("board: ")).unwrap();
    assert_eq!(Path::new(shown).canonicalize().unwrap(), canonical);
    if let Ok(keep) = std::env::var("CREST_KEEP_BOARD") {
        std::fs::copy(&html_path, keep).unwrap();
    }

    let sources: Vec<Evidence> = serde_json::from_str(&read(&folder.join("sources.json"))).unwrap();
    let ids: Vec<String> = sources.iter().map(|e| e.id.to_string()).collect();
    assert_eq!(ids, ["E1", "E2", "E3"]);
    assert!(sources[2].claim.contains("`burst`") && sources[2].claim.contains("failed"));
    assert!(sources[2].claim.contains(&model.hash()));
    let journal = read(&folder.join("journal.md"));
    assert!(journal.contains("## 12. Spec"), "the old journal is kept");
    assert!(journal.contains("## Simulation"));
    assert!(journal.contains("Scenario `one_request` passed [E2]."));
    assert!(journal.contains("Scenario `burst` failed [E3]."));
}

#[tokio::test(flavor = "multi_thread")]
async fn every_scenario_passing_exits_with_0() {
    let ws = workspace();
    let mut plan = sim::plan();
    plan["scenarios"][1]["expect"] = json!([{ "always": "api.inflight <= 6" }]);
    let mut model = sim::model();
    model["scenarios"][1]["expect"] = json!([{ "always": "api.inflight <= 6" }]);
    let script = vec![
        ScriptedResponse::tool_use("a", "submit_assertions", plan),
        ScriptedResponse::tool_use("b", "submit_sysmodel", model),
    ];
    let (app, _, terminal, output) = open(&ws, script).await;
    let exit = run_simulation(&app, FOLDER, &terminal).await;
    let out = output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    assert!(out.contains("2 of 2 scenarios passed"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_spec_fails_without_asking_the_model() {
    let ws = workspace();
    let (app, llm, terminal, output) = open(&ws, Vec::new()).await;
    let exit = run_simulation(&app, "0042", &terminal).await;
    assert_eq!(exit, Exit::TurnFailed);
    assert!(
        output.text().contains("no Spec `0042`"),
        "{}",
        output.text()
    );
    assert!(llm.requests().is_empty());
}
