# Themes and the window

crest's colors come from a **theme**: a Lua file that names the colors of the window, element by element and state by state, and the colors of code. This guide covers using a theme, writing your own, porting one from Vim or Neovim, and how the window opens (`app.opt.window`).

## Using a theme

| Theme | Where it comes from |
| --- | --- |
| `twocrest` (also `dark`) | Built in, the default: the palette of [twocrest.mx](https://twocrest.mx), ink and paper with a coral accent. |
| `twocrest-light` (also `light`) | Built in, the light variant. |
| `<name>` | The file `themes/<name>.lua` in crest's config folder: `~/.config/crest/themes/` on Linux and macOS (`$XDG_CONFIG_HOME/crest/themes/` when it is set), `%APPDATA%\crest\themes\` on Windows. |
| `<name>` | A theme defined in `init.lua` or a plugin with `app.theme.define{...}` (see [below](#overriding-a-theme-from-initlua)). |

The installers (`install.sh`, `install.ps1` and the Windows setup) copy 14 themes ported from the most used Neovim colorschemes into that folder, and a commented `init.lua` next to it, but only the files that are missing: what you edited is never replaced, and a reinstall brings back a theme you deleted. Homebrew does not write in your home, so it prints the two `cp -n` commands that do it. When you build crest yourself, copy [`examples/themes/`](../examples/themes/) by hand. The themes are catppuccin (mocha, latte), tokyonight (night, day), kanagawa-wave, rose-pine (and dawn), gruvbox (dark, light), nightfox, onedark, dracula, nord and everforest-dark; [`examples/themes/README.md`](../examples/themes/README.md) lists their sources and licenses.

Two ways to choose one:

- **`/theme`** opens a popup over every theme, with a swatch of each. `Up`/`Down` show each theme in the window as you move; `Enter` saves the one you are on, `Esc` goes back to the theme in use. `/theme nord` saves one directly. The pick is saved in `preferences.json` (in crest's data folder) and used from then on.
- **`app.opt.theme = "nord"`** in `init.lua` fixes it. It wins over the pick of `/theme` when crest starts (the popup tells you so when you save), and a workspace `.crest/init.lua` may set its own.

A theme that does not exist or fails to load is a warning at startup, and crest uses `twocrest`. Theme files are read when they are needed: a file you edit shows its changes the next time you open the `/theme` popup.

## Your first theme

A theme file returns a table. Only the colors you care about are needed: everything else is derived from them. This is a complete theme:

```lua
-- ~/.config/crest/themes/midnight.lua
return {
  appearance = "dark",
  ui = {
    background = "#14161b",
    foreground = "#d8dee9",
    accent = "#88c0d0",
  },
}
```

`/theme midnight` shows it. Borders, cards, hover and selected rows, the text on the accent, links, the caret and the selection all come from these three colors, and the terminal colors (and with them errors, warnings, diffs and code) fall back to TwoCrest's.

To change only some colors of an existing theme, start from it with `extends`:

```lua
-- ~/.config/crest/themes/nord-orange.lua
return {
  extends = "nord",
  ui = {
    accent = "#d08770",
    list = { selected = { border = "#d08770" } },
  },
}
```

Everything `nord` sets stays; what you set goes over it. A theme may extend a theme that extends another, up to 8 deep.

## The file

```lua
local p = { bg = "#1a1b26", fg = "#c0caf5", blue = "#7aa2f7", green = "#9ece6a" }

return {
  name = "tokyonight-night", -- optional; the file name is what /theme and app.opt.theme use
  appearance = "dark",       -- "dark" or "light"; picks the defaults for what is unset
  extends = nil,             -- the name of a theme to start from
  palette = p,               -- named colors: other values may use these names
  ui = { ... },              -- the colors of the window
  syntax = { ... },          -- the colors of code
}
```

Every field is optional. The file runs in the same sandbox as `init.lua` (no files, no network, an instruction and memory limit), so it may compute colors, but whatever it defines is gone once it returns. Files whose name starts with `_` (such as `_template.lua`) are not listed.

### Colors

A color is `"#rrggbb"`, `"#rrggbbaa"` (alpha last: `"#7aa2f733"` is 20 % opaque) or `"#rgb"`, or the name of a `palette` entry (`"blue"`). Palette entries must be hex; a theme that `extends` another can use the palette of its base.

While the file runs, a global `color` table derives colors. Its functions take and return hex strings, so pass the palette's values (`p.blue`), not their names:

| Function | Result |
| --- | --- |
| `color.alpha(c, a)` | `c` with opacity `a`, from 0 to 1. |
| `color.mix(a, b, t)` | `a` moved towards `b` by `t`, from 0 to 1. |
| `color.lighten(c, x)` | `c` towards white by `x`. |
| `color.darken(c, x)` | `c` towards black by `x`. |

```lua
ui = {
  selection = color.alpha(p.blue, 0.3),
  list = { hover = color.mix(p.bg, p.fg, 0.08) },
}
```

`color` is also available in `init.lua`, for [`app.opt.colors`](#overriding-a-theme-from-initlua).

### Elements and states

`ui` sets **tokens**. A token is a path `element[.state].attribute`:

- the **element** is a part of the window: `button`, `input`, `list`, `tab`, `popover`, `status_bar`...;
- the **state** is optional: `hover` (under the mouse), `active` (pressed), `focused`, `selected` and `disabled`;
- the **attribute** is `bg` (background), `fg` (text) or `border`.

Nested tables join with dots, so these three lines set the same token:

```lua
ui = { list = { selected = { border = "#7aa2f7" } } }
ui = { ["list.selected.border"] = "#7aa2f7" }
ui = { list = { ["selected.border"] = "#7aa2f7" } }
```

A plain color on a path that is not a token goes to its `bg`, or to its `fg` when the element has no `bg`: `accent = "blue"` sets `accent.bg`, `link = "blue"` sets `link.fg`, `list = { hover = "#292e42" }` sets `list.hover.bg`. `chart = { "#a", "#b", "#c" }` sets `chart.1`, `chart.2` and `chart.3`.

Not every element has every state: the table below lists every token crest knows. A path that is not in it is reported (`ui.list.hovr: unknown token`) and skipped.

### Every token

The tokens are listed in the order they are derived: a token you leave out takes the value in the last column, computed from tokens above it (after your own values and those of `extends` are applied). The three core tokens and the terminal colors fall back to TwoCrest.

| Token | What it paints | When unset |
| --- | --- | --- |
| `background` | the window background | `#0b0b0c` dark, `#f4f1ec` light |
| `foreground` | the default text | `#f4f1ec` dark, `#3d3a35` light |
| `accent.bg` | primary buttons, focus, highlights | `#d9604a` dark, `#ae3a25` light |
| `ansi.red` | red | `#f0524a` dark, `#c0392b` light |
| `ansi.green` | green | `#8fc27a` dark, `#3f7d3a` light |
| `ansi.yellow` | yellow | `#ffae42` dark, `#b8740c` light |
| `ansi.blue` | blue | `#7aa2d6` dark, `#3a6ea5` light |
| `ansi.magenta` | magenta | `#d9604a` dark, `#ae3a25` light |
| `ansi.cyan` | cyan | `#d6d2cb` dark, `#3d3a35` light |
| `border` | borders and rules | `background` 15 % towards `foreground` |
| `muted.bg` | cards, chips, code | `background` 6 % towards `foreground` |
| `muted.fg` | secondary text: hints, labels, timestamps | `foreground` 42 % towards `background` |
| `selection` | selected text | `accent.bg` at 30 % |
| `caret` | the text cursor | `accent.bg` |
| `overlay` | the shade behind dialogs | `background` at 70 % |
| `canvas.bg` | the background of diagrams and 3D views | `background` |
| `accent.fg` | text on the accent | `background` or `foreground`, whichever reads better on `accent.bg` |
| `accent.hover.bg` | the accent under the mouse | `accent.bg` 15 % towards `foreground` |
| `accent.active.bg` | the accent while pressed | `accent.bg` 25 % towards `background` |
| `button.bg` | buttons | `muted.bg` |
| `button.fg` | button labels | `foreground` |
| `button.hover.bg` | a button under the mouse; menu items too | `button.bg` 8 % towards `foreground` |
| `button.hover.fg` | a button label under the mouse | `foreground` |
| `button.active.bg` | a button while pressed | `button.bg` 15 % towards `foreground` |
| `button.disabled.fg` | the label of a disabled button | `foreground` at 40 % |
| `input.bg` | the prompt and text fields | `background` |
| `input.fg` | typed text | `foreground` |
| `input.border` | text field borders | `border` |
| `input.focused.border` | the border (ring) of the focused field | `accent.bg` |
| `input.disabled.fg` | text of a disabled field and placeholders | `foreground` at 40 % |
| `list.bg` | lists and tables | `background` |
| `list.even.bg` | every other row | `background` 2 % towards `foreground` |
| `list.header.bg` | table headers | `background` 4 % towards `foreground` |
| `list.header.fg` | table header text | `foreground` |
| `list.hover.bg` | a row under the mouse | `muted.bg` |
| `list.selected.bg` | the selected row or timeline block | `accent.bg` at 15 % |
| `list.selected.fg` | text of the selected row | `foreground` |
| `list.selected.border` | the edge of the selected row or block | `accent.bg` |
| `list.focused.border` | the edge of the focused timeline block | `accent.bg` |
| `popover.bg` | popups, menus, dialogs | `background` 3 % towards `foreground` |
| `popover.fg` | popup text | `foreground` |
| `popover.border` | popup borders | `border` |
| `title_bar.bg` | the title bar | `background` |
| `title_bar.border` | the title bar border | `border` |
| `status_bar.bg` | the status bar | `popover.bg` |
| `status_bar.fg` | status bar text | `muted.fg` |
| `status_bar.border` | the status bar border | `border` |
| `sidebar.bg` | the side panel | `popover.bg` |
| `sidebar.fg` | side panel text | `foreground` |
| `sidebar.border` | the side panel border | `border` |
| `sidebar.hover.bg` | a side panel item under the mouse | `muted.bg` |
| `sidebar.hover.fg` | its text | `foreground` |
| `sidebar.selected.bg` | the selected side panel item | `accent.bg` |
| `sidebar.selected.fg` | its text | `accent.fg` |
| `tab.bar.bg` | the tab bar | `popover.bg` |
| `tab.bg` | tabs | `background` |
| `tab.fg` | tab labels | `muted.fg` |
| `tab.selected.bg` | the current tab | `muted.bg` |
| `tab.selected.fg` | the current tab label | `foreground` |
| `scrollbar.bg` | the scrollbar track | transparent |
| `scrollbar.thumb.bg` | the scrollbar thumb | `border` 10 % towards `foreground` |
| `scrollbar.thumb.hover.bg` | the thumb under the mouse | `muted.fg` |
| `link.fg` | links | `accent.bg` |
| `link.hover.fg` | a link under the mouse | `accent.hover.bg` |
| `link.active.fg` | a link while pressed | `accent.active.bg` |
| `info.bg` | information: notices, running tools | `ansi.blue` |
| `info.fg` | text on it | `background` or `foreground`, whichever reads better on `info.bg` |
| `success.bg` | success: finished tools, approvals | `ansi.green` |
| `success.fg` | text on it | `background` or `foreground`, whichever reads better on `success.bg` |
| `warning.bg` | warnings | `ansi.yellow` |
| `warning.fg` | text on it | `background` or `foreground`, whichever reads better on `warning.bg` |
| `error.bg` | errors, failed tools, denials | `ansi.red` |
| `error.fg` | text on it | `background` or `foreground`, whichever reads better on `error.bg` |
| `diff.added.fg` | added lines | `success.bg` |
| `diff.added.bg` | the background of added lines | `diff.added.fg` at 12 % |
| `diff.removed.fg` | removed lines | `error.bg` |
| `diff.removed.bg` | the background of removed lines | `diff.removed.fg` at 12 % |
| `diff.hunk.fg` | hunk headers (`@@ ... @@`) | `info.bg` |
| `diff.header.fg` | file headers | `muted.fg` |
| `editor.active_line.bg` | the line of the cursor in code views | `muted.bg` |
| `editor.line_number.fg` | line numbers | `muted.fg` |
| `editor.active_line_number.fg` | the line number of the cursor | `foreground` |
| `chart.1` | the first series | `accent.bg` |
| `chart.2` | the second series | `warning.bg` |
| `chart.3` | the third series | `info.bg` |
| `chart.4` | the fourth series | `success.bg` |
| `chart.5` | the fifth series | `ansi.magenta` |
| `chart.grid` | chart grid lines | `border` |

Some tokens have no place in the window yet beyond the ones listed in their description; `button.disabled.fg` and `input.disabled.fg`, for instance, paint crest's own disabled controls, while the buttons of the component library compute their disabled look themselves.

### Syntax

`syntax` colors code: the code blocks of the timeline, file previews and the code editor views. Each entry is a color, or a table with `fg`, `italic` and `bold`, each optional:

```lua
syntax = {
  keyword = "magenta",
  comment = { fg = "comment", italic = true },
  ["function"] = { fg = "blue", bold = true }, -- `function` is a Lua keyword: quote it
  ["string.escape"] = "cyan",
}
```

Names are the highlight names of tree-sitter queries. A dotted name you leave out takes after its prefix when you set the prefix (`string.escape` follows `string`, italic and bold included). Underline is not supported. Unset, each name takes:

| Name | When unset |
| --- | --- |
| `comment` | `muted.fg`, italic |
| `comment.doc` | `muted.fg`, italic |
| `keyword` | `ansi.magenta` |
| `string` | `ansi.green` |
| `string.escape` | `ansi.cyan` |
| `string.regex` | `ansi.cyan` |
| `string.special` | `ansi.cyan` |
| `string.special.symbol` | `ansi.cyan` |
| `number` | `ansi.yellow` |
| `boolean` | `ansi.yellow` |
| `constant` | `ansi.yellow` |
| `function` | `ansi.blue` |
| `constructor` | `ansi.blue` |
| `type` | `ansi.cyan` |
| `enum` | `ansi.cyan` |
| `variant` | `ansi.cyan` |
| `variable` | `foreground` |
| `variable.special` | `ansi.red` |
| `property` | `foreground` |
| `attribute` | `ansi.yellow` |
| `label` | `ansi.blue` |
| `tag` | `ansi.red` |
| `operator` | `foreground` |
| `punctuation` | `muted.fg` |
| `punctuation.bracket` | `muted.fg` |
| `punctuation.delimiter` | `muted.fg` |
| `punctuation.special` | `ansi.magenta` |
| `punctuation.list_marker` | `accent.bg` |
| `preproc` | `ansi.magenta` |
| `embedded` | `foreground` |
| `title` | `accent.bg`, bold |
| `link_text` | `link.fg` |
| `link_uri` | `muted.fg`, italic |
| `text.literal` | `ansi.green` |
| `text.code.span` | `ansi.green` |
| `emphasis` | italic, the color of the text |
| `emphasis.strong` | bold, the color of the text |

## Overriding a theme from `init.lua`

`app.opt.colors` goes over whatever theme is chosen, by `app.opt.theme` or by `/theme`. It takes `ui`, `syntax` and `palette`, with the format of a theme file:

```lua
app.opt.colors = {
  ui = { caret = "#ff9e64", list = { selected = { border = "#ff9e64" } } },
  syntax = { comment = { italic = false } },
}
```

`app.theme.define` defines a whole theme without a file, in `init.lua` or in a plugin; it needs a `name`, and then `app.opt.theme` and `/theme` find it like a file:

```lua
app.theme.define{
  name = "work",
  extends = "catppuccin-latte",
  ui = { accent = "#1e66f5" },
}
app.opt.theme = "work"
```

A theme defined this way wins over a file of the same name; the workspace's wins over the global one's.

## Porting a Vim or Neovim colorscheme

The example themes are ports, so any of them is a model ([`tokyonight-night.lua`](../examples/themes/tokyonight-night.lua) is a good one). The steps:

1. Copy the colorscheme's palette into `palette`, with its own names. Prefer the resolved colors of a palette file (many themes have an `extras/` or `palette.lua`) to reading highlight groups one by one.
2. Map its highlight groups to crest's tokens:

   | Neovim group | crest token |
   | --- | --- |
   | `Normal` | `background`, `foreground` |
   | `NormalFloat`, `Pmenu` | `popover.bg`, `popover.fg` |
   | `FloatBorder`, `WinSeparator` | `popover.border`, `border` |
   | `PmenuSel` | `list.selected.bg`, `list.selected.fg` |
   | `CursorLine` | `list.hover.bg`, `editor.active_line.bg` |
   | `Visual` | `selection` |
   | `Cursor` | `caret` |
   | `StatusLine` | `status_bar.bg`, `status_bar.fg` |
   | `TabLine`, `TabLineSel`, `TabLineFill` | `tab.bg`/`tab.fg`, `tab.selected.*`, `tab.bar.bg` |
   | `LineNr`, `CursorLineNr` | `editor.line_number.fg`, `editor.active_line_number.fg` |
   | `DiffAdd`, `DiffDelete` | `diff.added.*`, `diff.removed.*` |
   | `DiagnosticError`, `...Warn`, `...Info`, `...Ok` | `error.bg`, `warning.bg`, `info.bg`, `success.bg` |
   | `Directory`, `Underlined` | `link.fg` |
   | `Comment`, `Keyword`, `String`, `Function`, `Type`, `@property`... | `syntax.comment`, `syntax.keyword`, ... |

3. Choose the accent: the color the theme uses for its own UI highlights (its selected tab, its prompt, its statusline mode). It paints primary buttons, the focused edge and the selection border.
4. Keep the text readable. crest's test of the example themes requires `foreground` on `background` at a WCAG contrast of at least 4.5, and text on the accent of at least 3; `accent.fg` is derived to whichever of `background` and `foreground` reads better, but set it when neither does.
5. Keep the upstream's license and credit: write its repository, the commit you copied from and its copyright line at the top of the file.

## Errors

A theme never stops crest. A file that does not load (a syntax error, a runtime error, no `return` of a table, the instruction limit) is a warning, and crest uses `twocrest` instead:

```text
theme `midnight`: /home/me/.config/crest/themes/midnight.lua: syntax error: midnight.lua:4: '}' expected near 'accent'; using the twocrest theme
unknown theme `nrod`; using the twocrest theme
theme `a`: `extends` loops back to it; using the twocrest theme
```

A bad value inside a theme that loads is skipped with a warning, and the token keeps the value it would have without it:

```text
/home/me/.config/crest/themes/midnight.lua: ui.list.hovr: unknown token
/home/me/.config/crest/themes/midnight.lua: ui.caret: `#12` is not a color: use #rrggbb or #rrggbbaa
theme `midnight`: ui.accent.bg: unknown color `bleu` (not in the palette)
/home/me/.config/crest/init.lua: app.opt.colors: syntax.kw: unknown syntax name
```

The warnings of the theme in use show at startup as notices; `/theme` shows those of the theme it saves.

## Window mode

`app.opt.window` says how the window opens:

```lua
app.opt.window = "fullscreen"  -- the default (on Windows: "maximized")
app.opt.window = "maximized"
app.opt.window = "windowed"    -- 1280×800, centered
app.opt.window = { mode = "windowed", width = 1440, height = 900 }
app.opt.window = { width = 1440, height = 900 }  -- a size alone means windowed
```

The size, at least 400 pixels each way, is the windowed size: full screen and maximized windows return to it. `F11` toggles full screen at any time.

It is read **from the global config only**, including before the project picker opens: the window exists before any project, and a cloned repository should not change it. In a workspace `.crest/init.lua` it is ignored with a notice. A malformed value is a warning, and the default is used.

By platform:

- **macOS and Wayland** open full screen or maximized directly.
- **X11** window managers ignore those requests while the window is being created, so crest opens it windowed and asks once it has drawn its first frame. A window manager that refuses leaves it centered at the windowed size.
- **Windows** defaults to maximized, because full screen hides the taskbar; `"fullscreen"` still asks for it.
