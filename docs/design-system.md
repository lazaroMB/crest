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

## Components (`ui::kit`)

Each one is a function that returns a GPUI element, so the caller can
still chain an id, a debug selector or a click handler.

| Component | Use it for | Notes |
| --- | --- | --- |
| `header(cx)` | The header row of a block | Children in a line, `Space::Md` apart. |
| `title(text)` | The title in a header | Semibold, one line, ellipsis, takes the free room. |
| `caption(text, cx)` | A count, a hint, what the block shows | Caption size, muted. |
| `toolbar(cx)` | A row of buttons and readouts | Caption size, `Space::Xs` apart. |
| `tool_button(id, label, cx)` | A toolbar action (zoom, fit, copy) | Accent colors; `id` is also its debug selector. Mark an active one with `.border_1()`. |
| `quiet_button(id, label, cx)` | A toolbar action that should not draw the eye | Secondary (button) colors. |
| `readout(text, cx)` | A value next to the buttons (zoom level, a count) | Muted fill. |
| `chip(label, tint, ChipKind::Soft \| Outline, cx)` | A label in a color: a status, a warning, a verdict | Soft: faint fill; Outline: border. |
| `filter_chip(label, tint, on, cx)` | Filtering a block by a colored category (severity, test outcome) | Keeps its color when off; fills when on. |
| `toggle_chip(label, on, cx)` | Picking an option or a mode (metric, color key, image mode) | Accent when on, muted when off. |

```rust
use crate::kit;

kit::header(cx)
    .child(kit::title(title))
    .child(kit::caption(format!("{} items", n), cx))
    .child(
        kit::toolbar(cx)
            .child(kit::readout(zoom_text, cx))
            .child(kit::tool_button("graph-fit", "fit", cx).on_click(cx.listener(|this, _, _, cx| this.fit(cx)))),
    )
```

On top of these, gpui-component's own widgets (`Button`, `Input`,
`Checkbox`, scrollbars...) follow the theme too: `crate::theme` feeds
them the color tokens and `radius.md` / `radius.lg`.

## Rules

Two tests in `crates/ui/src/kit/mod.rs` (`kit::lint`) check the sources
outside `kit/` and `theme.rs` (test code excluded):

- **No color literals** (`rgb(0x..)`, `rgba(0x..)`, `hsla(<number>..)`).
  Use a token. A color that must not follow the theme (dark labels on the
  flame graph's bright frames) carries `// design: fixed color` on its
  line or the one above.
- **No hand-made kit parts**: a header title built from
  `whitespace_nowrap + text_ellipsis + SEMIBOLD`, or a button painted in
  `accent.bg` by hand. Use `kit::title` and `kit::tool_button` /
  `kit::toggle_chip`.

## Adding a component

1. Look for the pattern repeated in at least two views; the component is
   what they share, not the union of their options.
2. Put it in `crates/ui/src/kit/`, sized with the shape tokens and colored
   with theme tokens, returning an element the caller can extend.
3. Move every copy to it in the same change, and list it in the table
   above.
4. If a copy is easy to write by hand again, add its pattern to
   `kit::lint`.
