//! The engine end to end over the in-memory fakes and a scripted model.

use std::sync::Arc;

use domain::discovery::{
    Actor, DiscoveryEvent, Level, Logged, RecordRef, RecordVerdict, Section, SourceId, SourceKind,
    Status, Verdict,
};
use domain::model::ModelPolicy;

use super::audit::AuditReport;
use super::engine::{
    DiscoveryDeps, DiscoveryEngine, correction_for, fix_rounds, model_rejects, reject_note,
};
use super::model::Model;
use super::records::{ClaimRecord, EvidenceRecord, names};
use super::session::DiscoverySession;
use crate::flows::research::nodes::spec::sink::MemorySpecSink;
use crate::ports::discovery::{DiscoveryId, DiscoveryLogPort, SourceStorePort, SourceText};
use crate::testing::discovery::{MemoryDiscoveryLog, MemorySourceStore};
use crate::testing::llm::{RoutedLlm, ScriptedResponse};
use crate::testing::system::RecordingSink;

const GOAL: &str = "Enable developers to resume a cancelled prototype step, providing a clean worktree every time.";
const BACKGROUND: &str = "The research flow runs fourteen nodes on graph-flow, one checkpoint per node.\n\nThe Prototype node creates a git worktree for its spike and removes it when the step completes.";
const CONSTRAINTS: &str = "- The domain gains no I/O.\n- Only MIT-compatible licenses.";
const SCOPE: &str =
    "### In\n- The Prototype node and its worktree.\n\n### Out\n- The other thirteen nodes.";
const FINDINGS: &str = "#### Behaviour\n- The worktree is created before the checkpoint that records it [E1].\n- A cancelled step leaves the branch behind [E1].";
const BREAKDOWN: &str = "1. The worktree outlives a cancelled step.\n2. A resumed step cannot find the worktree it left.";
const SOLUTION: &str = "### Proposed approach\n\nRecord the worktree in the log before creating it, so a resume can clean it up first.\n\n```mermaid\nflowchart LR\n  start --> record --> create\n```\n\n### 1. The worktree outlives a cancelled step\n\nAppend the start event before `git worktree add` and clean up on resume [E1].\n\n### 2. A resumed step cannot find the worktree it left\n\nRead the worktree path from the log instead of the node data [E1].\n\n### Future work\n\nOne worktree per discovery.";

struct Harness {
    llm: Arc<RoutedLlm>,
    log: Arc<MemoryDiscoveryLog>,
    sources: Arc<MemorySourceStore>,
    sink: Arc<MemorySpecSink>,
    events: Arc<RecordingSink>,
    engine: Arc<DiscoveryEngine>,
}

fn harness() -> Harness {
    let llm = Arc::new(
        RoutedLlm::new()
            .route("section `Goal`", [ScriptedResponse::text(GOAL)])
            .route("section `Background`", [ScriptedResponse::text(BACKGROUND)])
            .route(
                "section `Constraints`",
                [ScriptedResponse::text(CONSTRAINTS)],
            )
            .route("section `Scope`", [ScriptedResponse::text(SCOPE)])
            .route("section `Findings`", [ScriptedResponse::text(FINDINGS)])
            .route("section `Breakdown`", [ScriptedResponse::text(BREAKDOWN)])
            .route(
                "section `Solution proposal`",
                [ScriptedResponse::text(SOLUTION)],
            )
            .route("isolated reviewer", [])
            .route("isolated evaluator", [])
            .route("plan.json", []),
    );
    let log = Arc::new(MemoryDiscoveryLog::new());
    let sources = Arc::new(MemorySourceStore::new());
    let sink = Arc::new(MemorySpecSink::new());
    let events = Arc::new(RecordingSink::new());
    let model = Model::new(llm.clone(), ModelPolicy::default(), events.clone());
    let deps = DiscoveryDeps::new(log.clone(), sources.clone(), model, events.clone())
        .with_sink(sink.clone())
        .with_workspace_root("/ws")
        .with_human("alex")
        .with_clock(|| 42);
    let engine = Arc::new(DiscoveryEngine::new(deps));
    Harness {
        llm,
        log,
        sources,
        sink,
        events,
        engine,
    }
}

/// Stores one source and one confirmed piece of evidence, as an
/// exploration would.
async fn seed_evidence(h: &Harness, id: &DiscoveryId) {
    let stored = h
        .sources
        .put(
            id,
            SourceText {
                location: "crates/app/src/flows/research/nodes/prototype.rs".into(),
                title: Some("prototype.rs".into()),
                kind: SourceKind::Repo,
                text: "the worktree is created before the checkpoint that records it; a cancelled step leaves the branch behind".into(),
                fetched_at: 1,
            },
        )
        .await
        .unwrap();
    h.log
        .append(
            id,
            DiscoveryEvent::SourceFetched {
                source: stored.id,
                hash: stored.hash,
                location: stored.source.location,
                kind: SourceKind::Repo,
            },
            2,
        )
        .await
        .unwrap();
    let evidence = vec![EvidenceRecord {
        id: 1,
        claim: "the worktree is created before the checkpoint".into(),
        source: SourceId(1),
        span: "the worktree is created before the checkpoint that records it".into(),
    }];
    h.log
        .put_artifact(
            id,
            names::EVIDENCE,
            &serde_json::to_string(&evidence).unwrap(),
        )
        .await
        .unwrap();
    h.log
        .append(
            id,
            DiscoveryEvent::RecordChecked {
                record: RecordRef::Evidence { id: 1 },
                verdict: RecordVerdict::Confirmed,
                by: Actor::Process("span-check".into()),
            },
            3,
        )
        .await
        .unwrap();
}

fn plan_json(folder: &str) -> String {
    serde_json::json!({
        "spec": folder,
        "tasks": [{
            "id": "T1",
            "title": "Record the worktree before creating it",
            "description": "Append SpikeStarted before git worktree add.",
            "depends_on": [],
            "files": ["crates/app/src/flows/discovery/"],
            "acceptance": [{"kind": "test", "ref": "cargo test -p application discovery"}],
            "role": "main",
            "risk": "low"
        }],
        "hypothesis_tests": [],
        "constraint_tests": [
            {"constraint": "C1", "kind": "command", "ref": "bash scripts/check-arch.sh"},
            {"constraint": "C2", "kind": "command", "ref": "cargo deny check"}
        ]
    })
    .to_string()
}

