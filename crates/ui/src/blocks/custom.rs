//! Renderer of the `custom` block kind: the declarative tree a Lua plugin
//! returned, turned into components recursively.
//!
//! Tables, charts, Mermaid diagrams and markdown reuse the renderers of
//! their own block kinds; images ([`super::image_view`]) and JSON
//! ([`super::json`]) and 3D models ([`super::model_view`]) have views of
//! their own. Those keep GPUI state per
//! block id, so each such node gets an id derived from the block's (or the
//! modal's, see [`render_node`]) and its position in the tree. Links and
//! links inside markdown and HTML open the browser, and only for `http`,
//! `https` and `mailto` URLs. Nesting past [`MAX_DEPTH`] is cut with a
//! marker instead of recursing further.

use domain::block::{Block, BlockId, BlockKind, CustomNode, MermaidSpec, is_openable_url};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_component::text::TextView;
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use uuid::Uuid;

use crate::timeline::TimelineEntry;

/// Deepest node drawn; children below it show a marker.
pub const MAX_DEPTH: usize = 16;
/// Narrowest a child of a `Row` gets; children that do not fit side by
/// side at this width wrap to the next line.
pub const ROW_CHILD_MIN: f32 = 280.0;

/// Id of the `ordinal`-th stateful node (table, chart, markdown) of block
/// `parent`, in depth-first order. Stable across frames for the same tree.
pub fn node_id(parent: BlockId, ordinal: usize) -> BlockId {
    const MIX: u128 = 0x9e37_79b9_7f4a_7c15_f39c_c060_5ced_c834;
    let salt = (ordinal as u128 + 1).wrapping_mul(MIX);
    BlockId(Uuid::from_u128(parent.0.as_u128() ^ salt))
}

/// `source` as a fenced code block in `lang`, with a fence longer than any
/// backtick run inside it.
pub fn fenced_code(lang: Option<&str>, source: &str) -> String {
    let longest = source
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest.max(2) + 1);
    let newline = if source.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{}\n{source}{newline}{fence}\n", lang.unwrap_or(""))
}

/// Walks one tree, handing out ids to its stateful nodes.
struct Walk<'a> {
    entry: &'a TimelineEntry,
    next: usize,
}

