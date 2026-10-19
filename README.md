# crest

crest is a coding agent in the style of Claude Code, written in Rust. It opens in a full-screen window that looks like a terminal and works on a directory, the *workspace*: it reads, searches and edits files and runs commands with your approval.

What sets it apart:

- **A timeline with rich blocks.** The agent does not just answer with text. It shows charts (with their table, one `t` keypress away), sortable tables, Mermaid diagrams with pan and zoom, markdown documents, colored diffs and blocks defined by your own plugins.
- **Anthropic models by role.** Every call asks for a role (`main`, `plan`, `explore`, `compact`, `title`, `component`, `deep`), and the configuration decides which model serves it, with its fallbacks.
- **Search with `rg` and `fzf`.** The agent's tools and the window's pickers use the same processes, so you see what the model sees.
- **A prompt that is always ready to type**: `/` commands with a palette, `/!cmd` to run a shell command and hand its output to the model, shortcuts on the `Ctrl+Space` leader that work wherever the focus is, and every panel usable from the keyboard. The status bar shows the git branch and changes. TwoCrest theme (dark or light) and logo.
- **Config and plugins in Lua**, with Neovim-style key notation (`<leader>ff`, `<C-p>`). From Lua you can add tools, hooks, commands, blocks and modal windows (images with zoom, tables, charts, JSON trees, Mermaid, diffs, markdown and simple HTML), all inside a sandbox with instruction and memory limits. Plugins that need to call an API or a command-line program declare `network:` and `shell:` capabilities, which stay off until you approve them (`--trust-plugin`, `/plugin trust`).
- **Guided workflows.** The *research* workflow (X-Workflow, 14 nodes) goes from a problem to a Spec with its plan. The *implementation* workflow runs that plan task by task, with one commit per task and a final report.
- **Simulate a solution before building it.** `/simulate <spec>` turns a Spec into an executable model (assertions derived from its test protocol first, then the model from its diagrams), runs every scenario deterministically and opens a 3D circuit-board view where messages travel between components, with a timeline, failure markers and a state inspector. The verdicts become evidence in the Spec, and `board.html` shares the replay without crest.
- **Everything in one help.** `F1` (or `Ctrl+Space h`, or `/help`) opens a modal with every `/` command, the effective keymap, the agent's tools with their effects, the loaded plugins with their capabilities and trust, the MCP servers and their state, the models, the command-line flags and the paths in use, all searchable at once.
- **Hexagonal architecture.** The domain and the use cases know nothing about GPUI, HTTP, Lua or SQLite, and `make arch` verifies it.

## The screen

There are no screenshots yet. The window has three stacked areas and an optional side panel:

- **Timeline.** Takes up most of the screen. Every reply, tool call, diff or chart is a block, and you move between them with `j`/`k`.
- **Input.** Multi-line, below the timeline. It shows the current mode (`INSERT`, `NORMAL` or `COMMAND`) and the pending keys of a sequence.
- **Status bar.** At the bottom: mode, the turn's model, permission mode (`ask`, `auto-edit`, `plan`), session cost and workspace name. Notices are shown for a few seconds.
- **Workspace tree.** Optional panel on the left (`Ctrl+Space e`). On top of all of this appear the file and text pickers, the model menu, the modal windows that plugins open and the approval dialog with the diff or the exact command.

The theme is dark and the font monospace (JetBrains Mono if installed).

## Requirements

