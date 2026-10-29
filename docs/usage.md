# Usage

This guide covers the window, the command line, headless mode, sessions and MCP servers. The research and implementation workflows have a guide of their own: [workflows.md](workflows.md).

## The window

```bash
crest ~/src/proyecto  # workspace = ~/src/proyecto (or --workspace ~/src/proyecto)
crest .               # workspace = current directory
crest                 # no project yet: the project picker (also --pick)
```

The window opens full screen (maximized on Windows); `app.opt.window` in the global `init.lua` makes it open maximized or as a window of a given size ([themes.md](themes.md#window-mode)), and `F11` toggles full screen at any time. On X11 it opens as a normal window and requests full screen (or maximized) after the first frame; if the compositor refuses, it stays centered at 1280×800 (or the size you set). The title is `crest — <directory>`. The colors come from a **theme**: TwoCrest (the palette of [twocrest.mx](https://twocrest.mx): ink, paper, a coral accent and amber) dark by default, TwoCrest light, or any theme in the `themes/` folder of your config, such as the Neovim favourites the installers put there. `/theme` previews them as you move through the list and saves your pick; `app.opt.theme` fixes one ([themes.md](themes.md)). The window uses a single monospace font: JetBrains Mono if installed, otherwise the system monospace font. An empty session shows the TwoCrest mark and the keys to start with.

```text
┌──────────────┬───────────────────────────────────────────────┐
│ tree         │ timeline: blocks (text, tools, diffs,         │
│ (Ctrl+G e,   │ charts, tables, Mermaid, markdown, shell...)  │
│  hidden      │                                               │
│  by default) │                                               │
│              ├───────────────────────────────────────────────┤
│              │ ❯ multi-line prompt (/ opens the palette)     │
│              ├───────────────────────────────────────────────┤
│              │ ▲▲ │ model │ ask │ $0.00   ⟨notice⟩ main ~2 │ project │
└──────────────┴───────────────────────────────────────────────┘
```

From left to right, the status bar shows:

- the crest mark;
- the model of the current or last turn;
- the permission mode;
- the session cost;
- on the right, the git branch and its changes when the workspace is a repository (`main +1 ~2 ?3 ↑1`: staged, modified, untracked, conflicted, ahead and behind its upstream; green when clean, amber with changes, red with conflicts), refreshed at start, after each turn or command and every 5 seconds;
- the workspace.

Notices (for example `retrying in 2.0s (attempt 2)`, `opus failed, using sonnet` or a model fallback) stay for 6 s.

Each prompt runs as a **turn**, and only one turn runs at a time. While crest works (a turn, or a command such as `/research` or `/implement` running up to its next question) a line above the prompt says so: a spinner, the stage, what is going on now (the model a stage asks, a tool call, a check) and the elapsed time. If you send a prompt meanwhile, it is **queued**: the queued prompts are listed under that line and sent one by one when crest is free. `Up` in an empty prompt takes the last queued one back to edit it (empty the prompt to drop it). `Ctrl+C` in an empty prompt interrupts the turn, or the running command (`/research`, `/implement`, `/!cmd`...), and clears the queue. An interrupted flow stays saved at its last checkpoint: `/research resume` or `/implement resume` continues it. `Ctrl+Q` (`Cmd+Q` on macOS) or `/q` quits crest; `F11` toggles full screen.

### Typing, commands and shortcuts

There are no modes: the prompt is always ready to type (a multi-line editor, up to 8 rows before it scrolls).

- `Enter` sends the prompt; `Shift+Enter` adds a line.
- A line that starts with `/` is a **command** (`/model opus`, `/research …`). To send a prompt that starts with a slash, double it: `//etc/hosts` sends `/etc/hosts`.
- Typing `/` opens the **command palette** above the prompt, filtered as you type the name: `↑`/`↓` choose, `Tab` completes, `Enter` runs the chosen command (or, when it needs an argument, completes it up to there: `/implement ` waits for the Spec), `Esc` hides it.
- **Shortcuts** are chords (`Ctrl+C`, `Ctrl+Tab`) or the **leader** chord, `Ctrl+G`, followed by vim-style keys (`Ctrl+G e`, `Ctrl+G j`). The leader is `Ctrl+G` on Linux and on macOS alike: `Ctrl+G` is taken by macOS to switch the input source, and `Alt`/`Option` composes characters on a Mac, so no builtin shortcut uses Alt. They work wherever the focus is. While crest waits for the next key of a sequence, the line under the prompt shows what you have typed so far; after `timeoutlen` (1 s by default) an exact mapping runs if there is one, otherwise the keys are dropped.

### Focus

The prompt has the focus at start and gets it back by itself after a picker, a menu, a dialog or a modal closes.

- `Ctrl+Tab` (or `Ctrl+G w`) moves the focus prompt → timeline → tree (when shown) → prompt. The focused panel has an accent edge.
- `Esc` goes back to the prompt from anywhere.
- Typing a letter while the timeline or the tree has the focus types it into the prompt and moves the focus there, except the vim keys of the timeline below.

| Panel | Keys while it has the focus |
| --- | --- |
| Timeline | `k` / `j` or `↑` / `↓` select blocks, `g` / `G` or `Home` / `End` the first and last, `o` or `Enter` expands or collapses, `t` switches the view, `y` copies the block, `PageUp` / `PageDown` scroll. |
| Tree | `↑` / `↓` move, `→` / `←` open and close directories, `Enter` on a file inserts `@path`. |
| Help | See [Help](#help). |
| Plugin modal | `↑` / `↓`, `PageUp` / `PageDown`, `Home` / `End` scroll, `Esc` closes. |
| Approval dialog | `y`, `a`, `n`, `Esc`; `↑` / `↓`, `PageUp` / `PageDown` scroll the preview. |
| Simulation board | See [Simulation board](#simulation-board). |

### Default key bindings

`<leader>` is `Ctrl+G` in every builtin mapping. Changing `app.opt.leader` only affects mappings defined in Lua (see [plugins.md](plugins.md#4-keymaps)).

| Key | Action | What it does |
| --- | --- | --- |
| `Enter` | `submit` | Sends the prompt (if not empty) or runs the `/` command. |
| `Shift+Enter` | `newline` | Inserts a new line. |
| `Esc` | `focus.input` | Back to the prompt (hides the palette when it is open). |
| `Ctrl+Tab`, `Ctrl+G w` | `focus.next` | Moves the focus to the next panel. |
| `Ctrl+G k` / `Ctrl+G j` | `block.prev` / `block.next` | Previous / next block. |
| `Ctrl+G gg` / `Ctrl+G G` | `block.first` / `block.last` | First / last block. |
| `Ctrl+G o` | `block.toggle` | Expands or collapses the block (ToolCall, Thinking, Shell). |
| `Ctrl+G t` | `block.toggle_view` | Alternate view of the block (see [Blocks](#blocks)). |
| `Ctrl+G y` | `block.yank` | Copies the block's content to the clipboard. |
| `Ctrl+G i` | `image.mode` | Switches [pasted images](#pasted-images) between **→ Model** and **Chat only**. |
| `Ctrl+G ff` | `picker.files` | File picker (fzf). |
| `Ctrl+G fg` | `picker.grep` | Text search (rg). |
| `Ctrl+G e` | `panel.tree` | Shows or hides the workspace tree (shown, it takes the focus). |
| `Ctrl+G m` | `model.pick` | Puts `/model ` in the prompt, which opens the model popup (the same as `/model`). |
| `Ctrl+G p` | `permission.cycle` | Cycles `ask` → `auto-edit` → `plan`. |
| `Ctrl+G h`, `F1` | `help.open` | Opens the [help](#help). |
| `Ctrl+C` | `input.cancel` | In the prompt, as in a terminal: copies the text selected in the prompt; else clears the prompt (`Up` brings the text back); else interrupts the running turn or command. So with text in the prompt, `Ctrl+C Ctrl+C` clears and interrupts. With the focus on the chat and text selected in it, it copies that text; without a selection it interrupts. While a picker or a dialog is open it only interrupts. |
| `Ctrl+Shift+C`, `Ctrl+Insert` | `selection.copy` | Copies the selected text, from the prompt or else from the chat. Never interrupts. (On macOS, `Cmd+C`.) Selecting text with the mouse in the chat gives it the focus and shows a **Copy** hint next to the selection. |
| `Ctrl+Q` / `Cmd+Q` | — | Quits crest. |

You can redefine or add mappings with `app.keymap.set` ([plugins.md](plugins.md#appkeymapsetkeys-action)). A mapping must start with a chord or the leader: one that starts with a plain key (`j`, `?`) would steal typing, so it is ignored with a warning.

### `/` commands

| Command | What it does |
| --- | --- |
| `/q`, `/quit`, `/qa` | Quits crest. |
| `/help [section\|search]`, `/h` | Opens the [help](#help), on a section (`commands`, `keys`, `tools`, `plugins`, `mcp`, `models`, `cli`, `files`, or its number) or with that search already typed. |
| `/theme` | Opens the theme popup: `Up`/`Down` preview each theme in the window, `Enter` saves it (in `preferences.json`), `Tab` completes, `Esc` goes back to the theme in use. `/theme <name>` (or `/theme set <name>`) saves one directly. A theme set in `init.lua` still wins when crest starts. |
| `/model` | Opens the model popup over the models your providers let you use. |
| `/model <alias\|role\|id>` | Model for the **next** turn. Accepts an alias (`opus`), a role name (`plan`, which uses that role's whole spec) or a full id. `/model clear` goes back to the default model. |
| `/model set <id>` | Saves `<id>` as your model for the default provider (as `Enter` in the popup) and reopens the session. |
| `/mode <ask\|auto-edit\|plan>` | Changes the permission mode. |
| `/clear` | Starts a new session with an empty timeline. The previous one stays saved. Does not work while a turn is running. |
| `/modal close` | Closes the front [plugin modal](#plugin-modals). |
| `/project` | Closes this project and goes back to the [project picker](#opening-a-project). Not while a turn or command runs. |
| `/login [anthropic\|opencode]` | Asks for the API key of Anthropic or OpenCode (`Tab` switches) in a masked field, checks it, stores it in the system keyring and restarts the agent (it also opens by itself when there is no key). See [configuration.md](configuration.md#model-providers-and-api-keys). |
| `/logout [anthropic\|opencode]` | Deletes the API key of that account (of every account without an argument) from the system keyring and restarts the agent. |
| `/!<command> [args]`, `/! <command>` | Runs a [shell command](#shell-commands-) in the workspace. |
| `/open @<path>` | Shows a workspace file in a modal ([Opening files](#opening-files-open-path)). |
| `/mcp [list]` | A table of the MCP servers with their state, transport and what to do next. See [configuration.md](configuration.md#mcp). |
| `/mcp tools\|restart\|enable\|disable\|login\|logout <name>` | A server's tools; reconnect it; turn it on or off for the session; OAuth login or logout. |
| `/mcp trust <name>` | Approves a workspace MCP server and starts it. |
| `/plugin list` | Lists the plugins that declare capabilities, with their file, their capabilities and whether they are `approved` or `pending`. |
| `/plugin trust <name>` | Approves the capabilities of plugin `<name>` and activates them immediately (see [Plugin capabilities and trust](#plugin-capabilities-and-trust)). |
| `/implement <spec>`, `/implement resume` | Starts or resumes the implementation workflow for a Spec ([workflows.md](workflows.md#implementation-workflow)). |
| `/research <problem>`, `/research resume [n]`, `/research retry`, `/research list`, `/research` | Starts, resumes, retries, lists or queries the research workflow ([workflows.md](workflows.md#research-workflow)). |
| `/stage`, `/back <stage>`, `/skip`, `/retry`, `/abort` | Commands of the active flow (implementation, or else research). |
| `/simulate <spec>` | Simulates the solution of a Spec (its folder, its number or its path) and opens its board in a full-size modal ([workflows.md](workflows.md#simulating-a-solution)). The verdicts land in the timeline. |
| `/<name> <args>` | A command registered with `app.commands.register`. |
| `/<dotted.action>` | Runs a builtin action, for example `/picker.files` or `/panel.tree`. |

An unknown command shows `unknown command /x (use /help, /model, /mode, /clear, /project, /login, /logout, /modal, /q; /help lists every command)`.

### Shell commands (`/!`)

`/!git status`, `/!cargo test -p ui` or `/! ls -la | head` runs the line with `sh -c` in the workspace (pipes, globs and variables work), without an approval: you typed it. It is cut after 120 s and `Ctrl+C` stops it.

A **Shell** block shows the command, its exit status (`exit 0` in green, otherwise red) and its output, stderr in red; output longer than 20 lines is collapsed (`Ctrl+G o`, or `o`/`Enter` with the timeline focused, expands it). The output also goes to the model with your **next** prompt, tagged as `<bash-input>`, `<bash-stdout>` and `<bash-stderr>` (each stream cut to 20 kB), so you can run a command and then ask about it.

### Help

`F1`, `Ctrl+G h` or `/help` open the help: a modal at 90 % of the window with everything you can use in this session, taken from what is actually loaded (not from this page).

| # | Section | What it lists |
| --- | --- | --- |
| 1 | Commands | The builtin `/` commands with their usage, then the commands of your plugins (with the plugin and its `desc`). |
| 2 | Keys | The effective keymap: the defaults plus your `app.keymap.set` mappings, with the leader resolved, the action, its description and `overrides default` where a mapping replaced a builtin one. |
| 3 | Tools | Every tool the agent has, grouped Built-in / Plugin / MCP, with its description, its effect (`read`, `write`, `execute`, `network`; `depends on input` for `run_command`) and its input schema (`Enter` expands it). |
| 4 | Plugins | Each loaded plugin or config file: its file, layer (`global` or `workspace`), what it adds (tools, commands, keymaps, hooks), its capabilities and trust state (`approved`, `pending` with the `/plugin trust` command, or `no capabilities`) and its load problems. |
| 5 | MCP servers | Name, transport, origin and state: `ready (N tools)`, `failed: …`, `starting`, `needs login` (with `/mcp login`), `disabled` (with `/mcp enable`), or held back because the workspace config is not trusted (with the `/mcp trust` command). |
| 6 | Models | Each role and the model it resolves to, the aliases, the `--model` override, the model of the next turn and the permission mode. |
| 7 | CLI | The flags of `crest`, generated from the command-line parser. |
| 8 | Files & config | The workspace, the config files, the plugins directory, the session database, the trust stores and the startup problems. |

| Key | Effect |
| --- | --- |
| `1`–`8`, `Tab` / `Shift+Tab`, click | Switches section. |
| `j` / `k`, arrows, `PageUp` / `PageDown`, `Home` / `End` | Moves the selection. |
| `Shift+↑` / `Shift+↓`, `Ctrl+D` / `Ctrl+U` | Scrolls without moving the selection (to read a tall expanded row). |
| `Enter` | Expands the selected row (a tool's input schema). |
| `y` | Copies the selected row's command or name (`/plugin trust jira` for a pending plugin). |
| `/` | Goes to the search box: what you type filters every section at once (each word must appear in the row, ignoring case). The arrows still move; `Enter` goes back to the list. |
| `Esc`, `✕` | Closes the help; the focus goes back where it was. |

### File references with `@`

When you pick a file in the tree, in a picker or in a `Sources` block, `@path ` (or `@path:line ` for a grep result) is inserted at the prompt cursor, and the prompt gets the focus. It is plain prompt text: the model reads it as a reference and opens the file with its tools.

You can also type it. While the last word of the prompt starts with `@`, a popup lists the entries of the directory it names, relative to the workspace root (the directory crest was started in, or `--workspace`): `@` lists the root, `@src/` lists `src/`, `@src/co` the entries of `src/` that match `co`. Directories come first, marked `▸` and ending in `/`. `↑`/`↓` choose, `Tab` completes (a directory as `@src/components/`, whose entries are listed next; a file as `@src/main.rs `), `Enter` completes the chosen entry or, once the word already is it, sends the line, and `Esc` hides the popup. Gitignored and sensitive files (`.env*`, `*.pem`...) are not suggested, and a path that leaves the root (`@../`, `@/etc/`) gets no suggestions.

A path with spaces is quoted: `@"My Docs/notes v2.md"`. Completion switches to that form by itself when an entry has a space (`@My` → `@"My Docs/`), keeps the quote open while you go down directories and closes it after a file (`@"My Docs/notes v2.md" `). The tree, the pickers and `Sources` quote the paths they insert the same way, and `/open` accepts the form, closed or not.

### Opening files (`/open @path`)

`/open @src/main.rs` shows the file in a modal (`Esc` closes it). The `@` is not part of the path, which is resolved from the workspace root whatever the directory of a command or subprocess. The viewer depends on the extension: Markdown is rendered, JSON is a collapsible tree (or source when it does not parse), `.mmd`/`.mermaid` a diagram, `.diff`/`.patch` a colored diff, png/jpeg/gif/webp/svg/bmp/ico a picture with zoom and pan, `.obj`/`.stl`/`.ply` a 3D model, and anything else highlighted source, HTML and CSV included. The file is only shown, never run or rendered as a page.

A 3D model (OBJ, ASCII or binary STL, ASCII or binary PLY) opens in a viewer with an orbit camera, fitted to the model. Drag orbits; right-drag, middle-drag or `Shift`+drag pans; the wheel zooms. With the viewer focused (click it), `+`/`-` zoom, `0` fits the model again, `i` switches to the isometric view and `u` swaps the up axis (STL and PLY start with Z up, OBJ with Y up); the buttons in the corner do the same. Only the geometry is shown, flat shaded: materials, textures and colours are ignored. The line under the viewer gives the vertices, triangles and size in the file's units. Without a GPU (or with `CREST_RENDER3D=off`) the model is drawn with 2D paths, at most 20 000 triangles of it.

It follows the file tools' rules: a path outside the workspace (`..`, an absolute path elsewhere, a symlink pointing out) or matching the sensitive-path patterns is refused, and so are directories, binaries that are not pictures or models and files over 2 MiB (64 MiB and a million triangles for a model). Files longer than 20 000 lines are cut, and the title says so. The status bar says why a file did not open.

### Pasted images

Pasting an image (`Ctrl+V` with a screenshot or a copied picture) puts it in a tray above the prompt, with a thumbnail, its size in pixels and bytes, and a `×` to remove it. The text already typed stays, and you can paste several (up to 20). `Backspace` in an empty prompt removes the last one.

A chip in the tray says what `Enter` does with the images, and the line under it repeats it:

- **→ Model** (the default): the images go to the model with the prompt, in the same message after the text, and appear in the timeline under your prompt. If the model of the turn does not take images, nothing is sent and an error says so; an active research or implementation flow does not take them either.
- **Chat only**: the images (and the text, if any) are added to the timeline as Image blocks and saved with the session; the model is **not** called, and the images are not sent with later prompts.

A click on the chip or `Ctrl+G i` switches it; it goes back to **→ Model** after each send. A `/` command leaves the images in the tray.

Images go out as png, jpeg, gif or webp: bmp, tiff, ico and pnm are converted to png first, and svg is refused. An image larger than 8000 px on a side or 5 MB is refused with its size. Images are kept in memory until sent (nothing is written to disk); sent ones are stored in the session database with the conversation. Clicking an Image block in the timeline opens it in a modal with zoom and pan.

On Wayland, a copy that offers both text and an image may paste as text (GPUI reads the text first); screenshots paste as images. Copied files (from a file manager) are not attached.

### Pickers (`Ctrl+G ff`, `Ctrl+G fg`)

They use the same ports and the same defaults as the agent's `find_file` and `grep` tools, so you see what the model would see:

- they respect `.gitignore`;
- they exclude `.git/`;
- they apply the sensitive paths.

Results update as you type, with a 120 ms debounce. In `Ctrl+G fg` the text is a ripgrep regular expression.

| Key | Action |
| --- | --- |
| `↑` / `↓`, `Ctrl+P` / `Ctrl+N` | Move the selection. |
| `Enter` | Insert the `@...` reference into the prompt. |
| `Esc` | Close. |

### Workspace tree (`Ctrl+G e`)

A 280 px side panel, hidden at startup; shown, it takes the focus (`↑`/`↓` move, `→`/`←` open and close directories, `Enter` on a file inserts `@path`, typing or `Esc` goes back to the prompt). It comes from the same `tree` the agent uses: it shows up to 16 levels and leaves out ignored files and `.git/`.

- Clicking a directory opens or closes it.
- Clicking a file inserts `@path`.
- The tree refreshes by itself when files change (it watches the directory, with a 200 ms debounce). If the watcher cannot start, the notice `the tree will not refresh by itself: …` appears.

### Model popup (`Ctrl+G m`, `/model`)

`Ctrl+G m` and `/model` put `/model ` in the prompt. While the line is `/model ` (or `/model set `) followed by a word, a popup above the prompt lists, like the `@` one, the models that the providers with a key offer **and your account can use** (`GET /v1/models`, listed once per session, with their prefix: `opencode-zen:glm-5.3`), plus the aliases whose model is among them. The word filters them: ids or aliases that start with it (also after the prefix) come first, then those that contain it. `●` marks the model of the next turn.

| Key | Action |
| --- | --- |
| `↓` / `↑` | Move the selection. |
| typing | Filter by alias or id. |
| `Enter` | Save the model as yours for the default provider (`/model set <id>`); the session reopens (and resumes) so every role and workflow uses it. |
| `Tab` | Complete the line to `/model set <id>`. |
| `Esc` | Hide the popup; `Enter` then sends the line as typed (`/model opus`: the next turn only). |

The popup opens by itself when the default provider has no model yet (OpenCode right after `/login opencode`). See [Model providers](configuration.md#model-providers-and-api-keys).

### Approvals and permission modes

Each tool declares its effect (`read`, `write`, `execute` or `network`), and the permission mode decides what happens:

| Mode | Read | Edit files | Run commands / network |
| --- | --- | --- | --- |
| `ask` (default) | yes | asks | asks |
| `auto-edit` | yes | yes | asks |
| `plan` | yes | no (denied) | no (denied) |

To change mode: `Ctrl+G p`, `/mode <mode>` or `--mode` at startup.

`run_command` counts as a read only for `git log`, `git blame`, `git diff`, `git status` and `git show` without options that write (`--output`, `--no-index`, `--contents`, `--ext-diff`). Any other command counts as execution.

When approval is needed, a dialog opens with the colored diff (for edits) or with the exact command and its directory (for `run_command`):

| Key | Answer |
| --- | --- |
| `y` | Approve this once. |
| `a` | Always approve: creates a rule for **that same tool and that exact subject** (the same path or the same command line, nothing broader). |
| `n` | Reject. It first asks for an optional reason, which is passed to the model: type it and press `Enter`, or `Esc` to go back. |
| `Esc` | Reject without a reason. |
| `↑` / `↓`, `PageUp` / `PageDown`, `Home` / `End` | Scroll a long preview. |

There are also `Approve (y)`, `Always (a)` and `Reject (n)` buttons. If several requests arrive, they wait in a queue. A denied tool (by you, by the mode or by a hook) is not retried automatically: the model gets the reason and decides.

"Always" rules are saved with the session and come back when it is resumed. `--new` and headless without `--continue` start with no rules, while `/clear` keeps those of the current run.

The same dialog appears when the session reaches its spending cap (`app.opt.max_session_cost`). See [configuration.md](configuration.md#cost-and-spending-cap).

### Blocks

| Block | Source | Interaction |
| --- | --- | --- |
| Text | Model reply (streamed markdown); your prompt appears quoted with `>`. | — |
| Thinking | Extended thinking. | Collapsed to one line; `Enter` expands it. |
| ToolCall | Any tool: one line with name, subject and status (pending, awaiting approval, running, ok, failed, denied, cancelled). | `Enter` shows the input and the output. |
| Shell | `/!command` | `$ command`, exit status and output; long output collapsed until `Enter`. ANSI colours and bold/dim/italic/underline are drawn, the 16 basic colours from the theme's `ansi.*` tokens. |
| Diff | `edit_file`, `write_file` | Colored diff with its approval status. |
| Chart | `show_chart` tool (bar, line, area, pie). | **`t` toggles a table** of the same data. Hovering a bar shows its value; on lines and areas a crosshair shows every series at that point. Many categories or long labels switch bars to horizontal. |
| Table | `show_table` tool | Fills the block's width; long cells wrap (up to 8 lines) and numbers stay on one line, right-aligned. Shows 12 rows until **Show all N rows** (at most 500 drawn; `y` copies all). Clicking a header cycles descending → ascending → original, over every row. |
| Mermaid | `show_mermaid` tool | Dragging pans the diagram; `Ctrl`/`Cmd` + wheel zooms; with the diagram focused (click), `+`/`-` zoom and `0` resets. `t` shows the source. If it does not parse, the source and the error are shown. |
| Markdown | `show_markdown` tool | Document typography; `t` shows the source. |
| Custom | Lua plugin tools | See [plugins.md](plugins.md#appui-custom-blocks). |
| Sources, StageHeader | Research workflow | In Sources, clicking (or `Enter` on) a file inserts `@path:line`, and on a URL opens the browser. |
| Image | [Pasted images](#pasted-images), research workflow, `show_wavedrom` | Kept at its aspect ratio, at most 360 px high; a click opens it in a modal with zoom and pan. SVG is rasterized off the UI thread at the window's scale (sharper when zoomed). |
| Computed | `stats_*`, `calc`, `convert`, `solve_lp`, `solve_equations`, `monte_carlo`, `cash_flow`, `decision_matrix`, `app.compute.*` | A card of engine numbers; see [Statistics and calculations](#statistics-and-calculations). |
| Form | `ask_user` | Typed questions the turn waits on; see [Forms](#forms-ask_user). |
| Plot | `show_plot`, `app.ui.plot` | Functions and xy data on linear or log axes, a slider per parameter; hovering shows a crosshair with the values and a dot per layer. |
| Test report | `show_test_report` | Totals by outcome, failures first; a click on a chip filters, on a case shows its trace, on its location opens the file. |
| Findings | `show_findings` | SARIF results by file and severity; chips filter, the line opens the file there, `docs` opens the rule's page, a suggested fix shows as a diff. |
| Dataset | `show_dataset`, `app.formats.dataset` | Shape, a profile row per column (type, missing share, distinct, min, max, sparkline) and the first rows as a sortable table. |
| Molecule | `show_molecule`, `app.formats.molecule` | 2D structure tinted to the theme and a strip with formula, MW, charge and canonical SMILES; a chip compares the model's formula. |
| Plate | `show_plate`, `app.ui.plate` | A 96- or 384-well heatmap with a sequential, diverging or log scale and a legend; each well has a tooltip with its name, label and value. |
| Trace | `show_trace`, `app.formats.trace` | A distributed trace as a waterfall: one row per span under its parent, coloured by service, failed spans in red, events as ticks; `Ctrl`/`Cmd` + wheel zooms, dragging pans, hovering shows the attributes and `critical path` keeps only the spans on it. |
| Flame graph | `show_flamegraph`, `app.formats.flamegraph` | A profile as an icicle (callers above callees), widths by value; a click zooms into a frame, `reset` or Escape goes back, the search box highlights matching frames with their share, and a table lists the functions with the most self time. |
| Logs | `show_logs`, `app.formats.logs` | A log file as a filterable list with level colours, a histogram of lines over time, chips by level and by fields such as the service, a text or `/regex/` filter, follow mode; a row opens to its fields and stack trace. |
| Error | API, tool or config failures | Message and suggested action. |

In the table, `Enter` means `Ctrl+G o` from the prompt or `o`/`Enter` with the timeline focused, and `t` means `Ctrl+G t` from the prompt or `t` with the timeline focused. `t` only has an effect on Chart, Markdown and Mermaid. `Ctrl+G y` (or `y` with the timeline focused) copies the content of the selected block.

**Copy as.** The selected block shows a `copy as` row under it with the other forms it can be copied in: TSV and CSV for tables and charts (a chart as its data table), the JSON of a plot, the Mermaid source, the failing tests of a report, the `file:line` list of findings, the plain output of a Shell block (without colours), the profile and preview CSV of a dataset, the SMILES, canonical SMILES, formula and SVG of a molecule, a plate as CSV, the spans CSV and critical path of a trace, the folded stacks and self-time CSV of a flame graph, the lines of a log, and for a computed card the APA line, its reproduce lines (Lua, Python, R, shell) and the envelope JSON.

### Statistics and calculations

The model is told never to compute numbers in its head: it calls an engine, and the engine's numbers are the only ones the card shows.

| Tool | What the engine does |
| --- | --- |
| `stats_describe` | n, missing, mean, SD, median, quartiles, min/max, skew and kurtosis per variable (and per group with `by`); counts and percentages for categorical columns. |
| `stats_test` | Welch, Student, paired and one-sample t, Mann-Whitney, Wilcoxon, χ² and Fisher 2×2; `auto` chooses from Shapiro-Wilk and Brown-Forsythe checks. Effect sizes (Cohen's d, Hedges g with CIs, rank-biserial r, Cramér's V, odds ratio). From the third test of a session every card carries a `multiple_comparisons` warning with the count, suggesting Holm or a pre-registered plan. |
| `stats_regress` | OLS with coefficients, SE, t, p, CI, standardized β and VIF; R², adjusted R², F, AIC/BIC; Durbin-Watson, Jarque-Bera, Breusch-Pagan, the largest Cook's distance and the condition number as diagnostics. |
| `stats_correlate` | Pearson, Spearman or Kendall matrix with Holm (default), BH or Bonferroni adjustment. |
| `calc` | A sheet of named lines (`v = 100 km/h`, `t = 90 min`, `d = v * t -> km`) evaluated in order by numbat; each line shows its value, unit and dimension, and a unit error appears on its line. |
| `convert` | A unit conversion with the factor, whether it is exact, the dimension and an optional uncertainty. |
| `stats_distribution` | One numeric variable (per group with `by`): histogram with the binning rule and width stated (Freedman-Diaconis by default, Sturges, Scott or fixed), Gaussian density with its bandwidth rule (Silverman, Scott or fixed), box and violin with 1.5×IQR outliers, normal QQ with a pointwise band, ECDF with its DKW band. |
| `stats_contingency` | Two categorical columns (`row`, `column`) or the counts in `table`: observed and expected counts, row and column percentages, adjusted standardised residuals as a heatmap, Pearson's χ² or, with `test: auto`, Fisher's exact test when an expected count is below 5 (any r×c; simulated with a seed when too large to enumerate), Cramér's V and the odds ratio with its interval on 2×2 tables. |
| `stats_power` | Power and sample size for two-sample, one-sample and paired t, one-way ANOVA and two proportions (R's `pwr` formulas on noncentral t and F); solves for whichever of `n`, `power`, `effect` or `alpha` is missing and draws the power curve. |
| `stats_ab_test` | Visitors and conversions per arm: each rate with its interval, absolute and relative lift with intervals, a z test (Fisher when counts are small), achieved power, a sample-ratio-mismatch χ² against `expected_split`, and a peeking warning from `looks` and `planned_visitors`. |
| `stats_bootstrap` | A seeded bootstrap of the mean, median, SD, trimmed mean or a difference between two groups: B resamples (2000 by default), percentile, BCa, basic and normal intervals, the Monte Carlo SE and a histogram with the observed statistic. The seed is in the card, so Recompute reproduces it. |
| `stats_forecast` | MSTL decomposition (trend, one panel per season in `periods`, remainder) and an AutoETS forecast of `horizon` steps with 80 and 95 % bands; with `holdout` the last points are kept back and MAPE/sMAPE reported. Warns when the horizon goes far beyond the training range. |
| `solve_lp` | A linear or mixed-integer program (`objective` and `constraints` in calculator notation over `variables` with bounds, `integer` or `binary` flags): status, objective, each variable with its reduced cost, each constraint with slack, whether it binds and its dual, and a sentence per binding constraint ("one more hour of `labor` is worth 12.50"). |
| `solve_equations` | One equation or a small square system (`lhs = rhs` in calculator notation) by a bracket scan over `range` or damped Newton from the `guess` of each unknown: the solution, ‖F(x)‖ and per-equation residuals, iterations, tolerance and a plug-back row per equation. |
| `monte_carlo` | A seeded simulation of `model` over `inputs` with named distributions (fixed, uniform, normal, lognormal, triangular): P5/P50/P95, mean with its MCSE, a histogram, a convergence trace and a tornado sorted by swing. Without `model` it is a Fermi estimate: the product of lognormal ranges, with a source per leaf. |
| `cash_flow` | Net `flows` per period at `rate`, in decimal arithmetic: NPV, every IRR (with a warning on several sign changes), MIRR from `finance_rate` and `reinvest_rate`, simple and discounted payback and the NPV-against-rate curve with the IRR crossings. |
| `decision_matrix` | `options` scored on weighted `criteria` (each a benefit or a cost, normalised min-max, by the maximum or not at all): weighted totals, rank, how far each weight can move before the first place changes, and a tie warning. |

The data is a CSV/TSV file of the workspace (`data.path`, read through the same guard as `read_file`) or inline columns where `null` is a missing value. Every card has the same parts:

- **Header**: the method and why it was chosen, and the **provenance chip** (engine, a short BLAKE3 hash of the input, the time). A click on the chip shows the inputs and the reproduce lines.
- **Assumptions**: a checklist of each check (pass, fail, not checked), open when one fails. A calc sheet shows its unit readings as chips on their lines instead.
- **Results**: numbers right-aligned in the monospace font. A regression adds residuals vs fitted and a normal QQ plot; a descriptive card a small histogram per variable.
- **Sentence and APA line**: a plain-language sentence and the APA line (italic symbols, `Copy`), both filled from the engine's values by templates, never written by the model.
- **Warnings** (small n, expected counts below 5, VIF above 10, unadjusted correlations...) and **alternatives** to the chosen method.
- **Footer**: `Engine output` toggles the raw envelope, **Recompute** runs the stored request again (re-reading the file), and buttons copy the reproduce line (Lua, Python, R, shell) and the envelope JSON.

After Recompute the header shows **✓ Verified · reproduced <time>** when every value matches, or **✗ Mismatch · N values changed** with a diff of the changed values. **Stale input** means Recompute found the data file changed since the card was computed (a warning names the old and new hashes). A failed recompute says why under the card.

**Number slots.** Each tool result lists the card's values as slots, and the model cites them in its reply as `⟦id.key⟧` (for example `⟦st_1a2b3c4d5e6f.p_value⟧`; the ASCII form `[[calc:id.key]]` also works). The timeline replaces each slot with the engine's value as the card displays it; a slot that names no known value stays as written. Any other number the model typed (outside slots, math, code, years, versions and the numbers in your prompt; whole numbers up to 10 are ignored) is counted in a chip under the reply, **⚠ N numbers not from an engine**; a click lists them.

### Math in text

TeX math in replies and markdown blocks is typeset with RaTeX: inline `$..$`, `\(..\)` and `\ce{..}` (mhchem) on the text baseline, and display `$$..$$`, `\[..\]` and ```` ```math ```` fences centered (scrolled sideways when wider than the column). A single `$` follows pandoc's rule, so prices such as `$5 and $20` stay prose; code spans and fences are left alone. A formula that does not parse shows as a monospace chip with the source, the error and **Copy LaTeX**. A calc sheet shows its lines' formulas the same way.

### Forms (`ask_user`)

When the model needs structured answers it calls `ask_user` with a JSON Schema object instead of asking in prose, and the turn waits for you. Each property becomes a field: one-line or multi-line text, a number or integer with bounds, a checkbox, radio buttons for one choice and checkboxes for several; `*` marks the required ones. The form takes the focus: `Tab` / `Shift+Tab` move between the inputs, `Enter` in a one-line field submits, `Esc` cancels and `Space` toggles a checkbox or a radio button. **Submit** checks every field and shows the problems inline; the answer goes to the model as JSON and the block keeps it read-only. A cancelled form says so and the model goes on without it. A form left from an interrupted turn or a resumed session shows "Not waiting for an answer". In headless mode the form is asked on the terminal ([Headless mode](#headless-mode)).

### Plots, reports, data, lab and operations blocks

- **`show_plot`**: explicit functions `f(x)`, parametric curves `x(t), y(t)` and implicit curves `f(x, y) = 0`, in calculator notation (`2x`, `a sin(x)`, `x^2`, `sqrt`, `ln`, `pi`...), plus scatter or line series of numeric points. Each parameter gets a slider; dragging it re-plots at once. `x_scale` / `y_scale` take `log` for an axis in decades (values at or below 0 are skipped with a note) and `aspect = "equal"` keeps one unit as long on both axes.
- **`show_test_report`**: a JUnit XML file (cargo nextest, pytest `--junitxml`, Surefire, Jest/Vitest, gotestsum; nested suites read as `outer / inner`) or a saved libtest / nextest JSON event stream (`cargo nextest run --message-format libtest-json`). Passed tests are folded into one row.
- **`show_findings`**: a SARIF 2.1.0 log (clippy-sarif, semgrep, CodeQL, ESLint), the most severe files first. A result's first suggested fix is shown as a unified diff when the tool can read the file it edits; otherwise its edits are listed.
- **`show_wavedrom`**: a WaveDrom timing diagram from WaveJSON, drawn as an SVG Image block tinted to the theme.
- **`show_dataset`**: CSV, TSV, JSON lines or Parquet. Text files are read up to 32 MiB (beyond that the profile covers the first rows and says `sampled`), Parquet files up to 256 MiB; Arrow IPC is not supported. The model is told to look at a file this way before running statistics on it.
- **`show_molecule`**: a structure from SMILES with values computed from it, never from the model. With `claimed_formula` a chip shows whether the model's formula matches; a SMILES that does not parse shows the error (and the model gets it too).
- **`show_plate`**: a 96- or 384-well plate map. `scale` is `sequential` (default), `diverging` around `center` (fold changes, z-scores; 0 when the values straddle it, else their median) or `log` (concentrations, counts; wells at or below 0 are not coloured). Labels are drawn inside the wells, smaller on a 384-well plate.
- **`show_trace`**: OTLP JSON (an `ExportTraceServiceRequest`, or the collector file exporter's JSON lines), a Jaeger JSON download or Zipkin v2 JSON, told apart by content; files up to 32 MiB and 20,000 spans. The model gets the span and service counts, the critical path, the spans with the most self time and the errors, not the spans themselves.
- **`show_flamegraph`**: folded stacks (`a;b;c 12` lines from `stackcollapse-perf.pl`, `cargo flamegraph`, py-spy `--format raw`, async-profiler `collapsed`) or a pprof profile (`.pb.gz`, `.pprof`; `sample_type` picks `cpu`, `alloc_space`, `inuse_space`... when the profile has several). The model gets the total, the hottest path and the top functions by self time.
- **`show_logs`**: JSON lines (pino, bunyan, zap, slog, tracing JSON), logfmt or plain text (log4j/logback, Python logging, env_logger, tracing's default format), sniffed from the first lines unless `format` says. The first 16 MiB and 50,000 lines are shown (the block says when the file was cut). The model gets the line and level counts, the facets and the error messages grouped by shape.

### Plugin modals

Plugins can open modal windows over the session ([plugins.md](plugins.md#appmodal-modal-windows)): a report, a gallery of screenshots, a table with a chart, a JSON document. Each has a header with its title, the plugin that opened it and a close button, and its content scrolls in both directions when it does not fit. A modal never covers the whole window: at least 20 px stay free on every side.

| Key or action | Effect |
| --- | --- |
| `Esc` | Closes the front modal (the one opened last). |
| Click on the dimmed area, or the `✕` button | Same. |
| `/modal close` | Same, from the command line. |
| Wheel, the scrollbars, `↑` / `↓`, `PageUp` / `PageDown`, `Home` / `End` | Scrolls the content. |
| Images: `fit`, `1:1`, `−`, `+` | Fit to the width, natural size, zoom. When zoomed, drag pans and `Ctrl`/`Cmd` + wheel zooms; with the image focused (click), `+`/`-`, `1` and `0` do the same. |
| JSON: click a row | Opens or closes that branch. |
| Links | Open in your browser. |

The focus goes back to where it was when the last modal closes. An approval dialog opened while a modal is up appears above it. Modals are not saved with the session.

### Simulation board

`/simulate <spec>` (or the agent's `simulate_solution` tool) opens the board of a simulated Spec in a modal at 100%. The board is drawn in real 3D by an offscreen GPU renderer; on a machine without a usable GPU adapter it falls back, automatically, to a 2.5D vector drawing (the header says which one: `3D · <adapter>` or `2.5D · <reason>`; `CREST_RENDER3D=off` forces the fallback).

The board is black with graphite chips: the top of each chip glows with the colour of its machine's state (teal for the first, usually idle, state; orange, blue, violet… for the others; red when it is down), brighter while it is handling a message, with its queue as small amber blocks. A trace lights up in the colour of the message travelling it. **Selecting a chip** dims everything it is not connected to: its traces glow in the TwoCrest coral (or in the colour of the message on them), the chips at their other end keep a thin ring, and the inspector lists those connections (`→ db (query)`, `← web (request)`) and the chip's last activity (`1.200 ms · sent query → db`, `idle → busy on request`…).

| Key or action | Effect |
| --- | --- |
| Drag, `Shift+←/→/↑/↓` | Orbits (leaves the isometric preset for perspective). In the 2.5D fallback, pans. |
| Right-drag, middle-drag, `Shift`-drag, `w` / `a` / `s` / `d` | Pans. |
| Wheel, `+` / `-` | Zooms. |
| `i`, or the `iso` button | Back to the isometric preset (orthographic, 30°). |
| Click a chip, `Tab` / `Shift+Tab` | Inspects it: state, variables, queue and its machine as a state diagram with the current state lit. |
| `Space`, `▶` / `⏸` | Plays or pauses the scenario. Playing is paced by events: at `1×` each stretch between two moments where something happens (a message sent or arriving, a component starting or finishing its work) lasts between 0.8 and 3 s, so every message can be followed; quiet periods do not stretch and bursts do not flash by. |
| `←` / `→`, `⏮` / `⏭` | Previous / next event. |
| `−` / `+` next to the speed | Slower / faster (0.1× to 8×). |
| Click or drag the bar | Scrubs; the red marks are failed assertions. |
| A failed check on the right | Jumps to its counterexample. |
| Scenario buttons, seed `−` / `+` | Shows another scenario; re-runs this one with another seed. |
| `Esc` | Closes the board. |

### Cost

The bar shows the accumulated cost of the session:

- `$0.00` before the first reply;
- the session's tokens (`48.2k tok`) if the model used has no configured price, with a notice the first time naming the model; crest ships no prices, so this is the usual value until you set `app.opt.pricing`;
- otherwise the amount, with 4 decimals below 1 USD and 2 from then on.

See [configuration.md](configuration.md#cost-and-spending-cap).

### Decision model

With Cloudflare credentials, a small decision model (Clef) chooses, before each chat prompt, which optional tools the turn gets and whether a simpler or a stronger model answers it; a notice in the log says what it changed. It also judges the problem definitions of the research flow and reads free-text answers at the flows' gates. Without credentials crest works as before. See [configuration.md](configuration.md#decision-model-cloudflare-clef).

### Opening a project

Without a directory (`crest`, or the desktop launcher), or with `--pick`, the window opens in the **project picker**:

- The path field on top lists the directory it names, starting at the current one. Typing filters it: `~/src/` lists `~/src`, `~/src/cr` keeps the entries that match `cr` (`~` is your home). Hidden directories show once the filter starts with a dot.
- **Recent** projects (the workspaces with saved sessions, newest first) come first while there is no filter; one that no longer exists is shown greyed out and does not open.
- Directories that look like projects (with a `.git` or a `.crest`) are marked `◆` and listed first.

| Key | What it does |
| --- | --- |
| `↑` / `↓` | Choose a row. |
| `Enter` | Open the chosen row as the project; with no row, the typed path. |
| `Ctrl+Enter`, the **Open …** button | Open the directory being listed. |
| `Tab`, `→` | Enter the chosen directory. |
| `←` (no filter) | Go up to the parent. |
| Click / double click | Choose / enter (a recent project opens). |

A directory that cannot be read says why under the field; a project that fails to open brings the picker back with the reason. While a project opens the window says `Opening <dir>…`.

`/project` closes the open project (once no turn or command runs) and goes back to the picker, at the project's parent; the session is already saved. Everything the project started (MCP servers, plugins) stops with it.

## Command line

```text
crest [OPTIONS] [DIR]
```

| Flag | Value | What it does |
| --- | --- | --- |
| `[DIR]` | directory | Workspace to open. Without it the window opens the [project picker](#opening-a-project); headless runs use the current directory. |
| `--pick` | — | Opens the window in the project picker. Cannot be combined with `[DIR]`, `--workspace` or `--headless`. |
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
crest .                                      # window on the current directory, resumes the last session
crest                                        # window in the project picker
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

An `ask_user` form is asked field by field on the terminal: each shows its label (`*` when required), description and options. `Enter` alone keeps the default or skips an optional field, a choice takes the option's number or value (several separated by commas), a boolean `y` or `n`, and a text area reads lines until one with a single `.`. A value the field does not accept is explained and asked again; `/cancel` cancels the form. When stdin is not a terminal (piped input, CI) nobody can answer, so every form is cancelled with that reason and the model goes on without it. The new blocks print one dim line each (`[computed: Welch two-sample t-test]`, `[dataset: data/cars.csv]`, `[molecule: CCO]`...).

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
| `show_chart`, `show_table`, `show_mermaid`, `show_markdown`, `show_plot` | the block's data | read |
| `stats_describe`, `stats_test`, `stats_regress`, `stats_correlate`, `stats_distribution`, `stats_contingency`, `stats_bootstrap`, `stats_forecast` | `data` (`path` of a CSV/TSV, `delimiter?`, or inline `columns`) and the request (variables, test, `y`, `x`, `group`, method, `seed`, `horizon`...) | read |
| `stats_power` | `test`, two or three of `n`, `effect` (or `p1`, `p2`), `alpha`, `power`; `groups?`, `alternative?`, `solve_for?` | read |
| `stats_ab_test` | `control` and `treatment` (`visitors`, `conversions`, `name?`), `expected_split` (0.5), `conf_level?`, `looks?`, `planned_visitors?`, `minimum_lift?` | read |
| `solve_lp` | `sense`, `objective`, `variables[]` (`name`, `lower?`, `upper?`, `integer?`, `binary?`), `constraints[]` (`expr`, `name?`) | read |
| `solve_equations` | `equations[]`, `unknowns[]` (`name`, `guess?`), `range?`, `tolerance?`, `max_iterations?` | read |
| `monte_carlo` | `inputs[]` (`name`, `distribution`, `source?`, `unit?`), `model?`, `output?`, `unit?`, `samples` (10000), `seed` | read |
| `cash_flow` | `flows[]`, `rate`, `finance_rate?`, `reinvest_rate?`, `currency?`, `period?` | read |
| `decision_matrix` | `criteria[]` (`name`, `weight`, `direction`), `options[]` (`name`, `scores[]`), `normalization?` | read |
| `calc` | `lines` (`name = expr`, in order), `significant_digits` (6) | read |
| `convert` | `value?`, `from`, `to`, `uncertainty?`, `significant_digits` (6) | read |
| `ask_user` | `title`, `description?`, `schema` (a JSON Schema object), `submit_label?`: waits for the answer | read |
| `show_test_report`, `show_findings` | `path` of a JUnit XML or libtest/nextest JSON file / a SARIF file (32 MiB at most); `title?` for the report | read |
| `show_wavedrom` | `source` (WaveJSON; JSON5 style accepted) | read |
| `show_dataset` | `path` (CSV, TSV, JSON lines, Parquet), `rows` (20, at most 200) | read |
| `show_molecule` | `smiles`, `name?`, `claimed_formula?` | read |
| `show_plate` | `format` (96 or 384), `wells` (`well`, `value?`, `label?`), `value_label?`, `title?`, `scale?` (`sequential`, `diverging`, `log`), `center?` | read |
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
  - `/mcp trust tickets`, from the window, approves it and starts it right away.

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
