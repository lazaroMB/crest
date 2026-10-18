use std::sync::{Arc, Mutex};

use domain::block::{BlockKind, Cell, SourceRef};
use domain::model::{ModelPolicy, ModelRole};
use domain::permission::PermissionPolicy;
use domain::research::{ConstraintKind, ResearchState, Stage};
use domain::workspace::WorkspaceRoot;
use serde_json::json;

use super::*;
use crate::flows::research::node::NodeDeps;
use crate::ports::approval::ApprovalResponse;
use crate::ports::events::DomainEvent;
use crate::ports::llm::{LlmEvent, LlmRequest};
use crate::testing::fs::InMemoryFs;
use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
use crate::tools::budget::ResultBudget;
use crate::tools::files::ReadFileTool;
use crate::tools::registry::ToolRegistry;
use crate::usecases::run_turn::{AgentDeps, AgentService};

const CACHE: &str = "pub struct LruCache;\nimpl LruCache {\n    pub fn get(&self) {}\n}\n";
const MOKA: &str = "https://github.com/moka-rs/moka";
const BLOG: &str = "https://example.com/caching-sessions";

fn state() -> ResearchState {
    let mut state = ResearchState::default();
    state
        .problem_statements
        .push("a 500 requests por segundo el tiempo de respuesta sube de 10 ms a 500 ms".into());
    state.add_constraint(ConstraintKind::Performance, "p95 < 20 ms", None);
    state
}

fn repo_answer() -> ScriptedResponse {
    ScriptedResponse::text(
        json!({ "findings": [{
            "name": "LruCache", "summary": "caché LRU ya usada por el índice",
            "path": "src/cache.rs", "line_start": 1, "line_end": 4
        }]})
        .to_string(),
    )
}

/// A web answer whose text is split around a citation of moka.
fn web_answer() -> ScriptedResponse {
    let text = json!({ "options": [
        { "name": "moka", "kind": "library", "summary": "caché concurrente", "url": MOKA },
        { "name": "Cachear sesiones", "kind": "article", "summary": "TTL por sesión", "url": BLOG }
    ]})
    .to_string();
    let (a, b) = text.split_at(20);
    ScriptedResponse::events(vec![
        LlmEvent::MessageStart {
            model: "claude-test".into(),
        },
        LlmEvent::ServerToolUse {
            index: 0,
            name: "web_search".into(),
        },
        LlmEvent::BlockStop { index: 0 },
        LlmEvent::TextDelta {
            index: 1,
            text: a.into(),
        },
        LlmEvent::Citation {
            index: 1,
            citation: Citation {
                url: MOKA.into(),
                title: Some("moka".into()),
                cited_text: "A fast concurrent cache library for Rust".into(),
            },
        },
        LlmEvent::TextDelta {
            index: 1,
            text: b.into(),
        },
        LlmEvent::BlockStop { index: 1 },
        LlmEvent::MessageStop {
            stop_reason: domain::conversation::StopReason::EndTurn,
        },
    ])
}

fn agent(llm: &Arc<ScriptedLlm>, sink: &Arc<RecordingSink>) -> Arc<AgentService> {
    let fs = Arc::new(
        InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap()).with_file("src/cache.rs", CACHE),
    );
    let budget = ResultBudget::default();
    let mut tools = ToolRegistry::new().with_budget(budget);
    tools.register_typed(ReadFileTool::new(fs, budget)).unwrap();
    let deps = AgentDeps {
        llm: Arc::clone(llm) as _,
        events: Arc::clone(sink) as _,
        approval: Arc::new(ScriptedApproval::always(ApprovalResponse::Approve)),
        telemetry: Arc::new(RecordingTelemetry::new()),
    };
    let permissions = Arc::new(Mutex::new(PermissionPolicy::read_only()));
    Arc::new(AgentService::new(
        deps,
        tools,
        ModelPolicy::default(),
        permissions,
    ))
}

