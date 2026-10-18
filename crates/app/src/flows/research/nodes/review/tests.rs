use std::sync::Arc;

use domain::block::{BlockKind, SourceRef};
use domain::model::ModelPolicy;
use domain::plan::Plan;
use domain::research::{ConstraintKind, ResearchState, Stage, TestKind, TestRef};
use domain::workspace::WorkspaceRoot;
use serde_json::{Value, json};

use super::super::spec::render::{SpecAnswer, spec_markdown};
use super::checks::{EVIDENCE_STAGE, Severity};
use super::*;
use crate::flows::research::node::NodeDeps;
use crate::ports::events::DomainEvent;
use crate::testing::fs::InMemoryFs;
use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
use crate::testing::system::RecordingSink;

const FOLDER: &str = "0001-cache-de-sesiones";
const HISTORY: &str = "HISTORIAL-PRIVADO-DEL-FLUJO";
const REQUEST: &str = "PEDIDO-ORIGINAL-DEL-USUARIO";
const UNCITED: &str = "Los usuarios se quejan del login desde marzo.";
const SOUND: &str = "El login tarda 500 ms a 500 rps [E1] porque lee SQLite [E2].";

fn state() -> ResearchState {
    let mut s = ResearchState::default();
    let e1 = s.add_evidence(
        "El login tarda 500 ms a 500 rps",
        SourceRef::Command {
            command: "make bench-login".into(),
            output_excerpt: "p95 500ms".into(),
        },
        1,
    );
    s.add_evidence(
        "Cada login lee la sesión de SQLite",
        SourceRef::File {
            path: "crates/app/src/ports/store.rs".into(),
            line_start: Some(10),
            line_end: Some(20),
        },
        2,
    );
    s.add_hypothesis("Una caché en memoria baja el p95 a 20 ms", vec![], 1);
    s.add_constraint(
        ConstraintKind::Performance,
        "p95 < 20 ms a 500 rps",
        Some(TestRef {
            kind: TestKind::Command,
            reference: "make bench-login".into(),
        }),
    );
    s.add_decision("Caché LRU", vec!["Redis".into()], "sin servicios", vec![e1]);
    s.problem_statements = vec!["A 500 rps el login sube de 10 ms a 500 ms".into()];
    s.log(1, Stage::Intake, HISTORY);
    s
}

fn spec_md(background: &str, state: &ResearchState) -> String {
    let answer: SpecAnswer = serde_json::from_value(json!({
        "title": "Caché de sesiones",
        "problem": "A 500 rps el login sube de 10 ms a 500 ms [E1].",
        "background": background,
        "hypothesis": "Una caché en memoria baja el p95 a 20 ms.",
        "proposal": "Caché LRU delante del store [E2].",
        "black_box": "flowchart LR\n  login --> cache --> store",
        "functional": "El login consulta la caché antes que el store.",
        "sequence": "sequenceDiagram\n  login->>cache: get",
        "states": "stateDiagram-v2\n  [*] --> vacia",
        "technical": "`SessionCache` en `crates/app/src/ports/store.rs`.",
        "test_protocol": "`make bench-login` debe dar p95 < 20 ms.",
        "out_of_scope": "Redis.",
        "risks": "Invalidación."
    }))
    .unwrap();
    spec_markdown("0001", &answer, state)
}

fn plan() -> Plan {
    serde_json::from_value(json!({
        "spec": FOLDER,
        "tasks": [{
            "id": "T1",
            "title": "Caché",
            "description": "agregar la caché",
            "depends_on": [],
            "files": ["crates/app/src/ports/store.rs"],
            "acceptance": [{ "kind": "test", "ref": "cargo test cache" }],
            "risk": "low"
        }],
        "hypothesis_tests": [{ "kind": "command", "ref": "make bench-login", "expect": "p95 < 20ms" }],
        "constraint_tests": [{ "constraint": "C1", "kind": "command", "ref": "make bench-login" }]
    }))
    .unwrap()
}

fn context(
    llm: Arc<ScriptedLlm>,
    sink: Arc<RecordingSink>,
    state: ResearchState,
    spec_md: String,
) -> NodeContext {
    let deps = NodeDeps::new(llm, ModelPolicy::default(), sink).with_clock(|| 9);
    let ctx = NodeContext::new(Stage::CriticalReview, deps, REQUEST, state);
    let draft = SpecDraft {
        folder: FOLDER.into(),
        problem: "p".into(),
        title: "Caché de sesiones".into(),
        spec_md,
    };
    ctx.set_data(DRAFTS_KEY, vec![draft]).unwrap();
    ctx.set_data(PLANS_KEY, vec![FOLDER]).unwrap();
    ctx.set_data(PLANS_DATA_KEY, vec![plan()]).unwrap();
    ctx
}

