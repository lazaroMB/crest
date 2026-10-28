<div align="center">

<img src="assets/logo.svg" alt="crest logo" width="96">

# crest

**The coding agent that shows its work.**

A native, GPU-rendered workspace for Claude and the OpenCode models: it reads, searches and edits your code with your approval,<br>
and answers with charts, tables, diagrams, diffs and 3D models instead of a wall of text.

![License](https://img.shields.io/badge/license-GPL--3.0-blue)
![Rust](https://img.shields.io/badge/rust-1.99%2B-orange?logo=rust)
![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey)
![Models](https://img.shields.io/badge/models-Claude%20%7C%20OpenCode-8a63d2)
![Release](https://img.shields.io/badge/release-v0.16.0-d9604a)
![Status](https://img.shields.io/badge/status-work%20in%20progress-yellow)

<img src="assets/screenshots/hero.png" alt="crest finding a bug, fixing it, running the tests and charting their timings" width="900">

</div>

> [!WARNING]
> **crest is a work in progress.** It is usable day to day, but it is young: expect rough edges, missing features and breaking changes between releases. Some parts have only been tested with mocks (see [what is verified](docs/status.md)). Feedback and bug reports are very welcome.

**Contents**

- [Why crest](#why-crest)
- Features: [a timeline, not a transcript](#a-timeline-not-a-transcript) · [you approve every change](#you-approve-every-change) · [open anything](#open-anything) · [smart routing, smaller bills](#smart-routing-smaller-bills) · [everything in one place](#everything-in-one-place)
- Extend it: [connect any MCP server](#connect-any-mcp-server) · [make it yours](#make-it-yours) · [write a plugin](#write-a-plugin) · [themes](#themes)
- [Claude and OpenCode](#claude-and-opencode): [Claude](#claude) · [OpenCode](#opencode) · [both at once](#both-at-once)
- [Install](#install) · [Quick start](#quick-start) · [Documentation](#documentation)
- [Built with](#built-with) · [Vision and roadmap](#vision-and-roadmap) · [Status and license](#status-and-license)

## Why crest

<table>
<tr>
<td width="33%" valign="top"><b>A timeline, not a transcript</b><br>Every reply, tool call, diff, chart, table and diagram is a block you can expand, sort, zoom and copy.</td>
<td width="33%" valign="top"><b>You approve every change</b><br>Edits and commands wait for you, with the exact diff or command line. Allow once, always, or never.</td>
<td width="33%" valign="top"><b>Open anything</b><br>Code, markdown, JSON, diffs, pictures and 3D models (OBJ, STL, PLY) open in a modal with <code>/open @path</code>.</td>
</tr>
<tr>
<td valign="top"><b>Smart routing, smaller bills</b><br>An optional decision model (Cloudflare Clef) picks the tools and the model of each prompt in milliseconds.</td>
<td valign="top"><b>Connect any MCP server</b><br>Local or hosted, with a token or an OAuth login. Workspace servers wait until you trust them.</td>
<td valign="top"><b>Make it yours</b><br>Lua plugins add tools, commands, hooks, keymaps, blocks and modals, in a sandbox with explicit capabilities.</td>
</tr>
</table>

## A timeline, not a transcript

The agent answers with real components, drawn natively on the GPU: bar, line, area and pie charts (press `t` for the data behind them), sortable tables, Mermaid diagrams with pan and zoom, colored diffs, rendered markdown and the blocks your plugins define.

<img src="assets/screenshots/blocks.png" alt="A chart and a table in the timeline" width="900">

## You approve every change

Nothing touches your files or runs a command without asking (unless you choose `auto-edit`). The dialog shows the exact diff or command line; `y` approves, `a` always approves that file or command, `n` rejects. A `plan` mode lets the agent read but never write.

Secrets and personal data never leave your machine by accident: API keys, tokens, private keys, passwords, emails, cards, IBANs and national ids in tool results and in your prompts are masked as `[REDACTED:kind]` before the model, the timeline or the saved session see them. [What is masked](docs/configuration.md#privacy-secrets-and-personal-data).

<img src="assets/screenshots/approval.png" alt="An edit waiting for approval with its diff" width="900">

## Open anything

`/open @path` shows any file of the workspace in a modal: highlighted code, rendered markdown, JSON trees, Mermaid, diffs, pictures with zoom, and **3D models** (OBJ, STL and PLY) in an orbit viewer. Plugins open the same modals with their own content.

<table>
<tr>
<td width="50%"><img src="assets/screenshots/model3d.png" alt="An STL gear in the 3D viewer"></td>
<td width="50%"><img src="assets/screenshots/modal.png" alt="A plugin modal with a chart, a JSON tree and a diagram"></td>
</tr>
</table>

## Smart routing, smaller bills

With Cloudflare credentials, a small **decision model** ([Clef](https://developers.cloudflare.com/workers-ai/models/clef-flash/)) answers typed questions in milliseconds for a fraction of a cent:

- **which tools** each prompt needs, so the model gets a shorter, cheaper request;
- **which model** answers it: a quick one for a greeting, a strong one for a redesign.

Without credentials crest works exactly as before. [How to turn it on](docs/configuration.md#decision-model-cloudflare-clef).

With [rtk](https://github.com/rtk-ai/rtk) installed, `app.opt.rtk = true` compacts the output of `git`, `cargo`, test runners and other common commands before the model reads it. [How it works](docs/configuration.md#compact-command-output-with-rtk).

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

- `/mcp` shows every server, its state and the next step; `/mcp tools <name>` lists what the model sees, `/mcp login <name>` signs in through the browser and keeps the tokens in the system keyring.
- Their tools go through the same approvals as the builtin ones: `effect = "read"` lets a server run without asking.
- Servers declared in a workspace's `.crest/init.lua` do not start until you approve them with `/mcp trust <name>`, and ask again if their command or URL changes.
- Tool names and schemas are adapted to each provider, so the same server works with Claude and with the OpenCode models.

**Set up a server**: [the guide](docs/configuration.md#mcp-servers) · [every field](docs/plugins.md#appmcp-mcp-servers) · [`/mcp` commands](docs/configuration.md#mcp) · [OAuth and GitHub](docs/configuration.md#oauth) · [workspace trust](docs/usage.md#mcp-servers-and-trust) · [`npx` not found from the app menu](docs/configuration.md#npx-from-the-app-menu)

## Make it yours

Configuration and plugins are Lua, with Neovim-style key notation. A plugin is a folder with an `init.lua` in `~/.config/crest/plugins/`, and it can add:

| Extension point | What it does | Docs |
| --- | --- | --- |
| Tools | New tools the model can call, with a JSON schema and an effect for approvals | [`app.tools.register`](docs/plugins.md#apptoolsregisterdef) |
| Hooks | Inspect, rewrite or deny every tool call (builtin, plugin and MCP) before it runs | [`app.on`](docs/plugins.md#apponevent-fn-hooks) |
| Commands | Your own `/` commands, which can start a turn with any model role | [`app.commands.register`](docs/plugins.md#appcommandsregistername-fn-opts) |
| Keymaps | Bindings in Neovim notation, with your own leader | [`app.keymap.set`](docs/plugins.md#appkeymapsetkeys-action-opts) |
| Blocks | Charts, tables, diagrams, JSON trees, diffs and markdown in the timeline | [`app.ui`](docs/plugins.md#appui-custom-blocks) |
| Modals | Windows over crest with images, CSV tables, charts and documents | [`app.modal`](docs/plugins.md#appmodal-modal-windows) |
| MCP servers | Bundle the servers your plugin needs | [`app.mcp`](docs/plugins.md#appmcp-mcp-servers) |
| APIs, programs, secrets | HTTP requests, command-line programs and keyring secrets, once you approve them | [Capabilities](docs/plugins.md#capabilities) |

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

Plugins run in a sandbox: no files, no network, no programs, and limits on instructions and memory. A plugin that needs an API or a program declares it, and it stays off until you approve it with `/plugin trust <name>`:

```lua
app.plugin{ name = "jira", capabilities = { "network:jira.example.com", "shell:jq" } }
```

**Start from a recipe**: [a tool with a table and a pie chart](docs/plugins.md#1-a-todo_list-tool-with-a-custom-block) · [a hook that blocks `rm` and dry-runs `git push`](docs/plugins.md#2-a-hook-that-blocks-rm-and-forces-git-push---dry-run) · [a command on the `plan` model](docs/plugins.md#3-a-command-that-starts-a-turn-with-the-plan-role) · [a Jira tool with `app.http` and a secret](docs/plugins.md#example-an-api-tool-with-apphttp-and-appsecret) · [a `jq` wrapper](docs/plugins.md#example-a-cli-wrapper-with-appshellrun-and-jq) · [testing a plugin](docs/plugins.md#testing-a-plugin). Everything else is in [Writing plugins](docs/plugins.md).

**Try the demo plugin**: [`examples/plugins/showcase`](examples/plugins/showcase/init.lua) shows every extension point in one file. Add it to `~/.config/crest/init.lua` and type `/demo`:

```lua
app.plugins = { "~/path/to/crest/examples/plugins/showcase" }
```

### Themes

crest ships 14 themes ported from the most used Neovim colorschemes (catppuccin, tokyonight, kanagawa, rose-pine, gruvbox, nightfox, onedark, dracula, nord, everforest), and the installers copy them into your config folder. Type `/theme` and move through the list to see each one in the window, `Enter` to keep it. A theme is a Lua file that colors every element of the window in each of its states (hover, pressed, focused, selected, disabled) and every kind of code token, and anything it leaves out is derived from the rest:

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

`/login` opens the key dialog (`Tab` switches the account, `/login opencode` opens it on OpenCode). crest checks the key with the provider and stores it in the system keyring, never in a file; `/logout` deletes it. The account you log in to last becomes the **default provider**, the one that serves model ids without a prefix. `/model` (or `Ctrl+G m`) opens a popup with every model your accounts can use.

### Claude

With an Anthropic key, crest works out of the box: every role has a Claude model and a fallback, so a quick model explores and summarizes while a strong one plans.

| Role | Model | For |
| --- | --- | --- |
| `main` | `claude-sonnet-5-5` (falls back to Haiku) | The conversation and the agent loop |
| `plan` | `claude-opus-5-5` | Plans, design and large refactors |
| `explore`, `compact` | `claude-haiku-5-5` | Subagents, exploration and summarizing the history |
| `deep` | `claude-fable-5-1` | Very hard tasks, on demand (`/model deep`, `--model deep`) |

Extended thinking, a budget per role and the aliases `opus`, `sonnet`, `haiku` and `fable` are built in; `app.models.roles` changes any of them. Picking a model in `/model` replaces `main` and `component` and keeps the other roles. Anthropic does not let third-party apps use a Claude Pro/Max subscription, so crest needs an API key ([console.anthropic.com](https://console.anthropic.com/)).

### OpenCode

An [OpenCode](https://opencode.ai/) key gives crest the models of **OpenCode Go** (a flat-rate subscription for coding agents: its tokens cost nothing in the cost notices) and **OpenCode Zen** (pay as you go). On the first start after `/login opencode`, crest asks OpenCode which plans the key may use and lists only those models; after buying a subscription or credits, `/login opencode` again.

OpenCode has no built-in roles: pick a model in the `/model` popup and it serves every role, or set them in `init.lua`. Each model is called with the API of its family (Anthropic Messages, OpenAI Responses or Chat Completions), chosen from its name; Gemini models are not supported yet.

```lua
-- ~/.config/crest/init.lua
app.models.aliases = { glm = "opencode-go:glm-5.3", kimi = "opencode-go:kimi-k3" }
app.models.roles = {
  main    = { id = "glm", fallbacks = { "kimi" } },
  explore = "opencode-go:deepseek-v4.1-flash",
  compact = "opencode-go:deepseek-v4.1-flash",
}
```

### Both at once

With both keys, prefixed ids reach either account from anywhere: a role, an alias, `/model`, or a fallback chain that crosses providers, such as Claude first and GLM when Anthropic is overloaded. `app.opt.providers.default` fixes the default provider instead of the last login. [Providers, ids and roles in detail](docs/configuration.md#model-providers-and-api-keys).

```lua
app.models.roles = {
  main = { id = "sonnet", fallbacks = { "opencode-go:glm-5.3" } },
  explore = "opencode-go:deepseek-v4.1-flash",
}
```

## Everything in one place

`F1` (or `/help`) opens one searchable window with every `/` command, the key bindings, the agent's tools and their effects, the plugins and their capabilities, the MCP servers, the models in use, the command-line flags and the paths crest reads.

<img src="assets/screenshots/help.png" alt="The help window" width="900">

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

You can also download `crest-<version>-x86_64-pc-windows-msvc-setup.exe` from the [latest release](https://github.com/lazaroMB/crest/releases/latest) and run it. crest is not code-signed yet, so Windows SmartScreen warns about a setup downloaded with a browser: choose **More info → Run anyway**. (winget and the PowerShell command do not trigger it.) The Windows build is new and has only been tested in CI: reports are welcome.

**Linux** (x86_64 or aarch64, glibc 2.35+; no sudo), and macOS without Homebrew:

```bash
curl -fsSL https://raw.githubusercontent.com/lazaroMB/crest/main/scripts/install.sh | sh
```

It downloads the [latest release](https://github.com/lazaroMB/crest/releases/latest), checks its checksum and installs crest as an app: in the app menu on Linux (GNOME, KDE and other XDG desktops), in `~/Applications` on macOS (Launchpad, Spotlight, Dock), plus the `crest` command in `~/.local/bin`. `sh -s -- --version v0.16.0` picks a release, `--uninstall` removes it. The release tarballs carry the same script: extract one and run `./install.sh`.

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
crest                      # or pick a recent project
crest --headless "find where the config is parsed and explain it" --workspace .
```

- Type a prompt and press `Enter` (`Shift+Enter` for a new line). `↑` brings back what you sent before.
- `/` opens the command palette: `/open @file`, `/model`, `/mode`, `/theme`, `/mcp`, `/!git status`...
- `Ctrl+G` is the leader: `Ctrl+G ff` finds a file, `Ctrl+G fg` greps, `Ctrl+G e` shows the tree, `Ctrl+G m` picks a model.
- `Esc` always brings you back to the prompt; `F1` shows everything else.

Opening crest again in the same directory resumes the last session; `crest --new` starts a fresh one.

## Documentation

**Jump straight to**: [connect an MCP server](docs/configuration.md#mcp-servers) · [write your first plugin](docs/plugins.md#step-by-step-examples) · [plugin capabilities](docs/plugins.md#capabilities) · [create a theme](docs/themes.md#your-first-theme) · [permissions](docs/configuration.md#permissions-and-sensitive-paths) · [headless mode](docs/usage.md#headless-mode) · [a full `init.lua`](docs/configuration.md#full-example)

| Guide | What is in it |
| --- | --- |
| [Usage](docs/usage.md) | The window, every key and `/` command, approvals, blocks, the CLI, headless mode and sessions. |
| [Configuration](docs/configuration.md) | Claude and OpenCode keys, providers, `app.opt`, models by role, the decision model, permissions, MCP servers and a full `init.lua`. |
| [Themes](docs/themes.md) | Using, creating and porting color themes; how the window opens. |
| [Plugins](docs/plugins.md) | Writing Lua plugins: tools, hooks, blocks, modals, MCP and capabilities. |
| [Architecture](docs/architecture.md) | Layers, ports and adapters, the loop of a turn, persistence. |
| [Development](docs/development.md) | Requirements, `make` targets, tests, fakes, conventions and CI. |
| [Vision](docs/vision.md) | Where crest is going: principles and roadmap. |
| [Status](docs/status.md) | What is verified, what is not, and what is missing. |

## Built with

Rust (edition 2024), [GPUI](https://www.gpui.rs/) and [gpui-component](https://github.com/longbridge/gpui-component) for the native UI, wgpu for the 3D views, Lua 5.4 (mlua) for config and plugins, SQLite for sessions, `rg` and `fzf` for search, the Anthropic API for Claude, and the Anthropic, OpenAI Responses and Chat Completions APIs for OpenCode.

crest follows a hexagonal architecture: the domain and the use cases know nothing about GPUI, HTTP, Lua or SQLite, every port has an in-memory fake for the tests, and `make arch` enforces the boundaries.

```mermaid
flowchart TB
    bin["crates/bin · crest<br/>composition root: CLI, window or headless"]
    ui["crates/ui<br/>GPUI: timeline, blocks, modals, 3D views"]
    subgraph adapters["crates/adapters"]
        direction LR
        anthropic["anthropic<br/>Claude, OpenCode"] --- openai["openai<br/>OpenCode"] --- cloudflare["cloudflare<br/>Clef"] --- search["search<br/>rg, fzf"] --- fs["fs"] --- shell["shell"] --- web["web<br/>curl"]
        store["store<br/>SQLite"] --- lua["lua<br/>mlua"] --- mcp["mcp<br/>rmcp"] --- telemetry["telemetry"] --- export["export<br/>board.html"]
    end
    app["crates/app · application<br/>ports, use cases, tools, workflows"]
    domain["crates/domain<br/>pure types and rules"]
    bin --> ui & adapters & app & domain
    ui --> app & domain
    adapters --> app & domain
    app --> domain
```

## Vision and roadmap

crest is heading to **your project and your sessions** (compatible with `AGENTS.md` and `CLAUDE.md`, session tabs as in tmux, context and cost you can see, a definition of done), then **pipelines written in Lua** and **any model**, local ones included, then **review and trust** (a review panel, sandboxed commands, workspace trust). Read the [vision and roadmap](docs/vision.md).

## Status and license

crest is a **work in progress** by TwoCrest Systems. [docs/status.md](docs/status.md) lists what has been verified, what has not, and the known limitations.

crest is free software: you can redistribute it and modify it under the terms of the [GNU General Public License](LICENSE), version 3 or (at your option) any later version. For a commercial license, contact TwoCrest Systems. Third-party code keeps its own license: see [THIRD-PARTY.md](THIRD-PARTY.md).