| Component | Version | Notes |
| --- | --- | --- |
| Linux or macOS | — | Windows is out of scope. |
| Rust (rustup, rustfmt, clippy) | stable, ≥ 1.99 | Edition 2024. `rust-toolchain.toml` pins `stable`. |
| C compiler, `pkg-config`, `cmake`, `clang` | — | On macOS, the Xcode Command Line Tools. |
| GPUI libraries on Linux | — | `libxkbcommon`, `libxkbcommon-x11`, `wayland`, `libxcb`, `libx11-xcb`, `fontconfig`, `freetype`. |
| Vulkan on Linux | — | The loader and a GPU driver (GPUI renders with Vulkan). |
| macOS | ≥ 11 | Xcode with the Metal compiler (`make doctor` checks it). |
| `ripgrep` (`rg`) | ≥ 13.0 | The `grep`, `list_files`, `find_file` and `tree` tools. |
| `fzf` | ≥ 0.36 | `find_file` (`--filter --scheme=path`). Without `fzf`, it falls back to substring search. |
| `git` | — | Repo data and workflows. |
| `curl` | — | The `web_search` and `web_fetch` tools. Without it the model has no web. |
| `jq` | — | Only for `scripts/check-arch.sh`. |
| Anthropic API key | — | `ANTHROPIC_API_KEY` or the system keyring (service `crest`, entry `ANTHROPIC_API_KEY`). |

SQLite is compiled into the binary and TLS uses rustls, so neither `libsqlite3` nor OpenSSL is needed.

## Installation

```bash
git clone <repo> crest && cd crest
make deps      # installs what is missing (apt, dnf, yum, pacman, zypper, apk, xbps or Homebrew)
make doctor    # checks without installing anything; exits with 1 if something is missing
make release   # target/release/crest (or `make build` for debug)
make install   # copies the binary to ~/.local/bin (PREFIX=/other/path to change it)
```

`make deps` uses `scripts/install-deps.sh`, which accepts:

- `--check`: only checks;
- `--yes`: does not ask before installing;
- `--dev`: adds `cargo-insta` and `cargo-nextest`;
- `--no-rust`: neither checks nor installs Rust.

If your distro's `rg` or `fzf` is older than the minimum (Ubuntu 22.04's `fzf` is 0.29), the script downloads the official binaries to `~/.local/bin`. `make deps-dev` is the same as `make deps` with `--dev`.

Then set the key:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

