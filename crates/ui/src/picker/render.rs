//! Drawing the picker: its rows, option chips, preview and footer.

use super::preview::{RichPreview, overlay, valid_range};
use super::search::UNSUPPORTED_HINT;
use domain::block::{ImageData, ImageFit};
use domain::workspace::WorkspacePath;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, Window, div,
};
use gpui::{px, uniform_list};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::h_flex;
use gpui_component::input::Input;
use gpui_component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_component::v_flex;
use gpui_component::{ActiveTheme as _, IconName, Selectable as _, Sizable as _};

use super::{
    CHAR_WIDTH, GrepToggle, MODEL_PREVIEW_HEIGHT, POPUP_HEIGHT, POPUP_WIDTH, PREVIEW_IMAGE,
    PREVIEW_MODEL, PickerItem, PickerMode, PickerView, RESULTS_SHARE, TWO_COLUMNS_MIN_WIDTH,
    row_text, truncate_left,
};

/// The keys the footer lists; `Alt+P` only when the window is `wide`
/// enough for the preview.
pub fn footer_keys(mode: PickerMode, wide: bool) -> String {
    let mut keys = vec!["↑↓ navigate", "Enter insert", "Esc close"];
    if wide {
        keys.push("Alt+P preview");
    }
    if mode == PickerMode::Grep {
        keys.extend(["Alt+C case", "Alt+R regex", "Alt+W word"]);
    }
    keys.join(" · ")
}

impl PickerView {
    fn render_item(
        &self,
        ix: usize,
        item: &PickerItem,
        path_chars: usize,
        stale: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let location = match item.line {
            Some(line) => format!("{}:{line}", item.path),
            None => item.path.to_string(),
        };
        // A match line shares the row with its location.
        let budget = if item.text.is_some() {
            path_chars * 2 / 5
        } else {
            path_chars
        };
        let mark = HighlightStyle {
            background_color: Some(theme.warning.opacity(0.35)),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let mut row = h_flex()
            .id(ix)
            .gap_2()
            .px_2()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .child(SharedString::from(truncate_left(&location, budget.max(12)))),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = ix;
                this.confirm(window, cx);
            }));
        if item.recent {
            row = row.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("recent"),
            );
        }
        if let Some(text) = &item.text {
            let (shown, range) = row_text(text, item.column_range);
            row = row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_color(theme.muted_foreground)
                    .text_ellipsis()
                    .child(
                        StyledText::new(shown).with_highlights(range.map(|range| (range, mark))),
                    ),
            );
        }
        if ix == self.selected {
            row = row.bg(theme.list_active);
        }
        if stale {
            row = row.opacity(0.5);
        }
        row
    }

    fn render_chips(&self, cx: &mut Context<Self>) -> AnyElement {
        let options = self.options;
        h_flex()
            .gap_1()
            .children(GrepToggle::ALL.map(|toggle| {
                let on = options.get(toggle);
                div().debug_selector(|| toggle.selector().to_owned()).child(
                    Button::new(toggle.selector())
                        .ghost()
                        .xsmall()
                        .label(toggle.label())
                        .selected(on)
                        .toggled(on)
                        .tab_stop(false)
                        .tooltip(format!("{} ({})", toggle.name(), toggle.keys()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle(toggle, window, cx);
                            this.input.update(cx, |input, cx| input.focus(window, cx));
                        })),
                )
            }))
            .into_any_element()
    }

    fn render_preview(
        &self,
        path_chars: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let pane = v_flex()
            .id("picker-preview")
            .debug_selector(|| "picker-preview".to_owned())
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .border_l_1()
            .border_color(theme.border)
            .pl_2()
            .gap_1();
        let Some(preview) = &self.preview else {
            return pane
                .items_center()
                .justify_center()
                .text_color(muted)
                .child("Nothing to preview")
                .into_any_element();
        };
        let mut title = preview.path.to_string();
        if let Some(line) = preview.line {
            title.push_str(&format!(":{line}"));
        }
        let note = match (&preview.error, preview.loading) {
            (Some(error), _) => Some(error.clone()),
            (None, true) => Some("reading…".to_owned()),
            (None, false) => None,
        };
        let header = h_flex()
            .gap_2()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(SharedString::from(truncate_left(
                &title,
                path_chars.max(12),
            )))
            .children(note.map(|note| {
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(note))
            }));
        if let Some(rich) = &preview.rich {
            let body = self.render_rich(rich, &preview.path, window, cx);
            return pane.child(header).child(body).into_any_element();
        }
        let mark = HighlightStyle {
            background_color: Some(theme.warning.opacity(0.35)),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let match_ix = preview.match_index();
        let last = preview.start_line + preview.lines.len();
        let gutter = px(8. * last.to_string().len() as f32 + 4.);
        let rows = preview.lines.iter().enumerate().map(|(ix, line)| {
            let styles = self.preview_styles.get(ix).cloned().unwrap_or_default();
            let is_match = match_ix == Some(ix);
            let styles = match preview
                .column_range
                .and_then(|range| valid_range(line, range))
            {
                Some(range) if is_match => overlay(&styles, range, mark),
                _ => styles,
            };
            // An empty line still takes a row.
            let text = if line.is_empty() {
                StyledText::new(" ")
            } else {
                StyledText::new(line.clone()).with_highlights(styles)
            };
            h_flex()
                .gap_2()
                .whitespace_nowrap()
                .when(is_match, |row| row.bg(theme.list_active))
                .child(
                    div()
                        .flex_none()
                        .w(gutter)
                        .text_right()
                        .text_color(muted)
                        .child(SharedString::from((preview.start_line + ix).to_string())),
                )
                .child(div().child(text))
        });
        pane.child(header)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .font_family(theme.mono_font_family.clone())
                    .text_sm()
                    .children(rows),
            )
            .into_any_element()
    }
}

