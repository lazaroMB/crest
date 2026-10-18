//! Plugin modals over the window: one with every node kind (images, HTML,
//! markdown, a CSV-like table, a chart, JSON, Mermaid, a diff, a link),
//! a small anchored one, an update and the closing, as `app.modal.*`
//! would push them.
//!
//! `cargo run -p ui --example modal [--quit]`
//!
//! There is no screenshot in CI, so each scripted step prints the open
//! modals and the bounds computed for the current window size. With
//! `--quit` the app exits after the script; otherwise the modals stay
//! open for manual use (Esc closes the front one, drag and ctrl+wheel
//! pan and zoom the images).

#[path = "support/mod.rs"]
mod support;

use std::cell::Cell;
use std::rc::Rc;

use application::ports::events::DomainEvent;
use base64::Engine as _;
use domain::block::{
    Cell as TableCell, ChartKind, ChartSpec, Column, CustomNode, ImageData, ImageFit, Series,
    TableSpec,
};
use domain::modal::{ModalId, ModalSpec, Percent, modal_bounds};
use gpui::{App, AppContext as _, Entity, Window};
use ui::app::RootView;
use ui::modal::ModalHost;

/// Frames between two scripted steps.
const STEP_FRAMES: usize = 40;

/// A 1×1 transparent PNG.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="480" height="160" viewBox="0 0 480 160">
<rect width="480" height="160" rx="12" fill="#1e293b"/>
<rect x="20" y="100" width="60" height="40" fill="#38bdf8"/>
<rect x="100" y="70" width="60" height="70" fill="#38bdf8"/>
<rect x="180" y="40" width="60" height="100" fill="#38bdf8"/>
<rect x="260" y="60" width="60" height="80" fill="#38bdf8"/>
<rect x="340" y="20" width="60" height="120" fill="#22c55e"/>
<text x="20" y="30" fill="#e2e8f0" font-size="18" font-family="sans-serif">coverage by week</text>
</svg>"##;

fn text(s: &str) -> CustomNode {
    CustomNode::Text { text: s.into() }
}

fn spec(id: &str, title: &str, size: (f32, f32), position: &str, content: CustomNode) -> ModalSpec {
    let percent = |v: f32| Percent::new(v).unwrap_or_default();
    ModalSpec {
        id: ModalId::new(format!("demo/{id}")),
        plugin: "demo".into(),
        title: title.into(),
        width: percent(size.0),
        height: percent(size.1),
        position: position.parse().unwrap_or_default(),
        content,
    }
}

fn everything() -> CustomNode {
    let svg = base64::engine::general_purpose::STANDARD.encode(SVG);
    CustomNode::Col {
        children: vec![
            CustomNode::Markdown {
                source: "# Coverage report\n\n**87.5%** of lines covered. See [the docs](https://example.com/coverage).".into(),
            },
            CustomNode::Image {
                source: ImageData::Base64 { data: svg },
                media_type: Some("image/svg+xml".into()),
                alt: Some("coverage by week (svg)".into()),
                fit: ImageFit::Contain,
            },
            CustomNode::Image {
                source: ImageData::Base64 { data: PIXEL.into() },
                media_type: Some("image/png".into()),
                alt: Some("a 1×1 png at 1:1".into()),
                fit: ImageFit::Actual,
            },
            CustomNode::Html {
                source: "<h3>Slowest tests</h3><ol><li><code>parser::big</code> 2.1 s</li><li><b>ui::stress</b> 1.4 s</li></ol>".into(),
            },
            CustomNode::Table(TableSpec {
                columns: vec![Column::new("file"), Column::new("lines"), Column::new("covered")],
                rows: (0..40)
                    .map(|i| {
                        vec![
                            TableCell::Text(format!("src/module_{i}.rs")),
                            TableCell::Number(f64::from(100 + i * 7)),
                            TableCell::Number(f64::from(60 + (i * 13) % 40)),
                        ]
                    })
                    .collect(),
            }),
            CustomNode::Chart(ChartSpec {
                title: "Coverage".into(),
                kind: ChartKind::Line,
                labels: vec!["w1".into(), "w2".into(), "w3".into(), "w4".into()],
                series: vec![Series {
                    name: "%".into(),
                    values: vec![71.0, 78.5, 83.0, 87.5],
                }],
            }),
            CustomNode::Json {
                value: serde_json::json!({
                    "total": 87.5,
                    "crates": { "domain": 96.1, "ui": 74.3, "bin": [1, 2, 3] },
                    "flaky": null
                }),
                expand_depth: 2,
            },
            CustomNode::Mermaid {
                source: "graph LR; lua[app.modal.open] --> sink[EventSink] --> ui[ModalHost]".into(),
            },
            CustomNode::Diff {
                unified: "@@ -1,2 +1,2 @@\n-let x = 1;\n+let x = 2;\n context\n".into(),
            },
            CustomNode::Link {
                text: "Open the full report in the browser".into(),
                url: "https://example.com/report".into(),
            },
        ],
    }
}

