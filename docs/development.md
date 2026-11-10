# Development

This guide is for people changing crest's code. Before touching anything, read [architecture.md](architecture.md).

## Setting up the environment

```bash
make deps-dev   # dependencies + cargo-insta and cargo-nextest
make doctor     # checks without installing
make build
```

The toolchain is Rust 1.99 (`rust-toolchain.toml`), edition 2024, with `rust-version = 1.99`. If Rust does not come from rustup, `make doctor` warns that `rust-toolchain.toml` will not be applied.

### Requirements

| Component | Version | Notes |
| --- | --- | --- |
| Linux, macOS or Windows | — | Windows 10 1809+ (x86_64); see [Windows](#windows). |
| Rust (rustup, rustfmt, clippy) | ≥ 1.99 | Edition 2024. `rust-toolchain.toml` pins `1.99`. |
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

### Installing the dependencies

`make deps` uses `scripts/install-deps.sh`, which accepts:

- `--check`: only checks;
- `--yes`: does not ask before installing;
- `--dev`: adds `cargo-insta` and `cargo-nextest`;
- `--no-rust`: neither checks nor installs Rust.

If your distro's `rg` or `fzf` is older than the minimum (Ubuntu 22.04's `fzf` is 0.29), the script downloads the official binaries to `~/.local/bin`. `make deps-dev` is the same as `make deps` with `--dev`.

### Windows

The `make` targets and `scripts/*.sh` are for Linux and macOS; on Windows use cargo directly:

```powershell
winget install Rustlang.Rustup Git.Git BurntSushi.ripgrep.MSVC junegunn.fzf
winget install Microsoft.VisualStudio.2022.BuildTools  # "Desktop development with C++": MSVC and the Windows SDK
cargo build -p crest
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

- GPUI renders with DirectX 11. Release builds compile its shaders with `fxc.exe` from the Windows SDK (set `GPUI_FXC_PATH` if it is not found).
- `crest.exe` is a GUI program (`windows_subsystem = "windows"`): started from a terminal it attaches to that console, so `--help` and `--headless` print there, but the shell does not wait for it.
- `crates/platform` holds what differs per OS. Processes run without a console window; a command and its children are killed with `taskkill /T` (process groups on Unix); bare names are looked up with `PATHEXT` (`npx` is `npx.cmd`).
- Paths: the workspace accepts `\` as a separator and drive paths on its own drive (`C:\ws\src`), and rejects other drives, UNC shares and drive-relative paths (`C:src`); sensitive-path rules ignore case on Windows and macOS.
- Tests that need symlinks or POSIX tools (`sh`, `sleep`, `cat`) are `#[cfg(unix)]`; in-memory workspaces use `application::testing::fs::test_root()` (`C:\ws` on Windows).

### Repository layout

```text
Cargo.toml              workspace, shared dependencies and lints
Makefile                build, test and check targets
rust-toolchain.toml     Rust 1.99 with rustfmt and clippy
crates/
  domain/               pure types and rules
  app/                  `application`: ports, use cases, tools, workflows, fakes
  adapters/             anthropic, openai, cloudflare, search, fs, shell, web, store,
                        lua, mcp, telemetry, export, compute, formats, and support
                        (what the model adapters share: SSE loop, HTTP errors)
                        (export/vendor/three/ holds the inlined three.js, MIT)
  platform/             OS glue shared by adapters, UI and bin: spawning and killing
                        processes, PATH lookup, crest's directories, the Windows console
  ui/                   GPUI views (plus runnable examples/, `showcase` among them)
  bin/                  the `crest` binary and end-to-end integration tests
scripts/
  install-deps.sh       installs or checks dependencies (Bash 3.2+)
  check-arch.sh         hexagonal architecture rules
  package.sh            builds the release tarball (crest.app on macOS)
  package.ps1           builds the Windows release zip
  install.sh            installs crest as an app from a release or a tarball (POSIX sh)
  install.ps1           installs crest on Windows from a release or a zip
  screenshots.sh        takes the README's screenshots from the UI examples
  icons.sh              renders the app icons from assets/logo.svg (and crest.ico)
assets/                 logo, icons, .desktop entry, crest-launcher.sh,
                        macos/Info.plist.in, windows/ (crest.ico, crest.rc) and
                        screenshots/
examples/plugins/       showcase: a demo Lua plugin with every extension point
tests/
  arch/                 fixtures and tests for check-arch.sh
  install-deps/         tests for install-deps.sh
  install/              tests for install.sh and the app layout of package.sh
docs/                   user and developer guides
.github/workflows/      ci.yml, live.yml, release.yml
```

