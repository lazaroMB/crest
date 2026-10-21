//! Scenes for the README's screenshots: the real session (timeline, prompt,
//! status bar, modals, approval dialog, help) driven by a scripted fake
//! controller, so no API key is needed.
//!
//! `cargo run -p ui --example showcase -- --scene <name>`
//!
//! Scenes: `hero` (a turn with tool calls, a diff, a chart and an answer),
//! `research` (the research flow at work), `approval` (an edit waiting for
//! the user), `help` (the help modal), `model` (a 3D model opened with
//! `/open`) and `modal` (a plugin modal with rich content). Each one
//! prints `showcase: ready` once it is on screen and stays open;
//! `scripts/screenshots.sh` captures the window then.

#[path = "support/mod.rs"]
mod support;

use std::f32::consts::TAU;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use application::ports::approval::{
    ApprovalPort as _, ApprovalPreview, ApprovalRequest, ApprovalRequestId,
};
use application::ports::events::{DomainEvent, EventSink};
use application::testing::fs::InMemoryFs;
use application::testing::system::RecordingSink;
use domain::block::{
    Block, BlockKind, Cell, ChartKind, ChartSpec, Column, CustomNode, DiffSpec, MermaidSpec,
    Series, TableSpec, ToolStatus,
};
use domain::conversation::{SessionId, StopReason, ToolUseId, TurnId, Usage};
use domain::git::GitStatus;
use domain::modal::{ModalId, ModalPosition, ModalSpec, Percent};
use domain::model::{ModelId, ModelRole};
use domain::model3d::{ModelMesh, ModelSpec, UpAxis};
use domain::permission::{PermissionMode, ToolEffect};
use domain::workspace::WorkspaceRoot;
use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use gpui::{App, Entity};
use serde_json::json;
use ui::approval::UiApproval;
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
        on_thread(move || {
            if request.prompt == APPROVAL_PROMPT {
                approval(&sink, &approver, &request.prompt);
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
        let controller = Arc::new(Fake {
            sink: Arc::new(sink),
            approval,
            mode: Mutex::new(PermissionMode::Ask),
        });
        let mut parts = SessionParts::new(controller, events, approvals, fs.clone(), fs);
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
