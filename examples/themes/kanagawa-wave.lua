-- Kanagawa Wave for crest
-- Ported from https://github.com/rebelot/kanagawa.nvim (lua/kanagawa/colors.lua, lua/kanagawa/themes.lua, lua/kanagawa/highlights/*.lua @ bb85e4bfc8d89b0e62c8fa53ccdd13d12e2f77b3)
-- License: MIT, Copyright (c) 2021 Tommaso Laurenzi

-- The palette colors the wave theme uses.
local p = {
  sumiInk0 = "#16161d",
  sumiInk1 = "#181820",
  sumiInk2 = "#1a1a22",
  sumiInk3 = "#1f1f28",
  sumiInk4 = "#2a2a37",
  sumiInk5 = "#363646",
  sumiInk6 = "#54546d",
  waveBlue1 = "#223249",
  waveBlue2 = "#2d4f67",
  winterGreen = "#2b3328",
  winterYellow = "#49443c",
  winterRed = "#43242b",
  winterBlue = "#252535",
  autumnGreen = "#76946a",
  autumnRed = "#c34043",
  autumnYellow = "#dca561",
  samuraiRed = "#e82424",
  roninYellow = "#ff9e3b",
  waveAqua1 = "#6a9589",
  dragonBlue = "#658594",
  oldWhite = "#c8c093",
  fujiWhite = "#dcd7ba",
  fujiGray = "#727169",
  oniViolet = "#957fb8",
  oniViolet2 = "#b8b4d0",
  crystalBlue = "#7e9cd8",
  springViolet1 = "#938aa9",
  springViolet2 = "#9cabca",
  springBlue = "#7fb4ca",
  waveAqua2 = "#7aa89f",
  springGreen = "#98bb6c",
  boatYellow1 = "#938056",
  boatYellow2 = "#c0a36e",
  carpYellow = "#e6c384",
  sakuraPink = "#d27e99",
  waveRed = "#e46876",
  peachRed = "#ff5d62",
  surimiOrange = "#ffa066",
  katanaGray = "#717c7c",
}

return {
  name = "kanagawa-wave",
  appearance = "dark",
  palette = p,
  ui = {
    background = "sumiInk3", -- ui.bg
    foreground = "fujiWhite", -- ui.fg
    border = "sumiInk0", -- WinSeparator (ui.bg_m3)
    -- crystalBlue (syn.fun): Title and Directory.
    accent = {
      bg = "crystalBlue",
      fg = "sumiInk0",
      hover = color.mix(p.crystalBlue, p.fujiWhite, 0.15),
      active = color.mix(p.crystalBlue, p.sumiInk3, 0.25),
    },
    muted = { bg = "sumiInk4", fg = "springViolet1" }, -- ui.bg_p1, ui.special
    selection = "waveBlue1", -- Visual (ui.bg_visual)
    caret = "fujiWhite", -- Cursor
    overlay = color.alpha(p.sumiInk0, 0.7),
    button = {
      bg = "sumiInk4",
      fg = "fujiWhite",
      hover = "sumiInk5",
      active = "sumiInk6",
    },
    input = {
      bg = "sumiInk3",
      border = "sumiInk5",
      focused = { border = "crystalBlue" },
    },
    list = {
      hover = "sumiInk5", -- CursorLine (ui.bg_p2)
      selected = { bg = "waveBlue2", fg = "fujiWhite", border = "crystalBlue" }, -- PmenuSel
      even = color.mix(p.sumiInk3, p.sumiInk4, 0.4),
      header = { bg = "sumiInk2", fg = "oldWhite" },
    },
    popover = { bg = "sumiInk0", fg = "oldWhite", border = "sumiInk6" }, -- NormalFloat, FloatBorder
    title_bar = { bg = "sumiInk0", border = "sumiInk0" },
    status_bar = { bg = "sumiInk0", fg = "oldWhite", border = "sumiInk0" }, -- StatusLine
    sidebar = {
      bg = "sumiInk1", -- ui.bg_dim
      fg = "fujiWhite",
      border = "sumiInk0",
      hover = "sumiInk4",
      selected = { bg = "waveBlue2", fg = "fujiWhite" },
    },
    tab = {
      bar = "sumiInk3", -- TabLineFill
      bg = "sumiInk0", -- TabLine
      fg = "springViolet1",
      selected = { bg = "sumiInk4", fg = "oldWhite" }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = "waveBlue2", hover = "sumiInk6" } }, -- PmenuThumb
    link = { fg = "crystalBlue", hover = "springBlue", active = "waveAqua2" }, -- Directory
    info = "dragonBlue",
    success = "springGreen",
    warning = "roninYellow",
    error = "samuraiRed",
    diff = {
      added = { fg = "autumnGreen", bg = "winterGreen" }, -- diffAdded, DiffAdd
      removed = { fg = "autumnRed", bg = "winterRed" }, -- diffRemoved, DiffDelete
      hunk = "springBlue",
    },
    editor = {
      active_line = "sumiInk5", -- CursorLine
      line_number = "sumiInk6", -- LineNr (ui.nontext)
      active_line_number = "roninYellow", -- CursorLineNr
    },
    chart = { "crystalBlue", "carpYellow", "springBlue", "springGreen", "oniViolet" },
    -- The terminal colors of the wave theme.
    ansi = {
      red = "autumnRed",
      green = "autumnGreen",
      yellow = "boatYellow2",
      blue = "crystalBlue",
      magenta = "oniViolet",
      cyan = "waveAqua1",
    },
  },
  syntax = {
    comment = { fg = "fujiGray", italic = true },
    ["comment.doc"] = { fg = "fujiGray", italic = true },
    keyword = { fg = "oniViolet", italic = true },
    string = "springGreen",
    ["string.escape"] = { fg = "boatYellow2", bold = true },
    ["string.regex"] = "boatYellow2",
    ["string.special"] = "springBlue",
    ["string.special.symbol"] = "carpYellow",
    number = "sakuraPink",
    boolean = { fg = "surimiOrange", bold = true },
    constant = "surimiOrange",
    ["function"] = "crystalBlue",
    constructor = "springBlue",
    type = "waveAqua2",
    enum = "waveAqua2",
    variant = "surimiOrange",
    variable = "fujiWhite",
    ["variable.special"] = { fg = "waveRed", italic = true }, -- @variable.builtin
    property = "carpYellow", -- @variable.member (syn.identifier)
    attribute = "surimiOrange", -- @attribute links Constant
    label = "oniViolet",
    tag = "springBlue", -- Tag links Special
    operator = "boatYellow2",
    punctuation = "springViolet2",
    ["punctuation.bracket"] = "springViolet2",
    ["punctuation.delimiter"] = "springViolet2",
    ["punctuation.special"] = "springBlue",
    ["punctuation.list_marker"] = "springBlue",
    preproc = "waveRed",
    embedded = "fujiWhite",
    title = { fg = "crystalBlue", bold = true },
    link_text = "springBlue", -- Underlined
    link_uri = { fg = "springBlue", italic = false }, -- @string.special.url
    ["text.literal"] = "springGreen",
    ["text.code.span"] = "springGreen",
    emphasis = { italic = true },
    ["emphasis.strong"] = { bold = true, italic = false },
  },
}
