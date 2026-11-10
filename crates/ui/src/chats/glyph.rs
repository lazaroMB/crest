//! The icons of the sidebar's rows and menus. The assets test
//! (`assets.rs`) loads every one of them.

use gpui_component::Icon;

/// An icon of a row, a button or a menu row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Glyph {
    Plus,
    Folder,
    Branch,
    Archive,
    Restore,
    Trash,
    Pin,
    More,
}

impl Glyph {
    pub(super) fn icon(self) -> Icon {
        match self {
            Self::Plus => Icon::new(gpui_component::IconName::Plus),
            Self::Folder => Icon::new(gpui_component::IconName::Folder),
            Self::More => Icon::new(gpui_component::IconName::Ellipsis),
            Self::Branch => Icon::new(gpui_kit_assets::IconName::GitBranch),
            Self::Archive => Icon::new(gpui_kit_assets::IconName::Archive),
            Self::Restore => Icon::new(gpui_kit_assets::IconName::ArchiveRestore),
            Self::Trash => Icon::new(gpui_kit_assets::IconName::Trash),
            Self::Pin => Icon::new(gpui_kit_assets::IconName::Pin),
        }
    }
}

/// A small icon button at the end of a row: its own selector, a fill under
/// the pointer, and `click`.
pub(super) fn icon_button(
    selector: String,
    glyph: Glyph,
    click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    cx: &gpui::App,
) -> gpui::Stateful<gpui::Div> {
    use gpui::Styled as _;
    use gpui::{InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _};
    use gpui_component::{ActiveTheme as _, Sizable as _};
    let active = cx.theme().list_active;
    gpui::div()
        .id(gpui::SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .px_1()
        .rounded_sm()
        .cursor_pointer()
        .hover(move |style| style.bg(active))
        .on_click(click)
        .child(glyph.icon().small())
}
