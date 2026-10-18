# Writing plugins

A plugin is an `init.lua` that uses the `app` table to add things to crest:

- tools the model can call;
- hooks around every tool;
- `:` commands;
- key mappings;
- `Custom` blocks in the timeline;
- MCP servers.

The global config and the workspace config use the same API. The declarative options (`app.opt`, `app.models`, permissions) are in [configuration.md](configuration.md).

## Where they live and load order

```text
~/.config/crest/                 ($XDG_CONFIG_HOME/crest/)
  init.lua                       global config
  plugins/
    guardia/init.lua             plugin "guardia"
    todo/init.lua                plugin "todo"
<workspace>/.crest/init.lua      workspace config
```

1. The global config runs first.
2. Then each `plugins/<name>/init.lua`, in alphabetical order by folder name.
3. Then the folders listed in `app.plugins` in the global config that were not already in `plugins/`. Each entry may start with `~/`; otherwise it is resolved relative to the global config directory. The `init.lua` of each folder is loaded.
4. Finally the workspace config runs.

Plugins are only loaded from the global config: an `app.plugins` in the workspace is ignored, with a notice. Even so, the workspace's `init.lua` can use the whole API (tools, hooks, keymaps, commands), because it is just another layer. When things clash, precedence is workspace > global config > plugins (see [configuration.md](configuration.md#load-order-and-precedence)).

A plugin's name is its folder's name. Tools, hooks and blocks are attributed to that name, and the global and workspace configs appear as `config` and `workspace`.

Each file runs **once at startup**. The functions it registers (handlers, hooks, callbacks) stay alive in the VM and are called later. If you change a plugin, reopen crest.

## The sandbox

All Lua (the config and the plugins) runs in a single Lua 5.4 VM. That VM lives on its own thread, `crest-lua`, and serves calls one at a time.

| Available | Not available |
| --- | --- |
| `string` (without `string.dump`), `table`, `math`, `utf8`, `coroutine` | `io`, `debug`, `package` |
| `os.time`, `os.clock`, `os.date`, `os.difftime` | `os.execute`, `os.remove`, `os.rename`, `os.exit`, `os.getenv`, `os.tmpname`, `os.setlocale` |
| `load` with text only | `dofile`, `loadfile`, bytecode (a binary `init.lua` is rejected) |
| `require("app")`, which returns the file's `app` table | `require` of any other module or path |
| `print(...)`, which goes to the log (`CREST_LOG=warn,lua=info` to see it) | Network, shell and file system |

There are two limits:

- **Instructions**: each call (loading a file, a handler, a hook, a callback) can execute at most **100,000,000** VM instructions. Beyond that it is cut off with the error `instruction limit exceeded`. `pcall`, `xpcall` and `coroutine.resume` rethrow that error, so a plugin cannot catch it and keep looping.
- **Memory**: the whole VM is capped at **256 MiB**. Exceeding it raises `out of memory`.

Each file has its own environment: its global variables are not visible from other files. Within a single file, `local` variables captured by your functions do persist between calls. The `app` table belongs to each file, so reading `app.opt.leader` only returns what that file set.

## The `app` table

