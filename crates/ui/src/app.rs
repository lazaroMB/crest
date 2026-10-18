//! Application entry point: boots GPUI, opens the main window and owns the
//! app-wide key bindings.
//!
//! The window opens in fullscreen (`WindowBounds::Fullscreen`), which GPUI
//! implements on macOS, X11 and Wayland. A compositor that refuses the
//! request leaves the window at its restore bounds, centered on the primary
//! display.

use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, KeyBinding, ParentElement as _, Render,
    SharedString, Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    px, size,
};
use gpui_component::Root;

actions!(crest, [Quit]);

/// Size the window returns to when it leaves fullscreen.
const RESTORE_SIZE: (f32, f32) = (1280.0, 800.0);

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
pub fn run(workspace_label: &str) -> Result<(), UiError> {
    let title = window_title(workspace_label);
    let failure = std::rc::Rc::new(std::cell::RefCell::new(None));
    let failure_in_app = failure.clone();

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
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
        let view = cx.new(|_| RootView);
        cx.new(|cx| Root::new(view, window, cx))
    })
    .map(|_| ())
    .map_err(|err| UiError::OpenWindow(err.to_string()))
}

/// Root view of the main window.
pub struct RootView;

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child("crest")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_includes_workspace_label() {
        assert_eq!(window_title("proyecto-ide"), "crest — proyecto-ide");
    }

    #[test]
    fn title_without_label_is_app_name() {
        assert_eq!(window_title(""), "crest");
    }
}
