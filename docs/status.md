# Project status

Where crest is going next: [vision and roadmap](vision.md).

crest is a **work in progress**. Every phase of the original plan is implemented, `make check` passes, and CI runs on Ubuntu 24.04 and macOS 15 on every push; releases (Linux x86_64, macOS arm64 and x86_64) are built by `release.yml` on each `v*` tag. Before relying on crest for real work, keep in mind what has not been tested and what is still missing.

## Unverified

- It has not been tested against the real Anthropic API in the development environment: every test uses scripted models or `wiremock`, and `make test-live` has not been run here. The default model ids (`claude-sonnet-5-5`, `claude-opus-5-5`, `claude-haiku-5-5`, `claude-fable-5-1`) are validated against `GET /v1/models` at startup.
- macOS has not been tested on a real machine: CI builds and tests there, but nobody has used the window on a Mac.
- The decision model (Cloudflare Clef) has only been tested against a mock server, not with a real token.
- There is no desktop launcher on macOS (no `.app` bundle) and the Linux one (`make install`) has not been tried on a desktop session.
- Reading the API key or plugin secrets from the system keyring has not been tested (the tests use the environment fallback).

## Behavior to keep in mind

- `Ctrl+C` stops a running workflow step in the window; the workflow stays saved at its last checkpoint and `/research resume` or `/implement resume` runs the step again. In headless mode it kills the process (the same resume applies).
- At workflow gates, an empty line counts as an approval (`yes` / `continue`). In the implementation workflow that can commit with failing criteria or close the workflow.
- In the implementation workflow, only `edit_file` and `write_file` are restricted to the task's files: `run_command` can touch others (although the commit only includes the task's files).
- crest ships no model prices: until you set `app.opt.pricing`, the cost shows as `$?`. The cost lives in memory, so a resumed session starts again at `$0.00` against the cap.

## Not available yet

- The font is not configurable (JetBrains Mono when installed).
- `app.opt.leader` only affects mappings defined in Lua; the builtin ones always use `Ctrl+G`. Actions such as `session.resume` are not implemented.
- There is no command to list or pick older sessions: the workspace's last one is always resumed.
- Compaction assumes a 200,000-token context window, whatever the model, and its threshold is not configurable.
- `/mcp trust` takes effect the next time you open crest; `--trust-mcp` starts the server in the same run. (`/plugin trust`, by contrast, takes effect at once.)
- Plugin capabilities cover HTTP(S) to listed hosts and listed programs; there is no file-system capability, and a slow `app.http` or `app.shell.run` call holds the Lua VM (other hooks wait) until it finishes or times out.
- Simulation (MVP): flat state machines, FIFO queues and timers as self-messages; no latency percentiles, load generators, fault toggles in the board, GIF/video export or re-simulation inside `board.html` yet, and the board has only a dark theme. See [docs/research/solution-simulator.md](research/solution-simulator.md).
- Modals draw images, documents, tables, diagrams and 3D models (OBJ, STL, PLY, geometry only) natively, but cannot embed web pages (links open the browser), PDFs or a terminal yet; images larger than the GPU's texture limit (often 8192 or 16384 px) fail to draw. See [docs/research/rich-content-modals.md](research/rich-content-modals.md) for the roadmap.
- Langfuse export requires building with `--features langfuse`; the `make release` binaries do not include it.
- The macOS binaries from `release.yml` are not signed or notarized.
