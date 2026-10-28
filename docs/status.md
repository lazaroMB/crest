# Project status

Where crest is going next: [vision and roadmap](vision.md).

crest is a **work in progress**. Every phase of the original plan is implemented, `make check` passes, and CI runs on Ubuntu 24.04, macOS 15 and Windows (clippy and tests) on every push; releases (Linux x86_64, macOS arm64 and x86_64, Windows x86_64) are built by `release.yml` on each `v*` tag. Before relying on crest for real work, keep in mind what has not been tested and what is still missing.

## Unverified

- It has not been tested against the real Anthropic API in the development environment: every test uses scripted models or `wiremock`, and `make test-live` has not been run here. The default model ids (`claude-sonnet-5-5`, `claude-opus-5-5`, `claude-haiku-5-5`, `claude-fable-5-1`) are validated against `GET /v1/models` at startup.
- macOS has not been tested on a real machine: CI builds and tests there, but nobody has used the window on a Mac.
- Windows has only been built and tested in CI: nobody has used the window on a Windows machine. Tests that need symlinks or POSIX tools run on Unix only. On Windows a command's child processes are killed with `taskkill /T` while the command is alive; children left behind after it exits are not (Unix kills its whole process group).
- The decision model (Cloudflare Clef) has only been tested against a mock server, not with a real token.
- The installer (`scripts/install.sh`) is tested on a temporary prefix with a fake binary on Linux and macOS (CI), but `crest.app` has not been opened on a real Mac. macOS signing and notarization run in `release.yml` since v0.14.0. The Homebrew tap (`lazaroMB/homebrew-tap`) was created by hand with the v0.16.0 cask; `release.yml` updates it, and winget, on each tag once the `PACKAGES_TOKEN` secret is set (without it the jobs print a notice and skip). SignPath on Windows is wired but has not run yet. Windows stays unsigned until SignPath accepts the project (it asks for proof of reputation), so a setup downloaded with a browser gets a SmartScreen warning; winget and `install.ps1` do not. crest.app now re-executes itself through the login shell (`platform::login_shell`) instead of a launcher script; not tried on a Mac.
- Reading the API key or plugin secrets from the system keyring has not been tested (the tests use the environment fallback).
- The rtk integration (`app.opt.rtk`) has only been tested with a fake `rtk` script that follows the `rtk rewrite` exit-code contract, not with the real binary.
- The chat blocks for technical work (statistics and calc cards, recompute, number slots, LaTeX math, `ask_user` forms, plots with sliders, test reports, findings, WaveDrom, datasets, molecules, plates, ANSI colours, SVG images, copy-as) are covered by unit and `#[gpui::test]` tests only: the window has not been opened to look at them, so layout, colours and focus behaviour are unchecked. The statistics engine is checked against NIST StRD and R reference values; the Python and R reproduce lines have not been run against those tools.
- The privacy layer works by patterns: it catches well-known key formats and validated personal data, not every secret (a password with only letters, a custom token format). Add `app.opt.privacy.patterns` for your own formats.

## Behavior to keep in mind

- `Ctrl+C` stops a running workflow step in the window; the workflow stays saved at its last checkpoint and `/research resume` or `/implement resume` runs the step again. In headless mode it kills the process (the same resume applies).
- At workflow gates, an empty line counts as an approval (`yes` / `continue`). In the implementation workflow that can commit with failing criteria or close the workflow.
- In the implementation workflow, only `edit_file` and `write_file` are restricted to the task's files: `run_command` can touch others (although the commit only includes the task's files).
- crest ships no model prices: until you set `app.opt.pricing`, the cost segment shows the session's tokens (`48.2k tok`) instead. The cost lives in memory, so a resumed session starts again at `$0.00` against the cap.

## Not available yet

- The font is not configurable (JetBrains Mono when installed).
- `app.opt.leader` only affects mappings defined in Lua; the builtin ones always use `Ctrl+G`. Actions such as `session.resume` are not implemented.
- There is no command to list or pick older sessions: the workspace's last one is always resumed.
- Compaction assumes a 200,000-token context window, whatever the model, and its threshold is not configurable.
- `/mcp trust` takes effect the next time you open crest; `--trust-mcp` starts the server in the same run. (`/plugin trust`, by contrast, takes effect at once.)
- Plugin capabilities cover HTTP(S) to listed hosts and listed programs; there is no file-system capability, and a slow `app.http` or `app.shell.run` call holds the Lua VM (other hooks wait) until it finishes or times out.
- Simulation (MVP): flat state machines, FIFO queues and timers as self-messages; no latency percentiles, load generators, fault toggles in the board, GIF/video export or re-simulation inside `board.html` yet, and the board has only a dark theme. See [docs/research/solution-simulator.md](research/solution-simulator.md).
- Modals draw images, documents, tables, diagrams and 3D models (OBJ, STL, PLY, geometry only) natively, but cannot embed web pages (links open the browser), PDFs or a terminal yet; images larger than the GPU's texture limit (often 8192 or 16384 px) fail to draw. See [docs/research/rich-content-modals.md](research/rich-content-modals.md) for the roadmap.
- Chat blocks, phase 1 gaps: no Arrow IPC datasets; molecules have no stereo wedges; WaveDrom covers part of WaveJSON; copy-as appears only on the selected block. See the [research note](research/chat-blocks-for-technical-work.md#roadmap) for phase 2.
- Langfuse export requires building with `--features langfuse`; the `make release` binaries do not include it.
- No native installers on macOS and Linux (.dmg, .deb, AppImage) and no in-app updates: `brew upgrade`, `winget upgrade` or the install script again. See [docs/research/installation.md](research/installation.md).
