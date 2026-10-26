# Architecture

crest follows a hexagonal architecture (ports and adapters):

- The domain and the use cases know nothing about GPUI, HTTP, Lua, SQLite or external processes.
- All of that lives in adapters that implement traits (**ports**) defined in `application`.
- The binary is the only place that wires all the pieces together.

## Layers and crates

```mermaid
flowchart TB
    subgraph bin["crates/bin · crest (composition root)"]
        root["root.rs · build_app"]
        gui["gui.rs · UiController"]
        headless["headless/ · HeadlessSink, StdinApproval, StdinForm"]
    end
    subgraph ui["crates/ui · GPUI"]
        session["Session, Timeline, Input, Status, Picker, Tree, Approval, UiForm, ModalHost, SysBoard, render3d"]
        widgets["widgets/ · timeline, list, graph, tree"]
    end
    subgraph adapters["crates/adapters/*"]
        anthropic["anthropic"]
        search["search (rg, fzf)"]
        fs["fs"]
        shell["shell"]
        store["store (SQLite)"]
        lua["lua (mlua)"]
        mcp["mcp (rmcp)"]
        telemetry["telemetry"]
        export["export (board.html)"]
        compute["compute (stats, solvers, engineering, numbat)"]
        formats["formats (reports, devops, engineering and science files)"]
        web["web (curl)"]
    end
    subgraph app["crates/app · application"]
        ports["ports/*"]
        usecases["usecases/ · run_turn, compact, resume, simulate, compute, open_file"]
        tools["tools/ · registry, files, components, compute, formats, form, card, ghs, task, simulate, mcp, plugin"]
        interop["interop/ · A2UI, Adaptive Cards, MIME bundles, MCP structuredContent"]
        flows["flows/ · research, implement"]
        keymap["keymap.rs, cost.rs"]
    end
    domain["crates/domain · pure types and rules"]

    bin --> ui & adapters & app & domain
    ui --> app & domain
    adapters --> app & domain
    app --> domain
```

