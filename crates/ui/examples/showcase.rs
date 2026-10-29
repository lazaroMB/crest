//! Scenes for the README's screenshots: the real session (timeline, prompt,
//! status bar, modals, approval dialog, help) driven by a scripted fake
//! controller, so no API key is needed.
//!
//! `cargo run -p ui --example showcase -- --scene <name>`
//!
//! Scenes: `hero` (a turn with tool calls, a diff, a chart and an answer),
//! `research` (the research flow at work), `approval` (an edit waiting for
//! the user), `help` (the help modal), `model` (a 3D model opened with
//! `/open`), `modal` (a plugin modal with rich content), `blocks` (a chart
//! and tables), `routing` (how a prompt is routed) and `technical` (a
//! statistics card with its provenance, a trace waterfall and a form that
//! waits for an answer). Each one prints `showcase: ready` once it is on
//! screen and stays open; `scripts/screenshots.sh` captures the window then.

#[path = "support/mod.rs"]
mod support;

use std::f32::consts::TAU;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use application::ports::approval::{
    ApprovalPort as _, ApprovalPreview, ApprovalRequest, ApprovalRequestId,
};
use application::ports::events::{DomainEvent, EventSink};
use application::ports::form::{FormPort as _, FormRequest, FormRequestId};
use application::testing::fs::InMemoryFs;
use application::testing::system::RecordingSink;
use domain::block::{
    Block, BlockKind, Cell, ChartKind, ChartSpec, Column, CustomNode, DiffSpec, MermaidSpec,
    Series, TableSpec, ToolStatus,
};
use domain::computed::{
    Alternative, Assumption, AssumptionStatus, Computed, ComputedBody, ComputedValue,
    ComputedWarning, Engine, GroupSummary, InputRef, Interval, Narrative, TestBody, TestKind,
    WarningSeverity,
};
use domain::conversation::{SessionId, StopReason, ToolUseId, TurnId, Usage};
use domain::devops::trace::{SERVICE, TraceFormat, TraceSpec, critical_path};
use domain::form::FormSpec;
use domain::git::GitStatus;
use domain::modal::{ModalId, ModalPosition, ModalSpec, Percent};
use domain::model::{ModelId, ModelRole};
use domain::model3d::{ModelMesh, ModelSpec, UpAxis};
use domain::permission::{PermissionMode, ToolEffect};
use domain::widget::timeline::{Span, SpanStatus, TimeUnit, TimelineRow, TimelineSpec};
use domain::workspace::WorkspaceRoot;
use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use gpui::{App, Entity};
use serde_json::json;
use ui::approval::UiApproval;
use ui::form::UiForm;
use ui::help::{CommandRow, HelpSnapshot, PluginRow, ToolRow, ToolSource};
use ui::session::{
    ModelChoice, Reply, Session, SessionController, SessionParts, TurnEnd, TurnRequest,
    prompt_block,
};

const HERO_PROMPT: &str = "add() returns the difference for negative numbers: find out why, \
                           fix it and chart how long each test takes";
const APPROVAL_PROMPT: &str = "rename add to sum everywhere";
const BLOCKS_PROMPT: &str = "summarize the last release with a chart and a table";
const ROUTING_PROMPT: &str = "draw how a prompt is routed";
const TECHNICAL_PROMPT: &str = "is the new build faster? compare the response times, show me the \
                                slowest request and ask me what to ship";
const DIFF: &str = "--- a/src/math.rs
+++ b/src/math.rs
@@ -1,7 +1,7 @@
 /// Adds two integers.
 pub fn add(a: i64, b: i64) -> i64 {
-    if b < 0 { a - b.abs() } else { a + b }
+    a + b
 }
