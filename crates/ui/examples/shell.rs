//! The main window with its three views wired to a fake agent: the timeline,
//! the modal input and the status bar, all fed by one `RecordingSink`.
//!
//! `cargo run -p ui --example shell [workspace-label]`
//!
//! - insert: type and press Enter; the fake agent echoes the prompt with a
//!   tool call and a streamed reply. Shift+Enter adds a line, Esc goes to
//!   normal mode.
//! - normal: `j`/`k`/`gg`/`G` move the selection, Enter expands a tool
//!   call, `y` copies a block, `<Space>p` cycles the permission mode, `i`
//!   returns to insert, `:` opens the command line.
//! - command: `:error`, `:notice <text>`, `:retry`, `:q`.
//!
//! Quit with `:q` or Cmd/Ctrl+Q.
//!
//! With `--demo` the example types into its own window (through
//! `Window::dispatch_keystroke`, the same path as real keys), logs what the
//! views show after each step and quits.

#[path = "support/mod.rs"]
mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use application::keymap::Action;
use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use application::testing::system::RecordingSink;
use domain::block::{Block, BlockKind, ToolStatus};
use domain::conversation::{SessionId, StopReason, TurnId, Usage};
use domain::model::{ModelId, ModelRole};
use domain::permission::PermissionMode;
use futures::StreamExt as _;
use gpui::{App, AppContext as _, Entity};
use serde_json::json;
use ui::app::RootView;
use ui::input::{InputEvent, InputView};
use ui::status::StatusView;
use ui::timeline::TimelineView;

const MODEL: &str = "claude-sonnet-5-5";

/// The fake agent: emits the events of a turn on a background thread.
#[derive(Clone)]
struct FakeAgent {
    sink: Arc<RecordingSink>,
    session: SessionId,
    state: Arc<Mutex<AgentState>>,
}

#[derive(Default)]
struct AgentState {
    usage: Usage,
    cost: f64,
    mode: PermissionMode,
}

impl FakeAgent {
    fn emit(&self, event: DomainEvent) {
        self.sink.emit(event);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, AgentState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn turn(&self, prompt: String) {
        let agent = self.clone();
        std::thread::spawn(move || agent.run_turn(&prompt));
    }

    fn run_turn(&self, prompt: &str) {
        let turn_id = TurnId::new();
        self.emit(DomainEvent::TurnStarted {
            session_id: self.session,
            turn_id,
            model: ModelId::new(MODEL),
            role: ModelRole::Main,
        });
        let quoted: String = prompt.lines().map(|line| format!("> {line}\n")).collect();
        self.emit(DomainEvent::BlockAdded {
            block: Block::new(BlockKind::Text { markdown: quoted }),
        });

        let tool = Block::new(BlockKind::ToolCall {
            tool_use_id: format!("toolu_{}", turn_id),
            name: "echo".into(),
            summary: format!("{} chars", prompt.chars().count()),
            input: json!({ "text": prompt }),
            status: ToolStatus::Running,
            output: None,
        });
        let tool_id = tool.id;
        self.emit(DomainEvent::BlockAdded { block: tool });
        std::thread::sleep(Duration::from_millis(400));
        self.emit(DomainEvent::ToolStatusChanged {
            block_id: tool_id,
            status: ToolStatus::Succeeded,
            output: Some(prompt.to_owned()),
        });

        let reply = Block::new(BlockKind::Text {
            markdown: String::new(),
        });
        let reply_id = reply.id;
        self.emit(DomainEvent::BlockAdded { block: reply });
        let words = format!("**Eco:** {prompt}").replace('\n', " ");
        for word in words.split(' ') {
            std::thread::sleep(Duration::from_millis(60));
            self.emit(DomainEvent::TextDelta {
                block_id: reply_id,
                text: format!("{word} "),
            });
        }

        let (session, cost) = {
            let mut state = self.state();
            state.usage.input_tokens += 1200;
            state.usage.output_tokens += words.len() as u64;
            state.cost += 0.0137;
            (state.usage, state.cost)
        };
        self.emit(DomainEvent::UsageUpdated {
            turn: Usage::default(),
            session,
            cost_usd: Some(cost),
        });
        self.emit(DomainEvent::TurnFinished {
            turn_id,
            stop_reason: StopReason::EndTurn,
        });
    }

    fn command(&self, command: &str, cx: &mut App) {
        let (name, arg) = command.split_once(' ').unwrap_or((command, ""));
        match name {
            "q" | "quit" => cx.quit(),
            "error" => self.emit(DomainEvent::BlockAdded {
                block: Block::new(BlockKind::Error {
                    message: "the API returned 529 overloaded".into(),
                    hint: Some("wait a moment and send the prompt again".into()),
                }),
            }),
            "notice" => self.emit(DomainEvent::Notice {
                level: NoticeLevel::Info,
                message: arg.to_owned(),
            }),
            "retry" => self.emit(DomainEvent::Retrying {
                attempt: 1,
                delay_ms: 2000,
            }),
            _ => self.emit(DomainEvent::Notice {
                level: NoticeLevel::Warning,
                message: format!("unknown command :{command}"),
            }),
        }
    }

    fn cycle_permission(&self) {
        let mode = {
            let mut state = self.state();
            state.mode = state.mode.next();
            state.mode
        };
        self.emit(DomainEvent::ModeChanged { mode });
    }
}

fn main() {
    let demo = std::env::args().any(|arg| arg == "--demo");
    let workspace = std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with("--"))
        .unwrap_or_else(|| "example".to_owned());
    let (sink, mut rx) = RecordingSink::with_channel();
    let agent = FakeAgent {
        sink: Arc::new(sink),
        session: SessionId::new(),
        state: Arc::default(),
    };