#[tokio::test]
async fn a_research_discovery_runs_from_start_to_accept() {
    let h = harness();
    let mut session = DiscoverySession::new(h.engine.clone());
    let first = session
        .start(
            "the research flow leaves a worktree behind",
            Level::Research,
        )
        .await
        .unwrap();
    let id = session.active().cloned().unwrap();
    assert!(id.as_str().starts_with("0001-the-research-flow"), "{id}");
    assert!(
        first.contains("`Goal` written and audited: 0 error(s)"),
        "{first}"
    );
    assert!(first.contains("Answer `yes` to approve"), "{first}");
    assert!(session.is_active());
    assert_eq!(session.gate_info().unwrap().stage, "Goal");

    // The research level audits deterministically: no reviewer call.
    assert!(h.llm.requests("isolated reviewer").is_empty());

    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("`Goal` approved by human:alex"), "{out}");
    assert!(out.contains("`Background` written"), "{out}");
    let out = session.handle_line("ok").await.unwrap();
    // Constraints is audit-only: approved by the process, Scope follows.
    assert!(
        out.contains("`Constraints` approved by process:audit"),
        "{out}"
    );
    assert!(out.contains("`Scope` written"), "{out}");
    // Seed what Findings will cite before approving Scope.
    seed_evidence(&h, &id).await;
    let out = session
        .handle_line("yes, but keep the Out list short")
        .await
        .unwrap();
    assert!(
        out.contains("`Scope` approved by human:alex with a note: keep the Out list short"),
        "{out}"
    );
    assert!(
        out.contains("`Findings` approved by process:audit"),
        "{out}"
    );
    assert!(out.contains("`Breakdown` written"), "{out}");
    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("`Solution proposal` written"), "{out}");
    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("every section is approved"), "{out}");
    assert_eq!(session.gate_info().unwrap().stage, "accept");

    h.llm
        .push("plan.json", ScriptedResponse::text(plan_json(id.as_str())));
    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("accepted"), "{out}");
    assert!(!session.is_active());

    let loaded = h.engine.load(&id).await.unwrap();
    assert_eq!(loaded.state.status, Status::Accepted);
    assert_eq!(loaded.state.approved.len(), 7);
    assert_eq!(loaded.state.refusals, 0);
    let scope = &loaded.state.approved[&Section::Scope];
    assert_eq!(scope.choice, Verdict::ApproveWithNotes);
    assert_eq!(scope.by, Actor::Human("alex".into()));

    let paths = h.sink.paths();
    for file in [
        "discovery.md",
        "events.jsonl",
        "spec.md",
        "plan.json",
        "plan.md",
        "review.md",
        "sources.json",
    ] {
        assert!(
            paths.contains(&format!("{id}/{file}")),
            "{file} missing in {paths:?}"
        );
    }
    let spec = h.sink.file(id.as_str(), "spec.md").unwrap();
    assert!(spec.contains("## 1. Problem"));
    assert!(spec.contains("## 7. Constraints\n\n| Id | Kind | Constraint | Test |"));
    assert!(spec.contains("| C1 | architecture | The domain gains no I/O. |"));
    assert!(spec.contains("## 10. Sources\n\n- **[E1]**"));
    let doc = h.sink.file(id.as_str(), "discovery.md").unwrap();
    assert!(doc.contains("status: accepted · 7 of 7 sections approved"));
    assert!(doc.contains("## Scope\n\n_approved by human:alex at seq"));
    // The writer never saw a whole source: only the span excerpt.
    let findings_request = h.llm.requests("section `Findings`").pop().unwrap();
    let prompt = crate::flows::research::json::text_of(&findings_request.messages[0].content);
    assert!(prompt.contains("- [E1] evidence (from [S1])"));
    assert!(prompt.contains("- [S1] repo source, prototype.rs"));
    assert!(
        h.events.kinds().iter().any(|k| k == "block_added"),
        "{:?}",
        h.events.kinds()
    );
}

#[tokio::test]
async fn a_rejection_sends_the_note_back_to_the_writer() {
    let h = harness();
    h.llm.push("section `Goal`", ScriptedResponse::text(GOAL));
    let mut session = DiscoverySession::new(h.engine.clone());
    session
        .start("slow sessions", Level::Research)
        .await
        .unwrap();
    let id = session.active().cloned().unwrap();
    let out = session
        .handle_line("name the developers, not the users")
        .await
        .unwrap();
    assert!(
        out.contains("`Goal` rejected by human:alex: name the developers, not the users"),
        "{out}"
    );
    assert!(out.contains("`Goal` written and audited"), "{out}");
    let requests = h.llm.requests("section `Goal`");
    assert_eq!(requests.len(), 2);
    let second = crate::flows::research::json::text_of(&requests[1].messages[0].content);
    assert!(second.contains("# Correction\n\nThe previous draft was not accepted. Fix exactly this:\n\nname the developers, not the users"), "{second}");
    let loaded = h.engine.load(&id).await.unwrap();
    assert_eq!(
        reject_note(&loaded.events, Section::Goal).as_deref(),
        Some("name the developers, not the users"),
        "the note stands until the next verdict"
    );
    assert!(session.is_active());
}

#[tokio::test]
async fn reopen_drops_later_verdicts_and_rewrites() {
    let h = harness();
    h.llm.push("section `Goal`", ScriptedResponse::text(GOAL));
    let mut session = DiscoverySession::new(h.engine.clone());
    session
        .start("slow sessions", Level::Research)
        .await
        .unwrap();
    session.handle_line("yes").await.unwrap();
    session.handle_line("yes").await.unwrap();
    let id = session.active().cloned().unwrap();
    let before = h.engine.load(&id).await.unwrap();
    assert_eq!(
        before.state.approved.len(),
        3,
        "{:?}",
        before.state.approved.keys()
    );
    let out = session.reopen(Section::Goal, "wrong actor").await.unwrap();
    assert!(
        out.contains("verdicts dropped: Goal, Background, Constraints"),
        "{out}"
    );
    assert!(out.contains("`Goal` written and audited"), "{out}");
    let after = h.engine.load(&id).await.unwrap();
    assert!(after.state.approved.is_empty());
    assert_eq!(after.state.reopens, 1);
}

#[tokio::test]
async fn abort_freezes_and_resume_finds_the_pending_one() {
    let h = harness();
    let mut session = DiscoverySession::new(h.engine.clone());
    session
        .start("slow sessions", Level::Research)
        .await
        .unwrap();
    let id = session.active().cloned().unwrap();
    let mut fresh = DiscoverySession::new(h.engine.clone());
    assert!(
        fresh
            .status()
            .await
            .unwrap()
            .contains("`Goal` waits for a verdict")
    );
    let resumed = fresh.resume(None).await.unwrap();
    assert!(resumed.contains("Answer `yes` to approve"), "{resumed}");
    assert!(fresh.is_active());
    let out = fresh.abort().await.unwrap();
    assert!(out.contains("aborted"));
    let loaded = h.engine.load(&id).await.unwrap();
    assert_eq!(loaded.state.status, Status::Aborted);
    assert!(matches!(
        session.handle_line("yes").await.unwrap_err(),
        super::error::DiscoveryError::NotWaiting(_)
    ));
    assert!(fresh.pending().await.unwrap().is_none());
}