/// One scripted step on the host.
type Step = (&'static str, Box<dyn Fn() -> Option<DomainEvent>>);

fn script() -> Vec<Step> {
    vec![
        (
            "open the report (90% × 90%, center)",
            Box::new(|| {
                Some(DomainEvent::ModalOpened {
                    spec: spec(
                        "report",
                        "Coverage report",
                        (90.0, 90.0),
                        "center",
                        everything(),
                    ),
                })
            }),
        ),
        (
            "open a note (30% × 25%, top-right)",
            Box::new(|| {
                Some(DomainEvent::ModalOpened {
                    spec: spec(
                        "note",
                        "Note",
                        (30.0, 25.0),
                        "top-right",
                        text("Tests are running…"),
                    ),
                })
            }),
        ),
        (
            "update the note",
            Box::new(|| {
                Some(DomainEvent::ModalUpdated {
                    id: ModalId::new("demo/note"),
                    content: CustomNode::Markdown {
                        source: "**Done**: 412 passed, 0 failed".into(),
                    },
                })
            }),
        ),
        (
            "close the note",
            Box::new(|| {
                Some(DomainEvent::ModalClosed {
                    id: ModalId::new("demo/note"),
                })
            }),
        ),
        (
            "open a full-size modal (100% keeps the 20 px margin)",
            Box::new(|| {
                Some(DomainEvent::ModalOpened {
                    spec: spec(
                        "full",
                        "Full",
                        (100.0, 100.0),
                        "bottom-left",
                        text("full size"),
                    ),
                })
            }),
        ),
        (
            "close the full-size modal",
            Box::new(|| {
                Some(DomainEvent::ModalClosed {
                    id: ModalId::new("demo/full"),
                })
            }),
        ),
    ]
}

fn main() {
    let quit = std::env::args().any(|arg| arg == "--quit");
    support::run("crest — plugin modals", move |window, cx| {
        let host = cx.new(|cx| ModalHost::new(window, cx));
        let root = cx.new(|cx| {
            let mut root = RootView::new(cx);
            root.push_overlay(host.clone(), cx);
            root
        });
        let steps = Rc::new(script());
        schedule(window, host, steps, Rc::new(Cell::new(0)), quit);
        root.into()
    });
}

fn schedule(
    window: &Window,
    host: Entity<ModalHost>,
    steps: Rc<Vec<Step>>,
    frame: Rc<Cell<usize>>,
    quit: bool,
) {
    window.on_next_frame(move |window, cx| {
        frame.set(frame.get() + 1);
        window.refresh();
        let n = frame.get();
        if n.is_multiple_of(STEP_FRAMES) {
            let ix = n / STEP_FRAMES - 1;
            match steps.get(ix) {
                Some((name, event)) => {
                    if let Some(event) = event() {
                        host.update(cx, |host, cx| host.apply(event, window, cx));
                    }
                    report(name, &host, window, cx);
                }
                None => {
                    eprintln!("modal demo: done after {n} frames");
                    if quit {
                        cx.quit();
                    }
                    return;
                }
            }
        }
        schedule(window, host, steps, frame, quit);
    });
}

fn report(step: &str, host: &Entity<ModalHost>, window: &Window, cx: &App) {
    let viewport = window.viewport_size();
    let (w, h) = (f32::from(viewport.width), f32::from(viewport.height));
    let host = host.read(cx);
    let modals: Vec<String> = host
        .ids()
        .iter()
        .filter_map(|id| host.spec(id))
        .map(|spec| {
            let r = modal_bounds(w, h, spec);
            format!(
                "{} {}×{}% {} at ({:.0}, {:.0}) {:.0}×{:.0}",
                spec.id,
                spec.width.get(),
                spec.height.get(),
                spec.position,
                r.x,
                r.y,
                r.width,
                r.height
            )
        })
        .collect();
    eprintln!(
        "modal demo: {step}; window {w:.0}×{h:.0}; open: [{}]",
        modals.join("; ")
    );
}
