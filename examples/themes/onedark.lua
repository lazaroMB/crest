-- One Dark for crest
-- Ported from https://github.com/navarasu/onedark.nvim (lua/onedark/palette.lua "dark" style, lua/onedark/highlights.lua @ df4792accde9db0043121f32628bcf8e645d9aea)
-- License: MIT, Copyright (c) 2021 Navarasu

local p = {
  black = "#181a1f",
  bg0 = "#282c34",
  bg1 = "#31353f",
  bg2 = "#393f4a",
  bg3 = "#3b3f4c",
  bg_d = "#21252b",
  bg_blue = "#73b8f1",
  bg_yellow = "#ebd09c",
  fg = "#abb2bf",
  purple = "#c678dd",
  green = "#98c379",
  orange = "#d19a66",
  blue = "#61afef",
  yellow = "#e5c07b",
  cyan = "#56b6c2",
  red = "#e86671",
  grey = "#5c6370",
  light_grey = "#848b98",
  dark_cyan = "#2b6f77",
  dark_red = "#993939",
  dark_yellow = "#93691d",
  dark_purple = "#8a3fa0",
  diff_add = "#31392b",
  diff_delete = "#382b2c",
  diff_change = "#1c3448",
  diff_text = "#2c5372",
}

return {
  name = "onedark",
  appearance = "dark",
  palette = p,
  ui = {
    background = "bg0",
    foreground = "fg",
    border = "bg3", -- WinSeparator
    -- Blue: Directory and Function; PmenuSel uses its lighter bg_blue.
    accent = {
      bg = "blue",
      fg = "bg0",
      hover = "bg_blue",
      active = color.mix(p.blue, p.bg0, 0.25),
    },
    muted = { bg = "bg1", fg = "light_grey" },
    selection = "bg3", -- Visual
    caret = "fg", -- Cursor is reverse video
    overlay = color.alpha(p.black, 0.7),
    button = {
      bg = "bg1",
      fg = "fg",
      hover = "bg2",
      active = "bg3",
    },
    input = {
      bg = "bg0",
      border = "bg3",
      focused = { border = "blue" },
    },
    list = {
      hover = "bg1", -- CursorLine
      selected = { bg = "bg_blue", fg = "bg0", border = "blue" }, -- PmenuSel
      even = color.mix(p.bg0, p.bg1, 0.4),
      header = { bg = "bg_d", fg = "fg" },
    },
    popover = { bg = "bg1", fg = "fg", border = "grey" }, -- NormalFloat, FloatBorder
    title_bar = { bg = "bg_d", border = "bg3" },
    status_bar = { bg = "bg2", fg = "fg", border = "bg3" }, -- StatusLine
    sidebar = {
      bg = "bg_d", -- the sidebars of the plugin integrations
      fg = "fg",
      border = "bg3",
      hover = "bg1",
      selected = { bg = "bg2", fg = "fg" },
    },
    tab = {
      bar = "bg1", -- TabLineFill
      bg = "bg1", -- TabLine
      fg = "fg",
      selected = { bg = "fg", fg = "bg0" }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = "grey", hover = "light_grey" } }, -- PmenuThumb
    link = { fg = "blue", hover = "bg_blue", active = "cyan" }, -- Directory
    info = "cyan",
    success = "green",
    warning = "yellow",
    error = "red",
    diff = {
      added = { fg = "green", bg = "diff_add" }, -- DiffAdd
      removed = { fg = "red", bg = "diff_delete" }, -- DiffDelete
    },
    editor = {
      active_line = "bg1", -- CursorLine
      line_number = "grey", -- LineNr
      active_line_number = "fg", -- CursorLineNr
    },
    chart = { "blue", "yellow", "cyan", "green", "purple" },
    ansi = { red = "red", green = "green", yellow = "yellow", blue = "blue", magenta = "purple", cyan = "cyan" },
  },
  syntax = {
    comment = { fg = "grey", italic = true },
    ["comment.doc"] = { fg = "grey", italic = true },
    keyword = "purple",
    string = "green",
    ["string.escape"] = "red",
    ["string.regex"] = "orange",
    ["string.special"] = "dark_cyan",
    ["string.special.symbol"] = "cyan",
    number = "orange",
    boolean = "orange",
    constant = "orange",
    ["function"] = "blue",
    constructor = { fg = "yellow", bold = true },
    type = "yellow",
    enum = "yellow",
    variant = "orange", -- @lsp.type.enumMember
    variable = "fg",
    ["variable.special"] = "red", -- @variable.builtin
    property = "cyan",
    attribute = "cyan",
    label = "red",
    tag = "purple",
    operator = "fg",
    punctuation = "light_grey",
    ["punctuation.bracket"] = "light_grey",
    ["punctuation.delimiter"] = "light_grey",
    ["punctuation.special"] = "red",
    ["punctuation.list_marker"] = "red", -- @markup.list
    preproc = "purple",
    embedded = "fg",
    title = { fg = "orange", bold = true }, -- @markup.heading
    link_text = "blue",
    link_uri = { fg = "cyan", italic = false },
    ["text.literal"] = "green",
    ["text.code.span"] = "green",
    emphasis = { fg = "fg", italic = true },
    ["emphasis.strong"] = { fg = "fg", bold = true, italic = false },
  },
}
