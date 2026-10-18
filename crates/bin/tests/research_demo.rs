//! The phase 9 demo, without network: the real composition root (rg, fzf,
//! LocalFs, LocalShell, SQLite, Lua) with the model replaced by a script,
//! over a repository where `add` subtracts and `test.sh` shows it.
//!
//! From the bug described in the chat the flow goes through the 14 nodes
//! to `.crest/specs/0001-…/` with `spec.md` (a chart whose data is a CSV
//! in `evidence/`, Mermaid diagrams and cited sources), `plan.json`,
//! `plan.md`, `journal.md` and `sources.json`. On the way:
//!
//! - the app is closed while node 6 (Experiments) runs, and reopening it
//!   says so and `resume` continues at node 6;
//! - the critical review rejects a Spec with a claim without evidence and
//!   sends the flow back to Data, the stage that collects evidence;
//! - Prototipo is skipped with `/skip`.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use application::ports::plugin::ConfigSources;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use crest::headless::research::run_flow;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal};
use crest::root::{App, Config, Overrides, build_app};
use domain::permission::PermissionMode;
use domain::plan::Plan;
use serde_json::{Value, json};

const LIB: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a - b\n}\n";
const TEST_SH: &str = "echo \"FALLA: add(2, 2) = 0, se esperaba 4\"\n";
const BUG: &str = "add(2, 2) devuelve 0 en vez de 4; test.sh lo muestra";
const UNCITED: &str = "Los usuarios reportan sumas incorrectas desde hace meses.";

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

fn text(value: Value) -> ScriptedResponse {
    ScriptedResponse::text(value.to_string())
}

fn intake() -> ScriptedResponse {
    text(json!({
        "kind": "bug",
        "restated": "`add` resta sus argumentos en vez de sumarlos.",
        "questions": []
    }))
}

/// Observation: reads `src/lib.rs`, then reports.
fn observation() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::tool_use("obs1", "read_file", json!({ "path": "src/lib.rs" })),
        text(json!({
            "summary": "Una biblioteca con una función `add` y un script de prueba.",
            "architecture": "flowchart LR\n  test[test.sh] --> lib[src/lib.rs: add]",
            "key_files": [
                { "path": "src/lib.rs", "kind": "código", "role": "define add" },
                { "path": "test.sh", "kind": "test", "role": "muestra el resultado de add" }
            ],
            "observations": [{
                "claim": "`add` devuelve `a - b` en lugar de `a + b`",
                "source": { "type": "file", "path": "src/lib.rs", "line_start": 1, "line_end": 3 }
            }]
        })),
    ]
}

fn data() -> ScriptedResponse {
    text(json!({ "commits": [], "findings": [], "libraries": [] }))
}

fn analysis() -> ScriptedResponse {
    text(json!({
        "findings": [{
            "idea": "add suma sus argumentos",
            "statement": "add(2, 2) da 0 y no 4: resta",
            "verdict": "contradice",
            "kind": "conteo",
            "evidence_ids": ["E1"]
        }],
        "charts": [{
            "title": "add(2, 2)",
            "kind": "bar",
            "labels": ["expected", "obtained"],
            "series": [{ "name": "resultado", "values": [4.0, 0.0] }],
            "evidence_ids": ["E1"]
        }]
    }))
}

fn hypotheses(keep: bool) -> ScriptedResponse {
    let mut h = json!({
        "statement": "`add` usa el operador `-` en lugar de `+`.",
        "priority": 1,
        "causes": [{ "category": "Código", "cause": "operador equivocado" }],
        "evidence_ids": ["E1"]
    });
    if keep {
        h["id"] = json!("H1");
    }
    text(json!({ "effect": "add devuelve la resta", "hypotheses": [h] }))
}

/// Experiments: runs `sh test.sh` (approved on stdin), then reports.
fn experiments() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::tool_use(
            "exp1",
            "run_command",
            json!({ "command": "sh", "args": ["test.sh"] }),
        ),
        text(json!({
            "experiments": [{
                "hypothesis_id": "H1",
                "protocol": "correr test.sh, que imprime add(2, 2)",
                "command": "sh test.sh",
                "expect": "add(2, 2) = 0 confirma la resta",
                "result": "supported",
                "observation": "test.sh imprime add(2, 2) = 0",
                "measurements": [
                    { "label": "expected", "value": 4.0, "unit": "" },
                    { "label": "obtained", "value": 0.0, "unit": "" }
                ]
            }]
        })),
    ]
}

fn definition() -> Vec<ScriptedResponse> {
    vec![
        text(json!({
            "problems": [{
                "statement": "Al ejecutar `sh test.sh`, add(2, 2) devuelve 0 en vez de 4.",
                "evidence_ids": ["E1"]
            }],
            "constraints": [{
                "kind": "compatibility",
                "statement": "La firma de `add` no cambia.",
                "test": { "kind": "command", "ref": "sh test.sh" }
            }]
        })),
        text(json!({ "verdicts": [{ "index": 0, "measurable": true, "reason": "" }] })),
    ]
}