struct Run {
    outcome: NodeOutcome,
    state: ResearchState,
    blocks: Vec<BlockKind>,
    requests: Vec<LlmRequest>,
    options: Option<Vec<PriorSolution>>,
}

async fn run(state: ResearchState, with_agent: bool, script: Vec<ScriptedResponse>) -> Run {
    let llm = Arc::new(ScriptedLlm::new(script));
    let sink = Arc::new(RecordingSink::new());
    let mut deps = NodeDeps::new(
        Arc::clone(&llm) as _,
        ModelPolicy::default(),
        Arc::clone(&sink) as _,
    )
    .with_clock(|| 5);
    if with_agent {
        deps = deps.with_agent(agent(&llm, &sink));
    }
    let mut ctx = NodeContext::new(Stage::Research, deps, "el login es lento", state);
    let outcome = Research::new().run(&mut ctx).await.unwrap();
    let options = ctx.data(OPTIONS_KEY);
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
        options,
    }
}

#[tokio::test]
async fn records_repo_and_web_solutions_as_evidence() {
    let script = vec![
        ScriptedResponse::tool_use("toolu_1", "read_file", json!({ "path": "src/cache.rs" })),
        repo_answer(),
        web_answer(),
    ];
    let run = run(state(), true, script).await;
    assert_eq!(run.outcome, NodeOutcome::Done);

    // E1 from the repo, E2 from the moka citation, E3 for the uncited blog.
    let evidence = &run.state.evidence;
    assert_eq!(evidence.len(), 3);
    assert!(
        matches!(&evidence[0].source, SourceRef::File { path, line_start: Some(1), .. } if path == "src/cache.rs")
    );
    assert_eq!(
        evidence[1].claim,
        "A fast concurrent cache library for Rust"
    );
    assert!(matches!(&evidence[2].source, SourceRef::Url { url, .. } if url == BLOG));

    let options = run.options.unwrap();
    let ids: Vec<Vec<String>> = options
        .iter()
        .map(|o| o.evidence_ids.iter().map(ToString::to_string).collect())
        .collect();
    assert_eq!(ids, [["E1"], ["E2"], ["E3"]]);
    assert_eq!(options[0].kind, SolutionKind::Pattern);
    assert_eq!(options[1].kind, SolutionKind::Library);

    let [.., BlockKind::Table(table), BlockKind::Sources { sources }] = run.blocks.as_slice()
    else {
        panic!("expected Table and Sources, got {:?}", run.blocks);
    };
    assert_eq!(table.rows.len(), 3);
    assert_eq!(table.rows[0][3], Cell::Text("src/cache.rs:1-4".into()));
    assert_eq!(table.rows[1][1], Cell::Text("librería".into()));
    assert_eq!(sources.len(), 3);

    // The agent read the file with the explore role; the web request
    // carries the server tools.
    assert_eq!(run.requests.len(), 3);
    assert!(
        run.requests
            .iter()
            .all(|r| r.model.role == ModelRole::Explore)
    );
    assert!(run.requests[0].tools.iter().any(|t| t.name == "read_file"));
    assert!(run.requests[0].server_tools.is_empty());
    assert_eq!(run.requests[2].server_tools.len(), 2);
    assert!(
        run.state
            .journal
            .last()
            .unwrap()
            .text
            .contains("1 en el repo y 2 en la web")
    );
}

#[tokio::test]
async fn without_an_agent_only_searches_the_web() {
    let run = run(state(), false, vec![web_answer()]).await;
    assert_eq!(run.outcome, NodeOutcome::Done);
    assert_eq!(run.requests.len(), 1);
    assert_eq!(run.options.unwrap().len(), 2);
    assert!(run.state.journal[0].text.contains("no se exploró el repo"));
}

#[tokio::test]
async fn goes_back_without_a_problem_definition() {
    let run = run(ResearchState::default(), true, vec![]).await;
    assert!(matches!(
        run.outcome,
        NodeOutcome::Back {
            to: Stage::Definition,
            ..
        }
    ));
    assert!(run.requests.is_empty());
    assert!(run.blocks.is_empty());
}
