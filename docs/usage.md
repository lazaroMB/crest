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

Notices (for example «retrying in 2.0s», «x failed, using y» or a model fallback) stay for 6 s.

Each prompt runs as a **turn**, and only one turn runs at a time. If you send a prompt while a turn is running, it is **queued**: the bar shows how many are waiting, and it is sent when the turn finishes. `Ctrl+C` interrupts the turn and clears the queue. `Ctrl+Q` (`Cmd+Q` on macOS) or `:q` quits crest.

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
| command | `Esc` | `mode.normal` | Cancels the command. |
| all | `Ctrl+C` | `turn.interrupt` | Interrupts the running turn. Also works while a picker or a dialog is open. |
| all | `Ctrl+Q` / `Cmd+Q` | — | Quits crest. |

You can redefine or add mappings with `app.keymap.set` ([plugins.md](plugins.md#appkeymapsetmode-keys-action)).

### `:` commands

| Command | What it does |
| --- | --- |
| `:q`, `:quit`, `:qa` | Quits crest. |
| `:model <alias\|role\|id>` | Model for the **next** turn. Accepts an alias (`opus`), a role name (`plan`, which uses that role's whole spec) or a full id. With no argument it goes back to the default model. |
| `:mode <ask\|auto-edit\|plan>` | Changes the permission mode. |
| `:clear` | Starts a new session with an empty timeline. The previous one stays saved. Does not work while a turn is running. |
| `:mcp trust <name>` | Approves a workspace MCP server. It starts the next time you open crest. |
| `:implement <spec>` | Starts or resumes the implementation workflow for a Spec ([workflows.md](workflows.md#implementation-workflow)). |
| `:research <problem>`, `:research resume`, `:research` | Starts, resumes or queries the research workflow ([workflows.md](workflows.md#research-workflow)). |
| `:<name> <args>` | A command registered with `app.commands.register`. |
| `:<dotted.action>` | Runs a builtin action, for example `:picker.files` or `:panel.tree`. |

An unknown command shows «comando desconocido :x (usa :model, :mode, :clear, :mcp, :implement, :research o :q)».

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
- The tree refreshes by itself when files change (it watches the directory, with a 200 ms debounce). If the watcher cannot start, the notice «el árbol no se refrescará solo» appears.

### Model menu (`<Space>m`)

Lists «default (rol main)» and every alias, both the builtin ones (`opus`, `sonnet`, `haiku`, `fable`) and those in your config, with the model each one points to.

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

To change mode: `<Space>p`, `:mode <mode>` or `--mode` at startup.

`run_command` counts as a read only for `git log`, `git blame`, `git diff`, `git status` and `git show` without options that write (`--output`, `--no-index`, `--contents`, `--ext-diff`). Any other command counts as execution.

When approval is needed, a dialog opens with the colored diff (for edits) or with the exact command and its directory (for `run_command`):

| Key | Answer |
| --- | --- |
| `y` | Approve this once. |
| `a` | Always approve: creates a rule for **that same tool and that exact subject** (the same path or the same command line, nothing broader). |
| `n` | Reject. It first asks for an optional reason, which is passed to the model: type it and press `Enter`, or `Esc` to go back. |
| `Esc` | Reject without a reason. |

There are also «Approve (y)», «Always (a)» and «Reject (n)» buttons. If several requests arrive, they wait in a queue. A denied tool (by you, by the mode or by a hook) is not retried automatically: the model gets the reason and decides.

"Always" rules are saved with the session and come back when it is resumed. `--new` and headless without `--continue` start with no rules, while `:clear` keeps those of the current run.

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
| `--headless [<PROMPT>]` | text | Runs one turn with that prompt in the terminal, without a window. Without a prompt, it goes with `--implement` or `--research`. |
| `--implement <RUTA>` | Spec or `plan.json` | In headless mode, runs the implementation workflow. Requires `--headless` and cannot be combined with `--research`. |
| `--research <PROBLEMA>` | text or `resume` | In headless mode, runs the research workflow. Requires `--headless`. |
| `--model <ALIAS\|ID>` | alias, role or id | Model override for **every** turn of this run (like `:model`, but permanent). |
| `--mode <MODE>` | `ask`, `auto-edit`, `plan` | Initial permission mode. Defaults to `ask`. |
| `--yes` | — | In headless mode, approves every action without asking. Does not answer workflow gates. Requires `--headless`. |
| `--new` | — | The window opens with a new session instead of resuming the last one. Cannot be combined with `--headless`. |
| `--continue` | — | In headless mode, continues the workspace's last session, with its history. Requires `--headless`. |
| `--trust-mcp <NAME>` | name | Approves the MCP server `NAME` from the workspace's `.crest/init.lua`, starts it and remembers it. Can be repeated. |
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
crest --headless --implement 0001-add-suma-negativos
crest --trust-mcp tickets --trust-mcp docs
```

With `make` (`WORKSPACE` defaults to `.`):

```bash
make run WORKSPACE=~/src/api
make run-headless PROMPT="list the project's TODOs" WORKSPACE=~/src/api
```

### Exit codes

| Code | Window | Headless (turn) | Headless (workflows) |
| --- | --- | --- | --- |
| 0 | Closed normally. | The turn finished. | The workflow completed. |
| 1 | The window could not open. | The turn failed (model error, more than 50 model calls in one turn). | Aborted, sent back to the Spec, a step error, or stdin closed at a gate. |
| 2 | The workspace cannot be opened. | Unusable config (workspace, missing API key) or invalid arguments. | The workflow could not be opened (target, database) or invalid arguments. |
| 130 | — | Interrupted with `Ctrl+C`. | — (see below) |

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
¿Permitir edit_file src/lib.rs? [y]es / [a]lways / [n]o:
```

| Answer | Effect |
| --- | --- |
| `y`, `yes`, `s`, `si`, `sí` | Approve. |
| `a`, `always`, `siempre` | Always approve (for this exact subject). |
| `n`, `no`, or an empty line | Reject. |

Any other answer asks again. If stdin closes, the action is rejected. With `--yes` everything is approved and logged as «aprobado por --yes».

Answers can also be piped in: if stdin is not a terminal, crest echoes every line it reads so it shows up in the transcript.

```bash
printf 'y\n' | crest --headless "add a test for parse()" --workspace .
```

`Ctrl+C` during a headless turn:

- the first one cancels the turn («^C interrumpiendo el turno (otra vez para salir)») and exits with 130 after saving the session;
- the second one exits immediately with 130.

With `--research` and `--implement`, however, `Ctrl+C` kills the process: there is no orderly cancellation, and the workflow stays at its last checkpoint.

Without an API key, headless mode exits with the key error and code 2. The window, by contrast, opens anyway and shows that error as its first block.

## Sessions and resuming

- Each session (the history sent to the model, the timeline and the "always" rules) is saved in SQLite (`~/.local/share/crest/sessions.db`) **after every turn**.
- **Window**: on startup it resumes the **workspace's last session** (identified by the directory's canonical path). It redraws its timeline and shows «sesión anterior reanudada (N turnos); `crest --new` empieza una nueva». With `--new` it starts blank, and `:clear` does the same without closing.
- **Headless**: each run is a new session unless you pass `--continue`, which picks up the workspace's last session (the same one the window would open).
- Old sessions are never deleted, and there is no command to list them or to pick one other than the last.
- The accumulated cost is not saved: a resumed session starts at `$0.00`.
- **Compaction**: when the estimated context exceeds 80 % of 200,000 tokens, crest summarizes the old turns with the `compact` role, keeps the 2 most recent ones and shows «historial compactado: N turnos → resumen». If the API replies that the context is too long, it compacts and retries once. The threshold is not configurable.
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
| `task` | `description`, `prompt`: launches a read-only subagent with the `explore` role (at most 25 calls) that returns a summary | read |
| `mcp__<server>__<tool>` | the server's schema | the server's `effect` (execute by default) |
| plugin tools | their `schema` | their `effect` (execute by default) |

All paths are relative to the workspace. A path that leaves it, even through a symlink, fails, and so does a sensitive path. Each result is truncated to about 32 KB, with the note «N resultados más; refina la búsqueda».

## MCP servers and trust

Servers are declared in Lua ([plugins.md](plugins.md#appmcp-mcp-servers)). Their tools appear as `mcp__<server>__<tool>` and go through the same approvals as everything else, according to the server's `effect`.

- **Global config and plugins**: they start when crest opens.
- **Workspace config** (`.crest/init.lua`): they **do not start** until you trust them, because running them means running a program or contacting a URL chosen by whoever wrote the repository. Until then you see the notice:

  ```text
  servidor MCP `tickets` del workspace no iniciado: conectaría con https://tickets.example.com/mcp.
  Si confías en él, arranca con `crest --trust-mcp tickets` (se recuerda para este workspace)
  ```

  To trust one:
  - `crest --trust-mcp tickets` approves it and starts it in this same run;
  - `:mcp trust tickets`, from the window, approves it and it starts the next time you open crest.

  The approval is stored in `~/.local/share/crest/mcp-trust.json`, per workspace and with the exact transport (command, arguments, environment, URL, headers). If the repository changes the declaration, it becomes pending again. To revoke it, delete the entry from the file.
- On startup, crest waits up to 5 s for the servers to be ready. One that fails or is slow is reported as a notice, and crest carries on without it.
