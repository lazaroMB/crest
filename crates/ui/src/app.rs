//! Application entry point: boots GPUI, opens the main window and owns the
//! app-wide actions ([`Quit`], [`ToggleFullscreen`]).
//!
//! The window opens as `app.opt.window` says ([`WindowMode`]): fullscreen
//! by default (maximized on Windows), which GPUI implements on macOS, X11
//! and Wayland (on X11 fullscreen and maximized are requested after the
//! first frame; see `open_main_window`). A compositor that refuses the
//! request leaves the window at its restore bounds, centered on the primary
//! display. The keys come from the crest keymap (`app.quit` and
//! `window.fullscreen`, remappable): the session runs them, and the
//! workbench while no project is open.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, MouseButton, MouseMoveEvent,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, actions,
    canvas, div, px, size,
};
use gpui_component::{ActiveTheme as _, Root, Theme, h_flex, v_flex};

pub use application_core::window::WindowMode;

use crate::chats::ChatsPanel;
pub use crate::docs::DocView;

actions!(crest, [Quit, ToggleFullscreen]);

/// Monospace family used when the options name none.
pub const DEFAULT_FONT_FAMILY: &str = "JetBrains Mono";
/// Base font size in pixels.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Width of the draggable divider between the conversation and the doc
/// panel, in pixels: a 1 px rule (the panel's left border) in a slightly
/// wider grip.
pub const DOC_DIVIDER_PX: f32 = 3.0;
/// Share of the width each side of the split keeps at least.
pub const DOC_MIN_SIDE: f32 = 0.25;
/// Where the divider starts: half and half.
pub const DEFAULT_DOC_SPLIT: f32 = 0.5;

/// Keeps the conversation's `ratio` of a split `width` pixels wide so that
/// each side keeps at least a quarter of the width: the conversation at
/// least 25 %, the panel at least 25 % once the divider's pixels are taken
/// from its side.
#[must_use]
pub fn clamp_split(ratio: f32, width: f32) -> f32 {
    let low = DOC_MIN_SIDE;
    let divider = if width > 0.0 {
        DOC_DIVIDER_PX / width
    } else {
        0.0
    };
    let high = (1.0 - DOC_MIN_SIDE - divider).max(low);
    if ratio.is_finite() {
        ratio.clamp(low, high)
    } else {
        DEFAULT_DOC_SPLIT.clamp(low, high)
    }
}

/// Sent by the root when the person changes the doc view from the strip
/// ("show chat"): the host keeps the doc panel's own view in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocViewChanged(pub DocView);

/// A resolved color theme (`app.opt.theme`).
pub type ColorTheme = std::sync::Arc<domain::theme::Theme>;

/// Startup options of the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct UiOptions {
    /// Shown in the window title, usually the workspace directory name.
    pub workspace_label: String,
    /// Preferred monospace family for the whole UI. When it is not installed
    /// the theme's platform monospace font is used instead (see
    /// [`resolve_font_family`]).
    pub font_family: String,
    /// Base font size in pixels.
    pub font_size: f32,
    /// Color theme (`app.opt.theme`), TwoCrest dark by default.
    pub theme: ColorTheme,
    /// How the main window opens (`app.opt.window`).
    pub window: WindowMode,
}

impl Default for UiOptions {
    fn default() -> Self {
        Self {
            workspace_label: String::new(),
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            font_size: DEFAULT_FONT_SIZE,
            theme: crate::theme::default_theme(),
            window: WindowMode::default(),
        }
    }
}

/// Picks the font family to render with.
///
/// GPUI panics when it lays out text in a family it cannot load, and generic
/// names such as `monospace` are not resolved, so the preferred family is
/// used only when it is installed; otherwise `fallback`, the theme's
/// installed platform monospace font (Menlo, DejaVu Sans Mono, ...).
pub fn resolve_font_family(preferred: &str, installed: &[String], fallback: &str) -> String {
    let preferred = preferred.trim();
    if !preferred.is_empty() && installed.iter().any(|name| name == preferred) {
        preferred.to_owned()
    } else {
        fallback.to_owned()
    }
}

/// Errors that stop the UI from starting.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    /// The platform could not create the main window.
    #[error("failed to open the main window: {0}")]
    OpenWindow(String),
}

/// Title of the main window for a workspace label.
pub fn window_title(workspace_label: &str) -> String {
    if workspace_label.is_empty() {
        "crest".to_owned()
    } else {
        format!("crest — {workspace_label}")
    }
}