fn logged(seq: u64, event: DiscoveryEvent) -> Logged {
    Logged::new(seq, seq, event)
}

#[test]
fn round_counters_read_the_trail() {
    let s = Section::Findings;
    let audited = |seq, errors| {
        logged(
            seq,
            DiscoveryEvent::Audited {
                section: s,
                errors,
                warnings: 0,
                isolated: true,
                by: Actor::Process("audit".into()),
            },
        )
    };
    let verdict = |seq, choice, by: Actor| {
        logged(
            seq,
            DiscoveryEvent::Verdict {
                section: s,
                choice,
                by,
                note: Some("n".into()),
            },
        )
    };
    let model = Actor::Model("m".into());
    let events = vec![
        audited(1, 2),
        audited(2, 1),
        verdict(3, Verdict::RejectAndRevise, model.clone()),
        audited(4, 0),
        verdict(5, Verdict::RejectAndRevise, model.clone()),
        audited(6, 3),
    ];
    assert_eq!(fix_rounds(&events, s), 1);
    assert_eq!(fix_rounds(&events[..2], s), 2);
    assert_eq!(model_rejects(&events, s), 2);
    // A rejection note stands through the rewrites until the next verdict.
    assert_eq!(reject_note(&events, s).as_deref(), Some("n"));
    let mut with_write = events.clone();
    with_write.push(verdict(
        7,
        Verdict::RejectAndRevise,
        Actor::Human("a".into()),
    ));
    assert_eq!(reject_note(&with_write, s).as_deref(), Some("n"));
    with_write.push(logged(
        8,
        DiscoveryEvent::SectionWritten {
            section: s,
            hash: "h".into(),
        },
    ));
    assert_eq!(reject_note(&with_write, s).as_deref(), Some("n"));
    assert_eq!(model_rejects(&with_write, s), 2);
    with_write.push(verdict(9, Verdict::ApproveAsIs, model));
    assert_eq!(reject_note(&with_write, s), None);
    assert_eq!(model_rejects(&with_write, s), 0);
}

#[test]
fn a_rewrite_carries_the_rejection_note_and_the_audit_findings_together() {
    // The real order at a FixAudit step: the person rejects, the rewrite
    // is written and fails its audit; the next rewrite needs both.
    let s = Section::Findings;
    let audited = |seq| {
        logged(
            seq,
            DiscoveryEvent::Audited {
                section: s,
                errors: 1,
                warnings: 0,
                isolated: false,
                by: Actor::Process("audit".into()),
            },
        )
    };
    let events = vec![
        audited(1),
        logged(
            2,
            DiscoveryEvent::Verdict {
                section: s,
                choice: Verdict::RejectAndRevise,
                by: Actor::Human("alex".into()),
                note: Some("drop the second category".into()),
            },
        ),
        logged(
            3,
            DiscoveryEvent::SectionWritten {
                section: s,
                hash: "h2".into(),
            },
        ),
        audited(4),
    ];
    let report = AuditReport {
        section: s,
        findings: vec![domain::discovery::Finding {
            severity: domain::discovery::Severity::Error,
            message: "no citation in: \"x\"".into(),
        }],
        reviewer: None,
        by: Actor::Process("audit".into()),
    };
    assert_eq!(
        correction_for(&events, s, Some(&report)).as_deref(),
        Some("drop the second category\n\n- Error: no citation in: \"x\"")
    );
    assert_eq!(
        correction_for(&events, s, None).as_deref(),
        Some("drop the second category")
    );
    assert_eq!(correction_for(&events[..1], s, None), None);
    assert_eq!(
        correction_for(&events[..1], s, Some(&report)).as_deref(),
        Some("- Error: no citation in: \"x\"")
    );
}

// ---------------------------------------------------------------------
// The deep level: gates, explorer, records, risks, spike, evaluator.
// ---------------------------------------------------------------------

use std::sync::Mutex;

use domain::discovery::{SpikeId, SpikeScore};
use domain::permission::PermissionPolicy;

use crate::ports::approval::ApprovalResponse;
use crate::ports::shell::CommandOutput;
use crate::ports::web::WebPage;
use crate::testing::fs::{InMemoryFs, ScriptedShell, test_root};
use crate::testing::system::{RecordingTelemetry, ScriptedApproval};
use crate::testing::web::ScriptedWeb;
use crate::tools::registry::ToolRegistry;
use crate::usecases::run_turn::{AgentDeps, AgentService};

const STUDY_URL: &str = "https://example.org/habituation-study";
const STUDY_TEXT: &str = "Habituation to security warnings. In the study, more prompts per hour lowered attention per prompt, and lower attention per prompt raised the rate of unsafe approvals. Unsafe approvals in turn led teams to add more prompts per hour.";
const GATE_FILE: &str = "crates/app/src/approval.rs";
const GATE_TEXT: &str =
    "// The approval gate.\n// Every edit and every command asks for approval before it runs.\n";

const D_GOAL: &str = "Enable developers to keep their attention on the approvals that matter, providing fewer unsafe approvals.";
const D_BACKGROUND: &str = "crest asks before every edit and every command.\n\nA long session shows dozens of prompts and the person stops reading them.";
const D_CONSTRAINTS: &str = "- The domain gains no I/O.\n- Only MIT-compatible licenses.";
const D_SCOPE: &str =
    "### In\n- The approval gate of tool calls.\n\n### Out\n- The sandbox itself.";
const D_FINDINGS: &str = "#### Habituation\n- More prompts per hour lower attention per prompt [CL1].\n- Lower attention per prompt raises unsafe approvals [CL2].\n\n#### Today\n- Every edit and every command asks before it runs [E1].";
const D_CURRENT: &str = "Every tool call asks for approval, the prompts accumulate and attention per prompt falls, which raises unsafe approvals and brings more prompts [cld].\n\n```mermaid\nflowchart LR\n  prompts --> attention --> unsafe --> prompts\n```";
const D_DESIRED: &str = "1. Fewer than one prompt per minute in a normal session.\n2. No unsafe approval in the approval log.";
const D_GAP: &str = "Today every call asks and nothing groups them, so attention falls before the risky call arrives.";
const D_BREAKDOWN: &str = "1. Prompts arrive one by one with no grouping.\n2. A risky call looks the same as a routine one.";
const D_SOLUTION: &str = "### Proposed approach\n\nGroup routine calls under one approval and keep the single prompt for risky calls, as the spike showed [SP1].\n\n```mermaid\nflowchart LR\n  call --> classify --> group --> prompt\n```\n\n### 1. Prompts arrive one by one with no grouping\n\nBatch the routine calls of a turn under one approval [risks].\n\n### 2. A risky call looks the same as a routine one\n\nMark risky calls and never batch them [SP1].\n\n### Future work\n\nA sandbox for commands.";
const D_RISKS: &str = "### Reinforcing loop R1\n\nFrom loop R1 (prompts per hour, attention per prompt, unsafe approvals): more prompts make it worse, so a solution must not add prompts.";