fn gaps(list: Value) -> ScriptedResponse {
    ScriptedResponse::tool_use("r1", "submit_review", json!({ "gaps": list }))
}

fn blocks(sink: &RecordingSink) -> Vec<BlockKind> {
    sink.events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::BlockAdded { block } => Some(block.kind),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_uncited_claim_goes_back_to_the_evidence_stage() {
    let state = state();
    let md = spec_md(&format!("{SOUND}\n\n{UNCITED}"), &state);
    let llm = Arc::new(ScriptedLlm::new([]));
    let files = Arc::new(MemorySpecSink::new());
    let events = Arc::new(RecordingSink::new());
    let mut ctx = context(Arc::clone(&llm), Arc::clone(&events), state, md);
    let outcome = Review::new()
        .with_sink(files.clone())
        .run(&mut ctx)
        .await
        .unwrap();
    let NodeOutcome::Back { to, reason } = outcome else {
        panic!("expected Back, got {outcome:?}");
    };
    assert_eq!(to, EVIDENCE_STAGE);
    assert!(matches!(to, Stage::Data | Stage::Observation));
    assert!(reason.contains(UNCITED), "{reason}");
    assert!(llm.requests().is_empty());
    let review = files.file(FOLDER, REVIEW_FILE).unwrap();
    assert!(review.contains(UNCITED), "{review}");
    assert!(
        matches!(&blocks(&events)[0], BlockKind::Markdown { source, .. } if source.contains(UNCITED))
    );
    let stored: Vec<Gap> = ctx.data(GAPS_KEY).unwrap();
    assert_eq!(stored.len(), 1, "{stored:?}");
}

#[tokio::test]
async fn a_citation_to_unknown_evidence_is_rejected_without_the_model() {
    let state = state();
    let md = spec_md(SOUND, &state).replace("[E2].", "[E9].");
    let llm = Arc::new(ScriptedLlm::new([gaps(json!([]))]));
    let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), state, md);
    let outcome = Review::new().run(&mut ctx).await.unwrap();
    assert!(
        matches!(&outcome, NodeOutcome::Back { reason, .. } if reason.contains("E9")),
        "{outcome:?}"
    );
    assert!(llm.requests().is_empty());
}

#[tokio::test]
async fn a_sound_spec_without_gaps_is_done() {
    let state = state();
    let md = spec_md(SOUND, &state);
    let llm = Arc::new(ScriptedLlm::new([gaps(json!([{
        "claim": "Riesgos",
        "problem": "podría detallar la invalidación",
        "missing_stage": "modeling",
        "severity": "menor"
    }]))]));
    let events = Arc::new(RecordingSink::new());
    let files = Arc::new(MemorySpecSink::new());
    let mut ctx = context(Arc::clone(&llm), Arc::clone(&events), state, md);
    let outcome = Review::new()
        .with_sink(files.clone())
        .run(&mut ctx)
        .await
        .unwrap();
    assert_eq!(outcome, NodeOutcome::Done);
    let kinds = blocks(&events);
    assert!(
        matches!(&kinds[0], BlockKind::Markdown { source, .. } if source.starts_with("Revisión: sin huecos"))
    );
    assert!(matches!(&kinds[1], BlockKind::Table(t) if t.rows.len() == 1));
    assert!(
        files
            .file(FOLDER, REVIEW_FILE)
            .unwrap()
            .contains("Sin huecos graves")
    );
    let req = llm.last_request().unwrap();
    assert_eq!(req.tools[0].name, "submit_review");
}

#[tokio::test]
async fn a_definition_gap_goes_back_to_the_definition() {
    let state = state();
    let md = spec_md(SOUND, &state);
    let llm = Arc::new(ScriptedLlm::new([gaps(json!([
        {
            "claim": "Caché LRU",
            "problem": "no compara el tamaño",
            "missing_stage": "modeling",
            "severity": "grave"
        },
        {
            "claim": "A 500 rps el login sube de 10 ms a 500 ms",
            "problem": "no dice con qué hardware se mide",
            "missing_stage": "definition",
            "severity": "grave"
        }
    ]))]));
    let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), state, md);
    let outcome = Review::new().run(&mut ctx).await.unwrap();
    let NodeOutcome::Back { to, reason } = outcome else {
        panic!("expected Back");
    };
    assert_eq!(to, Stage::Definition);
    assert!(
        reason.contains("hardware") && reason.contains("tamaño"),
        "{reason}"
    );
}

