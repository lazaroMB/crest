-- Dracula for crest
-- Ported from https://github.com/dracula/vim (autoload/dracula.vim, colors/dracula_base.vim @ 2cf60539da5bea64be8adf8a73a103e6e5f7f12c)
-- License: MIT, Copyright (c) 2016 Dracula Theme

local p = {
  fg = "#f8f8f2",
  bglighter = "#424450",
  bglight = "#343746",
  bg = "#282a36",
  bgdark = "#21222c",
  bgdarker = "#191a21",
  comment = "#6272a4",
  selection = "#44475a",
  subtle = "#424450",
  cyan = "#8be9fd",
  green = "#50fa7b",
  orange = "#ffb86c",
  pink = "#ff79c6",
  purple = "#bd93f9",
  red = "#ff5555",
  yellow = "#f1fa8c",
}

return {
  name = "dracula",
  appearance = "dark",
  palette = p,
  ui = {
    background = "bg",
    foreground = "fg",
    -- WinSeparator draws comment on bgdark; comment is kept for popovers and
    -- the softer bglighter for the many borders inside the app.
    border = "bglighter",
    -- Purple: WildMenu and Directory.
    accent = {
      bg = "purple",
      fg = "bg",
      hover = color.mix(p.purple, p.fg, 0.15),
      active = color.mix(p.purple, p.bg, 0.25),
    },
    muted = { bg = "bglight", fg = "comment" },
    selection = "selection", -- Visual
    caret = "fg",
    overlay = color.alpha(p.bgdarker, 0.7),
    button = {
      bg = "bglight",
      fg = "fg",
      hover = "bglighter",
      active = "selection",
    },
    input = {
      bg = "bg",
      border = "bglighter",
      focused = { border = "purple" },
    },
    list = {
      hover = "subtle", -- CursorLine
      selected = { bg = "selection", fg = "fg", border = "purple" }, -- PmenuSel
      even = color.mix(p.bg, p.bglight, 0.5),
      header = { bg = "bgdark", fg = "fg" },
    },
    popover = { bg = "bgdark", fg = "fg", border = "comment" }, -- NormalFloat links Pmenu
    title_bar = { bg = "bgdark", border = "bgdarker" },
    status_bar = { bg = "bglighter", fg = "fg", border = "bgdarker" }, -- StatusLine
    sidebar = {
      bg = "bgdark",
      fg = "fg",
      border = "bgdarker",
      hover = "bglight",
      selected = { bg = "selection", fg = "fg" },
    },
    tab = {
      bar = "bgdark", -- TabLineFill
      bg = "bgdark", -- TabLine
      fg = "comment",
      selected = { bg = "bg", fg = "fg" }, -- TabLineSel links Normal
    },
    scrollbar = { thumb = { bg = "selection", hover = "comment" } }, -- PmenuThumb
    link = { fg = "cyan", hover = "purple", active = "pink" }, -- DraculaLink
    info = "cyan",
    success = "green",
    warning = "orange", -- DiagnosticWarn
    error = "red",
    diff = {
      -- Upstream colors diff lines by their text only; the backgrounds are tints.
      added = { fg = "green", bg = color.mix(p.bg, p.green, 0.12) },
      removed = { fg = "red", bg = color.mix(p.bg, p.red, 0.12) },
      hunk = "orange", -- DiffChange
    },
    editor = {
      active_line = "subtle", -- CursorLine
      line_number = "comment", -- LineNr
      active_line_number = "yellow", -- CursorLineNr
    },
    chart = { "purple", "orange", "cyan", "green", "pink" },
    -- The terminal colors: blue is purple and magenta is pink.
    ansi = { red = "red", green = "green", yellow = "yellow", blue = "purple", magenta = "pink", cyan = "cyan" },
  },
  syntax = {
    comment = { fg = "comment", italic = false },
    ["comment.doc"] = { fg = "comment", italic = false },
    keyword = "pink",
    string = "yellow",
    ["string.escape"] = "pink", -- links SpecialChar, then Special
    ["string.regex"] = "pink",
    ["string.special"] = "pink",
    ["string.special.symbol"] = "purple",
    number = "purple",
    boolean = "purple",
    constant = "purple",
    ["function"] = "green",
    constructor = "cyan",
    type = { fg = "cyan", italic = true },
    enum = "cyan",
    variant = "purple", -- @lsp.type.enumMember
    variable = "fg",
    ["variable.special"] = { fg = "purple", italic = true }, -- @variable.builtin
    property = "orange", -- @variable.member (@property itself is plain fg)
    attribute = { fg = "green", italic = true },
    label = { fg = "purple", italic = true },
    tag = "cyan",
    operator = "pink",
    punctuation = "fg",
    ["punctuation.bracket"] = "fg",
    ["punctuation.delimiter"] = "fg",
    ["punctuation.special"] = "pink",
    ["punctuation.list_marker"] = "pink", -- @markup.list links Special
    preproc = "pink",
    embedded = "fg",
    title = { fg = "yellow", bold = false }, -- @markup.heading
    link_text = "fg", -- @markup.link links Underlined
    link_uri = { fg = "yellow", italic = false }, -- @markup.link.uri
    ["text.literal"] = "yellow",
    ["text.code.span"] = "yellow",
    emphasis = { italic = true },
    ["emphasis.strong"] = { bold = true, italic = false },
  },
}