/// Needles of the routes the deep level adds.
const EXPLORER: &str = "Investigate the problem below for a deep discovery";
const CHECKER: &str = "You check one record";
const RISKS_WRITER: &str = "You write one record of a deep discovery";
const PROPOSER: &str = "You design one spike";
const WORKER: &str = "You run spike";
const REVIEWER: &str = "isolated reviewer";
const EVALUATOR: &str = "isolated evaluator";

fn clean_review() -> ScriptedResponse {
    ScriptedResponse::text(r#"{"errors": [], "warnings": []}"#)
}

fn evaluation(verdict: &str, note: &str) -> ScriptedResponse {
    ScriptedResponse::text(serde_json::json!({ "verdict": verdict, "note": note }).to_string())
}

fn record_ok() -> ScriptedResponse {
    ScriptedResponse::text(r#"{"ok": true, "why": ""}"#)
}

fn exploration() -> ScriptedResponse {
    ScriptedResponse::text(
        serde_json::json!({
            "sources": [
                {"location": STUDY_URL, "kind": "study", "title": "Habituation study"},
                {"location": GATE_FILE, "kind": "repo", "title": "approval.rs"}
            ],
            "structure": "The approval gate sits between the model's tool calls and the workspace; every call crosses it.",
            "variables": [
                "prompts per hour: rises with every tool call that needs approval",
                "attention per prompt: falls as prompts accumulate",
                "unsafe approvals: approvals given without reading"
            ],
            "claims": [
                {"origin": "prompts per hour", "destination": "attention per prompt",
                 "direction": "increasing origin decreases destination", "stated_sign": "-",
                 "span": "more prompts per hour lowered attention per prompt", "source": 1},
                {"origin": "attention per prompt", "destination": "unsafe approvals",
                 "direction": "decreasing origin increases destination", "stated_sign": "-",
                 "span": "lower attention per prompt raised the rate of unsafe approvals", "source": 1},
                {"origin": "unsafe approvals", "destination": "prompts per hour",
                 "direction": "increasing origin increases destination", "stated_sign": "+",
                 "span": "led teams to add more prompts per hour", "source": 1}
            ],
            "evidence": [
                {"claim": "every edit and command asks before it runs",
                 "span": "asks for approval before it runs", "source": 2}
            ]
        })
        .to_string(),
    )
}

fn spike_proposal() -> ScriptedResponse {
    ScriptedResponse::text(
        serde_json::json!({
            "hypothesis": "Grouping routine calls under one approval halves the prompts without hiding a risky call.",
            "criteria": [
                "prompts per session drop by at least 50%",
                "every risky call still prompts on its own",
                "the grouping test passes"
            ],
            "k": 2,
            "instructions": "Build a grouping prototype in the worktree and count prompts on the fixture session."
        })
        .to_string(),
    )
}

fn spike_outcome(met: [bool; 3]) -> ScriptedResponse {
    ScriptedResponse::text(
        serde_json::json!({
            "facts": "prompts fell from 40 to 12; the risky call prompted alone; the test passed",
            "met": met
        })
        .to_string(),
    )
}

/// The writer routes of every deep section, `copies` answers each.
fn deep_writers(llm: RoutedLlm, copies: usize) -> RoutedLlm {
    [
        (Section::Goal, D_GOAL),
        (Section::Background, D_BACKGROUND),
        (Section::Constraints, D_CONSTRAINTS),
        (Section::Scope, D_SCOPE),
        (Section::Findings, D_FINDINGS),
        (Section::CurrentSystem, D_CURRENT),
        (Section::DesiredState, D_DESIRED),
        (Section::Gap, D_GAP),
        (Section::Breakdown, D_BREAKDOWN),
        (Section::SolutionProposal, D_SOLUTION),
    ]
    .into_iter()
    .fold(llm, |llm, (section, text)| {
        llm.route(
            format!("section `{section}`"),
            std::iter::repeat_n(ScriptedResponse::text(text), copies),
        )
    })
}

struct DeepHarness {
    inner: Harness,
    web: Arc<ScriptedWeb>,
    shell: Arc<ScriptedShell>,
}

/// An agent over the same scripted model, with no tools: the explorer
/// and the worker answer their JSON in one turn.
fn agent(llm: &Arc<RoutedLlm>, events: &Arc<RecordingSink>) -> Arc<AgentService> {
    let deps = AgentDeps {
        llm: Arc::clone(llm) as _,
        events: Arc::clone(events) as _,
        approval: Arc::new(ScriptedApproval::always(ApprovalResponse::Approve)),
        telemetry: Arc::new(RecordingTelemetry::new()),
    };
    let permissions = Arc::new(Mutex::new(PermissionPolicy::read_only()));
    Arc::new(
        AgentService::new(
            deps,
            ToolRegistry::new(),
            ModelPolicy::default(),
            permissions,
        )
        .with_clock(|| 42),
    )
}

/// A deep harness whose evaluator answers `evaluations` in order, whose
/// worker reports `met`, and whose `git` exits 0 when `git_ok`. Every
/// other route holds enough answers for a second pass after a reopen.
fn deep_harness(evaluations: Vec<ScriptedResponse>, met: [bool; 3], git_ok: bool) -> DeepHarness {
    // The evaluator's prompt quotes the audit ("isolated reviewer ran"),
    // so its route must come before the reviewer's.
    let llm = deep_writers(RoutedLlm::new(), 3)
        .route(EVALUATOR, evaluations)
        .route(REVIEWER, std::iter::repeat_n(clean_review(), 24))
        .route(CHECKER, std::iter::repeat_n(record_ok(), 8))
        .route(
            RISKS_WRITER,
            std::iter::repeat_n(ScriptedResponse::text(D_RISKS), 2),
        )
        .route(PROPOSER, std::iter::repeat_n(spike_proposal(), 2))
        .route(EXPLORER, [exploration()])
        .route(WORKER, std::iter::repeat_n(spike_outcome(met), 2))
        .route("plan.json", []);
    let llm = Arc::new(llm);
    let log = Arc::new(MemoryDiscoveryLog::new());
    let sources = Arc::new(MemorySourceStore::new());
    let sink = Arc::new(MemorySpecSink::new());
    let events = Arc::new(RecordingSink::new());
    let web = Arc::new(ScriptedWeb::new().on_fetch(
        STUDY_URL,
        WebPage {
            url: STUDY_URL.into(),
            title: Some("Habituation study".into()),
            text: STUDY_TEXT.into(),
        },
    ));
    let fs = Arc::new(InMemoryFs::new(test_root()).with_file(GATE_FILE, GATE_TEXT));
    let shell = Arc::new(ScriptedShell::new().with_default(CommandOutput {
        status: Some(if git_ok { 0 } else { 128 }),
        ..CommandOutput::default()
    }));
    let model = Model::new(llm.clone(), ModelPolicy::default(), events.clone());
    let deps = DiscoveryDeps::new(log.clone(), sources.clone(), model, events.clone())
        .with_sink(sink.clone())
        .with_workspace_root("/ws")
        .with_human("alex")
        .with_clock(|| 42)
        .with_explorer(agent(&llm, &events))
        .with_worker(agent(&llm, &events))
        .with_fs(fs)
        .with_web(web.clone())
        .with_shell(shell.clone());
    let engine = Arc::new(DiscoveryEngine::new(deps));
    DeepHarness {
        inner: Harness {
            llm,
            log,
            sources,
            sink,
            events,
            engine,
        },
        web,
        shell,
    }
}

/// Starts a deep discovery and approves the four framing sections; the
/// fourth `yes` runs the engine through the gates. Returns the id and
/// the output of that fourth answer.
async fn through_framing(session: &mut DiscoverySession) -> (DiscoveryId, String) {
    let first = session
        .start(
            "approval fatigue: crest asks before every edit",
            Level::Deep,
        )
        .await
        .unwrap();
    assert!(
        first.contains(
            "`Goal` written and audited: 0 error(s), 0 warning(s), isolated reviewer ran"
        ),
        "{first}"
    );
    let id = session.active().cloned().unwrap();
    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("`Background` written"), "{out}");
    let out = session.handle_line("yes").await.unwrap();
    // Deep: Constraints is a person's call, not the audit's.
    assert!(out.contains("`Constraints` written"), "{out}");
    assert_eq!(session.gate_info().unwrap().stage, "Constraints");
    let out = session.handle_line("yes").await.unwrap();
    assert!(
        out.contains("`Constraints` approved by human:alex"),
        "{out}"
    );
    assert!(out.contains("`Scope` written"), "{out}");
    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("`Scope` approved by human:alex"), "{out}");
    (id, out)
}

