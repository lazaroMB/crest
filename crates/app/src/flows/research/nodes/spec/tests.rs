use std::sync::Arc;

use domain::block::{BlockKind, ChartKind, ChartSpec, Series, SourceRef};
use domain::model::ModelPolicy;
use domain::research::{
    ConstraintKind, ResearchState, Stage, TestKind, TestRef, extract_citations,
};
use serde_json::{Value, json};

use super::render::SECTIONS;
use super::sink::MemorySpecSink;
use super::*;
use crate::flows::research::node::NodeDeps;
use crate::ports::events::DomainEvent;
use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
use crate::testing::system::RecordingSink;

fn state(problems: &[&str]) -> ResearchState {
    let mut s = ResearchState::default();
    let e1 = s.add_evidence(
        "Login takes 500 ms at 500 rps",
        SourceRef::Command {
            command: "make bench-login".into(),
            output_excerpt: "p95 500ms".into(),
        },
        1,
    );
    s.add_evidence(
        "Every login reads the session from SQLite",
        SourceRef::File {
            path: "crates/app/src/ports/store.rs".into(),
            line_start: Some(10),
            line_end: Some(20),
        },
        2,
    );
    s.add_evidence(
        "Uncited evidence",
        SourceRef::Url {
            url: "https://example.com".into(),
            title: None,
        },
        3,
    );
    s.add_hypothesis(
        "An in-memory cache brings p95 down to 20 ms",
        vec!["reads".into()],
        1,
    );
    s.add_constraint(
        ConstraintKind::Performance,
        "p95 < 20 ms at 500 rps",
        Some(TestRef {
            kind: TestKind::Command,
            reference: "make bench-login".into(),
        }),
    );
    s.add_decision(
        "LRU cache",
        vec!["Redis".into()],
        "no extra services",
        vec![e1],
    );
    s.problem_statements = problems.iter().map(|p| (*p).to_owned()).collect();
    s
}

fn answer(title: &str, background: &str) -> Value {
    json!({
        "title": title,
        "problem": "At 500 rps login goes from 10 ms to 500 ms [E1].",
        "background": background,
        "hypothesis": "An in-memory cache brings p95 down to 20 ms.",
        "proposal": "LRU cache in front of the store [E2].",
        "black_box": "```mermaid\nflowchart LR\n  login --> cache --> store\n```",
        "functional": "Login checks the cache before the store.",
        "sequence": "sequenceDiagram\n  login->>cache: get",
        "states": "stateDiagram-v2\n  [*] --> empty",
        "technical": "`SessionCache` in `crates/app/src/ports/store.rs`.",
        "test_protocol": "`make bench-login` must give p95 < 20 ms.",
        "out_of_scope": "Distributed cache.",
        "risks": "Invalidation on logout."
    })
}

fn good(title: &str) -> ScriptedResponse {
    ScriptedResponse::tool_use(
        "u",
        "submit_spec",
        answer(title, "Every login reads SQLite [E2] and is slow [E1]."),
    )
}

fn context(llm: Arc<ScriptedLlm>, events: Arc<RecordingSink>, state: ResearchState) -> NodeContext {
    let deps = NodeDeps::new(llm, ModelPolicy::default(), events).with_clock(|| 1_000);
    NodeContext::new(Stage::Spec, deps, "login is slow", state)
}

fn blocks(events: &RecordingSink) -> Vec<BlockKind> {
    events
        .events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::BlockAdded { block } => Some(block.kind),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn writes_the_spec_folder_with_ten_sections_and_known_citations() {
    let llm = Arc::new(ScriptedLlm::new([good("Session cache")]));
    let events = Arc::new(RecordingSink::new());
    let files = Arc::new(MemorySpecSink::new().with_folder("0003-other"));
    let mut ctx = context(
        Arc::clone(&llm),
        Arc::clone(&events),
        state(&["slow login"]),
    );
    ctx.set_data(
        CHARTS_KEY,
        vec![ChartSpec {
            title: "Latency".into(),
            kind: ChartKind::Line,
            labels: vec!["100".into(), "500".into()],
            series: vec![Series {
                name: "p95".into(),
                values: vec![10.0, 500.0],
            }],
        }],
    )
    .unwrap();

    let outcome = Spec::with_sink(files.clone()).run(&mut ctx).await.unwrap();
    assert_eq!(outcome, NodeOutcome::Done);

    let folder = "0004-session-cache";
    let md = files.file(folder, "spec.md").unwrap();
    assert!(md.starts_with("# Spec 0004: Session cache\n"));
    let mut at = 0;
    for section in SECTIONS {
        let found = md[at..]
            .find(&format!("\n## {section}\n"))
            .unwrap_or_else(|| panic!("missing {section}:\n{md}"));
        at += found + 1;
    }
    assert!(md.contains("```mermaid\nflowchart LR"));
    assert!(md.contains("```mermaid\nsequenceDiagram"));
    assert!(md.contains("```mermaid\nstateDiagram-v2"));
    assert!(md.contains("| C1 | Performance | p95 < 20 ms at 500 rps | `make bench-login` |"));
    let ids = extract_citations(&md);
    ctx.state().cites_only_known_evidence(&ids).unwrap();
    assert!(md.contains(
        "- **[E2]** Every login reads the session from SQLite: `crates/app/src/ports/store.rs:10-20`"
    ));
    assert!(!md.contains("Uncited evidence"));

    let sources: Vec<domain::research::Evidence> =
        serde_json::from_str(&files.file(folder, "sources.json").unwrap()).unwrap();
    let cited: Vec<String> = sources.iter().map(|e| e.id.to_string()).collect();
    assert_eq!(cited, ["E1", "E2"]);
    let journal = files.file(folder, "journal.md").unwrap();
    assert!(journal.contains("## 12. Spec") && journal.contains("0004-session-cache"));
    assert_eq!(
        files
            .file(folder, "evidence/chart-1-latency.csv")
            .as_deref(),
        Some("label,p95\n100,10\n500,500\n")
    );
    assert!(
        md.contains("### Charts\n\n- **Latency**: data in [`evidence/chart-1-latency.csv`](evidence/chart-1-latency.csv)"),
        "{md}"
    );

    let kinds = blocks(&events);
    assert!(
        matches!(&kinds[0], BlockKind::Markdown { title: Some(t), source } if t == "0004-session-cache/spec.md" && source == &md)
    );
    assert!(matches!(&kinds[1], BlockKind::Sources { sources } if sources.len() == 2));

    let req = llm.last_request().unwrap();
    assert_eq!(
        req.tool_choice,
        crate::ports::llm::ToolChoice::Tool("submit_spec".into())
    );
    let drafts: Vec<SpecDraft> = ctx.data(DRAFTS_KEY).unwrap();
    assert_eq!(drafts[0].folder, folder);
    assert_eq!(drafts[0].spec_md, md);
}

#[tokio::test]
async fn unknown_citations_are_asked_again_once_then_fail() {
    let bad = || {
        ScriptedResponse::tool_use(
            "u",
            "submit_spec",
            answer("X", "According to [E9] it is slow."),
        )
    };
    let llm = Arc::new(ScriptedLlm::new([bad(), bad()]));
    let files = Arc::new(MemorySpecSink::new());
    let mut ctx = context(
        Arc::clone(&llm),
        Arc::new(RecordingSink::new()),
        state(&["p"]),
    );
    let err = Spec::with_sink(files.clone())
        .run(&mut ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, NodeError::Failed(m) if m.contains("E9")),
        "{err}"
    );
    let requests = llm.requests();
    assert_eq!(requests.len(), 2);
    assert!(format!("{:?}", requests[1].messages[0]).contains("E9"));
    assert!(files.paths().is_empty());
}

