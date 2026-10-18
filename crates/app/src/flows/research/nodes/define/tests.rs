use std::sync::Arc;

use domain::block::{BlockKind, Cell, SourceRef};
use domain::conversation::ContentPart;
use domain::model::{ModelPolicy, ModelRole};
use domain::research::{ConstraintKind, ResearchState, Stage};
use serde_json::json;

use super::*;
use crate::flows::research::node::NodeDeps;
use crate::ports::events::DomainEvent;
use crate::ports::llm::LlmRequest;
use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
use crate::testing::system::RecordingSink;

const GOOD: &str = "a 500 requests por segundo el tiempo de respuesta sube de 10 ms a 500 ms";
const BAD: &str = "el tiempo de respuesta depende de la carga";

fn state() -> ResearchState {
    let mut state = ResearchState::default();
    state.add_evidence(
        "p95 de 500 ms con 500 rps",
        SourceRef::Command {
            command: "make bench".into(),
            output_excerpt: "p95=500ms".into(),
        },
        1,
    );
    state.add_hypothesis(
        "la caché de sesiones se invalida en cada request",
        vec![],
        1,
    );
    state
}

fn draft(statement: &str) -> ScriptedResponse {
    ScriptedResponse::text(
        json!({
            "problems": [{ "statement": statement, "evidence_ids": ["E1"] }],
            "constraints": [
                { "kind": "performance", "statement": "p95 < 20 ms a 500 rps",
                  "test": { "kind": "command", "ref": "make bench-sessions" } },
                { "kind": "compatibility", "statement": "el formato de sesión no cambia", "test": null }
            ]
        })
        .to_string(),
    )
}

fn verdict(measurable: bool, reason: &str) -> ScriptedResponse {
    ScriptedResponse::text(
        json!({ "verdicts": [{ "index": 0, "measurable": measurable, "reason": reason }] })
            .to_string(),
    )
}

struct Run {
    outcome: NodeOutcome,
    state: ResearchState,
    blocks: Vec<BlockKind>,
    requests: Vec<LlmRequest>,
    problems: Option<Vec<ProblemDraft>>,
}

async fn run(
    state: ResearchState,
    reply: Option<&str>,
    script: impl IntoIterator<Item = ScriptedResponse>,
) -> Run {
    let llm = Arc::new(ScriptedLlm::new(script));
    let sink = Arc::new(RecordingSink::new());
    let deps = NodeDeps::new(
        Arc::clone(&llm) as _,
        ModelPolicy::default(),
        Arc::clone(&sink) as _,
    )
    .with_clock(|| 9);
    let mut ctx = NodeContext::new(Stage::Definition, deps, "el login es lento", state);
    if let Some(reply) = reply {
        ctx = ctx.with_reply(reply);
    }
    let outcome = Definition::new().run(&mut ctx).await.unwrap();
    let problems = ctx.data(PROBLEMS_KEY);
    let blocks = sink
        .events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::BlockAdded { block } => Some(block.kind),
            _ => None,
        })
        .collect();
    Run {
        outcome,
        state: ctx.into_state(),
        blocks,
        requests: llm.requests(),
        problems,
    }
}

fn user_text(req: &LlmRequest) -> String {
    req.messages
        .last()
        .unwrap()
        .content
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn writes_the_definition_and_the_constraint_table() {
    let run = run(state(), None, [draft(GOOD), verdict(true, "")]).await;
    assert_eq!(run.outcome, NodeOutcome::Done);
    assert_eq!(run.state.problem_statements, [GOOD]);
    let constraints = &run.state.constraints;
    assert_eq!(constraints.len(), 2);
    assert_eq!(constraints[0].id.to_string(), "C1");
    assert_eq!(constraints[0].kind, ConstraintKind::Performance);
    assert_eq!(
        constraints[0].test.as_ref().unwrap().reference,
        "make bench-sessions"
    );
    assert!(constraints[1].test.is_none());
    assert_eq!(run.problems.unwrap()[0].evidence_ids[0].to_string(), "E1");

    let [BlockKind::Markdown { source, .. }, BlockKind::Table(table)] = run.blocks.as_slice()
    else {
        panic!("expected Markdown and Table, got {:?}", run.blocks);
    };
    assert!(source.contains(&format!("1. {GOOD} [E1]")));
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.rows[0][1], Cell::Text("rendimiento".into()));
    assert_eq!(table.rows[1][3], Cell::Null);

    assert_eq!(run.requests.len(), 2);
    assert!(run.requests.iter().all(|r| r.model.role == ModelRole::Plan));
    assert!(user_text(&run.requests[0]).contains("E1: p95 de 500 ms"));
    assert!(user_text(&run.requests[1]).contains(GOOD));
    assert_eq!(run.state.journal.last().unwrap().stage, Stage::Definition);
}

#[tokio::test]
async fn asks_again_with_the_reason_when_the_heuristic_rejects() {
    // The heuristic rejects the first draft without asking the judge.
    let run = run(state(), None, [draft(BAD), draft(GOOD), verdict(true, "")]).await;
    assert_eq!(run.outcome, NodeOutcome::Done);
    assert_eq!(run.state.problem_statements, [GOOD]);
    assert_eq!(run.requests.len(), 3);
    let retry = user_text(&run.requests[1]);
    assert!(
        retry.contains(BAD) && retry.contains("«depende de»"),
        "{retry}"
    );
}

#[tokio::test]
async fn asks_the_user_when_still_unmeasurable() {
    let run = run(
        state(),
        None,
        [
            draft("el login tarda más de 2 s"),
            verdict(false, "no dice con cuántos usuarios"),
            draft(BAD),
        ],
    )
    .await;
    let NodeOutcome::NeedsInput { prompt } = run.outcome else {
        panic!("expected NeedsInput, got {:?}", run.outcome);
    };
    assert!(prompt.contains(BAD) && prompt.contains("magnitud con unidad"));
    let retry = user_text(&run.requests[2]);
    assert!(retry.contains("no dice con cuántos usuarios"));
    assert!(run.state.problem_statements.is_empty());
    assert!(run.state.constraints.is_empty());
    assert!(run.blocks.is_empty());
    assert!(run.state.journal.len() >= 2);
}

#[tokio::test]
async fn reruns_replace_the_definition_with_the_users_answer() {
    let mut previous = state();
    previous.problem_statements.push("vieja".into());
    previous.add_constraint(ConstraintKind::Other, "vieja", None);
    let run = run(
        previous,
        Some("con 500 rps"),
        [draft(GOOD), verdict(true, "")],
    )
    .await;
    assert!(user_text(&run.requests[0]).contains("Indicaciones del usuario: con 500 rps"));
    assert_eq!(run.state.problem_statements, [GOOD]);
    assert_eq!(run.state.constraints.len(), 2);
    assert_eq!(run.state.constraints[0].id.to_string(), "C1");
}

#[tokio::test]
async fn goes_back_when_evidence_or_hypotheses_are_missing() {
    let no_evidence = run(ResearchState::default(), None, []).await;
    assert!(matches!(
        no_evidence.outcome,
        NodeOutcome::Back {
            to: Stage::Data,
            ..
        }
    ));
    let mut only_evidence = state();
    only_evidence.hypotheses.clear();
    let no_hypotheses = run(only_evidence, None, []).await;
    assert!(matches!(
        no_hypotheses.outcome,
        NodeOutcome::Back {
            to: Stage::Hypotheses,
            ..
        }
    ));
    assert!(no_hypotheses.requests.is_empty());
}