To store it in the keyring instead of using the variable, see [docs/configuration.md](docs/configuration.md#anthropic-api-key).

## Quick start

```bash
crest ~/src/mi-proyecto       # or: make run WORKSPACE=~/src/mi-proyecto
crest                         # without a directory: choose the project in the picker
```

1. Type a prompt, for example "what does src/main.rs do?", and press `Enter`. `Shift+Enter` inserts a new line.
2. When the agent wants to edit a file or run a command, the approval dialog appears:
   - `y` approves;
   - `a` always approves (that same file or command);
   - `n` rejects.
3. Commands start with `/` (typing `/` opens the palette) and shortcuts with `Ctrl+Space`:
   - `/!git status` runs a command and shows its output (the model sees it with your next prompt);
   - `Alt+↑`/`Alt+↓` move between blocks and `Alt+Enter` expands a tool call (or `Ctrl+Tab` to focus the timeline and use the arrows);
   - `Ctrl+Space ff` searches files, `Ctrl+Space e` shows the tree;
   - `Ctrl+Space p` changes the permission mode;
   - `Esc` always brings you back to the prompt;
   - `/q` quits.
4. When you open crest again in the same directory, the last session is resumed. `crest --new` starts a new one.

Without a window (headless), crest runs one turn and prints it to the terminal, with approvals on stdin:

```bash
crest --headless "find where the config is parsed and explain it" --workspace .
crest --headless "fix the failing test" --mode auto-edit --yes
make run-headless PROMPT="list the TODOs" WORKSPACE=.
```

To research a problem and then implement the fix:

```text
/research the add() tests fail with negative numbers
…answer `yes` (or `sí`) at each gate…
/simulate 0001-<slug>      # optional: check the solution on a board first
/implement 0001-<slug>
```

## Documentation

| Guide | Contents |
| --- | --- |
| [docs/usage.md](docs/usage.md) | Window, modes, every key binding and `:` command, approvals, blocks, the full CLI, headless mode, sessions, MCP and plugin trust. |
| [docs/configuration.md](docs/configuration.md) | Paths, precedence, API key, `app.opt`, models by role, permissions, environment variables, Langfuse and a full `init.lua`. |
| [docs/plugins.md](docs/plugins.md) | How to write Lua plugins: sandbox, the `app` table, tools, hooks, `Custom` blocks, MCP, capabilities (HTTP, programs, secrets) and examples. |
| [docs/workflows.md](docs/workflows.md) | The research workflow (14 nodes, gates, Specs), simulating a solution (model, scenarios, board) and the implementation workflow (steps, gates, commits, report). |
| [docs/architecture.md](docs/architecture.md) | Layers, ports and adapters, the loop of a turn, events, threads and persistence. |
| [docs/development.md](docs/development.md) | Tests, fakes and how to add tools, blocks, adapters or nodes; commit conventions and CI. |

## Architecture at a glance

```mermaid
flowchart TB
    bin["crates/bin · crest<br/>composition root: CLI, window or headless"]
    ui["crates/ui<br/>GPUI: timeline, blocks, modal input, 3D board"]
    subgraph adapters["crates/adapters"]
        direction LR
        anthropic["anthropic"] --- search["search<br/>rg, fzf"] --- fs["fs"] --- shell["shell"] --- web["web<br/>curl"]
        store["store<br/>SQLite"] --- lua["lua<br/>mlua"] --- mcp["mcp<br/>rmcp"] --- telemetry["telemetry"] --- export["export<br/>board.html"]
    end
    app["crates/app · application<br/>ports, use cases, tools, workflows"]
    domain["crates/domain<br/>pure types and rules"]
    bin --> ui & adapters & app & domain
    ui --> app & domain
    adapters --> app & domain
    app --> domain
```

- `domain` depends on no crate of the project, and `application` only on `domain`.
- Each adapter and `ui` depend only on `application` and `domain`. Only `bin` wires them together.
- The use cases talk to the world through ports (`LlmPort`, `FileSystemPort`, `ShellPort`, `ApprovalPort`, `EventSink`...), and every port has a fake in `application::testing`.

The details are in [docs/architecture.md](docs/architecture.md).

## Repository layout

```text
Cargo.toml              workspace, shared dependencies and lints
Makefile                build, test and check targets
rust-toolchain.toml     stable toolchain with rustfmt and clippy
crates/
  domain/               pure types and rules
  app/                  `application`: ports, use cases, tools, workflows, fakes
  adapters/             anthropic, search, fs, shell, store, lua, mcp, telemetry, export
                        (export/vendor/three/ holds the inlined three.js, MIT)
  ui/                   GPUI views (plus runnable examples/)
  bin/                  the `crest` binary and end-to-end integration tests
scripts/
  install-deps.sh       installs or checks dependencies (Bash 3.2+)
  check-arch.sh         hexagonal architecture rules
  package.sh            builds the release tarball
tests/
  arch/                 fixtures and tests for check-arch.sh
  install-deps/         tests for install-deps.sh
docs/                   user and developer guides
.github/workflows/      ci.yml, live.yml, release.yml
```

## Makefile targets

| Target | What it does |
| --- | --- |
| `make help` | Lists the targets (default). |
| `make deps` / `make deps-dev` | Installs the dependencies (with development tools in the second). |
| `make doctor` | Checks the dependencies without installing. |
| `make build` / `make release` | Builds the workspace (debug) / the optimized `target/release/crest` binary. |
| `make run WORKSPACE=<dir>` | Opens the window on a directory (`.` by default). |
| `make run-headless PROMPT="..." [WORKSPACE=<dir>]` | One turn without a window. |
| `make fmt` / `make fmt-check` | Formats / checks the formatting. |
| `make lint` | Strict Clippy (`-D warnings`). |
| `make test` / `make test-live` | Offline tests / tests against the real API (needs `ANTHROPIC_API_KEY`). |
| `make arch` / `make test-arch` | Architecture rules / their tests. |
| `make test-deps` | Tests for `install-deps.sh`. |
| `make check` | `fmt-check`, `lint`, `test`, `arch`, `test-arch` and `test-deps`. |
| `make ci` | `doctor` + `check`. |
| `make package` | Release tarball in `dist/`. |
| `make install` | Copies the binary to `$(PREFIX)/bin` (`PREFIX=~/.local`). |
| `make clean` | `cargo clean`. |

## Development

```bash
make deps-dev
make check        # what must pass before every commit
make test-live    # optional: tests against the real API
```

- The tests use no network: use cases are tested with the fakes from `application::testing`, the API with `wiremock` and the UI with GPUI's test context.
- Tests against the real API are marked `#[ignore]` and need `ANTHROPIC_LIVE_TEST=1` (set by `make test-live`) and `ANTHROPIC_API_KEY`.
- CI (`.github/workflows/ci.yml`) runs `make ci` on Ubuntu 24.04 and macOS 15. `live.yml` runs `make test-live` manually, and `release.yml` publishes tarballs for Linux x86_64 and macOS (arm64 and x86_64) on every `v*` tag.

The conventions and extension points are in [docs/development.md](docs/development.md).

## Project status and known limitations

All 11 phases of the original plan are implemented and `make check` passes on Linux. Before relying on crest for real work, keep in mind what has not been tested and what is still missing:

**Unverified**

- It has not been tested against the real Anthropic API in the development environment: every test uses scripted models or `wiremock`, and `make test-live` has not been run here. The default model ids (`claude-sonnet-5-5`, `claude-opus-5-5`, `claude-haiku-5-5`, `claude-fable-5-1`) are validated against `GET /v1/models` at startup.
- macOS has not been tested on a real machine. The code and the CI account for it, but nothing has been run there.
- CI has never run on GitHub: the repository has no remote configured.
- Reading the API key or plugin secrets from the system keyring has not been tested (the tests use the environment fallback).

**Behavior to keep in mind**

- `Ctrl+C` stops a running workflow step in the window; the workflow stays saved at its last checkpoint and `/research resume` or `/implement resume` runs the step again. In headless mode it kills the process (the same resume applies).
- At workflow gates, an empty line counts as an approval (`yes` / `continue`). In the implementation workflow that can commit with failing criteria or close the workflow.
- In the implementation workflow, only `edit_file` and `write_file` are restricted to the task's files: `run_command` can touch others (although the commit only includes the task's files).
- crest ships no model prices: until you set `app.opt.pricing`, the cost shows as `$?`. The cost lives in memory, so a resumed session starts again at `$0.00` against the cap.

