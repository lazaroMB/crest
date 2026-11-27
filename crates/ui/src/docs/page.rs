//! The page: the doc as a WYSIWYG markdown editor.
//!
//! The page is a [`zorite_editor::EditorState`] owned by the panel
//! (`DocPanel::page_editor`). It keeps the markdown as the text and draws it
//! styled: the marks are hidden except on the caret's line, headings have
//! their real sizes, tables stay a grid with cell editing, a `mermaid` fence
//! is drawn as the diagram through [`Shared::diagram`] (the raw code shows
//! while the caret is inside) and an `![alt](assets/name)` line as the
//! picture of the doc's asset through [`Shared::picture`].
//!
//! The editor does not scroll, it grows with its text: the page puts it in a
//! scroll container, and the editor's scroll compensator keeps the visible
//! text still while a diagram or a picture lands above it.
//!
//! The editor has no accessor for its selection. The panel reads it back
//! through the clipboard writer ([`Probe`]): dispatching the editor's `Copy`
//! action with the probe armed captures the selected text instead of
//! writing it to the clipboard.

use gpui::prelude::FluentBuilder as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use domain::doc::diagram_spans;
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, Focusable as _, ImageFormat,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement as _, Pixels, RenderImage, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, font, px,
};
use gpui_component::ActiveTheme as _;
use image::{Frame, RgbaImage};
use zorite_editor::{EditorState, SyntaxStyle};

use super::DocPanel;
use crate::app::ColorTheme;
use crate::kit::quiet_button;
use crate::theme::{crest_theme, hsla};

/// Widest the page's column gets.
pub(super) const PAGE_WIDTH: f32 = 780.0;
/// Widest a picture is decoded: wider ones are scaled down once.
const PICTURE_MAX_WIDTH: u32 = 2000;
/// The diagram's width follows the page in steps of this many pixels, so a
/// window being resized does not draw every diagram again at each frame.
const WIDTH_STEP: f32 = 20.0;

/// The image format of an asset name, from its extension.
#[must_use]
pub fn image_format(name: &str) -> Option<ImageFormat> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::Webp),
        "svg" => Some(ImageFormat::Svg),
        _ => None,
    }
}

/// A drawn diagram: its bitmap and its size in logical pixels.
pub type Drawn = (Arc<RenderImage>, f32, f32);

/// How a diagram is drawn now: the colors, the font, the widest it may be
/// and the window's scale.
#[derive(Clone)]
pub struct Look {
    /// The colors in use.
    pub theme: ColorTheme,
    /// The font of the diagram's labels.
    pub font: SharedString,
    /// The widest the diagram may be, in logical pixels.
    pub max_width: f32,
    /// The window's scale factor.
    pub scale: f32,
}

/// Draws a diagram's source to a bitmap (and keeps what it drew).
pub type DrawDiagram = Box<dyn FnMut(&str, &Look) -> Option<Drawn>>;

/// The diagram drawer of the app: the raster cache of the mermaid block,
/// which draws once per source and look.
fn raster_drawer() -> DrawDiagram {
    let mut cache = crate::blocks::mermaid::raster::RasterCache::new();
    Box::new(move |source, look| {
        cache
            .get(source, &look.theme, &look.font, look.max_width, look.scale)
            .ok()
            .map(|raster| (raster.image, raster.width, raster.height))
    })
}

/// What the editor's providers read: the doc's assets (decoded once, by
/// name) and the diagram drawer with the look of this frame.
pub struct Shared {
    assets: Vec<(String, Vec<u8>)>,
    rendered: HashMap<String, Option<Arc<RenderImage>>>,
    look: Option<Look>,
    draw: DrawDiagram,
}

impl Shared {
    fn new(draw: DrawDiagram) -> Self {
        Self {
            assets: Vec::new(),
            rendered: HashMap::new(),
            look: None,
            draw,
        }
    }

    /// Takes the assets of the doc shown; the pictures are decoded when the
    /// page first asks for them.
    pub fn set_assets(&mut self, assets: &[(String, Vec<u8>)]) {
        self.assets = assets.to_vec();
        self.rendered.clear();
    }

    /// The picture of `src` (`assets/<name>`), decoded once. `None` for
    /// anything else, for an asset that is not a picture the page decodes
    /// and for one that is not in the doc.
    pub fn picture(&mut self, src: &str) -> Option<Arc<RenderImage>> {
        let name = src.strip_prefix("assets/")?;
        if !self.rendered.contains_key(name) {
            let decoded = self
                .assets
                .iter()
                .find(|(asset, _)| asset == name)
                .and_then(|(_, bytes)| decode(name, bytes));
            self.rendered.insert(name.to_owned(), decoded);
        }
        self.rendered.get(name).cloned().flatten()
    }