| Field | Signature | When to use it |
| --- | --- | --- |
| `app.opt` | table | Options (see [configuration.md](configuration.md#appopt-options)). |
| `app.models.roles`, `app.models.aliases` | tables | Models by role. |
| `app.permissions.sensitive_paths` | list of strings | Paths the tools cannot read. |
| `app.plugins` | list of strings | Extra plugins (global config only). |
| `app.keymap.set(mode, keys, action)` | see below | At load time. |
| `app.commands.register(name, fn)` | see below | At load time. |
| `app.tools.register(def)` | see below | At load time. |
| `app.on(event, fn)` | see below | At load time. |
| `app.mcp` / `app.mcp.add(name, def)` | see below | At load time. |
| `app.cmd(name)` | `string → nil` | Inside a keymap or command callback. |
| `app.send(text [, { role = ... }])` | `string, table? → nil` | Inside a keymap or command callback. |
| `app.ui.*` | node constructors | Inside a tool handler (or a callback). |

A malformed argument to `keymap.set`, `commands.register`, `tools.register`, `app.on` or `mcp.add` does **not** stop the file: it produces a diagnostic with the line of the call, and only that call is ignored (see [Errors and diagnostics](#errors-and-diagnostics)).

### `app.keymap.set(mode, keys, action)`

```lua
app.keymap.set("n", "<leader>r", "picker.grep")               -- builtin action
app.keymap.set({ "n", "i" }, "<C-x>", function() app.cmd("clear") end)
```

- **`mode`**: a mode or a list of modes. Each mode can be `"n"`/`"normal"`, `"i"`/`"insert"` or `"c"`/`"command"`.
- **`keys`**: the sequence in Neovim notation:
  - single characters (`gg`);
  - `<leader>`;
  - modifiers `<C-x>` (Ctrl), `<A-x>` or `<M-x>` (Alt), `<S-x>` (Shift), `<D-x>` (Cmd);
  - key names `<CR>`/`<Enter>`, `<Esc>`, `<Tab>`, `<BS>`, `<Del>`, `<Space>`, `<lt>`, `<gt>`, `<Bar>`, `<Bslash>`, `<Up>`, `<Down>`, `<Left>`, `<Right>`, `<Home>`, `<End>`, `<PageUp>`, `<PageDown>` and `<F1>`–`<F24>`.

  `<leader>` expands to the current `app.opt.leader` when the keymap is applied.
- **`action`**: the name of a builtin action or a Lua function. The function is called with no arguments (`nil`) and can use `app.cmd` and `app.send`. If it returns `app.ui.block(...)`, that block is added to the timeline.

A keymap replaces any other with the same keys in the same mode, builtin ones included. These are the builtin actions valid as a string:

| Action | What it does |
| --- | --- |
| `mode.normal`, `mode.insert`, `mode.command` | Changes mode. |
| `submit`, `newline` | Sends the prompt / inserts a new line (insert). |
| `block.next`, `block.prev`, `block.first`, `block.last` | Moves the timeline selection. |
| `block.toggle` | Expands or collapses the selected block. |
| `block.toggle_view` | Alternate view (chart ↔ table, document ↔ source). |
| `block.yank` | Copies the block to the clipboard. |
| `picker.files`, `picker.grep` | Opens a picker. |
| `panel.tree` | Shows or hides the tree. |
| `model.pick` | Model menu for the next turn. |
| `permission.cycle` | Cycles the permission mode. |
| `turn.interrupt` | Interrupts the turn. |

Any other name is accepted at load time, but pressing it only shows «la acción … todavía no hace nada». For example, `session.resume` is not implemented.

### `app.commands.register(name, fn)`

```lua
app.commands.register("review", function(args)
  app.send("Review the uncommitted changes. " .. args, { role = "plan" })
end)
```

- The command is used as `:review solo src/`, and `fn` receives the rest of the line as a string (`"solo src/"`, or `""` if there is nothing).
- If the name starts with `/`, that slash is ignored.
- The builtin commands `q`, `quit`, `qa`, `model`, `mode`, `clear`, `implement`, `research` and `mcp` are evaluated first, so a command with one of those names never runs.
- As with keymaps, if `fn` returns `app.ui.block(...)`, the block is shown. Any other return value is ignored.

### `app.cmd(name)` and `app.send(text, opts)`

They only have an effect inside a keymap or command callback. Inside a tool handler or a hook they are silently dropped. The actions accumulate in order, and the window runs them when the callback finishes.

- **`app.cmd("clear")`** runs a `:` command exactly as if you typed it: a builtin one, a registered one, or a dotted name, which runs that action (`app.cmd("picker.files")`). A command can call another up to 4 levels of nesting; beyond that it is ignored with a notice.
- **`app.send(text, { role = "plan" })`** sends `text` as the prompt of a new turn, with that model role (`main`, `plan`, `explore`, `compact`, `title`, `component` or `deep`). Without `role`, the turn uses `main`. If a turn is already running, the prompt is queued. An unknown role is a Lua error.

### `app.tools.register(def)`

```lua
app.tools.register({
  name = "todo_list",                 -- required, ^[a-zA-Z0-9_-]{1,128}$
  description = "Shows the project's pending tasks",
  schema = { type = "object", properties = { items = { type = "array" } } },
  effect = "read",                    -- "read" | "write" | "execute" (default) | "network"
  handler = function(input) ... end,  -- required
})
```

| Field | Type | Notes |
| --- | --- | --- |
| `name` | string | The name under which the model sees the tool. It cannot reuse the name of a builtin tool (`read_file`, `grep`, `show_chart`, `task`...): such a tool is ignored, with the notice «tool de plugin `x` ignorada». Across Lua layers, the highest-precedence one wins. |
| `description` | string | What the model reads to decide when to use it. |
| `schema` | JSON Schema table | The input it expects. Defaults to `{ type = "object" }`. The model's input must always be an object. |
| `effect` | string | What the permission policy uses (see the table in [configuration.md](configuration.md#permissions-and-sensitive-paths)). **It defaults to `"execute"`**, so the tool asks for approval in `ask` and `auto-edit` and is denied in `plan`. A read-only tool should declare `effect = "read"`. |
| `handler` | function | Receives the model's input as a table. |

The approval dialog shows `plugin `<plugin>`: <tool> <input JSON>`, and "always allow" applies to that exact input.

**Handler return value**:

| Returns | The model gets | The user sees |
| --- | --- | --- |
| `nil` | `(no output)` | The ToolCall block. |
| a string | the string as is | The ToolCall, with the text when expanded. |
| a number, boolean or table | its JSON | Same. |
| `app.ui.block(name, node)` | `[shown to the user as a block]` | A `Custom` block in the timeline. |
| a Lua error (`error("...")`) or the instruction limit | an error (`is_error`) with the message | The ToolCall in the failed state. |

The result goes through the same budget as the builtin tools (about 32 KB of text). If it is longer, it is truncated with a note.

### `app.on(event, fn)`: hooks

There are two events: `"before_tool"` and `"after_tool"`. Each hook applies to **every** tool: builtin, plugin, MCP and `task`. Several hooks on the same event run in the order the files were loaded.

**`before_tool`** receives `{ type = "before_tool", name = <tool>, input = <table> }` and can return:

| Return | Effect |
| --- | --- |
| `nil` (or anything other than the two below) | Continues unchanged. |
| `{ deny = true, reason = "..." }` | The tool does not run. The model gets `denied by hook: <reason>` and the ToolCall is marked as denied. The remaining hooks do not run. |
| `{ input = { ... } }` | The tool runs with this input, which **entirely replaces** the original. The remaining hooks already see the new input. |

The hook runs **before** the permission policy and the approval. A rewritten input is shown and approved as if the model had sent it. A hook cannot skip an approval: it can only deny or change the input.

**`after_tool`** receives `{ type = "after_tool", name, input, output = { text = "...", is_error = bool } }` once the tool has finished (also if it failed). Its return value is ignored. Denied calls, whether by a hook, by the policy or by the user, never reach `after_tool`.

If a hook raises an error or exhausts the instruction limit, the notice «before_tool hook failed for `x`: …» appears and the call continues with its original input.

These are the builtin tool inputs most often filtered (the full reference is in [usage.md](usage.md#agent-tools)):

```text
run_command  { command = "git", args = { "push" }, timeout_s = 120 }   -- no shell: command is the program
edit_file    { path = "src/a.rs", old = "...", new = "..." }
write_file   { path = "notas.md", content = "..." }
read_file    { path = "src/a.rs", offset = 1, limit = 2000 }
```

### `app.ui`: Custom blocks

The constructors return plain tables that describe nodes. Lua never touches GPUI: the window turns the tree into components. Wherever a node is expected, a string or a number becomes a text node.

| Constructor | Node | Details |
| --- | --- | --- |
| `app.ui.text(s)` | text | `s` is a string or a number (not a table). |
| `app.ui.row(children)` | horizontal row | `children` is a list of nodes or strings. |
| `app.ui.col(children)` | vertical column | Same. |
| `app.ui.list(items)` | bulleted list | Same. |
| `app.ui.table({ columns = ..., rows = ... })` | table | `columns` is a non-empty list of names or of `{ name = "x", align = "left" \| "center" \| "right" }`. `rows` is a list of rows, and each row is a list of scalar cells (string, number, boolean). Every row must have one cell per column. |
| `app.ui.chart({ title, kind, labels, series })` | chart | `kind` is `"bar"` (default), `"line"`, `"area"` or `"pie"`. `labels` is a list of strings and `series` is `{ { name = "x", values = { 1, 2 } }, ... }`. At least one series is required, each series has one finite value per label, and `pie` takes exactly one series. |
| `app.ui.code(lang, src)` | highlighted code | `lang` can be `nil`. |
| `app.ui.markdown(src)` | rendered markdown | |
| `app.ui.block(name, node)` | the `Custom` block | Validates the whole tree and wraps it as the plugin's block. It must be **returned** from the handler. |

An invalid argument (a table row of a different width, a series of a different length...) raises a Lua error prefixed with `app.ui.<function>:`, and the tool fails with that message. If you nest deeper than the window draws, the excess is cut off with a marker.

### `app.mcp`: MCP servers

There are two equivalent ways to declare them:

```lua
app.mcp = {
  git  = { command = "mcp-git", args = { "--repo", "." }, env = { RUST_LOG = "warn" } },
  docs = { url = "https://example.test/mcp", headers = { Authorization = "Bearer ..." } },
}
-- or, one at a time:
app.mcp.add("git", { command = "mcp-git", effect = "read", timeout = 30 })
```

| Field | For | Notes |
| --- | --- | --- |
| `command` | stdio | A child process that speaks MCP over stdin/stdout. |
| `args` | stdio | List of strings. |
| `env` | stdio | Table of strings. |
| `url` | HTTP | Streamable HTTP. Must start with `http://` or `https://`. |
| `headers` | HTTP | Table of strings. |
| `effect` | both | Same as for plugin tools. Defaults to `"execute"`. Applies to **all** of the server's tools. |
| `timeout` | both | Seconds (> 0) per call. Defaults to 120. |

- Each server has `command` **or** `url`, never both.
- An unknown field produces a diagnostic.
- The name can only contain letters, digits, and single `-` and `_` (not `__`), because the tools are exposed as `mcp__<server>__<tool>`.
- If one file uses both forms with the same name, `app.mcp.add` wins.

**Security**:

- Servers from the global config and from plugins start when crest opens.
- Workspace servers do not start until the user approves them with `crest --trust-mcp <name>` or `:mcp trust <name>` (see [usage.md](usage.md#mcp-servers-and-trust)). The approval is required again if the declaration changes.

## Errors and diagnostics

| Problem | What happens |
| --- | --- |
| Syntax or runtime error while loading a file | That file contributes nothing and the others load. An `Error` block is shown with `file:line: message`. |
| Infinite loop or excess memory while loading | As above, with `instruction limit exceeded` or `out of memory`. |
| Invalid argument to a registration function | Diagnostic with the line of the call. Only that call is ignored. |
| Invalid declarative field (`app.opt.x`, `app.models...`) | Diagnostic without a line. Only that field is ignored. |
| `app.plugins` folder without an `init.lua` | Diagnostic «cannot read the file». |
| Error in a tool handler | The model gets the error and the ToolCall is marked failed. |
| Error in a hook | Notice in the status bar. The tool continues. |
| Error in a keymap or command callback | `Error` block with `Lua: <message>`. |
| Keymap with invalid notation | Notice «keymap `x` ignorado: …» when the window opens. |

In headless mode the diagnostics go to the terminal. Run `crest --headless "hola"` to check that your config loads cleanly.

## Step-by-step examples

All four examples were loaded with the current implementation (a real `LuaHost`, no diagnostics), and the response of each handler, hook and callback was checked.

### 1. A `todo_list` tool with a Custom block

`~/.config/crest/plugins/todo/init.lua`:

```lua
local app = require("app")

app.tools.register({
  name = "todo_list",
  description = "Shows the user a list of pending tasks with their status. "
    .. "Use it when the user asks to see or summarize what is pending.",
  schema = {
    type = "object",
    properties = {
      items = {
        type = "array",
        items = {
          type = "object",
          properties = {
            tarea = { type = "string" },
            hecha = { type = "boolean" },
          },
          required = { "tarea" },
        },
      },
    },
    required = { "items" },
  },
  effect = "read",  -- only displays data: runs without asking for approval
  handler = function(input)
    local rows, hechas = {}, 0
    for i, item in ipairs(input.items) do
      local hecha = item.hecha == true
      if hecha then hechas = hechas + 1 end
      rows[i] = { item.tarea, hecha }
    end
    return app.ui.block("todo", app.ui.col({
      app.ui.text(string.format("%d of %d done", hechas, #input.items)),
      app.ui.table({ columns = { "task", { name = "done", align = "center" } }, rows = rows }),
      app.ui.chart({
        title = "Progress",
        kind = "pie",
        labels = { "done", "pending" },
        series = { { name = "tasks", values = { hechas, #input.items - hechas } } },
      }),
    }))
  end,
})
```

(`tarea` and `hecha` are the schema's field names, Spanish for "task" and "done".)

To try it, open crest and ask: "use todo_list to show the tasks: parser (done), docs, release". The timeline shows the `todo_list` ToolCall and, below it, the block with the text "1 of 3 done", the table and the pie chart. The model only gets `[shown to the user as a block]`.

### 2. A hook that blocks `rm` and forces `git push --dry-run`

`~/.config/crest/plugins/guardia/init.lua`:

```lua
local app = require("app")

local prohibidos = { rm = true, shred = true }

app.on("before_tool", function(ev)
  if ev.name ~= "run_command" then return nil end
  if prohibidos[ev.input.command] then
    return { deny = true, reason = "`" .. ev.input.command .. "` is blocked by the guardia plugin" }
  end
  -- git push always as a dry run
  if ev.input.command == "git" and ev.input.args and ev.input.args[1] == "push" then
    local args = { "push", "--dry-run" }
    for i = 2, #ev.input.args do args[#args + 1] = ev.input.args[i] end
    return { input = { command = "git", args = args, timeout_s = ev.input.timeout_s } }
  end
end)

app.on("after_tool", function(ev)
  if ev.output.is_error then
    print("failed " .. ev.name .. ": " .. ev.output.text)
  end
end)
```

This is how the hook responds to each input:

| Input | Result |
| --- | --- |
| `{ command = "rm", args = { "-rf", "x" } }` | Denied with the reason "`rm` is blocked by the guardia plugin". |
| `{ command = "git", args = { "push", "origin", "main" } }` | Runs as `git push --dry-run origin main`, and the approval shows that command. |
| `{ command = "ls" }` | Continues unchanged. |

To try it, ask "delete the target directory with rm -rf". The `run_command` ToolCall is denied without the approval dialog appearing. To see the `print` output, start with `CREST_LOG=warn,lua=info`.

Remember that `run_command` uses no shell: `command` is the program and `args` its arguments, so an `sh -c "rm …"` arrives as `command = "sh"`. If you want to block that too, inspect `ev.input.args`.

### 3. A command that starts a turn with the `plan` role

In `init.lua` (global or workspace):

```lua
local app = require("app")

app.commands.register("review", function(args)
  app.send("Review the uncommitted changes and list the risks. " .. args, { role = "plan" })
end)
```

To use it, press `Esc`, type `:review solo src/` and press `Enter`. This sends the prompt "Review the uncommitted changes and list the risks. solo src/", which is served by the `plan` role's model (`claude-opus-5-5` by default). The status bar shows the turn's model.

### 4. Keymaps

```lua
local app = require("app")

app.opt.leader = ","

-- ,x in normal: new session
app.keymap.set("n", "<leader>x", function() app.cmd("clear") end)

-- Ctrl+P in normal and insert: file picker
app.keymap.set({ "n", "i" }, "<C-p>", "picker.files")

-- ,t in normal: ask the agent to run the tests
app.keymap.set("n", "<leader>t", function()
  app.send("Run the workspace tests and summarize the failures in a table")
end)
```

With `leader = ","`, the mappings in this file use the comma, but the builtin ones (`<Space>ff`, `<Space>m`...) keep using space. If you want to move them, map them again:

```lua
app.keymap.set("n", "<leader>ff", "picker.files")
app.keymap.set("n", "<leader>fg", "picker.grep")
```

### Testing a plugin

1. Start headless on any directory: `crest --headless "list the files" --workspace /tmp/prueba`. Load diagnostics are printed to the terminal.
2. Open the window and check that there are no `Error` blocks at the top of the timeline.
3. For tools: ask for something that uses them, expand the ToolCall with `Enter` (in normal mode) and check the input and the output.
4. For hooks: use `print` and `CREST_LOG=warn,lua=info`.
5. If you write Rust, the tests in `crates/adapters/lua/tests/` (`plugin_tools.rs`, `plugin_hooks.rs`, `plugin_ui.rs`) show how to load an `init.lua` into a real `LuaHost` and run it inside the agent loop with a scripted model (`application::testing`).
