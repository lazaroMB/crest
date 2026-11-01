# Audit: performance, build and architecture

Status: **done, on branch `perf/audit`**, October 2026. An audit of crest in five areas (rendering, modals, heavy work and what can run in parallel, the build, and architecture and reuse), followed by its implementation in phases 1 to 4b and this report (phase 5). The branch starts at `main` e004b8a8; file and line references in the findings point at that commit unless a later path is given.

## Scope and method

- Three read-only passes over the code: `crates/ui`; `crates/app`, `crates/bin` and the adapters; and the build setup (`Cargo.toml`, `Makefile`, `.github/workflows/ci.yml`, `scripts/`).
- The scope chosen was performance, build and medium-cost refactors. The large architectural moves are described at the end as a future phase and are not implemented.
- One worker per phase, small thematic commits, targeted `cargo test -p <crate>` / `cargo clippy -p <crate>` runs under a memory cap (a workspace-wide test build ran the 30 GB machine out of memory once), and a single `make check` at the end of the branch.
- **No measurements were taken.** After the first attempt at a cold-build baseline was killed by the OOM and the second was stopped, the user decided to go ahead without timings. The only numbers in this note are the ones in [Phase 2b](#phase-2b-sessions-catalogs-and-cpu-work) (the `RegexSet` experiment and a debug-build compute test). [How to measure](#how-to-measure) lists the commands to get before/after figures.

## Findings

### Build and disk

- Every workspace crate and every dependency was built with full debug info, and dependencies at `opt-level = 0` (root `Cargo.toml`).
- `crates/ui/Cargo.toml:57` and `crates/bin/Cargo.toml:54` enabled `application/testing` in their dev-dependencies. With resolver 3 that built `application`, every adapter, `ui` and `bin` twice: once for `cargo build`, once with the feature for tests.
- Integration tests were one binary per file in `crates/bin/tests`, `adapters/formats/tests` and `adapters/lua/tests`: one GPUI-sized link per file.
- `ui`, `bin` and `formats` ran a doctest pass with no Rust doctests in them, and the 15 `ui/examples` were built by every `cargo test`.
- `make lint` and CI ran clippy with `--all-features`, which pulls Langfuse into every pass. CI had lint and tests in one job and saved the rust cache from every branch.
- `target/` grows without bound. The developer's `rustc` comes from Arch, not rustup, so every package update makes the whole of `target/` stale.

### Concurrency and heavy work

- `run_tools` (`crates/app/src/usecases/run_turn.rs:920`) ran every tool call of a turn one after another, including independent reads and `task` subagents.
- Startup (`crates/bin/src/root.rs:549-805`) ran its independent steps in sequence: tool checks (`rg`, then `fzf`, `adapters/search/src/lib.rs:111`), model ports and their validation, web tools, the decision model, the store, MCP; and read the keyring three times (`root.rs:1070`, `startup.rs:277,331`).
- Every request rebuilt the JSON schema of every tool (`crates/app/src/tools/registry.rs:365,482-542`) and `Typed::effect` cloned the input (`:371`); `context_estimate` serialized them again (`run_turn.rs:1172`).
- Every model call cloned the whole history (`run_turn.rs:1041`), and `adapters/anthropic/src/convert.rs:40-74` cloned it again, base64 images included.
- `before_tool`/`after_tool` hooks made a round trip to the Lua thread even with no hook registered (`run_turn.rs:1500-1533`).
- Saving a session deleted and reinserted every timeline block and deep-cloned the session twice (`adapters/store/src/session.rs:95,124-189`, `bin/src/root.rs:212-235,419-428`).
- The model catalogs were not cached: the OpenCode Zen catalog was listed twice in `bin/src/opencode.rs:153`, again in `startup.rs:228` and again in `gui.rs:1528`.
- CPU-bound work ran on one core: bootstrap, jackknife, Monte Carlo and tolerance replicates (`adapters/compute/src/bootstrap.rs:140-181,371`, `simulate.rs:240`, `tolerance.rs:144`), STEP shells (`adapters/formats/src/step.rs:175-206`) and waveform signals (`waveform.rs:111,155`).

### Rendering and modals

- `RootView::render` (`crates/ui/src/app.rs:361-405`) redrew the whole window on every notify, and the 100 ms activity spinner (`activity.rs:236`) notified ten times a second while a turn ran.
- About 28 block `sync()` functions (`blocks/logs.rs:71`, `table.rs:253`, `flame.rs:168`, …) compared the whole spec every frame; custom nodes (`custom.rs:156-200`) cloned theirs.
- `column_align` scanned every row for every table cell (`blocks/table.rs:266`); the "copy as" row built every format's text on every frame (`timeline/view.rs:756`, `blocks/copy.rs:489-539`); images, molecules and reactions cloned and rehashed their bytes each frame (`image.rs:162-176`, `svg::key`); charts planned their layout several times per frame (`chart/plot.rs:84,239`, `plot/mod.rs:309`); shell ANSI runs, tool-call pretty JSON and diff `line_stats` were rebuilt per frame (`shell.rs:103-159`, `timeline/render.rs:254`).
- The 3D renderer created its GPU device and read every frame back synchronously on the UI thread, and built section meshes there too (`render3d/gpu.rs:182-190,736-746`, `model_view.rs:187-205`).
- Mermaid layout and math typesetting ran on the UI thread; the math cache was cleared whole when full (`math.rs:149-177`).
- The file tree watcher watched `target/` and reloaded the panel even when hidden (`tree.rs:101-140,357`, `session.rs:727`).
- The picker drew every result row (`picker/mod.rs:1339`); the project picker, the input's text and its palette were recomputed per frame (`project_picker.rs:202-218`, `input.rs:522-614,1098,1279`); every stacked modal was drawn (`modal.rs:514`); pasted images were converted (BMP to PNG, base64) on the UI thread (`input/attachments.rs:65`); `CustomNode::Html` reparsed every frame (`custom.rs:172`).
- Each streamed text delta rescanned and renormalized the whole reply (`timeline/state.rs:61,371`, `math.rs:262`).

### Architecture and reuse

- The SSE loop, error mapping, HTTP client builder and retry clock were duplicated between `adapters/anthropic/src/stream.rs:338-379` and `openai/src/sse.rs:39-85`; Anthropic lacked the context-too-long and invalid-key detection OpenAI had. `KEYRING_SERVICE` was defined five times.
- `input/model_popup.rs`, `theme_popup.rs` and `path_popup.rs` repeated the same keys, rows and frame.
- Very large files: `ui/src/session.rs` (4731 lines), `picker/mod.rs`, `input.rs`, `approval.rs`, and `build_app` in `bin/src/root.rs` (about 500 lines). The spawn + oneshot + "the turn ended without an answer" pattern was repeated in `gui.rs:1279-1300` and `resume_pending`.
- The 27 functions of the Lua formats bridge (`adapters/lua/src/api/formats.rs`) re-implement argument parsing that the matching tools also do.

## What was already fine

Left alone: the virtualized timeline; the picker's background search; SQLite in WAL mode behind `spawn_blocking`; the formats tools marked `offloaded`; Lua on its own thread; regexes in `LazyLock`; the `join_all` over catalog providers; the per-spec validation in `domain/src/block.rs` (plan item 4.5: `Block::validate` already only dispatches to each spec's `validate`).