/// Boots GPUI and runs the event loop until the user quits, with an empty
/// [`RootView`].
///
/// On Linux this returns once the app quits; on macOS the platform may end
/// the process from inside the event loop instead.
pub fn run(opts: UiOptions) -> Result<(), UiError> {
    run_with(opts, |_, cx| cx.new(RootView::new))
}

/// Like [`run`], with the root view made by `build_root` once the main
/// window exists (GPUI, the theme and the app key bindings are ready).
///
/// `build_root` usually fills the slots of the root and focuses the input;
/// when it leaves nothing focused, the root view gets the focus.
pub fn run_with<F, V>(opts: UiOptions, build_root: F) -> Result<(), UiError>
where
    F: FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
    V: Render + Focusable,
{
    let title = window_title(&opts.workspace_label);
    let failure = std::rc::Rc::new(std::cell::RefCell::new(None));
    let failure_in_app = failure.clone();

    crate::assets::install(gpui_platform::application()).run(move |cx: &mut App| {
        gpui_component::init(cx);
        zorite_editor::bind_keys(cx);
        apply_theme(&opts, cx);
        bind_app_keys(cx);
        if let Err(err) = open_main_window(title, opts.window, build_root, cx) {
            *failure_in_app.borrow_mut() = Some(err);
            cx.quit();
            return;
        }
        cx.activate(true);
    });

    match failure.borrow_mut().take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn apply_theme(opts: &UiOptions, cx: &mut App) {
    // `change` loads the mode's theme and resolves its default monospace
    // family to one that is installed, which becomes our fallback.
    Theme::change(crate::theme::mode(opts.theme.appearance), None, cx);
    crate::theme::apply(&opts.theme, cx);
    let installed = cx.text_system().all_font_names();
    let fallback = Theme::global(cx).mono_font_family.clone();
    let family = SharedString::from(resolve_font_family(
        &opts.font_family,
        &installed,
        &fallback,
    ));
    let font_size = px(opts.font_size.max(1.0));
    Theme::update(cx, |theme| {
        theme.font_family = family.clone();
        theme.mono_font_family = family;
        theme.font_size = font_size;
        theme.mono_font_size = font_size;
    });
}

/// Switches the color theme (the project's `app.opt.theme` once it is
/// open, or a `/theme` preview), keeping the fonts already chosen.
pub fn set_theme(theme_data: &ColorTheme, cx: &mut App) {
    let theme = Theme::global(cx);
    let (family, mono, size, mono_size) = (
        theme.font_family.clone(),
        theme.mono_font_family.clone(),
        theme.font_size,
        theme.mono_font_size,
    );
    Theme::change(crate::theme::mode(theme_data.appearance), None, cx);
    crate::theme::apply(theme_data, cx);
    Theme::update(cx, |theme| {
        theme.font_family = family;
        theme.mono_font_family = mono;
        theme.font_size = size;
        theme.mono_font_size = mono_size;
    });
    cx.refresh_windows();
}

/// What the app-wide actions do. They have no GPUI key bindings: the
/// crest keymap dispatches them, so they can be remapped and the help
/// lists them.
fn bind_app_keys(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &ToggleFullscreen, cx| {
        if let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, _| window.toggle_fullscreen())
                .ok();
        }
    });
    // macOS keeps apps alive without windows; crest has a single window.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// What the main window asks the platform for: the bounds to open with,
/// and what to request after the first frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAs {
    Windowed,
    Maximized,
    Fullscreen,
}

/// How to open a window in `mode`: `(at open, after the first frame)`.
///
/// X11 window managers ignore the fullscreen and maximize requests GPUI
/// sends before the window is mapped, so on X11 the window opens windowed
/// and asks once it has drawn its first frame.
fn open_plan(mode: WindowMode, x11: bool) -> (OpenAs, Option<OpenAs>) {
    let wanted = match mode {
        WindowMode::Fullscreen { .. } => OpenAs::Fullscreen,
        WindowMode::Maximized { .. } => OpenAs::Maximized,
        WindowMode::Windowed { .. } => OpenAs::Windowed,
    };
    if x11 && wanted != OpenAs::Windowed {
        (OpenAs::Windowed, Some(wanted))
    } else {
        (wanted, None)
    }
}