### Makefile targets

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
| `make test-install` | Tests for `install.sh` and the app layout of `package.sh`. |
| `make check` | `fmt-check`, `lint`, `test`, `arch`, `test-arch`, `test-deps` and `test-install`. |
| `make ci` | `doctor` + `check`. |
| `make package` | Release tarball in `dist/`. |
| `make install` / `make uninstall` | Packages the release build and installs it with `scripts/install.sh`: `$(PREFIX)/bin/crest` plus the app menu entry on Linux, `~/Applications/crest.app` on macOS (`PREFIX=~/.local`). |
| `make clean` | `cargo clean`. |
| `make sweep` | `cargo sweep --time 14` (`SWEEP_DAYS`): deletes the artifacts no build has used in two weeks. Needs `cargo install cargo-sweep`. |

### Build times and disk

GPUI makes `target/` big and links slow, so the workspace trims both:

- `[profile.dev]` in the root `Cargo.toml`: line tables only for the workspace crates (file and line in backtraces), no debug info and `opt-level = 1` for the dependencies, and optimized build scripts and proc macros.
- The fakes are a crate of their own (`application-testing`), not a feature, so `cargo build` and `cargo test` build `application` and the crates above it once. `gpui`'s `test-support` feature cannot get the same treatment: it would put test code in the release binary, so `gpui`, `ui` and `crest` are still built twice (once with it, for tests).
- One integration test binary per crate (`tests/it/`), no doctest pass in `ui`, `crest` and `adapter-formats`, and the UI examples behind `--features examples`.
- The linker is already LLD: Rust's `x86_64-unknown-linux-gnu` target links with its bundled `rust-lld` (Arch's `rust` package included), so there is no `.cargo/config.toml`.

`make sweep` deletes the artifacts unused for two weeks. If your `rustc` comes from the distribution (Arch's `rust` package rather than rustup), every package update changes the compiler and makes all of `target/` stale: run `cargo sweep --installed` after it, which keeps only what the installed toolchain built.

### Screenshots

The README's screenshots (`assets/screenshots/`) come from the UI examples, which run on scripted fakes (no API key). The `showcase` example opens the real `Workbench` (the chats sidebar beside the session) and has one scene per screenshot: `hero`, `sidebar` (the full sidebar with projects, pinned and branched chats, Wild Chats and Archived), `tour` (the welcome tour on its second step), `menu` (the menu of the prompt's `+` button), `info` (the information mark with its kept notices), `picker` (the grep picker with its preview), `permissions`, `research`, `approval`, `help`, `model`, `modal`, `blocks`, `routing` and `technical` (a statistics card, a trace waterfall and a form); `sysboard` draws the simulation board. `scripts/screenshots.sh` builds them, opens each window, waits for the example's `ready` line (it carries the viewport size and the scale factor), captures the window, drops the title bar, applies the scene's crop and writes PNGs up to 1800 px wide:

```bash
scripts/screenshots.sh              # every scene
scripts/screenshots.sh hero help    # some of them
```

On Linux it captures with ImageMagick's `import` on the X display (through XWayland under a Wayland session) and needs ImageMagick and `xprop`. On macOS it captures with `screencapture -l` and the window id that `scripts/macos/window-id.swift` finds from the example's pid; it needs ImageMagick and `swiftc` (the Xcode command line tools), and the first run asks for the Screen Recording permission for your terminal (a blank capture means it was just granted: run again). Install JetBrains Mono first, or the text measures differently from the other screenshots. The windows show up on your screen for a few seconds. Run it again when the UI changes.

## Before every commit

```bash
make check   # fmt-check + lint + test + arch + test-arch + test-deps
```

| Target | What it runs |
| --- | --- |
| `make fmt` / `make fmt-check` | `cargo fmt --all` |
| `make lint` | `cargo clippy --workspace --all-targets -- -D warnings`, then the optional features on their own: `-p crest --features langfuse` and `-p ui --examples --features examples`. Without `--all-features` clippy builds the same feature set as `cargo build` and `cargo test`. |
| `make test` | `cargo nextest run --workspace` plus the doctests, if nextest is installed. Otherwise `cargo test --workspace`. |
| `make test-live` | The `#[ignore]` tests whose name contains `live`, with `ANTHROPIC_LIVE_TEST=1`. Needs `ANTHROPIC_API_KEY` in the environment. |
| `make arch` / `make test-arch` | `scripts/check-arch.sh` and its fixtures. |
| `make test-deps` | The tests of `scripts/install-deps.sh` (helpers and a macOS simulation). |
| `make test-install` | `tests/install/run.sh`: packages a fake binary for Linux and macOS and installs, launches and uninstalls it in a temporary prefix. |

