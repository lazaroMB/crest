# Vision and roadmap

## Vision

crest is the workspace where a coding agent **shows its work, verifies it, and leaves you in control**. It replaces the terminal transcript with a native window where every change, test, chart and diagram is something you can see and act on; it replaces loose prompts with guided pipelines that stop at the right moments for you; and it stays open: GPL, any model, your project's own instructions, and programmable end to end in Lua.

## Principles

1. **Show the work.** Everything the agent does is a block: diffs, tool calls, charts, tables, diagrams, 3D models. The UI is native and stable, so you can read a diff while the agent keeps working.
2. **Verify before done.** A turn or a pipeline is not finished until its checks pass. "Done" means tested, not claimed.
3. **You stay in control.** Changes wait for your approval with the exact diff or command; permission modes, a sandbox and explicit plugin capabilities decide what can run without asking. Nothing happens behind your back.
4. **Open and portable.** crest reads the `AGENTS.md` and `CLAUDE.md` your project already has, talks to any model provider (cloud or local), and is free software. Leaving costs nothing.
5. **Programmable.** Tools, commands, hooks, blocks, modals and whole **pipelines** are Lua, in a sandbox, with capabilities you approve.

## Where we are (v0.17)

- A native GPUI window: a timeline of rich blocks, a prompt with history, a status bar with model, permission mode, cost and git; a Chats panel that lists every project's chats and a rail of the questions asked.
- Blocks for technical work: test reports, findings, datasets, traces, flame graphs, logs, query plans, waveforms, CAD parts, molecules, sequences, spectra, maps, Gantt charts and more, computed from the file and summarised to the model.
- Numbers you can verify: statistics, calc sheets with units, solvers and engineering cards computed by deterministic engines, with provenance, Recompute, number slots in the reply and reproduce lines for Python and R; TeX math in replies; `ask_user` forms answered in the chat.
- Guided workflows: research (from a problem to a Spec and a plan: five stages by default, with the independent work in parallel and gates only where you decide, or the full fourteen with `--deep`), implementation (task by task, one commit each, a report) and simulation (a Spec's scenarios on a 3D board).
- Approvals with the exact diff, `ask` / `auto-edit` / `plan` modes and "allow always" rules.
- Lua config and plugins (tools, commands, hooks, keymaps, blocks, computed cards, format readers, forms, modals) with capabilities; MCP servers with explicit trust; web search and fetch through `curl`.
- An optional decision model (Cloudflare Clef) that routes each prompt's tools and model and reads answers at the gates.
- `/open` for code, documents, images and 3D models (OBJ, STL, PLY, STEP).
- Two model accounts, Anthropic and OpenCode (Go and Zen), with prefixed ids and fallback chains across them.
- Installers for the three platforms: a Homebrew cask with signed and notarized builds, winget and a PowerShell script, and `install.sh` for Linux.

What has not been verified and what is missing is in [status.md](status.md).

## Roadmap

Each item says what it is, why it matters, and when it counts as done. Versions are a direction, not dates.

### v0.7: your project, your sessions

**1. `AGENTS.md` and `CLAUDE.md` compatibility.**
crest loads the instructions a project already keeps for agents: `AGENTS.md` and `CLAUDE.md` at the workspace root, the nested ones in the directories the agent works in (they apply to their subtree), `CLAUDE.md`'s `@path` imports, and the user-level files (`~/.config/crest/AGENTS.md`, `~/.claude/CLAUDE.md`). They go into the system prompt (today a fixed text, `crates/bin/src/root.rs`) and are injected again after compaction, so rules are not forgotten mid-session. `/help` lists which files were loaded, and `/init` writes a first `AGENTS.md` for the project.
*Why:* instructions that agents ignore or lose are one of the most repeated complaints about coding agents, and teams should not maintain one file per tool.
*Done when:* a project set up for Claude Code (or any `AGENTS.md` tool) behaves the same in crest without changes.

**2. Session tabs, tmux style.**
Several sessions in one window, each with its own agent, turn and queue, running at the same time. A tab bar shows each one's state (working, waiting for approval, done, failed); an approval or a notice in a background tab marks it. Keys on the leader, as in tmux: `Ctrl+G c` new tab, `Ctrl+G x` close, `Ctrl+G n` / `Ctrl+G b` next and previous, `Ctrl+G 1`–`9` go to a tab, `Ctrl+G ,` rename, `Ctrl+G s` choose among saved sessions (which also ends "only the last session is resumed"). The tabs come back when the workspace is opened again.
*Why:* real work is parallel: a research flow running while you fix something small, two features at once.
*Done when:* two tasks progress in parallel in one window and you move between them as you would in tmux.

**3. Context and cost you can see.**
The status bar shows the context used against the model's window (with how much the tool definitions take), the cost per turn and per tab, and warns before compaction. The session's cost survives a resume.
*Why:* opaque limits and surprise bills are the top complaint about coding agents.
*Done when:* nobody has to guess why a session got expensive or why it compacted.

**4. Definition of done.**
`app.opt.done_check = { "make", "test" }` (or the check an `AGENTS.md` names): when a turn changed files, crest runs the check before the turn ends and gives the failures back to the model, a bounded number of times. The result is a block in the timeline.
*Why:* agents that report "done" without running anything are the second most repeated complaint. The implementation flow already verifies every task; the chat should too.
*Done when:* a chat turn that edits code never ends with a failing check unless the user accepts it.

### v0.8: pipelines in Lua

**5. Pipelines defined by plugins.**
`app.pipeline.define{ name, description, stages = { … } }` describes a guided workflow on the same engine as research and implementation: checkpoints, gates, resume, stage headers, the activity line and blocks in the timeline. A stage can be:

- an agent turn, with its `role`, `prompt`, allowed `tools` and an `output_schema` for validated JSON;
- a Lua function, which reads and writes the pipeline's state;
- a command (with a `shell:` capability);
- a gate: a confirmation or a choice, read by the decision model when there is one.

Transitions can branch on the state, go back, and loop with a limit. A pipeline runs with `/run <pipeline> <args>` or from a keymap, in its own tab. `app.decide{ state, questions }` gives plugins the decision model directly. The `showcase` plugin will ship an example ("triage a bug → fix → test → summary").

```lua
app.pipeline.define{
  name = "bugfix",
  description = "Reproduce, fix and verify a bug",
  stages = {
    { name = "reproduce", agent = { role = "explore", prompt = "Reproduce: {{args}}",
      output_schema = { type = "object", properties = { steps = { type = "array" } } } } },
    { name = "confirm", gate = "Is this the bug you meant?" },
    { name = "fix", agent = { role = "main", prompt = "Fix it with the smallest change." } },
    { name = "verify", run = { "make", "test" }, on_fail = "fix", max_loops = 3 },
  },
}
```

*Why:* the research and implementation flows are crest's strength, but every team has its own process (triage, release, migration, incident review). They should be able to write it.
*Done when:* the research flow could be rewritten as a Lua pipeline without losing features.

**6. Any model.**
An OpenAI-compatible adapter (OpenRouter, Ollama, LM Studio, vLLM and others), provider-prefixed model ids (`ollama:qwen3-coder`), and fallback chains across providers. *Started:* prefixed ids, cross-provider fallbacks and the Chat Completions and Responses adapters are in, with OpenCode Go and Zen as the first providers besides Anthropic; local servers are next. The decision model can send trivial prompts to a local model.
*Why:* usage limits that change without notice and a single provider make an agent fragile; local models make it private and cheap.
*Done when:* crest runs a whole session on a local model, and falls back to another provider when one fails.

**7. Memory that survives.**
A structured ledger of the session (files touched, decisions, errors, actions taken) that is never summarized and is injected again after compaction; the context window and the compaction threshold are set per model (today a fixed 200,000 tokens).
*Why:* summaries lose the details that matter, and agents forget what they did an hour ago.
*Done when:* after a compaction the agent still knows which files it changed and why.

### v0.9: review and trust

**8. Review panel.**
The changes of a turn as one change set, with accept or reject per hunk; automatic checkpoints, `/undo` and `/redo`.
*Why:* reviewing generated code is now the bottleneck, and it should be faster than reviewing a colleague's.
*Done when:* a multi-file change can be reviewed and partially accepted without leaving crest.

**9. Sandboxed commands.**
`run_command` inside an OS sandbox (bubblewrap or Landlock on Linux, `sandbox-exec` on macOS): writes only inside the workspace, no network unless allowed, and it fails closed when the sandbox cannot start. What runs inside the sandbox needs no approval.
*Why:* approval fatigue makes people click "yes" to everything; a boundary is safer than a hundred prompts.
*Done when:* a normal session asks only for what leaves the sandbox.

**10. Workspace trust.**
Before loading a new repository's `.crest/init.lua` and its MCP servers, crest asks once, and remembers the answer.
*Why:* opening an untrusted repository must not run its configuration.
*Done when:* nothing from a repository runs before you trust it.

### Later

- **A worktree per tab**, optional, so parallel tasks on one repository never step on each other; pipelines and implementation flows running side by side.
- **GitHub**: `/issue <n>` straight into research or implementation, opening pull requests and answering review comments (through `gh`).
- **Sharing**: export a session or a pipeline run as a self-contained HTML page, as `board.html` does for simulations.
- **Distribution**: a signed Windows installer (MSI) and packages for Linux distributions (macOS builds are signed and notarized since v0.14).

## Out of scope

- A web IDE or a hosted cloud service.
- Inline autocompletion while you type.
- Being a code editor: crest works next to the editor you already use.

## How to read this

This is a direction, not a schedule: priorities are reviewed at every release, and items may move between versions. Ideas and feedback are welcome as issues in the repository.
