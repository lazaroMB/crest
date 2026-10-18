//! Application entry point: boots GPUI, opens the main window and owns the
//! app-wide key bindings.
//!
//! The window opens in fullscreen (`WindowBounds::Fullscreen`), which GPUI
//! implements on macOS, X11 and Wayland. A compositor that refuses the
//! request leaves the window at its restore bounds, centered on the primary
//! display.

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, SharedString,
    Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, px, size,
};
use gpui_component::{ActiveTheme as _, Root, Theme, v_flex};

actions!(crest, [Quit]);

/// Size the window returns to when it leaves fullscreen.
const RESTORE_SIZE: (f32, f32) = (1280.0, 800.0);

/// Monospace family used when the options name none.
pub const DEFAULT_FONT_FAMILY: &str = "JetBrains Mono";
/// Base font size in pixels.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Color scheme of the UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemeMode {
    /// Dark theme (default).
    #[default]
    Dark,
    /// Light theme.
    Light,
}

impl From<ThemeMode> for gpui_component::ThemeMode {
    fn from(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::Dark,
            ThemeMode::Light => Self::Light,
        }
    }
}

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
    /// Color scheme.
    pub theme: ThemeMode,
}

impl Default for UiOptions {
    fn default() -> Self {
        Self {
            workspace_label: String::new(),
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            font_size: DEFAULT_FONT_SIZE,
            theme: ThemeMode::default(),
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

/// Boots GPUI and runs the event loop until the user quits.
///
/// On Linux this returns once the app quits; on macOS the platform may end
/// the process from inside the event loop instead.
pub fn run(opts: UiOptions) -> Result<(), UiError> {
    let title = window_title(&opts.workspace_label);
    let failure = std::rc::Rc::new(std::cell::RefCell::new(None));
    let failure_in_app = failure.clone();

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        apply_theme(&opts, cx);
        bind_app_keys(cx);
        if let Err(err) = open_main_window(title, cx) {
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
    Theme::change(opts.theme, None, cx);
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

fn bind_app_keys(cx: &mut App) {
    // `secondary` is Cmd on macOS and Ctrl elsewhere.
    cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    // macOS keeps apps alive without windows; crest has a single window.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

fn open_main_window(title: String, cx: &mut App) -> Result<(), UiError> {
    let restore = Bounds::centered(None, size(px(RESTORE_SIZE.0), px(RESTORE_SIZE.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Fullscreen(restore)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title)),
            ..Default::default()
        }),
        app_id: Some("crest".to_owned()),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        let view = cx.new(RootView::new);
        window.focus(&view.focus_handle(cx), cx);
        cx.new(|cx| Root::new(view, window, cx))
    })
    .map(|_| ())
    .map_err(|err| UiError::OpenWindow(err.to_string()))
}

/// Root view of the main window: timeline on top (grows), prompt input
/// below it and the status bar at the bottom.
///
/// Each area is a slot that later views fill (`set_timeline`, `set_input`,
/// `set_status`); an empty slot renders a placeholder box.
pub struct RootView {
    focus_handle: FocusHandle,
    timeline: Option<AnyView>,
    input: Option<AnyView>,
    status: Option<AnyView>,
}

impl RootView {
    /// Creates the root view with every slot empty.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            timeline: None,
            input: None,
            status: None,
        }
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
        v_flex()
            .id("crest-root")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(div().id("timeline").flex_1().min_h_0().p_2().child(slot(
                &self.timeline,
                "timeline",
                cx,
            )))
            .child(
                div()
                    .min_h(px(64.0))
                    .p_2()
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
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(opts.theme, ThemeMode::Dark);
        assert_eq!(opts.font_family, DEFAULT_FONT_FAMILY);
        assert!(opts.font_size > 0.0);
    }

    #[test]
    fn title_without_label_is_app_name() {
        assert_eq!(window_title(""), "crest");
    }
}
