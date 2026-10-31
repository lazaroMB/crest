//! TeX math in markdown, typeset by RaTeX.
//!
//! [`domain::math::normalize`] rewrites a markdown text before the
//! CommonMark parser sees it, so every formula reaches gpui-component's
//! `TextView` as a math node with its `_` and `\` intact. Two
//! `MarkdownPlugin`s claim those nodes:
//!
//! - [`InlineMath`] turns `$..$` into an inline object of the text flow,
//!   seated on the text baseline through the box height RaTeX reports, so
//!   selection, wrapping and streaming of the paragraph keep working;
//! - [`DisplayMath`] turns a `$$` fence or a ```` ```math ```` fence into a
//!   centered block (scrolled sideways when wider than the column).
//!
//! [`markdown`] builds a markdown `TextView` with both, and [`element`]
//! draws one formula outside markdown (calc sheets, cards).
//!
//! RaTeX lays the formula out and rasterizes it with the embedded KaTeX
//! fonts in the theme's foreground on a transparent background, at the
//! window's scale factor. A typical formula takes about 3 ms in a debug
//! build, so it runs on the background executor (several formulas in
//! parallel), once per (source, style, size, color, scale): the source
//! shows in a muted placeholder until it lands, and the windows are
//! refreshed then. [`MathCache`] keeps the results, keyed by a hash of
//! the source, and drops the least recently drawn past [`CACHE_BYTES`].
//! RaTeX only hands out a PNG, so the bitmap is decoded from it once per
//! formula. A formula RaTeX cannot parse shows as a monospace chip with
//! the source, the error and a "Copy LaTeX" button.

use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::Arc;

use domain::block::BlockId;
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, ElementId, Entity, Global, Hsla,
    InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels, RenderImage, Rgba,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, img, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin,
    TextView, TextViewState, markdown_ast::Node,
};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

/// Logical padding (px) around a rasterized formula, so antialiased
/// edges and accents that overshoot the box are not clipped.
const PAD: f32 = 2.0;

/// Display math is set this much larger than the surrounding text, as
/// KaTeX does.
const DISPLAY_SCALE: f32 = 1.15;

/// Most bytes of bitmaps the cache keeps.
pub const CACHE_BYTES: usize = 32 * 1024 * 1024;

/// Bytes an error counts for in the cache, so errors are evicted too.
const ERROR_BYTES: usize = 256;

/// A typeset formula.
pub struct Rendered {
    /// Straight BGRA pixels at the scale factor it was rendered for.
    pub image: Arc<RenderImage>,
    /// Logical size in px, padding included.
    pub width: f32,
    /// Logical height in px, padding included.
    pub height: f32,
    /// Distance in logical px from the top edge to the math baseline.
    pub baseline: f32,
}

/// Typesets `source` (math mode, no delimiters) at `font_size` logical px
/// per em, rasterized at `scale` device pixels per logical px in `color`.
/// `display` picks display style (large operators, limits above and
/// below) over text style.
///
/// # Errors
/// The parser's message when RaTeX cannot parse `source`, or the
/// rasterizer's when the formula is too large to draw.
pub fn rasterize(
    source: &str,
    display: bool,
    font_size: f32,
    scale: f32,
    color: Hsla,
) -> Result<Rendered, String> {
    let nodes = ratex_parser::parse(source).map_err(|e| e.to_string())?;
    let c: Rgba = color.into();
    let options = ratex_layout::LayoutOptions {
        style: if display {
            ratex_types::MathStyle::Display
        } else {
            ratex_types::MathStyle::Text
        },
        color: ratex_types::Color {
            r: c.r,
            g: c.g,
            b: c.b,
            a: c.a,
        },
        ..Default::default()
    };
    let layout = ratex_layout::layout(&nodes, &options);
    ratex_layout::validate_layout(&layout)?;
    let list = ratex_layout::to_display_list(&layout);
    let png = ratex_render::render_to_png(
        &list,
        &ratex_render::RenderOptions {
            font_size,
            padding: PAD,
            background_color: ratex_types::Color {
                r: 0.,
                g: 0.,
                b: 0.,
                a: 0.,
            },
            device_pixel_ratio: scale,
            ..Default::default()
        },
    )?;
    let mut pixels = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    // GPUI's RenderImage holds BGRA.
    for px in pixels.pixels_mut() {
        px.0.swap(0, 2);
    }
    let (w, h) = pixels.dimensions();
    Ok(Rendered {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])),
        width: w as f32 / scale,
        height: h as f32 / scale,
        baseline: PAD + list.height as f32 * font_size,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    /// Hash of the source: a lookup does not copy it.
    source: u64,
    display: bool,
    font_size: u32,
    color: u32,
    scale: u32,
}

