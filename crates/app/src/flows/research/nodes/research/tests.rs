use std::sync::{Arc, Mutex};

use domain::block::{BlockKind, Cell, SourceRef};
use domain::model::{ModelPolicy, ModelRole};
use domain::permission::PermissionPolicy;
use domain::research::{ConstraintKind, ResearchState, Stage};
use serde_json::json;

use super::*;
use crate::flows::research::node::NodeDeps;
use crate::ports::approval::ApprovalResponse;
use crate::ports::events::DomainEvent;
use crate::ports::llm::LlmRequest;
use crate::ports::web::{SearchHit, WebPage};
use crate::testing::fs::{InMemoryFs, test_root};
use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
use crate::testing::web::ScriptedWeb;
use crate::tools::budget::ResultBudget;
use crate::tools::files::ReadFileTool;
use crate::tools::registry::ToolRegistry;
use crate::tools::web::register_web_tools;
use crate::usecases::run_turn::{AgentDeps, AgentService};

const CACHE: &str = "pub struct LruCache;\nimpl LruCache {\n    pub fn get(&self) {}\n}\n";
const MOKA: &str = "https://github.com/moka-rs/moka";
const BLOG: &str = "https://example.com/caching-sessions";

fn state() -> ResearchState {
    let mut state = ResearchState::default();
    state
        .problem_statements
        .push("at 500 requests per second the response time rises from 10 ms to 500 ms".into());
    state.add_constraint(ConstraintKind::Performance, "p95 < 20 ms", None);
    state
}

fn repo_answer() -> ScriptedResponse {
    ScriptedResponse::text(
        json!({ "findings": [{
            "name": "LruCache", "summary": "LRU cache already used by the index",
            "path": "src/cache.rs", "line_start": 1, "line_end": 4
        }]})
        .to_string(),
    )
}

/// The web turn: a search, the moka page, then the options as JSON.
fn web_turn() -> Vec<ScriptedResponse> {
    let options = json!({ "options": [
        { "name": "moka", "kind": "library", "summary": "concurrent cache", "url": MOKA },
        { "name": "Caching sessions", "kind": "article", "summary": "per-session TTL", "url": BLOG }
    ]});
    vec![
        ScriptedResponse::tool_use(
            "w1",
            "web_search",
            json!({ "query": "rust concurrent cache" }),
        ),
        ScriptedResponse::tool_use("w2", "web_fetch", json!({ "url": MOKA })),
        ScriptedResponse::text(options.to_string()),
    ]
}

fn web() -> Arc<ScriptedWeb> {
    Arc::new(
        ScriptedWeb::new()
            .on_search(
                "rust concurrent cache",
                vec![
                    SearchHit {
                        title: "moka".into(),
                        url: MOKA.into(),
                        snippet: "Moka is a fast cache".into(),
                    },
                    SearchHit {
                        title: "Caching sessions".into(),
                        url: BLOG.into(),
                        snippet: "TTL per session".into(),
                    },
                ],
            )
            .on_fetch(
                MOKA,
                WebPage {
                    url: MOKA.into(),
                    title: Some("moka".into()),
                    text: "A fast concurrent cache library for Rust".into(),
                },
            ),
    )
}

fn agent(llm: &Arc<ScriptedLlm>, sink: &Arc<RecordingSink>, with_web: bool) -> Arc<AgentService> {
    let fs = Arc::new(InMemoryFs::new(test_root()).with_file("src/cache.rs", CACHE));
    let budget = ResultBudget::default();
    let mut tools = ToolRegistry::new().with_budget(budget);
    tools.register_typed(ReadFileTool::new(fs, budget)).unwrap();
    if with_web {
        register_web_tools(&mut tools, web(), 5).unwrap();
    }
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

/// Which agent the node gets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum With {
    Nothing,
    Agent,
    AgentAndWeb,
}

async fn run(state: ResearchState, with: With, script: Vec<ScriptedResponse>) -> Run {
    let llm = Arc::new(ScriptedLlm::new(script));
    let sink = Arc::new(RecordingSink::new());
    let mut deps = NodeDeps::new(
        Arc::clone(&llm) as _,
        ModelPolicy::default(),
        Arc::clone(&sink) as _,
    )
    .with_clock(|| 5);
    if with != With::Nothing {
        deps = deps.with_agent(agent(&llm, &sink, with == With::AgentAndWeb));
    }
    let mut ctx = NodeContext::new(Stage::Research, deps, "login is slow", state);
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
    let mut script = vec![
        ScriptedResponse::tool_use("toolu_1", "read_file", json!({ "path": "src/cache.rs" })),
        repo_answer(),
    ];
    script.extend(web_turn());
    let run = run(state(), With::AgentAndWeb, script).await;
    assert_eq!(run.outcome, NodeOutcome::Done);

    // E1 from the repo, E2 from the moka page read, E3 for the blog it
    // only found.
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
    assert_eq!(table.rows[1][1], Cell::Text("library".into()));
    assert_eq!(sources.len(), 3);

    // The agent read the file and the web with the explore role and local
    // tools only.
    assert_eq!(run.requests.len(), 5);
    assert!(
        run.requests
            .iter()
            .all(|r| r.model.role == ModelRole::Explore && r.server_tools.is_empty())
    );
    let tools: Vec<&str> = run.requests[0]
        .tools
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        tools.contains(&"read_file") && tools.contains(&"web_fetch"),
        "{tools:?}"
    );
    assert!(
        run.state
            .journal
            .last()
            .unwrap()
            .text
            .contains("1 in the repo and 2 on the web")
    );
}

#[tokio::test]
async fn without_web_tools_only_the_repo_is_explored() {
    let script = vec![repo_answer()];
    let run = run(state(), With::Agent, script).await;
    assert_eq!(run.outcome, NodeOutcome::Done);
    assert_eq!(run.requests.len(), 1);
    assert_eq!(run.options.unwrap().len(), 1);
    assert!(
        run.state.journal[0]
            .text
            .contains("the web was not searched")
    );
}

#[tokio::test]
async fn without_an_agent_nothing_is_searched() {
    let run = run(state(), With::Nothing, vec![]).await;
    assert_eq!(run.outcome, NodeOutcome::Done);
    assert!(run.requests.is_empty());
    assert!(run.options.unwrap().is_empty());
    let journal: Vec<&str> = run.state.journal.iter().map(|e| e.text.as_str()).collect();
    assert!(
        journal[0].contains("the repo was not explored"),
        "{journal:?}"
    );
    assert!(
        journal[1].contains("the web was not searched"),
        "{journal:?}"
    );
}

#[tokio::test]
async fn goes_back_without_a_problem_definition() {
    let run = run(ResearchState::default(), With::Agent, vec![]).await;
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
