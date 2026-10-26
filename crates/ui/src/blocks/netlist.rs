//! Renderer of the `netlist` block kind (`show_netlist`): a SPICE deck
//! or KiCad netlist as connectivity.
//!
//! A summary line comes first (format, design, component, net and pin
//! counts), then the nets with a single pin in the warning color. Then
//! the component–net graph ([`crate::widgets::graph`]): components and
//! nets as boxes, a pin as an edge labelled with the pin, ground and
//! open nets in their own colors; a click focuses a component or net and
//! its neighbours. The component table ([`crate::widgets::tree`]) lists
//! each component's kind, value and model or footprint, with its pins and
//! their nets under it. Reader notes (skipped lines, includes not
//! followed) are listed last.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::engineering::netlist::NetlistSpec;
use domain::widget::graph::GraphSpec;
use domain::widget::tree::TreeSpec;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::custom::node_id;
use super::trace::file_name;
use crate::timeline::TimelineEntry;
use crate::widgets;

/// Notes and open nets listed under the summary.
pub const LISTED: usize = 6;

/// The summary text.
#[must_use]
pub fn summary_text(spec: &NetlistSpec) -> String {
    let mut text = spec.format.label().to_owned();
    if let Some(design) = &spec.design {
        text.push_str(&format!(" · {design}"));
    }
    text.push_str(&format!(
        " · {} components · {} nets · {} pins",
        spec.components.len(),
        spec.nets().len(),
        spec.pin_count()
    ));
    if !spec.subcircuits.is_empty() {
        text.push_str(&format!(" · {} subcircuits", spec.subcircuits.len()));
    }
    text
}

/// The single-pin nets line, if any.
#[must_use]
pub fn open_text(spec: &NetlistSpec) -> Option<String> {
    let single = spec.single_pin_nets();
    if single.is_empty() {
        return None;
    }
    let names: Vec<String> = single
        .iter()
        .take(LISTED)
        .map(|n| {
            let pins: Vec<String> = n
                .pins
                .iter()
                .map(|(ix, pin)| format!("{}.{pin}", spec.components[*ix].reference))
                .collect();
            format!("{} ({})", n.name, pins.join(", "))
        })
        .collect();
    let more = single.len().saturating_sub(LISTED);
    Some(format!(
        "{} single-pin nets: {}{}",
        single.len(),
        names.join(", "),
        if more > 0 {
            format!(" … {more} more")
        } else {
            String::new()
        }
    ))
}

/// Per-block state: the spec, its graph (titled) and its table.
pub struct NetlistView {
    id: BlockId,
    spec: Rc<NetlistSpec>,
    graph: Rc<GraphSpec>,
    table: Rc<TreeSpec>,
}

impl NetlistView {
    fn new(id: BlockId, spec: &NetlistSpec) -> Self {
        let mut graph = spec.to_graph();
        graph.title = spec
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| spec.path.as_deref().map(|p| file_name(p).to_owned()))
            .or_else(|| spec.design.clone())
            .or_else(|| Some("Netlist".to_owned()));
        Self {
            id,
            spec: Rc::new(spec.clone()),
            graph: Rc::new(graph),
            table: Rc::new(spec.to_tree()),
        }
    }

    fn sync(&mut self, spec: &NetlistSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The graph drawn.
    pub fn graph(&self) -> &GraphSpec {
        &self.graph
    }

    /// The component table.
    pub fn table(&self) -> &TreeSpec {
        &self.table
    }
}

impl Render for NetlistView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let graph = self.graph.clone();
        let widget = widgets::graph::render(node_id(self.id, 0), &graph, window, cx);
        let table = self.table.clone();
        let table = widgets::tree::render(node_id(self.id, 1), &table, window, cx);
        let theme = cx.theme();
        let warnings = &self.spec.warnings;
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "netlist".to_owned())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .debug_selector(|| "netlist-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .when_some(open_text(&self.spec), |d, text| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.warning)
                        .debug_selector(|| "netlist-open".to_owned())
                        .child(text),
                )
            })
            .child(widget)
            .child(table)
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .debug_selector(|| "netlist-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone())))
                        .when(warnings.len() > LISTED, |d| {
                            d.child(format!("… {} more", warnings.len() - LISTED))
                        }),
                )
            })
    }
}

/// The netlist entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &NetlistSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<NetlistView> {
    let view = super::block_state(id, window, cx, |_, _| NetlistView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Netlist(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::engineering::netlist::{Component, NetlistFormat, PinNet};
    use gpui::{TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> NetlistSpec {
        let part = |reference: &str, nets: [&str; 2]| Component {
            reference: reference.into(),
            kind: "resistor".into(),
            value: Some("1k".into()),
            model: None,
            pins: nets
                .iter()
                .enumerate()
                .map(|(i, n)| PinNet {
                    pin: (i + 1).to_string(),
                    net: (*n).into(),
                    function: None,
                })
                .collect(),
        };
        NetlistSpec {
            title: None,
            path: Some("hw/div.cir".into()),
            format: NetlistFormat::Spice,
            design: Some("divider".into()),
            components: vec![part("R1", ["in", "out"]), part("R2", ["out", "0"])],
            subcircuits: Vec::new(),
            warnings: vec!["line 3: `.include x` not followed".into()],
        }
    }

    #[test]
    fn summary_and_open_nets() {
        assert_eq!(
            summary_text(&spec()),
            "SPICE · divider · 2 components · 3 nets · 4 pins"
        );
        assert_eq!(
            open_text(&spec()).as_deref(),
            Some("2 single-pin nets: 0 (R2.2), in (R1.1)")
        );
    }

    #[gpui::test]
    fn draws_graph_table_and_notes(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Netlist(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(1200.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "netlist",
            "netlist-summary",
            "netlist-open",
            "netlist-warnings",
            "tree",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let netlist = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert_eq!(
            netlist
                .read_with(cx, |n, _| n.graph().title.clone())
                .as_deref(),
            Some("div.cir")
        );
        assert_eq!(netlist.read_with(cx, |n, _| n.table().nodes.len()), 2);
    }
}
