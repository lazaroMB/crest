//! The left pane of the manager: the filter, the servers by group and the
//! **Add server** row.

use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px, relative,
};
use gpui_component::input::Input;
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::model::{ServerRow, row_meta};
use super::view::{McpView, dot, key_hint};
use crate::kit::{caption, quiet_button};

/// Share of the modal's width the list pane takes, between
/// [`LIST_MIN_PX`] and [`LIST_MAX_PX`], so a narrow modal leaves the detail
/// room.
const LIST_SHARE: f32 = 0.32;
const LIST_MIN_PX: f32 = 160.0;
const LIST_MAX_PX: f32 = 240.0;

impl McpView {
    /// The list pane.
    pub(super) fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| self.render_row(ix, *row, cx).into_any_element())
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .debug_selector(|| "mcp-empty".to_owned())
                .p_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(if self.listing.servers.is_empty() {
                    "No MCP servers yet"
                } else {
                    "No server matches"
                })
        });
        let add = self.listing.can_edit.then(|| {
            h_flex()
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    quiet_button("mcp-add", "+ Add server", cx).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.start_form(None, window, cx);
                        },
                    )),
                )
                .child(key_hint("a", cx))
        });
        v_flex()
            .debug_selector(|| "mcp-list".to_owned())
            .w(relative(LIST_SHARE))
            .min_w(px(LIST_MIN_PX))
            .max_w(px(LIST_MAX_PX))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .gap_1()
                    .px_2()
                    .py_2()
                    .child(caption("changes apply live — no restart", cx))
                    .child(Input::new(&self.filter)),
            )
            .child(
                crate::keyscroll::list("mcp-rows", &self.list_scroll, rows)
                    .flex_1()
                    .w_full()
                    .p_1(),
            )
            .children(empty)
            .children(add)
    }

    fn render_row(&self, ix: usize, row: ServerRow, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let server = &self.listing.servers[row.index];
        let first_of_group = ix == 0 || self.rows[ix - 1].group != row.group;
        let group = first_of_group.then(|| {
            div()
                .pt_2()
                .pb_0p5()
                .px_2()
                .text_xs()
                .text_color(theme.primary)
                .child(row.group.title())
        });
        let mut line = h_flex()
            .id(("mcp-row", ix))
            .debug_selector(move || format!("mcp-row-{ix}"))
            .gap_2()
            .px_2()
            .py_0p5()
            .rounded_sm()
            .cursor_pointer()
            .child(dot(&server.state, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(SharedString::from(server.spec.name.clone())),
            )
            .child(caption(row_meta(&server.state), cx).flex_none())
            .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)));
        if ix == self.selected {
            line = line.bg(theme.list_active);
        }
        if !server.enabled() {
            line = line.opacity(0.6);
        }
        v_flex().children(group).child(line)
    }
}
