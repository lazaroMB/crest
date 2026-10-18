//! Stress test of the timeline: 1000 varied blocks from a fake `EventSink`,
//! then a scripted scroll (moving the selection up to the first block and
//! back down) while the frame interval is logged.
//!
//! `cargo run -p ui --release --example timeline_stress [--quit]`
//!
//! With `--quit` the app exits after the sweep; quit any time with
//! Cmd/Ctrl+Q. The summary line reports the frame interval (time between
//! frames, vsync included) while scrolling.

#[path = "support/mod.rs"]
mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use application::keymap::Action;
use application::ports::events::{DomainEvent, EventSink};
use application::testing::system::RecordingSink;
use domain::block::{Block, BlockKind, ToolStatus};
use gpui::{App, AppContext as _, Entity, Window};
use serde_json::json;
use ui::app::RootView;
use ui::timeline::TimelineView;

const BLOCKS: usize = 1000;
/// Blocks the selection moves per frame during the sweep.
const STEP: usize = 4;

fn main() {
    let quit = std::env::args().any(|arg| arg == "--quit");
    let (sink, rx) = RecordingSink::with_channel();
    let started = Instant::now();
    std::thread::spawn(move || emit_blocks(&sink));

    support::run("crest — timeline stress", move |window, cx| {
        let timeline = cx.new(|cx| TimelineView::with_events(rx, cx));
        let root = cx.new(|cx| {
            let mut root = RootView::new(cx);
            root.set_timeline(timeline.clone(), cx);
            root
        });
        let stats = Rc::new(RefCell::new(Stats::new(started, quit)));
        schedule(window, timeline, stats);
        root.into()
    });
}

/// Emits `BLOCKS` blocks of every kind the timeline draws, with tool status
/// changes, then streams text into the last block.
fn emit_blocks(sink: &RecordingSink) {
    for i in 0..BLOCKS - 1 {
        let block = sample_block(i);
        let id = block.id;
        let is_tool = matches!(block.kind, BlockKind::ToolCall { .. });
        sink.emit(DomainEvent::BlockAdded { block });
        if is_tool {
            let status = if i % 10 == 1 {
                ToolStatus::Failed
            } else {
                ToolStatus::Succeeded
            };
            sink.emit(DomainEvent::ToolStatusChanged {
                block_id: id,
                status,
                output: Some(format!("{} lines", i * 3 % 97)),
            });
        }
    }
    let streamed = Block::new(BlockKind::Text {
        markdown: String::from("**Streaming:** "),
    });
    let id = streamed.id;
    sink.emit(DomainEvent::BlockAdded { block: streamed });
    for word in 0..200 {
        std::thread::sleep(Duration::from_millis(10));
        sink.emit(DomainEvent::TextDelta {
            block_id: id,
            text: format!("word{word} "),
        });
    }
}

fn sample_block(i: usize) -> Block {
    let kind = match i % 6 {
        0 => BlockKind::Text {
            markdown: format!(
                "### Respuesta {i}\n\nEl archivo `src/lib.rs` define **{n}** funciones:\n\n\
                 - `parse` lee la entrada\n- `apply` aplica el evento\n\n\
                 ```rust\nfn main() {{ println!(\"{i}\"); }}\n```",
                n = i % 17
            ),
        },
        1 | 3 => BlockKind::ToolCall {
            tool_use_id: format!("toolu_{i}"),
            name: ["read_file", "grep", "bash"][i % 3].to_owned(),
            summary: format!("src/module_{i}.rs:1-{}", 20 + i % 80),
            input: json!({ "path": format!("src/module_{i}.rs"), "limit": 200 }),
            status: ToolStatus::Running,
            output: None,
        },
        2 => BlockKind::Text {
            markdown: format!("Bloque corto número {i}."),
        },
        4 => BlockKind::Error {
            message: format!("tool `bash` exited with status {}", i % 5 + 1),
            hint: Some("revisa el comando o pide permisos con <leader>p".to_owned()),
        },
        _ => BlockKind::Thinking {
            text: format!("pensando sobre {i}"),
            collapsed: true,
        },
    };
    Block::new(kind)
}

struct Stats {
    started: Instant,
    quit: bool,
    loaded: bool,
    last_frame: Option<Instant>,
    sweep: Vec<Duration>,
    going_up: bool,
    done: bool,
}

impl Stats {
    fn new(started: Instant, quit: bool) -> Self {
        Self {
            started,
            quit,
            loaded: false,
            last_frame: None,
            sweep: Vec::new(),
            going_up: true,
            done: false,
        }
    }
}

fn schedule(window: &Window, timeline: Entity<TimelineView>, stats: Rc<RefCell<Stats>>) {
    window.on_next_frame(move |window, cx| {
        frame(window, cx, &timeline, &stats);
        if !stats.borrow().done {
            schedule(window, timeline, stats);
        }
    });
}

fn frame(
    window: &mut Window,
    cx: &mut App,
    timeline: &Entity<TimelineView>,
    stats: &Rc<RefCell<Stats>>,
) {
    let mut s = stats.borrow_mut();
    let now = Instant::now();
    let interval = s.last_frame.map(|last| now - last);
    s.last_frame = Some(now);
    if s.done {
        return;
    }
    let len = timeline.read(cx).state().len();
    if interval.is_none() {
        eprintln!(
            "timeline_stress: first frame after {:?} ({len} blocks applied)",
            s.started.elapsed()
        );
    }
    if !s.loaded {
        if len >= BLOCKS {
            s.loaded = true;
            eprintln!(
                "timeline_stress: all {len} blocks shown after {:?}",
                s.started.elapsed()
            );
        }
        window.refresh();
        return;
    }
    if let Some(interval) = interval {
        s.sweep.push(interval);
    }
    let action = Action::builtin(if s.going_up {
        "block.prev"
    } else {
        "block.next"
    });
    let selected = timeline.update(cx, |view, cx| {
        for _ in 0..STEP {
            view.handle_action(&action, cx);
        }
        view.state().selected()
    });
    if s.sweep.len().is_multiple_of(60) {
        eprintln!(
            "timeline_stress: frame {} selected {selected:?}",
            s.sweep.len()
        );
    }
    match selected {
        Some(0) if s.going_up => s.going_up = false,
        Some(ix) if !s.going_up && ix + 1 >= len => {
            s.done = true;
            report(&s.sweep);
            if s.quit {
                cx.quit();
            }
        }
        _ => {}
    }
}

fn report(intervals: &[Duration]) {
    let mut sorted = intervals.to_vec();
    sorted.sort();
    let n = sorted.len().max(1);
    let total: Duration = sorted.iter().sum();
    let pick = |q: f64| {
        sorted
            .get(((n as f64 - 1.0) * q) as usize)
            .copied()
            .unwrap_or_default()
    };
    eprintln!(
        "timeline_stress: sweep of {} frames: avg {:?}, p50 {:?}, p95 {:?}, max {:?}",
        sorted.len(),
        total / n as u32,
        pick(0.5),
        pick(0.95),
        sorted.last().copied().unwrap_or_default(),
    );
}
