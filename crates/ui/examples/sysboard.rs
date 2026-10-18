//! The simulation board of a small solution (client → api → db, one
//! passing and one failing scenario) in a full-size modal, drawn by the
//! offscreen wgpu renderer when there is an adapter.
//!
//! `cargo run -p ui --example sysboard [--quit] [--dump <file.ppm>]`
//!
//! There is no screenshot in CI, so the script logs the backend and
//! adapter, plays the scenario, scrubs, orbits and switches scenario,
//! printing how many GPU frames were rendered and what they cost (GPU
//! time and readback). `--dump` also renders one frame offscreen and
//! writes it as a PPM. With `--quit` the app exits after the script.

#[path = "support/mod.rs"]
mod support;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use application::testing::sim;
use domain::sysmodel::scene::{frame_at, scene_of};
use domain::sysmodel::{Board, SysModel, run_all};
use gpui::{App, AppContext as _, Entity, Window};
use ui::app::RootView;
use ui::modal::{ModalHost, board_modal_id};
use ui::render3d::Renderer3d;
use ui::render3d::camera::OrbitCamera;
use ui::sysboard::SysBoardView;

/// Frames between two scripted steps.
const STEP_FRAMES: usize = 45;

fn board() -> Board {
    let model = SysModel::from_value(sim::model()).unwrap_or_else(|e| panic!("fixture: {e}"));
    let runs = run_all(&model).unwrap_or_else(|e| panic!("fixture: {e}"));
    Board {
        spec: sim::FOLDER.into(),
        title: "Orders API".into(),
        model,
        runs,
    }
}

/// Renders the failing scenario mid-run offscreen and writes a PPM.
fn dump(path: &str) {
    let board = board();
    let mut renderer = match Renderer3d::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sysboard: no dump: {e}");
            return;
        }
    };
    let scene = scene_of(&board.model);
    let run = &board.runs[1];
    let t = run.trace.until_us / 40;
    let frame = frame_at(&board.model, &scene, &run.trace, t);
    let camera = OrbitCamera::fit(&scene);
    let size = (1600, 1000);
    for i in 0..5 {
        match renderer.render(&scene, &frame, &camera, Some("api"), size) {
            Ok(out) => {
                eprintln!(
                    "sysboard: offscreen frame {i}: {}x{} {} vertices, gpu {:.2} ms, readback {:.2} ms",
                    out.stats.width,
                    out.stats.height,
                    out.stats.vertices,
                    out.stats.gpu_ms,
                    out.stats.readback_ms
                );
                if i == 4 {
                    let bytes = out.image.as_bytes(0).unwrap_or_default();
                    let mut ppm = format!("P6\n{} {}\n255\n", size.0, size.1).into_bytes();
                    for px in bytes.chunks(4) {
                        ppm.extend_from_slice(&[px[2], px[1], px[0]]);
                    }
                    match std::fs::write(path, ppm) {
                        Ok(()) => eprintln!("sysboard: wrote {path}"),
                        Err(e) => eprintln!("sysboard: cannot write {path}: {e}"),
                    }
                }
            }
            Err(e) => {
                eprintln!("sysboard: offscreen render failed: {e}");
                return;
            }
        }
    }
}

type Step = (
    &'static str,
    fn(&mut SysBoardView, &mut gpui::Context<SysBoardView>),
);

fn script() -> Vec<Step> {
    vec![
        ("opened", |_, _| {}),
        ("play", |v, cx| v.toggle_play(cx)),
        ("playing", |_, _| {}),
        ("scrub to 50%", |v, cx| v.scrub(0.5, cx)),
        ("select api", |v, cx| v.select(Some("api"), cx)),
        ("scenario burst", |v, cx| {
            v.select_scenario(1, cx);
            v.toggle_play(cx);
        }),
        ("burst playing", |_, _| {}),
        ("step forward", |v, cx| v.step_forward(cx)),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let quit = args.iter().any(|a| a == "--quit");
    if let Some(i) = args.iter().position(|a| a == "--dump")
        && let Some(path) = args.get(i + 1)
    {
        dump(path);
    }
    support::run("crest — simulation board", move |window, cx| {
        let host = cx.new(|cx| ModalHost::new(window, cx));
        let root = cx.new(|cx| {
            let mut root = RootView::new(cx);
            root.push_overlay(host.clone(), cx);
            root
        });
        host.update(cx, |host, cx| host.open_board(board(), window, cx));
        let started = Rc::new(Cell::new(Instant::now()));
        schedule(
            window,
            host,
            Rc::new(script()),
            Rc::new(Cell::new(0)),
            started,
            quit,
        );
        root.into()
    });
}

fn view(host: &Entity<ModalHost>, cx: &App) -> Option<Entity<SysBoardView>> {
    host.read(cx)
        .view(&board_modal_id(sim::FOLDER))
        .cloned()
        .and_then(|v| v.downcast::<SysBoardView>().ok())
}

fn schedule(
    window: &Window,
    host: Entity<ModalHost>,
    steps: Rc<Vec<Step>>,
    frame: Rc<Cell<usize>>,
    started: Rc<Cell<Instant>>,
    quit: bool,
) {
    window.on_next_frame(move |window, cx| {
        frame.set(frame.get() + 1);
        window.refresh();
        let n = frame.get();
        if n.is_multiple_of(STEP_FRAMES) {
            let ix = n / STEP_FRAMES - 1;
            let Some(board) = view(&host, cx) else {
                eprintln!("sysboard: the board modal is not open");
                cx.quit();
                return;
            };
            let elapsed = started.get().elapsed();
            started.set(Instant::now());
            report(&board, ix, elapsed, cx);
            match steps.get(ix) {
                Some((_, step)) => board.update(cx, *step),
                None => {
                    eprintln!("sysboard: done after {n} frames");
                    if quit {
                        cx.quit();
                    }
                    return;
                }
            }
        }
        schedule(window, host, steps, frame, started, quit);
    });
}

fn report(board: &Entity<SysBoardView>, ix: usize, elapsed: std::time::Duration, cx: &App) {
    let v = board.read(cx);
    let (stats, frames) = v.render_stats();
    let p = v.playback();
    let step = ix
        .checked_sub(1)
        .and_then(|i| script().get(i).map(|s| s.0))
        .unwrap_or("start");
    let per_frame = elapsed.as_secs_f64() * 1000.0 / STEP_FRAMES as f64;
    let cost = stats.map_or_else(
        || "no GPU frame".to_owned(),
        |s| {
            format!(
                "last GPU frame {}x{} {} vertices: gpu {:.2} ms, readback {:.2} ms",
                s.width, s.height, s.vertices, s.gpu_ms, s.readback_ms
            )
        },
    );
    eprintln!(
        "sysboard: after `{step}`: backend {}; scenario {} t={}/{} µs playing={}; {frames} GPU frames so far; window frame {per_frame:.1} ms avg; {cost}",
        v.backend_name(),
        v.run().map_or("-", |r| r.scenario.as_str()),
        p.t_us,
        p.until_us,
        p.playing,
    );
}