fn open_main_window<F, V>(
    title: String,
    mode: WindowMode,
    build_root: F,
    cx: &mut App,
) -> Result<(), UiError>
where
    F: FnOnce(&mut Window, &mut App) -> Entity<V>,
    V: Render + Focusable,
{
    let (width, height) = mode.size();
    // Pixel sizes are small whole numbers: exact in f32.
    #[allow(clippy::cast_precision_loss)]
    let restore = Bounds::centered(None, size(px(width as f32), px(height as f32)), cx);
    let (at_open, later) = open_plan(mode, cx.compositor_name() == "X11");
    let window_bounds = match at_open {
        OpenAs::Windowed => WindowBounds::Windowed(restore),
        OpenAs::Maximized => WindowBounds::Maximized(restore),
        OpenAs::Fullscreen => WindowBounds::Fullscreen(restore),
    };
    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title)),
            ..Default::default()
        }),
        app_id: Some("crest".to_owned()),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        let view = build_root(window, cx);
        if window.focused(cx).is_none() {
            window.focus(&view.focus_handle(cx), cx);
        }
        match later {
            Some(OpenAs::Fullscreen) => {
                window.on_next_frame(|window, _| window.toggle_fullscreen())
            }
            Some(OpenAs::Maximized) => window.on_next_frame(|window, _| window.zoom_window()),
            Some(OpenAs::Windowed) | None => {}
        }
        cx.new(|cx| Root::new(view, window, cx))
    })
    .map(|_| ())
    .map_err(|err| UiError::OpenWindow(err.to_string()))
}

/// Root view of the main window: timeline on top (grows), prompt input
/// below it and the status bar at the bottom, with an optional side panel
/// on the left and overlays (pickers, dialogs) above everything.
///
/// Each area is a slot that later views fill (`set_timeline`, `set_input`,
/// `set_status`); an empty slot renders a placeholder box. The side panel
/// ([`Self::set_side_panel`]) takes no room when unset, and each overlay
/// ([`Self::push_overlay`]) is rendered as a child of a `relative`
/// container over the whole window, in the order pushed; overlays draw
/// nothing while closed and position themselves (`absolute`) when open.
///
/// The timeline, the side panel and the overlays pushed with
/// [`Self::push_cached_overlay`] are cached views (`AnyView::cached`): a
/// frame that another view asked for (the activity line's spinner, every
/// 100 ms during a turn) reuses what they drew last instead of rendering
/// them again. A cached view draws again only when it, or a view inside
/// it, calls `cx.notify()`, when its bounds change, or when the whole
/// window refreshes (focus, theme, resize). So everything such a view
/// shows must notify it when it changes: entities that are only read
/// while rendering (not drawn as child views) do not. The input and the
/// status bar are not cached: their height follows their content, and a
/// cached view is laid out at a style given up front.
pub struct RootView {
    focus_handle: FocusHandle,
    timeline: Option<AnyView>,
    input: Option<AnyView>,
    activity: Option<AnyView>,
    status: Option<AnyView>,
    side_panel: Option<SidePanel>,
    /// The chats sidebar, laid out over the top left of the body.
    sidebar: Option<Entity<ChatsPanel>>,
    overlays: Vec<Overlay>,
    /// The doc panel, shown as [`Self::doc_view`] says.
    doc_panel: Option<AnyView>,
    doc_view: DocView,
    /// The conversation's share of the split, remembered with the window.
    doc_split: f32,
    /// The agent's last line, in the strip under a doc shown alone.
    doc_strip: Option<String>,
    /// Bounds of the split area at the last paint, to turn a drag into a ratio.
    split_bounds: Rc<Cell<Bounds<Pixels>>>,
    split_drag: bool,
}

impl EventEmitter<DocViewChanged> for RootView {}

/// The side panel and how wide it is (zero while hidden).
struct SidePanel {
    view: AnyView,
    width: Box<dyn Fn(&App) -> Pixels>,
}

/// A layer above the window, cached or not.
struct Overlay {
    view: AnyView,
    cached: bool,
}

