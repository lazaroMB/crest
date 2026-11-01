//! Drawing the approval dialog: the preview, the rule chips and rows, the hints
//! and the footer.

use application::ports::approval::ApprovalPreview;
use application::ports::permissions::PERMISSIONS_FILE;
use gpui::prelude::FluentBuilder as _;
use gpui::px;
use gpui::{
    AnyElement, App, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::{
    ApprovalView, Choice, EXACT_ONLY_REASON, all_edits_hint, command_line, pattern_hint,
    pattern_text,
};

impl Render for ApprovalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shown) = &self.current else {
            return div().id("approval").into_any_element();
        };
        let request = shown.pending.request();
        let title = format!("Allow {}?", request.tool);
        let subject = request.subject.clone();
        let preview = Self::render_preview(&request.preview, cx);
        let rows: Vec<_> = self
            .choices()
            .into_iter()
            .enumerate()
            .map(|(ix, choice)| self.render_row(ix, choice, cx))
            .collect();
        let hints = self.render_hints(cx);
        let footer = self.render_footer(cx);
        let theme = cx.theme();
        let mut card = v_flex()
            .w(px(760.))
            .max_w_full()
            .gap_2()
            .p_4()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(SharedString::from(subject)),
                    ),
            )
            .child(
                crate::keyscroll::list("approval-preview", &self.preview_scroll, [preview])
                    .max_h(px(380.)),
            );
        if !self.queue.is_empty() {
            card = card.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!(
                        "{} more pending",
                        self.queue.len()
                    ))),
            );
        }
        let options = v_flex()
            .pt_2()
            .border_t_1()
            .border_color(theme.border)
            .children(rows)
            .children(hints);
        div()
            .id("approval")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().overlay)
            .child(card.child(options).child(footer))
            .into_any_element()
    }
}

impl ApprovalView {
    fn render_preview(preview: &ApprovalPreview, cx: &App) -> AnyElement {
        let theme = cx.theme();
        match preview {
            ApprovalPreview::Diff(spec) => v_flex()
                .gap_1()
                .child(crate::blocks::diff::render_header(spec, cx))
                .child(crate::blocks::diff::render_unified(&spec.unified, cx))
                .into_any_element(),
            ApprovalPreview::Command { command, args, cwd } => v_flex()
                .gap_1()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!("in {cwd}"))),
                )
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .p_2()
                        .bg(theme.secondary)
                        .child(SharedString::from(format!(
                            "$ {}",
                            command_line(command, args)
                        ))),
                )
                .into_any_element(),
            ApprovalPreview::Text { text } => div()
                .child(SharedString::from(text.clone()))
                .into_any_element(),
        }
    }

    /// The `‹ pattern ›` chip of the session (`project` false) and project
    /// rows. The arrows narrow and widen it; they are left out when there
    /// is only one rung.
    fn render_chip(&self, project: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(rung) = self.pattern() else {
            return div().into_any_element();
        };
        let suffix = if project { "-project" } else { "" };
        let theme = cx.theme();
        let adjustable = self.ladder.len() > 1;
        let arrow = |text: &'static str, name: &str, enabled: bool, cx: &mut Context<Self>| {
            let wider = name == "wider";
            let id = format!("approval-pattern-{name}{suffix}");
            let selector = id.clone();
            div()
                .id(SharedString::from(id))
                .debug_selector(move || selector)
                .px_1()
                .cursor_pointer()
                .text_color(if enabled {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .child(text)
                .on_click(cx.listener(move |this, _, _, cx| {
                    // The row under the arrow must not answer.
                    cx.stop_propagation();
                    if wider {
                        this.widen(cx);
                    } else {
                        this.narrow(cx);
                    }
                }))
        };
        let text = div()
            .font_family(theme.mono_font_family.clone())
            .px_1()
            .bg(theme.secondary)
            .rounded_sm()
            .overflow_hidden()
            .text_ellipsis()
            .max_w(px(380.))
            .child(SharedString::from(pattern_text(rung)));
        let selector = format!("approval-pattern{suffix}");
        let (narrower, wider) = (self.rung > 0, self.rung + 1 < self.ladder.len());
        h_flex()
            .debug_selector(move || selector)
            .min_w_0()
            .when(adjustable, |chip| {
                chip.child(arrow("‹", "narrower", narrower, cx))
            })
            .child(text)
            .when(adjustable, |chip| {
                chip.child(arrow("›", "wider", wider, cx))
            })
            .into_any_element()
    }

    fn render_row(&self, ix: usize, choice: Choice, cx: &mut Context<Self>) -> AnyElement {
        let selected = ix == self.cursor;
        let extra = match choice {
            Choice::Session => Some(self.render_chip(false, cx)),
            Choice::Project => Some(self.render_chip(true, cx)),
            Choice::AllEdits => Some(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .debug_selector(|| "approval-all-edits-hint".to_owned())
                    .child(SharedString::from(all_edits_hint(
                        self.mode_keys.as_deref(),
                    )))
                    .into_any_element(),
            ),
            Choice::Once | Choice::Reject => None,
        };
        let theme = cx.theme();
        let id = format!("approval-row-{}", choice.key());
        let selector = id.clone();
        h_flex()
            .id(SharedString::from(id))
            .debug_selector(move || selector)
            .gap_2()
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, |row| row.bg(theme.list_active))
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .child(if selected { "›" } else { "" }),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(theme.mono_font_family.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_color(if choice == Choice::Reject {
                        theme.danger
                    } else {
                        theme.primary
                    })
                    .child(SharedString::from(choice.key().to_string())),
            )
            .child(div().flex_none().child(choice.label()))
            .children(extra)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.cursor = ix;
                this.choose(choice, window, cx);
            }))
            .into_any_element()
    }

    /// Lines under the rows: why the pattern is exact, or what a broader
    /// one allows; where a project rule is saved while that row is under
    /// the cursor.
    fn render_hints(&self, cx: &App) -> Option<AnyElement> {
        let mut lines: Vec<(&'static str, String)> = Vec::new();
        if self.exact_only {
            lines.push(("approval-pattern-reason", EXACT_ONLY_REASON.to_owned()));
        } else if let Some(hint) = self.pattern().and_then(pattern_hint) {
            lines.push(("approval-pattern-hint", hint));
        }
        if self.selected() == Some(Choice::Project) {
            lines.push((
                "approval-project-file",
                format!("saved in .crest/{PERMISSIONS_FILE}"),
            ));
        }
        if lines.is_empty() {
            return None;
        }
        let color = cx.theme().muted_foreground;
        Some(
            v_flex()
                .pl(px(20.))
                .text_xs()
                .text_color(color)
                .children(lines.into_iter().map(|(selector, line)| {
                    div()
                        .debug_selector(move || selector.to_owned())
                        .child(SharedString::from(line))
                }))
                .into_any_element(),
        )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        if let Some(reason) = &self.reason {
            return h_flex()
                .gap_2()
                .child(div().text_color(theme.muted_foreground).child("Reason:"))
                .child(
                    div()
                        .flex_1()
                        .px_1()
                        .border_1()
                        .border_color(theme.border)
                        .child(SharedString::from(format!("{reason}▏"))),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("Enter reject · Esc back"),
                )
                .into_any_element();
        }
        div()
            .debug_selector(|| "approval-keys".to_owned())
            .text_xs()
            .text_color(theme.muted_foreground)
            .child("↑↓ choose · Tab/⇧Tab pattern · Enter confirm · Esc reject")
            .into_any_element()
    }
}