## What was done

### Phase 1: build and disk

- 190d198c build: lighter debug info and optimized dependencies in dev builds
- 8a97cc55 refactor(app): move the port fakes to an application-testing dev crate
- 8ec6caa5 test: one integration test binary per crate in crest, adapter-formats and adapter-lua
- e8d01acc build: no doctest pass for ui, crest and adapter-formats
- 6f886dde build(ui): put the examples behind an `examples` feature
- e3f96349 build: lint with the default features, split CI lint and tests, add make sweep
- bcc3a810 test(lua): allow unwrap in the shared helper of the merged test binary

`[profile.dev]` uses `debug = "line-tables-only"`, dependencies get no debug info and `opt-level = 1`, build scripts `opt-level = 3`. The fakes moved to `crates/app-testing` (package `application-testing`, its own layer in `scripts/check-arch.sh`, dev-dependency only); `application`'s own unit tests mount the same files with `#[path]`. Integration tests are `tests/it/main.rs` per crate. The examples need `--features examples`. CI has separate `lint` and test jobs, with `shared-key` per OS and `save-if: main`. `make sweep` runs `cargo sweep`; [development.md](../development.md#build-times-and-disk) explains the Arch `rustc` and `cargo sweep --installed`.

### Phase 2a: tools in parallel, startup, schemas, history, hooks