#[tokio::test]
async fn a_retry_with_valid_citations_succeeds() {
    let bad = ScriptedResponse::tool_use(
        "u",
        "submit_spec",
        answer("X", "According to [E9] it is slow."),
    );
    let llm = Arc::new(ScriptedLlm::new([bad, good("Cache")]));
    let files = Arc::new(MemorySpecSink::new());
    let mut ctx = context(llm, Arc::new(RecordingSink::new()), state(&["p"]));
    Spec::with_sink(files.clone()).run(&mut ctx).await.unwrap();
    assert!(files.file("0001-cache", "spec.md").is_some());
    assert!(
        ctx.state()
            .journal
            .iter()
            .any(|e| e.text.contains("asking again"))
    );
}

#[tokio::test]
async fn two_problem_statements_make_two_folders() {
    let llm = Arc::new(ScriptedLlm::new([
        good("Session cache"),
        good("User index"),
    ]));
    let files = Arc::new(MemorySpecSink::new().with_folder("0007-old"));
    let mut ctx = context(
        Arc::clone(&llm),
        Arc::new(RecordingSink::new()),
        state(&["slow login", "slow search"]),
    );
    Spec::with_sink(files.clone()).run(&mut ctx).await.unwrap();
    assert!(files.file("0008-session-cache", "spec.md").is_some());
    assert!(
        files
            .file("0009-user-index", "spec.md")
            .unwrap()
            .starts_with("# Spec 0009")
    );
    let first = format!("{:?}", llm.requests()[0].messages[0]);
    assert!(
        first.contains("slow search"),
        "the other problem is out of scope"
    );

    // A rerun keeps the folders of the same problems.
    llm.push(good("Another title"));
    llm.push(good("One more"));
    Spec::with_sink(files.clone()).run(&mut ctx).await.unwrap();
    let drafts: Vec<SpecDraft> = ctx.data(DRAFTS_KEY).unwrap();
    let folders: Vec<&str> = drafts.iter().map(|d| d.folder.as_str()).collect();
    assert_eq!(folders, ["0008-session-cache", "0009-user-index"]);
}

#[tokio::test]
async fn missing_inputs_go_back() {
    let llm = Arc::new(ScriptedLlm::new([]));
    let cases = [
        (state(&[]), Stage::Definition),
        (
            ResearchState {
                problem_statements: vec!["p".into()],
                ..ResearchState::default()
            },
            Stage::Observation,
        ),
        (
            {
                let mut s = state(&["p"]);
                s.decisions.clear();
                s
            },
            Stage::Modeling,
        ),
    ];
    for (s, stage) in cases {
        let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), s);
        let outcome = Spec::new().run(&mut ctx).await.unwrap();
        assert!(matches!(outcome, NodeOutcome::Back { to, .. } if to == stage));
    }
}

#[test]
fn recorded_charts_replace_the_ones_with_their_title() {
    let deps = NodeDeps::new(
        Arc::new(ScriptedLlm::new([])),
        ModelPolicy::default(),
        Arc::new(RecordingSink::new()),
    );
    let ctx = NodeContext::new(Stage::Analysis, deps, "p", ResearchState::default());
    let chart = |title: &str, value: f64| ChartSpec {
        title: title.into(),
        kind: ChartKind::Bar,
        labels: vec!["a".into()],
        series: vec![Series {
            name: "s".into(),
            values: vec![value],
        }],
    };
    record_charts(&ctx, &[chart("p95", 1.0), chart("commits", 2.0)]).unwrap();
    record_charts(&ctx, &[chart("p95", 3.0)]).unwrap();
    let all: Vec<ChartSpec> = ctx.data(CHARTS_KEY).unwrap();
    assert_eq!(all, [chart("p95", 3.0), chart("commits", 2.0)]);
}