";
const OLD_MATH: &str = "/// Adds two integers.
pub fn add(a: i64, b: i64) -> i64 {
    if b < 0 { a - b.abs() } else { a + b }
}
";
const NEW_MATH: &str = "/// Adds two integers.
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}
";
const RENAME_DIFF: &str = "--- a/src/math.rs
+++ b/src/math.rs
@@ -1,4 +1,4 @@
-/// Adds two integers.
-pub fn add(a: i64, b: i64) -> i64 {
+/// Sums two integers.
+pub fn sum(a: i64, b: i64) -> i64 {
     a + b
 }
";
const ANSWER: &str = "**Found it.** `add` special-cased negative numbers and subtracted \
`b.abs()` from `a`, so `add(5, -3)` gave `2`, but `add(-2, -3)` gave `-5 - 3`.

- The branch is gone: `add` is now `a + b`.
- A regression test covers both signs (`tests/math.rs`).
- `cargo test` passes: **24 passed**, the slowest test takes 41 ms.";
const FLOWCHART: &str = "flowchart LR
    A[Reproduce the failing test] --> B{Is the bug in add?}
    B -->|yes| C[Fix the branch on negative numbers]
    B -->|no| D[Check the callers]
    C --> E[Regression test for both signs]
    D --> E
";

/// A controller whose turns and commands only emit scripted events.
struct Fake {
    sink: Arc<RecordingSink>,
    approval: UiApproval,
    form: Arc<UiForm>,
    mode: Mutex<PermissionMode>,
}

/// Runs `script` on its own thread and answers with what it returns.
fn on_thread<T: Send + 'static>(
    script: impl FnOnce() -> T + Send + 'static,
) -> BoxFuture<'static, Option<T>> {
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        tx.send(script()).ok();
    });
    rx.map(Result::ok).boxed()
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn tool_call(name: &str, summary: &str, output: &str) -> BlockKind {
    BlockKind::ToolCall {
        tool_use_id: format!("toolu_{name}_{summary}"),
        name: name.into(),
        summary: summary.into(),
        input: json!({}),
        status: ToolStatus::Succeeded,
        output: Some(output.into()),
    }
}

fn add(sink: &RecordingSink, kind: BlockKind) {
    sink.emit(DomainEvent::BlockAdded {
        block: Block::new(kind),
    });
}

/// Streams `text` into a new text block, a few words at a time.
fn stream(sink: &RecordingSink, text: &str) {
    let block = Block::new(BlockKind::Text {
        markdown: String::new(),
    });
    let block_id = block.id;
    sink.emit(DomainEvent::BlockAdded { block });
    for chunk in text.split_inclusive(' ').collect::<Vec<_>>().chunks(4) {
        pause(25);
        sink.emit(DomainEvent::TextDelta {
            block_id,
            text: chunk.concat(),
        });
    }
}

fn start(sink: &RecordingSink, prompt: &str) -> TurnId {
    sink.emit(DomainEvent::BlockAdded {
        block: prompt_block(prompt),
    });
    let turn_id = TurnId::new();
    sink.emit(DomainEvent::TurnStarted {
        session_id: SessionId::new(),
        turn_id,
        model: ModelId::new("claude-sonnet-5-5"),
        role: ModelRole::Main,
    });
    turn_id
}

fn usage(sink: &RecordingSink, input: u64, output: u64, cost: f64) {
    let usage = Usage {
        input_tokens: input,
        output_tokens: output,
        ..Usage::default()
    };
    sink.emit(DomainEvent::UsageUpdated {
        turn: usage,
        session: usage,
        cost_usd: Some(cost),
    });
}

/// The hero turn: read, fix, test, chart, answer.
fn hero(sink: &RecordingSink, prompt: &str) {
    let turn_id = start(sink, prompt);
    add(
        sink,
        BlockKind::Thinking {
            text: "The tests fail only with negative numbers; look at add first.".into(),
            collapsed: true,
        },
    );
    add(
        sink,
        BlockKind::ToolCall {
            tool_use_id: "toolu_grep".into(),
            name: "grep".into(),
            summary: "\"fn add\" in src/".into(),
            input: json!({ "pattern": "fn add", "path": "src/" }),
            status: ToolStatus::Succeeded,
            output: Some("src/math.rs:2: pub fn add(a: i64, b: i64) -> i64 {".into()),
        },
    );
    add(
        sink,
        tool_call("read_file", "src/math.rs", "     1\t/// Adds two integers."),
    );
    add(
        sink,
        BlockKind::Diff(DiffSpec {
            path: "src/math.rs".into(),
            old_text: Some(OLD_MATH.into()),
            new_text: NEW_MATH.into(),
            unified: DIFF.into(),
            approval: None,
        }),
    );
    add(
        sink,
        tool_call("run_command", "cargo test", "test result: ok. 24 passed"),
    );
    add(
        sink,
        BlockKind::Chart(ChartSpec {
            title: "Slowest tests (ms)".into(),
            kind: ChartKind::Bar,
            labels: [
                "parse_config",
                "add_negative",
                "load_session",
                "render_board",
                "add_mixed",
            ]
            .map(String::from)
            .to_vec(),
            series: vec![Series {
                name: "ms".into(),
                values: vec![41.0, 33.0, 27.0, 18.0, 6.0],
            }],
        }),
    );
    stream(sink, ANSWER);
    usage(sink, 18_420, 1_216, 0.0735);
    sink.emit(DomainEvent::TurnFinished {
        turn_id,
        stop_reason: StopReason::EndTurn,
    });
}