- 0bad92b3 perf(app): skip the plugin round trip for tool hooks that are not registered
- 88a6487e perf(app): build the tool definitions once and share them between requests
- 13b752a3 perf(app): copy each history message once per turn instead of once per model call
- e5fc3605 perf(app): run consecutive read-only tool calls of a turn concurrently
- 4f5c09ef perf(bin): run the independent startup steps concurrently and read the keyring once
- 3e1d08f6 style: clippy fixes for the shared tool definitions and the gated tool calls

Tool calls are gated in order (hooks, `prepare`, policy); a run of consecutive calls that the policy allows and that only read or fetch runs with `for_each_concurrent(MAX_PARALLEL_TOOLS = 8)`. Writes, commands, calls that ask the user and `ask_user` are barriers that run alone. Results go back in call order; cancellation answers every call. Startup joins the model ports and their validation, web tools, MCP and the store; the keyring is read once (`startup::SecretsWanted::read`). Tool schemas are cached per type, `ToolRegistry::definitions()` returns a shared `Arc<[ToolDefinition]>` invalidated by `ToolSource::revision()` (MCP refreshes), `LlmRequest.messages` is `Vec<Arc<Message>>`, and `PluginHostPort::has_hooks(kind)` skips hook calls when none is registered.

### Phase 2b: sessions, catalogs and CPU work

- 39b23881 perf(store): save only the timeline blocks that changed and move the session into the save
- c7efdd01 perf(app): keep each provider's model list for five minutes and share it with the OpenCode probe
- 84d6bd31 perf(compute): compute bootstrap, jackknife, Monte Carlo and tolerance replicates on rayon with the same seeded draws
- 919205af perf(formats): mesh STEP shells and load waveform signals in parallel
- 077e6dcd style: rustfmt the session save and catalog cache tests

`SessionStorePort::save` takes the session by value; blocks are upserted by position with `ON CONFLICT … WHERE data_json IS NOT excluded.data_json`, so an unchanged row is not rewritten. `CachedCatalog` keeps each provider's list for 5 minutes (concurrent callers share one request; `/login` and `/logout` invalidate it). The compute replicates draw from the single ChaCha stream in sequence and compute statistics on rayon: results were checked bit for bit against the old code (both seeds, every statistic and distribution) and permanent tests compare them. In debug, the compute fingerprint test went from 3.7 s to 2.8 s; the sequential draw dominates for cheap statistics.

### Phase 3a: render caches

- 5a0d339d perf(ui): sync block state only on new revisions, align table columns once and plan charts once per width
- 83cd45c2 perf(ui): draw the copy-as row from cached labels and build the text on click
- 767bb2bd perf(ui): keep image and depiction data hashed in block state instead of cloning it per frame
- dd620bb5 perf(ui): make the text of shell, tool call, thinking and diff blocks once per revision
- 81b498b2 perf(ui): cache the timeline, the side panel and the modal host so the spinner does not redraw them
- dc398a5d style(ui): allow the image node view its eight arguments

The timeline, side panel and modal host are `AnyView::cached`: the spinner no longer redraws them. A cached view repaints when it or a child view notifies, its bounds or text style change, or the window refreshes; every block state that notifies is a child view, which was audited. `TimelineEntry.rev` (a global counter, renewed on upsert, edit, resolve and lint) gates block `sync()` through `blocks::synced_state` and derived text through `blocks::derived`. Tables keep their column alignment, charts keep their layout per width (`measured::LayoutMemo`), images keep `svg::SvgData` (bytes plus hash).

### Phase 3b: background work in the UI