    /// The drawing of the diagram `source`, with the look of this frame.
    pub fn diagram(&mut self, source: &str) -> Option<Drawn> {
        let look = self.look.clone()?;
        (self.draw)(source, &look)
    }

    /// Sets the look the next diagrams are drawn with.
    pub fn set_look(&mut self, look: Look) {
        self.look = Some(look);
    }
}

/// The frames gpui paints are BGRA: `bytes` decoded, scaled down when wider
/// than [`PICTURE_MAX_WIDTH`], and their red and blue swapped.
#[must_use]
pub fn decode(name: &str, bytes: &[u8]) -> Option<Arc<RenderImage>> {
    if matches!(image_format(name)?, ImageFormat::Svg) {
        return None;
    }
    let mut decoded = image::load_from_memory(bytes).ok()?;
    if decoded.width() > PICTURE_MAX_WIDTH {
        decoded = decoded.resize(
            PICTURE_MAX_WIDTH,
            u32::MAX,
            image::imageops::FilterType::Triangle,
        );
    }
    let mut rgba: RgbaImage = decoded.to_rgba8();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![Frame::new(rgba)])))
}

/// Reads the editor's selection back through its clipboard writer.
#[derive(Default)]
pub struct Probe {
    armed: bool,
    caught: Option<String>,
}

/// What the page keeps besides the editor.
pub struct PageState {
    pub(super) scroll: ScrollHandle,
    pub(super) shared: Rc<RefCell<Shared>>,
    pub(super) probe: Rc<RefCell<Probe>>,
    /// The theme the editor's style was built from.
    themed: Option<ColorTheme>,
    /// Set until the first paint has given the editor its geometry.
    pub(super) wants_geometry: bool,
}

impl PageState {
    /// The state of a new page, and the editor wired to it.
    pub fn new(window: &mut Window, cx: &mut Context<DocPanel>) -> (Self, Entity<EditorState>) {
        Self::with_drawer(raster_drawer(), window, cx)
    }

    /// As [`Self::new`], with the diagram drawer given.
    pub fn with_drawer(
        draw: DrawDiagram,
        window: &mut Window,
        cx: &mut Context<DocPanel>,
    ) -> (Self, Entity<EditorState>) {
        let scroll = ScrollHandle::new();
        let shared = Rc::new(RefCell::new(Shared::new(draw)));
        let probe = Rc::new(RefCell::new(Probe::default()));
        let editor = cx.new(|cx| EditorState::new(window, cx));
        let style = syntax_style(cx);
        let themed = Some(crest_theme(cx));
        editor.update(cx, |editor, cx| {
            editor.set_markdown_style(style, cx);
            let pictures = shared.clone();
            editor.set_block_image_provider(move |src| pictures.borrow_mut().picture(src));
            let diagrams = shared.clone();
            editor.set_block_mermaid_provider(move |source| diagrams.borrow_mut().diagram(source));
            let caught = probe.clone();
            editor.set_clipboard_writer(Rc::new(move |text: &str, cx: &mut App| {
                let mut probe = caught.borrow_mut();
                if probe.armed {
                    probe.caught = Some(text.to_owned());
                } else {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
                }
            }));
            let handle = scroll.clone();
            editor.set_scroll_compensator(move |delta: Pixels, _: &mut Window, _: &mut App| {
                let mut offset = handle.offset();
                offset.y -= delta;
                handle.set_offset(offset);
            });
        });
        (
            Self {
                scroll,
                shared,
                probe,
                themed,
                wants_geometry: true,
            },
            editor,
        )
    }

    /// The text the editor's selection holds, read through its `Copy`
    /// action; empty without a selection or before the editor is drawn.
    pub(super) fn selected_text(
        &self,
        editor: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut App,
    ) -> String {
        {
            let mut probe = self.probe.borrow_mut();
            probe.armed = true;
            probe.caught = None;
        }
        let focus = editor.read(cx).focus_handle(cx);
        focus.dispatch_action(&zorite_editor::Copy, window, cx);
        let mut probe = self.probe.borrow_mut();
        probe.armed = false;
        probe.caught.take().unwrap_or_default()
    }
}