Code rules, enforced by the workspace lints:

- `unsafe` is forbidden.
- `unwrap()` and `expect()` are denied outside tests.
- Errors are typed with `thiserror` in the libraries; `anyhow` is only used in the binary.
- The API key never appears in logs, tests or files. `crates/adapters/anthropic/tests/no_key_in_logs.rs` checks this.

Tests use no network, no `rg` and no window, except where they are adapter integration tests:

- Use cases are tested with the fakes of the `application-testing` crate (`application::testing` inside `application`'s own unit tests).
- The Anthropic API is tested with `wiremock`.
- GPUI tests use `gpui`'s `TestAppContext` (`test-support` feature), which opens no windows.
- Snapshots of the tools' JSON schemas are stored with `insta` (`crates/app/src/tools/snapshots/`, and `crates/app-core/src/tools/snapshots/` for the tools that live in `application-core`). If you change a schema, review the snapshot with `cargo insta review`.

## Fakes in `application-testing`

The fakes live in `crates/app-testing` (package `application-testing`). Another crate uses them as a dev-dependency only, `application-testing.workspace = true` under `[dev-dependencies]`, and imports them as `application_testing::llm::ScriptedLlm`; `scripts/check-arch.sh` rejects it anywhere else. `application-core` and `application` each mount the same files as their `testing` module for their own unit tests (`crate::testing::llm::…`), so the fakes' own tests run in both. They are not a feature of the application crates: a feature turned on by dev-dependencies would build them and every crate above them twice, once for `cargo build` and once for `cargo test`.

| Module | Fakes |
| --- | --- |
| `testing::llm` | `ScriptedLlm` (replies with a script of `ScriptedResponse`: text, tool uses, errors), `FakeModelCatalog`, `FakeMcp`. |
| `testing::fs` | `InMemoryFs` (in-memory `FileSystemPort`, `FileSearchPort` and `ContentSearchPort`, with `with_file`), `ScriptedShell`. |
| `testing::system` | `RecordingSink` (records the `DomainEvent`s), `ScriptedApproval` (scripted answers), `InMemorySessionStore`, `FakePluginHost`, `RecordingTelemetry`. |

The integration tests in `crates/bin/tests/it/` bring up the real composition root with a scripted model through `root::Overrides`, which lets you replace the LLM, the catalog, the config paths, the database and the MCP trust file:

- `headless_demo.rs`
- `resume.rs`
- `mcp_servers.rs`
- `ui_session.rs`
- `research_demo.rs`
- `implement_demo.rs`

The integration tests of `crest`, `adapter-formats` and `adapter-lua` are modules of a single test binary per crate (`tests/it/main.rs`): every test binary links the whole dependency tree, so one binary instead of a dozen saves most of the link time of `cargo test`. A new test file goes into `tests/it/` with its `mod` line in `main.rs`; run one file with `cargo test -p crest --test it resume::`.

For the UI there are runnable examples in `crates/ui/examples/`, for example `cargo run -p ui --features examples --example chart_blocks`. They sit behind the `examples` feature so that `cargo test` does not build them; `make lint` checks them.

## Adding a tool

1. Write a type that implements `TypedTool` (`crates/app-core/src/tools/registry.rs`). The input is a `struct` with `Deserialize + JsonSchema`, and the schema is derived with `schemars`. If you need full control over the schema, implement `Tool` directly.
   - `name`: must match `^[a-zA-Z0-9_-]{1,128}$` and be unique in the registry.
   - `description`: written for the model; say when to use it and when not to.
   - `effect(&input)`: returns `ToolEffect::{Read, Write, Execute, Network}`. This is what the permission policy decides on.
   - `prepare(input, ctx)`: validates and returns a `PreparedCall::new(subject, future)`. `subject` is what is shown and what an "always" rule covers (a path, a command line). Optionally add `.with_preview(ApprovalPreview::…)` and an extra block (like the `Diff` of `edit_file`). The future does the work and returns a `ToolOutput`.
2. If the tool needs I/O, ask for a port, not an adapter. If no suitable one exists, extend or create one in `crates/app-core/src/ports/`, with its fake in `testing`.
3. Register it in `register_tools` (`crates/bin/src/root/tool_registry.rs`, called by `build_app`) with `tools.register_typed(...)` or `tools.register(Arc::new(...))`. File tools are registered in `register_file_tools` and component tools in `register_component_tools`.
4. Add an `insta` snapshot of its definition, as in `tools/files.rs` and `tools/components.rs`, and tests with the fakes.

If a tool should show something rich in the timeline, return `ToolOutput::text(confirmation).with_block_update(BlockKind::…)`. The model only gets the confirmation, as with `show_chart` and friends.

## Adding a `/` command

Commands are read and routed in `application_core::usecases::commands` (`crates/app-core/src/usecases/commands.rs`); the window and the headless loops share that parse.

1. Add a `Command` variant and its arm in `Command::from_line`, with its aliases. If its arguments have forms, give it a typed sub-command with a `parse` (as `ImplementCommand`, `McpCommand`), and let the parse, not the handler, pick the usage text of a bad form.
2. Add its row (or one row per form) to `CATALOG` in `commands/catalog.rs`, with `Handler::Ui` or `Handler::Controller`: the help's Commands section and the unknown-command hint come from there, and a test checks that every name and alias parses to a command of that handler.
3. A session command: handle the variant in `Session::command_at` (`crates/ui/src/session/commands.rs`) and return `None` for it in `dispatch`. A controller command: add a `CommandHandler` method, its arm in `dispatch`, and implement it on `UiController` (`crates/bin/src/gui.rs`), usually through `spawn_command` so `Ctrl+C` stops it.
4. Test the parse and the routing in `commands/tests.rs` (the fake handler records what it is asked to run); test what the command does next to its implementation.

Commands of `init.lua` need none of this: they arrive as `Command::Other` and `CommandHandler::callback` runs them.

## Adding a block type

A rich block is its spec, one line in `block_kinds!`, a view module and one line in `blocks::VIEWS`; no `match` over the kinds needs editing.

1. **Domain**: write the spec type (serializable, `JsonSchema`) in its module of `crates/domain/src/` and `impl BlockSpec for YourSpec` (`crates/domain/src/block.rs`): `validate` for its invariants (forward to an inherent `validate` if callers use one) and `summary_line`, the one dim line the headless terminal prints for it (`[gantt: 4 tasks, 12 d, critical a → b]`). Both default to nothing. Then add the variant to the `block_kinds!` list in `block.rs`, with its doc comment (it is the JSON schema's description): `YourKind(Box<YourSpec>)`. The macro derives `BlockKind`, `BlockTag::YourKind` with its serde name (`your_kind`), `BlockKind::tag` and `BlockKind::spec`, through which `Block::validate` and `BlockKind::summary_line` reach the spec. Review and accept the `block_schema` snapshot. Inline struct variants (`Text { .. }`, `Shell { .. }`) are for the core chat kinds: their validation is the short `validate_inline` match and the headless sink prints them itself.
2. **View**: create `crates/ui/src/blocks/<type>.rs` with a `pub fn render(entry, window, cx) -> AnyElement` and a unit `pub struct View` that implements `BlockView` (`blocks/mod.rs`): `tag` and `render` are required; override `width` with `BlockWidth::Prose` (~760 px) for text-like blocks (the default, `Wide`, is ~1100 px for charts, tables and diagrams), `copy_formats` for the forms the copy row offers (`copy::CopyFormat`, helpers such as `copy::csv` in `blocks/copy.rs`; list them in that module's table), `has_alt_view` if `t` should toggle, `expanded_by_default` and `own_copy_buttons`. Add `&<type>::View` to `blocks::VIEWS`; the test `every_kind_has_one_view_or_is_drawn_by_the_timeline` fails until you do. If you need per-block GPUI state (scroll, zoom, sort order), use `blocks::block_state` as `table` and `mermaid` do; markdown or code inside a block goes through `blocks::math::markdown_block` / `code_block`, which keep the parsed document the same way (a `TextView` built from text alone loses it, and its highlights and heights, whenever the block scrolls out of view). If the block's height depends on its width (a chart, a diagram), wrap it in `blocks::measured::measured` with a closure that returns the height for a width instead of fixing a height.
3. **Timeline**: `copy_text` in `crates/ui/src/timeline/render.rs` is what `y` copies; rich kinds fall back to their JSON.
4. If the model should be able to create it, add a component tool in `crates/app/src/tools/components.rs` (or a formats tool in `tools/formats/`) and mention it in its prompt.

## Adding an adapter

1. Create the crate in `crates/adapters/<name>/`. Its internal dependencies can only be `application` and `domain` (`make arch` checks this).
2. Add it to `members` and to `[workspace.dependencies]` in the root `Cargo.toml`. Pin the versions of external crates in `[workspace.dependencies]`.
3. Implement the port's trait with the port's typed errors. Async methods must not block the runtime: use `spawn_blocking` for synchronous libraries, as SQLite and the keyring do.
4. Build it in `crates/bin/src/root.rs`. If the adapter is missing or fails, the composition root should degrade to a notice, unless no work is possible without it.
5. Write the adapter's own integration tests (`tempfile`, `wiremock`, real processes) and, where it applies, an `#[ignore]` test with `live` in its name.

## Adding or changing a workflow node

**Research workflow** (`crates/app/src/flows/research/`):

- The stages and their metadata live in `domain::research::Stage`: order (`Stage::ALL`), name (`name_es`), role, whether it has a gate, and phase. The optional nodes are in `OPTIONAL_STAGES` (`flows/research/node.rs`).
- Each node implements `ResearchNode` (`node.rs`):

  ```rust
  #[async_trait]
  pub trait ResearchNode: Send + Sync {
      fn stage(&self) -> Stage;
      fn has_gate(&self) -> bool { self.stage().has_gate() }
      async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError>;
  }
  ```

  `run` returns `NodeOutcome::Done`, `NeedsInput { prompt }` or `Back { to, reason }` (only to an earlier stage). Use `ctx` for the typed state (evidence, hypotheses, constraints, decisions, journal), the per-node data, `user_reply` (the answer to the gate), the models by role, the events and the agents (`agent` to act, `explore_agent` for read-only work).
- The node's file goes in `flows/research/nodes/<file>.rs` and is registered in `nodes::node_for(stage)`. The graph is assembled in `graph.rs::research_graph`, following `Stage::ALL`.
- Each node has tests with `ScriptedLlm` that check what it leaves in the context. See `nodes/*/tests.rs` and `flows/research/tests.rs`.

**Implementation workflow** (`crates/app/src/flows/implement/`): the steps implement `ImplementNode` (`node.rs`), live in `nodes/` and are assembled in `graph.rs`. Gates are interpreted by `gate.rs::parse_choice`. If you add a step, also update `state.rs` (steps and task states) and `report.rs`.

## Commit conventions

crest is licensed under GPL-3.0-or-later (see [LICENSE](../LICENSE)); contributions are accepted under the same license.

We use [Conventional Commits](https://www.conventionalcommits.org/) with the crate as the scope (`domain`, `application`, `search`, `ui`, `crest`, ...; `docs`, `ci` or `build` for what is not code). Messages are in the imperative:

```text
feat(search): parse rg --json match events

Turn each "match" event into a GrepHit with path, line and
column range. "context" events are attached to the previous hit.
```

- Types: `feat`, `fix`, `test`, `refactor`, `docs`, `build`, `ci` and `chore`.
- One purpose per commit. If the message needs an "and", it is two commits.
- Every commit builds and passes the tests of the crate it touches, so that `git bisect` works.
- The test goes in the same commit as the behavior it tests. Refactors, formatting and new dependencies go in commits of their own.
- Aim to keep each commit under about 150 changed lines, not counting snapshots.
- Branches are integrated with rebase, without squashing.

## CI

`.github/workflows/`:

| Workflow | When | What it does |
| --- | --- | --- |
| `ci.yml` | `push` and `pull_request` to `main` | `make ci` (`doctor` + `check`) on `ubuntu-24.04` and `macos-15`, split into a lint job (`fmt-check`, `lint`, `arch`, `test-arch`) and a test job that run in parallel, with a Rust cache per job and OS saved only from `main`; clippy and `cargo test` on `windows-2025`; `install-deps.sh --check` in containers (Ubuntu 24.04 and 22.04, Fedora, Arch). |
| `live.yml` | Manual (`workflow_dispatch`) | `make test-live` with the `ANTHROPIC_API_KEY` secret. |
| `release.yml` | `v*` tags (or manual) | Builds and packages (`scripts/package.sh`) for `x86_64`/`aarch64-unknown-linux-gnu` (on ubuntu-22.04 and 22.04-arm, because of glibc) and for `aarch64`/`x86_64-apple-darwin` (signed and notarized by `scripts/sign-macos.sh`), and a zip (`scripts/package.ps1`) for `x86_64-pc-windows-msvc` (signed by SignPath) whose `install.ps1` is tested on the runner. With a tag, it publishes a GitHub Release with checksums, then the Homebrew cask (`scripts/publish-homebrew.sh`) and the winget update (Komac). Each signing or publishing step is skipped without its secret (see the workflow's header). |

Actions are pinned by commit SHA.
