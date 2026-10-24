-- Tokyo Night (night) for crest
-- Ported from https://github.com/folke/tokyonight.nvim (extras/lua/tokyonight_night.lua, lua/tokyonight/groups/*.lua @ cdc07ac78467a233fd62c493de29a17e0cf2b2b6)
-- License: Apache-2.0, Copyright (c) Folke Lemaitre (the LICENSE file names no holder)

-- The resolved night colors (upstream nests diff, git and terminal; those
-- entries are flattened here as diff_add, git_add, ...).
local p = {
  bg = "#1a1b26",
  bg_dark = "#16161e",
  bg_dark1 = "#0c0e14",
  bg_float = "#16161e",
  bg_highlight = "#292e42",
  bg_popup = "#16161e",
  bg_search = "#3d59a1",
  bg_sidebar = "#16161e",
  bg_statusline = "#16161e",
  bg_visual = "#283457",
  black = "#15161e",
  blue = "#7aa2f7",
  blue0 = "#3d59a1",
  blue1 = "#2ac3de",
  blue2 = "#0db9d7",
  blue5 = "#89ddff",
  blue6 = "#b4f9f8",
  blue7 = "#394b70",
  border = "#15161e",
  border_highlight = "#27a1b9",
  comment = "#565f89",
  cyan = "#7dcfff",
  dark3 = "#545c7e",
  dark5 = "#737aa2",
  diff_add = "#243e4a",
  diff_change = "#1f2231",
  diff_delete = "#4a272f",
  diff_text = "#394b70",
  error = "#db4b4b",
  fg = "#c0caf5",
  fg_dark = "#a9b1d6",
  fg_float = "#c0caf5",
  fg_gutter = "#3b4261",
  fg_sidebar = "#a9b1d6",
  git_add = "#449dab",
  git_change = "#6183bb",
  git_delete = "#914c54",
  green = "#9ece6a",
  green1 = "#73daca",
  green2 = "#41a6b5",
  hint = "#1abc9c",
  info = "#0db9d7",
  magenta = "#bb9af7",
  magenta2 = "#ff007c",
  orange = "#ff9e64",
  purple = "#9d7cd8",
  red = "#f7768e",
  red1 = "#db4b4b",
  teal = "#1abc9c",
  terminal_black = "#414868",
  warning = "#e0af68",
  yellow = "#e0af68",
}

return {
  name = "tokyonight-night",
  appearance = "dark",
  palette = p,
  ui = {
    background = "bg",
    foreground = "fg",
    border = "border", -- WinSeparator
    -- Blue: TabLineSel, Title, Directory and Function.
    accent = {
      bg = "blue",
      fg = "black",
      hover = color.mix(p.blue, p.fg, 0.15),
      active = color.mix(p.blue, p.bg, 0.25),
    },
    muted = { bg = "bg_highlight", fg = "dark5" },
    selection = "bg_visual", -- Visual
    caret = "fg", -- Cursor
    overlay = color.alpha(p.bg_dark1, 0.7),
    button = {
      bg = "bg_highlight",
      fg = "fg",
      hover = "fg_gutter",
      active = "dark3",
    },
    input = {
      bg = "bg",
      border = "fg_gutter",
      focused = { border = "blue" },
    },
    list = {
      hover = "bg_highlight", -- CursorLine
      selected = { bg = color.mix(p.bg, p.fg_gutter, 0.8), fg = "fg", border = "blue" }, -- PmenuSel
      even = color.mix(p.bg, p.bg_highlight, 0.35),
      header = { bg = "bg_dark", fg = "fg_dark" },
    },
    popover = { bg = "bg_float", fg = "fg_float", border = "border_highlight" }, -- NormalFloat, FloatBorder
    title_bar = { bg = "bg_dark", border = "border" },
    status_bar = { bg = "bg_statusline", fg = "fg_sidebar", border = "border" }, -- StatusLine
    sidebar = {
      bg = "bg_sidebar",
      fg = "fg_sidebar",
      border = "border",
      hover = "bg_highlight",
      selected = { bg = color.mix(p.bg, p.fg_gutter, 0.8), fg = "fg" },
    },
    tab = {
      bar = "black", -- TabLineFill
      bg = "bg_statusline", -- TabLine
      fg = "dark5", -- upstream TabLine uses fg_gutter, too faint for labels
      selected = { bg = "blue", fg = "black" }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = "fg_gutter", hover = "dark3" } }, -- PmenuThumb
    link = { fg = "blue", hover = "cyan", active = "blue0" }, -- Directory
    info = "info",
    success = "green",
    warning = "warning",
    error = "error",
    diff = {
      added = { fg = "git_add", bg = "diff_add" }, -- diffAdded, DiffAdd
      removed = { fg = "red1", bg = "diff_delete" }, -- DiffDelete; git_delete is too dark for text
      hunk = "blue",
    },
    editor = {
      active_line = "bg_highlight", -- CursorLine
      line_number = "fg_gutter", -- LineNr
      active_line_number = "orange", -- CursorLineNr
    },
    chart = { "blue", "yellow", "green", "teal", "magenta" }, -- the rainbow colors
    ansi = { red = "red", green = "green", yellow = "yellow", blue = "blue", magenta = "magenta", cyan = "cyan" },
  },
  syntax = {
    comment = { fg = "comment", italic = true },
    ["comment.doc"] = { fg = "comment", italic = true },
    keyword = { fg = "purple", italic = true }, -- @keyword
    string = "green",
    ["string.escape"] = "magenta",
    ["string.regex"] = "blue6",
    ["string.special"] = "blue1", -- SpecialChar links Special
    ["string.special.symbol"] = "magenta",
    number = "orange",
    boolean = "orange",
    constant = "orange",
    ["function"] = "blue",
    constructor = "magenta",
    type = "blue1",
    enum = "blue1",
    variant = "orange", -- @lsp.type.enumMember links @constant
    variable = "fg",
    ["variable.special"] = "red", -- @variable.builtin
    property = "green1",
    attribute = "cyan", -- @attribute links PreProc
    label = "blue",
    tag = "magenta", -- @tag links Label, then Statement
    operator = "blue5",
    punctuation = "blue1",
    ["punctuation.bracket"] = "fg_dark",
    ["punctuation.delimiter"] = "blue5",
    ["punctuation.special"] = "blue5",
    ["punctuation.list_marker"] = "blue5", -- @markup.list
    preproc = "cyan",
    embedded = "fg",
    title = { fg = "blue", bold = true },
    link_text = "teal", -- @markup.link
    link_uri = { fg = "teal", italic = false }, -- Underlined, no color of its own
    ["text.literal"] = "green",
    ["text.code.span"] = "green",
    emphasis = { italic = true },
    ["emphasis.strong"] = { bold = true, italic = false },
  },
}