impl RootView {
    /// Creates the root view with every slot empty.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            timeline: None,
            input: None,
            activity: None,
            status: None,
            side_panel: None,
            sidebar: None,
            overlays: Vec::new(),
            doc_panel: None,
            doc_view: DocView::Hidden,
            doc_split: DEFAULT_DOC_SPLIT,
            doc_strip: None,
            split_bounds: Rc::default(),
            split_drag: false,
        }
    }

    /// Fills the doc panel slot. How it shows is [`Self::set_doc_view`]'s:
    /// beside the timeline, in its place, or not at all. It is a cached
    /// view like the timeline: the panel notifies itself when it changes.
    pub fn set_doc_panel(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.doc_panel = Some(view.into());
        cx.notify();
    }

    /// Shows the doc panel [`DocView::Split`] (timeline, divider, panel),
    /// [`DocView::Alone`] (the panel takes the body; the prompt and status
    /// bar stay, with a strip of the agent's last line and "show chat") or
    /// not at all. The sidebar and the side panel keep their state.
    pub fn set_doc_view(&mut self, view: DocView, cx: &mut Context<Self>) {
        if self.doc_view != view {
            self.doc_view = view;
            self.split_drag = false;
            cx.notify();
        }
    }

    /// How the doc panel shows.
    #[must_use]
    pub fn doc_view(&self) -> DocView {
        self.doc_view
    }

    /// The conversation's share of the split (0.25 to 0.75).
    #[must_use]
    pub fn doc_split(&self) -> f32 {
        self.doc_split
    }

    /// Sets the conversation's share of the split, kept within the limits
    /// for the width the split had at its last paint.
    pub fn set_doc_split(&mut self, ratio: f32, cx: &mut Context<Self>) {
        let width = f32::from(self.split_bounds.get().size.width);
        self.doc_split = clamp_split(ratio, width);
        cx.notify();
    }

    /// The agent's last line, shown in the strip above the prompt while the
    /// doc is alone.
    pub fn set_doc_strip(&mut self, line: Option<String>, cx: &mut Context<Self>) {
        if self.doc_strip != line {
            self.doc_strip = line;
            cx.notify();
        }
    }

    /// Fills the chats sidebar. It sits over the top left of the body, above
    /// the prompt and the status bar, which span the whole window; the body
    /// leaves it the room its width asks for ([`ChatsPanel::layout`]).
    pub fn set_sidebar(&mut self, sidebar: Entity<ChatsPanel>, cx: &mut Context<Self>) {
        self.sidebar = Some(sidebar);
        cx.notify();
    }

    /// Fills the side panel on the left (the workspace tree), `width(cx)`
    /// wide: zero while it is hidden. The panel is a cached view: when its
    /// width changes it must notify itself (the root then asks again).
    pub fn set_side_panel(
        &mut self,
        view: impl Into<AnyView>,
        width: impl Fn(&App) -> Pixels + 'static,
        cx: &mut Context<Self>,
    ) {
        self.side_panel = Some(SidePanel {
            view: view.into(),
            width: Box::new(width),
        });
        cx.notify();
    }

    /// Adds a layer above the window (a picker, the approval dialog).
    /// Later overlays are drawn above earlier ones.
    pub fn push_overlay(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.overlays.push(Overlay {
            view: view.into(),
            cached: false,
        });
        cx.notify();
    }

    /// [`Self::push_overlay`] for a cached view, laid out over the whole
    /// window: open, its root must fill it (`absolute` and `size_full`,
    /// since a cached view's root is laid out on its own), and everything
    /// it shows must notify it.
    pub fn push_cached_overlay(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.overlays.push(Overlay {
            view: view.into(),
            cached: true,
        });
        cx.notify();
    }

    /// Fills the timeline area.
    pub fn set_timeline(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.timeline = Some(view.into());
        cx.notify();
    }

    /// Fills the prompt input area.
    pub fn set_input(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.input = Some(view.into());
        cx.notify();
    }

    /// Fills the line between the timeline and the input (what the session
    /// is doing, the queued prompts); it takes no room when unset.
    pub fn set_activity(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.activity = Some(view.into());
        cx.notify();
    }

    /// Fills the status bar area.
    pub fn set_status(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.status = Some(view.into());
        cx.notify();
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Slot content: the view when present (cached at `cached` when given),
/// else placeholder text.
fn slot(
    view: &Option<AnyView>,
    cached: Option<StyleRefinement>,
    placeholder: &'static str,
    cx: &App,
) -> AnyElement {
    match (view, cached) {
        (Some(view), Some(style)) => view.clone().cached(style).into_any_element(),
        (Some(view), None) => view.clone().into_any_element(),
        (None, _) => div()
            .text_color(cx.theme().muted_foreground)
            .child(placeholder)
            .into_any_element(),
    }
}

impl Render for RootView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = body(self, cx).into_any_element();
        let (prompt, status) = footer(self, cx);
        div()
            .id("crest-root")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .child(v_flex().size_full().child(body).child(prompt).child(status))
            .children(self.overlays.iter().map(overlay_element))
    }
}

