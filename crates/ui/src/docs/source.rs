//! The source view: the doc's markdown in a multi-line editor
//! (gpui-component's `Textarea`, the engine the prompt uses) with soft wrap
//! and no line numbers, under the formatting toolbar. It edits the same text
//! as the page (the page, a live-preview editor, is the primary one); a
//! selection here floats the same Copy / "Ask agent" bar as on the page.

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
};
use gpui_component::input::Textarea;
use gpui_component::{ActiveTheme as _, v_flex};

use super::DocPanel;

/// The source editor under its toolbar.
pub fn render(panel: &DocPanel, cx: &mut Context<DocPanel>) -> AnyElement {
    let mono = cx.theme().mono_font_family.clone();
    let bg = cx.theme().background;
    v_flex()
        .id("doc-source")
        .debug_selector(|| "doc-source".to_owned())
        .size_full()
        .child(super::toolbar::render(panel, cx))
        .child(
            gpui::div()
                .id("doc-source-page")
                .flex_1()
                .min_h_0()
                .bg(bg)
                // A selection in the textarea floats the selection bar.
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, event, window, cx| this.source_mouse_up(event, window, cx)),
                )
                // The mock's padding (16 px top, 24 px sides) and line height.
                .pt_4()
                .pb_8()
                .px_6()
                .line_height(gpui::relative(1.55))
                .child(
                    Textarea::new(&panel.source)
                        .h_full()
                        .appearance(false)
                        .bordered(false)
                        .font_family(mono),
                ),
        )
        .into_any_element()
}

/// The stored file name for a picked picture: lower case, anything but
/// letters, digits and dots turned into dashes, and a suffix (`-2`, `-3`...)
/// when `taken` already holds it.
#[must_use]
pub fn asset_name(file_name: &str, taken: &[String]) -> String {
    let mut clean = String::new();
    for c in file_name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            clean.push(c);
        } else if !clean.ends_with('-') {
            clean.push('-');
        }
    }
    let clean = clean.trim_matches(|c| c == '-' || c == '.').to_owned();
    let clean = if clean.is_empty() {
        "image.png".to_owned()
    } else {
        clean
    };
    if !taken.contains(&clean) {
        return clean;
    }
    let (stem, ext) = match clean.rfind('.') {
        Some(dot) if dot > 0 => (&clean[..dot], &clean[dot..]),
        _ => (clean.as_str(), ""),
    };
    (2..)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|name| !taken.contains(name))
        .unwrap_or(clean)
}

/// The text of a file name without its extension, as the alt text.
#[must_use]
pub fn file_stem_of(file_name: &str) -> String {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    match base.rfind('.') {
        Some(dot) if dot > 0 => base[..dot].to_owned(),
        _ => base.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_are_clean_and_unique() {
        assert_eq!(asset_name("My Shot (1).PNG", &[]), "my-shot-1-.png");
        let taken = vec!["a.png".to_owned(), "a-2.png".to_owned()];
        assert_eq!(asset_name("a.png", &taken), "a-3.png");
        assert_eq!(asset_name("???", &[]), "image.png");
    }

    #[test]
    fn stems_drop_the_directory_and_extension() {
        assert_eq!(file_stem_of("/x/y/shot one.png"), "shot one");
        assert_eq!(file_stem_of("noext"), "noext");
    }
}
