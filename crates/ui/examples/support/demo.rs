//! Scripted block demos: a timeline seeded with blocks, then normal-mode
//! actions replayed every few frames while the frame interval is logged.
//!
//! There is no screenshot in CI, so the log is the evidence: each step
//! prints the action, the selected block and its view flags, and the run
//! ends with a frame-interval summary. With `--quit` the app exits after
//! the script; otherwise it stays open for manual use (`j`/`k` are not
//! bound here, the script drives the timeline).

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use application::keymap::Action;
use application::ports::events::{DomainEvent, EventSink as _};
use application::testing::system::RecordingSink;
use domain::block::Block;
use gpui::{App, AppContext as _, Entity, Window};
use ui::app::RootView;
use ui::timeline::TimelineView;

/// Frames between two scripted steps.
const STEP_FRAMES: usize = 30;

/// Opens a window titled `title` with `blocks` in the timeline and replays
/// `script` (builtin action names such as `"block.toggle_view"`).
pub fn run(title: &'static str, blocks: Vec<Block>, script: Vec<&'static str>) {
    let quit = std::env::args().any(|arg| arg == "--quit");
    let (sink, rx) = RecordingSink::with_channel();
    let total = blocks.len();
    for block in blocks {
        sink.emit(DomainEvent::BlockAdded { block });
    }
    crate::support::run(title, move |window, cx| {
        let timeline = cx.new(|cx| TimelineView::with_events(rx, cx));
        let root = cx.new(|cx| {
            let mut root = RootView::new(cx);
            root.set_timeline(timeline.clone(), cx);
            root
        });
        let demo = Rc::new(RefCell::new(Demo {
            title,
            total,
            script,
            quit,
            started: Instant::now(),
            frames: 0,
            last_frame: None,
            intervals: Vec::new(),
            next_step: 0,
            done: false,
        }));
        schedule(window, timeline, demo);
        root.into()
    });
}

struct Demo {
    title: &'static str,
    total: usize,
    script: Vec<&'static str>,
    quit: bool,
    started: Instant,
    frames: usize,
    last_frame: Option<Instant>,
    intervals: Vec<Duration>,
    next_step: usize,
    done: bool,
}

fn schedule(window: &Window, timeline: Entity<TimelineView>, demo: Rc<RefCell<Demo>>) {
    window.on_next_frame(move |window, cx| {
        frame(window, cx, &timeline, &demo);
        if !demo.borrow().done {
            schedule(window, timeline, demo);
        }
    });
}

fn frame(
    window: &mut Window,
    cx: &mut App,
    timeline: &Entity<TimelineView>,
    demo: &Rc<RefCell<Demo>>,
) {
    let mut d = demo.borrow_mut();
    let now = Instant::now();
    if let Some(last) = d.last_frame.replace(now) {
        d.intervals.push(now - last);
    }
    d.frames += 1;
    window.refresh();
    let len = timeline.read(cx).state().len();
    if len < d.total {
        return;
    }
    if !d.frames.is_multiple_of(STEP_FRAMES) {
        return;
    }
    let Some(name) = d.script.get(d.next_step).copied() else {
        d.done = true;
        report(d.title, d.started, &d.intervals);
        if d.quit {
            cx.quit();
        }
        return;
    };
    d.next_step += 1;
    let line = timeline.update(cx, |view, cx| {
        view.handle_action(&Action::builtin(name), cx);
        let state = view.state();
        match state.selected_entry() {
            Some(entry) => format!(
                "{name}: selected {:?} ({}) expanded={} alt_view={}",
                state.selected(),
                ui::timeline::kind_name(&entry.block.kind),
                entry.expanded,
                entry.alt_view
            ),
            None => format!("{name}: nothing selected"),
        }
    });
    eprintln!("{}: {line}", d.title);
}

fn report(title: &str, started: Instant, intervals: &[Duration]) {
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
        "{title}: done after {:?}; {} frames: avg {:?}, p50 {:?}, p95 {:?}, max {:?}",
        started.elapsed(),
        sorted.len(),
        total / n as u32,
        pick(0.5),
        pick(0.95),
        sorted.last().copied().unwrap_or_default(),
    );
}
