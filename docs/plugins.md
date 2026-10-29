# Writing plugins

A plugin is an `init.lua` that uses the `app` table to add things to crest:

- tools the model can call;
- hooks around every tool;
- `:` commands;
- key mappings;
- `Custom` blocks in the timeline;
- [modal windows](#appmodal-modal-windows) over crest's window, with images, tables, charts, JSON, diagrams and documents;
- MCP servers;
- HTTP requests, programs and secrets, through [capabilities](#capabilities) the user approves.

The global config and the workspace config use the same API. The declarative options (`app.opt`, `app.models`, permissions) are in [configuration.md](configuration.md).

> **A runnable tour:** [`examples/plugins/showcase/init.lua`](../examples/plugins/showcase/init.lua) uses every extension point below (a command, a tool, a keymap, a hook, `Custom` blocks and modals with every kind of node). Add its folder to `app.plugins` and type `/demo`.

## Where they live and load order

```text
~/.config/crest/                 ($XDG_CONFIG_HOME/crest/)
  init.lua                       global config
  plugins/
    guard/init.lua             plugin "guard"
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
| `print(...)`, which goes to the log (`CREST_LOG=warn,lua=info` to see it) | File system; network and programs except through approved [capabilities](#capabilities) |

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
| `app.keymap.set(keys, action [, { desc }])` | see below | At load time. |
| `app.keymap.del(keys)`, `app.keymap.unmap(keys)` | `string → nil` (see below) | At load time. |
| `app.commands.register(name, fn [, { desc }])` | see below | At load time. |
| `app.tools.register(def)` | see below | At load time. |
| `app.on(event, fn)` | see below | At load time. |
| `app.mcp` / `app.mcp.add(name, def)` | see below | At load time. |
| `app.cmd(name)` | `string → nil` | Inside a keymap or command callback. |
| `app.send(text [, { role = ... }])` | `string, table? → nil` | Inside a keymap or command callback. |
| `app.ui.*` | node constructors | Inside a tool handler (or a callback). |
| `app.compute.*` | `table → block` | Inside a tool handler (or a callback); see [below](#appcompute-engine-computed-cards). |
| `app.formats.dataset{ path, rows }`, `app.formats.molecule{ smiles, name, claimed_formula }`, `app.formats.trace{ path, title }`, `app.formats.flamegraph{ path, sample_type, title }`, `app.formats.logs{ path, format }` | `table → block` | Inside a tool handler (or a callback); see [below](#appformats-file-format-blocks). |
| `app.ask{ title, description, schema, order, submit_label }` | `table → answer?, why?, reply?` | Inside a tool or command handler; see [below](#appask-forms). |
| `app.modal.open(def)`, `app.modal.update(id, node)`, `app.modal.close(id)` | see [below](#appmodal-modal-windows) | Anywhere: handlers, commands, keymaps, hooks. |
| `app.plugin{ name, capabilities }` | table → nil | At the top of the file (see [Capabilities](#capabilities)). |
| `app.http.request(opts)`, `app.http.get(url, opts)`, `app.http.post(url, opts)` | → response table | Inside handlers and callbacks, with a `network:` capability. |
| `app.shell.run(program, args, opts)` | → result table | Inside handlers and callbacks, with a `shell:` capability. |
| `app.secret(name)` | `string → string?` | Inside handlers and callbacks, once the capabilities are approved. |

A malformed argument to `keymap.set`, `commands.register`, `tools.register`, `app.on` or `mcp.add` does **not** stop the file: it produces a diagnostic with the line of the call, and only that call is ignored (see [Errors and diagnostics](#errors-and-diagnostics)).

### `app.keymap.set(keys, action, opts)`

```lua
app.keymap.set("<leader>r", "picker.grep")               -- builtin action
app.keymap.set("<C-x>", function() app.cmd("clear") end, { desc = "Clear the session" })
```

crest has no modes: the prompt is always ready to type and a mapping works in the prompt, the timeline, the tree and the chats sidebar (pickers, dialogs, forms and the help keep their own keys; the help's Keys section lists them). The Neovim form with a mode first, `app.keymap.set("n", keys, action, opts)` (or a list of modes), is still accepted and the mode is ignored.

- **`keys`**: the sequence in Neovim notation. It must **start with a chord or `<leader>`** (`<C-x>`, `<leader>r`, `<F5>`): a mapping that starts with a plain key (`j`, `gg`, `?`) would steal typing, so it is ignored with a warning at startup.
  - single characters after the first key (`<leader>gg`);
  - `<leader>` (`Ctrl+G` unless `app.opt.leader` says otherwise);
  - modifiers `<C-x>` (Ctrl), `<A-x>` or `<M-x>` (Alt; unreliable on macOS, where Option composes characters), `<S-x>` (Shift), `<D-x>` (Cmd), `<P-x>` (the platform's primary modifier: Cmd on macOS, Ctrl elsewhere; the builtin `<P-q>` quits on both);
  - key names `<CR>`/`<Enter>`, `<Esc>`, `<Tab>`, `<BS>`, `<Del>`, `<Space>`, `<lt>`, `<gt>`, `<Bar>`, `<Bslash>`, `<Up>`, `<Down>`, `<Left>`, `<Right>`, `<Home>`, `<End>`, `<PageUp>`, `<PageDown>` and `<F1>`–`<F24>`.

  `<leader>` expands to `app.opt.leader` when the keymap is built, in the builtin mappings too: with `app.opt.leader = "<C-,>"`, `<C-,>ff` opens the file picker and `Ctrl+G` is free.
- **`action`**: the name of a builtin action or a Lua function. The function is called with no arguments (`nil`) and can use `app.cmd` and `app.send`. If it returns `app.ui.block(...)`, that block is added to the timeline.
- **`opts`** (optional, as in Neovim): a table whose `desc` (a string) describes the mapping in the help's Keys section (`F1`, `/help keys`), next to the plugin that set it. Without `desc`, a builtin action shows its own description. Anything other than a table, or a `desc` that is not a string, is a diagnostic and the call is ignored.

A keymap replaces any other with the same keys, builtin ones included; the layers apply in order (plugins, the global `keymap.toml`, the global `init.lua`, the workspace `keymap.toml`, the workspace `init.lua`), so a later one wins. When the window opens, crest warns (the notices in the status bar and the startup problems of the help's Files section) about:

- an action name that is not one of the builtin actions below: the mapping is skipped;
- the same keys mapped twice in one file (or by two plugins): the last one wins;
- a mapping that is the start of a longer one (`<leader>f` with `<leader>ff` also mapped, builtin or not): the short one waits for the next key and only runs after `timeoutlen`;
- keys that do not parse, or that start with a plain key: the mapping is skipped.

These are the builtin actions valid as a string:

| Action | What it does |
| --- | --- |
| `submit`, `newline` | Sends the prompt (or runs the `/` command) / inserts a new line. |
| `focus.input`, `focus.next` | Back to the prompt / the focus to the next panel. |
| `block.next`, `block.prev`, `block.first`, `block.last` | Moves the timeline selection. |
| `block.toggle` | Expands or collapses the selected block. |
| `block.toggle_view` | Alternate view (chart ↔ table, document ↔ source). |
| `block.yank` | Copies the block to the clipboard. |
| `image.mode` | Switches pasted images between sending them to the model and the chat only. |
| `picker.files`, `picker.grep` | Opens a picker. |
| `panel.tree` | Shows or hides the tree. |
| `model.pick` | Model menu for the next turn. |
| `permission.cycle` | Cycles the permission mode. |
| `input.cancel` | In the prompt, copies its selection, else clears the prompt, else interrupts (`Ctrl+C`). With the chat focused and text selected in it, copies that text. |
| `selection.copy` | Copies the selected text, from the prompt or else the chat (`Ctrl+Shift+C`, `Ctrl+Insert`). Never interrupts. |
| `turn.interrupt` | Interrupts the turn or the running command. |
| `queue.edit` | Takes the last queued prompt back into the prompt. |
| `help.open` | Opens the help modal. |
| `app.quit` | Quits crest (while a turn runs, asks for a second press within 3 s). |
| `window.fullscreen` | Enters or leaves full screen. |
| `theme.pick`, `mode.pick` | Puts `/theme ` / `/mode ` in the prompt, whose popup lists the choices. |
| `project.switch` | Goes back to the project picker (`/project`). |
| `panel.chats` | Shows or hides the chats sidebar. |
| `chat.new`, `chat.next`, `chat.prev` | A new chat in this project / the next (older) / the previous (newer) one. |
| `chat.rename`, `chat.delete` | Renames / asks to delete the open chat, in the sidebar. |
| `timeline.outline` | Opens or closes the menu of your questions. |
| `block.focus_widget` | Gives the keys to the widget of the selected block (`Esc` gives them back). |

Any other name (`session.resume`, a typo) is skipped with the warning ``keymap `<leader>r` ignored: unknown action `session.resume` `` when the window opens; before, it was accepted and only failed when pressed.

### `app.keymap.del(keys)`

```lua
app.keymap.del("<leader>x")      -- free Ctrl+G x (chat.delete)
app.keymap.unmap("<C-c>")        -- the same function; Ctrl+C then does nothing
app.keymap.set("<leader>x", "picker.grep")   -- the keys can be mapped again
```

Removes the mapping of `keys`, builtin ones included, with the same notation and `<leader>` as `set`; `unmap` is the same function, and the Neovim form with a mode first (`app.keymap.del("n", keys)`) is accepted. Removals run in call order with the mappings of the same file, and the layers apply in order, so a workspace `init.lua` can remove what the global one mapped. Only the exact keys go: deleting `<leader>f` leaves `<leader>ff`. A removal that finds nothing mapped is a startup warning, and a `keys` that is not a string is a diagnostic with the line.

### `app.commands.register(name, fn, opts)`

```lua
app.commands.register("review", function(args)
  app.send("Review the uncommitted changes. " .. args, { role = "plan" })
end, { desc = "Review the uncommitted changes" })
```

- The command is used as `/review only src/`, and `fn` receives the rest of the line as a string (`"only src/"`, or `""` if there is nothing).
- If the name starts with `/`, that slash is ignored.
- The builtin commands `q`, `quit`, `qa`, `model`, `mode`, `clear`, `modal`, `help`, `h`, `implement`, `research`, `simulate`, `mcp` and `plugin` are evaluated first, so a command with one of those names never runs.
- **`opts`** (optional): `{ desc = "..." }` describes the command in the help's Commands section, with the plugin that registered it. A bad `opts` is a diagnostic and the command is not registered.
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
| `name` | string | The name under which the model sees the tool. It cannot reuse the name of a builtin tool (`read_file`, `grep`, `show_chart`, `task`...): such a tool is ignored, with the notice ``plugin tool `x` ignored: …``. Across Lua layers, the highest-precedence one wins. |
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

If a hook raises an error or exhausts the instruction limit, the notice ``before_tool hook failed for `x`: …`` appears and the call continues with its original input.

These are the builtin tool inputs most often filtered (the full reference is in [usage.md](usage.md#agent-tools)):

```text
run_command  { command = "git", args = { "push" }, timeout_s = 120 }   -- no shell: command is the program
edit_file    { path = "src/a.rs", old = "...", new = "..." }
write_file   { path = "notes.md", content = "..." }
read_file    { path = "src/a.rs", offset = 1, limit = 2000 }
```

### `app.ui`: Custom blocks

The constructors return plain tables that describe nodes. Lua never touches GPUI: the window turns the tree into components. Wherever a node is expected, a string or a number becomes a text node.

| Constructor | Node | Details |
| --- | --- | --- |
| `app.ui.text(s)` | text | `s` is a string or a number (not a table). |
| `app.ui.row(children)` | horizontal row | `children` is a list of nodes or strings. They share the width equally and wrap to the next line when one would get narrower than 280 px. |
| `app.ui.col(children)` | vertical column | Same. |
| `app.ui.list(items)` | bulleted list | Same. |
| `app.ui.table({ columns = ..., rows = ... })` | table | `columns` is a non-empty list of names or of `{ name = "x", align = "left" \| "center" \| "right" }`. `rows` is a list of rows, and each row is a list of scalar cells (string, number, boolean). Every row must have one cell per column. |
| `app.ui.chart({ title, kind, labels, series })` | chart | `kind` is `"bar"` (default), `"line"`, `"area"` or `"pie"`. `labels` is a list of strings and `series` is `{ { name = "x", values = { 1, 2 } }, ... }`. At least one series is required, each series has one finite value per label, and `pie` takes exactly one series. |
| `app.ui.code(lang, src)` | highlighted code | `lang` can be `nil`. |
| `app.ui.markdown(src)` | rendered markdown | Links open in the browser (`http`, `https` and `mailto` only). |
| `app.ui.image(path)` or `app.ui.image{ path = ..., data = ..., media_type = ..., alt = ..., fit = ... }` | picture | Give `path` (a file of the workspace) **or** `data` (base64, which needs `media_type`, e.g. `"image/png"`). PNG, JPEG, GIF and WebP (animated too), SVG, BMP, TIFF and ICO. `fit` is `"contain"` (default: natural size, scaled down to the width) or `"actual"` (1:1). A toolbar switches between fit, 1:1 and zoom; when zoomed, drag pans and `Ctrl`/`Cmd` + wheel zooms. See the [security notes](#modal-security) about paths. |
| `app.ui.html(src)` | basic HTML | Headings, paragraphs, lists, tables, links, emphasis and code. No CSS layout, no scripts, no forms: it is not a browser. |
| `app.ui.json(value [, { expand = n }])` | collapsible JSON tree | `value` is any Lua value. The first `n` levels start open (default 1). Click a row to open or close it. |
| `app.ui.mermaid(src)` | Mermaid diagram | Pan and zoom like the Mermaid block. |
| `app.ui.plot{ title, x_range, y_range, x_label, y_label, x_scale, y_scale, aspect, functions, params, series }` | function plot | The fields of the `show_plot` tool (`x_scale` / `y_scale` are `"linear"` or `"log"`, `aspect` `"auto"` or `"equal"`). `functions` is a list of `{ kind = "explicit" \| "parametric" \| "implicit", expr, y_expr, t_range, label }` (`expr = "a*sin(x)"`), `params` of `{ name, min, max, step, value }` (one slider each), `series` of `{ name, x, y, style = "scatter" \| "line" }`, ranges `{ min, max }`. Omit a list you do not use: an empty table `{}` is read as an object and fails. |
| `app.ui.link(text, url)` or `app.ui.link(url)` | link | Opens the URL in your browser. Only `http`, `https` and `mailto` are accepted. |
| `app.ui.diff(unified)` | colored diff | A unified diff (`@@`, `+`, `-` lines). |
| `app.ui.csv(text [, { header = true, delimiter = "," }])` | table | Parses CSV (quotes, `""`, quoted newlines, `\r\n`) into a table node. Numbers become numbers (so they sort as such), empty cells nulls; short rows are padded. `header = false` names the columns `column 1`, `column 2`... Use `delimiter = "\t"` for TSV. |
| `app.ui.block(name, node)` | the `Custom` block | Validates the whole tree and wraps it as the plugin's block. It must be **returned** from the handler. |
| `app.ui.plate{ format, wells, value_label, title, scale, center }` | the `Plate` block | The fields of the `show_plate` tool. Not a node: a whole block, already wrapped like `app.ui.block`, to be **returned** from the handler. |

An invalid argument (a table row of a different width, a series of a different length...) raises a Lua error prefixed with `app.ui.<function>:`, and the tool fails with that message. If you nest deeper than the window draws, the excess is cut off with a marker.

### `app.compute`: engine-computed cards

The same engine the `stats_*`, `calc` and `convert` tools use ([usage.md](usage.md#statistics-and-calculations)). Each function takes the tool's request as a table and returns the **block** (a statistics card, a calc sheet or a conversion card): return it from a tool or command handler to show the card. The `reproduce.lua` line of every card is such a call, so it runs as written.

| Function | Request |
| --- | --- |
| `app.compute.describe{ data, variables, by }` | Descriptive statistics of `variables` (all numeric columns when absent), per level of `by`. |
| `app.compute.test{ data, test, y, group, y2, paired, levels, mu, table, alternative, conf_level, correct, exact }` | A hypothesis test. `test` is `"auto"` (default), `"welch_t"`, `"student_t"`, `"paired_t"`, `"one_sample_t"`, `"mann_whitney"`, `"wilcoxon"`, `"chi_square"` or `"fisher"`; `alternative` is `"two_sided"` (default), `"less"` or `"greater"`; `table` gives counts directly (χ², Fisher) instead of `data`. |
| `app.compute.regress{ data, y, x, conf_level }` | OLS of `y` on the list `x`. |
| `app.compute.correlate{ data, variables, method, adjustment, conf_level }` | Correlation matrix; `method` is `"pearson"`, `"spearman"` or `"kendall"`, `adjustment` `"holm"` (default), `"bh"`, `"bonferroni"` or `"none"`. |
| `app.compute.calc{ lines, significant_digits }` | A calc sheet: `lines = { "v = 100 km/h", "t = 90 min", "d = v * t -> km" }`. Reads no data. |
| `app.compute.convert{ value, from, to, uncertainty, significant_digits }` | A unit conversion (`from = "mi/h", to = "km/h"`; without `value`, `from` may be a whole quantity such as `"5 ft + 3 in"`). Reads no data. |
| `app.compute.distribution{ data, variable, by, bin_rule, bins, bandwidth_rule, bandwidth, conf_level }` | Histogram, density, box/violin, QQ and ECDF of `variable`; `bin_rule` is `"freedman_diaconis"` (default), `"sturges"`, `"scott"` or `"fixed"` (with `bins`), `bandwidth_rule` `"silverman"` (default), `"scott"` or `"fixed"` (with `bandwidth`). |
| `app.compute.contingency{ data, row, column, table, row_labels, column_labels, test, correct, alternative, conf_level, seed, simulations }` | A contingency table from two columns or from `table` (rows of counts); `test` is `"auto"` (default), `"chi_square"` or `"fisher"`. |
| `app.compute.power{ test, n, effect, p1, p2, alpha, power, groups, alternative, solve_for }` | Power or sample size; `test` is `"two_sample_t"`, `"one_sample_t"`, `"paired_t"`, `"anova"` or `"two_proportions"`; leave out the one to solve for (or name it in `solve_for`: `"n"`, `"power"`, `"effect"`, `"alpha"`). Reads no data. |
| `app.compute.ab_test{ control = { name, visitors, conversions }, treatment = { ... }, expected_split, conf_level, alternative, looks, planned_visitors, minimum_lift }` | An A/B test from its counts. Reads no data. |
| `app.compute.bootstrap{ data, y, statistic, group, y2, levels, trim, resamples, seed, conf_level }` | A seeded bootstrap; `statistic` is `"mean"` (default), `"median"`, `"sd"` or `"trimmed_mean"` (with `trim`); `group` with `levels`, or `y2`, bootstraps a difference. |
| `app.compute.forecast{ data, y, time, periods, horizon, holdout }` | MSTL decomposition and AutoETS forecast of `y` (`periods` is the list of season lengths). |
| `app.compute.optimize{ sense, objective, variables = { { name, lower, upper, integer, binary } }, constraints = { { name, expr } } }` | LP/MILP; `sense` is `"maximize"` or `"minimize"`, `objective` and each `expr` (`"2x + 3y <= 12"`) in calculator notation. Reads no data. |
| `app.compute.solve{ equations, unknowns = { { name, guess } }, range, tolerance, max_iterations }` | An equation or a small square system. Reads no data. |
| `app.compute.simulate{ inputs = { { name, distribution = { type = "normal", mean, sd }, source, unit } }, model, output, unit, samples, seed }` | Monte Carlo (with `model`) or a Fermi estimate (without). Reads no data. |
| `app.compute.cash_flow{ flows, rate, finance_rate, reinvest_rate, currency, period }` | NPV, IRR, MIRR and payback. Reads no data. |
| `app.compute.decide{ criteria = { { name, weight, direction } }, options = { { name, scores } }, normalization }` | A decision matrix; `direction` is `"benefit"` (default) or `"cost"`, `normalization` `"min_max"` (default), `"max"` or `"none"`. Reads no data. |

`data` is either a CSV/TSV **path** of the workspace, read through the same guard as the model's `read_file` (`delimiter = "\t"` picks the separator; by default a tab for `.tsv` and a comma otherwise), or an **inline** table `{ columns = { { name = "x", values = { 1.5, nil, 3 }, unit = "mm" }, ... } }`. Values are numbers or strings, and `nil` holes are missing values; a column shorter than the longest is padded with missing values, so a trailing `nil` is not lost.

```lua
app.tools.register({
  name = "mpg_by_gear",
  description = "Compares mpg between automatic and manual cars.",
  effect = "read",
  handler = function()
    return app.compute.test{ data = "data/cars.csv", y = "mpg", group = "am" }
  end,
})
```

The envelope is in `result["$crest_block"].envelope` (its `values`, `method`, `warnings`...), so a handler can read a number before returning the card. The card records `plugin:<name>` as its agent. A request the engine rejects (an unknown column, a path outside the workspace) raises a Lua error prefixed with `app.compute.<function>:`; the calls block the VM while the engine runs.

### `app.formats`: file-format blocks

`app.formats.dataset{ path, rows }`, `app.formats.molecule{ smiles, name, claimed_formula }`, `app.formats.trace{ path, title }`, `app.formats.flamegraph{ path, sample_type, title }` and `app.formats.logs{ path, format }` build the blocks of the `show_dataset`, `show_molecule`, `show_trace`, `show_flamegraph` and `show_logs` tools (see [usage.md](usage.md#plots-reports-data-lab-and-operations-blocks)). Each returns a block wrapped like `app.ui.block`, so a handler that returns it shows it; the block is in `result["$crest_block"]` (e.g. the molecule's `formula`). `path` is read through the same guard as the model's `read_file`, `rows` defaults to 20 (at most 200), `format` is `json`, `logfmt` or `text` (sniffed when omitted) and the same size limits as the tools apply. A file that cannot be read or parsed, or a SMILES that does not parse, raises a Lua error with the reason. The calls block the VM while they read and parse.

```lua
app.tools.register{
  name = 'peek',
  description = 'Preview a data file',
  parameters = { type = 'object', properties = { path = { type = 'string' } }, required = { 'path' } },
  handler = function(args) return app.formats.dataset{ path = args.path, rows = 10 } end,
}
```

### `app.ask`: forms

`app.ask{ title, description, schema, order, submit_label }` shows the same form as the `ask_user` tool, in place in the timeline, and waits for the answer. The fields are those of `ask_user` plus `order` (but `title` is required here), the property names in display order (Lua tables have no order, so without it the fields are sorted by name); `schema` may also be a JSON string, which keeps its order. It returns the submitted values by field name, or `nil` and the reason when the user cancels (or nobody can answer: the headless mode has no forms). A prompt sent while the form waits answers it in prose: `nil`, `"the user answered in the chat instead"` and the words as a third value. An answer that does not fit the fields is asked again with the reason, up to three times.

```lua
local answer, why, reply = app.ask{
  title = 'Deploy',
  schema = {
    properties = {
      env = { type = 'string', enum = { 'staging', 'prod' } },
      dry_run = { type = 'boolean', default = true },
    },
    required = { 'env' },
  },
  order = { 'env', 'dry_run' },
}
if not answer then return 'cancelled: ' .. why .. (reply and (' (' .. reply .. ')') or '') end
```

Like the other blocking calls it holds the VM thread while it waits, so use it in tools and commands, not in hooks. The block belongs to the run, like plugin modals: it is not saved with the session.

### `app.modal`: modal windows

A plugin can open windows over crest's window and fill them with the same nodes as `app.ui`: images, tables, charts, JSON, diagrams, documents. They are pushed at once from any plugin code (tool handlers, `:` commands, keymaps, hooks), so a tool can open a report and keep updating it while it works.

```lua
local id = app.modal.open{
  id = "coverage",              -- optional; opening an id that is open replaces that modal
  title = "Coverage report",    -- optional; default: the plugin's name
  width = "60%", height = 80,   -- percent of the window: a number or "NN%"; default 70 × 70
  position = "top-right",       -- optional; default "center"
  content = app.ui.col{ app.ui.image{ path = "target/cov.png" }, app.ui.markdown("# Summary") },
}
app.modal.update(id, app.ui.table{ columns = { "file", "%" }, rows = rows })  -- replaces the content
app.modal.close(id)
```

| Function | Returns | Notes |
| --- | --- | --- |
| `app.modal.open(def)` | the id (string) | Without `id` one is made up (`modal-1`, `modal-2`...). `content` is required: a node or a string. |
| `app.modal.update(id, node)` | nothing | Does nothing if that modal is not open (the user may have closed it). |
| `app.modal.close(id)` | nothing | Same. |

Ids belong to the plugin: two plugins can both use `"report"` without touching each other's modal. Bad arguments raise a Lua error prefixed with `app.modal.<function>:` (an invalid size, an unknown or contradictory position, an invalid tree), and nothing is shown.

**Geometry.** Sizes are percentages of crest's window, and the content always scrolls (both ways) when it is larger than the modal:

- A modal never covers the whole window: a **20 px margin** stays free on every side, so `100%` is the window minus 40 px in each direction.
- The size is that percentage of the room inside the margins.
- `position` pins the modal to the margin of one or two sides: `top`, `bottom`, `left`, `right`, or a combination of one vertical and one horizontal anchor (`top-left`, `top-right`, `bottom-left`, `bottom-right`; `right-top` and `"top right"` work too). On an axis without an anchor the modal is centered. `left-right` and `top-bottom` are errors.
- The modal follows the window when it is resized.

```text
┌───────────────────────── crest window ─────────────────────────┐
│                         20 px margin                           │
│   ┌──────────────── room for modals (100%) ────────────────┐   │
│   │ top-left                top                  top-right │   │
│   │                                                        │   │
│   │ left                   center                    right │   │
│   │                                                        │   │
│   │ bottom-left            bottom             bottom-right │   │
│   └────────────────────────────────────────────────────────┘   │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

In the window, the modal has a header with its title and the plugin, and a close button. `Esc` closes the front modal, as do a click on the dimmed area around it and `/modal close`. In headless mode there is no window: crest prints ``modal `<title>` opened (only visible in the window)`` and ignores updates.

<a id="modal-security"></a>**Security.** An image `path` (in a modal or a `Custom` block) is checked before the window reads it, with the same rules as the file tools: it must resolve, following symlinks, inside the workspace, and must not match `app.permissions.sensitive_paths` (`.env`, `.git/`, keys...). A rejected image is replaced with a text node such as `[image unavailable: .env: ...]`. Images inside HTML or markdown cannot read local files. Links only open `http`, `https` and `mailto` URLs. Web pages cannot be embedded: link to them instead (see [research/rich-content-modals.md](research/rich-content-modals.md)).

#### Example: an image gallery

```lua
app.commands.register("shots", function()
  local items = {}
  for _, name in ipairs({ "login.png", "settings.png", "error.png" }) do
    items[#items + 1] = app.ui.col{ app.ui.text(name), app.ui.image{ path = "screenshots/" .. name } }
  end
  app.modal.open{ id = "shots", title = "Screenshots", width = 90, height = 90, content = app.ui.col(items) }
end)
```

#### Example: a CSV table and a chart

```lua
app.plugin{ name = "bench", capabilities = { "shell:cat" } }
app.tools.register({
  name = "bench_report",
  description = "Shows the benchmark results",
  effect = "read",
  handler = function()
    local csv = app.shell.run("cat", { "target/bench.csv" }).stdout
    local labels, values = {}, {}
    for name, ms in csv:gmatch("\n([^,\n]+),([%d.]+)") do
      labels[#labels + 1] = name
      values[#values + 1] = tonumber(ms)
    end
    app.modal.open{
      id = "bench", title = "Benchmarks", width = "70%", position = "right",
      content = app.ui.col{
        app.ui.chart{ title = "ms per run", kind = "bar", labels = labels, series = { { name = "ms", values = values } } },
        app.ui.csv(csv),
      },
    }
    return "the results are on screen"
  end,
})
```

#### Example: a JSON inspector

```lua
app.on("after_tool", function(ev)
  if ev.name == "fetch_url" then
    app.modal.open{
      id = "last-fetch", title = "Last fetch_url", width = 40, height = 60, position = "bottom-right",
      content = app.ui.json(ev, { expand = 2 }),
    }
  end
end)
```

#### Example: a report with markdown and a diagram

```lua
app.commands.register("arch", function()
  app.modal.open{
    id = "arch", title = "Architecture",
    content = app.ui.col{
      app.ui.markdown("# Layers\n\nThe domain knows nothing about I/O. See [the docs](https://example.com/arch)."),
      app.ui.mermaid("graph TD; bin --> ui; bin --> adapters; ui --> application; adapters --> application; application --> domain"),
      app.ui.link("Open the full document", "https://example.com/arch"),
    },
  }
end)
```

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
| `args` | stdio | List of strings. `${VAR}` is read from the environment. |
| `env` | stdio | Table of strings. `${VAR}` is read from the environment. |
| `cwd` | stdio | Working directory. Defaults to the workspace root. |
| `url` | HTTP | Streamable HTTP. Must start with `http://` or `https://`. |
| `headers` | HTTP | Table of strings, e.g. `Authorization = "Bearer ${TOKEN}"`. |
| `oauth` | HTTP | A pre-registered OAuth client: `{ client_id, client_secret, scopes, callback_port }`. Without it, `/mcp login` registers crest dynamically. |
| `effect` | both | Same as for plugin tools. Defaults to `"execute"`. Applies to **all** of the server's tools. |
| `timeout` | both | Seconds (> 0) per call. Defaults to 120. |
| `startup_timeout` | both | Seconds (> 0) to start: process, handshake and tool list. Defaults to 120. |
| `disabled` | both | `true` declares it without starting it (`/mcp enable <name>`). |
| `type` | both | `"stdio"` or `"http"` (also `"local"`, `"remote"`, `"streamable-http"`); optional, checked against `command`/`url`. |

- Each server has `command` **or** `url`, never both.
- An unknown field produces a diagnostic.
- The name can only contain letters, digits, and single `-` and `_` (not `__`), because the tools are exposed as `mcp__<server>__<tool>`.
- Tool names are adapted for every model provider: characters other than letters, digits, `-` and `_` become `_`, and long names are shortened with a hash so `mcp__<server>__<tool>` fits in 64 characters. Input schemas are adapted too (a top-level `anyOf`/`oneOf`/`allOf` is merged into one object and local `$ref`s are inlined). `/mcp tools <name>` shows the names the model sees.
- If one file uses both forms with the same name, `app.mcp.add` wins.

**Security**:

- Servers from the global config and from plugins start when crest opens.
- Workspace servers do not start until the user approves them with `crest --trust-mcp <name>` or `/mcp trust <name>` (see [usage.md](usage.md#mcp-servers-and-trust)). The approval is required again if the declaration changes.

## Capabilities

The sandbox has no network, no programs and no access to the environment. A plugin that needs to call an API or a command-line program declares it with `app.plugin`, and the user approves that declaration once. Until then the APIs below raise an error.

### Declaring them: `app.plugin`

```lua
local app = require("app")

app.plugin{
  name = "jira",
  capabilities = { "network:jira.example.com", "network:*.github.com", "shell:jq", "shell:git" },
}
```

Write it at the top of the plugin's `init.lua`. It is also allowed in the global and in the workspace `init.lua`.

| Field | Notes |
| --- | --- |
| `name` | Letters, digits, `-` and `_` (at most 64). It names the plugin in notices, in `--trust-plugin` and in its secrets. In a plugin folder it defaults to the folder name, and a different name is reported as a diagnostic and ignored. In the global and workspace configs it defaults to `config` and `workspace`. |
| `capabilities` | A list of strings, each with one of the forms below. |

| Capability | Allows |
| --- | --- |
| `network:jira.example.com` | `app.http` to `https://jira.example.com` (port 443). |
| `network:*.github.com` | `app.http` over HTTPS to any subdomain of `github.com` (`api.github.com`, `a.b.github.com`), but not `github.com` itself. |
| `network:http://localhost:8080`, `network:https://host:8443` | That scheme, host and port exactly. Plain `http` is only allowed when written like this. |
| `shell:jq` | `app.shell.run("jq", ...)`. Only a program name: no paths and no arguments. |

- `app.plugin` may be called once per file; later calls are ignored with a diagnostic.
- A malformed entry (`disk:/`, `shell:/bin/sh`, `network:x.com/path`...), an unknown field or a bad `name` produces a diagnostic with the line, and only that entry is left out.
- The list is kept sorted and without duplicates, so reordering it does not count as a change.
- A plugin that declares nothing can use none of these APIs, and neither can `app.secret`.

### Approving them

On startup, each declaration that has not been approved yet shows a notice:

```text
plugin `jira` requests network: jira.example.com; shell: jq — approve with `crest --trust-plugin jira` or `/plugin trust jira`
```

(A declaration of the workspace config starts with ``workspace plugin `x` ``.)

- `crest --trust-plugin jira` (repeatable) approves it for this run and saves it.
- `/plugin trust jira`, in the window, saves it and activates it **immediately**, without restarting.
- `/plugin list` shows every declaration, the file it comes from, its capabilities and whether it is `approved` or `pending`.

An approval covers one exact declaration: the plugin name, the file that declared it (for the workspace config that path includes the workspace root), its layer and the exact capability list. If any of them changes (a new host, one more program), the plugin asks again. Approvals are stored in `plugin-trust.json` in crest's data directory (see [configuration.md](configuration.md#plugin-capabilities-and-secrets)); deleting an entry revokes it on the next start.

A call without approval fails with a Lua error such as:

```text
app.http: the capabilities of plugin `jira` (network: jira.example.com) are not approved; approve them with `crest --trust-plugin jira` or `/plugin trust jira`
```

Capabilities are **independent of tool approvals**: a tool registered by the plugin keeps its `effect`, and the permission policy asks for it as usual. Declare `effect = "network"` (or `"execute"`) for tools that call out, so that the user sees each call.

### `app.http`

```lua
local res = app.http.request{
  method = "POST",                          -- default "GET"
  url = "https://jira.example.com/rest/api/2/search",
  headers = { Authorization = "Bearer " .. token },
  query = { maxResults = 10 },              -- appended to the URL
  json = { jql = "assignee = currentUser()" }, -- or body = "raw string"
  timeout = 20,                             -- seconds, default 30
}
local res = app.http.get(url, opts)         -- same options, without url/method
local res = app.http.post(url, opts)
```

| Option | Notes |
| --- | --- |
| `method` | Any HTTP method, case-insensitive. Only `request` takes it. |
| `url` | `http` or `https`. Its scheme, host and port must match a declared `network:` capability. |
| `headers` | Table of strings (numbers and booleans are converted). |
| `query` | Table of scalars, added to the query string. |
| `body` / `json` | A string sent as is, or a table encoded as JSON (with `Content-Type: application/json` unless you set one). Not both. |
| `timeout` | Seconds, greater than 0 and at most 600. It covers the whole exchange: connection, redirects and body. |

The result is `{ status = 200, headers = { ["content-type"] = "..." }, body = "...", json = ... }`:

- `headers` has lowercase names; repeated headers are joined with `, `.
- `body` is the raw body (a Lua string, so binary is fine).
- `json` is the decoded body when the `content-type` mentions `json` and the body parses; otherwise `nil`.
- Any status (404, 500...) comes back as a result. Network errors, timeouts, a body larger than **10 MiB** and denied hosts raise a Lua error prefixed with `app.http:`.

Redirects are followed by hand, at most 5. Every target must also match a declared capability, otherwise the call fails (`redirect from … to … denied`). When a redirect changes host, `Authorization` and `Cookie` are dropped. A `303`, or a `301`/`302` after a `POST`, continues as a `GET` without body.

### `app.shell.run`

```lua
local out = app.shell.run("jq", { "-r", ".name" }, {
  stdin = '{"name":"crest"}',  -- optional
  cwd = "sub/dir",             -- optional, inside the workspace
  env = { NO_COLOR = "1" },    -- optional, extra variables
  timeout = 10,                -- seconds, default 60
})
-- out = { status = 0, stdout = "crest\n", stderr = "", timed_out = false, truncated = false }
```

- There is no shell: `program` is the name of a declared `shell:` capability and `args` (strings or numbers) are passed as is, so pipes, globs and `$VAR` are not interpreted.
- The program is looked up in the **absolute** directories of `PATH`; relative entries such as `.` are skipped, so a repository cannot slip in its own `jq`.
- It runs in the workspace root. `cwd` may be relative to it or absolute, but once symlinks are resolved it must be a directory inside the workspace.
- It inherits crest's environment minus `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `MISTRAL_API_KEY` and every `CREST_PLUGIN_*` variable (other plugins' secrets). `env` adds variables on top.
- It runs in its own process group. When `timeout` (at most 3600 s) elapses, the whole group is killed: `timed_out` is `true` and `status` is `nil`.
- `status` is the exit code (`nil` if a signal killed it). `stdout` and `stderr` keep at most 1 MiB each; `truncated` says whether something was dropped.
- A program that is not declared, not found or cannot start raises a Lua error prefixed with `app.shell.run:`. A non-zero exit is not an error.

### `app.secret`

```lua
local token = app.secret("token")  -- string, or nil if it is not stored
```

It reads, in this order:

1. the system keyring, service `crest`, entry `plugin:<plugin>:<name>`;
2. the environment variable `CREST_PLUGIN_<PLUGIN>_<NAME>`, uppercase, with every character other than a letter or digit turned into `_` (`app.secret("api.token")` in plugin `jira-cloud` reads `CREST_PLUGIN_JIRA_CLOUD_API_TOKEN`).

A plugin only reads its own secrets, and only after its capabilities were approved. crest never logs the values. How to store one is described in [configuration.md](configuration.md#plugin-capabilities-and-secrets).

### Security notes

- The APIs are bound to the file that declared them: they check that the Lua function calling them was written in that file. Another plugin that gets hold of your `app` table or your functions cannot use them, and `load` cannot pretend to be that file.
- They are only active once the files have loaded: calling them at the top level of `init.lua` fails with `capabilities are not active while init.lua loads`. Use them inside tool handlers, hooks and callbacks.
- The VM runs one call at a time, so a slow request or program delays other Lua calls (hooks of other tools included) until it finishes or hits its timeout. The window and the agent keep running meanwhile; the timeouts are the guard, because the instruction limit does not count time spent waiting.
- What a tool returns goes to the model. Do not return secrets or whole responses that contain them.
- The workspace config is code from the repository: approve its capabilities only if you trust that repository.

### Example: an API tool with `app.http` and `app.secret`

`~/.config/crest/plugins/jira/init.lua`:

```lua
local app = require("app")

app.plugin{ name = "jira", capabilities = { "network:jira.example.com" } }

local BASE = "https://jira.example.com/rest/api/2"

app.tools.register({
  name = "jira_issue",
  description = "Fetches a Jira issue by key (e.g. CR-7): summary, status and assignee.",
  schema = {
    type = "object",
    properties = { key = { type = "string", description = "Issue key, e.g. CR-7" } },
    required = { "key" },
  },
  effect = "network",
  handler = function(input)
    local token = app.secret("token")
    if not token then
      error("no Jira token: store it with `secret-tool store --label=crest service crest username plugin:jira:token`")
    end
    local res = app.http.get(BASE .. "/issue/" .. input.key, {
      headers = { Authorization = "Bearer " .. token, Accept = "application/json" },
      query = { fields = "summary,status,assignee" },
      timeout = 15,
    })
    if res.status ~= 200 or not res.json then
      error("Jira answered " .. res.status)
    end
    local f = res.json.fields
    return {
      key = res.json.key,
      summary = f.summary,
      status = f.status and f.status.name,
      assignee = f.assignee and f.assignee.displayName,
    }
  end,
})
```

1. Store the token: `secret-tool store --label=crest service crest username plugin:jira:token`, or export `CREST_PLUGIN_JIRA_TOKEN`.
2. Start once with `crest --trust-plugin jira` (or open crest and type `/plugin trust jira`).
3. Ask "what is the status of CR-7?". Because of `effect = "network"`, the call asks for approval in `ask` mode; the model gets the table as JSON.

For GitHub, declare `"network:api.github.com"`, read `app.secret("token")` and send `Authorization = "Bearer " .. token` plus `Accept = "application/vnd.github+json"`.

### Example: a CLI wrapper with `app.shell.run` and `jq`

`~/.config/crest/plugins/jsonq/init.lua`:

```lua
local app = require("app")

app.plugin{ name = "jsonq", capabilities = { "shell:jq" } }

app.tools.register({
  name = "json_query",
  description = "Runs a jq filter over a JSON file of the workspace and returns the result.",
  schema = {
    type = "object",
    properties = {
      file = { type = "string", description = "Path relative to the workspace" },
      filter = { type = "string", description = "jq filter, e.g. .dependencies | keys" },
    },
    required = { "file", "filter" },
  },
  effect = "read",
  handler = function(input)
    local out = app.shell.run("jq", { input.filter, input.file }, { timeout = 10 })
    if out.timed_out then error("jq timed out") end
    if out.status ~= 0 then error("jq failed: " .. out.stderr) end
    return out.stdout
  end,
})
```

Approve it with `crest --trust-plugin jsonq` and ask "list the dependencies in package.json". Note that `jq` reads the path it is given, so this tool can read any file the user can, sensitive paths included: the `effect = "read"` above lets it run without asking, so only declare it like that if that is acceptable, or use `effect = "execute"` to approve each call.

## Errors and diagnostics

| Problem | What happens |
| --- | --- |
| Syntax or runtime error while loading a file | That file contributes nothing and the others load. An `Error` block is shown with `file:line: message`. |
| Infinite loop or excess memory while loading | As above, with `instruction limit exceeded` or `out of memory`. |
| Invalid argument to a registration function | Diagnostic with the line of the call. Only that call is ignored. |
| Invalid declarative field (`app.opt.x`, `app.models...`) | Diagnostic without a line. Only that field is ignored. |
| `app.plugins` folder without an `init.lua` | Diagnostic `cannot read the file: …`. |
| Error in a tool handler | The model gets the error and the ToolCall is marked failed. |
| Error in a hook | Notice in the status bar. The tool continues. |
| Error in a keymap or command callback | `Error` block with `Lua: <message>`. |
| Malformed `app.plugin` entry | Diagnostic with the line of the call. Only that entry is ignored. |
| Capability API without approval | Lua error saying how to approve it (`crest --trust-plugin <name>` or `/plugin trust <name>`). |
| Keymap with invalid notation, a plain first key or an unknown action | Notice ``keymap `x` ignored: …`` when the window opens. |
| Keys mapped twice in a file, a mapping that starts a longer one, `keymap.del` of unmapped keys | A warning notice when the window opens; the keymap still loads. |
| Problem in a `keymap.toml` | A startup warning with the file (and the line of a syntax error); a file that does not parse is ignored, a bad entry is skipped. |

In headless mode the diagnostics go to the terminal. Run `crest --headless "hello"` to check that your config loads cleanly.

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
            task = { type = "string" },
            done = { type = "boolean" },
          },
          required = { "task" },
        },
      },
    },
    required = { "items" },
  },
  effect = "read",  -- only displays data: runs without asking for approval
  handler = function(input)
    local rows, done_count = {}, 0
    for i, item in ipairs(input.items) do
      local done = item.done == true
      if done then done_count = done_count + 1 end
      rows[i] = { item.task, done }
    end
    return app.ui.block("todo", app.ui.col({
      app.ui.text(string.format("%d of %d done", done_count, #input.items)),
      app.ui.table({ columns = { "task", { name = "done", align = "center" } }, rows = rows }),
      app.ui.chart({
        title = "Progress",
        kind = "pie",
        labels = { "done", "pending" },
        series = { { name = "tasks", values = { done_count, #input.items - done_count } } },
      }),
    }))
  end,
})
```

To try it, open crest and ask: "use todo_list to show the tasks: parser (done), docs, release". The timeline shows the `todo_list` ToolCall and, below it, the block with the text "1 of 3 done", the table and the pie chart. The model only gets `[shown to the user as a block]`.

### 2. A hook that blocks `rm` and forces `git push --dry-run`

`~/.config/crest/plugins/guard/init.lua`:

```lua
local app = require("app")

local blocked = { rm = true, shred = true }

app.on("before_tool", function(ev)
  if ev.name ~= "run_command" then return nil end
  if blocked[ev.input.command] then
    return { deny = true, reason = "`" .. ev.input.command .. "` is blocked by the guard plugin" }
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
| `{ command = "rm", args = { "-rf", "x" } }` | Denied with the reason "`rm` is blocked by the guard plugin". |
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

To use it, type `/review only src/` and press `Enter`. This sends the prompt "Review the uncommitted changes and list the risks. only src/", which is served by the `plan` role's model (`claude-opus-5-5` by default). The status bar shows the turn's model.

### 4. Keymaps

```lua
local app = require("app")

app.opt.leader = "<C-,>"

-- Ctrl+, x: new session
app.keymap.set("<leader>X", function() app.cmd("clear") end)

-- Ctrl+P: file picker
app.keymap.set("<C-p>", "picker.files")

-- Ctrl+, t: ask the agent to run the tests
app.keymap.set("<leader>t", function()
  app.send("Run the workspace tests and summarize the failures in a table")
end)
```

With `leader = "<C-,>"`, the mappings in this file and the builtin ones (`Ctrl+, ff`, `Ctrl+, m`...) use `Ctrl+,`, and `Ctrl+G` does nothing. Earlier versions left the builtin mappings on `Ctrl+G`; to keep one there, map it with the chord written out:

```lua
app.keymap.set("<C-g>ff", "picker.files")
```

A leader that does not parse, or that is a plain key the prompt types (`" "`, `","`), is a startup warning and the leader stays `Ctrl+G`.

### Testing a plugin

1. Start headless on any directory: `crest --headless "list the files" --workspace /tmp/try`. Load diagnostics are printed to the terminal.
2. Open the window and check that there are no `Error` blocks at the top of the timeline.
3. For tools: ask for something that uses them, expand the ToolCall with `Ctrl+G o` and check the input and the output.
4. For hooks: use `print` and `CREST_LOG=warn,lua=info`.
5. If you write Rust, the tests in `crates/adapters/lua/tests/` (`plugin_tools.rs`, `plugin_hooks.rs`, `plugin_ui.rs`) show how to load an `init.lua` into a real `LuaHost` and run it inside the agent loop with a scripted model (`application::testing`).