fn source_hash(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// Where a formula is in the cache.
#[derive(Clone)]
pub enum Typeset {
    /// Being typeset in the background.
    Loading,
    /// Ready to draw.
    Ready(Arc<Rendered>),
    /// RaTeX's message.
    Failed(SharedString),
}

impl Typeset {
    fn bytes(&self) -> usize {
        match self {
            Self::Loading => 0,
            Self::Ready(r) => {
                let size = r.image.size(0);
                usize::try_from(size.width.0).unwrap_or(0)
                    * usize::try_from(size.height.0).unwrap_or(0)
                    * 4
            }
            Self::Failed(_) => ERROR_BYTES,
        }
    }
}

/// Typeset formulas by (source hash, style, size, color, scale), so a
/// frame only looks them up. Errors are kept too.
struct MathCache(super::svg::Lru<Key, Typeset>);

impl Default for MathCache {
    fn default() -> Self {
        Self(super::svg::Lru::new(CACHE_BYTES))
    }
}

impl Global for MathCache {}

/// The cached raster of `source`, typesetting it in the background on
/// first use ([`Typeset::Loading`] until it lands).
pub fn typeset(
    source: &str,
    display: bool,
    font_size: Pixels,
    color: Hsla,
    window: &Window,
    cx: &mut App,
) -> Typeset {
    let font_size = f32::from(font_size);
    let scale = window.scale_factor();
    let key = Key {
        source: source_hash(source),
        display,
        font_size: font_size.to_bits(),
        color: u32::from(Rgba::from(color)),
        scale: scale.to_bits(),
    };
    let cache = cx.default_global::<MathCache>();
    if let Some(hit) = cache.0.get(&key) {
        return hit;
    }
    cache.0.insert(key, Typeset::Loading, 0);
    let source = source.to_owned();
    let task = cx.background_executor().spawn(async move {
        match rasterize(&source, display, font_size, scale, color) {
            Ok(r) => Typeset::Ready(Arc::new(r)),
            Err(message) => Typeset::Failed(message.into()),
        }
    });
    cx.spawn(async move |cx| {
        let state = task.await;
        cx.update(|cx| {
            let bytes = state.bytes();
            cx.default_global::<MathCache>().0.insert(key, state, bytes);
            cx.refresh_windows();
        });
    })
    .detach();
    Typeset::Loading
}

/// One formula as an element outside markdown, in the theme's foreground
/// at the text size: inline (text style, sits on a line of text) or
/// display (display style, centered).
pub fn element(source: &str, display: bool, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let (color, size) = (theme.foreground, theme.font_size);
    if display {
        display_element(source, size * DISPLAY_SCALE, color, window, cx)
    } else {
        match typeset(source, false, size, color, window, cx) {
            Typeset::Ready(r) => image_element(&r),
            Typeset::Loading => inline_placeholder(source, cx),
            Typeset::Failed(message) => inline_error(source, &message, cx),
        }
    }
}

/// A markdown `TextView` that shows TeX math: `source` normalized by
/// [`domain::math::normalize`], with [`InlineMath`] and [`DisplayMath`].
///
/// Its parse lives in element state, which GPUI drops when the element is
/// not drawn for a frame. Blocks of the timeline scroll out of view and
/// back; they use [`markdown_block`] so the parse survives.
pub fn markdown(id: impl Into<ElementId>, source: &str) -> TextView {
    with_math(TextView::markdown(
        id,
        SharedString::from(domain::math::normalize(source)),
    ))
}

/// `view` with the math plugins.
pub fn with_math(view: TextView) -> TextView {
    view.plugin(InlineMath).plugin(DisplayMath)
}

/// [`with_math`] for gpui-base's own `TextView`, which the modal uses
/// for a document (it exposes the image resolver gpui-component's wrapper
/// hides; the theme's style and highlighter reach it through the
/// `TextViewDefaults` gpui-component installs).
pub fn with_math_base(view: gpui_base::text::TextView) -> gpui_base::text::TextView {
    view.plugin(InlineMath).plugin(DisplayMath)
}

/// The parsed document of a markdown block, kept across frames.
///
/// A `TextView` built from text keeps its [`TextViewState`] (the parsed
/// tree, the code highlights, the measured heights) in element state,
/// which GPUI drops as soon as the element misses a frame; a block that
/// scrolls out of the timeline and back is parsed and measured again. This
/// entity holds the state for the life of the block instead (through
/// [`super::block_state`]) and only hands the text over when the source
/// changed, so a frame that draws an unchanged block costs one comparison.
pub struct MarkdownState {
    /// What the shown text was derived from, as the renderer gave it.
    source: String,
    /// Which derivation produced the shown text (a document, a fenced
    /// source...), so the same source in another view is not mistaken for
    /// the one already shown.
    variant: String,
    /// Whether `source` cannot hold math ([`may_hold_math`]), so that it
    /// normalizes to itself.
    plain: bool,
    state: Entity<TextViewState>,
}

/// Whether `text` may hold a formula or a dollar [`domain::math::normalize`]
/// rewrites: without `$`, `\` or a `math` fence it returns the text as is.
fn may_hold_math(text: &str) -> bool {
    text.contains(['$', '\\']) || text.contains("math")
}

/// `at`, moved back to the start of the character it falls in.
fn floor_boundary(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

impl MarkdownState {
    fn new(variant: &str, source: &str, shown: &str, cx: &mut Context<Self>) -> Self {
        Self {
            plain: !may_hold_math(source),
            source: source.to_owned(),
            variant: variant.to_owned(),
            state: document(variant, shown, cx),
        }
    }

    /// The document state, for a `TextView::new`.
    pub fn state(&self) -> &Entity<TextViewState> {
        &self.state
    }

    /// Hands `derive(source)` to the document when `source` or `variant`
    /// differ from what it shows.
    fn sync(
        &mut self,
        variant: &str,
        source: &str,
        derive: &impl Fn(&str) -> String,
        cx: &mut Context<Self>,
    ) {
        if self.variant == variant && self.source == source {
            return;
        }
        // A streamed reply grows by appends. While it holds no math at all
        // its normalized form is itself, so the new tail goes straight to
        // the document instead of normalizing the whole reply again.
        if variant == DOCUMENT
            && self.variant == DOCUMENT
            && self.plain
            && let Some(tail) = source.strip_prefix(self.source.as_str())
            // "math" may straddle the old end.
            && !may_hold_math(&source[floor_boundary(source, self.source.len().saturating_sub(4))..])
        {
            self.source.push_str(tail);
            self.state
                .update(cx, |state, cx| state.set_text(source, cx));
            return;
        }
        self.plain = !may_hold_math(source);
        self.source.clear();
        self.source.push_str(source);
        let reformat = (self.variant == HTML) != (variant == HTML);
        self.variant.clear();
        self.variant.push_str(variant);
        let shown = derive(source);
        if reformat {
            // Markdown and HTML are parsed differently: a new document.
            self.state = document(variant, &shown, cx);
        } else {
            self.state
                .update(cx, |state, cx| state.set_text(&shown, cx));
        }
    }
}

/// A document of `text`: HTML for the [`HTML`] variant, markdown
/// otherwise.
fn document(variant: &str, text: &str, cx: &mut App) -> Entity<TextViewState> {
    if variant == HTML {
        cx.new(|cx| TextViewState::html(text, cx))
    } else {
        cx.new(|cx| TextViewState::markdown(text, cx))
    }
}

/// Variant of [`markdown_block`].
const DOCUMENT: &str = "document";
/// Variant of [`html_block`].
const HTML: &str = "html";

/// The persistent [`MarkdownState`] of block `id` showing `derive(source)`.
fn document_state(
    id: BlockId,
    variant: &str,
    source: &str,
    derive: impl Fn(&str) -> String,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TextViewState> {
    let holder = super::block_state::<MarkdownState>(id, window, cx, |_, cx| {
        MarkdownState::new(variant, source, &derive(source), cx)
    });
    holder.update(cx, |holder, cx| holder.sync(variant, source, &derive, cx));
    holder.read(cx).state().clone()
}

/// [`markdown`] for block `id`, whose parse is kept across frames.
pub fn markdown_block(id: BlockId, source: &str, window: &mut Window, cx: &mut App) -> TextView {
    let state = document_state(id, DOCUMENT, source, domain::math::normalize, window, cx);
    with_math(TextView::new(&state))
}

/// `source` as an HTML document for block `id`, whose parse is kept across
/// frames like [`markdown_block`]'s.
pub fn html_block(id: BlockId, source: &str, window: &mut Window, cx: &mut App) -> TextView {
    let state = document_state(id, HTML, source, str::to_owned, window, cx);
    TextView::new(&state)
}

/// `source` as one fenced code block in `lang` for block `id`, highlighted
/// once and kept across frames.
pub fn code_block(
    id: BlockId,
    lang: Option<&str>,
    source: &str,
    window: &mut Window,
    cx: &mut App,
) -> TextView {
    let variant = format!("code:{}", lang.unwrap_or_default());
    let state = document_state(
        id,
        &variant,
        source,
        |source| super::custom::fenced_code(lang, source),
        window,
        cx,
    );
    TextView::new(&state)
}

/// The data of a math node: the TeX source.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Formula {
    source: String,
}

const INLINE: &str = "crest-math-inline";
const DISPLAY: &str = "crest-math-display";

/// Inline `$..$` nodes as typeset objects on the text baseline.
pub struct InlineMath;

impl MarkdownPlugin for InlineMath {
    fn name(&self) -> &str {
        INLINE
    }

    fn parse(&self, node: &Node, _cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let Node::InlineMath(math) = node else {
            return None;
        };
        let source = math.value.clone();
        Some(
            MarkdownNode::new(
                INLINE,
                Formula {
                    source: source.clone(),
                },
            )
            .text(format!("${source}$")),
        )
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let formula = node.data::<Formula>()?;
        let color = context.text_style().color;
        Some(
            match typeset(
                &formula.source,
                false,
                context.font_size(),
                color,
                window,
                cx,
            ) {
                Typeset::Ready(r) => {
                    InlineElement::new(image_element(&r)).with_baseline(px(r.baseline))
                }
                Typeset::Loading => InlineElement::new(inline_placeholder(&formula.source, cx)),
                Typeset::Failed(message) => {
                    InlineElement::new(inline_error(&formula.source, &message, cx))
                }
            },
        )
    }
}

/// `$$` fences and ```` ```math ```` fences as centered display blocks.
pub struct DisplayMath;

impl MarkdownPlugin for DisplayMath {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        DISPLAY
    }

    fn parse(&self, node: &Node, _cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let source = match node {
            Node::Math(math) => math.value.clone(),
            Node::Code(code) if code.lang.as_deref() == Some("math") => code.value.clone(),
            _ => return None,
        };
        let text = format!("$$\n{source}\n$$");
        Some(MarkdownNode::new(DISPLAY, Formula { source }).text(text))
    }

    fn render(
        &self,
        node: &MarkdownNode,
        window: &mut Window,
        cx: &mut App,
    ) -> impl gpui::IntoElement {
        let Some(formula) = node.data::<Formula>() else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (color, size) = (theme.foreground, theme.font_size * DISPLAY_SCALE);
        display_element(&formula.source, size, color, window, cx)
    }
}

fn image_element(r: &Rendered) -> AnyElement {
    img(r.image.clone())
        .w(px(r.width))
        .h(px(r.height))
        .flex_none()
        .into_any_element()
}

fn display_element(
    source: &str,
    size: Pixels,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match typeset(source, true, size, color, window, cx) {
        Typeset::Ready(r) => div()
            .id(element_id("math-display", source))
            .w_full()
            .overflow_x_scroll()
            .py_1()
            .child(h_flex().child(div().mx_auto().child(image_element(&r))))
            .into_any_element(),
        Typeset::Loading => div()
            .w_full()
            .py_1()
            .child(h_flex().child(div().mx_auto().child(inline_placeholder(source, cx))))
            .into_any_element(),
        Typeset::Failed(message) => display_error(source, &message, cx),
    }
}

/// A formula still being typeset: its source, muted.
fn inline_placeholder(source: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .font_family(theme.mono_font_family.clone())
        .text_color(theme.muted_foreground)
        .child(SharedString::from(source.to_owned()))
        .into_any_element()
}

fn element_id(prefix: &str, source: &str) -> ElementId {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    ElementId::Name(SharedString::from(format!(
        "{prefix}-{:x}",
        hasher.finish()
    )))
}

fn copy_button(source: &str, prefix: &str) -> Button {
    let copy = source.to_owned();
    Button::new(element_id(prefix, source))
        .ghost()
        .xsmall()
        .label("Copy LaTeX")
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
        })
}

