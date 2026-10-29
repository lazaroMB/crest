//! Renderer of the `training_curves` block kind (`show_training_curves`):
//! ML training curves on the plot painter.
//!
//! A summary line comes first (runs, metrics, smoothing). Chips pick the
//! metric and the x axis (step, epoch or time when every run has it), and
//! toggle the raw values and a logarithmic y axis. The plot draws every
//! run's smoothed curve in its colour over the raw values drawn faint,
//! and its best value as a point ([`TrainingSpec::plot`]). Under it, the
//! engine's summary of the metric per run (count, best and last with
//! their steps, minimum, maximum) on the tree table, then the envelope's
//! provenance and the reader's notes.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::science::training::{Goal, TrainingAxis, TrainingSpec, value_text};
use domain::widget::tree::{ColumnKind, TreeColumn, TreeNode, TreeSpec};
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use serde_json::{Map, json};

use super::custom::node_id;
use crate::theme::token;
use crate::timeline::TimelineEntry;
use crate::widgets;

/// Notes listed.
const LISTED: usize = 5;

/// The summary text.
#[must_use]
pub fn summary_text(spec: &TrainingSpec) -> String {
    format!(
        "{} runs · {} metrics · EMA {} (raw drawn faint)",
        spec.runs.len(),
        spec.metrics.len(),
        spec.smoothing
    )
}

/// The engine's summary of `metric`, one row per run.
#[must_use]
pub fn summary_table(spec: &TrainingSpec, metric: &str) -> TreeSpec {
    let names = ["n", "best", "best step", "last", "last step", "min", "max"];
    let columns = names
        .iter()
        .map(|n| TreeColumn {
            name: (*n).to_owned(),
            kind: if n.ends_with("step") || *n == "n" {
                ColumnKind::Number
            } else {
                ColumnKind::Text
            },
            unit: None,
        })
        .collect();
    let nodes = spec
        .summaries
        .iter()
        .filter(|s| s.metric == metric)
        .map(|s| {
            let step = |row| spec.x_at(&s.run, row, TrainingAxis::Step);
            let best = s.best();
            let mut values = Map::new();
            values.insert("n".into(), json!(s.n));
            values.insert("best".into(), json!(value_text(best.value)));
            values.insert("best step".into(), json!(step(best.row)));
            values.insert("last".into(), json!(value_text(s.last.value)));
            values.insert("last step".into(), json!(step(s.last.row)));
            values.insert("min".into(), json!(value_text(s.min.value)));
            values.insert("max".into(), json!(value_text(s.max.value)));
            TreeNode {
                label: s.run.clone(),
                badge: Some(
                    match s.goal {
                        Goal::Minimize => "lower is better",
                        Goal::Maximize => "higher is better",
                    }
                    .into(),
                ),
                tone: None,
                values,
                children: Vec::new(),
            }
        })
        .collect();
    TreeSpec {
        title: Some(format!("{metric} by run")),
        columns,
        nodes,
        expand_depth: 1,
    }
}

/// Per-block state: the spec and what is shown.
pub struct TrainingView {
    id: BlockId,
    spec: Rc<TrainingSpec>,
    metric: String,
    axis: TrainingAxis,
    log_y: bool,
    raw: bool,
    table: Rc<TreeSpec>,
}

impl TrainingView {
    fn new(id: BlockId, spec: &TrainingSpec) -> Self {
        Self {
            id,
            metric: spec.metric.clone(),
            axis: spec.axis,
            log_y: spec.log_y,
            raw: true,
            table: Rc::new(summary_table(spec, &spec.metric)),
            spec: Rc::new(spec.clone()),
        }
    }

    fn sync(&mut self, spec: &TrainingSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The metric shown.
    pub fn metric(&self) -> &str {
        &self.metric
    }

    /// Shows `metric`.
    pub fn set_metric(&mut self, metric: &str, cx: &mut Context<Self>) {
        if self.spec.metrics.iter().any(|m| m == metric) {
            self.metric = metric.to_owned();
            self.table = Rc::new(summary_table(&self.spec, metric));
            cx.notify();
        }
    }

    /// The x axis.
    pub fn axis(&self) -> TrainingAxis {
        self.axis
    }

    /// Plots against `axis`, when every run has it.
    pub fn set_axis(&mut self, axis: TrainingAxis, cx: &mut Context<Self>) {
        if self.spec.axes().contains(&axis) {
            self.axis = axis;
            cx.notify();
        }
    }

    /// Whether the y axis is logarithmic.
    pub fn log_y(&self) -> bool {
        self.log_y
    }

    /// Switches the y axis between linear and logarithmic.
    pub fn toggle_log(&mut self, cx: &mut Context<Self>) {
        self.log_y = !self.log_y;
        cx.notify();
    }

    /// Shows or hides the raw values under the smoothed curves.
    pub fn toggle_raw(&mut self, cx: &mut Context<Self>) {
        self.raw = !self.raw;
        cx.notify();
    }

    fn chip(
        &self,
        id: String,
        label: String,
        on: bool,
        cx: &mut Context<Self>,
        click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .when(on, |d| {
                d.bg(token(cx, "accent.bg"))
                    .text_color(token(cx, "accent.fg"))
            })
            .when(!on, |d| d.text_color(muted))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                click(this, cx);
            }))
            .child(SharedString::from(label))
            .into_any_element()
    }
}