/// A turn that answers with rich blocks: charts, a table, a diagram.
fn blocks(sink: &RecordingSink, prompt: &str) {
    let turn_id = start(sink, prompt);
    add(
        sink,
        tool_call("run_command", "git log v0.5.0..v0.6.0 --stat", "42 commits"),
    );
    add(
        sink,
        BlockKind::Text {
            markdown: "**v0.6.0** in numbers: 42 commits, 3 features, 1 fix.".into(),
        },
    );
    add(
        sink,
        BlockKind::Chart(ChartSpec {
            title: "Lines changed by crate".into(),
            kind: ChartKind::Bar,
            labels: ["ui", "app", "adapters", "domain", "docs"]
                .map(String::from)
                .to_vec(),
            series: vec![Series {
                name: "lines".into(),
                values: vec![2140.0, 1480.0, 760.0, 410.0, 230.0],
            }],
        }),
    );
    add(
        sink,
        BlockKind::Table(TableSpec {
            columns: vec![
                Column::new("feature"),
                Column::new("crate"),
                Column::new("tests"),
            ],
            rows: vec![
                vec![
                    Cell::Text("3D models in /open".into()),
                    Cell::Text("ui".into()),
                    Cell::Number(31.0),
                ],
                vec![
                    Cell::Text("decision model (Clef)".into()),
                    Cell::Text("app".into()),
                    Cell::Number(27.0),
                ],
                vec![
                    Cell::Text("prompt history".into()),
                    Cell::Text("ui".into()),
                    Cell::Number(9.0),
                ],
            ],
        }),
    );
    add(
        sink,
        BlockKind::Table(TableSpec {
            columns: vec![Column::new("change"), Column::new("why it matters")],
            rows: vec![
                vec![
                    Cell::Text("charts on an own renderer".into()),
                    Cell::Text(
                        "Bars keep a sane width on wide screens, sit centered in their \
                         slot and group by category; lines and areas get a crosshair \
                         with one row per series."
                            .into(),
                    ),
                ],
                vec![
                    Cell::Text("tables that wrap".into()),
                    Cell::Text(
                        "Long cells wrap instead of being cut, each row takes the height \
                         it needs and the columns share the width of the block."
                            .into(),
                    ),
                ],
                vec![
                    Cell::Text("a centered column".into()),
                    Cell::Text("Prose stays readable at ~760px; rich blocks get ~1100px.".into()),
                ],
            ],
        }),
    );
    usage(sink, 12_800, 960, 0.0526);
    sink.emit(DomainEvent::TurnFinished {
        turn_id,
        stop_reason: StopReason::EndTurn,
    });
}

/// A turn for technical work: a statistics card computed by an engine, a
/// reply that cites it through number slots, a trace waterfall and a form
/// the turn waits on.
fn technical(sink: &RecordingSink, form: &UiForm, prompt: &str) {
    let turn_id = start(sink, prompt);
    add(
        sink,
        tool_call(
            "show_dataset",
            "bench/latency.csv",
            "48 rows × 2 columns: latency_ms (numeric), build (2 levels)",
        ),
    );
    add(
        sink,
        tool_call(
            "stats_test",
            "latency_ms by build",
            "Welch two-sample t-test: t = −4.45, p < .001, d = 1.28",
        ),
    );
    let (envelope, body) = latency_card();
    add(
        sink,
        BlockKind::Computed {
            envelope: Box::new(envelope),
            body: Box::new(body),
        },
    );
    stream(
        sink,
        "Build v0.18 answers in ⟦st_bench.mean_b⟧ ms on average against \
         ⟦st_bench.mean_a⟧ ms for v0.17: Welch t(⟦st_bench.df⟧) = ⟦st_bench.statistic⟧, \
         p ⟦st_bench.p_value⟧, Cohen's d = ⟦st_bench.cohens_d⟧ (large). Variances and \
         normality hold, so the card uses Welch by default.\n\nThe slowest request of the \
         new build is below: one payment attempt failed and was retried, and the retry is \
         most of the request.",
    );
    add(
        sink,
        tool_call(
            "show_trace",
            "traces/checkout-slow.json",
            "1 trace, 9 spans, 4 services; critical path 412 ms; 1 error",
        ),
    );
    add(sink, BlockKind::Trace(Box::new(checkout_trace())));
    let spec = ship_form();
    let block = Block::new(BlockKind::Form(spec.clone()));
    let block_id = block.id;
    sink.emit(DomainEvent::BlockAdded { block });
    // The turn waits here until the form is answered, as `ask_user` does.
    let _answer = futures::executor::block_on(form.ask(FormRequest {
        id: FormRequestId(1),
        block_id,
        tool_use_id: ToolUseId::new("toolu_ask_user"),
        spec,
        error: None,
    }));
    stream(
        sink,
        "Noted. I will prepare the canary with those settings.",
    );
    usage(sink, 21_310, 1_480, 0.0862);
    sink.emit(DomainEvent::TurnFinished {
        turn_id,
        stop_reason: StopReason::EndTurn,
    });
}

