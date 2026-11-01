//! Shape tokens of the theme in use as GPUI lengths.
//!
//! Each token is a size in pixels at a 16px rem
//! ([`domain::theme::SHAPE`]); it becomes rems here, so it scales with the
//! window's rem size like GPUI's own `gap_2` or `text_sm`. The defaults
//! are those same GPUI sizes:
//!
//! | token | default | GPUI |
//! |---|---|---|
//! | [`Space::Xxs`] … [`Space::Xl`] | 2, 4, 6, 8, 12, 16 | `gap_0p5`, `gap_1`, `gap_1p5`, `gap_2`, `gap_3`, `gap_4` |
//! | [`Radius::Sm`], [`Radius::Md`], [`Radius::Lg`] | 4, 6, 8 | `rounded_sm`, `rounded_md`, `rounded_lg` |
//! | [`TextRole::Caption`] … [`TextRole::Heading`] | 12, 14, 16, 18 | `text_xs`, `text_sm`, `text_base`, `text_lg` |

use gpui::{App, Rems, Styled, rems};

use crate::theme::crest_theme;

/// The default rem size the token values are written for.
const REM_PX: f32 = 16.0;

/// A step of the spacing scale (`space.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Space {
    /// `space.xxs`: hairline gaps (an icon and its label).
    Xxs,
    /// `space.xs`: gaps in a row of chips or a toolbar.
    Xs,
    /// `space.sm`: padding of chips and small buttons.
    Sm,
    /// `space.md`: gaps between the parts of a block.
    Md,
    /// `space.lg`: padding of cards and dialogs.
    Lg,
    /// `space.xl`: gaps between sections.
    Xl,
}

impl Space {
    /// Its token path.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Xxs => "space.xxs",
            Self::Xs => "space.xs",
            Self::Sm => "space.sm",
            Self::Md => "space.md",
            Self::Lg => "space.lg",
            Self::Xl => "space.xl",
        }
    }

    /// Its length in the theme in use.
    #[must_use]
    pub fn get(self, cx: &App) -> Rems {
        size(self.path(), cx)
    }
}

/// A corner radius (`radius.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Radius {
    /// `radius.sm`: chips, badges, small buttons.
    Sm,
    /// `radius.md`: cards, inputs, popovers.
    Md,
    /// `radius.lg`: dialogs and large panels.
    Lg,
}

impl Radius {
    /// Its token path.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Sm => "radius.sm",
            Self::Md => "radius.md",
            Self::Lg => "radius.lg",
        }
    }

    /// Its length in the theme in use.
    #[must_use]
    pub fn get(self, cx: &App) -> Rems {
        size(self.path(), cx)
    }
}

/// What a piece of text is, which sets its size (`text.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextRole {
    /// `text.caption`: captions, readouts, chip labels.
    Caption,
    /// `text.body`: body text of blocks and lists.
    Body,
    /// `text.title`: block and section titles.
    Title,
    /// `text.heading`: dialog and page headings.
    Heading,
}

impl TextRole {
    /// Its token path.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Caption => "text.caption",
            Self::Body => "text.body",
            Self::Title => "text.title",
            Self::Heading => "text.heading",
        }
    }

    /// Its font size in the theme in use.
    #[must_use]
    pub fn get(self, cx: &App) -> Rems {
        size(self.path(), cx)
    }
}

/// A shape token of the theme in use, in rems.
#[must_use]
pub fn size(path: &str, cx: &App) -> Rems {
    to_rems(crest_theme(cx).size(path))
}

fn to_rems(px: u16) -> Rems {
    rems(f32::from(px) / REM_PX)
}

/// Shape tokens on any styled element: `div().tk_gap(Space::Md, cx)`.
pub trait TokenStyled: Styled + Sized {
    /// Gap between children.
    #[must_use]
    fn tk_gap(self, space: Space, cx: &App) -> Self {
        self.gap(space.get(cx))
    }

    /// Padding on every side.
    #[must_use]
    fn tk_p(self, space: Space, cx: &App) -> Self {
        self.p(space.get(cx))
    }

    /// Left and right padding.
    #[must_use]
    fn tk_px(self, space: Space, cx: &App) -> Self {
        self.px(space.get(cx))
    }

    /// Top and bottom padding.
    #[must_use]
    fn tk_py(self, space: Space, cx: &App) -> Self {
        self.py(space.get(cx))
    }

    /// Corner radius on every corner.
    #[must_use]
    fn tk_rounded(self, radius: Radius, cx: &App) -> Self {
        self.rounded(radius.get(cx))
    }

    /// Font size of a text role.
    #[must_use]
    fn tk_text(self, role: TextRole, cx: &App) -> Self {
        self.text_size(role.get(cx))
    }
}

impl<T: Styled> TokenStyled for T {}

#[cfg(test)]
mod tests {
    use domain::theme::{SHAPE, shape_spec};

    use super::*;

    const ALL: &[&str] = &[
        Space::Xxs.path(),
        Space::Xs.path(),
        Space::Sm.path(),
        Space::Md.path(),
        Space::Lg.path(),
        Space::Xl.path(),
        Radius::Sm.path(),
        Radius::Md.path(),
        Radius::Lg.path(),
        TextRole::Caption.path(),
        TextRole::Body.path(),
        TextRole::Title.path(),
        TextRole::Heading.path(),
    ];

    #[test]
    fn every_token_is_a_shape_token_and_back() {
        for path in ALL {
            assert!(shape_spec(path).is_some(), "{path} is not in SHAPE");
        }
        for spec in SHAPE {
            assert!(ALL.contains(&spec.path), "{} has no enum value", spec.path);
        }
    }

    #[test]
    fn defaults_are_the_gpui_sizes() {
        // gap_2 is 0.5rem, rounded_sm 0.25rem, text_xs 0.75rem.
        let at = |path| to_rems(shape_spec(path).unwrap().default);
        assert_eq!(at("space.md"), rems(0.5));
        assert_eq!(at("radius.sm"), rems(0.25));
        assert_eq!(at("text.caption"), rems(0.75));
    }
}