/// The editor's colors from the theme in use.
fn syntax_style(cx: &App) -> SyntaxStyle {
    let theme = cx.theme();
    let crest = crest_theme(cx);
    let color = |path: &str| hsla(crest.color(path));
    let muted = color("muted.fg");
    SyntaxStyle {
        block_label: None,
        block_label_gen: 0,
        block_ref_count: None,
        marker: muted.opacity(0.6),
        code: color("ansi.yellow"),
        code_bg: color("muted.bg"),
        link: color("link.fg"),
        tag: color("ansi.green"),
        quote: muted,
        alert_note: color("info.bg"),
        alert_tip: color("success.bg"),
        alert_important: color("ansi.magenta"),
        alert_warning: color("warning.bg"),
        alert_caution: color("error.bg"),
        alert_icons: None,
        rule: color("border"),
        mark_bg: color("warning.bg").opacity(0.35),
        popover_bg: color("popover.bg"),
        popover_border: color("border"),
        popover_fg: color("popover.fg"),
        popover_hover: color("accent.bg").opacity(0.18),
        popover_divider: color("border"),
        popover_danger: color("error.bg"),
        mono: font(theme.mono_font_family.clone()),
        property_icon: None,
    }
}

/// The page of `panel`'s doc, under the formatting toolbar.
pub fn render(panel: &mut DocPanel, window: &mut Window, cx: &mut Context<DocPanel>) -> AnyElement {
    let theme = crest_theme(cx);
    if panel
        .page
        .themed
        .as_ref()
        .is_none_or(|themed| !Arc::ptr_eq(themed, &theme))
    {
        panel.page.themed = Some(theme.clone());
        let style = syntax_style(cx);
        panel
            .page_editor
            .update(cx, |editor, cx| editor.set_markdown_style(style, cx));
    }
    let viewport = panel.page.scroll.bounds().size.width;
    let column = if f32::from(viewport) > 1.0 {
        (f32::from(viewport) - 48.0).min(PAGE_WIDTH)
    } else {
        PAGE_WIDTH
    };
    let look = Look {
        theme,
        font: cx.theme().font_family.clone(),
        max_width: (column / WIDTH_STEP).floor().max(1.0) * WIDTH_STEP,
        scale: window.scale_factor(),
    };
    panel.page.shared.borrow_mut().set_look(look);
    let (font_family, font_size, fg) = {
        let theme = cx.theme();
        (theme.font_family.clone(), theme.font_size, theme.foreground)
    };
    // The "edit" button of each diagram sits at the top right of its block,
    // from the editor's layout of the last frame.
    let editor = panel.page_editor.read(cx);
    let top = editor.offset_screen_top(0);
    let text = editor.text().to_owned();
    let mut buttons = Vec::new();
    if let Some(top) = top {
        for (index, span) in diagram_spans(&text).iter().enumerate() {
            if let Some(at) = editor.offset_screen_top(span.start) {
                buttons.push((index, at - top));
            }
        }
    }
    panel.page.wants_geometry = top.is_none();
    // The span "add to doc" just added: a wash of the accent behind the
    // editor that fades out, from the editor's layout of the last frame.
    let flash = match (top, &panel.added) {
        (Some(top), Some((range, began))) => {
            let t = (began.elapsed().as_secs_f32() / super::ADDED_FADE.as_secs_f32()).min(1.0);
            let start = editor.offset_screen_top(range.start);
            let end = editor.bounds_for_offset(range.end.saturating_sub(1).max(range.start));
            match (start, end) {
                (Some(start), Some(end)) if t < 1.0 => {
                    let y = (start - top).max(px(0.));
                    let height = (end.bottom() - top - y).max(px(0.));
                    let eased = 1.0 - (1.0 - t) * (1.0 - t);
                    Some((y, height, 0.28 * (1.0 - eased)))
                }
                _ => None,
            }
        }
        _ => None,
    };
    if panel.reveal_added && top.is_some() {
        panel.reveal_added = false;
        let (editor, scroll) = (panel.page_editor.clone(), panel.page.scroll.clone());
        let span = panel.added.as_ref().map(|(range, _)| range.clone());
        // Once the next frame has laid the new text out.
        window.on_next_frame(move |_, cx| {
            let Some(span) = span else { return };
            let editor = editor.read(cx);
            let (Some(first), Some(last)) = (
                editor.offset_screen_top(span.start),
                editor.bounds_for_offset(span.end.saturating_sub(1).max(span.start)),
            ) else {
                return;
            };
            let view = scroll.bounds();
            let mut offset = scroll.offset();
            // "Nearest": only as far as the span needs, its top kept in view.
            if last.bottom() > view.bottom() {
                let down = last.bottom() - view.bottom() + px(16.);
                let room = (first - view.top() - px(16.)).max(px(0.));
                offset.y -= down.min(room);
            } else if first < view.top() {
                offset.y += (view.top() - first + px(16.)).min(-offset.y);
            } else {
                return;
            }
            scroll.set_offset(offset);
        });
        cx.notify();
    }
    let accent = crate::theme::token(cx, "accent.bg");
    let mut body = div()
        .id("doc-page-body")
        .relative()
        .w_full()
        // zorite scrolls a wide table inside the editor and paints its row and
        // column handles (outline and `+ -` pills) with the scrolled content,
        // unclipped: without this they land outside the page.
        .min_w_0()
        .overflow_x_hidden()
        .when_some(flash, |body, (y, height, alpha)| {
            body.child(
                div()
                    .id("doc-added-flash")
                    .debug_selector(|| "doc-added-flash".to_owned())
                    .absolute()
                    .top(y)
                    .left(px(-6.))
                    .right(px(-6.))
                    .h(height)
                    .rounded(px(6.))
                    .bg(accent.opacity(alpha)),
            )
        })
        .child(panel.page_editor.clone());
    for (index, y) in buttons {
        body = body.child(
            div()
                .absolute()
                .top(y + px(6.))
                .right(px(8.))
                .invisible()
                .group_hover("doc-page", |style| style.visible())
                .child(
                    quiet_button("doc-diagram-edit", "edit", cx)
                        .on_click(cx.listener(move |this, _, _, cx| this.open_diagram(index, cx))),
                ),
        );
    }
    let page = div()
        .id("doc-page")
        .debug_selector(|| "doc-page".to_owned())
        .group("doc-page")
        .relative()
        .w_full()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&panel.page.scroll)
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, window, cx| {
                this.page_mouse_up(event, window, cx);
            }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .max_w(px(PAGE_WIDTH))
                .mx_auto()
                .min_h_full()
                .px_6()
                .pt_4()
                .pb_8()
                .font_family(font_family)
                .text_size(font_size)
                .line_height(gpui::relative(1.55))
                .text_color(fg)
                .child(body)
                // The blank space under the text puts the caret at its end.
                .child(
                    div()
                        .id("doc-page-tail")
                        .flex_1()
                        .min_h(px(120.))
                        .w_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                this.caret_to_end(window, cx);
                            }),
                        ),
                ),
        )
        .child(
            div()
                .absolute()
                .top_1()
                .right_2()
                .invisible()
                .group_hover("doc-page", |style| style.visible())
                .child(
                    quiet_button("doc-edit-source", "edit in source", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_source(cx))),
                ),
        );
    gpui_component::v_flex()
        .id("doc-page-view")
        .size_full()
        .child(super::toolbar::render(panel, cx))
        .child(page)
        .into_any_element()
}

