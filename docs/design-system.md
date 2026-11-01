# Design system

crest's UI is built from **tokens** (named colors and sizes a theme sets)
and the shared components of `crates/ui/src/kit`. A view or block should
not pick a color or a size by itself: it asks for a token, so every theme
reaches it.

## Tokens

- **Colors**: `crate::theme::token(cx, "accent.bg")`. Every color token is
  listed in [themes](themes.md#every-token).
- **Shape**: spacing, radii and text sizes, from `ui::kit::tokens`:

  ```rust
  use ui::kit::{Radius, Space, TextRole, TokenStyled as _};

  div()
      .tk_gap(Space::Md, cx)
      .tk_px(Space::Sm, cx)
      .tk_rounded(Radius::Sm, cx)
      .tk_text(TextRole::Caption, cx)
  ```

  The defaults are GPUI's own sizes (`gap_2` is `Space::Md`, `rounded_sm`
  is `Radius::Sm`, `text_xs` is `TextRole::Caption`), so moving a view to
  tokens does not change how it looks with the built-in themes. The list
  of shape tokens and how a theme sets them is in
  [themes](themes.md#shape).

Layout sizes that are not part of the visual language (a column width, a
minimum chart height) stay as plain `px(..)`.
