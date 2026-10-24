-- Gruvbox Light for crest
-- Ported from https://github.com/ellisonleao/gruvbox.nvim (lua/gruvbox.lua @ 154eb5ff5b96d0641307113fa385eaf0d36d9796),
-- the Neovim port of https://github.com/morhetz/gruvbox (the palette's origin)
-- License: MIT, Copyright (c) 2021 NpX

local p = {
  dark0_hard = "#1d2021",
  dark0 = "#282828",
  dark0_soft = "#32302f",
  dark1 = "#3c3836",
  dark2 = "#504945",
  dark3 = "#665c54",
  dark4 = "#7c6f64",
  light0_hard = "#f9f5d7",
  light0 = "#fbf1c7",
  light0_soft = "#f2e5bc",
  light1 = "#ebdbb2",
  light2 = "#d5c4a1",
  light3 = "#bdae93",
  light4 = "#a89984",
  bright_red = "#fb4934",
  bright_green = "#b8bb26",
  bright_yellow = "#fabd2f",
  bright_blue = "#83a598",
  bright_purple = "#d3869b",
  bright_aqua = "#8ec07c",
  bright_orange = "#fe8019",
  neutral_red = "#cc241d",
  neutral_green = "#98971a",
  neutral_yellow = "#d79921",
  neutral_blue = "#458588",
  neutral_purple = "#b16286",
  neutral_aqua = "#689d6a",
  neutral_orange = "#d65d0e",
  faded_red = "#9d0006",
  faded_green = "#79740e",
  faded_yellow = "#b57614",
  faded_blue = "#076678",
  faded_purple = "#8f3f71",
  faded_aqua = "#427b58",
  faded_orange = "#af3a03",
  dark_red = "#722529",
  dark_green = "#62693e",
  dark_aqua = "#49503b",
  light_red = "#fc9487",
  light_green = "#d5d39b",
  light_aqua = "#e8e5b5",
  gray = "#928374",
}

-- The colors of the light background at the default contrast, as upstream's
-- get_colors() picks them.
local c = {
  bg0 = p.light0,
  bg1 = p.light1,
  bg2 = p.light2,
  bg3 = p.light3,
  bg4 = p.light4,
  fg1 = p.dark1,
  fg4 = p.dark4,
  red = p.faded_red,
  green = p.faded_green,
  yellow = p.faded_yellow,
  blue = p.faded_blue,
  purple = p.faded_purple,
  aqua = p.faded_aqua,
  orange = p.faded_orange,
  diff_red = p.light_red,
  diff_green = p.light_green,
  gray = p.gray,
}

return {
  name = "gruvbox-light",
  appearance = "light",
  palette = p,
  ui = {
    background = c.bg0,
    foreground = c.fg1,
    border = c.bg3, -- WinSeparator
    -- Blue: the PmenuSel background.
    accent = {
      bg = c.blue,
      fg = c.bg0,
      hover = color.mix(c.blue, c.fg1, 0.15),
      active = color.mix(c.blue, c.bg0, 0.25),
    },
    muted = { bg = c.bg1, fg = c.fg4 },
    selection = c.bg3, -- Visual
    caret = c.fg1, -- Cursor is reverse video
    overlay = color.alpha(p.light0_hard, 0.7),
    button = {
      bg = c.bg1,
      fg = c.fg1,
      hover = c.bg2,
      active = c.bg3,
    },
    input = {
      bg = c.bg0,
      border = c.bg2,
      focused = { border = c.blue },
    },
    list = {
      hover = c.bg1, -- CursorLine
      selected = { bg = c.blue, fg = c.bg0, border = c.blue }, -- PmenuSel (its text is bg2; bg0 reads better)
      even = color.mix(c.bg0, c.bg1, 0.4),
      header = { bg = c.bg1, fg = c.fg1 },
    },
    popover = { bg = c.bg1, fg = c.fg1, border = c.bg3 }, -- NormalFloat
    title_bar = { bg = c.bg1, border = c.bg3 },
    status_bar = { bg = c.bg2, fg = c.fg1, border = c.bg3 }, -- StatusLine
    sidebar = {
      bg = c.bg1, -- SignColumn
      fg = c.fg1,
      border = c.bg3,
      hover = c.bg2,
      selected = { bg = c.bg3, fg = c.fg1 },
    },
    tab = {
      bar = c.bg1, -- TabLineFill
      bg = c.bg1, -- TabLine
      fg = c.fg4, -- upstream TabLine uses bg4, too faint for labels
      selected = { bg = c.bg0, fg = c.green }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = c.bg4, hover = c.fg4 } }, -- PmenuThumb
    link = { fg = c.blue, hover = c.aqua, active = c.green }, -- Underlined
    info = c.blue,
    success = c.green,
    warning = c.yellow,
    error = c.red,
    diff = {
      added = { fg = c.green, bg = c.diff_green }, -- DiffAdd
      removed = { fg = c.red, bg = c.diff_red }, -- DiffDelete
      hunk = c.aqua,
    },
    editor = {
      active_line = c.bg1, -- CursorLine
      line_number = c.bg4, -- LineNr
      active_line_number = c.yellow, -- CursorLineNr
    },
    chart = { c.blue, c.yellow, c.aqua, c.green, c.purple },
    ansi = { red = c.red, green = c.green, yellow = c.yellow, blue = c.blue, magenta = c.purple, cyan = c.aqua },
  },
  syntax = {
    comment = { fg = c.gray, italic = true },
    ["comment.doc"] = { fg = c.gray, italic = true },
    keyword = c.red,
    string = { fg = c.green, italic = true }, -- italic.strings is on by default
    ["string.escape"] = { fg = c.orange, italic = false }, -- SpecialChar
    ["string.regex"] = { fg = c.green, italic = true },
    ["string.special"] = { fg = c.orange, italic = false },
    ["string.special.symbol"] = { fg = c.blue, italic = false }, -- links Identifier
    number = c.purple,
    boolean = c.purple,
    constant = c.purple,
    ["function"] = { fg = c.green, bold = true },
    constructor = c.orange, -- links Special
    type = c.yellow,
    enum = c.yellow,
    variant = c.purple,
    variable = c.fg1,
    ["variable.special"] = c.orange, -- @variable.builtin links Special
    property = c.blue, -- links Identifier
    attribute = c.aqua, -- links PreProc
    label = c.red,
    tag = c.orange, -- Tag links Special
    operator = c.orange,
    punctuation = c.orange, -- Delimiter
    ["punctuation.bracket"] = c.orange,
    ["punctuation.delimiter"] = c.orange,
    ["punctuation.special"] = c.orange,
    ["punctuation.list_marker"] = c.orange,
    preproc = c.aqua,
    embedded = c.fg1,
    title = { fg = c.green, bold = true },
    link_text = c.blue,
    link_uri = { fg = c.blue, italic = false },
    ["text.literal"] = { fg = c.green, italic = true }, -- @markup.raw links String
    ["text.code.span"] = { fg = c.green, italic = true },
    emphasis = { italic = true },
    ["emphasis.strong"] = { bold = true, italic = false },
  },
}