fn args_of(call: &crate::ports::shell::RunCommandRequest) -> Vec<&str> {
    call.args.iter().map(String::as_str).collect()
}

#[tokio::test]
async fn a_deep_discovery_opens_its_gates_and_runs_to_accept() {
    let d = deep_harness(
        [
            evaluation("approve-with-notes", "mark CL2 as a study result"),
            evaluation("approve-as-is", ""),
            evaluation("approve-as-is", ""),
            evaluation("approve-as-is", ""),
            evaluation("approve-as-is", ""),
            evaluation("approve-as-is", ""),
        ]
        .into(),
        [true, true, true],
        true,
    );
    let h = &d.inner;
    let mut session = DiscoverySession::new(h.engine.clone());
    let (id, out) = through_framing(&mut session).await;

    // The problem gate: the explorer ran once, after Scope, with the framing.
    let explorer = h.llm.requests(EXPLORER);
    assert_eq!(explorer.len(), 1);
    let prompt = crate::flows::research::json::text_of(&explorer[0].messages[0].content);
    assert!(prompt.contains("# Problem\n\napproval fatigue"), "{prompt}");
    assert!(prompt.contains("## Scope\n\n### In"), "{prompt}");
    assert!(
        out.contains(
            "gate of `Findings` opened: 2 source(s), structure: Confirmed, variables: Confirmed, \
             1 evidence record(s), 3 of 3 claim(s) kept, cld: 3 edge(s), 1 loop(s)"
        ),
        "{out}"
    );
    // Sources are fetched by the engine, never taken from the explorer.
    assert_eq!(d.web.fetches(), vec![STUDY_URL.to_owned()]);
    let stored = h.sources.sources(&id).await.unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].source.kind, SourceKind::Study);
    assert_eq!(stored[1].source.kind, SourceKind::Repo);
    assert!(
        stored[1]
            .source
            .text
            .contains("asks for approval before it runs")
    );

    // Findings: isolated review, evaluator verdict with a note.
    assert!(
        out.contains(
            "`Findings` written and audited: 0 error(s), 0 warning(s), isolated reviewer ran"
        ),
        "{out}"
    );
    assert!(
        out.contains("`Findings` approved by model:")
            && out.contains("with a note: mark CL2 as a study result"),
        "{out}"
    );
    let findings = h.llm.requests("section `Findings`");
    assert_eq!(findings.len(), 1);
    let packet = crate::flows::research::json::text_of(&findings[0].messages[0].content);
    assert!(packet.contains("- [E1] evidence (from [S2])"), "{packet}");
    assert!(
        packet.contains("- [CL1] claim (from [S1]): prompts per hour → attention per prompt"),
        "{packet}"
    );
    assert!(
        packet.contains("- [S1] study source, Habituation study"),
        "{packet}"
    );
    assert!(packet.contains("- [cld] record `cld`"), "{packet}");

    // The solution gate: risks, then one spike in a worktree.
    assert!(
        out.contains("gate of `Solution proposal` opened: risks: Confirmed, SP1: Satisfactory (3 of 3 criteria)"),
        "{out}"
    );
    let worktree = format!(".crest/spikes/{id}-SP1");
    let branch = format!("spike/{id}-SP1");
    let calls = d.shell.calls();
    let args: Vec<Vec<&str>> = calls.iter().map(args_of).collect();
    assert_eq!(
        args,
        vec![
            vec![
                "worktree",
                "add",
                "-b",
                branch.as_str(),
                worktree.as_str(),
                "HEAD"
            ],
            vec!["worktree", "remove", "--force", worktree.as_str()],
            vec!["branch", "-D", branch.as_str()],
        ]
    );
    let worker = h.llm.requests(WORKER);
    assert_eq!(worker.len(), 1);
    let instructions = crate::flows::research::json::text_of(&worker[0].messages[0].content);
    assert!(
        instructions.contains("## Frozen criteria (2 of 3 must hold)"),
        "{instructions}"
    );
    assert!(
        instructions.contains(&format!("Work only inside the git worktree `{worktree}`")),
        "{instructions}"
    );
    // SpikeStarted goes to the log before the worktree exists.
    let loaded = h.engine.load(&id).await.unwrap();
    let started = loaded
        .events
        .iter()
        .position(|e| matches!(e.event, DiscoveryEvent::SpikeStarted { .. }))
        .unwrap();
    let scored = loaded
        .events
        .iter()
        .position(|e| matches!(e.event, DiscoveryEvent::SpikeScored { .. }))
        .unwrap();
    assert!(started < scored);
    assert_eq!(
        loaded.state.spikes[&SpikeId(1)].score.map(|(s, _)| s),
        Some(SpikeScore::Satisfactory)
    );

    // Every section approved; the evaluator approved the six it owns.
    assert!(out.contains("every section is approved"), "{out}");
    assert_eq!(session.gate_info().unwrap().stage, "accept");
    assert_eq!(loaded.state.approved.len(), 10);
    for s in [
        Section::Goal,
        Section::Background,
        Section::Constraints,
        Section::Scope,
    ] {
        assert!(loaded.state.approved[&s].by.is_human(), "{s}");
    }
    for s in [
        Section::Findings,
        Section::CurrentSystem,
        Section::DesiredState,
        Section::Gap,
        Section::Breakdown,
        Section::SolutionProposal,
    ] {
        assert!(
            matches!(loaded.state.approved[&s].by, Actor::Model(_)),
            "{s}"
        );
    }
    assert_eq!(
        loaded.state.approved[&Section::Findings].choice,
        Verdict::ApproveWithNotes
    );
    assert_eq!(h.llm.requests(EVALUATOR).len(), 6);
    assert_eq!(h.llm.requests(REVIEWER).len(), 10);
    assert_eq!(h.llm.requests(CHECKER).len(), 3);
    for record in [
        RecordRef::Structure,
        RecordRef::Variables,
        RecordRef::Cld,
        RecordRef::Risks,
        RecordRef::Claim { id: 1 },
        RecordRef::Claim { id: 2 },
        RecordRef::Claim { id: 3 },
        RecordRef::Evidence { id: 1 },
    ] {
        assert_eq!(
            loaded.state.records[&record].verdict,
            RecordVerdict::Confirmed,
            "{record}"
        );
    }
    assert_eq!(loaded.state.refusals, 0);

    // Accept: the derived files, the spike and the sources are in the folder.
    h.llm
        .push("plan.json", ScriptedResponse::text(plan_json(id.as_str())));
    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("accepted"), "{out}");
    let paths = h.sink.paths();
    for file in [
        "discovery.md",
        "events.jsonl",
        "spec.md",
        "plan.json",
        "plan.md",
        "review.md",
        "sources.json",
        "spikes/SP1.md",
        "sources/S1.txt",
        "sources/S2.txt",
    ] {
        assert!(
            paths.contains(&format!("{id}/{file}")),
            "{file} missing in {paths:?}"
        );
    }
    let spec = h.sink.file(id.as_str(), "spec.md").unwrap();
    assert!(spec.contains("## 1. Problem"), "{spec}");
    assert!(
        spec.contains("prompts --> attention --> unsafe --> prompts"),
        "{spec}"
    );
    assert!(
        spec.contains("- **[CL1]** prompts per hour → attention per prompt"),
        "{spec}"
    );
    let doc = h.sink.file(id.as_str(), "discovery.md").unwrap();
    assert!(
        doc.contains("status: accepted · 10 of 10 sections approved"),
        "{doc}"
    );
    let spike = h.sink.file(id.as_str(), "spikes/SP1.md").unwrap();
    assert!(spike.contains("## Validated if (2 of 3)"), "{spike}");
    assert_eq!(
        h.engine.load(&id).await.unwrap().state.status,
        Status::Accepted
    );
    assert_eq!(h.log.list("/ws").await.unwrap().len(), 1);
}