/// The timeline and the activity line: what the conversation shows. In a
/// split it is `share` of the split's width.
fn conversation(root: &RootView, share: Option<f32>, cx: &App) -> impl IntoElement {
    let column = v_flex()
        .debug_selector(|| "conversation".to_owned())
        .h_full();
    match share {
        Some(share) => column.flex_none().w(gpui::relative(share)),
        None => column.flex_1().min_w_0(),
    }
    .child(
        div()
            .id("timeline")
            .relative()
            .flex_1()
            .min_h_0()
            .p_2()
            .child(slot(
                &root.timeline,
                Some(StyleRefinement::default().size_full()),
                "timeline",
                cx,
            ))
            .child(crate::tour::probe(crate::tour::Anchor::Timeline)),
    )
    .children(root.activity.clone())
}

/// The prompt and the status bar. They span the whole window; the sidebar
/// and the tree stop where the prompt starts.
fn footer(root: &RootView, cx: &App) -> (impl IntoElement, impl IntoElement) {
    let theme = cx.theme();
    let (border, status_bg) = (theme.border, theme.title_bar);
    let prompt = div()
        .debug_selector(|| "prompt-row".to_owned())
        .relative()
        .px_2()
        .pb_1()
        .border_t_1()
        .border_color(border)
        .child(slot(&root.input, None, "input", cx))
        .child(crate::tour::probe(crate::tour::Anchor::Prompt));
    let status = div()
        .debug_selector(|| "status-row".to_owned())
        .px_2()
        .py_1()
        .border_t_1()
        .border_color(border)
        .bg(status_bg)
        .child(slot(&root.status, None, "status", cx));
    (prompt, status)
}

/// What is above the prompt: the sidebar, the tree and the conversation.
/// The sidebar is an overlay on the top left, drawn last so it paints over
/// the band and the column the layout keeps free for it; the hidden state
/// floats its rows over the conversation without taking any room.
fn body(root: &RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let side_panel = root.side_panel.as_ref().map(|panel| {
        let width = (panel.width)(cx);
        panel
            .view
            .clone()
            .cached(StyleRefinement::default().w(width).h_full().flex_none())
    });
    let room = root
        .sidebar
        .as_ref()
        .map(|sidebar| sidebar.read(cx).layout());
    let doc = root
        .doc_panel
        .as_ref()
        .filter(|_| root.doc_view != DocView::Hidden);
    let main = match doc {
        None => conversation(root, None, cx).into_any_element(),
        Some(panel) if root.doc_view == DocView::Split => split(root, panel, cx),
        Some(panel) => alone(root, panel, cx),
    };
    div()
        .id("crest-body")
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(
            h_flex()
                .size_full()
                .pl(room.map_or(px(0.), |room| room.reserve))
                .pt(room.map_or(px(0.), |room| room.band))
                .children(side_panel)
                .child(main),
        )
        .children(root.sidebar.clone())
}

/// The doc panel as a cached view filling its area.
fn doc_panel_view(panel: &AnyView) -> AnyElement {
    panel
        .clone()
        .cached(StyleRefinement::default().size_full())
        .into_any_element()
}

/// Timeline, a draggable divider and the doc panel. The ratio is the
/// timeline's share of the area they share (the tree keeps its own slot to
/// the left); a drag is read from the mouse over the whole area.
fn split(root: &RootView, panel: &AnyView, cx: &mut Context<RootView>) -> AnyElement {
    let theme = cx.theme();
    let (border, accent) = (theme.border, theme.accent);
    let bounds = root.split_bounds.clone();
    let area = bounds.clone();
    let probe = canvas(
        move |new, window, _| {
            if bounds.replace(new).size != new.size {
                window.refresh();
            }
        },
        |_, (), _, _| {},
    )
    .absolute()
    .size_full();
    let divider = div()
        .id("doc-divider")
        .debug_selector(|| "doc-divider".to_owned())
        .flex_none()
        .w(px(DOC_DIVIDER_PX))
        .h_full()
        .flex()
        .justify_center()
        .hover(move |style| style.bg(accent.opacity(0.4)))
        .cursor_col_resize()
        .child(div().w(px(1.)).h_full().bg(border))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                if event.click_count == 2 {
                    this.set_doc_split(DEFAULT_DOC_SPLIT, cx);
                } else {
                    this.split_drag = true;
                }
                cx.stop_propagation();
            }),
        );
    h_flex()
        .id("doc-split")
        .debug_selector(|| "doc-split".to_owned())
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            if !this.split_drag {
                return;
            }
            if event.pressed_button != Some(MouseButton::Left) {
                this.split_drag = false;
                return;
            }
            let area = area.get();
            let width = f32::from(area.size.width);
            if width > 0.0 {
                let ratio = f32::from(event.position.x - area.origin.x) / width;
                this.set_doc_split(ratio, cx);
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.split_drag = false),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.split_drag = false),
        )
        .child(probe)
        .child(conversation(
            root,
            Some(clamp_split(
                root.doc_split,
                f32::from(root.split_bounds.get().size.width),
            )),
            cx,
        ))
        .child(divider)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(doc_panel_view(panel)),
        )
        .into_any_element()
}

