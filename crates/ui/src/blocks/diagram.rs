//! Renderer of the `diagram` block kind (`show_dot`, `show_er`): a DOT
//! graph or an ER diagram on the graph widget.
//!
//! A summary line comes first: the source (DOT, or SQL DDL and its
//! dialect), the node and edge (or table and foreign key) counts and the
//! clusters. Then the graph ([`crate::widgets::graph`]): layered layout,
//! clusters (or schemas) as boxes, pan, zoom and a click to focus a node,
//! whose attributes (a table's columns) show in the detail line. An ER
//! diagram adds its tables below ([`crate::widgets::tree`]): columns with
//! their types, PK / FK badges and the referenced column. Skipped
//! statements and DOT features not drawn are listed last.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::devops::diagram::{DiagramSource, DiagramSpec};
use domain::widget::graph::GraphSpec;
use domain::widget::tree::TreeSpec;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use super::trace::file_name;
use crate::timeline::TimelineEntry;
use crate::widgets;

/// Warnings listed under the diagram.
pub const WARNINGS_SHOWN: usize = 5;

/// The summary text: source, counts and clusters.
#[must_use]
pub fn summary_text(spec: &DiagramSpec) -> String {
    let g = &spec.graph;
    match spec.source {
        DiagramSource::Dot => {
            let mut text = format!(
                "DOT · {} · {} nodes · {} edges",
                if g.directed { "digraph" } else { "graph" },
                g.nodes.len(),
                g.edges.len()
            );
            let clusters = g.groups_in_use().len();
            if clusters > 0 {
                text.push_str(&format!(" · {clusters} clusters"));
            }
            text
        }
        DiagramSource::SqlDdl => {
            let mut text = "SQL DDL".to_owned();
            if let Some(dialect) = &spec.dialect {
                text.push_str(&format!(" ({dialect})"));
            }
            text.push_str(&format!(
                " · {} tables · {} foreign keys",
                spec.tables.len(),
                g.edges.len()
            ));
            text
        }
    }
}

/// Per-block state: the spec, its graph (titled) and, for DDL, its
/// tables tree.
pub struct DiagramView {
    id: BlockId,
    spec: Rc<DiagramSpec>,
    graph: Rc<GraphSpec>,
    tables: Option<Rc<TreeSpec>>,
}

impl DiagramView {
    fn new(id: BlockId, spec: &DiagramSpec) -> Self {
        let mut graph = spec.graph.clone();
        graph.title = spec
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| graph.title.clone())
            .or_else(|| spec.path.as_deref().map(|p| file_name(p).to_owned()))
            .or_else(|| {
                Some(match spec.source {
                    DiagramSource::Dot => "Graph".to_owned(),
                    DiagramSource::SqlDdl => "ER diagram".to_owned(),
                })
            });
        let tables = (!spec.tables.is_empty()).then(|| Rc::new(spec.tables_tree()));
        Self {
            id,
            spec: Rc::new(spec.clone()),
            graph: Rc::new(graph),
            tables,
        }
    }

    fn sync(&mut self, spec: &DiagramSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The graph drawn.
    pub fn graph(&self) -> &GraphSpec {
        &self.graph
    }

    /// The tables tree of an ER diagram.
    pub fn tables(&self) -> Option<&TreeSpec> {
        self.tables.as_deref()
    }
}

impl Render for DiagramView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let graph = self.graph.clone();
        let widget = widgets::graph::render(node_id(self.id, 0), &graph, window, cx);
        let tables = self
            .tables
            .clone()
            .map(|tree| widgets::tree::render(node_id(self.id, 1), &tree, window, cx));
        let theme = cx.theme();
        let warnings = &self.spec.warnings;
        let notes = warnings
            .iter()
            .take(WARNINGS_SHOWN)
            .map(|w| div().child(w.clone()));
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "diagram".to_owned())
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .debug_selector(|| "diagram-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .child(widget)
            .children(tables)
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(theme.warning)
                        .debug_selector(|| "diagram-warnings".to_owned())
                        .children(notes)
                        .when(warnings.len() > WARNINGS_SHOWN, |d| {
                            d.child(format!("… {} more", warnings.len() - WARNINGS_SHOWN))
                        }),
                )
            })
    }
}

/// The diagram entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &DiagramSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<DiagramView> {
    let view = super::block_state(id, window, cx, |_, _| DiagramView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Diagram(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::devops::diagram::{ErColumn, ErTable};
    use gpui::{TestAppContext, point, px, size};
    use serde_json::json;

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn er() -> DiagramSpec {
        let graph: GraphSpec = serde_json::from_value(json!({
            "nodes": [{"id": "users", "kind": "table"}, {"id": "orders", "kind": "table"}],
            "edges": [{"from": "orders", "to": "users", "label": "user_id → id"}]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let col = |name: &str| ErColumn {
            name: name.into(),
            data_type: "bigint".into(),
            primary_key: name == "id",
            nullable: true,
            unique: false,
            references: None,
        };
        DiagramSpec {
            title: None,
            path: Some("db/schema.sql".into()),
            source: DiagramSource::SqlDdl,
            dialect: Some("postgresql".into()),
            graph,
            tables: vec![
                ErTable {
                    name: "users".into(),
                    columns: vec![col("id")],
                },
                ErTable {
                    name: "orders".into(),
                    columns: vec![col("id"), col("user_id")],
                },
            ],
            warnings: vec!["skipped `CREATE TABLE broken …`".into()],
        }
    }

    #[test]
    fn summaries_name_source_and_counts() {
        assert_eq!(
            summary_text(&er()),
            "SQL DDL (postgresql) · 2 tables · 1 foreign keys"
        );
    }

    #[gpui::test]
    fn draws_graph_tables_and_warnings(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Diagram(Box::new(er())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(1000.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in ["diagram", "diagram-summary", "diagram-warnings", "tree"] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let diagram = cx.update(|window, cx| super::view(id, &er(), window, cx));
        assert_eq!(
            diagram
                .read_with(cx, |d, _| d.graph().title.clone())
                .as_deref(),
            Some("schema.sql")
        );
        assert_eq!(
            diagram.read_with(cx, |d, _| d.tables().map(|t| t.nodes.len())),
            Some(2)
        );
    }
}