/// Investigación: nothing in the repo, nothing on the web.
fn research() -> Vec<ScriptedResponse> {
    vec![
        text(json!({ "findings": [] })),
        text(json!({ "options": [] })),
    ]
}

fn brainstorm() -> ScriptedResponse {
    text(json!({ "ideas": [
        { "name": "Corregir el operador", "description": "cambiar `-` por `+`", "evidence_ids": ["E1"] },
        { "name": "Reescribir add", "description": "usar `i32::wrapping_add`", "evidence_ids": [] }
    ]}))
}

fn modeling() -> ScriptedResponse {
    text(json!({
        "criteria": ["simplicidad", "riesgo"],
        "discarded": [],
        "scores": [
            { "title": "Corregir el operador", "values": [5, 5] },
            { "title": "Reescribir add", "values": [3, 4] }
        ],
        "chosen": "Corregir el operador",
        "rationale": "El defecto es el operador [E1]; cambiarlo es lo mínimo.",
        "evidence_ids": ["E1"],
        "black_box": {
            "piece": "add corregida",
            "interactions": [{ "with": "test.sh", "how": "la llama" }]
        }
    }))
}

fn spec(background: &str) -> ScriptedResponse {
    ScriptedResponse::tool_use(
        "spec1",
        "submit_spec",
        json!({
            "title": "Suma correcta en add",
            "problem": "Al ejecutar `sh test.sh`, add(2, 2) devuelve 0 en vez de 4 [E1].",
            "background": background,
            "hypothesis": "`add` usa el operador `-` en lugar de `+`.",
            "proposal": "Cambiar el operador de `add` [E1].",
            "black_box": "flowchart LR\n  test[test.sh] --> add[add corregida]",
            "functional": "`add(a, b)` devuelve `a + b`.",
            "sequence": "sequenceDiagram\n  test.sh->>add: add(2, 2)\n  add-->>test.sh: 4",
            "states": "stateDiagram-v2\n  [*] --> corregida",
            "technical": "Editar `src/lib.rs`: `a - b` pasa a `a + b`.",
            "test_protocol": "`sh test.sh` debe imprimir add(2, 2) = 4.",
            "out_of_scope": "Overflow.",
            "risks": "Ninguno."
        }),
    )
}

fn plan() -> ScriptedResponse {
    ScriptedResponse::tool_use(
        "plan1",
        "submit_plan",
        json!({
            "spec": "x",
            "tasks": [{
                "id": "T1",
                "title": "Corregir el operador de add",
                "description": "Cambiar `a - b` por `a + b` en src/lib.rs",
                "depends_on": [],
                "files": ["src/lib.rs"],
                "acceptance": [{ "kind": "command", "ref": "sh test.sh" }],
                "role": "main",
                "risk": "low"
            }],
            "hypothesis_tests": [{ "kind": "command", "ref": "sh test.sh", "expect": "add(2, 2) = 4" }],
            "constraint_tests": [{ "constraint": "C1", "kind": "command", "ref": "sh test.sh" }]
        }),
    )
}

fn review_without_gaps() -> ScriptedResponse {
    ScriptedResponse::tool_use("rev1", "submit_review", json!({ "gaps": [] }))
}

/// Until the app is closed during node 6: Intake to Hipótesis, then the
/// first call of Experiments never answers.
fn script_before_closing() -> Vec<ScriptedResponse> {
    let mut script = vec![intake()];
    script.extend(observation());
    script.extend([data(), analysis(), hypotheses(false)]);
    script.push(ScriptedResponse::text("…").hang_after(0));
    script
}

/// After reopening: Experiments to the review that rejects the Spec,
/// the second pass from Data, and the review that approves it.
fn script_after_reopening() -> Vec<ScriptedResponse> {
    let sound = "La función `add` resta en vez de sumar [E1] y test.sh lo confirma [E2].";
    let mut script = experiments();
    script.extend(definition());
    script.extend(research());
    script.extend([
        brainstorm(),
        modeling(),
        spec(&format!("{sound}\n\n{UNCITED}")),
        plan(),
    ]);
    // The review rejects without asking the model; back at Data.
    script.extend([data(), analysis(), hypotheses(true)]);
    // H1 already has a result: Experiments runs no agent turn.
    script.extend(definition());
    script.extend(research());
    script.extend([brainstorm(), modeling(), spec(sound), plan()]);
    script.push(review_without_gaps());
    script
}

struct Workspace {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
}

fn workspace() -> Workspace {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(root.join("test.sh"), TEST_SH).unwrap();
    std::fs::write(root.join("README.md"), "demo\n").unwrap();
    let db = dir.path().join("crest.db");
    Workspace {
        _dir: dir,
        root,
        db,
    }
}

