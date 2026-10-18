# Development

This guide is for people changing crest's code. Before touching anything, read [architecture.md](architecture.md).

## Setting up the environment

```bash
make deps-dev   # dependencies + cargo-insta and cargo-nextest
make doctor     # checks without installing
make build
```

The toolchain is stable Rust (`rust-toolchain.toml`), edition 2024, with `rust-version = 1.99`. If Rust does not come from rustup, `make doctor` warns that `rust-toolchain.toml` will not be applied.

## Before every commit

```bash
make check   # fmt-check + lint + test + arch + test-arch + test-deps
```

| Target | What it runs |
| --- | --- |
| `make fmt` / `make fmt-check` | `cargo fmt --all` |
| `make lint` | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| `make test` | `cargo nextest run --workspace` plus the doctests, if nextest is installed. Otherwise `cargo test --workspace`. |
| `make test-live` | The `#[ignore]` tests whose name contains `live`, with `ANTHROPIC_LIVE_TEST=1`. Needs `ANTHROPIC_API_KEY` in the environment. |
| `make arch` / `make test-arch` | `scripts/check-arch.sh` and its fixtures. |
| `make test-deps` | The tests of `scripts/install-deps.sh` (helpers and a macOS simulation). |

Code rules, enforced by the workspace lints:

- `unsafe` is forbidden.
- `unwrap()` and `expect()` are denied outside tests.
- Errors are typed with `thiserror` in the libraries; `anyhow` is only used in the binary.
- The API key never appears in logs, tests or files. `crates/adapters/anthropic/tests/no_key_in_logs.rs` checks this.

Tests use no network, no `rg` and no window, except where they are adapter integration tests:

- Use cases are tested with the fakes from `application::testing`.
- The Anthropic API is tested with `wiremock`.
- GPUI tests use `gpui`'s `TestAppContext` (`test-support` feature), which opens no windows.
- Snapshots of the tools' JSON schemas are stored with `insta` (`crates/app/src/tools/snapshots/`). If you change a schema, review the snapshot with `cargo insta review`.

## Fakes in `application::testing`

To use the fakes from another crate, enable the feature: `application = { workspace = true, features = ["testing"] }` under `[dev-dependencies]`.

| Module | Fakes |
| --- | --- |
| `testing::llm` | `ScriptedLlm` (replies with a script of `ScriptedResponse`: text, tool uses, errors), `FakeModelCatalog`, `FakeMcp`. |
| `testing::fs` | `InMemoryFs` (in-memory `FileSystemPort`, `FileSearchPort` and `ContentSearchPort`, with `with_file`), `ScriptedShell`. |
| `testing::system` | `RecordingSink` (records the `DomainEvent`s), `ScriptedApproval` (scripted answers), `InMemorySessionStore`, `FakePluginHost`, `RecordingTelemetry`. |

The integration tests in `crates/bin/tests/` bring up the real composition root with a scripted model through `root::Overrides`, which lets you replace the LLM, the catalog, the config paths, the database and the MCP trust file:

- `headless_demo.rs`
- `resume.rs`
- `mcp_servers.rs`
- `ui_session.rs`
- `research_demo.rs`
- `implement_demo.rs`

For the UI there are runnable examples in `crates/ui/examples/`, for example `cargo run -p ui --example chart_blocks`.

## Adding a tool

1. Write a type that implements `TypedTool` (`crates/app/src/tools/registry.rs`). The input is a `struct` with `Deserialize + JsonSchema`, and the schema is derived with `schemars`. If you need full control over the schema, implement `Tool` directly.
   - `name`: must match `^[a-zA-Z0-9_-]{1,128}$` and be unique in the registry.
   - `description`: written for the model; say when to use it and when not to.
   - `effect(&input)`: returns `ToolEffect::{Read, Write, Execute, Network}`. This is what the permission policy decides on.
   - `prepare(input, ctx)`: validates and returns a `PreparedCall::new(subject, future)`. `subject` is what is shown and what an "always" rule covers (a path, a command line). Optionally add `.with_preview(ApprovalPreview::…)` and an extra block (like the `Diff` of `edit_file`). The future does the work and returns a `ToolOutput`.
2. If the tool needs I/O, ask for a port, not an adapter. If no suitable one exists, extend or create one in `crates/app/src/ports/`, with its fake in `testing`.
3. Register it in `crates/bin/src/root.rs::build_app` with `tools.register_typed(...)` or `tools.register(Arc::new(...))`. File tools are registered in `register_file_tools` and component tools in `register_component_tools`.
4. Add an `insta` snapshot of its definition, as in `tools/files.rs` and `tools/components.rs`, and tests with the fakes.

If a tool should show something rich in the timeline, return `ToolOutput::text(confirmation).with_block_update(BlockKind::…)`. The model only gets the confirmation, as with `show_chart` and friends.

## Adding a block type

1. **Domain**: add the variant to `BlockKind` in `crates/domain/src/block.rs`, with its serializable `JsonSchema` payload. If it has invariants, add a `validate`. Update the `match` in `Block::validate` and the snapshots.
2. **Timeline**: `crates/ui/src/timeline/`.
   - `render.rs`: `kind_name` and the `copy_text` text that `y` copies.
   - `state.rs`: `has_alt_view` if `t` should toggle, and `expanded_by_default`.
3. **Render**: create `crates/ui/src/blocks/<type>.rs` with `pub fn render(entry, window, cx) -> AnyElement` and register it in `blocks::render` (`blocks/mod.rs`). If you need per-block GPUI state (scroll, zoom, sort order), use `blocks::block_state` as `table` and `mermaid` do.
4. **Headless**: decide how `crates/bin/src/headless/sink.rs` prints it.
5. If the model should be able to create it, add a component tool in `crates/app/src/tools/components.rs` and mention it in `COMPONENT_TOOLS_PROMPT`.

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
| `ci.yml` | `push` and `pull_request` to `main` | `make ci` (`doctor` + `check`) on `ubuntu-24.04` and `macos-15`, plus `install-deps.sh --check` in containers (Ubuntu 24.04 and 22.04, Fedora, Arch). |
| `live.yml` | Manual (`workflow_dispatch`) | `make test-live` with the `ANTHROPIC_API_KEY` secret. |
| `release.yml` | `v*` tags (or manual) | Builds and packages (`scripts/package.sh`) for `x86_64-unknown-linux-gnu` (on ubuntu-22.04, because of glibc) and for `aarch64`/`x86_64-apple-darwin`, unsigned. With a tag, it publishes a GitHub Release with checksums. |

Actions are pinned by commit SHA.
