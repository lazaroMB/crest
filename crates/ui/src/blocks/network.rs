//! Renderer of the `network` block kind (`show_network`): a network on
//! the graph widget with the metrics the engine computed.
//!
//! A summary line comes first (nodes, edges, direction, density,
//! components, communities and modularity). Chips pick the metric the
//! nodes are sized by (degree, betweenness, closeness, PageRank); the
//! graph ([`crate::widgets::graph`]) is a seeded force layout with nodes
//! coloured by community, and focusing a node lists its metrics. The
//! metrics table ([`crate::widgets::tree`]) is sorted by the chosen
//! metric. The envelope's provenance and the engine's notes come last.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::science::network::{NetworkMetric, NetworkSpec, metric_text};
use domain::widget::graph::GraphSpec;
use domain::widget::tree::TreeSpec;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use crate::theme::token;
use crate::timeline::TimelineEntry;
use crate::widgets;

/// Notes listed.
const LISTED: usize = 5;

/// The summary text.
#[must_use]
pub fn summary_text(spec: &NetworkSpec) -> String {
    let mut parts = vec![
        format!("{} nodes", spec.nodes.len()),
        format!("{} edges", spec.edges.len()),
        if spec.directed {
            "directed"
        } else {
            "undirected"
        }
        .to_owned(),
    ];
    if spec.weighted {
        parts.push("weighted".into());
    }
    parts.push(format!("density {}", metric_text(spec.density())));
    let mut components = format!("{} components", spec.components);
    if let Some(s) = spec.strong_components {
        components.push_str(&format!(" ({s} strong)"));
    }
    parts.push(components);
    parts.push(format!(
        "{} communities · modularity {}",
        spec.communities,
        metric_text(spec.modularity)
    ));
    parts.join(" · ")
}

/// Per-block state: the spec, the sizing metric and the graph and table
/// built for it.
pub struct NetworkView {
    id: BlockId,
    spec: Rc<NetworkSpec>,
    metric: NetworkMetric,
    graph: Rc<GraphSpec>,
    table: Rc<TreeSpec>,
}

impl NetworkView {
    fn new(id: BlockId, spec: &NetworkSpec) -> Self {
        let metric = spec.size_by;
        Self {
            id,
            graph: Rc::new(spec.graph(metric)),
            table: Rc::new(spec.table(metric)),
            spec: Rc::new(spec.clone()),
            metric,
        }
    }

    fn sync(&mut self, spec: &NetworkSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The metric nodes are sized by.
    pub fn metric(&self) -> NetworkMetric {
        self.metric
    }

    /// Sizes the nodes (and sorts the table) by `metric`.
    pub fn set_metric(&mut self, metric: NetworkMetric, cx: &mut Context<Self>) {
        if metric != self.metric {
            self.metric = metric;
            self.graph = Rc::new(self.spec.graph(metric));
            self.table = Rc::new(self.spec.table(metric));
            cx.notify();
        }
    }

    /// The graph drawn.
    pub fn graph(&self) -> &GraphSpec {
        &self.graph
    }

    /// The metrics table.
    pub fn table(&self) -> &TreeSpec {
        &self.table
    }
}

impl Render for NetworkView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let graph = Rc::clone(&self.graph);
        let graph = widgets::graph::render(node_id(self.id, 0), &graph, window, cx);
        let table = Rc::clone(&self.table);
        let table = widgets::tree::render(node_id(self.id, 1), &table, window, cx);
        let muted = cx.theme().muted_foreground;
        let current = self.metric;
        let chips: Vec<AnyElement> = NetworkMetric::ALL
            .into_iter()
            .map(|m| {
                let on = m == current;
                let key = m.key();
                div()
                    .id(SharedString::from(format!("network-size-{key}")))
                    .debug_selector(move || format!("network-size-{key}"))
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
                        this.set_metric(m, cx);
                    }))
                    .child(m.label())
                    .into_any_element()
            })
            .collect();
        let warnings = self.spec.warnings.clone();
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "network".to_owned())
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "network-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .child(div().text_color(muted).child("Size by"))
                    .children(chips)
                    .child(div().text_color(muted).child("· colour: community")),
            )
            .child(graph)
            .child(table)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "network-provenance".to_owned())
                    .child(super::computed::provenance_text(&self.spec.metrics)),
            )
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "network-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone()))),
                )
            })
    }
}

/// The network entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &NetworkSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<NetworkView> {
    let view = super::block_state(id, window, cx, |_, _| NetworkView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Network(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::computed::{Computed, Engine};
    use domain::science::network::{NetworkEdge, NetworkNode};
    use gpui::{Modifiers, TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> NetworkSpec {
        let node = |id: &str, degree: f64, pr: f64| NetworkNode {
            id: id.into(),
            label: None,
            degree,
            in_degree: None,
            out_degree: None,
            strength: degree,
            betweenness: if id == "b" { 1.0 } else { 0.0 },
            closeness: 0.5,
            pagerank: pr,
            component: 0,
            community: usize::from(id == "c"),
        };
        let edge = |a: &str, b: &str| NetworkEdge {
            from: a.into(),
            to: b.into(),
            weight: None,
        };
        NetworkSpec {
            title: None,
            path: Some("net/path.csv".into()),
            directed: false,
            weighted: false,
            nodes: vec![
                node("a", 1.0, 0.6),
                node("b", 2.0, 0.3),
                node("c", 1.0, 0.1),
            ],
            edges: vec![edge("a", "b"), edge("b", "c")],
            components: 1,
            strong_components: None,
            communities: 2,
            modularity: 0.1,
            betweenness_sources: None,
            seed: 1,
            size_by: NetworkMetric::Degree,
            metrics: Computed::new(
                "nw_1",
                "network.metrics",
                "m",
                Engine {
                    name: "crest-graph".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            warnings: vec!["1 self-loops dropped".into()],
        }
    }

    #[test]
    fn summary() {
        assert_eq!(
            summary_text(&spec()),
            "3 nodes · 2 edges · undirected · density 0.6667 · 1 components · 2 communities · \
             modularity 0.1000"
        );
    }

    #[gpui::test]
    fn draws_graph_and_table_and_switches_the_metric(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (timeline, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Network(Box::new(spec())));
        let id = block.id;
        timeline.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1400.)), |_, _| {
            div().size_full().child(timeline.clone())
        });
        cx.run_until_parked();
        for selector in [
            "network",
            "network-summary",
            "network-size-pagerank",
            "network-provenance",
            "network-warnings",
            "tree",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let view = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert_eq!(
            view.read_with(cx, |v, _| v.graph().nodes[1].size),
            Some(1.0),
            "b has the highest degree"
        );
        let chip = cx.debug_bounds("network-size-pagerank").unwrap().center();
        cx.simulate_click(chip, Modifiers::none());
        assert_eq!(
            view.read_with(cx, |v, _| v.metric()),
            NetworkMetric::Pagerank
        );
        assert_eq!(
            view.read_with(cx, |v, _| v.graph().nodes[0].size),
            Some(1.0)
        );
        assert_eq!(
            view.read_with(cx, |v, _| v.table().nodes[0].label.clone()),
            "a"
        );
    }
}