/// The app over `ws` with `script` as the model and `answers` as stdin.
async fn open(
    ws: &Workspace,
    script: Vec<ScriptedResponse>,
    answers: &str,
) -> (App, Arc<ScriptedLlm>, StdinApproval, Terminal, Buffer) {
    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    let input = Box::new(Cursor::new(answers.as_bytes().to_vec()));
    let approval = StdinApproval::new(terminal.clone(), input, false);
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
    let app = build_app(&config, frontend(&terminal, approval.clone()), overrides)
        .await
        .unwrap();
    (app, llm, approval, terminal, output)
}

/// The only Spec folder written.
fn spec_folder(root: &Path) -> PathBuf {
    let specs = root.join(".crest/specs");
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&specs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(folders.len(), 1, "{folders:?}");
    folders.pop().unwrap()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[tokio::test(flavor = "multi_thread")]
async fn from_a_bug_in_the_chat_to_a_spec_with_a_chart_mermaid_and_sources() {
    let ws = workspace();

    // 1. The bug is described; the app is closed while node 6 runs.
    let before = script_before_closing();
    let calls = before.len();
    let (mut app, llm, input, terminal, output) = open(&ws, before, "sí\nsí\n").await;
    let flow = run_flow(&mut app, BUG, &input, &terminal);
    let closed = async {
        while llm.requests().len() < calls {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    tokio::select! {
        exit = flow => panic!("the flow ended before node 6: {exit:?}\n{}", output.text()),
        () = closed => {}
    }
    drop(app);
    let first = output.text();
    assert!(first.contains("Experiments"), "{first}");

    // 2. Reopening says a flow is pending at node 6 and resumes it there.
    let answers = "y\nsí\nsí\n/skip\nsí\nsí\nsí\n/skip\nsí\n";
    let (mut app, llm, input, terminal, output) =
        open(&ws, script_after_reopening(), answers).await;
    let startup = output.text();
    assert!(
        startup.contains("research flow is pending") && startup.contains("6/14"),
        "{startup}"
    );
    let exit = run_flow(&mut app, "resume", &input, &terminal).await;
    let out = output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    assert_eq!(llm.remaining(), 0, "{out}");
    let first_resumed = &llm.requests()[0];
    assert!(
        serde_json::to_string(&first_resumed.messages)
            .unwrap()
            .contains("Experiments"),
        "the first call after reopening is node 6"
    );
    assert!(out.contains("Allow run_command"), "{out}");
    // The agents' final JSON stays out of the timeline; their tool calls
    // (the acting nodes') do not.
    assert!(
        !out.contains("\"hypothesis_id\"") && !first.contains("\"key_files\""),
        "{out}"
    );
    assert!(out.contains("run_command"), "{out}");
    assert!(out.contains("Review: 1 gaps"), "{out}");

    // 3. The folder of the Spec.
    let folder = spec_folder(&ws.root);
    assert!(
        folder
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("0001-"),
        "{folder:?}"
    );
    let spec_md = read(&folder.join("spec.md"));
    assert!(spec_md.contains("```mermaid\nflowchart LR"), "{spec_md}");
    assert!(
        spec_md.contains("[E1]") && spec_md.contains("[E2]"),
        "{spec_md}"
    );
    assert!(spec_md.contains("## 10. Sources"), "{spec_md}");
    assert!(spec_md.contains("`src/lib.rs:1-3`"), "{spec_md}");
    assert!(!spec_md.contains(UNCITED), "{spec_md}");
    let chart = spec_md
        .lines()
        .find_map(|l| {
            let start = l.find("](evidence/")? + 2;
            Some(l[start..l.len() - 1].to_owned())
        })
        .unwrap_or_else(|| panic!("no chart linked:\n{spec_md}"));
    let csv = read(&folder.join(&chart));
    assert!(csv.starts_with("label,"), "{csv}");
    assert!(
        csv.contains("expected,4") && csv.contains("obtained,0"),
        "{csv}"
    );

    let plan: Plan = serde_json::from_str(&read(&folder.join("plan.json"))).unwrap();
    plan.validate().unwrap();
    assert!(plan.topological_order().is_ok());
    assert_eq!(read(&folder.join("plan.md")), plan.to_markdown());
    let sources: Vec<Value> = serde_json::from_str(&read(&folder.join("sources.json"))).unwrap();
    assert_eq!(sources.len(), 2, "{sources:?}");

    // 4. The review rejected the uncited claim and sent the flow back to
    //    Data, as the journal tells.
    let journal = read(&folder.join("journal.md"));
    assert!(journal.contains("Back to Data"), "{journal}");
    assert!(journal.contains(UNCITED), "{journal}");
    assert!(journal.contains("Stage skipped (/skip)"), "{journal}");
    assert!(read(&folder.join("review.md")).contains("No major gaps") || out.contains("no gaps"));

    // 5. With the flow finished the chat is the chat again.
    assert!(!app.research.as_ref().unwrap().is_active());
    assert_eq!(
        read(&ws.root.join("src/lib.rs")),
        LIB,
        "the flow only investigates"
    );
}