impl Render for TrainingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plot = self
            .spec
            .plot(&self.metric, self.axis, self.log_y, self.raw);
        let canvas = super::plot::static_plot(&plot, &format!("training-{}", self.id), window, cx);
        let table = Rc::clone(&self.table);
        let table = widgets::tree::render(node_id(self.id, 0), &table, window, cx);
        let metrics: Vec<AnyElement> = self
            .spec
            .metrics
            .clone()
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                let on = m == self.metric;
                let name = m.clone();
                self.chip(
                    format!("training-metric-{i}"),
                    m,
                    on,
                    cx,
                    move |this, cx| {
                        this.set_metric(&name, cx);
                    },
                )
            })
            .collect();
        let axes: Vec<AnyElement> = self
            .spec
            .axes()
            .into_iter()
            .map(|a| {
                let key = match a {
                    TrainingAxis::Step => "step",
                    TrainingAxis::Epoch => "epoch",
                    TrainingAxis::Time => "time",
                };
                self.chip(
                    format!("training-x-{key}"),
                    a.name().to_owned(),
                    a == self.axis,
                    cx,
                    move |this, cx| this.set_axis(a, cx),
                )
            })
            .collect();
        let log = self.chip(
            "training-log".into(),
            "log y".into(),
            self.log_y,
            cx,
            |this, cx| {
                this.toggle_log(cx);
            },
        );
        let raw = self.chip(
            "training-raw".into(),
            "raw".into(),
            self.raw,
            cx,
            |this, cx| {
                this.toggle_raw(cx);
            },
        );
        let muted = cx.theme().muted_foreground;
        let warnings = self.spec.warnings.clone();
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "training".to_owned())
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "training-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .text_xs()
                    .child(div().text_color(muted).child("Metric"))
                    .children(metrics)
                    .child(div().text_color(muted).child("· x"))
                    .children(axes)
                    .child(div().text_color(muted).child("·"))
                    .child(log)
                    .child(raw),
            )
            .child(canvas)
            .child(table)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "training-provenance".to_owned())
                    .child(super::computed::provenance_text(&self.spec.stats)),
            )
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "training-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone()))),
                )
            })
    }
}

/// The curves entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &TrainingSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<TrainingView> {
    let view = super::block_state(id, window, cx, |_, _| TrainingView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::TrainingCurves(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::computed::{Computed, Engine};
    use domain::science::training::{
        AtRow, MetricSeries, RunAxes, SeriesSummary, TrainingRun, ema,
    };
    use gpui::{Modifiers, TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> TrainingSpec {
        let series = |metric: &str, values: Vec<f64>| MetricSeries {
            metric: metric.into(),
            rows: (0..values.len()).collect(),
            smoothed: ema(&values, 0.5),
            values,
        };
        let summary = |metric: &str, goal| SeriesSummary {
            run: "a".into(),
            metric: metric.into(),
            n: 3,
            min: AtRow { value: 0.2, row: 2 },
            max: AtRow { value: 1.0, row: 0 },
            last: AtRow { value: 0.2, row: 2 },
            goal,
            goal_given: false,
        };
        TrainingSpec {
            title: None,
            paths: vec!["runs/a.csv".into()],
            runs: vec![TrainingRun {
                name: "a".into(),
                axes: RunAxes {
                    step: vec![1.0, 2.0, 3.0],
                    epoch: Some(vec![0.0, 0.0, 1.0]),
                    time: None,
                },
                series: vec![
                    series("loss", vec![1.0, 0.5, 0.2]),
                    series("acc", vec![0.2, 0.5, 1.0]),
                ],
            }],
            metrics: vec!["loss".into(), "acc".into()],
            metric: "loss".into(),
            axis: TrainingAxis::Step,
            smoothing: 0.5,
            log_y: false,
            summaries: vec![
                summary("loss", Goal::Minimize),
                summary("acc", Goal::Maximize),
            ],
            stats: Computed::new(
                "ml_1",
                "ml.training_curves",
                "s",
                Engine {
                    name: "crest-curves".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            warnings: vec!["note".into()],
        }
    }

    #[test]
    fn summary_and_table() {
        assert_eq!(
            summary_text(&spec()),
            "1 runs · 2 metrics · EMA 0.5 (raw drawn faint)"
        );
        let t = summary_table(&spec(), "acc");
        assert_eq!(t.nodes.len(), 1);
        assert_eq!(t.nodes[0].values["best"], json!("1"));
        assert_eq!(t.nodes[0].values["best step"], json!(1.0));
    }

    #[gpui::test]
    fn draws_and_switches_metric_axis_and_scale(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (timeline, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::TrainingCurves(Box::new(spec())));
        let id = block.id;
        timeline.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1400.)), |_, _| {
            div().size_full().child(timeline.clone())
        });
        cx.run_until_parked();
        for selector in [
            "training",
            "training-summary",
            "training-metric-1",
            "training-x-epoch",
            "training-log",
            "training-provenance",
            "training-warnings",
            "tree",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let view = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        for selector in ["training-metric-1", "training-x-epoch", "training-log"] {
            let at = cx.debug_bounds(selector).unwrap().center();
            cx.simulate_click(at, Modifiers::none());
        }
        assert_eq!(view.read_with(cx, |v, _| v.metric().to_owned()), "acc");
        assert_eq!(view.read_with(cx, |v, _| v.axis()), TrainingAxis::Epoch);
        assert!(view.read_with(cx, |v, _| v.log_y()));
    }
}
