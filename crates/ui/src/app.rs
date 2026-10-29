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

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, px, size,
};
use gpui_component::{ActiveTheme as _, Root, Theme, h_flex, v_flex};

pub use application::window::WindowMode;

actions!(crest, [Quit, ToggleFullscreen]);

/// Monospace family used when the options name none.
pub const DEFAULT_FONT_FAMILY: &str = "JetBrains Mono";
/// Base font size in pixels.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

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

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
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
pub struct RootView {
    focus_handle: FocusHandle,
    timeline: Option<AnyView>,
    input: Option<AnyView>,
    activity: Option<AnyView>,
    status: Option<AnyView>,
    side_panel: Option<AnyView>,
    overlays: Vec<AnyView>,
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
            overlays: Vec::new(),
        }
    }

    /// Fills the side panel on the left (the workspace tree).
    pub fn set_side_panel(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.side_panel = Some(view.into());
        cx.notify();
    }

    /// Adds a layer above the window (a picker, the approval dialog).
    /// Later overlays are drawn above earlier ones.
    pub fn push_overlay(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.overlays.push(view.into());
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

/// Slot content: the view when present, else placeholder text.
fn slot(view: &Option<AnyView>, placeholder: &'static str, cx: &App) -> AnyElement {
    match view {
        Some(view) => view.clone().into_any_element(),
        None => div()
            .text_color(cx.theme().muted_foreground)
            .child(placeholder)
            .into_any_element(),
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let status_bg = theme.title_bar;
        let session = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(div().id("timeline").flex_1().min_h_0().p_2().child(slot(
                &self.timeline,
                "timeline",
                cx,
            )))
            .children(self.activity.clone())
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .border_t_1()
                    .border_color(border)
                    .child(slot(&self.input, "input", cx)),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(border)
                    .bg(status_bg)
                    .child(slot(&self.status, "status", cx)),
            );
        div()
            .id("crest-root")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .child(
                h_flex()
                    .size_full()
                    .children(self.side_panel.clone())
                    .child(session),
            )
            .children(self.overlays.iter().cloned())
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

    #[test]
    fn title_without_label_is_app_name() {
        assert_eq!(window_title(""), "crest");
    }
}