#[tokio::test]
async fn two_model_rejections_hand_the_section_to_a_person() {
    let d = deep_harness(
        [
            evaluation("reject-and-revise", "cite the study for the second bullet"),
            evaluation("reject-and-revise", "still unsupported"),
            evaluation("approve-as-is", ""),
            evaluation("approve-as-is", ""),
            evaluation("approve-as-is", ""),
            evaluation("approve-as-is", ""),
            evaluation("approve-as-is", ""),
        ]
        .into(),
        [true, true, true],
        true,
    );
    let h = &d.inner;
    let mut session = DiscoverySession::new(h.engine.clone());
    let (id, out) = through_framing(&mut session).await;
    assert!(
        out.contains("`Findings` rejected by model:")
            && out.contains("cite the study for the second bullet"),
        "{out}"
    );
    assert!(
        out.contains("The evaluator rejected it 2 times; the decision is yours."),
        "{out}"
    );
    assert!(session.is_active());
    assert_eq!(session.gate_info().unwrap().stage, "Findings");
    // Three drafts: the first, and one per rejection, each carrying the note.
    let drafts = h.llm.requests("section `Findings`");
    assert_eq!(drafts.len(), 3);
    let second = crate::flows::research::json::text_of(&drafts[1].messages[0].content);
    assert!(second.contains("# Correction\n\nThe previous draft was not accepted. Fix exactly this:\n\ncite the study for the second bullet"), "{second}");
    let third = crate::flows::research::json::text_of(&drafts[2].messages[0].content);
    assert!(third.contains("still unsupported"), "{third}");
    // No second exploration: the records survive a rewrite.
    assert_eq!(h.llm.requests(EXPLORER).len(), 1);
    // Structure, variables, cld, three claims and one evidence; risks come later.
    assert_eq!(h.engine.load(&id).await.unwrap().state.records.len(), 7);

    // The person decides; the rest runs on the evaluator.
    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("`Findings` approved by human:alex"), "{out}");
    assert!(out.contains("every section is approved"), "{out}");
    let loaded = h.engine.load(&id).await.unwrap();
    assert_eq!(loaded.state.approved.len(), 10);
    assert_eq!(model_rejects(&loaded.events, Section::Findings), 0);
}

#[tokio::test]
async fn a_failed_spike_keeps_the_solution_gate_closed() {
    let d = deep_harness(
        std::iter::repeat_n(evaluation("approve-as-is", ""), 6).collect(),
        [true, false, false],
        true,
    );
    let h = &d.inner;
    let mut session = DiscoverySession::new(h.engine.clone());
    let (id, out) = through_framing(&mut session).await;
    assert!(
        out.contains("SP1: NotSatisfactory (1 of 3 criteria)"),
        "{out}"
    );
    assert!(
        out.contains("gate of `Solution proposal` closed: at least one spike scored satisfactory"),
        "{out}"
    );
    assert!(
        !session.is_active(),
        "a closed gate stops the engine without a question"
    );
    let loaded = h.engine.load(&id).await.unwrap();
    assert_eq!(loaded.state.approved.len(), 9);
    assert_eq!(
        loaded.state.spikes[&SpikeId(1)].score.map(|(s, _)| s),
        Some(SpikeScore::NotSatisfactory)
    );
    // The worktree was still cleaned up.
    let calls = d.shell.calls();
    let args: Vec<Vec<&str>> = calls.iter().map(args_of).collect();
    assert_eq!(args[1][..3], ["worktree", "remove", "--force"]);
    assert!(h.llm.requests("section `Solution proposal`").is_empty());
    let status = session.status().await.unwrap();
    assert!(
        status.contains("gate of `Solution proposal` closed"),
        "{status}"
    );
}

