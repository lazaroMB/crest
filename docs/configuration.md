# Configuration

crest is configured with Lua files (`init.lua`). You may have none at all: without a config it starts with the defaults described below. To write plugins (tools, hooks, blocks), see [plugins.md](plugins.md). This guide covers the declarative options.

## Files and paths

| What | Path | Notes |
| --- | --- | --- |
| Global config | `$XDG_CONFIG_HOME/crest/init.lua`, or `~/.config/crest/init.lua` | A relative `XDG_CONFIG_HOME` is ignored. macOS uses the same path (not `~/Library`). |
| Plugins | `<global config dir>/plugins/<name>/init.lua` | Loaded in alphabetical order, followed by those listed in `app.plugins`. |
| Workspace config | `<workspace>/.crest/init.lua` | Optional. It may ship with the repository. |
| Database | `$XDG_DATA_HOME/crest/sessions.db`, or `~/.local/share/crest/sessions.db` | SQLite. Stores the chat sessions and the workflow checkpoints. |
| Approved MCP servers | `$XDG_DATA_HOME/crest/mcp-trust.json`, or `~/.local/share/crest/mcp-trust.json` | Written with `0600` permissions. |
| Approved plugin capabilities | `$XDG_DATA_HOME/crest/plugin-trust.json`, or `~/.local/share/crest/plugin-trust.json` | Written with `0600` permissions. See [Plugin capabilities and secrets](#plugin-capabilities-and-secrets). |
| Research workflow Specs | `<workspace>/.crest/specs/NNNN-slug/` | See [workflows.md](workflows.md). |

### Load order and precedence

The files run in this order:

1. The global config.
2. The plugins, first those in `plugins/` in alphabetical order, then those in `app.plugins`.
3. The workspace config.

Each file runs in its own environment and has its own `app` table. A file cannot see the global variables or the `app` table of the others.

When the layers are combined, precedence from lowest to highest is: plugins, global config, workspace config. The global config wins over its plugins, and the workspace wins over both. The merge works like this:

| Item | How it is combined |
| --- | --- |
| `app.opt.*` | Key by key. |
| `app.models.roles` | Role by role and field by field. For example, if the workspace only sets `max_tokens`, it keeps the `id` and `thinking` of the global layer. `fallbacks` is replaced as a whole. |
| `app.models.aliases` | Key by key. |
| Keymaps | By keys: the last definition wins. |
| Commands, tools and MCP servers | By name: the highest-precedence layer wins. |
| Hooks | All are kept, in the order the files ran (global, plugins, workspace). |
| `app.permissions.sensitive_paths` | The global config (or a plugin) can **replace** the default list. The workspace can only **add** patterns, so a cloned repository cannot remove the protection. |
| `app.plugins` | Only read in the global config. In the workspace it is ignored with a notice. |

> The workspace config is Lua code from the repository: it runs in the same sandbox as the rest (see [plugins.md](plugins.md#the-sandbox)) and can register tools, hooks, keymaps and commands. The MCP servers it declares do not start until you approve them (see [MCP servers](#mcp-servers)).

### Errors in `init.lua`

A file that fails (syntax error, runtime error, instruction or memory limit) **contributes nothing**, but the others still load. crest starts regardless. Each problem shows up:

- in the window, as an `Error` block at the top of the timeline, with the text `file:line: message` and the hint "crest started without what that file defines (using the defaults); fix it and reopen crest";
- in headless mode, in the output.

A mistyped field (for example `app.opt.timeoutlen = "500"`) is dropped on its own, with a diagnostic, and the rest of the file still applies. Some typical messages:

```text
/home/yo/.config/crest/init.lua:3: syntax error: unexpected symbol near '='
/home/yo/.config/crest/init.lua: app.opt.timeoutlen must be a whole number of milliseconds
/home/yo/.config/crest/init.lua: app.models.roles: unknown role `mian`
/home/yo/.config/crest/init.lua:12: keymap.set: the key sequence must be a string
/home/yo/.config/crest/plugins/hang/init.lua: instruction limit exceeded
```

Changes to `init.lua` are read at startup, so you have to reopen crest for them to take effect.

## Anthropic API key

crest looks for the key in this order:

1. The `ANTHROPIC_API_KEY` environment variable, if it is not empty.
2. The system keyring (the `keyring` crate), under the **service `crest`** and the **entry (user) `ANTHROPIC_API_KEY`**.

The key **never** goes in `init.lua`, and it does not appear in logs or traces.

To store it in the keyring:

```bash
# Linux (libsecret / GNOME Keyring, KWallet via Secret Service)
secret-tool store --label="crest" service crest username ANTHROPIC_API_KEY

# macOS (Keychain)
security add-generic-password -s crest -a ANTHROPIC_API_KEY -w
```

> The commands above are each system's standard tools. crest only reads the `crest` / `ANTHROPIC_API_KEY` entry through `keyring`, and that read has not been tested in this environment against a real keyring.

If there is no key:

- the window opens anyway, and the first block is the error "the Anthropic API key is missing: export ANTHROPIC_API_KEY or store it in the system keyring (service `crest`, entry `ANTHROPIC_API_KEY`)";
- headless mode exits with code 2.

The API URL is fixed (`https://api.anthropic.com`): there is no variable to change it.

## `app.opt` options

| Option | Type | Default | Effect |
| --- | --- | --- | --- |
| `leader` | string in key notation (`"<C-g>"`, `"<C-,>"`) | `"<C-g>"` | The value of `<leader>` in keymaps **defined in Lua**. Builtin mappings always use `Ctrl+G` (see [usage.md](usage.md#default-key-bindings)). Use a chord: a plain key would be typed into the prompt. Avoid `<C-Space>` (macOS takes it to switch the input source) and Alt chords (Option composes characters on a Mac). |
| `timeoutlen` | integer, ms | `1000` | How long a prefix (`<leader>`, `<leader>f` of `<leader>ff`) waits for the next key. |
| `pricing` | table `{ [model or prefix] = { input, output, cache_read, cache_write } }` | empty | Prices in USD per million tokens. Without prices, the cost shows as `$?`. |
| `max_session_cost` | number > 0 (USD) | no cap | Spending cap per session: when it is reached, crest asks for confirmation before the next model call. |
| `web` | table, see [Web search](#web-search) | DuckDuckGo, 8 results, 20 s, any domain | The `web_search` and `web_fetch` tools: on or off, search backend, limits and domains. |
| `theme` | `"dark"` or `"light"` | `"dark"` | The TwoCrest theme of the window ([usage.md](usage.md#the-window)). `"twocrest"` and `"twocrest-light"` are accepted too; any other value is a startup warning and the dark theme is used. |

Any other `app.opt` key is accepted without error and stored, but crest does not use it. There are no font options: the window uses JetBrains Mono at 14 px if installed, otherwise the system monospace font.

### Cost and spending cap

```lua
app.opt.pricing = {
  -- The key is an exact id or a prefix; the longest matching key wins.
  ["claude-sonnet-5"] = { input = 3, output = 15, cache_read = 0.3, cache_write = 3.75 },
}
app.opt.max_session_cost = 5
```

- The numbers are only an example: crest ships no built-in prices, so use the provider's current ones. All four fields are required, must be numbers ≥ 0, and no other fields are allowed.
- Cost is added per response, including compaction summaries. It shows in the status bar: `$0.00` before the model is used, `$?` if a model without a price was used, and otherwise the running total (four decimals below 1 USD).
- When the cap is reached, crest asks "The session spent $X of $Y; continue?" through the approval dialog (in headless mode, on stdin):
  - **approve** raises the cap by the same amount again;
  - **always** removes the cap for the session;
  - **reject** ends the turn ("turn stopped by the spend cap ($X of $Y)").
- The cost lives in memory: a resumed session counts from zero again.

## Web search

The model can search the web (`web_search`) and read pages (`web_fetch`). Both run on your machine with `curl`: crest calls the search engine itself and converts pages to text; nothing goes through the model provider's web tools. They are offered to the chat, to `task` subagents and to the read-only agent of the research flow.

```lua
app.opt.web = {
  enabled = true,                -- false removes both tools
  backend = "duckduckgo",        -- "duckduckgo" (no key), "searxng" or "brave"
  searxng_url = "https://searx.example.org",  -- required with backend = "searxng"
  max_results = 8,               -- results of a search that does not ask for a number (1-10)
  timeout = 20,                  -- seconds per request (1-300)
  blocked_domains = { "pinterest.com" },      -- or allowed_domains = { ... }; never both
}
```

- **curl is required.** Without it crest starts with the warning "web search unavailable: install curl" and the tools are not offered.
- **No approval.** Both tools only read (a search, a GET), so they run in every permission mode, plan included, without asking. To stop them, set `enabled = false`, limit the domains, or add a `deny` rule on `web_fetch` / `web_search` (the subject is the URL or the query).
- **Backends:**
  - `duckduckgo` reads the HTML results page of `html.duckduckgo.com`. It needs no key, but DuckDuckGo refuses automated searches when there are too many in a short time; the tool then says so.
  - `searxng` uses the JSON API of a SearXNG instance (`/search?format=json`); the instance must have the JSON format enabled.
  - `brave` uses the Brave Search API. The key comes from `BRAVE_API_KEY` or the keyring (service `crest`, entry `BRAVE_API_KEY`); without one crest warns and uses DuckDuckGo. The key is passed to curl on its stdin, never in its arguments, and commands run by the model (`run_command`, Lua shells, MCP servers) do not inherit `BRAVE_API_KEY`.
- **Domains.** `allowed_domains` and `blocked_domains` include subdomains (`rust-lang.org` covers `blog.rust-lang.org`). They apply to the URL read, to the address it redirects to, and to the search results.
- **Pages.** HTML is converted to plain text without navigation, headers, footers, scripts and styles; other text types (JSON, XML, plain text) are kept as is; PDFs and binary files are refused. curl follows up to 5 redirects and reads at most 5 MB; the text is cut to the tool result budget.
- A workspace `app.opt.web` replaces the global one as a whole (it is one `app.opt` key).

## Models by role

Every model call asks for a **role**, and the configuration decides which model serves each role.

| Role | What for | `id` | `max_tokens` | `thinking` | `fallbacks` |
| --- | --- | --- | --- | --- | --- |
| `main` | Conversation and the agent loop | `claude-sonnet-5-5` | 16000 | budget 8000 | `claude-haiku-5-5` |
| `plan` | Plans, design, large refactors; the workflows' reasoning nodes | `claude-opus-5-5` | 32000 | budget 16000 | `claude-sonnet-5-5` |
| `explore` | The `task` subagent and exploration nodes | `claude-haiku-5-5` | 8000 | no | — |
| `compact` | Summarizing the history | `claude-haiku-5-5` | 4000 | no | — |
| `title` | Titles and short names (reserved: no component uses it on its own today) | `claude-haiku-5-5` | 1024 | no | — |
| `component` | Structured data for blocks (reserved: the `show_*` tools are called by the turn's model) | `claude-sonnet-5-5` | 16000 | no | `claude-haiku-5-5` |
| `deep` | Very hard tasks, on demand (`--model deep`, `/model deep`, `app.send(…, { role = "deep" })` or the `role` of a `plan.json` task) | `claude-fable-5-1` | 32000 | budget 16000 | `claude-opus-5-5`, `claude-sonnet-5-5` |

Builtin aliases: `opus` → `claude-opus-5-5`, `sonnet` → `claude-sonnet-5-5`, `haiku` → `claude-haiku-5-5` and `fable` → `claude-fable-5-1`.

### `app.models.roles`

```lua
app.models.roles = {
  main    = { id = "sonnet", max_tokens = 16000, thinking = { budget = 8000 }, fallbacks = { "haiku" } },
  plan    = { id = "opus", thinking = "adaptive" },
  explore = "haiku",                    -- shorthand for { id = "haiku" }
  compact = { thinking = false },       -- only changes that field
}
```

| Field | Accepted forms |
| --- | --- |
| (the whole role) | A table with the fields below, or a string, which is equivalent to `{ id = "..." }`. |
| `id` | A model id or an alias. |
| `max_tokens` | A positive integer. |
| `thinking` | `false`, `"disabled"` or `"off"` (no thinking); `"adaptive"`; a number N (a budget of N tokens); `{ budget = N }` or `{ budget_tokens = N }`; `{ type = "disabled" }` or `{ type = "adaptive" }`. `true` is not valid. |
| `fallbacks` | A list of ids or aliases. They are tried in order on 529 or 5xx errors from the main model. |

An unknown role or field produces a diagnostic. Aliases are expanded once (not recursively) in `id` and in `fallbacks`.

### `app.models.aliases`

```lua
app.models.aliases = { rapido = "claude-haiku-5-5", opus = "claude-opus-5-5" }
```

Aliases are merged with the builtin ones (the workspace wins) and appear in the window's `Ctrl+G m` menu.

### Resolution order

1. **Turn override**: `--model`, `/model <x>` or the `Ctrl+G m` menu.
   - If the value is a **role name** (`plan`), that role's whole spec is used.
   - If it is an alias or an id, it only replaces the `id` and keeps the role's `max_tokens`, `thinking` and `fallbacks`.
   - Aliases are checked before role names.
2. **Role pinned** by a tool or a workflow (for example, the `task` subagent asks for `explore`, and `app.send(..., { role = "plan" })` asks for `plan`).
3. Workspace config.
4. Global config, with its plugins.
5. Builtin values (the table above).

At startup, crest queries `GET /v1/models` and waits at most 5 s. If a role points to a model that does not exist, a notice is shown and the first available fallback is used. If the catalog does not respond, the notice "could not validate the configured models: …" appears and nothing is validated.

## Permissions and sensitive paths

The initial permission mode is chosen with `--mode` (`ask` by default). It is not configured in Lua. How to change it in the window is described in [usage.md](usage.md#approvals-and-permission-modes).

| Tool effect | `ask` | `auto-edit` | `plan` |
| --- | --- | --- | --- |
| `read` (reading, searching, blocks, `task`, `git log/diff/status/show/blame`) | allow | allow | allow |
| `write` (`edit_file`, `write_file`) | ask | allow | deny |
| `execute` (`run_command`, plugin or MCP tools without `effect`) | ask | ask | deny |
| `network` | ask | ask | deny |

**Sensitive paths** are paths that no tool can read or write.

- The default list is `.env*`, `*.pem`, `id_rsa*` and `.git/`.
- Each pattern is matched against **every component** of the path: it only supports `*` and `?`, and cannot contain `/` except for a trailing `/`, which marks a directory. For example, `.env*` denies `.env` and `config/.env.local`, and `.git/` denies everything inside `.git`.

```lua
-- Global config: replaces the default list (include the ones you want to keep).
app.permissions.sensitive_paths = { ".env*", "*.pem", "id_rsa*", ".git/", "*.key" }

-- Workspace config: only ADDS to the current list.
app.permissions.sensitive_paths = { "secrets/" }
```

An invalid pattern (empty, or with a `/` in the middle) makes crest fall back to the default list and show a notice.

The "always allow" rules you grant in an approval dialog are saved with the session and come back when the session is resumed. There is no Lua syntax to declare them.

## MCP servers

They are declared in `app.mcp`, either as a table keyed by name or with `app.mcp.add`. All fields are described in [plugins.md](plugins.md#appmcp-mcp-servers).

```lua
app.mcp.add("docs", { command = "mcp-docs", args = { "--root", "." }, effect = "read", timeout = 30 })
app.mcp = { tickets = { url = "https://tickets.example.com/mcp", headers = { Authorization = "Bearer …" } } }
```

- Servers from the **global config** (and its plugins) start when crest opens.
- Those from the **workspace** stay pending, with a notice like "workspace MCP server `x` not started: it would run `…`. If you trust it, start crest with `crest --trust-mcp x` (remembered for this workspace)". They start once you approve them with `crest --trust-mcp <name>` or with `/mcp trust <name>` (the latter takes effect the next time you open crest).
- The approval is stored per workspace in `mcp-trust.json`, together with the exact transport: if the command, arguments, environment, URL or headers change, crest asks again. To revoke an approval, delete its entry from the file.
- On startup, crest waits up to 5 s for the servers to be ready. The per-call limit for a tool is `timeout` if you declare it, or 120 s otherwise.

## Plugin capabilities and secrets

Plugins declare network and shell capabilities with `app.plugin{ capabilities = ... }` ([plugins.md](plugins.md#capabilities)). They stay inactive until you approve them:

- `crest --trust-plugin <name>` (repeatable) or `/plugin trust <name>` in the window, which activates them at once. `/plugin list` shows each declaration and whether it is approved.
- Approvals are stored in `plugin-trust.json` (path in [Files and paths](#files-and-paths)), written with `0600` permissions. Each entry holds the plugin name, the `init.lua` that declared it, its layer (`global` or `workspace`) and the exact capability list:

  ```json
  {
    "plugins": [
      {
        "plugin": "jira",
        "source": "/home/me/.config/crest/plugins/jira/init.lua",
        "origin": "global",
        "capabilities": ["network:jira.example.com", "shell:jq"]
      }
    ]
  }
  ```

  If the declaration changes in any way, the plugin asks again. To revoke an approval, delete its entry; it takes effect on the next start.

A plugin's secrets (`app.secret("<name>")`) are read from the system keyring, service `crest`, entry `plugin:<plugin>:<name>`, and otherwise from the environment variable `CREST_PLUGIN_<PLUGIN>_<NAME>` (uppercase; every character other than a letter or digit becomes `_`). To store one in the keyring:

```bash
# Linux (libsecret / GNOME Keyring, KWallet via Secret Service); prompts for the value
secret-tool store --label=crest service crest username plugin:jira:token
# macOS (Keychain)
security add-generic-password -s crest -a plugin:jira:token -w
# or, for this shell only
export CREST_PLUGIN_JIRA_TOKEN=...
```

Only a plugin whose capabilities were approved can read secrets, and only its own. crest never logs them, and the programs run by `run_command` or `app.shell.run` do not inherit any `CREST_PLUGIN_*` variable.

## Environment variables

| Variable | Effect |
| --- | --- |
| `ANTHROPIC_API_KEY` | The API key. Takes priority over the keyring. |
| `XDG_CONFIG_HOME` | Base of the global config and the plugins (`$XDG_CONFIG_HOME/crest/`). Only used if absolute. |
| `XDG_DATA_HOME` | Base of `sessions.db`, `mcp-trust.json` and `plugin-trust.json` (`$XDG_DATA_HOME/crest/`). Only used if absolute. |
| `CREST_PLUGIN_<PLUGIN>_<NAME>` | A plugin secret, read by `app.secret("<name>")` when the keyring has no entry (see [Plugin capabilities and secrets](#plugin-capabilities-and-secrets)). Never passed to the programs crest runs. |
| `CREST_LOG` | Log filter in `tracing`'s `EnvFilter` syntax, for example `info`, `debug` or `warn,lua=info`. Defaults to `warn`. The log goes to stderr. |
| `LANGFUSE_PUBLIC_KEY`, `LANGFUSE_SECRET_KEY` | Enable trace export to Langfuse, **only in a binary built with `--features langfuse`**. Both are needed: if one is missing, crest shows a notice and does not export. |
| `LANGFUSE_HOST` | Langfuse instance. Defaults to `https://cloud.langfuse.com`. |
| `ANTHROPIC_LIVE_TEST` | Development only: `make test-live` sets it to `1`. |

Lua's `print()` goes to the log at `info` level with the `lua` target. To see it: `CREST_LOG=warn,lua=info crest`.

## Telemetry and Langfuse

crest emits `tracing` spans per turn, per model call and per tool. By default they are only logged, according to `CREST_LOG`. To send them to Langfuse:

```bash
cargo build --release -p crest --features langfuse
export LANGFUSE_PUBLIC_KEY=pk-lf-...
export LANGFUSE_SECRET_KEY=sk-lf-...
# export LANGFUSE_HOST=https://my-langfuse.example.com
./target/release/crest
```

`make release` and the release binaries are built **without** that feature. Telemetry never prevents startup: any problem is shown as a notice.

## Full example

A global config, checked against the current implementation (it loads without diagnostics, except for the listed plugin if its folder does not exist):

```lua
-- ~/.config/crest/init.lua
local app = require("app")

-- Keys -------------------------------------------------------------------
app.opt.leader = "<C-g>" -- <leader> for the keymaps in this file
app.opt.timeoutlen = 600    -- ms to wait between the keys of a sequence

-- Cost (USD per million tokens; an example, use current prices) ---------
app.opt.pricing = {
  ["claude-sonnet-5"] = { input = 3, output = 15, cache_read = 0.3, cache_write = 3.75 },
  ["claude-haiku-5"]  = { input = 1, output = 5,  cache_read = 0.1, cache_write = 1.25 },
}
app.opt.max_session_cost = 5  -- pause and ask on reaching 5 USD

-- Web search (curl) --------------------------------------------------------
app.opt.web = { blocked_domains = { "pinterest.com" } }

-- Models by role ---------------------------------------------------------
app.models.aliases = {
  rapido = "claude-haiku-5-5",
}
app.models.roles = {
  main    = { id = "sonnet", max_tokens = 16000, thinking = { budget = 8000 },
              fallbacks = { "haiku" } },
  plan    = { id = "opus", thinking = "adaptive" },
  explore = "rapido",                 -- shorthand for { id = "rapido" }
  compact = { max_tokens = 4000, thinking = false },
}

-- Paths the tools cannot read (replaces the default list) ---------------
app.permissions.sensitive_paths = { ".env*", "*.pem", "id_rsa*", ".git/", "*.key" }

-- MCP servers from the global config: they start without asking for trust
app.mcp.add("docs", {
  command = "npx",
  args = { "-y", "@modelcontextprotocol/server-filesystem", "/home/yo/notas" },
  effect = "read",
  timeout = 30,
})

-- Plugins outside ~/.config/crest/plugins/ ------------------------------
app.plugins = { "~/src/mis-plugins/revisor" }

-- Mappings and commands --------------------------------------------------
app.keymap.set("<leader>x", function() app.cmd("clear") end)
app.keymap.set("<C-p>", "picker.files")

app.commands.register("review", function(args)
  app.send("Review the uncommitted changes and list the risks. " .. args, { role = "plan" })
end)
```

And a workspace config that adjusts it:

```lua
-- <workspace>/.crest/init.lua
local app = require("app")

app.opt.max_session_cost = 2          -- stricter in this repo
app.models.roles = { main = { max_tokens = 8000 } }  -- only changes max_tokens
app.permissions.sensitive_paths = { "secrets/" }     -- ADDED to the global list

-- A server from the repo: stays pending until `crest --trust-mcp tickets`
app.mcp = {
  tickets = { url = "https://tickets.example.com/mcp", headers = { Authorization = "Bearer x" } },
}

app.commands.register("tests", function()
  app.send("Run `cargo test --workspace` and summarize the failures in a table")
end)
```

With both together, the result is:

- `main` uses `claude-sonnet-5-5` with `max_tokens = 8000`, a thinking budget of 8000 and the `claude-haiku-5-5` fallback.
- The spending cap is 2 USD.
- The sensitive paths are `.env*`, `*.pem`, `id_rsa*`, `.git/`, `*.key` and `secrets/`.
- `docs` starts and `tickets` stays pending approval.
