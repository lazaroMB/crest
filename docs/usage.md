# Usage

This guide covers the window, the command line, headless mode, sessions and MCP servers. The research and implementation workflows have a guide of their own: [workflows.md](workflows.md).

## The window

```bash
crest                 # workspace = current directory
crest ~/src/proyecto  # or --workspace ~/src/proyecto
```

The window opens full screen. On X11 it opens as a normal window and requests full screen after the first frame; if the compositor refuses, it stays centered at 1280×800. The title is `crest — <directory>`. It uses a dark theme and a single monospace font: JetBrains Mono if installed, otherwise the system monospace font.

```text
┌──────────────┬───────────────────────────────────────────────┐
│ tree         │ timeline: blocks (text, tools, diffs,         │
│ (<Space>e,   │ charts, tables, Mermaid, markdown...)         │
│  hidden by   │                                               │
│  default)    │                                               │
│              ├───────────────────────────────────────────────┤
│              │ multi-line input                              │
│              │ INSERT  <pending keys>                        │
│              ├───────────────────────────────────────────────┤
│              │ INSERT │ model │ ask │ $0.00 │ project ⟨notice⟩│
└──────────────┴───────────────────────────────────────────────┘
```

From left to right, the status bar shows:

- the input mode;
- the model of the current or last turn;
- the permission mode;
- the session cost;
- the workspace.

Notices (for example `retrying in 2.0s (attempt 2)`, `opus failed, using sonnet` or a model fallback) stay for 6 s.

Each prompt runs as a **turn**, and only one turn runs at a time. While crest works (a turn, or a command such as `/research` or `/implement` running up to its next question) a line above the prompt says so: a spinner, the stage, what is going on now (the model a stage asks, a tool call, a check) and the elapsed time. If you send a prompt meanwhile, it is **queued**: the queued prompts are listed under that line and sent one by one when crest is free. `Up` in an empty prompt takes the last queued one back to edit it (empty the prompt to drop it). `Ctrl+C` interrupts the turn and clears the queue; a running command cannot be interrupted, so there it only clears the queue. `Ctrl+Q` (`Cmd+Q` on macOS) or `/q` quits crest.

### Modes

| Mode | What for | How to enter |
| --- | --- | --- |
| **insert** | Typing the prompt (a multi-line editor, up to 8 rows before it scrolls). | Initial mode. `i` from normal, or click the editor. |
| **normal** | Navigating the timeline and using the `<leader>` shortcuts. | `Esc` from insert or command. |
| **command** | Typing a one-line `:` command. | `:` from normal. `Esc`, or `Backspace` on an empty line, goes back to normal. |

In insert mode, keys without a mapping go to the editor. While crest waits for the next key of a sequence (`g` of `gg`, `<Space>` of `<Space>ff`), the line under the editor shows what you have typed so far. After `timeoutlen` (1 s by default), the exact mapping runs if there is one; otherwise the keys are dropped (in insert mode they are typed as text).

### Default key bindings