#[tokio::test]
async fn resume_cleans_an_unfinished_spike_first() {
    let d = deep_harness(Vec::new(), [true, true, true], true);
    let h = &d.inner;
    let mut session = DiscoverySession::new(h.engine.clone());
    session
        .start("approval fatigue", Level::Deep)
        .await
        .unwrap();
    let id = session.active().cloned().unwrap();
    // A crash between SpikeStarted and SpikeScored leaves this in the log.
    h.log
        .append(
            &id,
            DiscoveryEvent::SpikeStarted {
                spike: SpikeId(1),
                worktree: ".crest/spikes/old-SP1".into(),
                branch: "spike/old-SP1".into(),
            },
            5,
        )
        .await
        .unwrap();
    let mut fresh = DiscoverySession::new(h.engine.clone());
    let out = fresh.resume(None).await.unwrap();
    assert!(
        out.starts_with("cleaned up 1 unfinished spike(s)\n"),
        "{out}"
    );
    assert!(out.contains("Answer `yes` to approve"), "{out}");
    let calls = d.shell.calls();
    let args: Vec<Vec<&str>> = calls.iter().map(args_of).collect();
    assert_eq!(
        args,
        vec![
            vec!["worktree", "remove", "--force", ".crest/spikes/old-SP1"],
            vec!["branch", "-D", "spike/old-SP1"],
        ]
    );
    let loaded = h.engine.load(&id).await.unwrap();
    assert_eq!(
        loaded.state.spikes[&SpikeId(1)].score.map(|(s, _)| s),
        Some(SpikeScore::NotSatisfactory)
    );
    assert_eq!(loaded.state.unfinished_spikes().count(), 0);
}

#[tokio::test]
async fn without_an_explorer_the_problem_gate_reports_itself_closed() {
    // The research harness has no explorer, web, fs or shell.
    let h = harness();
    h.llm.push(REVIEWER, clean_review());
    for _ in 0..4 {
        h.llm.push(REVIEWER, clean_review());
    }
    let mut session = DiscoverySession::new(h.engine.clone());
    session
        .start("approval fatigue", Level::Deep)
        .await
        .unwrap();
    for _ in 0..3 {
        session.handle_line("yes").await.unwrap();
    }
    let out = session.handle_line("yes").await.unwrap();
    assert!(out.contains("gate of `Findings` closed:"), "{out}");
    assert!(
        out.contains("`structure` confirmed after the `Scope` verdict"),
        "{out}"
    );
    assert!(out.contains("no explorer agent is configured"), "{out}");
    assert!(!session.is_active());
    assert!(h.llm.requests("section `Findings`").is_empty());
}

#[tokio::test]
async fn reopening_scope_rechecks_the_stored_records_instead_of_exploring_again() {
    // Six approvals for the first pass, six for the second.
    let d = deep_harness(
        std::iter::repeat_n(evaluation("approve-as-is", ""), 12).collect(),
        [true, true, true],
        true,
    );
    let h = &d.inner;
    let mut session = DiscoverySession::new(h.engine.clone());
    let (id, out) = through_framing(&mut session).await;
    assert!(out.contains("every section is approved"), "{out}");

    let out = session
        .reopen(Section::Scope, "the sandbox belongs in scope")
        .await
        .unwrap();
    assert!(
        out.contains("verdicts dropped: Scope, Findings, Current system, Desired state, Gap, Breakdown, Solution proposal"),
        "{out}"
    );
    assert!(out.contains("`Scope` written and audited"), "{out}");
    assert_eq!(session.gate_info().unwrap().stage, "Scope");
    let out = session.handle_line("yes").await.unwrap();
    // The gate reopens on the stored records: two checker calls, no
    // explorer, no fetch, no new record.
    assert!(
        out.contains("gate of `Findings` opened: re-checked the stored records, structure: Confirmed, variables: Confirmed, 1 evidence record(s), 3 of 3 claim(s) kept, cld: 3 edge(s), 1 loop(s)"),
        "{out}"
    );
    assert_eq!(h.llm.requests(EXPLORER).len(), 1);
    assert_eq!(d.web.fetches().len(), 1);
    assert_eq!(h.sources.sources(&id).await.unwrap().len(), 2);
    // Risks and the spike are redone: they hang on the new verdicts.
    assert!(out.contains("gate of `Solution proposal` opened: risks: Confirmed, SP2: Satisfactory (3 of 3 criteria)"), "{out}");
    assert!(out.contains("every section is approved"), "{out}");
    let loaded = h.engine.load(&id).await.unwrap();
    assert_eq!(loaded.state.approved.len(), 10);
    assert_eq!(loaded.state.reopens, 1);
    assert_eq!(loaded.state.spikes.len(), 2);
    let claims: Vec<ClaimRecord> = h.engine.artifact_json(&id, names::CLAIMS).await.unwrap();
    assert_eq!(
        claims.len(),
        3,
        "a rerun never adds a second copy of a record"
    );
    let evidence: Vec<EvidenceRecord> = h.engine.artifact_json(&id, names::EVIDENCE).await.unwrap();
    assert_eq!(evidence.len(), 1);
    assert_eq!(h.llm.requests(CHECKER).len(), 6);
    assert_eq!(h.llm.requests(EVALUATOR).len(), 12);
}

#[tokio::test]
async fn a_spike_whose_worktree_cannot_be_created_does_not_run() {
    let d = deep_harness(
        std::iter::repeat_n(evaluation("approve-as-is", ""), 6).collect(),
        [true, true, true],
        false,
    );
    let h = &d.inner;
    let mut session = DiscoverySession::new(h.engine.clone());
    let (id, out) = through_framing(&mut session).await;
    assert!(
        out.contains("SP1: NotSatisfactory (0 of 3 criteria)"),
        "{out}"
    );
    assert!(out.contains("gate of `Solution proposal` closed"), "{out}");
    assert!(
        h.llm.requests(WORKER).is_empty(),
        "the worker never runs in the workspace itself"
    );
    let calls = d.shell.calls();
    let args: Vec<Vec<&str>> = calls.iter().map(args_of).collect();
    assert_eq!(args.len(), 1, "{args:?}");
    assert_eq!(args[0][..3], ["worktree", "add", "-b"]);
    let spike = h.sink.file(id.as_str(), "spikes/SP1.md").unwrap();
    assert!(
        spike.contains("the git worktree could not be created"),
        "{spike}"
    );
    assert!(
        h.events.kinds().iter().any(|k| k == "notice"),
        "{:?}",
        h.events.kinds()
    );
}

