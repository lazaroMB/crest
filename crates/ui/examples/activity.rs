//! The activity line, the prompt queue and the terminal-style prompt over
//! the real session, with a fake controller: `/research` runs four stages
//! for about eight seconds (stage headers, activity lines, tool calls and
//! a flowchart with long labels) and each prompt streams a short answer.
//!
//! `cargo run -p ui --example activity [--demo]`
//!
//! Run `/research`, then type prompts while it works: they are listed under
//! the activity line and sent one by one when it ends; `Up` in an empty
//! prompt takes the last one back.
//!
//! With `--demo` the example types into its own window (through
//! `Window::dispatch_keystroke`, the same path as real keys), logs the
//! activity line and the queue after each step and quits.

#[path = "support/mod.rs"]
mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use application::testing::fs::InMemoryFs;
use application::testing::system::RecordingSink;
use domain::block::{Block, BlockKind, MermaidSpec, ToolStatus};
use domain::conversation::{SessionId, StopReason, TurnId};
use domain::model::{ModelId, ModelRole};
use domain::permission::PermissionMode;
use domain::workspace::WorkspaceRoot;
use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use gpui::{App, Entity};
use serde_json::json;
use ui::approval::UiApproval;
use ui::session::{
    ModelChoice, Reply, Session, SessionController, SessionParts, TurnEnd, TurnRequest,
    prompt_block,
};

const FLOWCHART: &str = "flowchart LR
    A[Read the bug report and reproduce the failing behaviour] --> B{Does the test suite already cover the subtraction case?}
    B -->|no, write a failing test first| C[Add a regression test for add returning the difference]
    B -->|yes| D[Fix the operator in add and run the whole suite]
    C --> D
";

/// A controller whose turns and `/research` only emit scripted events.
struct Fake {
    sink: Arc<RecordingSink>,
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

fn tool_call(name: &str, summary: &str) -> Block {
    Block::new(BlockKind::ToolCall {
        tool_use_id: format!("toolu_{name}"),
        name: name.into(),
        summary: summary.into(),
        input: json!({}),
        status: ToolStatus::Succeeded,
        output: Some("3 matches".into()),
    })
}

impl SessionController for Fake {
    fn run_turn(&self, request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
        let sink = Arc::clone(&self.sink);
        on_thread(move || {
            sink.emit(DomainEvent::BlockAdded {
                block: prompt_block(&request.prompt),
            });
            let turn_id = TurnId::new();
            sink.emit(DomainEvent::TurnStarted {
                session_id: SessionId::new(),
                turn_id,
                model: ModelId::new("claude-opus-5-5"),
                role: ModelRole::Main,
            });
            let block = Block::new(BlockKind::Text {
                markdown: String::new(),
            });
            let block_id = block.id;
            sink.emit(DomainEvent::BlockAdded { block });
            for word in format!("Answering \"{}\" word by word.", request.prompt).split(' ') {
                pause(150);
                sink.emit(DomainEvent::TextDelta {
                    block_id,
                    text: format!("{word} "),
                });
            }
            sink.emit(DomainEvent::TurnFinished {
                turn_id,
                stop_reason: StopReason::EndTurn,
            });
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

    fn run_command(&self, name: &str, _args: &str) -> Option<BoxFuture<'static, Reply>> {
        if name != "research" {
            return None;
        }
        let sink = Arc::clone(&self.sink);
        let reply = on_thread(move || {
            let stages = ["Intake", "Observation", "Data", "Analysis"];
            for (ix, stage) in stages.iter().enumerate() {
                sink.emit(DomainEvent::BlockAdded {
                    block: Block::new(BlockKind::StageHeader {
                        stage: (*stage).into(),
                        index: ix as u32 + 1,
                        total: 14,
                    }),
                });
                sink.emit(DomainEvent::Activity {
                    message: format!("{stage}: asking claude-opus-5-5"),
                });
                pause(1000);
                sink.emit(DomainEvent::Activity {
                    message: format!("explore · grep \"fn add\" ({stage})"),
                });
                pause(600);
                if ix == 2 {
                    sink.emit(DomainEvent::BlockAdded {
                        block: tool_call("grep", "\"fn add\" in src/"),
                    });
                    sink.emit(DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Mermaid(MermaidSpec {
                            source: FLOWCHART.into(),
                            title: Some("How the fix goes".into()),
                        })),
                    });
                }
                pause(400);
            }
            Reply::Notice(NoticeLevel::Info, "research waits for your answer".into())
        });
        Some(reply.map(|reply| reply.unwrap_or(Reply::Done)).boxed())
    }
}

fn main() {
    let demo = std::env::args().any(|arg| arg == "--demo");
    support::run("crest — activity", move |window, cx| {
        let root = WorkspaceRoot::new("/workspace").unwrap_or_else(|err| panic!("{err}"));
        let fs = Arc::new(InMemoryFs::new(root).with_file("README.md", "# demo\n"));
        let (sink, events) = RecordingSink::with_channel();
        let (_approval, approvals) = UiApproval::channel();
        let controller = Arc::new(Fake {
            sink: Arc::new(sink),
            mode: Mutex::new(PermissionMode::Ask),
        });
        let mut parts = SessionParts::new(controller, events, approvals, fs.clone(), fs);
        parts.workspace_label = "activity".into();
        let (root, session) = ui::session::open(parts, window, cx);
        if demo {
            run_demo(window.window_handle(), session, cx);
        }
        root.into()
    });
}

/// What `--demo` types, each followed by a pause (ms) and a log line.
const DEMO_STEPS: &[(&str, u64)] = &[
    ("/ r e s e a r c h enter", 1500),
    (
        "t h e n space l o o k space a t space t h e space t e s t s enter",
        400,
    ),
    ("a n d space t h e space d o c s enter", 1500),
    ("up", 800),
    ("space t o o enter", 6000),
    ("escape", 3000),
];

fn run_demo(window: gpui::AnyWindowHandle, session: Entity<Session>, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(800))
            .await;
        for (keys, pause) in DEMO_STEPS {
            for key in keys.split(' ') {
                let Ok(keystroke) = gpui::Keystroke::parse(key) else {
                    continue;
                };
                window
                    .update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))
                    .ok();
            }
            cx.background_executor()
                .timer(Duration::from_millis(*pause))
                .await;
            cx.update(|cx| {
                let session = session.read(cx);
                let activity = session.activity().read(cx);
                eprintln!(
                    "demo [{keys}]: line={:?} queue={:?} prompt={:?}",
                    activity.line(cx),
                    activity.state().queue(),
                    session.input().read(cx).text(cx),
                );
            });
        }
        eprintln!("demo: done");
        cx.update(|cx| cx.quit());
    })
    .detach();
}
