//! Stress test of the table block: a 10 000-row table in the timeline,
//! expanded and collapsed a few times (expanded it lays out 500 rows of
//! wrapping cells), then sorted by each column while the frame interval
//! is logged.
//!
//! `cargo run -p ui --release --example table_stress [--quit]`
//!
//! With `--quit` the app exits after the sweep; quit any time with
//! Cmd/Ctrl+Q. Intervals include vsync, so at 60 Hz ~16.7 ms is the floor.

#[path = "support/mod.rs"]
mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use application::keymap::Action;
use application::ports::events::{DomainEvent, EventSink as _};
use application::testing::system::RecordingSink;
use domain::block::{Block, BlockId, BlockKind, Cell, Column, TableSpec};
use gpui::{App, AppContext as _, Entity, Window};
use ui::app::RootView;
use ui::blocks::table::sort::Direction;
use ui::blocks::table::table_view;
use ui::timeline::TimelineView;

const ROWS: usize = 10_000;
/// Frames the table is expanded and collapsed for, alternating.
const TOGGLES: usize = 40;

fn spec() -> TableSpec {
    let exts = ["rs", "toml", "md", "lua", "json", "yaml"];
    TableSpec {
        columns: vec![
            Column::new("path"),
            Column::new("ext"),
            Column::new("bytes"),
            Column::new("lines"),
            Column::new("generated"),
        ],
        rows: (0..ROWS)
            .map(|i| {
                let ext = exts[i % exts.len()];
                let lines = if i % 13 == 0 {
                    Cell::Null
                } else {
                    Cell::Number(((i * 7919) % 4000) as f64)
                };
                vec![
                    Cell::Text(format!("crates/mod_{}/src/file_{i}.{ext}", i % 97)),
                    Cell::Text(ext.to_owned()),
                    Cell::Number(((i * 104_729) % 250_000) as f64),
                    lines,
                    Cell::Bool(i % 5 == 0),
                ]
            })
            .collect(),
    }
}

fn main() {
    let quit = std::env::args().any(|arg| arg == "--quit");
    let (sink, rx) = RecordingSink::with_channel();
    let spec = spec();
    let table = Block::new(BlockKind::Table(spec.clone()));
    let id = table.id;
    sink.emit(DomainEvent::BlockAdded {
        block: Block::new(BlockKind::Text {
            markdown: format!("Sizes of **{ROWS}** files:"),
        }),
    });
    sink.emit(DomainEvent::BlockAdded { block: table });

    support::run("crest — table stress", move |window, cx| {
        let timeline = cx.new(|cx| TimelineView::with_events(rx, cx));
        let root = cx.new(|cx| {
            let mut root = RootView::new(cx);
            root.set_timeline(timeline.clone(), cx);
            root
        });
        let stats = Rc::new(RefCell::new(Stats {
            started: Instant::now(),
            quit,
            id,
            spec,
            phase: Phase::Load,
            last_frame: None,
            toggle: Vec::new(),
            sort: Vec::new(),
        }));
        schedule(window, timeline, stats);
        root.into()
    });
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Load,
    /// Expanding and collapsing; the frames done.
    Toggle(usize),
    /// Sorting; the column and direction applied next.
    Sort(usize, bool),
    Done,
}

struct Stats {
    started: Instant,
    quit: bool,
    id: BlockId,
    spec: TableSpec,
    phase: Phase,
    last_frame: Option<Instant>,
    toggle: Vec<Duration>,
    sort: Vec<Duration>,
}

fn schedule(window: &Window, timeline: Entity<TimelineView>, stats: Rc<RefCell<Stats>>) {
    window.on_next_frame(move |window, cx| {
        frame(window, cx, &timeline, &stats);
        if stats.borrow().phase != Phase::Done {
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
    let interval = s.last_frame.replace(now).map(|last| now - last);
    window.refresh();
    match s.phase {
        Phase::Load => {
            if timeline.read(cx).state().len() < 2 {
                return;
            }
            timeline.update(cx, |view, cx| {
                view.handle_action(&Action::builtin("block.last"), cx);
            });
            eprintln!(
                "table_stress: {ROWS} rows shown after {:?}",
                s.started.elapsed()
            );
            s.phase = Phase::Toggle(0);
        }
        Phase::Toggle(done) => {
            if let Some(interval) = interval {
                s.toggle.push(interval);
            }
            let view = table_view(s.id, &s.spec, window, cx);
            // End expanded, so the sorts lay out the most rows.
            view.update(cx, |view, cx| view.set_expanded(done % 2 == 0, cx));
            s.phase = if done + 1 >= TOGGLES {
                report("expand", &s.toggle);
                Phase::Sort(0, false)
            } else {
                Phase::Toggle(done + 1)
            };
        }
        Phase::Sort(col, ascending) => {
            if let Some(interval) = interval {
                s.sort.push(interval);
            }
            let view = table_view(s.id, &s.spec, window, cx);
            let started = Instant::now();
            let sort = if ascending {
                Direction::Ascending
            } else {
                Direction::Descending
            };
            let first = view.update(cx, |view, cx| {
                view.table_mut().set_sort(Some((col, sort)));
                cx.notify();
                view.table().cell(0, col).cloned()
            });
            eprintln!(
                "table_stress: sort column {col} {sort:?} in {:?}, first cell {first:?}",
                started.elapsed()
            );
            s.phase = match (col + 1 < s.spec.columns.len(), ascending) {
                (_, false) => Phase::Sort(col, true),
                (true, true) => Phase::Sort(col + 1, false),
                (false, true) => {
                    report("sort", &s.sort);
                    if s.quit {
                        cx.quit();
                    }
                    Phase::Done
                }
            };
        }
        Phase::Done => {}
    }
}

fn report(what: &str, intervals: &[Duration]) {
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
        "table_stress: {what} of {} frames: avg {:?}, p50 {:?}, p95 {:?}, max {:?}",
        sorted.len(),
        total / n as u32,
        pick(0.5),
        pick(0.95),
        sorted.last().copied().unwrap_or_default(),
    );
}