- 40d3dbba perf(ui): handle streamed text deltas without rescanning or renormalizing the whole reply
- feb8bdb4 perf(ui): typeset math in the background with a byte-bounded cache keyed by hash
- d74a795e perf(ui): lay mermaid diagrams out in the background and keep the old one until the new lands
- f6f24c94 perf(ui): start the tree watcher in the background, skip gitignored paths and reload a hidden panel only when shown
- b0a2b317 perf(ui): virtualize picker results, read the prompt text, palette and project rows once per change, prepare pastes in the background
- 0dcaa15d perf(ui): keep HTML node parses across frames and skip modals hidden under another
- 240cf487 perf(ui): open the 3D device in the background, read frames back without blocking and cut model sections off the UI thread
- 02d4f73d style(ui): name the model viewer's colour pair and report watcher failures in the panels example

The 3D device opens in the background (`Backend::Pending` draws as vectors meanwhile); frames are submitted with `map_async` and polled on the next frame, showing the previous one; section meshes are built off-thread. Mermaid and math lay out in the background with a placeholder (math in a 32 MiB `svg::Lru`). The tree watcher reads the root `.gitignore` and `.git/info/exclude` (crate `ignore`), watches top-level directories that are not ignored, and a hidden panel only marks itself stale. Picker results are a `uniform_list`. A card fully covered by a modal above is not drawn. Pasted images are prepared in the background and land in paste order. Streaming appends skip the rescan and the math normalization when the new text cannot hold math. A bug found on the way: `theme::default_theme()` built a new `Arc` per call, so without a global theme the mermaid relayout looped; it is now a `LazyLock`.

### Phase 4a: paste fix, adapter support, completion popups

- 7ad1eedd fix(ui): send a prompt typed during an image paste together with the pasted images
- 7c3ff806 refactor(adapters): name the keyring service once, in platform
- ceb7a057 refactor(adapters): share the SSE loop, HTTP client and status mapping of the model adapters in adapter-support
- cea90f48 refactor(ui): one completion popup for the @ paths, /model and /theme lists, sharing keys, rows and frame with the palette

`crates/adapters/support` (`adapter-support`, its own layer in `check-arch.sh`) holds `sse::{Decoder, llm_events}`, the reqwest client, `network_error`, `retry_after_ms`, the shared 4xx/5xx mapping and `TokioClock`. `KEYRING_SERVICE` lives in `platform`. `input/completion.rs` has one `Popup<T>` for the three lists; the net saving was about 30 lines once docs and tests are counted (about 300 lines of duplication removed).

### Phase 4b: splitting large files

- ed37cea0, e0964468, b5a8ba01 split `ui/src/session.rs` into `session/{mod,keys,queue,commands,login,model,theme,overlays,tests}.rs`
- 61e211a3, c2aa917c, cc9d1034 split `picker/mod.rs` into `items`, `search`, `render` and `tests`
- 2bef183f, 28e9e18f split `input.rs` into `input/{keys,palette,popups,paste,render,tests}.rs`
- 722df91b, 23852504 split `approval.rs` into `approval/{inbox,render,tests}.rs`
- 2d89b1bb, 600e5434, efaaf618 move `model_ports`, the tool registry, the stores and the resumed session out of `build_app` (`root/{llm_ports,tool_registry,stores}.rs`; `build_app` went from about 556 to 436 lines)
- 751fc9c1 refactor(bin): one helper runs a UI turn on the runtime and answers on a channel (`UiController::spawn_turn`)
- aa95c6c1 test(bin): toggle the image mode once the pasted image is in the tray; 442b3234 style(bin)

Moves only, no logic changes; public paths are kept by re-exports.

### Phase 5: this report

- dbd33488 fix(ui): apply an image mode switch pressed during a paste once the images land

Since phase 3b, `<leader>i` pressed while a paste was still being prepared found an empty tray and was silently ignored. The switch is now a step of the paste chain (`PasteStep::ToggleMode` in `crates/ui/src/input/paste.rs`), like a prompt sent during a paste: it applies to the tray once the images before it land, and switches, prompts and pastes keep the order they were made in. Test: `input::tests::a_mode_switch_during_a_paste_applies_once_the_images_land`; the workaround in `ui_session::chat_only_images_are_saved_without_a_model_call` is gone.

## Skipped, and why

