<div align="center">

<img src="assets/logo.svg" alt="crest logo" width="96">

# crest

**The coding agent that shows its work, checks its numbers and asks before it touches a file.**

A native, GPU-rendered workspace for Claude and the OpenCode models. Your chats live in projects, every reply is a block you can open,<br>
every number comes from an engine, and three commands take a problem from research to a Spec to commits.

![License](https://img.shields.io/badge/license-GPL--3.0-blue)
![Rust](https://img.shields.io/badge/rust-1.99%2B-orange?logo=rust)
![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey)
![Models](https://img.shields.io/badge/models-Claude%20%7C%20OpenCode-8a63d2)
![Release](https://img.shields.io/badge/release-v0.19.0-d9604a)
![Status](https://img.shields.io/badge/status-work%20in%20progress-yellow)

<img src="assets/screenshots/hero.png" alt="crest finding a bug, fixing it, running the tests and charting their timings" width="900">

</div>

> [!WARNING]
> **crest is a work in progress.** It is usable day to day, but it is young: expect rough edges, missing features and breaking changes between releases. Some parts have only been tested with mocks (see [what is verified](docs/status.md)). Feedback and bug reports are very welcome.

<table>
<tr>
<td width="33%" valign="top"><b>You see everything.</b><br>Every reply, tool call, diff, test report, trace and chart is one of 47 kinds of block you can expand, sort, zoom and copy, drawn natively on the GPU. Your projects and chats sit in a sidebar; the questions you asked, on a rail at the right.</td>
<td width="33%" valign="top"><b>No number is a guess.</b><br>The model decides what to compute; a deterministic engine, checked against NIST StRD and R, computes it. Each card shows its provenance, a Recompute button and a line to reproduce it in Python or R, and a number the model typed by itself is flagged under the reply.</td>
<td width="33%" valign="top"><b>Nothing happens behind your back.</b><br>Edits and commands wait for you with the exact diff or command line: allow once, for the session or for the project. Keys and personal data are masked before the model sees them. Plugins and MCP servers run with the capabilities you approve, in a sandbox.</td>
</tr>
</table>

## Install

**macOS** (11 or newer, Apple Silicon or Intel), with [Homebrew](https://brew.sh):

```bash
brew install --cask lazaromb/tap/crest
```

It installs `crest.app` in `/Applications`, the `crest` command, and `rg` and `fzf`. `brew upgrade` updates it. The app is signed and notarized by Apple.

**Windows** (10 1809 or newer, x86_64; no administrator rights):

```powershell
winget install LazaroMB.Crest
```

Or, without winget, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/lazaroMB/crest/main/scripts/install.ps1 | iex
```

Both install crest as an app: `crest.exe` in `%LOCALAPPDATA%\Programs\crest`, an entry in the Start menu and in Settings > Apps (uninstall it there, or with `install.ps1 -Uninstall`), and the `crest` command on your user `PATH`. `winget upgrade` or the same command again updates it. Install `rg`, `fzf` and `git` too (`winget install BurntSushi.ripgrep.MSVC junegunn.fzf Git.Git`).

You can also download `crest-<version>-x86_64-pc-windows-msvc-setup.exe` from the [latest release](https://github.com/lazaroMB/crest/releases/latest) and run it. crest is not code-signed yet, so Windows SmartScreen warns about a setup downloaded with a browser: choose **More info → Run anyway**. (winget and the PowerShell command do not trigger it.)

**Linux** (x86_64 or aarch64, glibc 2.35+; no sudo), and macOS without Homebrew:

```bash
curl -fsSL https://raw.githubusercontent.com/lazaroMB/crest/main/scripts/install.sh | sh
```

It downloads the [latest release](https://github.com/lazaroMB/crest/releases/latest), checks its checksum and installs crest as an app: in the app menu on Linux (GNOME, KDE and other XDG desktops), in `~/Applications` on macOS (Launchpad, Spotlight, Dock), plus the `crest` command in `~/.local/bin`. `sh -s -- --version v0.19.0` picks a release, `--uninstall` removes it. The release tarballs carry the same script: extract one and run `./install.sh`.

Opened from the app menu or the Dock, crest gets the environment of your login shell (`~/.zprofile`, `~/.profile`), not the one of `~/.zshrc`: keep the API key in the keyring (below).

**From source** (Rust 1.99+):

```bash
git clone https://github.com/lazaroMB/crest && cd crest
make deps       # installs what is missing: rg, fzf, the GPUI libraries...
make install    # the same app install, from your build (make uninstall removes it)
```

Then open crest and give it an Anthropic or an OpenCode key: without one the window asks for it (or run `/login`; see [Claude and OpenCode](#claude-and-opencode)). The key is stored in the system keyring (GNOME Keyring, KWallet, the macOS Keychain or the Windows Credential Manager). From a terminal you can also store it yourself or use the environment:

```bash
secret-tool store --label="crest" service crest username ANTHROPIC_API_KEY   # Linux (OPENCODE_API_KEY likewise)
security add-generic-password -s crest -a ANTHROPIC_API_KEY -w               # macOS
# or: export ANTHROPIC_API_KEY=sk-ant-...   (opened from the app menu, crest only sees it in ~/.profile or ~/.zprofile)
```

Providers, prefixed model ids and how to point the roles at OpenCode models are in [docs/configuration.md](docs/configuration.md#model-providers-and-api-keys).

Requirements, every `make` target and the repository layout are in [docs/development.md](docs/development.md).

## Quick start

```bash
crest ~/src/my-project     # open a workspace
crest                      # or pick a recent project (crest --pick always asks)
crest --mode plan --model opus .                 # start read-only, on Opus
crest --headless "find where the config is parsed and explain it" --workspace .
crest --headless --research "add() returns the difference for negative numbers"
```

- The first start opens a five-step **welcome tour** of the window (the sidebar, the timeline, the prompt, the commands); `/tour` replays it. [The tour](docs/usage.md#welcome-tour-tour).
- Type a prompt and press `Enter` (`Shift+Enter` for a new line). `↑` brings back what you sent before.
- `/` opens the command palette: `/open @file`, `/model`, `/mode`, `/theme`, `/mcp`, `/research`, `/simulate`, `/implement`, `/project`, `/!git status`...
- `Ctrl+G` is the leader: `Ctrl+G ff` finds a file, `Ctrl+G fg` greps, `Ctrl+G e` shows the tree, `Ctrl+G m` picks a model, `Ctrl+G p` cycles the permission mode. In the timeline, `j`/`k` move between blocks, `gg`/`G` jump to the ends, `o` expands, `t` shows the data behind a chart, `y` copies. `Ctrl+Tab` moves between panels.
- `Esc` always brings you back to the prompt; `Ctrl+C` interrupts a turn, as in a terminal; `F1` shows everything else.

Opening crest again in the same directory resumes the last chat of that project; the sidebar opens any other, and `crest --new` starts a fresh one. Every flag is in [the command line](docs/usage.md#command-line).

## New in v0.19.0

What changed since v0.18.0, each with its section below:

- An **MCP servers manager** (`/mcp`, `Ctrl+G S`, the sidebar's dock): add, edit, test, log in, turn on or off and remove servers live, saved in an `mcp.json` next to `init.lua` → [Connect any MCP server](#connect-any-mcp-server).
- A **welcome tour** on the first start, replayed with `/tour` → [Quick start](#quick-start).
- A **chats and projects sidebar** in three widths, projects with several directories, **Wild Chats** without a directory, chat menus (pin, rename, move, archive, delete) and **branching** a chat from any point → [Your chats and your projects](#your-chats-and-your-projects).
- The prompt's **`+` menu** (Chat, Research, Deep research, inserts), the **information mark** that keeps errors and warnings, a **grouped command palette** with a which-key popup, **pickers with a preview** of images and 3D models, and overlays that all close with `Esc`, a click outside or `✕` → [A timeline, not a transcript](#a-timeline-not-a-transcript).
- Permission rules **by pattern**, the `e` answer and the `Tab` ladder in the approval dialog, sensitive paths a workspace can only tighten → [You approve every change](#you-approve-every-change).
- The **discovery research engine**, `/research --quick`, per-stage time and cost, retries that never trap the chat, spikes in a git worktree → [From a problem to a Spec to commits](#from-a-problem-to-a-spec-to-commits).
- **Faster**: up to eight read-only tool calls at once, a concurrent startup and render caches → [Smart routing, smaller bills](#smart-routing-smaller-bills).
- **`keymap.toml`** without Lua, a leader that moves the builtin mappings too, `app.opt.shell`, `CREST_LOG`, and a **design system** behind every view → [Make it yours](#make-it-yours).

**Contents**: [Why crest](#why-crest) · [Your chats and your projects](#your-chats-and-your-projects) · [A timeline, not a transcript](#a-timeline-not-a-transcript) · [Blocks for technical work](#blocks-for-technical-work) · [Numbers you can verify](#numbers-you-can-verify) · [From a problem to a Spec to commits](#from-a-problem-to-a-spec-to-commits) · [You approve every change](#you-approve-every-change) · [Open anything](#open-anything) · [Smart routing, smaller bills](#smart-routing-smaller-bills) · [Connect any MCP server](#connect-any-mcp-server) · [Make it yours](#make-it-yours) · [Claude and OpenCode](#claude-and-opencode) · [Everything in one place](#everything-in-one-place) · [Documentation](#documentation) · [Built with](#built-with) · [Vision and roadmap](#vision-and-roadmap) · [Status and license](#status-and-license)

## Why crest

Most coding agents live in a terminal transcript: the diff scrolls by, the test output is text, the numbers are whatever the model typed, and the plan is in its head. crest runs the same loop (read, search, edit, run, with your approval) in a native window where each of those things is an object: a diff you can read while the agent keeps working, a test report with the failures first, a statistics card computed by an engine and verified on demand, a Spec folder you can simulate before you implement it. It is free software (GPL-3.0), programmable end to end in Lua, and talks to Anthropic and OpenCode with one key each.

## Your chats and your projects

The sidebar on the left keeps your work in sight. It has three widths, cycled with `Ctrl+G s` or the mark at its top: hidden (the mark alone; hovering it reveals the rows over the conversation), a strip of project initials, and the full list with **New chat**, **Search**, every project and its chats, newest first. A project can span several directories (every one is a workspace root for the file and search tools), and chats with no directory at all live under **Wild Chats**: they answer without file tools, for the questions that have nothing to do with code. [Sessions and the sidebar](docs/usage.md#sessions-and-resuming) · [projects with several directories](docs/usage.md#agent-tools).

Each row has a menu: **pin** it to the top, **rename** it, **move** it to another project, **archive** it into a folded **Archived (N)** section (and restore it from there) or **delete** it. `Ctrl+G B` **branches** the open chat: a copy up to the selected block, marked "Branched from", to try another direction without losing the first. A chat with a running turn shows its activity, and switching away parks the turn until you come back. `/project` (or `crest --pick`) changes project without closing the window. [Chat and project menus](docs/usage.md#chat-and-project-menus) · [opening a project](docs/usage.md#opening-a-project).

<table>
<tr>
<td width="50%"><img src="assets/screenshots/sidebar.png" alt="The full sidebar: a pinned project, a branched chat, Wild Chats and the Archived section"></td>
<td width="50%"><img src="assets/screenshots/tour.png" alt="The welcome tour pointing at the sidebar"></td>
</tr>
</table>

## Your AGENTS.md, as it is

crest reads the instructions your project already keeps for agents: `AGENTS.md`, `CLAUDE.md` and `CLAUDE.local.md` at the root, your own `~/.claude/CLAUDE.md` and `~/.config/crest/AGENTS.md`, `@path` imports, and the files of subdirectories, which are attached the first time the agent works in them. An edited file applies on the next request, compaction never drops them, and `/init` has the agent write a first `AGENTS.md` for you. A repository set up for Claude Code, Cursor or Codex works the same here. [Project instructions](docs/usage.md#project-instructions-agentsmd-and-claudemd).

## A timeline, not a transcript

The agent answers with real components, drawn natively on the GPU: bar, line, area and pie charts (press `t` for the data behind them), sortable tables, Mermaid diagrams with pan and zoom, colored diffs, rendered markdown, JSON trees, images, the blocks your plugins define and 47 kinds of block in all ([every block](docs/usage.md#blocks)). Replies and markdown typeset TeX math (inline `$..$`, display `$$..$$`, `\ce{}` for chemistry) on the text baseline ([math in text](docs/usage.md#math-in-text)). A rail of marks on the right lists the questions you asked and jumps to any of them ([focus](docs/usage.md#focus)).

The prompt has no modes: it is always ready to type, and everything else is a key or a menu away. The round **`+` button** opens a menu with the prompt's mode (**Chat**, **Research** or **Deep research**: the line is sent as `/research <text>` or `/research --deep <text>`) and two inserts, `@` for a workspace file and `/` for a command; a chip next to the button shows the mode until you change it back ([prompt mode](docs/usage.md#prompt-mode-the--button)). `@` completes paths as you type ([file references](docs/usage.md#file-references-with-)), and a pasted screenshot lands in a tray above the prompt, up to 20 at a time, sent to the model or kept in the chat only ([pasted images](docs/usage.md#pasted-images)). A prompt sent while crest works is queued and goes next.

<img src="assets/screenshots/menu.png" alt="The menu of the prompt's + button: Chat, Research, Deep research, and the @ and / inserts" width="900">

`/` alone lists the command **groups** (Chat, Flows, Stages, Model & permissions, Workspace, App, and one per plugin); a name after it filters every command; a pause after the leader shows a **which-key** popup with the keys that can follow ([typing, commands and shortcuts](docs/usage.md#typing-commands-and-shortcuts)). `Ctrl+G ff` and `Ctrl+G fg` open the file and grep **pickers**, two columns wide on a window of 860 px or more, with the file, the picture or the 3D model previewed on the right, `Alt+C`/`R`/`W` for case, regex and whole words, and the recent files first ([pickers](docs/usage.md#pickers-ctrlg-ff-ctrlg-fg)). Every picker, popup, menu and modal closes the same way: `Esc`, a click outside, or its `✕`.

<img src="assets/screenshots/picker.png" alt="The grep picker with its preview column" width="900">

An error or warning does not vanish with the status bar: the **information mark** at the bottom-right corner keeps it, shows it as a toast for three seconds, and opens a panel with every kept notice (each with its own **Dismiss**) and the steps too quick for the activity line ([the window](docs/usage.md#the-window)).

<img src="assets/screenshots/info.png" alt="The information mark with its panel of kept notices" width="640">

Behind the blocks are over 70 builtin tools: files (`list_files`, `find_file`, `tree`, `grep`, `read_file`, `edit_file`, `write_file`, `run_command`), display (`show_chart`, `show_plot`, `show_table`, `show_mermaid`, `show_markdown`, 31 `show_*` readers for file formats and the GHS hazard card), `ask_user` forms, 22 compute tools, `web_search` and `web_fetch` when `app.opt.web.enabled` is on (DuckDuckGo, Brave or SearXNG, run with `curl` on your machine), a read-only `task` subagent for exploring, `simulate_solution`, plus whatever your MCP servers and plugins add. [Agent tools](docs/usage.md#agent-tools) · [web search](docs/configuration.md#web-search).

<img src="assets/screenshots/blocks.png" alt="A chart and a table in the timeline" width="900">

## Blocks for technical work

Point the agent at a test report, a trace, a dataset, a CAD part or a molecule and it shows the file in the block that fits it. The block is computed from the file, not from the model's memory, and the model gets a summary (counts, the hottest path, the failing cases, the errors), never the raw rows.

| If you work on | Blocks | From | In the block |
| --- | --- | --- | --- |
| Code and tests | Diff, Shell, TestReport, Findings, Benchmarks | JUnit XML, libtest and nextest JSON, SARIF, criterion, ANSI output | failures first, a click opens the file, a suggested fix shows as a diff |
| Data and statistics | Dataset, Plot, Computed, TrainingCurves, ArrayFile | CSV, TSV, JSON lines, Parquet, HDF5, NetCDF, inline columns | a profile per column, a slider per parameter, Recompute, an APA line |
| Services and performance | Trace, FlameGraph, Logs, QueryPlan, Har, TerraformPlan, Diagram | OTLP, Jaeger, Zipkin, folded stacks, pprof, JSON lines, logfmt, PostgreSQL `EXPLAIN`, HAR, DOT, SQL DDL | critical path, zoom into a frame, filter by level or `/regex/`, an ER diagram from the schema |
| Hardware and CAD | Waveform, Netlist, CadModel, Drawing, IfcSchedule, Material | VCD, FST, GHW, SPICE, KiCad, STEP, DXF, IFC | the 3D viewer, signals over time, a room schedule, a parts list |
| Chemistry and biology | Molecule, Molecule3d, MoleculeGrid, Reaction, Spectrum, Sequence, Plate, Ghs | SMILES, MOL, SDF, XYZ, PDB, JCAMP-DX, mzML, MGF, GenBank, FASTA, PubChem | formula and mass computed from the structure, a balanced reaction, a plasmid map, a 96- or 384-well heatmap |
| Planning and money | Gantt, GeoMap, Network, Form, and the Computed cards for cash flow, LP, Monte Carlo, decision matrices and BOM costs | CPM tasks, GeoJSON, inline inputs | the critical path of a plan, every IRR, duals per constraint, P5/P50/P95, a typed form answered in the chat |

The selected block offers a **copy as** row with the other forms it can take: TSV and CSV, the JSON of a plot, the failing tests of a report, the `file:line` list of findings, the SMILES and SVG of a molecule, the folded stacks of a flame graph, the APA line and the envelope JSON of a card. When the model needs structured answers it calls `ask_user` and a typed form appears in the chat (text, numbers with bounds, choices, dates); the turn waits, and you can park it and come back.

<img src="assets/screenshots/technical.png" alt="A trace waterfall with its critical path and a form waiting in the timeline" width="900">

**Every block**: [the table in Usage](docs/usage.md#blocks) · [plots, reports, data, lab and operations](docs/usage.md#plots-reports-data-lab-and-operations-blocks) · [forms](docs/usage.md#forms-ask_user) · [why these blocks](docs/research/chat-blocks-for-technical-work.md).

## Numbers you can verify

The model decides what to compute. A deterministic engine computes it. The card shows only the engine's numbers, each with its provenance.

- **One block kind, 22 cards.** Descriptive statistics, hypothesis tests, regression, correlation, distributions, contingency tables, power, A/B tests, bootstrap, forecasts; calc sheets with units and unit conversions (numbat); linear and mixed-integer programs, equations, Monte Carlo and Fermi estimates, cash flows, decision matrices; and for engineers, Bode plots, Karnaugh maps, beams, tolerance stacks and BOM costs.
- **Provenance and Recompute.** The header shows the method and why it was chosen, and a chip with the engine, a BLAKE3 hash of the input and the time. **Recompute** runs the stored request again and marks the card `✓ Verified` or `✗ Mismatch · N values changed`, and `Stale input` when the data file changed.
- **No number is the model's guess.** The reply cites values through slots (`⟦id.p_value⟧`) that the timeline fills from the card. Any other number the model typed is counted in a chip under the reply, `⚠ N numbers not from an engine`.
- **Reproduce it elsewhere.** Every card carries lines for Lua, Python, R and the shell. The statistics engine is checked against NIST StRD and R reference values.

```text
v = 100 km/h
t = 90 min
d = v * t -> km
```

[Statistics and calculations](docs/usage.md#statistics-and-calculations) · [`app.compute` from a plugin](docs/plugins.md#appcompute-engine-computed-cards).

## From a problem to a Spec to commits

Three commands walk a problem through research, simulation and implementation inside the chat, stopping at gates you answer with a plain line.

| Command | What it does | What it leaves |
| --- | --- | --- |
| `/research <problem>` | The stages of the [Hashlabs X-Workflow](https://www.hashlabs.com/guides/en/methodology/X-Workflow.html): five by default, with two gates and the independent work in parallel; the full fourteen, with five gates, with `--deep`; `--quick` skips the gates. | `.crest/specs/NNNN-slug/` with `spec.md`, `plan.json`, sources and a journal. |
| `/simulate <spec>` | Turns the Spec into an executable model, freezes the assertions of its test protocol (`always`, `never`, `eventually … within`, `count`, `path`) and runs every scenario on an isometric 3D board. A failure comes with a counterexample. | `simulation/` with the model, the traces and a self-contained `board.html` that works offline. |
| `/implement <spec>` | Runs `plan.json` task by task with the agent, one commit per task, checks included, up to three fix attempts per task. | One commit per task and a `report.md`. |

Every step is a checkpoint in SQLite: close crest and `/research resume` picks up where it stopped; `/research list` shows every flow of the workspace with its stage and what each stage took (`46s · 38 calls · $0.41`). A stage that fails is retried twice by itself, then saved as failed with its reason, so a flow never traps the chat. `/stage`, `/back`, `/skip`, `/retry` and `/abort` steer the workflow; `/aside` opens a side chat to look something up before answering a gate, and `/flow` goes back with the answer. With the [decision model](#smart-routing-smaller-bills), crest tells a side question from an answer by itself. In deep mode the prototype stage builds a spike in a `git worktree`, so your tree stays clean. With `app.opt.research = { engine = "discovery" }` the research runs on the **discovery engine**: a document with a section contract, an append-only event log as its state, audits with provenance before each verdict, `/reopen <section>` to write a section again and `/research accept` to freeze the discovery and derive its Spec. The same workflows run in a terminal with `--headless --research`, `--implement` or `--simulate`, with `--continue` to resume and `--yes` to approve tool calls. A simulation is a behavioural sketch, not a proof about the real system: it shows where a design breaks under its own assertions, with the state that breaks it.

<table>
<tr>
<td width="50%"><img src="assets/screenshots/research.png" alt="The research workflow at a gate"></td>
<td width="50%"><img src="assets/screenshots/board.png" alt="A solution simulated on the 3D board"></td>
</tr>
</table>

**Read on**: [the fast flow](docs/workflows.md#fast-flow-default) · [deep mode](docs/workflows.md#deep-mode---deep) · [side questions](docs/workflows.md#side-questions) · [the spike](docs/workflows.md#prototype-spike) · [the discovery engine](docs/workflows.md#research-on-the-discovery-engine) · [simulating a solution](docs/workflows.md#simulating-a-solution) · [the board](docs/workflows.md#the-board) · [implementation workflow](docs/workflows.md#implementation-workflow) · [the simulator](docs/research/solution-simulator.md).

## You approve every change

Nothing touches your files or runs a command without asking (unless you choose `auto-edit`). The dialog shows the exact diff or command line; `y` approves once, `s` allows a pattern such as `cargo test *` for the session, `p` saves it for the project in `.crest/permissions.local.toml` (where you can also write deny rules, which win over everything else), `e` approves every edit for the rest of the session, `n` rejects, and `Tab` widens or narrows the pattern before you allow it (`cargo test -p ui` → `cargo test *` → `cargo *`; shells, interpreters and `npx` only ever get the exact command). `/permissions` (or `Ctrl+G A`) lists the rules in force, removes them or moves a session rule to the project. A `plan` mode lets the agent read but never write. [Approvals and patterns](docs/usage.md#approvals-and-permission-modes) · [the patterns](docs/usage.md#patterns) · [`/permissions`](docs/usage.md#permission-rules-permissions).

`.env`, keys, `.git/` and the other sensitive paths are refused to every tool, and a workspace config can only add patterns to that list, never remove one ([permissions and sensitive paths](docs/configuration.md#permissions-and-sensitive-paths)). Secrets and personal data never leave your machine by accident: the key formats of seven vendors, JWTs, private keys, passwords, emails, cards, IBANs and national ids (validated by Luhn, mod-97 and the DNI letter, so a random number is not masked) in tool results and in your prompts become `[REDACTED:kind]` before the model, the timeline or the saved session see them ([what is masked](docs/configuration.md#privacy-secrets-and-personal-data)). `app.opt.max_session_cost` caps what a session may spend, and `app.opt.pricing` gives the prices per model (without them the status bar counts tokens instead of dollars). [Cost and spending cap](docs/configuration.md#cost-and-spending-cap).

<table>
<tr>
<td width="50%"><img src="assets/screenshots/approval.png" alt="An edit waiting for approval with its diff"></td>
<td width="50%"><img src="assets/screenshots/permissions.png" alt="The /permissions modal with session, project and deny rules"></td>
</tr>
</table>

## Open anything

`/open @path` shows any file of the workspace in a modal: highlighted code, rendered markdown, JSON trees, Mermaid, diffs, pictures with zoom, and **3D models** (OBJ, STL, PLY and STEP) in an orbit viewer. Plugins open the same modals with their own content.

<table>
<tr>
<td width="50%"><img src="assets/screenshots/model3d.png" alt="An STL gear in the 3D viewer"></td>
<td width="50%"><img src="assets/screenshots/modal.png" alt="A plugin modal with a chart, a JSON tree and a diagram"></td>
</tr>
</table>

## Smart routing, smaller bills

With Cloudflare credentials, a small **decision model** ([Clef](https://developers.cloudflare.com/workers-ai/models/clef-flash/)) answers typed questions in milliseconds for a fraction of a cent:

- **which tools** each prompt needs, so the model gets a shorter, cheaper request;
- **which model** answers it: a quick one for a greeting, a strong one for a redesign;
- **what a line at a workflow gate means**: an answer, a side question, or "back to the flow".

Without credentials crest works exactly as before. [How to turn it on](docs/configuration.md#decision-model-cloudflare-clef).

With [rtk](https://github.com/rtk-ai/rtk) installed, `app.opt.rtk = true` compacts the output of `git`, `cargo`, test runners and other common commands before the model reads it, often by 60–90 %. [How it works](docs/configuration.md#compact-command-output-with-rtk).

**Fast where it counts.** Read-only tool calls of one turn run in parallel, up to eight at once; the window, the config, the models and the MCP servers start concurrently; markdown parses, charts and layouts are cached across frames and `/open` documents are virtualized. The pass has not been measured on a benchmark yet: [the audit says what changed and how to measure it](docs/research/performance-architecture-audit.md#behaviour-users-may-notice).

<img src="assets/screenshots/routing.png" alt="A sequence diagram of how a prompt is routed" width="900">

## Connect any MCP server

Give the agent your issue tracker, your docs or your database: any server that speaks the [Model Context Protocol](https://modelcontextprotocol.io/), local or hosted, is a few lines of `~/.config/crest/init.lua`:

```lua
-- A local server, started with npx
app.mcp.add("fs", { command = "npx", args = { "-y", "@modelcontextprotocol/server-filesystem", "." } })
-- A hosted server with a token in a header, read from the environment
app.mcp.add("context7", { url = "https://mcp.context7.com/mcp", headers = { Authorization = "Bearer ${CONTEXT7_API_KEY}" } })
-- A hosted server with an OAuth login: run /mcp login linear once
app.mcp.add("linear", { url = "https://mcp.linear.app/mcp" })
```

- Or skip the Lua: `/mcp` (`Ctrl+G S`) opens the **MCP servers manager**, a window that adds, edits, tests, logs in, turns on or off and removes servers while crest runs, saving them in `~/.config/crest/mcp.json` or `<workspace>/.crest/mcp.json`. [The manager](docs/usage.md#mcp-servers-mcp).
- `/mcp` shows every server, its state and the next step; `/mcp tools <name>` lists what the model sees, `/mcp login <name>` signs in through the browser and keeps the tokens in the system keyring, and `restart`, `enable`, `disable` and `logout` do what they say.
- Their tools go through the same approvals as the builtin ones: `effect = "read"` lets a server run without asking.
- Servers declared in a workspace's `.crest/init.lua` do not start until you approve them with `/mcp trust <name>` (or `crest --trust-mcp <name>`), which starts them right away, and ask again if their command or URL changes.
- Tool names and schemas are adapted to each provider, so the same server works with Claude and with the OpenCode models.

**Set up a server**: [the guide](docs/configuration.md#mcp-servers) · [every field](docs/plugins.md#appmcp-mcp-servers) · [`/mcp` commands](docs/configuration.md#mcp) · [OAuth and GitHub](docs/configuration.md#oauth) · [workspace trust](docs/usage.md#mcp-servers-and-trust) · [`npx` not found from the app menu](docs/configuration.md#npx-from-the-app-menu)

## Make it yours

Configuration and plugins are Lua, with Neovim-style key notation; keys alone also go in a `keymap.toml` (`[[bind]]`, `[[unbind]]`), with no Lua at all, and `app.opt.leader` moves every `<leader>` mapping, the builtin ones included ([`keymap.toml` and the leader](docs/configuration.md#keymaptoml-and-the-leader)). `app.opt.shell` picks the shell of `/!` ([shell](docs/configuration.md#shell)) and `CREST_LOG` the log filter ([environment variables](docs/configuration.md#environment-variables)). A plugin can add:

| Extension point | What it does | Docs |
| --- | --- | --- |
| Tools | New tools the model can call, with a JSON schema and an effect for approvals | [`app.tools.register`](docs/plugins.md#apptoolsregisterdef) |
| Hooks | Inspect, rewrite or deny every tool call (builtin, plugin and MCP) before it runs | [`app.on`](docs/plugins.md#apponevent-fn-hooks) |
| Commands | Your own `/` commands, which can start a turn with any model role | [`app.commands.register`](docs/plugins.md#appcommandsregistername-fn-opts) |
| Keymaps | Bindings in Neovim notation, with your own leader | [`app.keymap.set`](docs/plugins.md#appkeymapsetkeys-action-opts) |
| Blocks | 23 constructors: charts, tables, plots, diagrams, JSON trees, diffs, markdown, timelines, graphs, trees, filterable lists, plates and materials | [`app.ui`](docs/plugins.md#appui-custom-blocks) |
| Computed cards | 22 engine functions: statistics, calc sheets with units, LP, equations, Monte Carlo, cash flow, decisions and the engineering cards | [`app.compute`](docs/plugins.md#appcompute-engine-computed-cards) |
| Format blocks | 27 readers: datasets, molecules, traces, flame graphs, logs, waveforms, STEP, DXF, GenBank, spectra, maps and more | [`app.formats`](docs/plugins.md#appformats-file-format-blocks) |
| Forms | Ask the user a typed form and wait for the answer | [`app.ask`](docs/plugins.md#appask-forms) |
| Modals | Windows over crest with images, CSV tables, charts and documents | [`app.modal`](docs/plugins.md#appmodal-modal-windows) |
| MCP servers | Bundle the servers your plugin needs | [`app.mcp`](docs/plugins.md#appmcp-mcp-servers) |
| APIs, programs, secrets | HTTP requests, command-line programs and keyring secrets, once you approve them | [Capabilities](docs/plugins.md#capabilities) |

Every view of the window is built from one **design system**: color tokens and 13 shape tokens (spacing, radii, text sizes) that a theme can set, ten kit components, and a lint that keeps hand-made colors out of the code ([design-system.md](docs/design-system.md#tokens) · [components](docs/design-system.md#components-uikit) · [the rules](docs/design-system.md#rules)).

### Write a plugin

A tool that answers with a chart instead of text, and a guard that no `rm` gets past:

```lua
-- ~/.config/crest/plugins/team/init.lua
local app = require("app")

app.tools.register({
  name = "velocity",
  description = "Shows the points closed in each sprint as a chart.",
  schema = { type = "object", properties = { sprints = { type = "array" }, points = { type = "array" } } },
  effect = "read",   -- only displays data: no approval needed
  handler = function(input)
    return app.ui.block("velocity", app.ui.chart({
      title = "Velocity", kind = "line", labels = input.sprints,
      series = { { name = "points", values = input.points } },
    }))
  end,
})

app.on("before_tool", function(ev)
  if ev.name == "run_command" and ev.input.command == "rm" then
    return { deny = true, reason = "rm is blocked by the team plugin" }
  end
end)
```

Plugins run in a sandbox: no files, no network, no programs, 100 million instructions and 256 MiB per call. A plugin that needs an API or a program declares it, and it stays off until you approve it with `/plugin trust <name>` (or `crest --trust-plugin <name>`):

```lua
app.plugin{ name = "jira", capabilities = { "network:jira.example.com", "shell:jq" } }
```

**Start from a recipe**: [a tool with a table and a pie chart](docs/plugins.md#1-a-todo_list-tool-with-a-custom-block) · [a hook that blocks `rm` and dry-runs `git push`](docs/plugins.md#2-a-hook-that-blocks-rm-and-forces-git-push---dry-run) · [a command on the `plan` model](docs/plugins.md#3-a-command-that-starts-a-turn-with-the-plan-role) · [a Jira tool with `app.http` and a secret](docs/plugins.md#example-an-api-tool-with-apphttp-and-appsecret) · [a `jq` wrapper](docs/plugins.md#example-a-cli-wrapper-with-appshellrun-and-jq) · [testing a plugin](docs/plugins.md#testing-a-plugin). Everything else is in [Writing plugins](docs/plugins.md).

**Try the demo plugin**: [`examples/plugins/showcase`](examples/plugins/showcase/init.lua) shows every extension point in one file. A plugin is a folder with an `init.lua`, in `~/.config/crest/plugins/` or listed in `~/.config/crest/init.lua`; then type `/demo` (or `/demo chart`, `table`, `block`, `stats`, `ask`), or press `Ctrl+G d`:

```lua
app.plugins = { "~/path/to/crest/examples/plugins/showcase" }
```

### Themes

crest ships 14 themes ported from the most used Neovim colorschemes (catppuccin, tokyonight, kanagawa, rose-pine, gruvbox, nightfox, onedark, dracula, nord, everforest), and two of its own, `twocrest` and `twocrest-light`, compiled into the binary so the window has a look before any file is copied. Type `/theme` and move through the list to see each one in the window, `Enter` to keep it. A theme is a Lua file that colors every element of the window in each of its states (hover, pressed, focused, selected, disabled) and every kind of code token, and anything it leaves out is derived from the rest:

```lua
app.opt.theme  = "catppuccin-mocha"
app.opt.window = "maximized"   -- or "fullscreen", "windowed", { width = 1440, height = 900 }
app.opt.colors = { ui = { caret = "#fab387" } }   -- tweak any theme
```

**[Create your own theme](docs/themes.md)**, port one from Vim, or start from the [example themes](examples/themes/).

## Claude and OpenCode

crest speaks to two model accounts, each with its own key, and can use both at once:

| Account | What you get | Model ids | Key |
| --- | --- | --- | --- |
| **Anthropic** | The Claude family, with built-in roles tuned for each job | `claude-sonnet-5-5`, `opus`, `haiku`... (no prefix) | `ANTHROPIC_API_KEY`, pay per token |
| **OpenCode** | **Go** (flat-rate subscription) and **Zen** (pay as you go): GLM, Kimi, DeepSeek, Qwen, MiniMax, GPT, Claude and more | `opencode-go:glm-5.3`, `opencode-zen:claude-sonnet-5-5` | `OPENCODE_API_KEY`, one key for both |

`/login` opens the key dialog (`Tab` switches the account, `/login opencode` opens it on OpenCode). crest checks the key with the provider and stores it in the system keyring, never in a file; `/logout` deletes it. The account you log in to last becomes the **default provider**, the one that serves model ids without a prefix. `/model` (or `Ctrl+G m`) opens a popup with every model your accounts can use. These are the only providers today (Anthropic, OpenCode Go and OpenCode Zen; `app.opt.providers.opencode` takes a `base_url` and the wire `formats`); local models are on the [roadmap](#vision-and-roadmap).

### Claude

With an Anthropic key, crest works out of the box: every role has a Claude model and a fallback, so a quick model explores and summarizes while a strong one plans.

| Role | Model | For |
| --- | --- | --- |
| `main` | `claude-sonnet-5-5` (falls back to Haiku) | The conversation and the agent loop |
| `component` | `claude-sonnet-5-5` (falls back to Haiku) | Structured data for blocks |
| `plan` | `claude-opus-5-5` (falls back to Sonnet) | Plans, design and large refactors |
| `explore`, `compact`, `title` | `claude-haiku-5-5` | Subagents, exploration, summarizing the history, naming chats |
| `deep` | `claude-fable-5-1` (falls back to Opus, then Sonnet) | Very hard tasks, on demand (`/model deep`, `--model deep`) |

Extended thinking, a budget per role and the aliases `opus`, `sonnet`, `haiku` and `fable` are built in; `app.models.roles` changes any of them, and a model picked in `/model` takes `main` and `component` and leaves the other roles alone. Anthropic does not let third-party apps use a Claude Pro/Max subscription, so crest needs an API key ([console.anthropic.com](https://console.anthropic.com/)).

### OpenCode

An [OpenCode](https://opencode.ai/) key gives crest the models of **OpenCode Go** (a flat-rate subscription for coding agents: its tokens cost nothing in the cost notices) and **OpenCode Zen** (pay as you go). On the first start after `/login opencode`, crest asks OpenCode which plans the key may use and lists only those models; after buying a subscription or credits, `/login opencode` again.

OpenCode has no built-in roles: pick a model in the `/model` popup and it serves every role, or set them in `init.lua`. Each model is called with the API of its family (Anthropic Messages, OpenAI Responses or Chat Completions), chosen from its name.

### Both at once

With both keys, prefixed ids reach either account from anywhere: a role, an alias, `/model`, or a fallback chain that crosses providers, such as Claude first and GLM when Anthropic is overloaded. `app.opt.providers.default` fixes the default provider instead of the last login. [Providers, ids and roles in detail](docs/configuration.md#model-providers-and-api-keys).

```lua
-- ~/.config/crest/init.lua
app.models.aliases = { glm = "opencode-go:glm-5.3", kimi = "opencode-go:kimi-k3" }
app.models.roles = {
  main    = { id = "sonnet", fallbacks = { "glm", "kimi" } },
  explore = "opencode-go:deepseek-v4.1-flash",
  compact = "opencode-go:deepseek-v4.1-flash",
}
```

## Everything in one place

`F1` (or `/help`) opens one searchable window with eight sections: every `/` command, the key bindings (`c` copies a snippet that remaps the selected one), the agent's tools and their effects, the plugins and their capabilities, the MCP servers, the models in use, the command-line flags and the paths crest reads. [The help](docs/usage.md#help).

<img src="assets/screenshots/help.png" alt="The help window" width="900">

## Documentation

**Jump straight to**: [connect an MCP server](docs/configuration.md#mcp-servers) · [write your first plugin](docs/plugins.md#step-by-step-examples) · [plugin capabilities](docs/plugins.md#capabilities) · [every block](docs/usage.md#blocks) · [statistics and calc cards](docs/usage.md#statistics-and-calculations) · [the research workflow](docs/workflows.md#research-workflow) · [simulating a solution](docs/workflows.md#simulating-a-solution) · [web search](docs/configuration.md#web-search) · [spending cap](docs/configuration.md#cost-and-spending-cap) · [create a theme](docs/themes.md#your-first-theme) · [permissions](docs/configuration.md#permissions-and-sensitive-paths) · [headless mode](docs/usage.md#headless-mode) · [a full `init.lua`](docs/configuration.md#full-example)

| Guide | What is in it |
| --- | --- |
| [Usage](docs/usage.md) | The window, the sidebar, the tour, every key and `/` command, approvals, blocks (statistics and calc cards, math, forms, plots, test reports, datasets, molecules, traces), the CLI, headless mode and sessions. |
| [Configuration](docs/configuration.md) | Claude and OpenCode keys, providers, `app.opt`, models by role, the spending cap, web search, the decision model, permissions, privacy, `keymap.toml`, MCP servers and a full `init.lua`. |
| [Workflows](docs/workflows.md) | The research workflow (its stages, gates and `.crest/specs/`), the discovery engine, simulating a solution (`/simulate`, the board, `simulation/`) and the implementation workflow (steps, gates, commits, `report.md`). |
| [Themes](docs/themes.md) | Using, creating and porting color themes; the shape tokens; how the window opens. |
| [Plugins](docs/plugins.md) | Writing Lua plugins: tools, hooks, blocks, computed cards, format readers, forms, modals, MCP and capabilities. |
| [Design system](docs/design-system.md) | The tokens, the kit components and the rules every view of the window follows. |
| [Architecture](docs/architecture.md) | Layers, ports and adapters, the loop of a turn, persistence. |
| [Development](docs/development.md) | Requirements, `make` targets, tests, fakes, screenshots, conventions and CI. |
| [Vision](docs/vision.md) | Where crest is going: principles and roadmap. |
| [Status](docs/status.md) | What is verified, what is not, and what is missing. |

**Research notes**: [chat blocks for technical work](docs/research/chat-blocks-for-technical-work.md) (which blocks, verified computation, the roadmap) · [the discovery flow](docs/research/discovery-flow.md) (a governed research at two levels) · [the solution simulator](docs/research/solution-simulator.md) · [the performance and architecture audit](docs/research/performance-architecture-audit.md) · [rich content in modals](docs/research/rich-content-modals.md) (what GPUI can draw, why no web views) · [installation](docs/research/installation.md) (how crest is packaged on each platform).

## Built with

Rust (edition 2024). The window is [GPUI](https://www.gpui.rs/) and [gpui-component](https://github.com/longbridge/gpui-component), with wgpu for the offscreen 3D views, RaTeX for math and resvg for SVG. The engines are numbat for units and statrs under a statistics engine checked against NIST StRD; the workflows run on graph-flow; MCP goes through rmcp. The format blocks read STEP with truck and ruststep, DXF, IFC, VCD/FST/GHW waveforms with wellen, molecules with chematic, GenBank with gb-io, spectra with mzdata, GeoJSON with geo and proj4rs, HDF5 and NetCDF with hdf5-pure, Parquet, graphs with petgraph and SQL schemas with sqlparser. Config and plugins are Lua 5.4 (mlua), sessions are SQLite, search is `rg` and `fzf`, the web tools run `curl`, and the models are reached through the Anthropic API for Claude and the Anthropic, OpenAI Responses and Chat Completions APIs for OpenCode. Langfuse tracing is a cargo feature (`--features langfuse`) set up from environment variables.

crest follows a hexagonal architecture: the domain and the use cases know nothing about GPUI, HTTP, Lua or SQLite, every port has an in-memory fake for the tests, and `make arch` enforces the boundaries. CI builds and tests it on Ubuntu 24.04, macOS 15 and Windows on every push.

```mermaid
flowchart TB
    bin["crates/bin · crest<br/>composition root: CLI, window or headless"]
    ui["crates/ui<br/>GPUI: timeline, blocks, modals, 3D views"]
    subgraph adapters["crates/adapters"]
        direction LR
        anthropic["anthropic<br/>Claude, OpenCode"] --- openai["openai<br/>OpenCode"] --- cloudflare["cloudflare<br/>Clef"] --- compute["compute<br/>statrs, numbat"] --- formats["formats<br/>file parsers"] --- search["search<br/>rg, fzf"] --- fs["fs"]
        shell["shell"] --- web["web<br/>curl"] --- store["store<br/>SQLite"] --- lua["lua<br/>mlua"] --- mcp["mcp<br/>rmcp"] --- telemetry["telemetry"] --- export["export<br/>board.html"]
    end
    app["crates/app · application<br/>tools, workflows"]
    core["crates/app-core · application-core<br/>ports, use cases, agent loop"]
    domain["crates/domain<br/>pure types and rules"]
    platform["crates/platform<br/>processes, directories, console"]
    bin --> ui & adapters & app & domain & platform
    ui --> core & domain
    adapters --> core & domain
    lua --> app
    adapters -.-> platform
    app --> core
    core --> domain
```

## Vision and roadmap

crest is the workspace where a coding agent **shows its work, verifies it, and leaves you in control**. Next come **your project and your sessions** (session tabs as in tmux, context and cost you can see, a definition of done that runs your checks before a turn ends), **pipelines written in Lua**, **any model** including local ones, **memory that survives compaction**, and **review and trust** (a review panel with `/undo`, sandboxed commands, workspace trust). Signed and notarized macOS builds are already here. Read the [vision and roadmap](docs/vision.md).

## Status and license

crest is a **work in progress** by TwoCrest Systems. [docs/status.md](docs/status.md) lists what has been verified, what has not, and the known limitations. There is no changelog file: the notes of each version are on its [GitHub release](https://github.com/lazaroMB/crest/releases).

crest is free software: you can redistribute it and modify it under the terms of the [GNU General Public License](LICENSE), version 3 or (at your option) any later version. For a commercial license, contact TwoCrest Systems. Third-party code keeps its own license: see [THIRD-PARTY.md](THIRD-PARTY.md).
