-- Tokyo Night (day) for crest
-- Ported from https://github.com/folke/tokyonight.nvim (extras/lua/tokyonight_day.lua, lua/tokyonight/groups/*.lua @ cdc07ac78467a233fd62c493de29a17e0cf2b2b6)
-- License: Apache-2.0, Copyright (c) Folke Lemaitre (the LICENSE file names no holder)

-- The resolved day colors (upstream nests diff, git and terminal; those
-- entries are flattened here as diff_add, git_add, ...).
local p = {
  bg = "#e1e2e7",
  bg_dark = "#d0d5e3",
  bg_dark1 = "#c1c9df",
  bg_float = "#d0d5e3",
  bg_highlight = "#c4c8da",
  bg_popup = "#d0d5e3",
  bg_search = "#7890dd",
  bg_sidebar = "#d0d5e3",
  bg_statusline = "#d0d5e3",
  bg_visual = "#b7c1e3",
  black = "#b4b5b9",
  blue = "#2e7de9",
  blue0 = "#7890dd",
  blue1 = "#188092",
  blue2 = "#07879d",
  blue5 = "#006a83",
  blue6 = "#2e5857",
  blue7 = "#92a6d5",
  border = "#b4b5b9",
  border_highlight = "#4094a3",
  comment = "#848cb5",
  cyan = "#007197",
  dark3 = "#8990b3",
  dark5 = "#68709a",
  diff_add = "#b7ced5",
  diff_change = "#d5d9e4",
  diff_delete = "#dababe",
  diff_text = "#92a6d5",
  error = "#c64343",
  fg = "#3760bf",
  fg_dark = "#6172b0",
  fg_float = "#3760bf",
  fg_gutter = "#a8aecb",
  fg_sidebar = "#6172b0",
  git_add = "#4197a4",
  git_change = "#506d9c",
  git_delete = "#c47981",
  green = "#587539",
  green1 = "#387068",
  green2 = "#38919f",
  hint = "#118c74",
  info = "#07879d",
  magenta = "#9854f1",
  magenta2 = "#d20065",
  orange = "#b15c00",
  purple = "#7847bd",
  red = "#f52a65",
  red1 = "#c64343",
  teal = "#118c74",
  terminal_black = "#a1a6c5",
  warning = "#8c6c3e",
  yellow = "#8c6c3e",
}

return {
  name = "tokyonight-day",
  appearance = "light",
  palette = p,
  ui = {
    background = "bg",
    foreground = "fg",
    border = "border", -- WinSeparator
    -- Blue: TabLineSel, Title, Directory and Function.
    accent = {
      bg = "blue",
      fg = "bg", -- black (TabLineSel fg) is too faint on blue in the day style
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
      selected = { bg = "blue", fg = "bg" }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = "fg_gutter", hover = "dark3" } }, -- PmenuThumb
    link = { fg = "blue", hover = "cyan", active = "blue0" }, -- Directory
    info = "info",
    success = "green",
    warning = "warning",
    error = "error",
    diff = {
      added = { fg = "green", bg = "diff_add" }, -- DiffAdd; git_add is too faint for text on day
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