/// A Welch t-test of the response times of two builds, as the statistics
/// engine would return it.
fn latency_card() -> (Computed, ComputedBody) {
    let shown = |v: f64, display: &str| ComputedValue {
        display: Some(display.into()),
        ..ComputedValue::new(v)
    };
    let mut c = Computed::new(
        "st_bench",
        "stats.ttest_welch",
        "Welch two-sample t-test",
        Engine {
            name: "crest-stats".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            backend: Some("statrs 0.19.1".into()),
            platform: Some("x86_64-linux".into()),
        },
    );
    c.method.why = Some("two independent groups; Welch needs no equal-variance assumption".into());
    c.method.alternatives = vec!["student_t".into(), "mann_whitney".into()];
    c.values.insert("mean_a".into(), shown(184.6, "184.6"));
    c.values.insert("mean_b".into(), shown(158.2, "158.2"));
    c.values.insert("statistic".into(), shown(-4.45, "−4.45"));
    c.values.insert("df".into(), shown(45.8, "45.8"));
    c.values
        .insert("p_value".into(), shown(0.000_056, "< .001"));
    c.values.insert(
        "cohens_d".into(),
        ComputedValue {
            ci: Some(Interval {
                level: 0.95,
                lo: 0.66,
                hi: 1.9,
                method: None,
            }),
            label: Some("large".into()),
            ..shown(1.28, "1.28")
        },
    );
    for (id, check, stat, p) in [
        ("normality_a", "Shapiro-Wilk (v0.17)", 0.97, 0.68),
        ("normality_b", "Shapiro-Wilk (v0.18)", 0.96, 0.44),
        ("equal_var", "Brown-Forsythe", 0.27, 0.61),
    ] {
        c.assumptions.push(Assumption {
            id: id.into(),
            check: Some(check.into()),
            stat: Some(stat),
            p: Some(p),
            status: AssumptionStatus::Pass,
            consequence: None,
            note: None,
        });
    }
    c.warnings.push(ComputedWarning {
        code: "small_n".into(),
        severity: WarningSeverity::Info,
        text: "24 runs per group: the interval of d is wide".into(),
    });
    c.inputs.push(InputRef {
        name: "latency.csv".into(),
        role: Some("data".into()),
        path: Some("bench/latency.csv".into()),
        hash: Some("blake3:4e1c0a97b2d35f68".into()),
        rows: Some(48),
        columns: vec!["latency_ms".into(), "build".into()],
        filter: None,
    });
    c.reproduce.lua = Some(
        "app.compute.test{ data = \"bench/latency.csv\", test = \"welch\", value = \
         \"latency_ms\", by = \"build\" }"
            .into(),
    );
    c.reproduce.python =
        Some("scipy.stats.ttest_ind(a['latency_ms'], b['latency_ms'], equal_var=False)".into());
    c.reproduce.r =
        Some("t.test(latency_ms ~ build, data = read.csv(\"bench/latency.csv\"))".into());
    c.narrative = Some(Narrative {
        template: "apa_ttest".into(),
        text_slots: vec!["statistic".into(), "df".into(), "p_value".into()],
    });
    let group = |name: &str, mean: f64, sd: f64| GroupSummary {
        name: name.into(),
        n: 24,
        mean: Some(mean),
        sd: Some(sd),
        median: None,
    };
    let body = ComputedBody::Test(TestBody {
        test: TestKind::WelchT,
        alternative: Alternative::TwoSided,
        groups: vec![group("v0.17", 184.6, 21.3), group("v0.18", 158.2, 19.8)],
        observed: None,
        expected: None,
    });
    (c, body)
}

