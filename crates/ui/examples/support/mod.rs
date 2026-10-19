//! Boot code shared by the examples: theme, font, Cmd/Ctrl+Q and a window
//! whose content the example builds.
//!
//! `ui::app::run` opens the placeholder layout only, so the examples that
//! fill the slots repeat its setup here.

#![allow(dead_code)]

use gpui::{
    AnyView, App, AppContext as _, Bounds, KeyBinding, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, px, size,
};
use gpui_component::{Root, Theme, ThemeMode};

/// Runs GPUI with crest's dark theme and opens one window whose root view
/// `build` returns, focusing it.
pub fn run(title: &str, build: impl FnOnce(&mut Window, &mut App) -> AnyView + 'static) {
    run_sized(title, (1280.0, 800.0), build);
}

/// [`run`] with a window of `width × height` logical pixels.
pub fn run_sized(
    title: &str,
    (width, height): (f32, f32),
    build: impl FnOnce(&mut Window, &mut App) -> AnyView + 'static,
) {
    let title = SharedString::from(title.to_owned());
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        apply_theme(cx);
        cx.bind_keys([KeyBinding::new("secondary-q", ui::app::Quit, None)]);
        cx.on_action(|_: &ui::app::Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title),
                ..Default::default()
            }),
            app_id: Some("crest".to_owned()),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            let view = build(window, cx);
            cx.new(|cx| Root::new(view, window, cx))
        });
        if let Err(err) = opened {
            eprintln!("failed to open the window: {err}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}

fn apply_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let installed = cx.text_system().all_font_names();
    let fallback = Theme::global(cx).mono_font_family.clone();
    let family = SharedString::from(ui::app::resolve_font_family(
        ui::app::DEFAULT_FONT_FAMILY,
        &installed,
        &fallback,
    ));
    Theme::update(cx, |theme| {
        theme.font_family = family.clone();
        theme.mono_font_family = family;
        theme.font_size = px(ui::app::DEFAULT_FONT_SIZE);
        theme.mono_font_size = px(ui::app::DEFAULT_FONT_SIZE);
    });
}