    support::run(&format!("crest — {workspace}"), move |window, cx| {
        let timeline = cx.new(TimelineView::new);
        let status = cx.new(|_| StatusView::new(workspace));
        status.update(cx, |status, cx| {
            status.state_mut(cx).set_model(ModelId::new(MODEL));
        });
        let input = cx.new(|cx| InputView::new(window, cx));

        // One consumer fans every event out to the timeline and the bar.
        let (to_timeline, to_status) = (timeline.clone(), status.clone());
        cx.spawn(async move |cx| {
            while let Some(event) = rx.next().await {
                to_timeline.update(cx, |view, cx| view.apply(&event, cx));
                to_status.update(cx, |view, cx| view.apply(&event, cx));
            }
        })
        .detach();

        wire_input(&input, &timeline, &status, agent, cx);

        let root = cx.new(|cx| {
            let mut root = RootView::new(cx);
            root.set_timeline(timeline.clone(), cx);
            root.set_input(input.clone(), cx);
            root.set_status(status.clone(), cx);
            root
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        if demo {
            run_demo(window.window_handle(), timeline, input, status, cx);
        }
        root.into()
    });
}

fn wire_input(
    input: &Entity<InputView>,
    timeline: &Entity<TimelineView>,
    status: &Entity<StatusView>,
    agent: FakeAgent,
    cx: &mut App,
) {
    let (timeline, status) = (timeline.clone(), status.clone());
    cx.subscribe(input, move |_, event: &InputEvent, cx| match event {
        InputEvent::Submit(prompt) => agent.turn(prompt.clone()),
        InputEvent::Command(command) => agent.command(command, cx),
        InputEvent::ModeChanged(mode) => {
            status.update(cx, |status, cx| status.set_input_mode(*mode, cx));
        }
        InputEvent::Action(action) => {
            if timeline.update(cx, |view, cx| view.handle_action(action, cx)) {
                return;
            }
            match action {
                Action::Builtin(name) if name == "permission.cycle" => agent.cycle_permission(),
                other => agent.emit(DomainEvent::Notice {
                    level: NoticeLevel::Info,
                    message: format!("{other:?} is not wired in this example"),
                }),
            }
        }
    })
    .detach();
}

/// Keys typed by `--demo`, each followed by a pause and a log line.
const DEMO_STEPS: &[(&str, u64)] = &[
    ("h o l a space c r e s t enter", 2500),
    ("escape k k", 300),
    ("enter", 300),
    ("g g", 300),
    ("shift-g", 300),
    ("space p", 300),
    (": n o t i c e space h i enter", 300),
    ("i", 300),
];

fn run_demo(
    window: gpui::AnyWindowHandle,
    timeline: Entity<TimelineView>,
    input: Entity<InputView>,
    status: Entity<StatusView>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(500))
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
                let timeline = timeline.read(cx).state();
                let selected = timeline.selected();
                let expanded = selected.and_then(|ix| timeline.get(ix)).map(|e| e.expanded);
                let status = status.read(cx).state();
                let notice = status
                    .notice(cx.background_executor().now())
                    .map(|n| n.message.clone());
                eprintln!(
                    "demo [{keys}]: blocks={} selected={selected:?} expanded={expanded:?} \
                     input={:?} text={:?} status={:?} notice={notice:?}",
                    timeline.len(),
                    input.read(cx).mode(),
                    input.read(cx).text(cx),
                    status.segments(),
                );
            });
        }
        eprintln!("demo: done");
        cx.update(|cx| cx.quit());
    })
    .detach();
}