| Crate (package) | Path | Contents |
| --- | --- | --- |
| `domain` | `crates/domain` | Pure types and rules: the conversation (`Session`, `Turn`, `Message`), `Block` and its payloads, `ModelPolicy` and roles, `PermissionPolicy`, `WorkspacePath` and sensitive paths, research types (`Evidence`, `Hypothesis`, `Constraint`, `Decision`), the `plan.json` schema, the chat-block specs (`computed` envelopes and card bodies, `widget` specs for the timeline, list, graph and tree with their pure layouts, `devops`, `engineering`, `science`, `gantt` and `interop` specs, each with `validate()`) and `sysmodel` (the `crest.sysmodel/v1` model, its expression language, lint, the deterministic simulator, traces, the assertion checker, the board layout and its renderer-agnostic 3D scene). No tokio, no I/O. |
| `application` | `crates/app` | Ports (async traits), retries and model fallback over any provider (`llm_retry`), the router of prefixed model ids (`providers`), use cases (`AgentService::run_turn`, compaction, resume), the tool registry and the tool definitions, `interop` (pure mappings of A2UI, Adaptive Cards, Jupyter MIME bundles and MCP `structuredContent` onto crest blocks and forms), the keymap parser, cost, the graph-flow workflows and the fakes (`testing`, behind the `testing` feature). |
| `adapter-anthropic` | `crates/adapters/anthropic` | Messages API with SSE streaming, the `GET /v1/models` catalog and server tools (web search and fetch); also the client of OpenCode's Messages endpoint and the keyring entries of every key. |
| `adapter-openai` | `crates/adapters/openai` | OpenAI Chat Completions and Responses with SSE streaming, and the `GET /models` catalog (OpenCode's GPT, Grok and open models). |
| `adapter-search` | `crates/adapters/search` | `rg --files`, `rg --json` and `fzf --filter` as processes without a shell. Includes version detection. |
| `adapter-fs` | `crates/adapters/fs` | Reading and writing through `WorkspaceGuard` (canonicalization and sensitive paths) and diffs with `similar`. |
| `adapter-shell` | `crates/adapters/shell` | `tokio::process` with a timeout, `cwd` in the workspace and capped output. |
| `adapter-web` | `crates/adapters/web` | `WebPort` over `curl` (no shell, minimal environment, headers on stdin): DuckDuckGo HTML, SearXNG or Brave search, and pages to text with `html2text`. |
| `adapter-store` | `crates/adapters/store` | SQLite (embedded `rusqlite`): sessions and graph-flow's `SessionStorage`. |
| `adapter-lua` | `crates/adapters/lua` | `mlua` host (Lua 5.4) on its own thread, with the sandbox, config loading and merging, and the `app` API. |
| `adapter-mcp` | `crates/adapters/mcp` | `rmcp` client (stdio and streamable HTTP). |
| `adapter-export` | `crates/adapters/export` | `SimExport`: the self-contained `board.html` replay of a simulated board, with three.js r160 vendored in `vendor/three/` (MIT) and inlined. |
| `adapter-compute` | `crates/adapters/compute` | `StatsEngine`: crest-stats (descriptives, t / rank / χ² / Fisher tests, OLS, correlation with assumption checks and misuse warnings, on `statrs`) and crest-calc (`numbat` unit sheets and conversions), plus the phase-2 cards: distributions, contingency tables, power (own noncentral t and F), A/B tests, bootstrap, forecasts (`augurs` MSTL + AutoETS) and, as crest-solve, LP/MILP (`microlp`, duals from the dual LP), equations, Monte Carlo (seeded ChaCha20), cash flows and BOMs (`rust_decimal`), decision matrices, Bode, Karnaugh maps, beams and tolerance stacks; every result is a `Computed` envelope with BLAKE3 input hashes and reproduce lines. Synchronous and CPU-bound: callers run it on a blocking thread. |
| `adapter-formats` | `crates/adapters/formats` | `Formats`: JUnit XML (`quick-xml`) and libtest/nextest JSON, SARIF 2.1.0 (own serde structs), WaveDrom to SVG (`wavedrom`), CSV/TSV/JSON-lines/Parquet previews and profiles (`parquet` record API, no arrow) and 2D molecules from SMILES (`chematic`); traces, folded stacks and pprof (`prost`), logs, PostgreSQL plans, HAR, Terraform plans, DOT and SQL DDL (`sqlparser`), criterion results; waveforms (`wellen`), netlists, STEP (`truck-stepio`, `truck-meshalgo`), DXF (`dxf`), IFC schedules (`ifc-lite-core`); sequences (`gb-io`), reactions, SAR descriptors and 3D embedding (`chematic`), spectra (own JCAMP-DX reader, `mzdata`), GeoJSON maps (`geojson`, `geo`, `proj4rs`), CPM schedules, HDF5/NetCDF (`hdf5-pure`, own NetCDF-3 reader) and networks (`petgraph`). Chemistry and science results carry their own `Computed` envelopes (engines `crest-chem`, `crest-geo`, `crest-plan`, `crest-array`, `crest-graph`, `crest-curves`) inside the viewer specs. Synchronous; callers run it on a blocking thread. |
| `adapter-telemetry` | `crates/adapters/telemetry` | `tracing` spans and, with the `langfuse` feature, OTLP export to Langfuse. |
| `ui` | `crates/ui` | The GPUI window and `gpui-component`: virtualized timeline, blocks, modal input, status bar, tree, pickers, approval dialog, plugin modals, the simulation board (`sysboard/`) and its offscreen wgpu renderer (`render3d/`, with the CPU section plane and picking). `widgets/` holds the shared timeline, filterable list, graph and tree table that the domain blocks and `CustomNode` variants draw with. |
| `crest` | `crates/bin` | Composition root: CLI, adapter construction, window or headless. |

## Ports and adapters

| Port | File | Methods | Real adapter | Fake (`application::testing`) |
| --- | --- | --- | --- | --- |
| `LlmPort` | `ports/llm.rs` | `stream(LlmRequest)`, `count_tokens` | `adapter_anthropic::AnthropicLlm`. In the research workflow it is wrapped by `WebToolsLlm` (bin). | `llm::ScriptedLlm` |
| `ModelCatalogPort` | `ports/llm.rs` | `list_models()` | `adapter_anthropic::AnthropicCatalog` | `llm::FakeModelCatalog` |
| `McpPort` | `ports/mcp.rs` | `list_tools`, `call_tool`, `servers` (tools carry `output_schema` and `_meta.ui` visibility; results `structured_content`, `meta` and per-item `audience`) | `adapter_mcp::McpManager` (with the bin's `TimedMcp`, for per-server timeouts) | `llm::FakeMcp` |
| `FileSystemPort` | `ports/fs.rs` | `root`, `read_file`, `read_bytes`, `read_head`, `exists`, `list_dir` (sorted `DirEntry`s, sensitive entries left out, at most 10 000), `prepare_edit`, `prepare_write`, `apply` | `adapter_fs::LocalFs` | `fs::InMemoryFs` |
| `FileSearchPort` | `ports/search.rs` | `list_files`, `find_file`, `tree` | `adapter_search::RipgrepSearch` | `fs::InMemoryFs` |
| `ContentSearchPort` | `ports/search.rs` | `grep` | `adapter_search::RipgrepSearch` | `fs::InMemoryFs` |
| `ShellPort` | `ports/shell.rs` | `run`, `classify` | `adapter_shell::LocalShell` | `fs::ScriptedShell` |
| `ApprovalPort` | `ports/approval.rs` | `request(ApprovalRequest)` | `ui::approval::UiApproval` (window), `crest::headless::StdinApproval` (terminal) | `system::ScriptedApproval` |
| `EventSink` | `ports/events.rs` | `emit(DomainEvent)` | `crest::gui::ChannelSink`, `crest::headless::HeadlessSink`. Both are wrapped in a `TimelineSink` (bin), which records the timeline that gets saved. | `system::RecordingSink` |
| `SessionStorePort` | `ports/store.rs` | `save`, `load`, `latest_for_workspace`, `list`, `delete` | `adapter_store::SqliteSessionStore` | `system::InMemorySessionStore` |
| `PluginHostPort` | `ports/plugin.rs` | `load_config`, `invoke_callback`, `call_tool`, `run_hook` | `adapter_lua::LuaHandle` | `system::FakePluginHost` |
| `ThemeSourcePort` | `ports/theme.rs` | `list`, `load(name)` (sync; theme files are small and local) | `adapter_lua::LuaThemeSource` (`<config dir>/themes/*.lua`, each in a fresh sandbox) | `theme::FakeThemes` |
| `ComputePort` | `ports/compute.rs` (requests in `compute/{stats,solve,engineering}.rs`) | `describe`, `test`, `regress`, `correlate`, `calc`, `convert`, `distribution`, `contingency`, `power`, `ab_test`, `bootstrap`, `forecast`, `optimize`, `solve`, `simulate`, `cash_flow`, `decide`, `bom`, `tolerance`, `beam`, `karnaugh`, `bode`, `recompute`, `run` (sync, CPU-bound; `run` dispatches a stored `ComputeRequest`) | `adapter_compute::StatsEngine` | `compute::FakeCompute` |
| `FormatsPort` | `ports/formats.rs` (science requests in `formats/sciences.rs`) | `parse_test_report`, `parse_sarif`, `render_wavedrom`, `preview_dataset`, `depict_molecule`; `parse_trace`, `parse_profile`, `parse_logs`, `parse_query_plan`, `parse_har`, `parse_terraform_plan`, `parse_dot`, `parse_ddl`, `parse_benchmarks`; `parse_waveform`, `parse_netlist`, `parse_step`, `parse_dxf`, `parse_ifc_schedule`; `parse_sequence`, `depict_reaction`, `molecule_grid`, `parse_spectrum`, `molecule_3d`, `parse_geojson`, `schedule_gantt`, `inspect_array_file`, `analyse_network`, `training_curves` (sync; bytes in, a domain spec out) | `adapter_formats::Formats` | `formats::FakeFormats` |
| `FormPort` | `ports/form.rs` | `ask(FormRequest) → FormResponse` (`Submitted{answer, action}` carries the button pressed on a card form) | `ui::form::UiForm` (window), `crest::headless::StdinForm` (terminal) | `form::ScriptedForm` |
| `WebPort` | `ports/web.rs` | `search`, `fetch`, `backend` | `adapter_web::CurlWeb`; besides the web tools, `show_ghs` and `app.formats.ghs` fetch PubChem through it | `web::ScriptedWeb` |
| `SimExport` | `ports/sim.rs` | `board_html(&Board)` | `adapter_export::HtmlExporter` | — (the use case tests use a stub) |
| `TelemetryPort` | `ports/telemetry.rs` | `turn_started`, `llm_call`, `tool_call`, `turn_finished` | `adapter_telemetry::TracingTelemetry` (and `NoopTelemetry` in the port itself) | `system::RecordingTelemetry` |
| `SessionStorage` (graph-flow trait) | — | — | `adapter_store::SqliteFlowStorage` | graph-flow's in-memory storage |

Themes are split the same way: `domain::theme` holds the colors, the tokens and how missing ones are derived; `application::theme` parses theme tables, resolves `extends` and keeps the catalog (built-in TwoCrest, files, `app.theme.define`); `ui::theme` is the only place that maps a resolved theme onto gpui-component (and keeps it as a global for crest's own views).

The UI also defines its own inbound port, `ui::session::SessionController`, with the methods `run_turn`, `permission_mode`, `models`, `new_session`, `run_callback`, `run_command` and `recompute` (re-runs a computed card's envelope). The binary implements it (`UiController`) on top of the real `AgentService`. The UI depends only on `application` and `domain`.

## Chat blocks

The blocks for technical work follow one layout ([research note](research/chat-blocks-for-technical-work.md)):

- **Domain**: a spec per block kind (`domain::{computed, devops, engineering, science, gantt, widget, interop}`) with `validate()` and pure helpers (statistics of a plan, the critical path of a trace, a sequence search, the graph layouts). Specs are `BlockKind` variants (boxed when large), widgets also `CustomNode` variants, so Lua can build them.
- **Ports**: parsers and engines sit behind `FormatsPort` (file → spec) and `ComputePort` (request → `Computed` envelope); the request of every computed card is stored in its envelope, so `ComputePort::run` can recompute it.
- **Tools** in `application::tools::{compute, formats, form, card, ghs}`: each reads its file through `FileSystemPort` (with the `read_file` guard and a size cap), runs the port through the `Offload` (a blocking thread), emits the block and returns a compact summary with number slots; the model never gets the data.
- **UI**: one renderer per block in `ui::blocks`, built from the shared widgets in `ui::widgets` where they fit; copy-as entries in `ui::blocks::copy`. The headless sink prints one tag line per block.
- **Interop**: `application::interop` maps A2UI streams and Adaptive Cards to `Custom` trees (`CustomNode::Card`, `Placeholder`) plus a `FormSpec` with a `body` and `actions` when the card can be answered, MIME bundles to the block of their richest media type, and MCP results to a block next to the text the model gets. It is pure code with caps on size, nodes and depth; nothing is fetched or run.

### Third-party crates added for the chat blocks (phase 2)

All are pinned in the root `Cargo.toml` and compatible with crest's GPL-3.0-or-later.

| Crate | Version | Licence | Used by |
| --- | --- | --- | --- |
| `rand_chacha` | 0.10.0 | MIT OR Apache-2.0 | `adapter-compute`: seeded resampling and simulation |
| `augurs-core`, `augurs-ets`, `augurs-mstl` | 0.10.2 | MIT OR Apache-2.0 | `adapter-compute`: forecasts |
| `microlp` | 0.6.0 | Apache-2.0 | `adapter-compute`: LP/MILP |
| `rust_decimal` | 1.43.0 | MIT | `adapter-compute`: cash flows, BOM |
| `prost` (derive only), `flate2` | 0.14.4, 1.1.10 | Apache-2.0; MIT OR Apache-2.0 | `adapter-formats`: pprof profiles |
| `sqlparser` | 0.63.0 | Apache-2.0 | `adapter-formats`: ER diagrams |
| `wellen` | 0.25.7 | BSD-3-Clause | `adapter-formats`: VCD, FST, GHW |
| `ruststep`, `truck-stepio`, `truck-meshalgo` | 0.4.0, 0.3.0, 0.4.0 | Apache-2.0 | `adapter-formats`: STEP |
| `dxf`, `encoding_rs` | 0.6.2, 0.8.42 | MIT; (Apache-2.0 OR MIT) AND BSD-3-Clause | `adapter-formats`: DXF |
| `ifc-lite-core` | 22.6.0 | MPL-2.0 (unmodified) | `adapter-formats`: IFC |
| `gb-io` | 0.9.0 | MIT | `adapter-formats`: GenBank |
| `mzdata` | 0.67.4 | Apache-2.0 | `adapter-formats`: mzML, MGF |
| `geojson`, `geo`, `proj4rs` | 1.0.0, 0.33.1, 0.2.1 | MIT OR Apache-2.0 | `adapter-formats`: maps |
| `hdf5-pure` | 0.47.0 | MIT OR Apache-2.0 (with HDF5/ZFP BSD-3 notices) | `adapter-formats`: HDF5, NetCDF-4 |
| `petgraph` | 0.8.3 | MIT OR Apache-2.0 | `adapter-formats`: networks |

`chematic` gained the `rxn`, `threed` and `mol` features (MIT OR Apache-2.0), `adapter-formats` uses `blake3` for its envelopes, and `ui` uses `regex` for the list filter. Features are chosen to keep C and C++ builds out (no `vtk` feature for truck-meshalgo, no `recursive-protection` for sqlparser, `zlib-rs` for flate2 and mzdata).

## A turn: `run_turn`

`AgentService::run_turn` (`crates/app/src/usecases/run_turn.rs`) is the agent loop:

```mermaid
sequenceDiagram
    participant UI as UI / headless
    participant S as AgentService
    participant M as LlmPort
    participant H as PluginHost (hooks)
    participant P as PermissionPolicy
    participant A as ApprovalPort
    participant T as Tool
    UI->>S: run_turn(prompt, TurnOptions)
    S->>S: ModelPolicy::resolve(role, override)
    S-->>UI: TurnStarted
    loop until the model asks for no tools
        S->>S: cancelled? 50 calls? spending cap? compact?
        S->>M: stream(history + system + tools)
        M-->>S: TextDelta / ThinkingDelta / ToolUse / Usage
        S-->>UI: BlockAdded, TextDelta, UsageUpdated
        loop each tool_use, in order
            S->>H: before_tool
            H-->>S: Continue / ReplaceInput / Deny
            S->>T: prepare(input) → subject, preview (diff, command)
            S->>P: decide(tool, effect, subject)
            alt Ask
                S-->>UI: ApprovalRequested
                S->>A: request(preview)
                A-->>S: Approve / ApproveAlways / Deny
            end
            S->>T: run
            T-->>S: output (truncated to ~32 KB)
            S->>H: after_tool
            S-->>UI: ToolStatusChanged
        end
        S->>S: tool_results as a user message
    end
    S-->>UI: TurnFinished
```

Each iteration of the loop goes through these steps:

1. **Pre-checks**, in this order:
   - cancellation;
   - the limit of 50 model calls per turn (exceeding it gives `TooManyIterations`);
   - spending cap: if it has been reached, asks through the `ApprovalPort` with the `spend_cap` tool;
   - compaction: if the estimated context exceeds 0.8 × 200,000 tokens, summarizes the old turns with the `compact` role and keeps the last 2.
2. **Request**:
   - the session history plus the turn's messages;
   - the system prompt, marked for prompt caching;
   - the tool definitions, sorted by name, with the last one marked for caching.
3. **Streaming**:
   - text and thinking open blocks and emit deltas;
   - each `tool_use` opens a `ToolCall` block;
   - `Retrying` and `ModelFallback` are forwarded to the UI;
   - `usage` updates the cost and the context estimate.

   If the API replies that the context is too long, the loop compacts and retries once.
4. **No tools**: if the model asked for no tools, the turn ends with its `stop_reason`.
5. **Tools**, one at a time and in order. For each one:
   1. the `before_tool` hook;
   2. `effect`;
   3. `prepare`, which returns the subject and the preview;
   4. the permission policy: `Allow`, `Ask` (which goes through `ApprovalPort`) or `Deny`;
   5. execution;
   6. the output budget;
   7. the `after_tool` hook.

   Denials are returned to the model as a `tool_result` with an error.
6. **Cancellation**: the `CancelToken` races against the stream and against each tool. On cancellation, the running call and the pending ones get "cancelled by user" and the turn ends as `Interrupted`, with the session left consistent.

The permission policy (`domain::permission`) combines the mode (`ask`, `auto-edit`, `plan`) with the tool's effect (`read`, `write`, `execute`, `network`) and with the user's rules:

- a `Deny` rule always wins;
- `AllowAlways` turns an `Ask` into `Allow`;
- the subagents' `read_only()` policy denies everything that is not a read.

## Domain events to the UI

The use cases draw nothing: they emit `DomainEvent` (`ports/events.rs`) through the `EventSink`.

| Event | Meaning |
| --- | --- |
| `TurnStarted { model, role, … }` | A turn started with this model. |
| `BlockAdded { block }` / `BlockUpdated { block }` | New block / replacement of the block with the same id. |
| `TextDelta` / `ThinkingDelta { block_id, text }` | Streaming text or thinking. |
| `ToolStatusChanged { block_id, status, output }` | A ToolCall changed status. |
| `ApprovalRequested { request_id, block_id }` | A call is waiting for approval. |
| `UsageUpdated { turn, session, cost_usd }` | Tokens and cost. |
| `ModelFallback { from, to }`, `Retrying { attempt, delay_ms }` | Retries and model changes. |
| `ModeChanged { mode }` | Permission mode (the window emits it when it changes). |
| `TurnFinished { stop_reason }` / `TurnFailed { error }` | End of the turn. |
| `Notice { level, message }` | Notice for the status bar. |
| `ModalOpened { spec }` / `ModalUpdated { id, content }` / `ModalClosed { id }` | A plugin modal (`app.modal.*`), pushed by the Lua host. |
| `SimulationReady { board }` | A simulated Spec (`/simulate`, `simulate_solution`): the window opens its board in a full-size modal. Like the modal events, it is not saved with the timeline. |

```mermaid
flowchart LR
    uc["AgentService / workflows (tokio)"] -->|emit| ts["TimelineSink<br/>(records the timeline)"]
    ts --> cs["ChannelSink<br/>futures::mpsc::unbounded"]
    cs --> consume["Session::consume<br/>(GPUI executor)"]
    consume --> tl["TimelineView"] & st["StatusView"]
    ts --> hs["HeadlessSink (terminal)"]
```

In the window, a single GPUI task consumes the channel. It batches the events already queued and applies them in order to `TimelineState`, which is a pure model tested without a window, and to the status bar. `TimelineSink` applies the same events to another `TimelineState`, which is the one saved with the session.

The Lua host has the front end's sink too (`HostOptions::events`), so `app.modal.*` emit modal events directly from the `crest-lua` thread; they skip `TimelineSink` (modals are not saved) and the session hands them to the `ModalHost` overlay. Before any sink sees them, `ImageGuardSink` (`crates/bin/src/images.rs`) checks every image path in modals and `Custom` blocks with the workspace guard (inside the workspace, not sensitive) and replaces rejected images with a text node. `modal_bounds` (`domain::modal`) is the pure geometry rule the overlay draws with.

## Threads

```mermaid
flowchart LR
    subgraph main["main thread"]
        gpui["GPUI: event loop, views"]
    end
    subgraph tokio["multi-threaded tokio runtime"]
        turns["turns, workflows, adapters<br/>(reqwest, rg/fzf, MCP)"]
        blocking["spawn_blocking:<br/>SQLite, keyring, /mcp"]
    end
    lua["crest-lua thread<br/>(Lua VM)"]
    gpui -- "UiController: handle.spawn + oneshot" --> turns
    gpui -- "TokioPort (tree, pickers)" --> turns
    turns -- "ChannelSink (events)" --> gpui
    turns -- "UiApproval (request + oneshot)" --> gpui
    turns -- "LuaHandle: std mpsc + oneshot" --> lua
```

- `main.rs` creates a multi-threaded tokio runtime. GPUI runs its own executor on the main thread (`gui::run`).
- `build_app` runs with `handle.block_on` before the window opens.
- Each turn, each workflow line and each command (`/implement`, `/research`, Lua commands) is launched on tokio with `handle.spawn`. GPUI awaits a `oneshot` and never runs tokio futures.
- Tree and picker searches go through `TokioPort`, which runs them on the runtime.
- Approvals travel through a channel to the dialog, and each request carries its own response `oneshot`. If the turn stops waiting (for example, because you cancelled it), the dialog closes.
- The Lua VM lives on its own `std::thread` (`crest-lua`) and serves requests one at a time. `LuaHandle` sends closures and awaits the result without blocking the runtime.
- SQLite and the keyring run in `spawn_blocking`.
- Headless mode uses `runtime.block_on` on the use case, without GPUI.

## Persistence

The database is a single SQLite file: `$XDG_DATA_HOME/crest/sessions.db` (by default `~/.local/share/crest/sessions.db`). It uses WAL and a 5 s `busy_timeout`, and its migrations are embedded and versioned with `PRAGMA user_version`.

| Migration | Tables | What they store |
| --- | --- | --- |
| `0001_init.sql` | `sessions`, `timeline_blocks`, `permission_rules` | The session (history as JSON), its timeline block by block and its "always allow" rules. |
| `0002_turn_count.sql` | `sessions.turn_count` | Number of turns (to list without deserializing). |
| `0003_flow_sessions.sql` | `flow_sessions` | graph-flow checkpoints of both workflows, with optimistic locking by version. |

- The session is saved after every turn, as `StoredSession { session, timeline, permission_rules }`.
- On startup, `resume_latest` takes the workspace's most recent session (by canonical path), restores its rules and re-emits its timeline.
- Outside SQLite live `mcp-trust.json` (approved MCP servers) and the Specs (`<workspace>/.crest/specs/`).

## Composition root

`crates/bin/src/root.rs::build_app` builds everything:

1. `WorkspaceGuard::open`: canonicalizes the workspace (if it fails, the error is fatal).
2. Starts `LuaHost` and loads the config. Errors become `Error` blocks and crest carries on with the defaults.
3. Sensitive paths, `RipgrepSearch` and the `rg` and `fzf` checks.
4. API key, `AnthropicLlm` and catalog validation (5 s at most).
5. `SqliteSessionStore`.
6. `ToolRegistry`: file tools, component tools, compute, format and form tools (`ask_user`, `show_card`), `show_ghs` when the web tools are on, `task`, `simulate_solution` (over the `Simulator`, which `/simulate` and `--simulate` share), plugin tools and MCP tools (with trust-based admission).
7. Session resume, `AgentService` with compaction, hooks and `CostTracker`.
8. `SqliteFlowStorage`, `ResearchSession` and `ImplementDeps`.

Every non-essential step degrades to a notice: without Lua, without a database or without the catalog, crest starts anyway. Only the workspace and the API key are fatal. In the window, not even a missing API key prevents it from opening: `UnavailableController` answers every prompt with the error.

## Dependency rules

| Layer | May depend on | Forbidden |
| --- | --- | --- |
| `domain` | no crate of the project | I/O crates |
| `application` (must be named so) | `domain` | I/O crates, adapters, `ui` |
| adapters | `application`, `domain`, `platform` | other adapters, `ui` |
| `ui` | `application`, `domain`, `platform` | adapters |
| `platform` (`crates/platform`) | no crate of the project | everything internal |
| `bin` | everything | — |

`platform` is the operating-system glue several outer crates need (spawning and killing processes, `PATH` lookup with `PATHEXT`, crest's config and data directories, the Windows console). It only uses `std`, `dirs` and `windows-sys`, and is the one crate allowed `unsafe` (two Win32 console calls).

The I/O crates that `domain` and `application` cannot use (in normal and build dependencies) are:

`tokio`, `reqwest`, `reqwest-eventsource`, `hyper`, `gpui`, `gpui-component`, `mlua`, `rusqlite`, `sqlx`, `rmcp`, `notify`, `keyring`, `opentelemetry*` and `tracing-opentelemetry`.

`application` uses `tokio` only in its dev-dependencies (for the tests).

`scripts/check-arch.sh` (`make arch`) enforces these rules:

- It reads `cargo metadata --no-deps --offline` with `jq`.
- It classifies each crate by its manifest path. A path that fits no layer is an error.
- It checks internal dependencies, including dev and build ones.
- It requires the `crates/app` package to be named `application`.
- It exits with 0 if all is well, 1 if there are violations and 2 on misuse.
- It works with Bash 3.2. `make test-arch` tests it against the workspaces in `tests/arch/fixtures/`.