/// The slowest checkout request: a failed payment attempt and its retry.
fn checkout_trace() -> TraceSpec {
    let row = |id: &str, label: &str, depth: u32, service: &str, start: f64, end: f64, status| {
        let mut attrs = serde_json::Map::new();
        attrs.insert(SERVICE.into(), json!(service));
        TimelineRow {
            id: id.into(),
            label: label.into(),
            depth,
            group: None,
            spans: vec![Span {
                start,
                end,
                label: None,
                color_key: Some(service.into()),
                status: Some(status),
                attrs,
            }],
            markers: Vec::new(),
        }
    };
    let ok = SpanStatus::Ok;
    let rows = vec![
        row("s1", "POST /checkout", 0, "gateway", 0.0, 412.0, ok),
        row("s2", "auth.verify", 1, "gateway", 2.0, 18.0, ok),
        row("s3", "checkout.create", 1, "checkout", 20.0, 405.0, ok),
        row("s4", "SELECT cart", 2, "db", 24.0, 61.0, ok),
        row("s5", "pricing.rules", 2, "checkout", 63.0, 98.0, ok),
        row(
            "s6",
            "payments.charge",
            2,
            "payments",
            100.0,
            388.0,
            SpanStatus::Warning,
        ),
        row(
            "s7",
            "POST /charge · attempt 1",
            3,
            "payments",
            104.0,
            230.0,
            SpanStatus::Error,
        ),
        row(
            "s8",
            "POST /charge · attempt 2",
            3,
            "payments",
            236.0,
            384.0,
            ok,
        ),
        row("s9", "INSERT order", 2, "db", 390.0, 404.0, ok),
    ];
    let timeline = TimelineSpec {
        title: None,
        unit: TimeUnit::Ms,
        rows,
        range: None,
    };
    TraceSpec {
        title: Some("checkout, slowest request of the run".into()),
        path: Some("traces/checkout-slow.json".into()),
        format: TraceFormat::Otlp,
        traces: 1,
        services: ["gateway", "checkout", "db", "payments"]
            .map(String::from)
            .to_vec(),
        started_at: None,
        total_spans: 9,
        critical_path: critical_path(&timeline),
        timeline,
    }
}

/// The form `ask_user` shows to decide what to ship.
fn ship_form() -> FormSpec {
    serde_json::from_value(json!({
        "title": "Ship the new build?",
        "description": "v0.18 is faster on this benchmark, with one failed payment attempt in its slowest request.",
        "submit_label": "Decide",
        "fields": [
            {"name": "decision", "type": "enum", "label": "Decision", "required": true,
             "options": [
                {"value": "ship", "label": "Ship to 10 % of traffic"},
                {"value": "hold", "label": "Hold until the failed attempt is understood"},
                {"value": "rerun", "label": "Rerun the benchmark with 200 requests"}
             ]},
            {"name": "window", "type": "date_range", "label": "Canary window",
             "min": "2026-11-01", "max": "2026-12-31"},
            {"name": "notify", "type": "multi_enum", "label": "Notify", "default": ["sre"],
             "options": [{"value": "payments"}, {"value": "sre"}, {"value": "release"}]},
            {"name": "notes", "type": "multiline", "label": "Notes"}
        ]
    }))
    .unwrap_or_else(|err| panic!("{err}"))
}

/// A turn that draws how the decision model routes a prompt.
fn routing(sink: &RecordingSink, prompt: &str) {
    let turn_id = start(sink, prompt);
    add(
        sink,
        BlockKind::Mermaid(MermaidSpec {
            source: "sequenceDiagram\n  participant U as You\n  participant C as crest\n  \
                     participant D as Clef\n  participant M as Claude\n  U->>C: prompt\n  \
                     C->>D: which tools? which model?\n  D-->>C: probabilities\n  \
                     C->>M: turn with the chosen tools\n  M-->>C: answer and blocks\n  \
                     C-->>U: timeline"
                .into(),
            title: Some("How a prompt is routed".into()),
        }),
    );
    pause(500);
    stream(
        sink,
        "One call to the decision model per prompt: cheap, fast, and the turn falls back \
         to every tool and the main model when it is not configured.",
    );
    usage(sink, 12_800, 960, 0.0526);
    sink.emit(DomainEvent::TurnFinished {
        turn_id,
        stop_reason: StopReason::EndTurn,
    });
}

/// A turn whose edit waits for the user in the approval dialog.
fn approval(sink: &RecordingSink, approval: &UiApproval, prompt: &str) {
    let turn_id = start(sink, prompt);
    add(
        sink,
        tool_call(
            "grep",
            "\"add(\" in src/ tests/",
            "src/math.rs:2\ntests/math.rs:5",
        ),
    );
    add(
        sink,
        BlockKind::Text {
            markdown: "`add` is used in two files; first `src/math.rs`:".into(),
        },
    );
    let request = ApprovalRequest {
        id: ApprovalRequestId(1),
        tool_use_id: ToolUseId::new("toolu_edit"),
        tool: "edit_file".into(),
        effect: ToolEffect::Write,
        subject: "src/math.rs".into(),
        preview: ApprovalPreview::Diff(DiffSpec {
            path: "src/math.rs".into(),
            old_text: Some(NEW_MATH.into()),
            new_text: NEW_MATH.replace("add", "sum").replace("Adds", "Sums"),
            unified: RENAME_DIFF.into(),
            approval: None,
        }),
    };
    usage(sink, 9_120, 412, 0.0312);
    // Never answered: the dialog stays for the screenshot.
    futures::executor::block_on(approval.request(request)).ok();
    sink.emit(DomainEvent::TurnFinished {
        turn_id,
        stop_reason: StopReason::EndTurn,
    });
}

