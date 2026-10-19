-- showcase: a tour of what a crest plugin can do, in one file.
--
-- crest is a work in progress, and so is this plugin: it exists to show
-- the extension points, not to be useful on its own.
--
-- Install it by adding its folder to your global config
-- (~/.config/crest/init.lua):
--
--   app.plugins = { "~/path/to/crest/examples/plugins/showcase" }
--
-- or by copying the folder to ~/.config/crest/plugins/showcase. Then:
--
--   /demo             a modal with every kind of node
--   /demo chart       one node per modal: chart, table, diagram, json,
--   /demo table       diff, code, html
--   /demo block       the same dashboard as a block in the timeline
--   /demo stats       what the hooks counted so far
--   /demo ask         sends a prompt (with the `plan` role)
--   Ctrl+G d          opens the tour (the <leader>d keymap)
--
-- The model can call the `demo_dashboard` tool too ("show me the demo
-- dashboard"). Nothing here needs a capability: no network, no programs.

local app = require("app")

local WIP = "> **Work in progress.** crest is under active development: "
  .. "expect rough edges and breaking changes between releases."

-- Nodes ---------------------------------------------------------------

local function chart()
  return app.ui.chart({
    title = "Weekly active sessions",
    kind = "line",
    labels = { "W1", "W2", "W3", "W4", "W5", "W6" },
    series = {
      { name = "chat", values = { 12, 19, 23, 31, 38, 44 } },
      { name = "research", values = { 2, 3, 6, 5, 9, 12 } },
    },
  })
end

local function table_node()
  -- app.ui.csv parses CSV into a sortable table; numbers sort as numbers.
  return app.ui.csv([[
feature,crate,status,tests
3D model viewer,ui,shipped,31
decision model (Clef),app,shipped,27
prompt history,ui,shipped,9
desktop launcher,bin,planned,0
]])
end

local function diagram()
  return app.ui.mermaid([[
flowchart LR
  P[Your prompt] --> T{Tools?}
  T -->|files| F[read, grep, edit]
  T -->|plugin| X[demo_dashboard]
  F --> A[Answer with blocks]
  X --> A
]])
end

local function json_node()
  return app.ui.json({
    plugin = "showcase",
    status = "wip",
    extension_points = { "commands", "tools", "keymaps", "hooks", "blocks", "modals" },
    sandbox = { instructions = 100000000, memory_mib = 256 },
  }, { expand = 2 })
end

local function diff()
  return app.ui.diff([[
--- a/greeting.lua
+++ b/greeting.lua
@@ -1,3 +1,3 @@
 local function greet(name)
-  return "hello " .. name
+  return ("Hello, %s!"):format(name)
 end
]])
end

local function code()
  return app.ui.code("lua", [[
app.commands.register("hello", function(args)
  return app.ui.block("hello", app.ui.markdown("# Hello, " .. args))
end, { desc = "Say hello" })]])
end

local function html()
  return app.ui.html([[
<h3>Plain HTML works too</h3>
<p>Headings, <b>emphasis</b>, <code>code</code>, lists and tables, drawn natively:</p>
<ul><li>no scripts</li><li>no CSS layout</li><li>links open the browser</li></ul>
]])
end

local function dashboard()
  return app.ui.col({
    app.ui.markdown("## showcase dashboard\n\n" .. WIP),
    app.ui.row({ chart(), json_node() }),
    table_node(),
  })
end

local function tour()
  return app.ui.col({
    app.ui.markdown("# What a plugin can show\n\n" .. WIP
      .. "\n\nEverything in this window comes from `examples/plugins/showcase/init.lua`."),
    app.ui.row({ chart(), json_node() }),
    table_node(),
    diagram(),
    app.ui.row({ diff(), code() }),
    html(),
    app.ui.link("Read the plugin guide", "https://github.com/lazaroMB/crest/blob/main/docs/plugins.md"),
  })
end

-- Hooks: count every tool call (they only observe) ----------------------

local calls = { total = 0, by_tool = {} }

app.on("after_tool", function(ev)
  calls.total = calls.total + 1
  calls.by_tool[ev.name] = (calls.by_tool[ev.name] or 0) + 1
end)

-- Command -------------------------------------------------------------

local single = {
  chart = { "Chart", chart },
  table = { "Table (from CSV)", table_node },
  diagram = { "Mermaid diagram", diagram },
  json = { "JSON tree", json_node },
  diff = { "Diff", diff },
  code = { "Highlighted code", code },
  html = { "Basic HTML", html },
}

app.commands.register("demo", function(args)
  local what = (args or ""):match("^%s*(%S*)")
  if what == "" or what == "tour" then
    app.modal.open({ id = "tour", title = "showcase · tour (WIP)", width = 85, height = 85, content = tour() })
  elseif single[what] then
    local title, build = single[what][1], single[what][2]
    app.modal.open({ id = what, title = "showcase · " .. title, width = "60%", height = "70%", content = build() })
  elseif what == "block" then
    return app.ui.block("dashboard", dashboard())
  elseif what == "stats" then
    app.modal.open({
      id = "stats", title = "showcase · tool calls seen by the hook",
      width = 40, height = 50, position = "bottom-right",
      content = app.ui.json(calls, { expand = 2 }),
    })
  elseif what == "ask" then
    app.send("Explain the architecture of this project with a Mermaid diagram.", { role = "plan" })
  else
    return app.ui.block("help", app.ui.markdown(
      "Unknown section `" .. what .. "`. Try `/demo`, `/demo chart`, `/demo table`, "
        .. "`/demo diagram`, `/demo json`, `/demo diff`, `/demo code`, `/demo html`, "
        .. "`/demo block`, `/demo stats` or `/demo ask`."))
  end
end, { desc = "Tour of what plugins can show (WIP)" })

-- Keymap --------------------------------------------------------------

app.keymap.set("<leader>d", function() app.cmd("demo") end, { desc = "Open the showcase tour" })

-- Tool ----------------------------------------------------------------

app.tools.register({
  name = "demo_dashboard",
  description = "Shows the showcase plugin's demo dashboard (a chart, a JSON tree and a table) "
    .. "as a block in the timeline. Use it when the user asks for the demo dashboard.",
  schema = { type = "object", properties = {} },
  effect = "read",
  handler = function()
    return app.ui.block("dashboard", dashboard())
  end,
})
