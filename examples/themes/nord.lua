-- Nord for crest
-- Ported from https://github.com/nordtheme/vim (colors/nord.vim @ f13f5dfbb784deddbc1d8195f34dfd9ec73e2295)
-- License: MIT, Copyright (c) 2016-present Sven Greb <development@svengreb.de> (https://www.svengreb.de)

local p = {
  nord0 = "#2e3440",
  nord1 = "#3b4252",
  nord2 = "#434c5e",
  nord3 = "#4c566a",
  nord3_bright = "#616e88", -- the brightened nord3 used for comments
  nord4 = "#d8dee9",
  nord5 = "#e5e9f0",
  nord6 = "#eceff4",
  nord7 = "#8fbcbb",
  nord8 = "#88c0d0",
  nord9 = "#81a1c1",
  nord10 = "#5e81ac",
  nord11 = "#bf616a",
  nord12 = "#d08770",
  nord13 = "#ebcb8b",
  nord14 = "#a3be8c",
  nord15 = "#b48ead",
}

return {
  name = "nord",
  appearance = "dark",
  palette = p,
  ui = {
    background = "nord0",
    foreground = "nord4",
    border = "nord2", -- VertSplit
    -- nord8: Function, Directory and the StatusLine and TabLineSel text.
    accent = {
      bg = "nord8",
      fg = "nord0",
      hover = color.mix(p.nord8, p.nord6, 0.15),
      active = color.mix(p.nord8, p.nord0, 0.25),
    },
    -- No Nord color sits between nord3 and nord4, so secondary text is a mix.
    muted = { bg = "nord1", fg = color.mix(p.nord4, p.nord0, 0.42) },
    selection = "nord2", -- Visual
    caret = "nord4", -- Cursor
    overlay = color.alpha(p.nord0, 0.7),
    button = {
      bg = "nord1",
      fg = "nord4",
      hover = "nord2",
      active = "nord3",
    },
    input = {
      bg = "nord0",
      border = "nord3",
      focused = { border = "nord8" },
    },
    list = {
      hover = "nord1", -- CursorLine
      selected = { bg = "nord3", fg = "nord8", border = "nord8" }, -- PmenuSel
      even = color.mix(p.nord0, p.nord1, 0.4),
      header = { bg = "nord1", fg = "nord4" },
    },
    popover = { bg = "nord2", fg = "nord4", border = "nord3" }, -- Pmenu (NormalFloat links it)
    title_bar = { bg = "nord1", border = "nord2" },
    status_bar = { bg = "nord3", fg = "nord8", border = "nord2" }, -- StatusLine
    sidebar = {
      bg = "nord1",
      fg = "nord4",
      border = "nord2",
      hover = "nord2",
      selected = { bg = "nord3", fg = "nord6" },
    },
    tab = {
      bar = "nord1", -- TabLineFill
      bg = "nord1", -- TabLine
      fg = "nord4",
      selected = { bg = "nord3", fg = "nord8" }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = "nord3", hover = "nord3_bright" } }, -- PmenuThumb
    link = { fg = "nord8", hover = "nord7", active = "nord9" }, -- Directory
    info = "nord8",
    success = "nord14",
    warning = "nord13",
    error = "nord11",
    diff = {
      -- Upstream draws diffs in reverse video (nord0 on the color); these are tints.
      added = { fg = "nord14", bg = color.mix(p.nord0, p.nord14, 0.15) },
      removed = { fg = "nord11", bg = color.mix(p.nord0, p.nord11, 0.15) },
      hunk = "nord9", -- DiffText
    },
    editor = {
      active_line = "nord1", -- CursorLine
      line_number = "nord3", -- LineNr
      active_line_number = "nord4", -- CursorLineNr
    },
    chart = { "nord8", "nord13", "nord10", "nord14", "nord15" },
    ansi = { red = "nord11", green = "nord14", yellow = "nord13", blue = "nord9", magenta = "nord15", cyan = "nord8" },
  },
  syntax = {
    comment = { fg = "nord3_bright", italic = false }, -- nord_italic_comments is off by default
    ["comment.doc"] = { fg = "nord3_bright", italic = false },
    keyword = "nord9",
    string = "nord14",
    ["string.escape"] = "nord13", -- SpecialChar
    ["string.regex"] = "nord13",
    ["string.special"] = "nord13",
    ["string.special.symbol"] = "nord13",
    number = "nord15",
    boolean = "nord9",
    constant = "nord4",
    ["function"] = "nord8",
    -- Type is nord9, but the language groups (typescriptClassName, ...) draw
    -- class and type names in nord7, which keeps them apart from keywords.
    constructor = "nord7",
    type = "nord7",
    enum = "nord7",
    variant = "nord4",
    variable = "nord4",
    ["variable.special"] = "nord9", -- jsThis links Keyword
    property = "nord7", -- jsonKeyword, yamlKey, htmlArg
    attribute = "nord12", -- Decorator
    label = "nord9",
    tag = "nord9", -- htmlTag links Keyword
    operator = "nord9",
    punctuation = "nord6", -- Delimiter
    ["punctuation.bracket"] = "nord6",
    ["punctuation.delimiter"] = "nord6",
    ["punctuation.special"] = "nord9",
    ["punctuation.list_marker"] = "nord9", -- markdownListMarker links Keyword
    preproc = "nord9",
    embedded = "nord4",
    title = { fg = "nord8", bold = true }, -- markdownH1
    link_text = "nord8", -- markdownLinkText
    link_uri = { fg = "nord4", italic = false }, -- markdownUrl
    ["text.literal"] = "nord7", -- markdownCode
    ["text.code.span"] = "nord7",
    emphasis = { italic = true },
    ["emphasis.strong"] = { bold = true, italic = false },
  },
}