/// The research flow at its fifth stage, with the activity line on.
fn research(sink: &RecordingSink) -> Reply {
    let stages = ["Intake", "Observation", "Data", "Analysis", "Hypotheses"];
    for (ix, stage) in stages.iter().enumerate() {
        add(
            sink,
            BlockKind::StageHeader {
                stage: (*stage).into(),
                index: u32::try_from(ix).unwrap_or(0) + 1,
                total: 14,
            },
        );
        match ix {
            1 => add(sink, tool_call("grep", "\"fn add\" in src/", "1 match")),
            2 => add(
                sink,
                BlockKind::Table(TableSpec {
                    columns: vec![
                        Column::new("evidence"),
                        Column::new("source"),
                        Column::new("confidence"),
                    ],
                    rows: vec![
                        vec![
                            Cell::Text("add(-2, -3) returns -8".into()),
                            Cell::Text("cargo test".into()),
                            Cell::Text("high".into()),
                        ],
                        vec![
                            Cell::Text("the branch on b < 0 subtracts".into()),
                            Cell::Text("src/math.rs:3".into()),
                            Cell::Text("high".into()),
                        ],
                        vec![
                            Cell::Text("no test covers two negatives".into()),
                            Cell::Text("tests/math.rs".into()),
                            Cell::Text("medium".into()),
                        ],
                    ],
                }),
            ),
            3 => add(
                sink,
                BlockKind::Mermaid(MermaidSpec {
                    source: FLOWCHART.into(),
                    title: Some("How the fix goes".into()),
                }),
            ),
            _ => {}
        }
        pause(150);
    }
    sink.emit(DomainEvent::Activity {
        message: "Hypotheses: asking claude-opus-5-5 · 3 hypotheses from 3 pieces of evidence"
            .into(),
    });
    // Keeps the activity line on screen.
    pause(600_000);
    Reply::Done
}

/// A gear, extruded along Z: `teeth` teeth between `inner` and `outer`
/// radius, `thickness` high, with a hub hole.
fn gear(teeth: usize, inner: f32, outer: f32, hole: f32, thickness: f32) -> ModelMesh {
    let steps = teeth * 4;
    let rim = |i: usize| -> f32 {
        // Two points at the top of each tooth, two at its root.
        if i % 4 < 2 { outer } else { inner }
    };
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    let mut ring = |radius: &dyn Fn(usize) -> f32, z: f32| -> u32 {
        let first = u32::try_from(positions.len()).unwrap_or(0);
        for i in 0..steps {
            let a = TAU * (i as f32 + 0.5) / steps as f32;
            let r = radius(i);
            positions.push([r * a.cos(), r * a.sin(), z]);
        }
        first
    };
    let (outer_top, outer_bottom) = (ring(&rim, thickness), ring(&rim, 0.0));
    let (hole_top, hole_bottom) = (ring(&|_| hole, thickness), ring(&|_| hole, 0.0));
    let n = u32::try_from(steps).unwrap_or(0);
    for i in 0..n {
        let j = (i + 1) % n;
        let quad = |t: &mut Vec<[u32; 3]>, a: u32, b: u32, c: u32, d: u32| {
            t.push([a, b, c]);
            t.push([a, c, d]);
        };
        // Outer wall, hole wall, top and bottom faces.
        quad(
            &mut triangles,
            outer_bottom + i,
            outer_bottom + j,
            outer_top + j,
            outer_top + i,
        );
        quad(
            &mut triangles,
            hole_bottom + j,
            hole_bottom + i,
            hole_top + i,
            hole_top + j,
        );
        quad(
            &mut triangles,
            hole_top + i,
            hole_top + j,
            outer_top + j,
            outer_top + i,
        );
        quad(
            &mut triangles,
            outer_bottom + i,
            outer_bottom + j,
            hole_bottom + j,
            hole_bottom + i,
        );
    }
    ModelMesh {
        positions,
        triangles,
        up: UpAxis::Z,
    }
}

fn modal(id: &str, title: &str, percent: f32, content: CustomNode) -> DomainEvent {
    let size = Percent::new(percent).unwrap_or_default();
    DomainEvent::ModalOpened {
        spec: ModalSpec {
            id: ModalId::new(id),
            plugin: "crest".into(),
            title: title.into(),
            width: size,
            height: size,
            position: ModalPosition::CENTER,
            content,
        },
    }
}

