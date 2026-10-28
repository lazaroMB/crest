//! The phase 10 demo, without network: a Spec with a `plan.json` of two
//! tasks over a real git repository with a real bug and a failing test,
//! implemented end to end by the real composition root (LocalFs,
//! LocalShell with git, SQLite) with the model replaced by a script and
//! the gates and approvals answered on a simulated stdin. The checks run
//! `sh` scripts: Unix only.

#![cfg(unix)]
// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use application::ports::plugin::ConfigSources;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use crest::headless::implement::run_flow;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal};
use crest::root::{App, Config, Overrides, build_app};
use domain::permission::PermissionMode;
use serde_json::json;

const SPEC: &str = "0001-calculadora";
const CALC: &str = "add() { echo $(($1 - $2)); }\n";
const WRONG_FIX: &str = "add() { echo $(($1 + $2 + 1)); }\n";
const ADD_TEST: &str = ". ./calc.sh\n\
    got=$(add 2 3)\n\
    [ \"$got\" = 5 ] || { echo \"add 2 3 = $got, esperaba 5\"; exit 1; }\n";
const MUL_TEST: &str = ". ./calc.sh\n\
    got=$(mul 2 3)\n\
    [ \"$got\" = 6 ] || { echo \"mul 2 3 = $got, esperaba 6\"; exit 1; }\n";

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

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn plan() -> String {
    json!({
        "spec": SPEC,
        "tasks": [
            {
                "id": "T2",
                "title": "Agregar la multiplicación",
                "description": "Agregar `mul a b` a calc.sh.",
                "depends_on": ["T1"],
                "files": ["calc.sh"],
                "acceptance": [{ "kind": "test", "ref": "sh tests/mul.sh" }],
                "risk": "low"
            },
            {
                "id": "T1",
                "title": "Corregir la suma",
                "description": "`add` resta en vez de sumar.",
                "depends_on": [],
                "files": ["calc.sh"],
                "acceptance": [{ "kind": "test", "ref": "sh tests/add.sh" }],
                "risk": "low"
            }
        ],
        "hypothesis_tests": [
            { "kind": "command", "ref": "sh bench.sh", "expect": "p95 < 20ms" }
        ],
        "constraint_tests": [
            { "constraint": "C1", "kind": "test", "ref": "sh tests/add.sh" }
        ]
    })
    .to_string()
}

/// A git repository with the bug, its tests, the Spec and its plan, all
/// committed on `main`.
fn repository(dir: &Path) -> PathBuf {
    let ws = dir.join("calc");
    let spec = ws.join(".crest/specs").join(SPEC);
    std::fs::create_dir_all(&spec).unwrap();
    std::fs::create_dir_all(ws.join("tests")).unwrap();
    std::fs::write(ws.join("calc.sh"), CALC).unwrap();
    std::fs::write(ws.join("tests/add.sh"), ADD_TEST).unwrap();
    std::fs::write(ws.join("tests/mul.sh"), MUL_TEST).unwrap();
    std::fs::write(ws.join("bench.sh"), "echo \"p95: 4ms\"\n").unwrap();
    std::fs::write(ws.join("README.md"), "# calc\n").unwrap();
    std::fs::write(
        spec.join("spec.md"),
        "# Calculadora\n\nHipótesis: con `add` corregido y `mul`, la calculadora \
         responde en p95 < 20ms.\n",
    )
    .unwrap();
    std::fs::write(spec.join("plan.json"), plan()).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "crest test"),
        ("user.email", "crest@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(&ws, &["config", key, value]);
    }
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-q", "-m", "inicial"]);
    ws
}

struct Demo {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    db: PathBuf,
    output: Buffer,
    terminal: Terminal,
    input: StdinApproval,
    app: App,
}

async fn demo(answers: &str, script: Vec<ScriptedResponse>) -> Demo {
    let dir = tempfile::tempdir().unwrap();
    let workspace = repository(dir.path());
    let db = dir.path().join("crest.db");
    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    let input = StdinApproval::new(
        terminal.clone(),
        Box::new(Cursor::new(answers.as_bytes().to_vec())),
        false,
    );
    let overrides = Overrides {
        llm: Some(Arc::new(ScriptedLlm::new(script))),
        config_sources: Some(ConfigSources::default()),
        db_path: Some(db.clone()),
        ..Overrides::default()
    };
    let config = Config {
        workspace: workspace.clone(),
        mode: PermissionMode::AutoEdit,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
        session_id: None,
    };
    let app = build_app(&config, frontend(&terminal, input.clone()), overrides)
        .await
        .unwrap();
    Demo {
        _dir: dir,
        workspace,
        db,
        output,
        terminal,
        input,
        app,
    }
}

impl Demo {
    async fn run(&self, target: &str) -> Exit {
        run_flow(&self.app, target, &self.input, &self.terminal).await
    }

    fn file(&self, path: &str) -> String {
        std::fs::read_to_string(self.workspace.join(path)).unwrap()
    }
}

fn write(id: &str, path: &str, content: &str) -> ScriptedResponse {
    ScriptedResponse::tool_use(
        id,
        "write_file",
        json!({ "path": path, "content": content }),
    )
}

