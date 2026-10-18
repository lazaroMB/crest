# crest documentation

For the project overview, installation and quick start, see the [main README](../README.md).

| Guide | For | Contents |
| --- | --- | --- |
| [usage.md](usage.md) | People using crest | The window, the modes, the key bindings, the `:` commands, approvals, blocks, the CLI, headless mode, sessions and MCP servers. |
| [configuration.md](configuration.md) | People configuring crest | `init.lua` paths and precedence, the API key, `app.opt`, models by role, permissions, environment variables, Langfuse and a full example. |
| [plugins.md](plugins.md) | People writing plugins | The Lua sandbox, the `app` table (keymaps, commands, tools, hooks, `app.ui`, `app.modal`, MCP), diagnostics and step-by-step examples. |
| [workflows.md](workflows.md) | People using the workflows | The research workflow (14 nodes, gates, `.crest/specs/`) and the implementation workflow (steps, gates, commits, `report.md`). |
| [architecture.md](architecture.md) | People reading the code | The hexagonal layers, ports and adapters, the loop of a turn, events, threads, persistence and the dependency rules. |
| [development.md](development.md) | Contributors | Tests and fakes, how to add tools, blocks, adapters or nodes, commit conventions and CI. |

Research notes:

- [research/rich-content-modals.md](research/rich-content-modals.md): how plugin modals draw rich content (images, documents, tables, PDF, 3D, terminal), the GPUI constraints, why web pages are not embedded, and the roadmap.