/// The doc panel in place of the conversation, with the activity line and
/// the strip of the agent's last line under it.
fn alone(root: &RootView, panel: &AnyView, cx: &mut Context<RootView>) -> AnyElement {
    let theme = cx.theme();
    let (border, muted, accent, fg) = (
        theme.border,
        theme.muted_foreground,
        crate::theme::token(cx, "accent.bg"),
        theme.foreground,
    );
    let line = root.doc_strip.clone().unwrap_or_default();
    let strip = h_flex()
        .id("doc-strip")
        .debug_selector(|| "doc-strip".to_owned())
        .gap_2()
        .items_center()
        .px_2p5()
        .py_1()
        .text_xs()
        .border_t_1()
        .border_color(border)
        .child(div().flex_none().text_color(accent).child("agent"))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(muted)
                .child(SharedString::from(line)),
        )
        .child(
            div()
                .id("doc-show-chat")
                .debug_selector(|| "doc-show-chat".to_owned())
                .flex_none()
                .cursor_pointer()
                .text_color(muted)
                .hover(move |style| style.text_color(fg))
                .child("show chat")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_doc_view(DocView::Split, cx);
                    cx.emit(DocViewChanged(DocView::Split));
                })),
        );
    v_flex()
        .debug_selector(|| "doc-alone".to_owned())
        .flex_1()
        .min_w_0()
        .h_full()
        .child(div().flex_1().min_h_0().child(doc_panel_view(panel)))
        .children(root.activity.clone())
        .child(strip)
        .into_any_element()
}