#[tokio::test]
async fn implements_the_spec_end_to_end() {
    let fixed = "add() { echo $(($1 + $2)); }\n";
    let with_mul = format!("{fixed}mul() {{ echo $(($1 * $2)); }}\n");
    let script = vec![
        // T1: a wrong fix, which the tests catch, then the right one.
        write("toolu_1", "calc.sh", WRONG_FIX),
        ScriptedResponse::text("Cambié la resta por una suma.\nSTATUS: done"),
        ScriptedResponse::tool_use(
            "toolu_2",
            "edit_file",
            json!({ "path": "calc.sh", "old": " + 1))", "new": "))" }),
        ),
        ScriptedResponse::text("Sobraba un +1.\nSTATUS: done"),
        // T2: mul, and README.md, which is not a file of the task.
        write("toolu_3", "calc.sh", &with_mul),
        write("toolu_4", "README.md", "# calc\n\n`add` y `mul`.\n"),
        ScriptedResponse::text(
            "Agregué mul y lo documenté.\nIDEA: agregar la división\nSTATUS: done",
        ),
    ];
    // Plan gate, the approval of README.md, report gate.
    let demo = demo("sí\ny\nsí\n", script).await;
    let exit = demo.run(&format!(".crest/specs/{SPEC}")).await;
    let out = demo.output.text();
    assert_eq!(exit, Exit::Success, "{out}");

    // The branch of the Spec, one commit per task with its id.
    let ws = &demo.workspace;
    assert_eq!(
        git(ws, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        format!("spec/{SPEC}")
    );
    let log = git(ws, &["log", "--format=%s%n%b--", "main..HEAD"]);
    let commits: Vec<_> = log.split("--\n").filter(|c| !c.trim().is_empty()).collect();
    assert_eq!(commits.len(), 2, "{log}");
    assert!(commits[1].starts_with("fix: Corregir la suma\n"), "{log}");
    assert!(commits[1].contains("Task: T1"), "{log}");
    assert!(
        commits[0].starts_with("feat: Agregar la multiplicación\n"),
        "{log}"
    );
    assert!(commits[0].contains("Task: T2"), "{log}");
    // Each task committed only its files (and the approved extra one).
    let first = git(ws, &["show", "--name-only", "--format=", "HEAD~1"]);
    assert_eq!(first.trim(), "calc.sh");
    let second = git(ws, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(second.lines().collect::<Vec<_>>(), ["README.md", "calc.sh"]);
    assert_eq!(demo.file("calc.sh"), with_mul);

    // Tests with a retry: the first run fails, the fix passes.
    assert!(out.contains("== Load plan (1/6) =="), "{out}");
    assert!(out.contains("T1 → T2"), "{out}");
    assert!(out.contains("sh tests/add.sh | fails (exit 1)"), "{out}");
    assert!(out.contains("sh tests/add.sh | passes"), "{out}");
    // README.md asked and was approved.
    assert!(
        out.contains("Allow write_file README.md? [y]es / [a]lways / [n]o: y\n"),
        "{out}"
    );
    // Verification: expected against obtained.
    assert!(
        out.contains("Test | Command | Expected | Obtained | Holds"),
        "{out}"
    );
    assert!(
        out.contains("Hypothesis | sh bench.sh | p95 < 20ms | p95 4ms | yes"),
        "{out}"
    );
    assert!(
        out.contains("C1 | sh tests/add.sh | passes (exit 0) | passes | yes"),
        "{out}"
    );
    assert!(out.contains("implementation flow finished"), "{out}");

    // report.md in the Spec folder, with the deviation from the journal.
    let report = demo.file(&format!(".crest/specs/{SPEC}/report.md"));
    assert!(report.contains("- Tasks done: 2 of 2"), "{report}");
    assert!(report.contains("README.md"), "{report}");
    assert!(report.contains("agregar la división"), "{report}");
    assert!(report.contains("| Hypothesis |"), "{report}");

    // The flow was checkpointed in SQLite: opening the plan again finds the
    // finished flow and starts a new one.
    assert!(demo.db.exists());
    let again = demo
        .app
        .open_implementation(&format!(".crest/specs/{SPEC}/plan.json"))
        .await
        .unwrap();
    assert!(!again.resumed());
}

#[tokio::test]
async fn a_task_that_exhausts_its_retries_stops_and_asks() {
    // The agent never gets T1 right: the first try and three fixes.
    let mut script = Vec::new();
    for n in 0..4 {
        script.push(write(&format!("toolu_{n}"), "calc.sh", WRONG_FIX));
        script.push(ScriptedResponse::text("Listo.\nSTATUS: done"));
    }
    let demo = demo("yes\nback to spec\n", script).await;
    let exit = demo.run(SPEC).await;
    let out = demo.output.text();
    assert_eq!(exit, Exit::TurnFailed, "{out}");

    assert_eq!(out.matches("sh tests/add.sh | fails (exit 1)").count(), 4);
    assert!(out.contains("used up its 3 fix attempts"), "{out}");
    assert!(out.contains("add 2 3 = 6, esperaba 5"), "{out}");
    assert!(out.contains("/back spec"), "{out}");
    // Nothing was committed.
    let ws = &demo.workspace;
    assert_eq!(git(ws, &["log", "--format=%s", "main..HEAD"]), "");
}

#[tokio::test]
async fn closed_stdin_leaves_the_flow_at_its_gate_to_resume() {
    let demo = demo("", Vec::new()).await;
    assert_eq!(demo.run(SPEC).await, Exit::TurnFailed);
    let session = demo.app.open_implementation(SPEC).await.unwrap();
    assert!(session.resumed());
    let out = demo.output.text();
    assert!(out.contains("stdin closed"), "{out}");
}
