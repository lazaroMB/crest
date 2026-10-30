# crest documentation

For the project overview, installation and quick start, see the [main README](../README.md).

| Guide | For | Contents |
| --- | --- | --- |
| [usage.md](usage.md) | People using crest | The window, the modes, the key bindings, the `/` commands, approvals, blocks (statistics and calc cards, math, forms, plots, test reports, datasets, molecules), the CLI, headless mode, sessions and MCP servers. |
| [configuration.md](configuration.md) | People configuring crest | `init.lua` paths and precedence, the Claude and OpenCode keys, `app.opt`, models by role, permissions, MCP servers (`/mcp`, OAuth), plugin capabilities and secrets, environment variables, Langfuse and a full example. |
| [themes.md](themes.md) | People customizing the look | Using a theme and `/theme`, writing one (tokens, states, syntax, `extends`, `app.opt.colors`), porting a Vim colorscheme, and the window mode (`app.opt.window`). |
| [plugins.md](plugins.md) | People writing plugins | The Lua sandbox, the `app` table (keymaps, `/` commands, tools, hooks, `app.ui`, `app.compute`, `app.modal`, MCP), capabilities (`app.http`, `app.shell.run`, `app.secret`), diagnostics and step-by-step examples. |
| [workflows.md](workflows.md) | People using the workflows | The research workflow (14 nodes, gates, `.crest/specs/`), simulating a solution (`/simulate`, the board, `simulation/`) and the implementation workflow (steps, gates, commits, `report.md`). |
| [architecture.md](architecture.md) | People reading the code | The hexagonal layers, ports and adapters, the loop of a turn, events, threads, persistence and the dependency rules. |
| [development.md](development.md) | Contributors | Tests and fakes, how to add tools, blocks, adapters or nodes, commit conventions and CI. |

Research notes:

- [research/chat-blocks-for-technical-work.md](research/chat-blocks-for-technical-work.md): which new chat blocks crest should render for developers, engineers, architects, chemists, physicists and data people; computation you can verify (the `Computed` envelope), statistics cards, LaTeX math, interop formats and the roadmap (phase 1 implemented).
- [research/discovery-flow.md](research/discovery-flow.md): proposal to replace the research flow with a governed discovery at two levels (`/research`, `/research --deep`): a document with a section contract, an append-only event log as the state, audits before verdicts with provenance, sources from the open web and the literature (papers, books, standards, docs) with stored text and verbatim spans, a computed causal diagram and spikes with frozen criteria.
- [research/graph-vs-state-machine.md](research/graph-vs-state-machine.md): a task graph (graph-flow, the research flow today) against an event-sourced state machine (the proposed discovery) as engines for a guided investigation, with worked examples and a recommendation.
- [research/research-flow-as-is.md](research/research-flow-as-is.md): the research flow as it is in 0.15: the two pipelines with their gates and go-backs, the node contract, the status machine, what is stored and written, and what to keep.
- [research/installation.md](research/installation.md): how crest is packaged and installed on each platform.
- [research/rich-content-modals.md](research/rich-content-modals.md): how plugin modals draw rich content (images, documents, tables, PDF, 3D, terminal), the GPUI constraints, why web pages are not embedded, and the roadmap.
- [research/solution-simulator.md](research/solution-simulator.md): an isometric 3D "motherboard" view of each solution with an executable, checked simulation of its scenarios (implemented as an MVP; see its status section).