impl Walk<'_> {
    fn next_id(&mut self) -> BlockId {
        let id = node_id(self.entry.block.id, self.next);
        self.next += 1;
        id
    }

    /// An entry of its own for a node drawn by another block's renderer.
    fn child_entry(&mut self, kind: BlockKind) -> TimelineEntry {
        let id = self.next_id();
        TimelineEntry {
            block: Block { id, kind },
            expanded: true,
            alt_view: self.entry.alt_view,
            numbers: Default::default(),
        }
    }

    fn children(
        &mut self,
        nodes: &[CustomNode],
        depth: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        nodes
            .iter()
            .map(|node| self.node(node, depth + 1, window, cx))
            .collect()
    }

    fn node(
        &mut self,
        node: &CustomNode,
        depth: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if depth > MAX_DEPTH {
            return div()
                .text_color(cx.theme().muted_foreground)
                .child("… (nested too deep)")
                .into_any_element();
        }
        match node {
            CustomNode::Text { text } => div()
                .child(SharedString::from(text.clone()))
                .into_any_element(),
            CustomNode::Row { children } => h_flex()
                .w_full()
                .flex_wrap()
                .gap_2()
                .items_start()
                .children(
                    self.children(children, depth, window, cx)
                        .into_iter()
                        .map(|child| {
                            div()
                                .flex_grow_1()
                                .flex_shrink_1()
                                .flex_basis(px(0.))
                                .min_w(px(ROW_CHILD_MIN))
                                .child(child)
                        }),
                )
                .into_any_element(),
            CustomNode::Col { children } => v_flex()
                .gap_1()
                .children(self.children(children, depth, window, cx))
                .into_any_element(),
            CustomNode::List { items } => {
                let items = self.children(items, depth, window, cx);
                let bullet = cx.theme().muted_foreground;
                v_flex()
                    .gap_0p5()
                    .children(items.into_iter().map(|item| {
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(div().text_color(bullet).child("•"))
                            .child(div().flex_1().min_w_0().child(item))
                    }))
                    .into_any_element()
            }
            CustomNode::Table(spec) => {
                let entry = self.child_entry(BlockKind::Table(spec.clone()));
                super::table::render(&entry, window, cx)
            }
            CustomNode::Chart(spec) => {
                let entry = self.child_entry(BlockKind::Chart(spec.clone()));
                super::chart::render(&entry, window, cx)
            }
            CustomNode::Code { lang, source } => TextView::markdown(
                element_id("custom-code", self.next_id()),
                SharedString::from(fenced_code(lang.as_deref(), source)),
            )
            .into_any_element(),
            CustomNode::Markdown { source } => TextView::markdown(
                element_id("custom-markdown", self.next_id()),
                SharedString::from(source.clone()),
            )
            .on_link_click(open_link)
            .into_any_element(),
            CustomNode::Html { source } => TextView::html(
                element_id("custom-html", self.next_id()),
                SharedString::from(source.clone()),
            )
            .on_link_click(open_link)
            .into_any_element(),
            CustomNode::Image {
                source,
                media_type,
                alt,
                fit,
            } => super::image_view::image_view(
                self.next_id(),
                source,
                media_type.as_deref(),
                alt.as_deref(),
                *fit,
                window,
                cx,
            )
            .into_any_element(),
            CustomNode::Json {
                value,
                expand_depth,
            } => super::json::json_view(self.next_id(), value, *expand_depth, window, cx)
                .into_any_element(),
            CustomNode::Mermaid { source } => {
                let entry = self.child_entry(BlockKind::Mermaid(MermaidSpec {
                    source: source.clone(),
                    title: None,
                }));
                super::mermaid::render(&entry, window, cx)
            }
            CustomNode::Diff { unified } => super::diff::render_unified(unified, cx),
            CustomNode::Model3d(spec) => {
                super::model_view::model_view(self.next_id(), spec, window, cx).into_any_element()
            }
            CustomNode::Link { text, url } => {
                let id = element_id("custom-link", self.next_id());
                let url = url.clone();
                div()
                    .id(id)
                    .debug_selector(|| "custom-link".to_owned())
                    .text_color(cx.theme().link)
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .child(SharedString::from(text.clone()))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        if is_openable_url(&url) {
                            cx.open_url(&url);
                        }
                    })
                    .into_any_element()
            }
        }
    }
}

/// Opens a link of markdown or HTML in the browser, when it is `http`,
/// `https` or `mailto`.
fn open_link(url: &SharedString, _: &ClickEvent, _: &mut Window, cx: &mut App) {
    if is_openable_url(url) {
        cx.open_url(url);
    }
}

fn element_id(prefix: &str, id: BlockId) -> ElementId {
    ElementId::Name(SharedString::from(format!("{prefix}-{id}")))
}

/// Draws `tree` as the content of `entry`.
pub fn render_tree(
    entry: &TimelineEntry,
    tree: &CustomNode,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    Walk { entry, next: 0 }.node(tree, 1, window, cx)
}

/// Draws `tree` outside the timeline (a modal): its stateful nodes get ids
/// derived from `parent`, so they keep their state across frames.
pub fn render_node(
    parent: BlockId,
    tree: &CustomNode,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let entry = TimelineEntry {
        block: Block {
            id: parent,
            kind: BlockKind::Text {
                markdown: String::new(),
            },
        },
        expanded: true,
        alt_view: false,
        numbers: Default::default(),
    };
    render_tree(&entry, tree, window, cx)
}

/// Draws the block held by `entry`: the plugin and block name, then the tree.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Custom { plugin, name, tree } = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let body = render_tree(entry, tree, window, cx);
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!("{plugin} · {name}"))),
        )
        .child(body)
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests;