| Item | Why |
| --- | --- |
| `clang` + `-fuse-ld=lld` in `.cargo/config.toml` | Already the case: `x86_64-unknown-linux-gnu` links with the bundled `rust-lld` (Arch's 1.99 and rustup stable since 1.90; the binary's `.comment` says "Linker: LLD 23.1.1"). A config would add a CI dependency for nothing. |
| `split-debuginfo = "unpacked"` | windows-msvc only accepts `packed` and profiles cannot be set per target; with line tables only the gain is small. |
| One build of `gpui` with `test-support` | The feature would end up in the release binary, so `gpui`, `ui` and `crest` still build twice (documented in development.md). |
| `RegexSet` pre-check in `Redactor::redact` (`app/src/privacy.rs:173`) | Measured in release with identical output: about 7× slower (84 ms vs 11 ms on 274 KB of code, 401 vs 55 ms on 1.3 MB of Spanish text, 489 vs 67 ms mixed). The union of rules with Unicode `\b` loses each regex's literal prefilter (`sk-ant-`, `AKIA`, …) and runs on the PikeVM. Reverted. |
| Borrowing wire types in the anthropic/openai adapters | `WireContentBlock` is also deserialized; borrowing needs lifetimes through it. The history is still copied once per turn (the session would have to hold `Arc<Message>`). |
| `uniform_list` for long text blocks | Needs a fixed height and a nested scroll inside the timeline's measured `list`, a UX change. GPUI caches line shaping, so the cost is layout only. |
| Direct pixmap for math | `ratex-render` 0.1.15 only exposes `render_to_png`; the PNG decode stays, now off the UI thread. |
| Drawing only the top modal | The design keeps lower modals visible around a smaller one; only fully covered cards are skipped. |
| Caching the input and status bar views | `AnyView::cached` needs the layout style up front and their height depends on their content. The picker, login and approval dialogs are not cached either (same inset problem; closed, they draw an empty `div`). |
| Lua formats bridge through the tool inputs (plan 4.4) | The tool inputs have `deny_unknown_fields`, other defaults, trimming and messages (`` `path` is required ``, "counts from 1", …) that the Lua tests check; deserializing them would change behaviour while saving 1 to 5 lines per function. The real saving (about 900 lines) needs a shared builder per tool: see the future phase. |
| Per-spec validation (plan 4.5) | Already done. |
| `sessions.data_json` rewrite on every save | Not in the plan; the whole history row is still rewritten. |

## Behaviour users may notice