/// What the showcase plugin's `/demo` opens.
fn rich_modal() -> DomainEvent {
    let content = CustomNode::Col {
        children: vec![
            CustomNode::Markdown {
                source: "# Release dashboard\n\nEverything below is **one Lua call**: \
                         `app.modal.open{ content = app.ui.col{ … } }`."
                    .into(),
            },
            CustomNode::Row {
                children: vec![
                    CustomNode::Chart(ChartSpec {
                        title: "Merged pull requests".into(),
                        kind: ChartKind::Line,
                        labels: ["Jun", "Jul", "Aug", "Sep", "Oct"]
                            .map(String::from)
                            .to_vec(),
                        series: vec![
                            Series {
                                name: "features".into(),
                                values: vec![12.0, 18.0, 15.0, 24.0, 31.0],
                            },
                            Series {
                                name: "fixes".into(),
                                values: vec![9.0, 7.0, 11.0, 8.0, 6.0],
                            },
                        ],
                    }),
                    CustomNode::Json {
                        value: json!({
                            "version": "0.6.0",
                            "targets": ["x86_64-linux", "aarch64-macos", "x86_64-macos"],
                            "checks": { "fmt": true, "clippy": true, "tests": 1530 }
                        }),
                        expand_depth: 2,
                    },
                ],
            },
            CustomNode::Mermaid {
                source: "flowchart LR\n  tag[git tag v0.6.0] --> ci[CI on Linux and macOS] \
                         --> build[Release builds] --> pub[GitHub release]"
                    .into(),
            },
        ],
    };
    modal("demo", "showcase · release dashboard", 85.0, content)
}

impl SessionController for Fake {
    fn run_turn(&self, request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
        let sink = Arc::clone(&self.sink);
        let approver = self.approval.clone();
        let form = Arc::clone(&self.form);
        on_thread(move || {
            if request.prompt == APPROVAL_PROMPT {
                approval(&sink, &approver, &request.prompt);
            } else if request.prompt == TECHNICAL_PROMPT {
                technical(&sink, &form, &request.prompt);
            } else if request.prompt == BLOCKS_PROMPT {
                blocks(&sink, &request.prompt);
            } else if request.prompt == ROUTING_PROMPT {
                routing(&sink, &request.prompt);
            } else {
                hero(&sink, &request.prompt);
            }
            TurnEnd::Finished(StopReason::EndTurn)
        })
        .map(|end| end.unwrap_or(TurnEnd::Failed("turn lost".into())))
        .boxed()
    }

    fn permission_mode(&self) -> PermissionMode {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_permission_mode(&self, mode: PermissionMode) {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner) = mode;
    }

    fn models(&self) -> Vec<ModelChoice> {
        Vec::new()
    }

    fn new_session(&self) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    fn run_command(&self, name: &str, args: &str) -> Option<BoxFuture<'static, Reply>> {
        let sink = Arc::clone(&self.sink);
        let args = args.to_owned();
        let reply = match name {
            "research" => on_thread(move || {
                add(
                    &sink,
                    BlockKind::Text {
                        markdown: format!("> /research {args}"),
                    },
                );
                research(&sink)
            }),
            "open" => on_thread(move || {
                sink.emit(modal(
                    "open:models/gear.stl",
                    "models/gear.stl",
                    85.0,
                    CustomNode::Model3d(ModelSpec {
                        name: "gear.stl".into(),
                        mesh: gear(18, 16.0, 19.0, 5.0, 6.0),
                        unit: Some("mm".into()),
                    }),
                ));
                Reply::Done
            }),
            "demo" => on_thread(move || {
                sink.emit(rich_modal());
                Reply::Done
            }),
            _ => return None,
        };
        Some(reply.map(|reply| reply.unwrap_or(Reply::Done)).boxed())
    }

    fn git_status(&self) -> BoxFuture<'static, Option<GitStatus>> {
        futures::future::ready(Some(GitStatus {
            branch: Some("fix/add-negatives".into()),
            modified: 2,
            ahead: 1,
            ..GitStatus::default()
        }))
        .boxed()
    }

    fn help(&self) -> BoxFuture<'static, HelpSnapshot> {
        futures::future::ready(help_snapshot()).boxed()
    }
}

fn tool(name: &str, description: &str, source: ToolSource, effect: &str) -> ToolRow {
    ToolRow {
        name: name.into(),
        description: description.into(),
        source,
        effect: effect.into(),
        schema: String::new(),
    }
}