/// A formula that did not parse, inside a line: its source in monospace,
/// the error as a tooltip of the copy button.
fn inline_error(source: &str, message: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .gap_1()
        .px_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.danger)
        .font_family(theme.mono_font_family.clone())
        .text_color(theme.danger)
        .child(SharedString::from(source.to_owned()))
        .child(
            copy_button(source, "math-copy-inline").tooltip(SharedString::from(message.to_owned())),
        )
        .into_any_element()
}

/// A display formula that did not parse: the error, the source in
/// monospace and a "Copy LaTeX" button.
fn display_error(source: &str, message: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .gap_1()
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.danger)
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(SharedString::from(format!("TeX error: {message}"))),
                )
                .child(copy_button(source, "math-copy-display")),
        )
        .child(
            div()
                .font_family(theme.mono_font_family.clone())
                .text_sm()
                .text_color(theme.foreground)
                .child(SharedString::from(source.to_owned())),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[gpui::test]
    fn blocks_keep_their_parse_across_frames(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|_, cx| crate::timeline::TimelineView::new(cx));
        let id = BlockId::new();
        let holder = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| crate::blocks::existing_block_state::<MarkdownState>(id, cx))
                .unwrap_or_else(|| panic!("the block has a document state"))
        };
        let rendered = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            holder(cx).read_with(cx, |holder, cx| {
                holder.state.read(cx).rendered_text().as_str().to_owned()
            })
        };
        cx.update(|window, cx| {
            markdown_block(id, "# One", window, cx);
        });
        let first = holder(cx).read_with(cx, |holder, _| holder.state.entity_id());
        assert!(rendered(cx).contains("One"));

        // The same source on a later frame: the same document, untouched.
        cx.update(|window, cx| {
            markdown_block(id, "# One", window, cx);
        });
        assert_eq!(
            holder(cx).read_with(cx, |holder, _| holder.state.entity_id()),
            first
        );

        // A change of source reaches the document; so does a change of view
        // of the same source (the document as a fenced code block).
        cx.update(|window, cx| {
            markdown_block(id, "# Two", window, cx);
        });
        let text = rendered(cx);
        assert!(text.contains("Two") && !text.contains("One"), "{text:?}");
        cx.update(|window, cx| {
            code_block(id, Some("markdown"), "# Two", window, cx);
        });
        assert!(
            rendered(cx).contains("# Two"),
            "the source view shows the markup"
        );
        assert_eq!(
            holder(cx).read_with(cx, |holder, _| holder.state.entity_id()),
            first,
            "one document per block"
        );
    }

    #[gpui::test]
    fn html_nodes_keep_their_parse_and_get_their_own_document(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|_, cx| crate::timeline::TimelineView::new(cx));
        let id = BlockId::new();
        let document = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            let holder = cx
                .update(|_, cx| crate::blocks::existing_block_state::<MarkdownState>(id, cx))
                .unwrap_or_else(|| panic!("the node has a document state"));
            holder.read_with(cx, |holder, cx| {
                (
                    holder.state.entity_id(),
                    holder.state.read(cx).rendered_text().as_str().to_owned(),
                )
            })
        };
        cx.update(|window, cx| {
            html_block(id, "<p>One <b>bold</b></p>", window, cx);
        });
        let (first, text) = document(cx);
        assert!(text.contains("One bold"), "{text:?}");
        cx.update(|window, cx| {
            html_block(id, "<p>One <b>bold</b></p>", window, cx);
        });
        assert_eq!(document(cx).0, first, "kept across frames");
        // The same node as markdown is parsed as markdown, in a new document.
        cx.update(|window, cx| {
            markdown_block(id, "<b>raw</b> *em*", window, cx);
        });
        let (second, _) = document(cx);
        assert_ne!(second, first);
    }

    #[gpui::test]
    fn streamed_appends_reach_the_document_and_math_is_still_normalized(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|_, cx| crate::timeline::TimelineView::new(cx));
        let id = BlockId::new();
        let mut source = String::new();
        for piece in [
            "Plain ",
            "streamed ",
            "text, then ",
            "a price of $5 ",
            "and $x_1$.",
        ] {
            source.push_str(piece);
            cx.update(|window, cx| {
                markdown_block(id, &source, window, cx);
            });
        }
        // The same text handed over at once.
        let whole = BlockId::new();
        cx.update(|window, cx| {
            markdown_block(whole, &source, window, cx);
        });
        cx.run_until_parked();
        let shown = |id: BlockId, cx: &mut gpui::VisualTestContext| {
            let holder = cx
                .update(|_, cx| crate::blocks::existing_block_state::<MarkdownState>(id, cx))
                .unwrap_or_else(|| panic!("the block has a document state"));
            holder.read_with(cx, |holder, cx| {
                assert_eq!(holder.source, source);
                assert!(!holder.plain, "the dollars end the plain run");
                holder.state.read(cx).rendered_text().as_str().to_owned()
            })
        };
        let streamed = shown(id, cx);
        assert!(streamed.starts_with("Plain streamed text, then a price of "));
        assert_eq!(streamed, shown(whole, cx));
    }

    #[test]
    fn rasterizes_inline_and_display_with_the_baseline_inside() {
        let inline = rasterize(r"x_i^2 + \frac{a}{b}", false, 16.0, 2.0, gpui::black())
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(inline.width > 0. && inline.height > 0.);
        assert!(inline.baseline > PAD && inline.baseline < inline.height);
        let display = rasterize(r"\sum_{i=1}^n x_i", true, 16.0, 1.0, gpui::black())
            .unwrap_or_else(|e| panic!("{e}"));
        // Display style puts the limits above and below: taller.
        let text = rasterize(r"\sum_{i=1}^n x_i", false, 16.0, 1.0, gpui::black())
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(display.height > text.height);
    }

    #[test]
    fn scale_multiplies_pixels_not_logical_size() {
        let one = rasterize("x", false, 16.0, 1.0, gpui::black()).unwrap_or_else(|e| panic!("{e}"));
        let two = rasterize("x", false, 16.0, 2.0, gpui::black()).unwrap_or_else(|e| panic!("{e}"));
        assert!((one.width - two.width).abs() <= 1.0);
        let pixels = two.image.size(0);
        assert!(pixels.width.0 as f32 >= two.width * 1.9);
    }

    #[test]
    fn mhchem_typesets() {
        assert!(rasterize(r"\ce{2H2 + O2 -> 2H2O}", false, 16.0, 1.0, gpui::black()).is_ok());
    }

    #[test]
    fn parse_errors_carry_the_message() {
        let Err(message) = rasterize(r"\frac{a", false, 16.0, 1.0, gpui::black()) else {
            panic!("an unclosed group must not typeset");
        };
        assert!(!message.is_empty());
        assert!(rasterize(r"\notacommand{x}", false, 16.0, 1.0, gpui::black()).is_err());
    }

    #[gpui::test]
    fn text_block_typesets_inline_display_and_broken_math(cx: &mut gpui::TestAppContext) {
        use application::ports::events::DomainEvent;
        use domain::block::{Block, BlockKind};
        use gpui::{point, size};

        use crate::timeline::TimelineView;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let markdown = "The mean $\\bar x_1$ costs $5 and $20, with \\(a_i\\):\n\n\
                        $$\\sum_{i=1}^n x_i$$\n\nand $\\frac{a$ breaks; \\ce{H2O}.\n";
        let block = Block::new(BlockKind::Text {
            markdown: markdown.into(),
        });
        view.update(cx, |view, cx| {
            view.apply(&DomainEvent::BlockAdded { block }, cx);
        });
        for _ in 0..2 {
            cx.draw(point(px(0.), px(0.)), size(px(700.), px(500.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        }
        cx.update(|_, cx| {
            let cache = &cx.default_global::<MathCache>().0;
            let typeset = |source: &str, display: bool| {
                let hash = source_hash(source);
                cache
                    .entries()
                    .find(|(k, _)| k.source == hash && k.display == display)
                    .map(|(_, v)| match v {
                        Typeset::Ready(_) => Some(true),
                        Typeset::Failed(_) => Some(false),
                        Typeset::Loading => None,
                    })
            };
            assert_eq!(typeset("\\bar x_1", false), Some(Some(true)));
            assert_eq!(typeset("a_i", false), Some(Some(true)));
            assert_eq!(typeset("\\ce{H2O}", false), Some(Some(true)));
            assert_eq!(typeset("\\sum_{i=1}^n x_i", true), Some(Some(true)));
            assert_eq!(typeset("\\frac{a", false), Some(Some(false)));
            // Prices are prose, not a formula.
            assert_eq!(typeset("5 and ", false), None);
            assert_eq!(cache.entries().count(), 5);
        });
    }

    #[test]
    fn typical_formula_is_cheap() {
        let _ = rasterize("x", false, 16.0, 2.0, gpui::black());
        let start = Instant::now();
        for _ in 0..10 {
            let _ = rasterize(
                r"\hat\beta = (X^\top X)^{-1} X^\top y",
                false,
                16.0,
                2.0,
                gpui::black(),
            );
        }
        eprintln!("rasterize: {:?} per formula", start.elapsed() / 10);
    }
}
