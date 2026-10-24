-- Rosé Pine for crest
-- Ported from https://github.com/rose-pine/neovim (lua/rose-pine/palette.lua, lua/rose-pine.lua, lua/rose-pine/config.lua @ ff483051a47e27d84bdef47703538df1ed9f4a47)
-- License: MIT, Copyright (c) 2023 Rosé Pine

-- The main variant.
local p = {
  _nc = "#16141f",
  base = "#191724",
  surface = "#1f1d2e",
  overlay = "#26233a",
  muted = "#6e6a86",
  subtle = "#908caa",
  text = "#e0def4",
  love = "#eb6f92",
  gold = "#f6c177",
  rose = "#ebbcba",
  pine = "#31748f",
  foam = "#9ccfd8",
  iris = "#c4a7e7",
  leaf = "#95b1ac",
  highlight_low = "#21202e",
  highlight_med = "#403d52",
  highlight_high = "#524f67",
}

-- Upstream `blend` mixes a color over base.
local function blend(c, amount)
  return color.mix(p.base, c, amount)
end

return {
  name = "rose-pine",
  appearance = "dark",
  palette = p,
  ui = {
    background = "base",
    foreground = "text",
    -- WinSeparator uses muted (groups.border); that is kept for popovers, and
    -- the softer highlight_med for the many borders inside the app.
    border = "highlight_med",
    -- Iris: links (groups.link), the Visual tint and hints.
    accent = {
      bg = "iris",
      fg = "base",
      hover = color.mix(p.iris, p.text, 0.15),
      active = color.mix(p.iris, p.base, 0.25),
    },
    muted = { bg = "surface", fg = "subtle" },
    selection = blend(p.iris, 0.15), -- Visual
    caret = "text", -- Cursor is highlight_high, too faint for a thin caret
    overlay = color.alpha(p._nc, 0.7),
    button = {
      bg = "overlay",
      fg = "text",
      hover = "highlight_med",
      active = "highlight_high",
    },
    input = {
      bg = "base",
      border = "highlight_med",
      focused = { border = "iris" },
    },
    list = {
      hover = "overlay", -- CursorLine
      selected = { bg = "overlay", fg = "text", border = "iris" }, -- PmenuSel
      even = color.mix(p.base, p.surface, 0.5),
      header = { bg = "surface", fg = "subtle" },
    },
    popover = { bg = "surface", fg = "text", border = "muted" }, -- NormalFloat (groups.panel), FloatBorder
    title_bar = { bg = "surface", border = "highlight_med" },
    status_bar = { bg = "surface", fg = "subtle", border = "highlight_med" }, -- StatusLine
    sidebar = {
      bg = "surface",
      fg = "text",
      border = "highlight_med",
      hover = "overlay",
      selected = { bg = "highlight_med", fg = "text" },
    },
    tab = {
      bar = "surface", -- TabLineFill
      bg = "surface", -- TabLine
      fg = "subtle",
      selected = { bg = "overlay", fg = "text" }, -- TabLineSel
    },
    scrollbar = { thumb = { bg = "muted", hover = "subtle" } }, -- PmenuThumb
    link = { fg = "iris", hover = "rose", active = "pine" },
    info = "foam",
    success = "leaf",
    warning = "gold",
    error = "love",
    diff = {
      added = { fg = "foam", bg = blend(p.foam, 0.2) }, -- DiffAdd (groups.git_add)
      removed = { fg = "love", bg = blend(p.love, 0.2) }, -- DiffDelete (groups.git_delete)
      hunk = "iris",
    },
    editor = {
      active_line = "overlay", -- CursorLine
      line_number = "muted", -- LineNr
      active_line_number = "text", -- CursorLineNr
    },
    chart = { "iris", "gold", "foam", "leaf", "rose" },
    ansi = { red = "love", green = "pine", yellow = "gold", blue = "foam", magenta = "iris", cyan = "rose" },
  },
  syntax = {
    comment = { fg = "subtle", italic = true },
    ["comment.doc"] = { fg = "subtle", italic = true },
    keyword = "pine",
    string = "gold",
    ["string.escape"] = "pine",
    ["string.regex"] = "iris",
    ["string.special"] = "gold",
    ["string.special.symbol"] = "text", -- links Identifier
    number = "gold",
    boolean = "rose",
    constant = "gold",
    ["function"] = "rose",
    constructor = "foam",
    type = "foam",
    enum = "foam",
    variant = "gold",
    variable = { fg = "text", italic = true },
    ["variable.special"] = { fg = "love", italic = true, bold = true }, -- @variable.builtin
    property = { fg = "foam", italic = true },
    attribute = "iris",
    label = "foam",
    tag = "foam",
    operator = "subtle",
    punctuation = "subtle",
    ["punctuation.bracket"] = "subtle",
    ["punctuation.delimiter"] = "subtle",
    ["punctuation.special"] = "subtle",
    ["punctuation.list_marker"] = "pine", -- @markup.list
    preproc = "iris", -- PreProc links PreCondit
    embedded = "text",
    title = { fg = "foam", bold = true },
    link_text = "iris",
    link_uri = { fg = "iris", italic = false }, -- @markup.link.url
    ["text.literal"] = "gold",
    ["text.code.span"] = "gold", -- @markup.raw.markdown_inline
    emphasis = { italic = true },
    ["emphasis.strong"] = { bold = true, italic = false },
  },
}