fn help_snapshot() -> HelpSnapshot {
    HelpSnapshot {
        commands: vec![CommandRow {
            name: "demo".into(),
            usage: "/demo [chart|table|diagram|json|diff|ask]".into(),
            description: "Tour of what plugins can show".into(),
            source: "showcase".into(),
        }],
        tools: vec![
            tool(
                "read_file",
                "Reads a file of the workspace",
                ToolSource::Builtin,
                "read",
            ),
            tool(
                "grep",
                "Searches the workspace with rg",
                ToolSource::Builtin,
                "read",
            ),
            tool(
                "edit_file",
                "Replaces a unique snippet of a file",
                ToolSource::Builtin,
                "write",
            ),
            tool(
                "run_command",
                "Runs a program, no shell",
                ToolSource::Builtin,
                "execute",
            ),
            tool(
                "show_chart",
                "Draws a chart in the timeline",
                ToolSource::Builtin,
                "read",
            ),
            tool(
                "demo_dashboard",
                "A dashboard block from the showcase plugin",
                ToolSource::Plugin("showcase".into()),
                "read",
            ),
        ],
        plugins: vec![PluginRow {
            name: "showcase".into(),
            path: "examples/plugins/showcase/init.lua".into(),
            layer: "global".into(),
            adds: vec![
                "tool demo_dashboard".into(),
                "command /demo".into(),
                "keymap <leader>d".into(),
            ],
            capabilities: Vec::new(),
            trust: "none needed".into(),
            approve: None,
            diagnostics: Vec::new(),
        }],
        ..HelpSnapshot::default()
    }
}

fn main() {
    let scene = std::env::args()
        .skip_while(|arg| arg != "--scene")
        .nth(1)
        .unwrap_or_else(|| "hero".into());
    support::run_sized("crest", (1280.0, 960.0), move |window, cx| {
        let root = WorkspaceRoot::new("/workspace").unwrap_or_else(|err| panic!("{err}"));
        let fs = Arc::new(
            InMemoryFs::new(root)
                .with_file(
                    "src/math.rs",
                    "pub fn add(a: i64, b: i64) -> i64 { a + b }\n",
                )
                .with_file("tests/math.rs", "#[test]\nfn adds() {}\n"),
        );
        let (sink, events) = RecordingSink::with_channel();
        let (approval, approvals) = UiApproval::channel();
        let (form, forms) = UiForm::channel();
        let controller = Arc::new(Fake {
            sink: Arc::new(sink),
            approval,
            form: Arc::new(form),
            mode: Mutex::new(PermissionMode::Ask),
        });
        let mut parts = SessionParts::new(controller, events, approvals, fs.clone(), fs);
        parts.forms = Some(forms);
        parts.workspace_label = "calc".into();
        parts.model = Some(ModelId::new("claude-sonnet-5-5"));
        let (root, session) = ui::session::open(parts, window, cx);
        play(&scene, window.window_handle(), session, cx);
        root.into()
    });
}

/// What each scene types (a prompt sent with `Enter`, or keys).
fn script(scene: &str) -> Vec<Step> {
    match scene {
        "hero" => vec![Step::Send(HERO_PROMPT)],
        "research" => vec![Step::Send(
            "/research add() returns the difference for negative numbers",
        )],
        "approval" => vec![Step::Send(APPROVAL_PROMPT)],
        "blocks" => vec![Step::Send(BLOCKS_PROMPT)],
        "routing" => vec![Step::Send(ROUTING_PROMPT)],
        "technical" => vec![Step::Send(TECHNICAL_PROMPT)],
        "help" => vec![Step::Send(HERO_PROMPT), Step::Keys("f1")],
        "model" => vec![
            Step::Send(HERO_PROMPT),
            Step::Send("/open @models/gear.stl"),
        ],
        "modal" => vec![Step::Send(HERO_PROMPT), Step::Send("/demo")],
        other => {
            eprintln!("showcase: unknown scene {other:?}");
            Vec::new()
        }
    }
}

enum Step {
    Send(&'static str),
    Keys(&'static str),
}

fn play(scene: &str, window: gpui::AnyWindowHandle, session: Entity<Session>, cx: &mut App) {
    let steps = script(scene);
    cx.spawn(async move |cx| {
        let executor = cx.background_executor().clone();
        let wait = |ms| executor.timer(Duration::from_millis(ms));
        wait(600).await;
        for step in steps {
            let keys = match step {
                Step::Send(text) => {
                    let input = cx.update(|cx| session.read(cx).input().clone());
                    window
                        .update(cx, |_, window, cx| {
                            input.update(cx, |input, cx| input.set_text(text, window, cx));
                        })
                        .ok();
                    "enter"
                }
                Step::Keys(keys) => keys,
            };
            for key in keys.split(' ') {
                if let Ok(keystroke) = gpui::Keystroke::parse(key) {
                    window
                        .update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))
                        .ok();
                }
            }
            wait(4_000).await;
        }
        eprintln!("showcase: ready");
    })
    .detach();
}
