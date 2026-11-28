//! The formatting toolbar above the source editor: Bold, Italic, Heading,
//! List, Quote, Code, Table, Link, Image and New diagram. Each button has a
//! tooltip with its name and acts on the editor's selection through
//! [`domain::doc::format_selection`] (the panel runs it, see
//! `DocPanel::run_tool`). Link first asks for an address in a small prompt
//! and does nothing when it is left empty.

use domain::doc::Format;
use gpui::{
    AnyElement, App, Context, Div, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Stateful, StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::input::Input;
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit_assets::IconName;

use super::DocPanel;
use crate::kit::quiet_button;
use crate::theme::token;

/// One button of the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// `**selection**`.
    Bold,
    /// `*selection*`.
    Italic,
    /// `## ` on the lines.
    Heading,
    /// `- ` on the lines.
    List,
    /// `> ` on the lines.
    Quote,
    /// `` `selection` `` or a fenced block.
    Code,
    /// A table template on its own block.
    Table,
    /// `[selection](address)`.
    Link,
    /// `![alt](assets/name)` from a picture file.
    Image,
    /// Opens the diagram editor for a new diagram.
    Diagram,
}

impl Tool {
    /// Every button, in order.
    pub const ALL: [Tool; 10] = [
        Tool::Bold,
        Tool::Italic,
        Tool::Heading,
        Tool::List,
        Tool::Quote,
        Tool::Code,
        Tool::Table,
        Tool::Link,
        Tool::Image,
        Tool::Diagram,
    ];

    /// The button's debug selector.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Tool::Bold => "doc-tool-bold",
            Tool::Italic => "doc-tool-italic",
            Tool::Heading => "doc-tool-heading",
            Tool::List => "doc-tool-list",
            Tool::Quote => "doc-tool-quote",
            Tool::Code => "doc-tool-code",
            Tool::Table => "doc-tool-table",
            Tool::Link => "doc-tool-link",
            Tool::Image => "doc-tool-image",
            Tool::Diagram => "doc-tool-diagram",
        }
    }

    /// The button's glyph.
    #[must_use]
    pub const fn icon(self) -> IconName {
        match self {
            Tool::Bold => IconName::Bold,
            Tool::Italic => IconName::Italic,
            Tool::Heading => IconName::Heading,
            Tool::List => IconName::List,
            Tool::Quote => IconName::Quote,
            Tool::Code => IconName::Code,
            Tool::Table => IconName::Table,
            Tool::Link => IconName::Link,
            Tool::Image => IconName::Image,
            Tool::Diagram => IconName::Workflow,
        }
    }

    /// The group the button sits in, groups being set apart by a rule.
    #[must_use]
    pub const fn group(self) -> u8 {
        match self {
            Tool::Bold | Tool::Italic | Tool::Heading => 0,
            Tool::List | Tool::Quote | Tool::Code | Tool::Table => 1,
            Tool::Link | Tool::Image | Tool::Diagram => 2,
        }
    }

    /// The tooltip.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Tool::Bold => "Bold",
            Tool::Italic => "Italic",
            Tool::Heading => "Heading",
            Tool::List => "List",
            Tool::Quote => "Quote",
            Tool::Code => "Code",
            Tool::Table => "Insert table",
            Tool::Link => "Link",
            Tool::Image => "Insert image",
            Tool::Diagram => "New diagram",
        }
    }

    /// The builtin action that presses the button from the keyboard.
    #[must_use]
    pub const fn action_name(self) -> &'static str {
        match self {
            Tool::Bold => "doc.bold",
            Tool::Italic => "doc.italic",
            Tool::Heading => "doc.heading",
            Tool::List => "doc.list",
            Tool::Quote => "doc.quote",
            Tool::Code => "doc.code",
            Tool::Table => "doc.table",
            Tool::Link => "doc.link",
            Tool::Image => "doc.image",
            Tool::Diagram => "doc.diagram",
        }
    }

    /// The button an action name presses, if it is one of the toolbar's.
    #[must_use]
    pub fn from_action_name(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.action_name() == name)
    }

    /// The formatting it applies, for the buttons that format text.
    #[must_use]
    pub const fn format(self) -> Option<Format> {
        match self {
            Tool::Bold => Some(Format::Bold),
            Tool::Italic => Some(Format::Italic),
            Tool::Heading => Some(Format::Heading),
            Tool::List => Some(Format::List),
            Tool::Quote => Some(Format::Quote),
            Tool::Code => Some(Format::Code),
            Tool::Table => Some(Format::Table),
            Tool::Link => Some(Format::Link),
            Tool::Image => Some(Format::Image),
            Tool::Diagram => None,
        }
    }
}

