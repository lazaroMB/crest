-- Nightfox for crest
-- Ported from https://github.com/EdenEast/nightfox.nvim (lua/nightfox/palette/nightfox.lua, lua/nightfox/group/*.lua @ 4dacd3f0185a2227bdf3b6c0975a8f0bf87cac9a)
-- License: MIT, Copyright (c) 2021 James Simpson

-- Upstream builds each color as a Shade (base, bright, dim); the bright and
-- dim values are written out here as <color>_bright and <color>_dim.
local p = {
  black = "#393b44",
  black_bright = "#575860",
  black_dim = "#30323a",
  red = "#c94f6d",
  red_bright = "#d16983",
  red_dim = "#ab435d",
  green = "#81b29a",
  green_bright = "#8ebaa4",
  green_dim = "#6e9783",
  yellow = "#dbc074",
  yellow_bright = "#e0c989",
  yellow_dim = "#baa363",
  blue = "#719cd6",
  blue_bright = "#86abdc",
  blue_dim = "#6085b6",
  magenta = "#9d79d6",
  magenta_bright = "#baa1e2",
  magenta_dim = "#8567b6",
  cyan = "#63cdcf",
  cyan_bright = "#7ad5d6",
  cyan_dim = "#54aeb0",
  white = "#dfdfe0",
  white_bright = "#e4e4e5",
  white_dim = "#bebebe",
  orange = "#f4a261",
  orange_bright = "#f6b079",
  orange_dim = "#cf8a52",
  pink = "#d67ad2",
  pink_bright = "#dc8ed9",
  pink_dim = "#b668b3",
  comment = "#738091",
  bg0 = "#131a24", -- dark bg (status line and float)
  bg1 = "#192330", -- default bg
  bg2 = "#212e3f", -- lighter bg (colorcolumn, folds)
  bg3 = "#29394f", -- lighter bg (cursor line)
  bg4 = "#39506d", -- conceal, border fg
  fg0 = "#d6d6d7", -- lighter fg
  fg1 = "#cdcecf", -- default fg
  fg2 = "#aeafb0", -- darker fg (status line)
  fg3 = "#71839b", -- darker fg (line numbers, fold columns)
  sel0 = "#2b3b51", -- popup bg, visual selection bg
  sel1 = "#3c5372", -- popup sel bg, search bg
}

return {
  name = "nightfox",
  appearance = "dark",
  palette = p,
  ui = {
    background = "bg1",
    foreground = "fg1",
    border = "bg4", -- the palette's border color (WinSeparator is bg0, nearly invisible)
    -- Blue: the active tab and border of upstream's terminal extras.
    accent = {
      bg = "blue",
      fg = "bg0",
      hover = "blue_bright",
      active = "blue_dim",
    },
    muted = { bg = "bg2", fg = "fg3" },
    selection = "sel0", -- Visual
    caret = "fg1", -- Cursor
    overlay = color.alpha(p.bg0, 0.7),
    button = {
      bg = "bg2",
      fg = "fg1",
      hover = "bg3",
      active = "sel1",
    },
    input = {
      bg = "bg1",
      border = "bg4",
      focused = { border = "blue" },
    },
    list = {
      hover = "bg3", -- CursorLine
      selected = { bg = "sel1", fg = "fg1", border = "blue" }, -- PmenuSel
      even = color.mix(p.bg1, p.bg2, 0.5),
      header = { bg = "bg0", fg = "fg2" },
    },
    popover = { bg = "bg0", fg = "fg1", border = "fg3" }, -- NormalFloat, FloatBorder
    title_bar = { bg = "bg0", border = "bg4" },
    status_bar = { bg = "bg0", fg = "fg2", border = "bg4" }, -- StatusLine
    sidebar = {
      bg = "bg0",
      fg = "fg1",
      border = "bg4",
      hover = "bg2",
      selected = { bg = "sel0", fg = "fg0" },
    },
    tab = {
      bar = "bg0", -- TabLineFill
      bg = "bg2", -- TabLine
      fg = "fg2",
      selected = { bg = "fg3", fg = "bg1" }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = "sel1", hover = "fg3" } }, -- PmenuThumb
    link = { fg = "blue_bright", hover = "cyan", active = "blue" }, -- Directory (syntax.func)
    info = "blue",
    success = "green",
    warning = "yellow",
    error = "red",
    diff = {
      added = { fg = "green", bg = color.mix(p.bg1, p.green_dim, 0.15) }, -- diffAdded, DiffAdd
      removed = { fg = "red", bg = color.mix(p.bg1, p.red_dim, 0.15) }, -- diffRemoved, DiffDelete
      hunk = "orange_bright", -- diffLine
      header = "blue", -- diffFile
    },
    editor = {
      active_line = "bg3", -- CursorLine
      line_number = "fg3", -- LineNr
      active_line_number = "yellow", -- CursorLineNr
    },
    chart = { "blue", "yellow", "cyan", "green", "magenta" },
    ansi = { red = "red", green = "green", yellow = "yellow", blue = "blue", magenta = "magenta", cyan = "cyan" },
  },
  syntax = {
    comment = { fg = "comment", italic = false }, -- upstream styles default to none
    ["comment.doc"] = { fg = "comment", italic = false },
    keyword = "magenta",
    string = "green",
    ["string.escape"] = { fg = "yellow_bright", bold = true },
    ["string.regex"] = "yellow_bright",
    ["string.special"] = "blue_bright", -- links Special
    ["string.special.symbol"] = "blue_bright",
    number = "orange",
    boolean = "orange", -- links Number
    constant = "orange_bright",
    ["function"] = "blue_bright",
    constructor = "cyan",
    type = "yellow",
    enum = "yellow",
    variant = "orange_bright",
    variable = "white",
    ["variable.special"] = "red", -- @variable.builtin (builtin0)
    property = "blue", -- field
    attribute = "orange_bright", -- links Constant
    label = "magenta_bright", -- links Conditional
    tag = "magenta",
    operator = "fg2",
    punctuation = "fg2",
    ["punctuation.bracket"] = "fg2",
    ["punctuation.delimiter"] = "fg2",
    ["punctuation.special"] = "cyan_bright", -- builtin1
    ["punctuation.list_marker"] = "cyan_bright",
    preproc = "pink_bright",
    embedded = "fg1",
    title = { fg = "blue_bright", bold = true },
    link_text = { fg = "magenta", bold = true }, -- @markup.link
    link_uri = { fg = "orange_bright", italic = true }, -- @markup.link.url
    ["text.literal"] = { fg = "cyan", italic = true }, -- @markup.raw
    ["text.code.span"] = { fg = "cyan", italic = true },
    emphasis = { italic = true },
    ["emphasis.strong"] = { bold = true, italic = false },
  },
}