`<leader>` is the space bar in every builtin mapping. Changing `app.opt.leader` only affects mappings defined in Lua (see [plugins.md](plugins.md#4-keymaps)).

| Mode | Key | Action | What it does |
| --- | --- | --- | --- |
| insert | `Esc` | `mode.normal` | Switches to normal. |
| insert | `Enter` | `submit` | Sends the prompt (if not empty). |
| insert | `Shift+Enter` | `newline` | Inserts a new line. |
| normal | `i` | `mode.insert` | Back to insert. |
| normal | `j` / `k` | `block.next` / `block.prev` | Next / previous block. |
| normal | `gg` / `G` | `block.first` / `block.last` | First / last block. |
| normal | `Enter` | `block.toggle` | Expands or collapses the block (ToolCall, Thinking). |
| normal | `t` | `block.toggle_view` | Alternate view of the block (see [Blocks](#blocks)). |
| normal | `y` | `block.yank` | Copies the block's content to the clipboard. |
| normal | `<Space>ff` | `picker.files` | File picker (fzf). |
| normal | `<Space>fg` | `picker.grep` | Text search (rg). |
| normal | `<Space>e` | `panel.tree` | Shows or hides the workspace tree. |
| normal | `<Space>m` | `model.pick` | Picks the model for the **next** turn. |
| normal | `<Space>p` | `permission.cycle` | Cycles `ask` → `auto-edit` → `plan`. |
| normal | `:` | `mode.command` | Opens the command line. |
| normal | `?` | `help.open` | Opens the [help](#help). |
| command | `Esc` | `mode.normal` | Cancels the command. |
| all | `Ctrl+C` | `turn.interrupt` | Interrupts the running turn. Also works while a picker or a dialog is open. |
| all | `F1` | `help.open` | Opens the [help](#help). |
| all | `Ctrl+Q` / `Cmd+Q` | — | Quits crest. |

You can redefine or add mappings with `app.keymap.set` ([plugins.md](plugins.md#appkeymapsetmode-keys-action)).

### `:` commands

| Command | What it does |
| --- | --- |
| `/q`, `/quit`, `/qa` | Quits crest. |
| `/help [section\|search]`, `/h` | Opens the [help](#help), on a section (`commands`, `keys`, `tools`, `plugins`, `mcp`, `models`, `cli`, `files`, or its number) or with that search already typed. |
| `/model <alias\|role\|id>` | Model for the **next** turn. Accepts an alias (`opus`), a role name (`plan`, which uses that role's whole spec) or a full id. With no argument it goes back to the default model. |
| `/mode <ask\|auto-edit\|plan>` | Changes the permission mode. |
| `/clear` | Starts a new session with an empty timeline. The previous one stays saved. Does not work while a turn is running. |
| `/modal close` | Closes the front [plugin modal](#plugin-modals). |
| `/mcp trust <name>` | Approves a workspace MCP server. It starts the next time you open crest. |
| `/plugin list` | Lists the plugins that declare capabilities, with their file, their capabilities and whether they are `approved` or `pending`. |
| `/plugin trust <name>` | Approves the capabilities of plugin `<name>` and activates them immediately (see [Plugin capabilities and trust](#plugin-capabilities-and-trust)). |
| `/implement <spec>` | Starts or resumes the implementation workflow for a Spec ([workflows.md](workflows.md#implementation-workflow)). |
| `/research <problem>`, `/research resume`, `/research` | Starts, resumes or queries the research workflow ([workflows.md](workflows.md#research-workflow)). |
| `/simulate <spec>` | Simulates the solution of a Spec (its folder, its number or its path) and opens its board in a full-size modal ([workflows.md](workflows.md#simulating-a-solution)). The verdicts land in the timeline. |
| `:<name> <args>` | A command registered with `app.commands.register`. |
| `:<dotted.action>` | Runs a builtin action, for example `/picker.files` or `:panel.tree`. |

An unknown command shows `unknown command :x (use /help, /model, /mode, /clear, /modal, /q; /help lists every command)`.

### Help

`?` in normal mode, `F1` in any mode or `/help` open the help: a modal at 90 % of the window with everything you can use in this session, taken from what is actually loaded (not from this page).

| # | Section | What it lists |
| --- | --- | --- |
| 1 | Commands | The builtin `:` commands with their usage, then the commands of your plugins (with the plugin and its `desc`). |
| 2 | Keys | The effective keymap per mode: the defaults plus your `app.keymap.set` mappings, with the leader resolved, the action, its description and `overrides default` where a mapping replaced a builtin one. |
| 3 | Tools | Every tool the agent has, grouped Built-in / Plugin / MCP, with its description, its effect (`read`, `write`, `execute`, `network`; `depends on input` for `run_command`) and its input schema (`Enter` expands it). |
| 4 | Plugins | Each loaded plugin or config file: its file, layer (`global` or `workspace`), what it adds (tools, commands, keymaps, hooks), its capabilities and trust state (`approved`, `pending` with the `/plugin trust` command, or `no capabilities`) and its load problems. |
| 5 | MCP servers | Name, transport, origin and state: `ready (N tools)`, `failed: …`, `starting`, or held back because the workspace config is not trusted (with the `/mcp trust` command). |
| 6 | Models | Each role and the model it resolves to, the aliases, the `--model` override, the model of the next turn and the permission mode. |
| 7 | CLI | The flags of `crest`, generated from the command-line parser. |
| 8 | Files & config | The workspace, the config files, the plugins directory, the session database, the trust stores and the startup problems. |

| Key | Effect |
| --- | --- |
| `1`–`8`, `Tab` / `Shift+Tab`, click | Switches section. |
| `j` / `k`, arrows, `PageUp` / `PageDown`, `Home` / `End` | Moves the selection. |
| `Enter` | Expands the selected row (a tool's input schema). |
| `y` | Copies the selected row's command or name (`/plugin trust jira` for a pending plugin). |
| `/` | Goes to the search box: what you type filters every section at once (each word must appear in the row, ignoring case). The arrows still move; `Enter` goes back to the list. |
| `Esc`, `✕` | Closes the help; the focus goes back where it was. |

### File references with `@`

When you pick a file in the tree, in a picker or in a `Sources` block, `@path ` (or `@path:line ` for a grep result) is inserted at the prompt cursor, and the input switches to insert. It is plain prompt text: the model reads it as a reference and opens the file with its tools.

### Pickers (`<Space>ff`, `<Space>fg`)

They use the same ports and the same defaults as the agent's `find_file` and `grep` tools, so you see what the model would see:

- they respect `.gitignore`;
- they exclude `.git/`;
- they apply the sensitive paths.

Results update as you type, with a 120 ms debounce. In `<Space>fg` the text is a ripgrep regular expression.

| Key | Action |
| --- | --- |
| `↑` / `↓`, `Ctrl+P` / `Ctrl+N` | Move the selection. |
| `Enter` | Insert the `@...` reference into the prompt. |
| `Esc` | Close. |

### Workspace tree (`<Space>e`)

A 280 px side panel, hidden at startup. It comes from the same `tree` the agent uses: it shows up to 16 levels and leaves out ignored files and `.git/`.

- Clicking a directory opens or closes it.
- Clicking a file inserts `@path`.
- The tree refreshes by itself when files change (it watches the directory, with a 200 ms debounce). If the watcher cannot start, the notice `the tree will not refresh by itself: …` appears.

### Model menu (`<Space>m`)

Lists `default (main role)` and every alias, both the builtin ones (`opus`, `sonnet`, `haiku`, `fable`) and those in your config, with the model each one points to.

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑`, `Tab` | Move the selection. |
| `Enter` | Pick. |
| `Esc` / `q` | Close. |

The choice applies to the next turn only. For every turn, use `--model` at startup.

### Approvals and permission modes

Each tool declares its effect (`read`, `write`, `execute` or `network`), and the permission mode decides what happens:

| Mode | Read | Edit files | Run commands / network |
| --- | --- | --- | --- |
| `ask` (default) | yes | asks | asks |
| `auto-edit` | yes | yes | asks |
| `plan` | yes | no (denied) | no (denied) |

To change mode: `<Space>p`, `/mode <mode>` or `--mode` at startup.

`run_command` counts as a read only for `git log`, `git blame`, `git diff`, `git status` and `git show` without options that write (`--output`, `--no-index`, `--contents`, `--ext-diff`). Any other command counts as execution.

When approval is needed, a dialog opens with the colored diff (for edits) or with the exact command and its directory (for `run_command`):

| Key | Answer |
| --- | --- |
| `y` | Approve this once. |
| `a` | Always approve: creates a rule for **that same tool and that exact subject** (the same path or the same command line, nothing broader). |
| `n` | Reject. It first asks for an optional reason, which is passed to the model: type it and press `Enter`, or `Esc` to go back. |
| `Esc` | Reject without a reason. |

There are also `Approve (y)`, `Always (a)` and `Reject (n)` buttons. If several requests arrive, they wait in a queue. A denied tool (by you, by the mode or by a hook) is not retried automatically: the model gets the reason and decides.

"Always" rules are saved with the session and come back when it is resumed. `--new` and headless without `--continue` start with no rules, while `/clear` keeps those of the current run.

The same dialog appears when the session reaches its spending cap (`app.opt.max_session_cost`). See [configuration.md](configuration.md#cost-and-spending-cap).

### Blocks

| Block | Source | Interaction |
| --- | --- | --- |
| Text | Model reply (streamed markdown); your prompt appears quoted with `>`. | — |
| Thinking | Extended thinking. | Collapsed to one line; `Enter` expands it. |
| ToolCall | Any tool: one line with name, subject and status (pending, awaiting approval, running, ok, failed, denied, cancelled). | `Enter` shows the input and the output. |
| Diff | `edit_file`, `write_file` | Colored diff with its approval status. |
| Chart | `show_chart` tool (bar, line, area, pie). | **`t` toggles a table** of the same data. |
| Table | `show_table` tool | Virtualized. Clicking a column's sort icon cycles descending → ascending → original. |
| Mermaid | `show_mermaid` tool | Dragging pans the diagram; `Ctrl`/`Cmd` + wheel zooms; with the diagram focused (click), `+`/`-` zoom and `0` resets. `t` shows the source. If it does not parse, the source and the error are shown. |
| Markdown | `show_markdown` tool | Document typography; `t` shows the source. |
| Custom | Lua plugin tools | See [plugins.md](plugins.md#appui-custom-blocks). |
| Sources, StageHeader, Image | Research workflow | In Sources, clicking (or `Enter` on) a file inserts `@path:line`, and on a URL opens the browser. |
| Error | API, tool or config failures | Message and suggested action. |

`t` only has an effect on Chart, Markdown and Mermaid. `y` copies the content of the selected block.

### Plugin modals

Plugins can open modal windows over the session ([plugins.md](plugins.md#appmodal-modal-windows)): a report, a gallery of screenshots, a table with a chart, a JSON document. Each has a header with its title, the plugin that opened it and a close button, and its content scrolls in both directions when it does not fit. A modal never covers the whole window: at least 20 px stay free on every side.

| Key or action | Effect |
| --- | --- |
| `Esc` | Closes the front modal (the one opened last). |
| Click on the dimmed area, or the `✕` button | Same. |
| `/modal close` | Same, from the command line. |
| Wheel, or the scrollbars | Scrolls the content. |
| Images: `fit`, `1:1`, `−`, `+` | Fit to the width, natural size, zoom. When zoomed, drag pans and `Ctrl`/`Cmd` + wheel zooms; with the image focused (click), `+`/`-`, `1` and `0` do the same. |
| JSON: click a row | Opens or closes that branch. |
| Links | Open in your browser. |

The focus goes back to where it was when the last modal closes. An approval dialog opened while a modal is up appears above it. Modals are not saved with the session.

### Simulation board

`/simulate <spec>` (or the agent's `simulate_solution` tool) opens the board of a simulated Spec in a modal at 100%. The board is drawn in real 3D by an offscreen GPU renderer; on a machine without a usable GPU adapter it falls back, automatically, to a 2.5D vector drawing (the header says which one: `3D · <adapter>` or `2.5D · <reason>`; `CREST_RENDER3D=off` forces the fallback).

| Key or action | Effect |
| --- | --- |
| Drag | Orbits (leaves the isometric preset for perspective). In the 2.5D fallback, pans. |
| Right-drag, middle-drag or `Shift`-drag | Pans. |
| Wheel, `+` / `-` | Zooms. |
| `i`, or the `iso` button | Back to the isometric preset (orthographic, 30°). |
| Click a chip | Inspects it: state, variables, queue and its machine as a state diagram with the current state lit. |
| `Space`, `▶` / `⏸` | Plays or pauses the scenario (at `1×` a whole scenario lasts 10 s). |
| `←` / `→`, `⏮` / `⏭` | Previous / next event. |
| `−` / `+` next to the speed | Slower / faster (0.1× to 4×). |
| Click or drag the bar | Scrubs; the red marks are failed assertions. |
| A failed check on the right | Jumps to its counterexample. |
| Scenario buttons, seed `−` / `+` | Shows another scenario; re-runs this one with another seed. |
| `Esc` | Closes the board. |

### Cost

The bar shows the accumulated cost of the session:

- `$0.00` before the first reply;
- `$?` if the model used has no configured price; crest ships no prices, so this is the usual value until you set `app.opt.pricing`;
- otherwise the amount, with 4 decimals below 1 USD and 2 from then on.

See [configuration.md](configuration.md#cost-and-spending-cap).

## Command line

```text
crest [OPTIONS] [DIR]
```

| Flag | Value | What it does |
| --- | --- | --- |
| `[DIR]` | directory | Workspace to open. Defaults to the current directory. |
| `--workspace <DIR>` | directory | Same as `[DIR]`. The two cannot be combined. |
| `--headless [<PROMPT>]` | text | Runs one turn with that prompt in the terminal, without a window. Without a prompt, it goes with `--implement`, `--research` or `--simulate`. |
| `--implement <RUTA>` | Spec or `plan.json` | In headless mode, runs the implementation workflow. Requires `--headless` and cannot be combined with `--research`. |
| `--research <PROBLEMA>` | text or `resume` | In headless mode, runs the research workflow. Requires `--headless`. |
| `--simulate <SPEC>` | Spec folder, number or path | In headless mode, simulates the Spec's solution: prints every scenario's verdict and the path of its `board.html`. Requires `--headless`; cannot be combined with `--implement` or `--research`. |
| `--model <ALIAS\|ID>` | alias, role or id | Model override for **every** turn of this run (like `/model`, but permanent). |
| `--mode <MODE>` | `ask`, `auto-edit`, `plan` | Initial permission mode. Defaults to `ask`. |
| `--yes` | — | In headless mode, approves every action without asking. Does not answer workflow gates. Requires `--headless`. |
| `--new` | — | The window opens with a new session instead of resuming the last one. Cannot be combined with `--headless`. |
| `--continue` | — | In headless mode, continues the workspace's last session, with its history. Requires `--headless`. |
| `--trust-mcp <NAME>` | name | Approves the MCP server `NAME` from the workspace's `.crest/init.lua`, starts it and remembers it. Can be repeated. |
| `--trust-plugin <NAME>` | name | Approves the capabilities (network, shell) that plugin `NAME` declares, activates them in this run and remembers them until the declaration changes. Can be repeated. |
| `-h`, `--help` / `-V`, `--version` | — | Help / version. |

Examples:

```bash
crest                                        # window on the current directory, resumes the last session
crest --new ~/src/api                        # window, new session
crest --mode plan --model opus .             # read-only, with Opus on every turn
crest --headless "explain what src/main.rs does" --workspace ~/src/api
crest --headless "run the tests and fix the one that fails" --mode auto-edit --yes
crest --headless "and the second failure?" --continue
crest --headless --research "add() gets negative numbers wrong"
crest --headless --research resume
crest --headless --implement 0001-negative-add
crest --headless --simulate 0001-negative-add
crest --trust-mcp tickets --trust-mcp docs
crest --trust-plugin jira --trust-plugin jsonq
```

With `make` (`WORKSPACE` defaults to `.`):

```bash
make run WORKSPACE=~/src/api
make run-headless PROMPT="list the project's TODOs" WORKSPACE=~/src/api
```

### Exit codes

| Code | Window | Headless (turn) | Headless (workflows) | Headless (`--simulate`) |
| --- | --- | --- | --- | --- |
| 0 | Closed normally. | The turn finished. | The workflow completed. | Every scenario passed. |
| 1 | The window could not open. | The turn failed (model error, more than 50 model calls in one turn). | Aborted, sent back to the Spec, a step error, or stdin closed at a gate. | A scenario failed, or no simulation (unknown Spec, no valid model). |
| 2 | The workspace cannot be opened. | Unusable config (workspace, missing API key) or invalid arguments. | The workflow could not be opened (target, database) or invalid arguments. | Unusable config or invalid arguments. |
| 130 | — | Interrupted with `Ctrl+C`. | — (see below) | — |

## Headless mode

`crest --headless "<prompt>"` runs **one** turn and prints its events to the terminal:

- the text, streamed;
- the thinking, collapsed to one line;
- one line per tool and status;
- the colored diffs;
- notices and errors;
- at the end, the token usage.

Each approval shows the diff or the command and asks:

```text
Allow edit_file src/lib.rs? [y]es / [a]lways / [n]o:
```

| Answer | Effect |
| --- | --- |
| `y`, `yes` (Spanish aliases: `s`, `si`, `sí`) | Approve. |
| `a`, `always` (alias: `siempre`) | Always approve (for this exact subject). |
| `n`, `no`, or an empty line | Reject. |

Any other answer asks again. If stdin closes, the action is rejected. With `--yes` everything is approved and logged as `approved by --yes: <tool> <subject>`.

Answers can also be piped in: if stdin is not a terminal, crest echoes every line it reads so it shows up in the transcript.

```bash
printf 'y\n' | crest --headless "add a test for parse()" --workspace .
```

`Ctrl+C` during a headless turn:

- the first one cancels the turn (`^C interrupting the turn (again to quit)`) and exits with 130 after saving the session;
- the second one exits immediately with 130.

With `--research` and `--implement`, however, `Ctrl+C` kills the process: there is no orderly cancellation, and the workflow stays at its last checkpoint.

Without an API key, headless mode exits with the key error and code 2. The window, by contrast, opens anyway and shows that error as its first block.

## Sessions and resuming

- Each session (the history sent to the model, the timeline and the "always" rules) is saved in SQLite (`~/.local/share/crest/sessions.db`) **after every turn**.
- **Window**: on startup it resumes the **workspace's last session** (identified by the directory's canonical path). It redraws its timeline and shows "previous session resumed (N turns); `crest --new` starts a new one". With `--new` it starts blank, and `/clear` does the same without closing.
- **Headless**: each run is a new session unless you pass `--continue`, which picks up the workspace's last session (the same one the window would open).
- Old sessions are never deleted, and there is no command to list them or to pick one other than the last.
- The accumulated cost is not saved: a resumed session starts at `$0.00`.
- **Compaction**: when the estimated context exceeds 80 % of 200,000 tokens, crest summarizes the old turns with the `compact` role, keeps the 2 most recent ones and shows `history compacted: N turns → summary`. If the API replies that the context is too long, it compacts and retries once. The threshold is not configurable.
- A turn can make at most 50 model calls. Beyond that, it fails.

## Agent tools

| Tool | Input | Effect |
| --- | --- | --- |
| `list_files` | `glob?`, `limit` (500) | read |
| `find_file` | `query`, `limit` (20) | read |
| `tree` | `path?`, `depth` (3) | read |
| `grep` | `pattern`, `path?`, `glob?`, `fixed`, `ignore_case`, `context` (2), `limit` (100) | read |
| `read_file` | `path`, `offset?`, `limit?` (2000 lines; without a range it does not read binaries or files over 2 MiB) | read |
| `edit_file` | `path`, `old`, `new` (`old` must appear exactly once) | write |
| `write_file` | `path`, `content` | write |
| `run_command` | `command`, `args[]`, `timeout_s` (120). No shell, at the workspace root. | execute (read for some `git` commands) |
| `show_chart`, `show_table`, `show_mermaid`, `show_markdown` | the block's data | read |
| `web_search` | `query`, `max_results?` (1–10; `app.opt.web.max_results`, 8, by default): title, URL and snippet of each result | network, no approval |
| `web_fetch` | `url` (http or https): the page as text, with its final URL and title | network, no approval |
| `task` | `description`, `prompt`: launches a read-only subagent with the `explore` role (at most 25 calls, with the web tools when they are on) that returns a summary | read |
| `simulate_solution` | `spec`: a folder of `.crest/specs/`, its number or its path. Simulates the solution of that Spec, records the results in its folder and opens the board in the window; returns the verdicts | write |
| `mcp__<server>__<tool>` | the server's schema | the server's `effect` (execute by default) |
| plugin tools | their `schema` | their `effect` (execute by default) |

### Researching on the web

Ask for it in plain words ("investigate how other crates solve X", "compare A and B", "what changed in version N"): the model searches with `web_search`, reads the best pages with `web_fetch`, searches again where the answer is incomplete, and can split a broad question into `task` subagents that research in parallel. It cites the URL of each source, and after the answer a **Sources** block lists the pages it read (clickable in the window; in headless mode, a numbered `sources:` list). The searches run on your machine with curl; see [configuration.md](configuration.md#web-search) for the backend, the domains and how to turn the web off.

All paths are relative to the workspace. A path that leaves it, even through a symlink, fails, and so does a sensitive path. Each result is truncated to about 32 KB, with the note `N more results; refine the search`.

## MCP servers and trust

Servers are declared in Lua ([plugins.md](plugins.md#appmcp-mcp-servers)). Their tools appear as `mcp__<server>__<tool>` and go through the same approvals as everything else, according to the server's `effect`.

- **Global config and plugins**: they start when crest opens.
- **Workspace config** (`.crest/init.lua`): they **do not start** until you trust them, because running them means running a program or contacting a URL chosen by whoever wrote the repository. Until then you see the notice:

  ```text
  workspace MCP server `tickets` not started: it would connect to https://tickets.example.com/mcp. If you trust it, start crest with `crest --trust-mcp tickets` (remembered for this workspace)
  ```

  To trust one:
  - `crest --trust-mcp tickets` approves it and starts it in this same run;
  - `/mcp trust tickets`, from the window, approves it and it starts the next time you open crest.

  The approval is stored in `~/.local/share/crest/mcp-trust.json`, per workspace and with the exact transport (command, arguments, environment, URL, headers). If the repository changes the declaration, it becomes pending again. To revoke it, delete the entry from the file.
- On startup, crest waits up to 5 s for the servers to be ready. One that fails or is slow is reported as a notice, and crest carries on without it.

## Plugin capabilities and trust

A plugin can ask to make HTTP requests to some hosts or to run some programs by declaring capabilities with `app.plugin` ([plugins.md](plugins.md#capabilities)). Nothing is active until you approve it. On startup each pending declaration shows a notice:

```text
plugin `jira` requests network: jira.example.com; shell: jq — approve with `crest --trust-plugin jira` or `/plugin trust jira`
```

- `crest --trust-plugin jira` approves it for this run and remembers it.
- `/plugin trust jira`, from the window, approves it and activates it immediately.
- `/plugin list` shows every declaration and its state.

This applies to plugins of the global config too, not only to the workspace's. The approval is stored in `~/.local/share/crest/plugin-trust.json` with the plugin's name, its file and the exact list: if the plugin asks for something else, it becomes pending again. Until then, its calls to `app.http`, `app.shell.run` or `app.secret` fail with an error that says how to approve it, and the model sees that error. The tools of the plugin keep going through the normal approvals.