/// A 24 px icon button of the panel's heads and toolbars: the glyph in the
/// muted color, the foreground under the pointer, the accent when `on`.
/// `name` is its tooltip.
pub(super) fn icon_tool(
    id: &'static str,
    icon: Icon,
    name: &'static str,
    on: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (muted, fg, selected) = (theme.muted_foreground, theme.foreground, theme.list_active);
    let accent = token(cx, "accent.bg");
    let button = div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(24.))
        .h(px(24.))
        .rounded_sm()
        .cursor_pointer()
        .tooltip(move |window, cx| Tooltip::new(name).build(window, cx))
        .child(icon.small());
    if on {
        button.text_color(accent).bg(selected)
    } else {
        button
            .text_color(muted)
            .hover(move |style| style.text_color(fg).bg(selected))
    }
}

/// The thin vertical rule between groups of buttons.
pub(super) fn rule(cx: &App) -> Div {
    div().w(px(1.)).h(px(16.)).mx_1().bg(cx.theme().border)
}

/// The toolbar, and under it the address prompt while a link is asked for.
pub fn render(panel: &DocPanel, cx: &mut Context<DocPanel>) -> AnyElement {
    let (border, bar_bg) = (cx.theme().border, cx.theme().muted);
    let mut bar = h_flex()
        .id("doc-toolbar")
        .debug_selector(|| "doc-toolbar".to_owned())
        .gap_0p5()
        .items_center()
        .px_2()
        .py_1()
        .bg(bar_bg)
        .border_b_1()
        .border_color(border);
    let mut group = None;
    for tool in Tool::ALL {
        if group.is_some_and(|g| g != tool.group()) {
            bar = bar.child(rule(cx));
        }
        group = Some(tool.group());
        bar = bar.child(
            icon_tool(tool.id(), Icon::new(tool.icon()), tool.name(), false, cx)
                // Keep the editor's selection and focus while the button is pressed.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| this.run_tool(tool, window, cx))),
        );
    }
    let mut column = gpui_component::v_flex().child(bar);
    if let Some(prompt) = &panel.link {
        column = column.child(
            h_flex()
                .gap_2()
                .items_center()
                .px_2()
                .pb_1()
                .child(
                    gpui::div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Link address"),
                )
                .child(gpui::div().flex_1().child(Input::new(&prompt.input)))
                .child(
                    quiet_button("doc-link-ok", "Add", cx)
                        .on_click(cx.listener(|this, _, window, cx| this.confirm_link(window, cx))),
                ),
        );
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_has_a_name_and_a_distinct_id() {
        let mut ids: Vec<_> = Tool::ALL.iter().map(|t| t.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Tool::ALL.len());
        assert!(Tool::ALL.iter().all(|t| !t.name().is_empty()));
        let mut names: Vec<_> = Tool::ALL.iter().map(|t| t.action_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Tool::ALL.len());
        for tool in Tool::ALL {
            assert_eq!(Tool::from_action_name(tool.action_name()), Some(tool));
        }
        assert_eq!(Tool::from_action_name("doc.new"), None);
        assert_eq!(Tool::Diagram.format(), None);
        assert_eq!(Tool::Bold.format(), Some(Format::Bold));
    }
}