/// The common prefix and suffix of `old` and `new` cut at character
/// boundaries: `(start, old_end, new_end)`, so `old[start..old_end]` is what
/// `new[start..new_end]` replaces.
#[must_use]
pub fn changed_region(old: &str, new: &str) -> (usize, usize, usize) {
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !(old.is_char_boundary(start) && new.is_char_boundary(start)) {
        start -= 1;
    }
    let max_tail = old.len().min(new.len()) - start;
    let mut tail = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_tail)
        .take_while(|(a, b)| a == b)
        .count();
    while !(old.is_char_boundary(old.len() - tail) && new.is_char_boundary(new.len() - tail)) {
        tail -= 1;
    }
    (start, old.len() - tail, new.len() - tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_come_from_the_extension() {
        assert_eq!(image_format("A.PNG"), Some(ImageFormat::Png));
        assert_eq!(image_format("x.jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(image_format("x.bin"), None);
    }

    #[test]
    fn the_changed_region_is_what_differs() {
        assert_eq!(changed_region("a bold b", "a **bold** b"), (2, 6, 10));
        assert_eq!(changed_region("same", "same"), (4, 4, 4));
        assert_eq!(changed_region("", "x"), (0, 0, 1));
        // Never inside a character.
        let (start, old_end, new_end) = changed_region("aé", "aè");
        assert_eq!((start, old_end, new_end), (1, 3, 3));
        assert_eq!(changed_region("ab", "abab"), (2, 2, 4));
    }

    #[test]
    fn pictures_decode_to_bgra_once_and_only_from_assets() {
        use base64::Engine as _;
        // A 1x1 red pixel.
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==")
            .unwrap();
        let mut shared = Shared::new(Box::new(|_, _| None));
        shared.set_assets(&[("red.png".into(), png), ("notes.txt".into(), vec![1])]);
        let image = shared.picture("assets/red.png").expect("decoded");
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
        let again = shared.picture("assets/red.png").unwrap();
        assert!(Arc::ptr_eq(&image, &again), "decoded once");
        assert!(shared.picture("assets/notes.txt").is_none());
        assert!(shared.picture("assets/gone.png").is_none());
        assert!(shared.picture("https://x/y.png").is_none());
        assert!(
            shared.diagram("flowchart TD\n A-->B").is_none(),
            "no look yet"
        );
    }
}
