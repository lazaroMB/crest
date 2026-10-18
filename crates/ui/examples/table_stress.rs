//! Stress test of the table block: a 10 000-row table in the timeline,
//! scrolled top to bottom a few rows per frame, then sorted by each column
//! while the frame interval is logged.
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
use gpui_component::table::ColumnSort;
use ui::app::RootView;
use ui::blocks::table::table_state;
use ui::timeline::TimelineView;

const ROWS: usize = 10_000;
/// Rows the table scrolls per frame during the sweep.
const STEP: usize = 40;

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
            markdown: format!("Tamaños de **{ROWS}** archivos:"),
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
            scroll: Vec::new(),
            sort: Vec::new(),
        }));
        schedule(window, timeline, stats);
        root.into()
    });
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Load,
    /// Scrolling; the row at the top of the viewport.
    Scroll(usize),
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
    scroll: Vec<Duration>,
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
            s.phase = Phase::Scroll(0);
        }
        Phase::Scroll(row) => {
            if let Some(interval) = interval {
                s.scroll.push(interval);
            }
            let state = table_state(s.id, &s.spec, window, cx);
            state.update(cx, |state, cx| state.scroll_to_row(row, cx));
            if row % 2000 < STEP {
                eprintln!("table_stress: scrolled to row {row}");
            }
            s.phase = if row + STEP >= ROWS {
                report("scroll", &s.scroll);
                Phase::Sort(0, false)
            } else {
                Phase::Scroll(row + STEP)
            };
        }
        Phase::Sort(col, ascending) => {
            if let Some(interval) = interval {
                s.sort.push(interval);
            }
            let state = table_state(s.id, &s.spec, window, cx);
            let started = Instant::now();
            let sort = if ascending {
                ColumnSort::Ascending
            } else {
                ColumnSort::Descending
            };
            let first = state.update(cx, |state, cx| {
                state.delegate_mut().sort(col, sort);
                state.scroll_to_row(0, cx);
                cx.notify();
                let delegate = state.delegate();
                delegate
                    .order()
                    .first()
                    .map(|&ix| delegate.spec().rows[ix][col].clone())
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