**Not available yet**

- The font is not configurable (JetBrains Mono when installed).
- `app.opt.leader` only affects mappings defined in Lua; the builtin ones always use `Ctrl+Space`. Actions such as `session.resume` are not implemented.
- There is no command to list or pick older sessions: the workspace's last one is always resumed.
- Compaction assumes a 200,000-token context window, whatever the model, and its threshold is not configurable.
- `/mcp trust` takes effect the next time you open crest; `--trust-mcp` starts the server in the same run. (`/plugin trust`, by contrast, takes effect at once.)
- Plugin capabilities cover HTTP(S) to listed hosts and listed programs; there is no file-system capability, and a slow `app.http` or `app.shell.run` call holds the Lua VM (other hooks wait) until it finishes or times out.
- Simulation (MVP): flat state machines, FIFO queues and timers as self-messages; no latency percentiles, load generators, fault toggles in the board, GIF/video export or re-simulation inside `board.html` yet, and the board has only a dark theme. See [docs/research/solution-simulator.md](docs/research/solution-simulator.md).
- Plugin modals draw images, documents, tables and diagrams natively, but cannot embed web pages (links open the browser), PDFs, 3D models or a terminal yet; images larger than the GPU's texture limit (often 8192 or 16384 px) fail to draw. See [docs/research/rich-content-modals.md](docs/research/rich-content-modals.md) for the roadmap.
- Langfuse export requires building with `--features langfuse`; the `make release` binaries do not include it.
- The macOS binaries from `release.yml` are not signed or notarized.
