//! Renderer of the `query_plan` block kind (`show_query_plan`): a SQL
//! query plan on the tree table widget.
//!
//! A summary line comes first: the engine and options, the node count,
//! the planning and execution times and the hot node count. Two lines
//! follow when there is something to say: the slowest nodes by self time
//! (with their share) and the worst row misestimates. Then the plan
//! itself ([`crate::widgets::tree`], built by
//! [`QueryPlanSpec::to_tree`]): time, self time and rows as bars,
//! estimates, loops, buffers and cost per node, hot nodes badged in the
//! danger color, misestimates and spills in the warning color.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::devops::query_plan::{MISESTIMATE_FACTOR, QueryPlanSpec, round_factor};
use domain::widget::timeline::{TimeUnit, format_duration};
use domain::widget::tree::{TreeSpec, format_number};
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use super::trace::caption;
use crate::timeline::TimelineEntry;
use crate::widgets;

/// Nodes named in the "slowest" and "misestimates" lines.
pub const TOP_NODES: usize = 3;

/// The summary text: engine, options, nodes and times.
#[must_use]
pub fn summary_text(spec: &QueryPlanSpec) -> String {
    let mut text = format!("{} · EXPLAIN", spec.engine.name());
    if spec.analyzed {
        text.push_str(" ANALYZE");
    }
    if spec.buffers {
        text.push_str(" BUFFERS");
    }
    text.push_str(&format!(" · {} nodes", spec.nodes.len()));
    if let Some(t) = spec.planning_time {
        text.push_str(&format!(" · planning {}", format_duration(t, TimeUnit::Ms)));
    }
    if let Some(t) = spec.execution_time {
        text.push_str(&format!(
            " · execution {}",
            format_duration(t, TimeUnit::Ms)
        ));
    }
    if !spec.analyzed
        && let Some(cost) = spec.nodes.first().and_then(|n| n.total_cost)
    {
        text.push_str(&format!(" · cost {}", format_number(cost)));
    }
    text
}

/// `Seq Scan on orders 49%`, one per slowest node.
#[must_use]
pub fn slowest_text(spec: &QueryPlanSpec) -> Vec<String> {
    spec.slowest(TOP_NODES)
        .into_iter()
        .map(|ix| {
            let share = spec.self_share(ix).unwrap_or(0.0) * 100.0;
            format!("{} {share:.0}%", spec.nodes[ix].label())
        })
        .collect()
}

/// `Hash Join ×96 under`, one per bad estimate.
#[must_use]
pub fn misestimate_text(spec: &QueryPlanSpec) -> Vec<String> {
    spec.misestimates(MISESTIMATE_FACTOR, TOP_NODES)
        .into_iter()
        .map(|m| {
            format!(
                "{} ×{} {}",
                spec.nodes[m.node].label(),
                format_number(round_factor(m.factor)),
                if m.under { "under" } else { "over" }
            )
        })
        .collect()
}

/// Per-block state: the spec and its tree.
pub struct QueryPlanView {
    id: BlockId,
    spec: Rc<QueryPlanSpec>,
    tree: Rc<TreeSpec>,
}

impl QueryPlanView {
    fn new(id: BlockId, spec: &QueryPlanSpec) -> Self {
        let mut tree = spec.to_tree();
        tree.title = Some(caption(
            spec.title.as_deref(),
            spec.path.as_deref(),
            "Query plan",
        ));
        Self {
            id,
            spec: Rc::new(spec.clone()),
            tree: Rc::new(tree),
        }
    }

    fn sync(&mut self, spec: &QueryPlanSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The tree drawn.
    pub fn tree(&self) -> &TreeSpec {
        &self.tree
    }

    fn summary(&self, cx: &App) -> AnyElement {
        let spec = &self.spec;
        let theme = cx.theme();
        let hot = (0..spec.nodes.len()).filter(|&ix| spec.is_hot(ix)).count();
        let line = |selector: &'static str, head: &str, items: Vec<String>, color| {
            (!items.is_empty()).then(|| {
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .debug_selector(move || selector.to_owned())
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(head.to_owned()),
                    )
                    .children(
                        items
                            .into_iter()
                            .map(|item| div().text_color(color).child(item)),
                    )
            })
        };
        v_flex()
            .gap_0p5()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .debug_selector(|| "query-plan-summary".to_owned())
                    .child(summary_text(spec))
                    .when(hot > 0, |d| {
                        d.child(div().text_color(theme.danger).child(format!("{hot} hot")))
                    }),
            )
            .children(line(
                "query-plan-slowest",
                "slowest",
                slowest_text(spec),
                theme.danger,
            ))
            .children(line(
                "query-plan-misestimates",
                "misestimates",
                misestimate_text(spec),
                theme.warning,
            ))
            .into_any_element()
    }
}

impl Render for QueryPlanView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = self.tree.clone();
        let widget = widgets::tree::render(node_id(self.id, 0), &tree, window, cx);
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "query-plan".to_owned())
            .child(self.summary(cx))
            .child(widget)
    }
}

/// The plan entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &QueryPlanSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<QueryPlanView> {
    let view = super::block_state(id, window, cx, |_, _| QueryPlanView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::QueryPlan(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::devops::query_plan::{PlanEngine, PlanNode};
    use gpui::{TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> QueryPlanSpec {
        let node = |node_type: &str, depth, time, rows, planned| PlanNode {
            node_type: node_type.into(),
            depth,
            actual_time: Some(time),
            actual_rows: Some(rows),
            plan_rows: Some(planned),
            loops: Some(1.0),
            ..PlanNode::default()
        };
        let mut scan = node("Seq Scan", 1, 70.0, 10_000.0, 9_000.0);
        scan.relation = Some("orders".into());
        let mut spec = QueryPlanSpec {
            title: None,
            path: Some("plans/q.json".into()),
            engine: PlanEngine::Postgres,
            analyzed: true,
            buffers: false,
            planning_time: Some(0.5),
            execution_time: Some(100.0),
            trigger_time: None,
            jit_time: None,
            nodes: vec![
                node("Hash Join", 0, 100.0, 5_000.0, 50.0),
                scan,
                node("Hash", 1, 20.0, 10.0, 10.0),
            ],
        };
        spec.analyse();
        spec
    }

    #[test]
    fn summary_lines_name_the_hot_spots() {
        let spec = spec();
        assert_eq!(
            summary_text(&spec),
            "PostgreSQL · EXPLAIN ANALYZE · 3 nodes · planning 500 µs · execution 100 ms"
        );
        assert_eq!(
            slowest_text(&spec),
            ["Seq Scan on orders 70%", "Hash 20%", "Hash Join 10%"]
        );
        assert_eq!(misestimate_text(&spec), ["Hash Join ×100 under"]);
    }

    #[gpui::test]
    fn draws_the_summary_and_the_plan_tree(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::QueryPlan(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "query-plan",
            "query-plan-summary",
            "query-plan-slowest",
            "query-plan-misestimates",
            "tree",
            "tree-row-2",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let plan = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert_eq!(
            plan.read_with(cx, |p, _| p.tree().title.clone()).as_deref(),
            Some("q.json")
        );
    }
}