- **Read-only tool calls run in parallel.** Consecutive reads, searches, web fetches and `task` subagents that the policy allows run together (at most 8). In the timeline every call of such a run shows its input first, then their `Running` and final statuses interleave; `after_tool` hooks interleave too. The results reach the model in call order.
- **Parallel `task` subagents cannot raise approval prompts**, concurrent or not (see [Approvals and parallel subagents](#approvals-and-parallel-subagents)).
- **Anthropic error mapping.** Through the shared mapping, a 403 that names the key is an authentication error, and context-too-long is recognized on 400/404/413/422 also when the body does not parse (for example OpenCode's `/anthropic` endpoint). "invalid x-api-key" and "prompt is too long" are recognized by both adapters. An OpenAI non-2xx below 400 is now `InvalidRequest` instead of `Server`.
- **Missing API key before MCP.** Without a key, crest fails before starting MCP servers, so no server is launched and no login page opens.
- **Model lists are cached for 5 minutes**; `/login` and `/logout` refresh them. The running catalog still uses the key it started with (as before).
- **Paste and submit.** Pasted images land a moment after the paste. `Enter` pressed meanwhile takes the prompt out of the editor at once and sends it with the images when they land; `<leader>i` pressed meanwhile applies once they land. A paste made after `Enter` waits for the next prompt. There is no "preparing" indicator.
- **Placeholders.** Mermaid diagrams show "Laying out the diagram…" and math shows its source dimmed until the background work lands; a re-laid-out diagram keeps the old one meanwhile. A 3D view shows the vector drawing (or the board background with labels) until the first GPU frame.
- **File tree.** Changes under ignored paths (`target/`, anything in the root `.gitignore`) no longer reload the panel; nested `.gitignore` files and the global excludes are not read, so changes there only cost a reload. A hidden panel reloads when shown.

## Approvals and parallel subagents

Question: now that `task` calls run concurrently, can several subagents put approval dialogs on screen at once? **No**, for three independent reasons:

1. **The subagent never reaches `Ask`.** Its policy is `PermissionPolicy::read_only()` (`crates/app/src/tools/task.rs`, `subagent()`). `PermissionPolicy::decide` (`crates/domain/src/permission.rs`) denies every non-read with `DenyReason::ReadOnly` before looking at rules, and `mode_decision` allows `Read` in every mode; only a `Deny` rule can change that, and it denies. Its registry only holds read tools (the web tools declare `ToolEffect::Read` too).
2. **Even if it did, nothing would reach the user.** Its `ApprovalPort` is `DenyAll`, which answers `ApprovalError::Unavailable`; `ApprovalGate::ask` turns that into a rejection. Its events go to `DiscardEvents`, so no `ApprovalRequested` reaches the UI either.
3. **In the parent turn, calls that ask run alone.** `GatedCall::runs_alongside` (`crates/app/src/usecases/run_turn.rs`) is false for `GateStep::Ask`, so such a call ends the parallel run and waits for its own approval; approvals of one turn come one at a time, as before.

Approvals from elsewhere running at the same time as a turn (a workflow, a second chat) are still possible, as before this branch. The dialog already handles them: `ApprovalDialog` (`crates/ui/src/approval.rs`) shows one request and keeps the rest in a `VecDeque`, drops requests whose requester stopped waiting, and the inbox (`approval/inbox.rs`) is an unbounded channel. Nothing is broken; no change was made.

## Check in the running app

None of this was looked at in the window; the UI is covered by `#[gpui::test]` tests only. A `make run` session should check:

- **Spinner and streaming:** a turn with a long timeline; nothing freezes and streamed text keeps appearing. Math (inline and display) in a streamed reply: placeholder, then the formula, without odd layout jumps.
- **Parallel tools:** a prompt that makes the model call several reads or `task`s at once; the statuses settle, every block ends with its result.
- **Side panel:** `panel.tree` show/hide (width, focus border, "Loading the files…", expanding folders); with `cargo build` running it must not reload; create a file in `src/` with the panel hidden, then show it; create a top-level directory and a file inside it.
- **Modals:** a plugin modal and `/open` of a long file (backdrop covering the window, centred card, scroll, `Esc`, click outside); two plugin modals with a smaller one on top (the lower one stays visible); the sysboard / simulation modal.
- **3D:** `/simulate` and `/open` of an STL or STEP file. The first time shows the vector view or the background, then 3D; orbit, zoom, section cut (the old mesh until the cut one lands), hidden CAD solids, coloured 3D molecules, smooth sysboard playback, no flicker; also with `CREST_RENDER3D=off`.
- **Interactive blocks:** zoom/pan of mermaid and diagrams (and the brief placeholder), list/tree filters, table sort and expand, plot sliders, the map, forms, chart hover; custom blocks with HTML nodes.
- **Text:** selection and the "Copy" hint (it disappears when deselected); "copy as" buttons copy the right format; expanding and collapsing shell, tool calls, thinking and diff blocks.
- **Theme:** `/theme` preview with arrows and the wheel (mermaid and diagram colours, the old diagram until the new one lands); `Esc` goes back to the previous theme.
- **Images:** tinted SVG images, molecules and reactions; pasting a PNG (and a BMP if possible); pasting and pressing `Enter` at once (the prompt goes with the image); pasting and pressing `<leader>i` at once (the tray shows "Chat only").
- **Input and pickers:** the `/` palette and the `@`, `/model`, `/theme` popups; the file and grep pickers (arrows, click, scrollbar, preview); the project picker while typing.
- **Other:** a real 403 or 413 from OpenCode through the Anthropic endpoint; `/login` then `/model` (the list refreshes); the approval dialog; resuming a chat.

## How to measure

Every command below should run alone and, on a machine like the development one, under a memory cap: `systemd-run --user --scope -p MemoryMax=16G -p MemorySwapMax=0 --quiet env CARGO_BUILD_JOBS=6 cargo …`. To get before and after numbers, check out `main` and `perf/audit` in two worktrees with separate `target/` directories.

- **Build:** `cargo clean && time cargo build -p crest` (cold); `touch crates/app/src/usecases/run_turn.rs && time cargo build -p crest` (incremental); `time cargo test -p crest --no-run`; `du -sh target`. `cargo build --timings` writes a per-crate report to `target/cargo-timings/`.
- **Startup:** `build_app` has no span of its own yet. Add a temporary `tracing::info_span!("build_app")` around it in `crates/bin/src/root.rs` and run with `CREST_LOG=info` and real keys, MCP servers configured; or time to first window with a release build.
- **UI:** `cargo run --release -p ui --example timeline_stress --features examples` and `--example table_stress`; scroll and stream through them and compare CPU use (`top`, `perf top -p`) and smoothness before and after; the phase 3a test `a_frame_for_the_activity_line_reuses_the_cached_slots` (`crates/ui/src/app.rs`) is the cheap proxy.
- **Compute:** the seeded tests in `adapters/compute` in release (`cargo test --release -p adapter-compute`), or a `criterion` bench on `bootstrap`/`simulate` with a costly statistic.
- **Session save:** `strace -c -e trace=pwrite64,fsync` on a long chat, or the WAL size after a save.

## Future phase

- **`application-core`.** *Done (`arch/2-application-core`).* New crate `crates/app-core` (package `application-core`) with the ports, the use cases (all but `simulate`), the tool registry and budgets, privacy, themes, keymap, cost, the provider router and the two tool families the agent loop knows by name (`ask_user`, `web_search`/`web_fetch`, whose run_turn tests drive them). `application` keeps the flows, the other tools and `usecases::simulate` (it builds on the research flow's spec writer) and re-exports the core modules under their old paths, so `bin`, `flows` and the tools did not change their imports. The constants the loop and `/open` borrowed from tools (`ASK_USER`, the STEP limits, the web tool names and line prefixes) moved down first. `ui`, `application-testing` and every adapter but `adapter-lua` depend on `application-core` only (`ui` and `adapter-mcp` keep `application` as a dev-dependency for tests that drive the file and MCP tools); `scripts/check-arch.sh` has an `app-core` layer and forbids `ui → application` outside `[dev-dependencies]`. Touching `crates/app/src/tools/formats/gantt.rs` now re-checks `application`, `adapter-lua` and `crest` only; `cargo check -p ui` is a no-op.
- **`BlockTag` / `BlockView` registry.** Every block kind is spelled out in several matches: the `domain` enum and its validation, the timeline renderer and `blocks/` in `ui`, copy-as, and the Lua bridge. A registry keyed by a `BlockTag`, with one `BlockView` implementation per kind, would turn adding a block into one module plus one registration.
- **Command routing into `application::usecases::commands`.** `/` commands are split by name in `crates/ui/src/session/commands.rs` and carried out by the matches in `crates/bin/src/gui.rs` (and, for the workflows, `crates/bin/src/headless/`); moving the routing into a use case would share it between front ends and make it testable without the UI or the composition root.
- **Per-domain features in `adapter-formats`** (devops, engineering, science, CAD/3D, …) and a **`syntax-all` feature for the tree-sitter grammars**, so a development build compiles only what is being worked on; the default build keeps everything.
- **Shared builders for the formats blocks.** *Done (`arch/1-formats-builders`).* Each tool in `crates/app/src/tools/formats/` exposes `normalize_<kind>` (the synchronous checks, defaults and limits, so `prepare` still answers `InvalidInput`) and `build_<kind>(ports, call)` (read through the guard, parse on the `Offload`, title, `validate`); the tool adds its summary and block, and `adapters/lua/src/api/formats.rs` deserializes the Lua table into the tool's input (`serde_path_to_error` over `mlua`'s deserializer) and registers each function on the same pair. The tool's messages won; Lua gained `deny_unknown_fields`, the path-like baseline check and the waveform pattern cap, the tool gained `rows ≥ 1`. The Lua bridge went from 1203 to 343 lines of non-test code (`sciences.rs` folded in). The five tools without a Lua function (`show_test_report`, `show_findings`, `show_wavedrom`, `show_plate`, `show_material`) were left as they were.