/// A layer above the window: cached ones fill it.
fn overlay_element(overlay: &Overlay) -> AnyElement {
    let view = overlay.view.clone();
    if overlay.cached {
        view.cached(
            StyleRefinement::default()
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
        )
        .into_any_element()
    } else {
        view.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_asks_for_fullscreen_and_maximized_after_the_first_frame() {
        let size = (1280, 800);
        let full = WindowMode::Fullscreen {
            width: size.0,
            height: size.1,
        };
        let max = WindowMode::Maximized {
            width: size.0,
            height: size.1,
        };
        let windowed = WindowMode::Windowed {
            width: size.0,
            height: size.1,
        };
        assert_eq!(open_plan(full, false), (OpenAs::Fullscreen, None));
        assert_eq!(open_plan(max, false), (OpenAs::Maximized, None));
        assert_eq!(open_plan(windowed, false), (OpenAs::Windowed, None));
        assert_eq!(
            open_plan(full, true),
            (OpenAs::Windowed, Some(OpenAs::Fullscreen))
        );
        assert_eq!(
            open_plan(max, true),
            (OpenAs::Windowed, Some(OpenAs::Maximized))
        );
        assert_eq!(open_plan(windowed, true), (OpenAs::Windowed, None));
    }

    #[test]
    fn title_includes_workspace_label() {
        assert_eq!(window_title("proyecto-ide"), "crest — proyecto-ide");
    }

    fn installed() -> Vec<String> {
        vec!["DejaVu Sans Mono".to_owned(), "JetBrains Mono".to_owned()]
    }

    #[test]
    fn installed_preferred_font_is_used() {
        let family = resolve_font_family("JetBrains Mono", &installed(), "DejaVu Sans Mono");
        assert_eq!(family, "JetBrains Mono");
    }

    #[test]
    fn missing_font_falls_back() {
        let family = resolve_font_family("Iosevka", &installed(), "DejaVu Sans Mono");
        assert_eq!(family, "DejaVu Sans Mono");
    }

    #[test]
    fn generic_or_empty_font_falls_back() {
        for preferred in ["monospace", "", "  "] {
            let family = resolve_font_family(preferred, &installed(), "DejaVu Sans Mono");
            assert_eq!(family, "DejaVu Sans Mono");
        }
    }

    #[test]
    fn default_options_are_dark_and_mono() {
        let opts = UiOptions::default();
        assert_eq!(opts.theme.name, "twocrest");
        assert_eq!(opts.font_family, DEFAULT_FONT_FAMILY);
        assert!(opts.font_size > 0.0);
    }

    /// A view that counts how many times it was drawn.
    struct Counting(std::rc::Rc<std::cell::Cell<usize>>);

    impl Render for Counting {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            div().size_full()
        }
    }

    #[gpui::test]
    fn a_frame_for_the_activity_line_reuses_the_cached_slots(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (root, cx) = cx.add_window_view(|_, cx| RootView::new(cx));
        let mut draws = Vec::new();
        let mut views = Vec::new();
        for _ in 0..4 {
            let count = std::rc::Rc::new(std::cell::Cell::new(0));
            views.push(cx.new(|_| Counting(count.clone())));
            draws.push(count);
        }
        let [timeline, panel, activity, input] = [0, 1, 2, 3].map(|ix| views[ix].clone());
        root.update(cx, |root, cx| {
            root.set_timeline(timeline.clone(), cx);
            root.set_side_panel(panel.clone(), |_| px(100.), cx);
            root.set_activity(activity.clone(), cx);
            root.set_input(input.clone(), cx);
        });
        cx.run_until_parked();
        let counts = || draws.iter().map(|d| d.get()).collect::<Vec<_>>();
        let before = counts();
        assert!(before.iter().all(|n| *n > 0), "{before:?}");
        activity.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let after = counts();
        assert_eq!(after[0], before[0], "the timeline is reused");
        assert_eq!(after[1], before[1], "the side panel is reused");
        assert!(after[2] > before[2], "the activity line draws again");
        assert!(after[3] > before[3], "the input is not cached");
        timeline.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(counts()[0] > after[0], "a notified timeline draws again");
    }

    #[test]
    fn the_split_keeps_a_quarter_of_the_width_for_each_side() {
        for width in [720.0_f32, 1024.0, 1440.0] {
            for ratio in [-1.0, 0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0, 7.0, f32::NAN] {
                let kept = clamp_split(ratio, width);
                let timeline = kept * width;
                let panel = (1.0 - kept) * width - DOC_DIVIDER_PX;
                assert!(timeline >= width * DOC_MIN_SIDE - 0.01, "{width} {ratio}");
                assert!(panel >= width * DOC_MIN_SIDE - 0.01, "{width} {ratio}");
            }
            assert_eq!(clamp_split(0.5, width), 0.5, "the middle stays");
        }
        assert_eq!(clamp_split(0.1, 0.0), DOC_MIN_SIDE, "no width yet");
    }

    /// A root with a timeline, the activity line, the input and a doc
    /// panel, each counting its draws.
    type DocRoot<'a> = (
        Entity<RootView>,
        &'a mut gpui::VisualTestContext,
        Vec<std::rc::Rc<std::cell::Cell<usize>>>,
        Entity<Counting>,
    );

    fn doc_root(cx: &mut gpui::TestAppContext) -> DocRoot<'_> {
        cx.update(gpui_component::init);
        let (root, cx) = cx.add_window_view(|_, cx| RootView::new(cx));
        let mut draws = Vec::new();
        let mut views = Vec::new();
        for _ in 0..4 {
            let count = std::rc::Rc::new(std::cell::Cell::new(0));
            views.push(cx.new(|_| Counting(count.clone())));
            draws.push(count);
        }
        root.update(cx, |root, cx| {
            root.set_timeline(views[0].clone(), cx);
            root.set_doc_panel(views[1].clone(), cx);
            root.set_activity(views[2].clone(), cx);
            root.set_input(views[3].clone(), cx);
        });
        cx.run_until_parked();
        (root, cx, draws, views[1].clone())
    }

    #[gpui::test]
    fn the_doc_panel_splits_takes_the_body_or_hides(cx: &mut gpui::TestAppContext) {
        let (root, cx, draws, _) = doc_root(cx);
        let has = |cx: &mut gpui::VisualTestContext, selector: &'static str| {
            cx.debug_bounds(selector).is_some()
        };
        assert_eq!(draws[1].get(), 0, "hidden: the panel is not drawn");
        assert!(!has(cx, "doc-split") && has(cx, "conversation"));

        root.update(cx, |root, cx| root.set_doc_view(DocView::Split, cx));
        cx.run_until_parked();
        assert!(draws[1].get() > 0);
        assert!(has(cx, "doc-split") && has(cx, "doc-divider") && has(cx, "conversation"));
        let width = cx.debug_bounds("doc-split").unwrap().size.width;
        let divider = cx.debug_bounds("doc-divider").unwrap();
        assert_eq!(divider.size.width, px(DOC_DIVIDER_PX));
        let timeline = cx.debug_bounds("conversation").unwrap().size.width;
        assert!((f32::from(timeline) - f32::from(width) / 2.0).abs() < 1.0);

        root.update(cx, |root, cx| root.set_doc_view(DocView::Alone, cx));
        cx.run_until_parked();
        assert!(has(cx, "doc-alone") && has(cx, "doc-strip") && has(cx, "prompt-row"));
        assert!(!has(cx, "conversation") && !has(cx, "doc-split"));

        root.update(cx, |root, cx| root.set_doc_view(DocView::Hidden, cx));
        cx.run_until_parked();
        assert!(has(cx, "conversation") && !has(cx, "doc-alone"));
        assert_eq!(
            root.read_with(cx, |root, _| root.doc_view()),
            DocView::Hidden
        );
    }

    #[gpui::test]
    fn dragging_the_divider_moves_the_split_within_its_limits(cx: &mut gpui::TestAppContext) {
        let (root, cx, _, _) = doc_root(cx);
        root.update(cx, |root, cx| root.set_doc_view(DocView::Split, cx));
        cx.run_until_parked();
        let area = cx.debug_bounds("doc-split").unwrap();
        let at = cx.debug_bounds("doc-divider").unwrap().center();
        cx.simulate_mouse_down(at, MouseButton::Left, gpui::Modifiers::none());
        let far_left = gpui::point(area.origin.x + px(2.), at.y);
        cx.simulate_mouse_move(far_left, MouseButton::Left, gpui::Modifiers::none());
        let low = root.read_with(cx, |root, _| root.doc_split());
        assert!((low - DOC_MIN_SIDE).abs() < 1e-4, "{low}");
        let third = gpui::point(area.origin.x + area.size.width * 0.6, at.y);
        cx.simulate_mouse_move(third, MouseButton::Left, gpui::Modifiers::none());
        let mid = root.read_with(cx, |root, _| root.doc_split());
        assert!((mid - 0.6).abs() < 0.01, "{mid}");
        cx.simulate_mouse_up(third, MouseButton::Left, gpui::Modifiers::none());
        // The ratio is remembered when the panel is hidden and shown again.
        root.update(cx, |root, cx| root.set_doc_view(DocView::Hidden, cx));
        root.update(cx, |root, cx| root.set_doc_view(DocView::Split, cx));
        cx.run_until_parked();
        let again = cx.debug_bounds("conversation").unwrap().size.width;
        let width = cx.debug_bounds("doc-split").unwrap().size.width;
        assert!((f32::from(again) / f32::from(width) - mid).abs() < 0.01);
    }

    #[gpui::test]
    fn show_chat_in_the_strip_goes_back_to_the_split(cx: &mut gpui::TestAppContext) {
        let (root, cx, _, _) = doc_root(cx);
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = seen.clone();
        cx.update(|_, cx| {
            cx.subscribe(&root, move |_, event: &DocViewChanged, _| {
                sink.borrow_mut().push(*event);
            })
            .detach();
        });
        root.update(cx, |root, cx| {
            root.set_doc_strip(Some("Rewrote the passage".into()), cx);
            root.set_doc_view(DocView::Alone, cx);
        });
        cx.run_until_parked();
        let button = cx.debug_bounds("doc-show-chat").unwrap().center();
        cx.simulate_click(button, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            root.read_with(cx, |root, _| root.doc_view()),
            DocView::Split
        );
        assert_eq!(*seen.borrow(), vec![DocViewChanged(DocView::Split)]);
    }

    #[gpui::test]
    fn a_frame_for_the_activity_line_reuses_the_cached_doc_panel(cx: &mut gpui::TestAppContext) {
        let (root, cx, draws, _) = doc_root(cx);
        root.update(cx, |root, cx| root.set_doc_view(DocView::Split, cx));
        cx.run_until_parked();
        let before: Vec<usize> = draws.iter().map(|d| d.get()).collect();
        root.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let after: Vec<usize> = draws.iter().map(|d| d.get()).collect();
        assert_eq!(after[0], before[0], "the timeline is reused");
        assert_eq!(after[1], before[1], "the doc panel is reused");
    }

    #[test]
    fn title_without_label_is_app_name() {
        assert_eq!(window_title(""), "crest");
    }
}