impl PickerView {
    /// A picture or a model as a still, or the card of a file the preview
    /// cannot draw. The still sits under a transparent layer: a click on it
    /// must not take the focus from the input (the keys would stop
    /// working) nor start a drag; `Enter` and `/open` give the real viewer.
    fn render_rich(
        &self,
        rich: &RichPreview,
        path: &WorkspacePath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let still = |view: AnyElement| {
            div()
                .id("picker-preview-still")
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .child(view)
                .child(
                    div()
                        .id("picker-preview-shield")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                )
                .into_any_element()
        };
        match rich {
            RichPreview::Image { path: absolute } => {
                let data = ImageData::Path {
                    path: absolute.clone(),
                };
                let view = crate::blocks::image_view::image_view(
                    PREVIEW_IMAGE,
                    &data,
                    None,
                    path.file_name(),
                    ImageFit::Contain,
                    crate::timeline::NO_REV,
                    window,
                    cx,
                );
                still(view.into_any_element())
            }
            RichPreview::Model(spec) => {
                let view = crate::blocks::model_view::model_view_inline(
                    PREVIEW_MODEL,
                    spec,
                    crate::timeline::NO_REV,
                    MODEL_PREVIEW_HEIGHT,
                    window,
                    cx,
                );
                still(view.into_any_element())
            }
            RichPreview::Unsupported { reason } => v_flex()
                .id("picker-preview-unsupported")
                .debug_selector(|| "picker-preview-unsupported".to_owned())
                .flex_1()
                .min_h_0()
                .w_full()
                .items_center()
                .justify_center()
                .gap_1()
                .text_color(muted)
                .child(div().text_color(theme.foreground).child("No preview"))
                .child(SharedString::from(reason.clone()))
                .child(div().text_xs().child(UNSUPPORTED_HINT))
                .into_any_element(),
        }
    }
}

impl Render for PickerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(mode) = self.mode else {
            return div().into_any_element();
        };
        let viewport = window.viewport_size();
        let width = POPUP_WIDTH.min(f32::from(viewport.width) - 32.).max(240.);
        let height = POPUP_HEIGHT
            .min(f32::from(viewport.height) - 120.)
            .max(200.);
        let wide = width >= TWO_COLUMNS_MIN_WIDTH;
        let two_columns = self.preview_on && wide;
        let results_width = if two_columns {
            width * RESULTS_SHARE
        } else {
            width
        };
        let path_chars = (results_width / CHAR_WIDTH) as usize;
        let preview_chars = ((width - results_width) / CHAR_WIDTH) as usize;
        let status = self.status_text(cx);
        let stale = self.search.is_some();
        let theme = cx.theme();
        let (bg, border, muted) = (theme.popover, theme.border, theme.muted_foreground);
        // The rows are one line each: only those in view are built.
        let rows = uniform_list(
            "picker-rows",
            self.items.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|ix| {
                        let item = this.items.get(ix)?;
                        Some(
                            this.render_item(ix, item, path_chars, stale, cx)
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        let results = div()
            .id("picker-results")
            .relative()
            .child(rows)
            .scrollbar(&self.scroll, ScrollbarAxis::Vertical)
            .when(two_columns, |div| div.w(px(results_width)).flex_none())
            .when(!two_columns, |div| div.flex_1())
            .min_w_0()
            .h_full();
        let preview = two_columns.then(|| self.render_preview(preview_chars, window, cx));
        let chips = (mode == PickerMode::Grep).then(|| self.render_chips(cx));
        // The backdrop: a click outside the popup closes it.
        div()
            .id("picker-backdrop")
            .debug_selector(|| "picker-backdrop".to_owned())
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(80.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| this.dismiss(window, cx)),
            )
            .child(
                v_flex()
                    .id("picker")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .w(px(width))
                    .h(px(height))
                    .gap_1()
                    .p_2()
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_color(muted).child(mode.title()))
                            .child(div().flex_1().child(Input::new(&self.input)))
                            .child(div().text_xs().text_color(muted).child("Esc"))
                            .child(
                                div().debug_selector(|| "picker-close".to_owned()).child(
                                    Button::new("picker-close")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("Close (Esc)")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.dismiss(window, cx);
                                        })),
                                ),
                            ),
                    )
                    .children(chips)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .items_start()
                            .gap_2()
                            .child(results)
                            .children(preview),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(muted)
                            .child(
                                div()
                                    .debug_selector(|| "picker-status".to_owned())
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(status)),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "picker-keys".to_owned())
                                    .flex_none()
                                    .child(SharedString::from(footer_keys(mode, wide))),
                            ),
                    ),
            )
            .into_any_element()
    }
}
