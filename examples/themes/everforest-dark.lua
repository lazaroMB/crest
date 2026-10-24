-- Everforest Dark (medium) for crest
-- Ported from https://github.com/sainnhe/everforest (autoload/everforest.vim, colors/everforest.vim @ 85a86eb62409e3ec88713bff3d1b9d7374e112e4)
-- License: MIT, Copyright (c) 2019 sainnhe

-- The dark palette with the medium background.
local p = {
  bg_dim = "#232a2e",
  bg0 = "#2d353b",
  bg1 = "#343f44",
  bg2 = "#3d484d",
  bg3 = "#475258",
  bg4 = "#4f585e",
  bg5 = "#56635f",
  bg_visual = "#543a48",
  bg_red = "#514045",
  bg_yellow = "#4d4c43",
  bg_green = "#425047",
  bg_blue = "#3a515d",
  bg_purple = "#4a444e",
  fg = "#d3c6aa",
  red = "#e67e80",
  orange = "#e69875",
  yellow = "#dbbc7f",
  green = "#a7c080",
  aqua = "#83c092",
  blue = "#7fbbb3",
  purple = "#d699b6",
  grey0 = "#7a8478",
  grey1 = "#859289",
  grey2 = "#9da9a0",
  statusline1 = "#a7c080",
  statusline2 = "#d3c6aa",
  statusline3 = "#e67e80",
}

return {
  name = "everforest-dark",
  appearance = "dark",
  palette = p,
  ui = {
    background = "bg0",
    foreground = "fg",
    border = "bg4", -- VertSplit
    -- Green (statusline1): PmenuSel and TabLineSel.
    accent = {
      bg = "statusline1",
      fg = "bg0",
      hover = color.mix(p.statusline1, p.fg, 0.2),
      active = color.mix(p.statusline1, p.bg0, 0.25),
    },
    muted = { bg = "bg1", fg = "grey1" },
    selection = "bg_visual", -- Visual
    caret = "fg", -- Cursor is reverse video
    overlay = color.alpha(p.bg_dim, 0.7),
    button = {
      bg = "bg1",
      fg = "fg",
      hover = "bg2",
      active = "bg3",
    },
    input = {
      bg = "bg0",
      border = "bg4",
      focused = { border = "statusline1" },
    },
    list = {
      hover = "bg1", -- CursorLine
      selected = { bg = "statusline1", fg = "bg0", border = "statusline1" }, -- PmenuSel
      even = color.mix(p.bg0, p.bg1, 0.5),
      header = { bg = "bg_dim", fg = "fg" },
    },
    popover = { bg = "bg2", fg = "fg", border = "grey1" }, -- NormalFloat, FloatBorder (float_style bright)
    title_bar = { bg = "bg_dim", border = "bg4" },
    status_bar = { bg = "bg2", fg = "grey1", border = "bg4" }, -- StatusLine
    sidebar = {
      bg = "bg_dim",
      fg = "fg",
      border = "bg4",
      hover = "bg1",
      selected = { bg = "bg2", fg = "fg" },
    },
    tab = {
      bar = "bg1", -- TabLineFill
      bg = "bg3", -- TabLine
      fg = "grey2",
      selected = { bg = "statusline1", fg = "bg0" }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = "grey0", hover = "grey1" } }, -- PmenuThumb (pmenu_style bright)
    -- Blue (TSURI): Directory is green, the same as the accent.
    link = { fg = "blue", hover = "aqua", active = "green" },
    info = "blue",
    success = "green",
    warning = "yellow",
    error = "red",
    diff = {
      added = { fg = "green", bg = "bg_green" }, -- DiffAdd
      removed = { fg = "red", bg = "bg_red" }, -- DiffDelete
      hunk = "blue", -- DiffText
    },
    editor = {
      active_line = "bg1", -- CursorLine
      line_number = "bg5", -- LineNr (ui_contrast low)
      active_line_number = "grey1", -- CursorLineNr
    },
    chart = { "green", "yellow", "blue", "aqua", "purple" },
    ansi = { red = "red", green = "green", yellow = "yellow", blue = "blue", magenta = "purple", cyan = "aqua" },
  },
  -- From the tree-sitter groups (TS*), which differ from the Vim syntax
  -- groups in places: strings are aqua there, green in Vim syntax.
  syntax = {
    comment = { fg = "grey1", italic = true },
    ["comment.doc"] = { fg = "grey1", italic = true },
    keyword = "red",
    string = "aqua",
    ["string.escape"] = "green",
    ["string.regex"] = "green",
    ["string.special"] = "yellow", -- links SpecialChar
    ["string.special.symbol"] = "fg",
    number = "purple",
    boolean = "purple",
    constant = "fg", -- TSConstant (built-in constants are purple)
    ["function"] = "green",
    constructor = "green",
    type = "yellow", -- YellowItalic, italic only with enable_italic
    enum = "yellow",
    variant = "purple",
    variable = "fg",
    ["variable.special"] = "purple", -- TSVariableBuiltin
    property = "blue",
    attribute = "purple",
    label = "orange",
    tag = "orange",
    operator = "orange",
    punctuation = "grey1",
    ["punctuation.bracket"] = "fg",
    ["punctuation.delimiter"] = "grey1",
    ["punctuation.special"] = "blue",
    ["punctuation.list_marker"] = "red",
    preproc = "purple",
    embedded = "fg",
    title = { fg = "orange", bold = true },
    link_text = "aqua", -- TSTextReference links Constant
    link_uri = { fg = "blue", italic = false }, -- TSURI
    ["text.literal"] = "green", -- TSLiteral links String
    ["text.code.span"] = "green",
    emphasis = { italic = true },
    ["emphasis.strong"] = { bold = true, italic = false },
  },
}