#[tokio::test]
async fn cancel_names_which_discovery_and_accept_derives_again() {
    let h = harness();
    let mut first = DiscoverySession::new(h.engine.clone());
    first.start("slow sessions", Level::Research).await.unwrap();
    let older = first.active().cloned().unwrap();
    let mut second = DiscoverySession::new(h.engine.clone());
    h.llm.push("section `Goal`", ScriptedResponse::text(GOAL));
    second.start("flaky tests", Level::Research).await.unwrap();
    let newer = second.active().cloned().unwrap();
    assert_ne!(older, newer);

    // `/research cancel 2`: the list is newest first, so 2 is the older one.
    let mut other = DiscoverySession::new(h.engine.clone());
    assert_eq!(other.cancel(Some("2")).await.unwrap(), 1);
    assert_eq!(
        h.engine.load(&older).await.unwrap().state.status,
        Status::Aborted
    );
    assert_eq!(
        h.engine.load(&newer).await.unwrap().state.status,
        Status::Open
    );
    assert_eq!(other.last(), Some(&older));
    assert!(matches!(
        other.cancel(Some("nope")).await.unwrap_err(),
        super::error::DiscoveryError::Unknown(_)
    ));
    // `all` aborts every open one and reports the count; nothing twice.
    assert_eq!(other.cancel(Some("all")).await.unwrap(), 1);
    assert_eq!(other.cancel(Some("all")).await.unwrap(), 0);
    assert_eq!(other.cancel(None).await.unwrap(), 0);

    // Accept, then accept again: the log is unchanged and the Spec is
    // derived a second time (the retry path of a failed derivation).
    h.llm.push("section `Goal`", ScriptedResponse::text(GOAL));
    let mut session = DiscoverySession::new(h.engine.clone());
    session
        .start("slow sessions", Level::Research)
        .await
        .unwrap();
    let id = session.active().cloned().unwrap();
    session.handle_line("yes").await.unwrap();
    session.handle_line("yes").await.unwrap();
    seed_evidence(&h, &id).await;
    session.handle_line("yes").await.unwrap();
    session.handle_line("yes").await.unwrap();
    session.handle_line("yes").await.unwrap();
    h.llm
        .push("plan.json", ScriptedResponse::text(plan_json(id.as_str())));
    let out = session.accept().await.unwrap();
    assert!(out.contains("accepted and frozen"), "{out}");
    assert_eq!(session.last(), Some(&id));
    let events = h.engine.load(&id).await.unwrap().events.len();
    h.llm
        .push("plan.json", ScriptedResponse::text(plan_json(id.as_str())));
    let out = session.accept().await.unwrap();
    assert!(out.contains("accepted and frozen"), "{out}");
    assert_eq!(h.engine.load(&id).await.unwrap().events.len(), events);
    assert_eq!(h.llm.requests("plan.json").len(), 2);
    assert!(h.engine.deps().lock.folders().is_empty());
}

// --- Review fixes: resume of a scored spike, duplicate sources -----------

#[tokio::test]
async fn resume_logs_the_score_a_stored_scored_spike_holds() {
    let d = deep_harness(Vec::new(), [true, true, true], true);
    let h = &d.inner;
    let mut session = DiscoverySession::new(h.engine.clone());
    session
        .start("approval fatigue", Level::Deep)
        .await
        .unwrap();
    let id = session.active().cloned().unwrap();
    h.log
        .append(
            &id,
            DiscoveryEvent::SpikeStarted {
                spike: SpikeId(1),
                worktree: ".crest/spikes/old-SP1".into(),
                branch: "spike/old-SP1".into(),
            },
            5,
        )
        .await
        .unwrap();
    // The artifact was written scored; the process died before SpikeScored.
    let mut spike = domain::discovery::Spike::frozen(
        SpikeId(1),
        "the hypothesis",
        vec!["a".into(), "b".into(), "c".into()],
        3,
    )
    .unwrap();
    spike.record("it held", vec![true, true, true]).unwrap();
    h.log
        .put_artifact(
            &id,
            &super::spikes::spike_artifact(SpikeId(1)),
            &serde_json::to_string(&spike).unwrap(),
        )
        .await
        .unwrap();
    let mut fresh = DiscoverySession::new(h.engine.clone());
    fresh.resume(None).await.unwrap();
    let loaded = h.engine.load(&id).await.unwrap();
    assert_eq!(
        loaded.state.spikes[&SpikeId(1)].score.map(|(s, _)| s),
        Some(SpikeScore::Satisfactory)
    );
}

#[tokio::test]
async fn a_source_listed_twice_is_fetched_and_stored_once() {
    let d = deep_harness(Vec::new(), [true, true, true], true);
    let h = &d.inner;
    let mut session = DiscoverySession::new(h.engine.clone());
    session.start("dup sources", Level::Deep).await.unwrap();
    let id = session.active().cloned().unwrap();
    let src = |location: &str| super::explore::ExplorerSource {
        location: location.to_owned(),
        kind: "study".into(),
        title: None,
    };
    let sources = vec![
        src(STUDY_URL),
        src(GATE_FILE),
        src(&format!("  {STUDY_URL} ")),
    ];
    let mut texts = std::collections::BTreeMap::new();
    let stored = super::explore::store_sources(&h.engine, &id, &sources, &[], &mut texts)
        .await
        .unwrap();
    assert_eq!(stored.fetched, 2);
    assert_eq!(stored.ids.len(), 3);
    assert_eq!(stored.ids[0], stored.ids[2], "the duplicate resolves to S1");
    assert_ne!(stored.ids[0], stored.ids[1]);
    assert_eq!(d.web.fetches(), vec![STUDY_URL.to_owned()]);
    assert_eq!(h.sources.sources(&id).await.unwrap().len(), 2);
    let fetched = h
        .engine
        .load(&id)
        .await
        .unwrap()
        .events
        .iter()
        .filter(|e| matches!(e.event, DiscoveryEvent::SourceFetched { .. }))
        .count();
    assert_eq!(fetched, 2);
}

// --- audit lines share writers::findings_text ----------------------------
mod audit_lines_share_findings_text {
    use domain::discovery::contract::Citable;
    use domain::discovery::{Actor, Section, check_section};

    use super::super::audit::{AuditReport, ReviewerReport};
    use super::super::writers::findings_text;

    fn report(reviewer: Option<ReviewerReport>) -> AuditReport {
        AuditReport {
            section: Section::Findings,
            findings: check_section(Section::Findings, "- uncited claim", &Citable::none()),
            reviewer,
            by: Actor::Process("audit".to_owned()),
        }
    }

    #[test]
    fn correction_and_markdown_reuse_the_writer_format() {
        let r = report(Some(ReviewerReport {
            errors: vec!["bad".to_owned()],
            warnings: vec!["meh".to_owned()],
        }));
        let text = findings_text(&r.findings);
        assert!(text.starts_with("- Error: "), "{text}");
        assert_eq!(
            r.correction(),
            format!("{text}\n- Error (reviewer): bad\n- Warning (reviewer): meh")
        );
        assert!(
            r.markdown()
                .contains(&format!("{text}\n- Error (reviewer): bad\n"))
        );
    }

    #[test]
    fn no_findings_gives_no_blank_lines() {
        let mut r = report(None);
        r.findings.clear();
        assert_eq!(r.correction(), "");
        assert!(r.markdown().ends_with(".\n"));
    }
}
