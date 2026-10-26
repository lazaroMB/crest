//! Renderer of the `gantt` block kind (`show_gantt`): a project scheduled
//! by the critical path method.
//!
//! A summary line comes first (tasks, milestones, project duration and
//! dates, calendar), then the critical path, then the chart on the
//! timeline widget ([`crate::widgets::timeline`]): a bar per task from its
//! early start to its early finish, critical tasks in the danger colour,
//! the total slack drawn faint after each bar, milestones as markers;
//! hovering shows the engine's early/late dates and slack. The schedule
//! table ([`crate::widgets::tree`]) lists every task's numbers, with the
//! envelope's provenance under it. Scheduler notes come last.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::gantt::{Calendar, GanttSpec, days};
use domain::widget::timeline::TimelineSpec;
use domain::widget::tree::TreeSpec;
use gpui::{
    AnyElement, App, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::custom::node_id;
use crate::timeline::TimelineEntry;
use crate::widgets;

/// Notes listed under the chart.
const LISTED: usize = 5;

/// The summary text: tasks, milestones, duration, dates and calendar.
#[must_use]
pub fn summary_text(spec: &GanttSpec) -> String {
    let mut parts = vec![
        format!("{} tasks", spec.tasks.len()),
        format!("{} milestones", spec.milestones().count()),
        format!("{} working days", days(spec.project_duration)),
    ];
    if let Some(start) = &spec.start {
        parts.push(format!(
            "{start} → {}",
            spec.finish_text(spec.project_duration)
        ));
    }
    parts.push(
        match spec.calendar {
            Calendar::AllDays => "every day",
            Calendar::Weekdays => "Mon–Fri",
        }
        .to_owned(),
    );
    parts.join(" · ")
}

/// The critical path as `Critical path: A → B → C`.
#[must_use]
pub fn critical_text(spec: &GanttSpec) -> String {
    let names: Vec<&str> = spec
        .critical_path
        .iter()
        .map(|id| {
            spec.tasks
                .iter()
                .find(|t| &t.id == id)
                .map_or(id.as_str(), |t| t.label())
        })
        .collect();
    format!("Critical path: {}", names.join(" → "))
}

/// Per-block state: the spec, its timeline and its table.
pub struct GanttView {
    id: BlockId,
    spec: Rc<GanttSpec>,
    timeline: Rc<TimelineSpec>,
    table: Rc<TreeSpec>,
}

impl GanttView {
    fn new(id: BlockId, spec: &GanttSpec) -> Self {
        Self {
            id,
            timeline: Rc::new(spec.timeline()),
            table: Rc::new(spec.table()),
            spec: Rc::new(spec.clone()),
        }
    }

    fn sync(&mut self, spec: &GanttSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The chart drawn.
    pub fn timeline(&self) -> &TimelineSpec {
        &self.timeline
    }

    /// The schedule table.
    pub fn table(&self) -> &TreeSpec {
        &self.table
    }
}

impl Render for GanttView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let timeline = Rc::clone(&self.timeline);
        let chart = widgets::timeline::render(node_id(self.id, 0), &timeline, window, cx);
        let table = Rc::clone(&self.table);
        let table = widgets::tree::render(node_id(self.id, 1), &table, window, cx);
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let warnings = self.spec.warnings.clone();
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "gantt".to_owned())
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "gantt-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(danger)
                    .debug_selector(|| "gantt-critical".to_owned())
                    .child(SharedString::from(critical_text(&self.spec))),
            )
            .child(chart)
            .child(table)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "gantt-provenance".to_owned())
                    .child(super::computed::provenance_text(&self.spec.schedule)),
            )
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(theme.warning)
                        .debug_selector(|| "gantt-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone()))),
                )
            })
    }
}

/// The Gantt entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &GanttSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<GanttView> {
    let view = super::block_state(id, window, cx, |_, _| GanttView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Gantt(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::computed::{Computed, Engine};
    use domain::gantt::{Dependency, DependencyKind, GanttTask};
    use domain::widget::timeline::SpanStatus;
    use gpui::{TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> GanttSpec {
        let task = |id: &str, d: f64, es: f64, slack: f64| GanttTask {
            id: id.into(),
            name: Some(format!("Task {id}")),
            group: None,
            duration: d,
            milestone: d == 0.0,
            depends_on: Vec::new(),
            not_before: None,
            early_start: es,
            early_finish: es + d,
            late_start: es + slack,
            late_finish: es + d + slack,
            total_slack: slack,
            free_slack: slack,
            critical: slack == 0.0,
        };
        let mut b = task("b", 2.0, 3.0, 0.0);
        b.depends_on.push(Dependency {
            task: "a".into(),
            kind: DependencyKind::Fs,
            lag: 0.0,
        });
        GanttSpec {
            title: Some("Release".into()),
            path: None,
            start: Some("2026-11-02".into()),
            calendar: Calendar::Weekdays,
            tasks: vec![
                task("a", 3.0, 0.0, 0.0),
                b,
                task("c", 1.0, 3.0, 1.0),
                task("m", 0.0, 5.0, 0.0),
            ],
            project_duration: 5.0,
            critical_path: vec!["a".into(), "b".into(), "m".into()],
            schedule: Computed::new(
                "pl_1",
                "plan.cpm",
                "Critical path method",
                Engine {
                    name: "crest-plan".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            warnings: vec!["note".into()],
        }
    }

    #[test]
    fn summary_and_critical_path() {
        assert_eq!(
            summary_text(&spec()),
            "4 tasks · 1 milestones · 5 working days · 2026-11-02 → 2026-11-06 · Mon–Fri"
        );
        assert_eq!(
            critical_text(&spec()),
            "Critical path: Task a → Task b → Task m"
        );
    }

    #[gpui::test]
    fn draws_chart_table_and_notes(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Gantt(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1200.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "gantt",
            "gantt-summary",
            "gantt-critical",
            "gantt-provenance",
            "gantt-warnings",
            "tree",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let gantt = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        let (critical, slack) = gantt.read_with(cx, |g, _| {
            let rows = &g.timeline().rows;
            (rows[0].spans[0].status, rows[2].spans[1].status)
        });
        assert_eq!(critical, Some(SpanStatus::Critical));
        assert_eq!(slack, Some(SpanStatus::Slack));
        assert_eq!(gantt.read_with(cx, |g, _| g.table().nodes.len()), 4);
    }
}
