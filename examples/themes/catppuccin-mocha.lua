-- Catppuccin Mocha for crest
-- Ported from https://github.com/catppuccin/nvim (lua/catppuccin/palettes/mocha.lua, lua/catppuccin/groups/*.lua @ edefef779ab08ce1a4a404713e3012b0d202bd35)
-- License: MIT, Copyright (c) 2021 Catppuccin

local p = {
  rosewater = "#f5e0dc",
  flamingo = "#f2cdcd",
  pink = "#f5c2e7",
  mauve = "#cba6f7",
  red = "#f38ba8",
  maroon = "#eba0ac",
  peach = "#fab387",
  yellow = "#f9e2af",
  green = "#a6e3a1",
  teal = "#94e2d5",
  sky = "#89dceb",
  sapphire = "#74c7ec",
  blue = "#89b4fa",
  lavender = "#b4befe",
  text = "#cdd6f4",
  subtext1 = "#bac2de",
  subtext0 = "#a6adc8",
  overlay2 = "#9399b2",
  overlay1 = "#7f849c",
  overlay0 = "#6c7086",
  surface2 = "#585b70",
  surface1 = "#45475a",
  surface0 = "#313244",
  base = "#1e1e2e",
  mantle = "#181825",
  crust = "#11111b",
}

-- CursorLine: surface0 blended 64% over base (upstream U.darken).
local cursor_line = color.mix(p.base, p.surface0, 0.64)

return {
  name = "catppuccin-mocha",
  appearance = "dark",
  palette = p,
  ui = {
    background = "base",
    foreground = "text",
    border = "crust", -- WinSeparator
    -- Mauve, the default accent of the Catppuccin ports (the nvim port draws
    -- float borders and titles in blue, used here for popovers and links).
    accent = {
      bg = "mauve",
      fg = "crust",
      hover = color.mix(p.mauve, p.text, 0.15),
      active = color.mix(p.mauve, p.base, 0.2),
    },
    muted = { bg = "surface0", fg = "subtext0" },
    selection = "surface1", -- Visual
    caret = "rosewater", -- Cursor
    overlay = color.alpha(p.crust, 0.7),
    button = {
      bg = "surface0",
      fg = "text",
      hover = "surface1",
      active = "surface2",
    },
    input = {
      bg = "base",
      border = "surface1",
      focused = { border = "mauve" },
    },
    list = {
      hover = cursor_line,
      selected = { bg = "surface0", fg = "text", border = "mauve" }, -- PmenuSel
      even = color.mix(p.base, p.mantle, 0.5),
      header = { bg = "mantle", fg = "subtext1" },
    },
    popover = { bg = "mantle", fg = "text", border = "blue" }, -- NormalFloat, FloatBorder
    title_bar = { bg = "mantle", border = "crust" },
    status_bar = { bg = "mantle", fg = "text", border = "crust" }, -- StatusLine
    sidebar = {
      bg = "mantle",
      fg = "text",
      border = "crust",
      hover = "surface0",
      selected = { bg = "surface1", fg = "text" },
    },
    tab = {
      bar = "mantle", -- TabLineFill
      bg = "crust", -- TabLine
      fg = "overlay0",
      selected = { bg = "base", fg = "text" }, -- TabLineSel links Normal
    },
    scrollbar = { thumb = { bg = "overlay0", hover = "overlay1" } }, -- PmenuThumb
    link = { fg = "blue", hover = "sapphire", active = "lavender" }, -- Directory
    info = "sky",
    success = "green",
    warning = "yellow",
    error = "red",
    diff = {
      added = { fg = "green", bg = color.mix(p.base, p.green, 0.18) }, -- DiffAdd
      removed = { fg = "red", bg = color.mix(p.base, p.red, 0.18) }, -- DiffDelete
      hunk = "overlay2",
      header = "blue", -- diffFile
    },
    editor = {
      active_line = cursor_line,
      line_number = "surface1", -- LineNr
      active_line_number = "lavender", -- CursorLineNr
    },
    chart = { "mauve", "peach", "blue", "green", "pink" },
    ansi = { red = "red", green = "green", yellow = "yellow", blue = "blue", magenta = "pink", cyan = "sky" },
  },
  syntax = {
    comment = { fg = "overlay2", italic = true },
    ["comment.doc"] = { fg = "overlay2", italic = true },
    keyword = "mauve",
    string = "green",
    ["string.escape"] = "pink",
    ["string.regex"] = "pink",
    ["string.special"] = "pink",
    ["string.special.symbol"] = "flamingo",
    number = "peach",
    boolean = "peach",
    constant = "peach",
    ["function"] = "blue",
    constructor = "yellow",
    type = "yellow",
    enum = "yellow",
    variant = "teal", -- @lsp.type.enumMember
    variable = "text",
    ["variable.special"] = "red", -- @variable.builtin
    property = "lavender",
    attribute = "peach", -- @attribute links Constant
    label = "sapphire",
    tag = "blue",
    operator = "sky",
    punctuation = "overlay2",
    ["punctuation.bracket"] = "overlay2",
    ["punctuation.delimiter"] = "overlay2",
    ["punctuation.special"] = "pink",
    ["punctuation.list_marker"] = "teal", -- @markup.list
    preproc = "pink",
    embedded = "text",
    title = { fg = "blue", bold = true },
    link_text = "lavender",
    link_uri = { fg = "blue", italic = true },
    ["text.literal"] = "green",
    ["text.code.span"] = "green",
    emphasis = { fg = "red", italic = true },
    ["emphasis.strong"] = { fg = "red", bold = true, italic = false },
  },
}