#[tokio::test]
async fn the_reviewer_request_holds_no_flow_history() {
    let state = state();
    let md = spec_md(SOUND, &state);
    let llm = Arc::new(ScriptedLlm::new([gaps(json!([]))]));
    let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), state, md);
    Review::new().run(&mut ctx).await.unwrap();
    let req = llm.last_request().unwrap();
    assert_eq!(req.messages.len(), 1);
    let text = format!("{:?} {:?}", req.system, req.messages);
    assert!(!text.contains(HISTORY), "{text}");
    assert!(!text.contains(REQUEST), "{text}");
    let prompt = format!("{:?}", req.messages[0].content);
    assert!(prompt.contains("Caché de sesiones"));
    assert!(prompt.contains("hypothesis_tests"));
    assert!(prompt.contains("[E2] Cada login lee la sesión de SQLite"));
}

#[tokio::test]
async fn the_model_may_not_send_the_flow_to_the_review_itself() {
    let state = state();
    let md = spec_md(SOUND, &state);
    let bad = json!([{
        "claim": "x",
        "problem": "y",
        "missing_stage": "critical_review",
        "severity": "grave"
    }]);
    let llm = Arc::new(ScriptedLlm::new([gaps(bad), gaps(json!([]))]));
    let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), state, md);
    assert_eq!(
        Review::new().run(&mut ctx).await.unwrap(),
        NodeOutcome::Done
    );
    assert_eq!(llm.requests().len(), 2);
}

#[tokio::test]
async fn plan_and_constraint_holes_are_found_without_the_model() {
    let mut state = state();
    state.add_constraint(ConstraintKind::Security, "sin secretos en logs", None);
    let md = spec_md(SOUND, &state);
    let llm = Arc::new(ScriptedLlm::new([]));
    let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), state, md);
    let mut no_hypothesis = plan();
    no_hypothesis.hypothesis_tests.clear();
    ctx.set_data(PLANS_DATA_KEY, vec![no_hypothesis]).unwrap();
    let outcome = Review::new().run(&mut ctx).await.unwrap();
    assert!(
        matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Definition,
                ..
            }
        ),
        "{outcome:?}"
    );
    let stored: Vec<Gap> = ctx.data(GAPS_KEY).unwrap();
    let problems: Vec<&str> = stored.iter().map(|g| g.problem.as_str()).collect();
    assert!(
        problems.iter().any(|p| p.contains("no tiene una prueba")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("hypothesis_tests")),
        "{problems:?}"
    );
    assert!(problems.iter().any(|p| p.contains("C2")), "{problems:?}");
    assert!(stored.iter().all(|g| g.severity == Severity::Major));
    assert!(llm.requests().is_empty());
}

#[tokio::test]
async fn the_workspace_supplies_the_plan_and_checks_cited_files() {
    let state = state();
    let md = spec_md(SOUND, &state);
    let json = serde_json::to_string(&plan()).unwrap();
    let fs = Arc::new(
        InMemoryFs::new(WorkspaceRoot::new(std::env::temp_dir()).unwrap())
            .with_file(&format!(".crest/specs/{FOLDER}/plan.json"), json),
    );
    let llm = Arc::new(ScriptedLlm::new([]));
    let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), state, md);
    ctx.set_data(PLANS_DATA_KEY, Vec::<Plan>::new()).unwrap();
    let outcome = Review::new()
        .with_workspace(fs)
        .run(&mut ctx)
        .await
        .unwrap();
    let NodeOutcome::Back { to, reason } = outcome else {
        panic!("expected Back");
    };
    assert_eq!(to, Stage::Observation);
    assert!(reason.contains("crates/app/src/ports/store.rs"), "{reason}");
    let stored: Vec<Gap> = ctx.data(GAPS_KEY).unwrap();
    assert_eq!(stored.len(), 1, "{stored:?}");
}

#[tokio::test]
async fn without_a_spec_it_goes_back_to_the_spec() {
    let deps = NodeDeps::new(
        Arc::new(ScriptedLlm::new([])),
        ModelPolicy::default(),
        Arc::new(RecordingSink::new()),
    );
    let mut ctx = NodeContext::new(Stage::CriticalReview, deps, "p", ResearchState::default());
    let outcome = Review::new().run(&mut ctx).await.unwrap();
    assert!(matches!(
        outcome,
        NodeOutcome::Back {
            to: Stage::Spec,
            ..
        }
    ));
}
