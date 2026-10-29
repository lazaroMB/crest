# Configuration

crest is configured with Lua files (`init.lua`). You may have none at all: without a config it starts with the defaults described below. To write plugins (tools, hooks, blocks), see [plugins.md](plugins.md). This guide covers the declarative options.

## Files and paths

| What | Path | Notes |
| --- | --- | --- |
| Global config | `$XDG_CONFIG_HOME/crest/init.lua`, or `~/.config/crest/init.lua` (Windows: `%APPDATA%\crest\init.lua`) | A relative `XDG_CONFIG_HOME` is ignored. macOS uses the same path (not `~/Library`). |
| Plugins | `<global config dir>/plugins/<name>/init.lua` | Loaded in alphabetical order, followed by those listed in `app.plugins`. |
| Workspace config | `<workspace>/.crest/init.lua` | Optional. It may ship with the repository. |
| Theme files | `<global config dir>/themes/<name>.lua` | One theme per file, picked with `app.opt.theme` or `/theme`. The installers copy the [example themes](../examples/themes/) and a commented `init.lua` here when they are missing. See [themes.md](themes.md). |
| Database | `$XDG_DATA_HOME/crest/sessions.db`, or `~/.local/share/crest/sessions.db` (Windows: `%LOCALAPPDATA%\crest\sessions.db`) | SQLite. Stores the chat sessions and the workflow checkpoints. |
| Approved MCP servers | `$XDG_DATA_HOME/crest/mcp-trust.json`, or `~/.local/share/crest/mcp-trust.json` (Windows: under `%LOCALAPPDATA%\crest\`) | Written with `0600` permissions (on Windows the folder's access list, private to the user, applies). |
| Approved plugin capabilities | `$XDG_DATA_HOME/crest/plugin-trust.json`, or `~/.local/share/crest/plugin-trust.json` (Windows: under `%LOCALAPPDATA%\crest\`) | Written with `0600` permissions. See [Plugin capabilities and secrets](#plugin-capabilities-and-secrets). |
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
| `app.opt.window` | Only read in the global config (the window opens before any project). In the workspace it is ignored with a notice. |
| `app.theme.define` | By theme name: the highest-precedence layer wins. |

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

## Model providers and API keys

crest talks to two model providers, each with its own key:

| Account | Providers (model id prefix) | Key |
| --- | --- | --- |
| Anthropic | `anthropic` (the default: ids without prefix, such as `claude-sonnet-5-5`) | `ANTHROPIC_API_KEY`, pay per token on the Anthropic API |
| OpenCode | `opencode-go:` (the Go subscription, flat rate) and `opencode-zen:` (Zen, pay as you go) | `OPENCODE_API_KEY`, from the OpenCode console; the same key serves both |

Anthropic does not let third-party apps use a Claude Pro/Max subscription, so the Anthropic provider needs an API key. OpenCode Go is a subscription meant for "OpenCode and other coding agents": crest identifies itself with `User-Agent: crest/<version>` and sends the `x-opencode-session` header OpenCode requires.

The easiest way to give crest a key: open crest and paste it in the dialog it shows when there is none, or run `/login` (`/login opencode` opens it on OpenCode; `Tab` switches the account). crest checks the key with the provider (`GET /v1/models`), stores it in the system keyring and restarts the agent of the window; a key the provider rejects is not stored, and one that cannot be checked (no network) is stored with a warning. `/logout opencode` (or `anthropic`) deletes one key, `/logout` deletes them all. The key goes from that masked field to the keyring: it is not added to the prompt history nor to the timeline.

crest looks for each key in this order:

1. The environment variable (`ANTHROPIC_API_KEY`, `OPENCODE_API_KEY`), if it is not empty.
2. The system keyring (the `keyring` crate), under the **service `crest`** and the **entry (user)** of the same name.

Keys **never** go in `init.lua`, and they do not appear in logs or traces.

To store one in the keyring:

```bash
# Linux (libsecret / GNOME Keyring, KWallet via Secret Service)
secret-tool store --label="crest" service crest username ANTHROPIC_API_KEY
secret-tool store --label="crest" service crest username OPENCODE_API_KEY

# macOS (Keychain)
security add-generic-password -s crest -a ANTHROPIC_API_KEY -w

# Windows (Credential Manager, a generic credential): use /login, or
cmdkey /generic:ANTHROPIC_API_KEY.crest /user:ANTHROPIC_API_KEY /pass
```

> The commands above are each system's standard tools. crest only reads the `crest` / `<KEY NAME>` entries through `keyring`, and that read has not been tested in this environment against a real keyring.

If no provider has a key:

- the window opens anyway with the `/login` dialog, and the first block is the error "no model provider has an API key: …";
- headless mode exits with code 2.

**The default provider** serves model ids without a prefix (the built-in roles are such Claude ids). crest picks it in this order:

1. `app.opt.providers.default`, when `init.lua` sets it;
2. the account of the last successful `/login` (OpenCode means `opencode-go`, or `opencode-zen` when the key has no Go subscription), if it still has a key;
3. the only account with a key;
4. Anthropic.

So after `/login opencode` with no Anthropic key, OpenCode is the default and the Claude roles are not used. OpenCode has no built-in roles: crest opens the model popup (`/model`, `Ctrl+G m`), which lists the models of your providers' catalogs that your account can use. `Enter` saves the pick, which then serves every role that `init.lua` does not set. Until a model is picked, a prompt is refused with "no model is selected … Pick one with /model" and nothing is sent. With Anthropic as the default, a saved pick replaces the `main` and `component` models and the other roles keep their Claude defaults.

The last login and the picks (one per default provider) are saved in `preferences.json` in crest's data directory (`~/.local/share/crest/` or `%LOCALAPPDATA%\crest\`).

**What an OpenCode key may use.** OpenCode lists every model of a plan to any key, but answers 403 to the ones the account cannot use: Go models without a Go subscription, and Zen's free models (`*-free`, `big-pickle`) outside OpenCode's own client. So the first start after `/login opencode` asks once: one request of one token to a model of each plan (Go, a small paid Zen model, a free Zen model). A 403 rules that plan out: its models leave the catalog and the popup, and if it was the default (Go), Zen takes its place. Any other answer (no network, a timeout) leaves it unknown: its models stay, a plan that answered is preferred to it, and it is asked again at the next start. The answer is saved in `preferences.json` (`opencode_access`); after buying a subscription or credits, `/login opencode` again so crest asks anew.

A 401 means the key is wrong ("authentication failed … run /login"); a 403 means the key works but the account cannot use that model ("your account cannot use this model: …"). Either is kept in the timeline as an error with what to do.

### Model ids and `app.opt.providers`

A model id is `provider:model` (`opencode-go:glm-5.3`, `opencode-zen:claude-sonnet-5-5`); an id without a known prefix goes to the default provider. Fallback chains may cross providers (`fallbacks = { "opencode-go:glm-5.3" }` after a Claude model).

OpenCode serves each model with the API of its family, and crest picks it from the model name: Claude, MiniMax M and Qwen speak Anthropic Messages (`…/anthropic/v1/messages`); GPT, Grok and Muse speak OpenAI Responses (`…/openai/v1/responses`); GLM, Kimi, DeepSeek, MiMo and the other open models speak Chat Completions (`…/openai/v1/chat/completions`). Gemini models use Google's API, which crest does not speak. When the guess is wrong for a model, say it in `formats`:

```lua
app.opt.providers = {
  default = "anthropic",          -- provider of ids without prefix: "anthropic", "opencode-go" or "opencode-zen"
  opencode = {
    formats = { ["minimax-m2.7"] = "chat" },   -- "anthropic", "chat" or "responses"
    -- base_url = "https://opencode.ai/inference",   -- tests only
  },
}
```

`thinking` only reaches Anthropic Messages models (as today) and Responses models (as a reasoning effort: low under 4000 tokens of budget, medium under 16000, high above); Chat Completions models think as their provider decides, and their reasoning shows as thinking.

The models of OpenCode Go cost nothing per token in the cost notices (it is a flat-rate plan) unless `app.opt.pricing` gives them a price; Zen models, like Anthropic ones, have no price unless you configure it.

The Anthropic API URL is fixed (`https://api.anthropic.com`): there is no variable to change it.

## `app.opt` options

| Option | Type | Default | Effect |
| --- | --- | --- | --- |
| `leader` | string in key notation (`"<C-g>"`, `"<C-,>"`, `"<P-,>"`: `<P-…>` is Cmd on macOS and Ctrl elsewhere) | `"<C-g>"` | The value of `<leader>` in keymaps **defined in Lua**. Builtin mappings always use `Ctrl+G` (see [usage.md](usage.md#default-key-bindings)). Use a chord: a plain key would be typed into the prompt. Avoid `<C-Space>` (macOS takes it to switch the input source) and Alt chords (Option composes characters on a Mac). |
| `timeoutlen` | integer, ms | `1000` | How long a prefix (`<leader>`, `<leader>f` of `<leader>ff`) waits for the next key. |
| `pricing` | table `{ [model or prefix] = { input, output, cache_read, cache_write } }` | empty | Prices in USD per million tokens. Without prices, the status bar shows the session's tokens instead of the cost. |
| `max_session_cost` | number > 0 (USD) | no cap | Spending cap per session: when it is reached, crest asks for confirmation before the next model call. |
| `web` | table, see [Web search](#web-search) | DuckDuckGo, 8 results, 20 s, any domain | The `web_search` and `web_fetch` tools: on or off, search backend, limits and domains. |
| `shell` | program name, or table `{ program, args }` | `sh -c`; on Windows `pwsh` (or `powershell`) `-NoProfile -NonInteractive -Command` | The shell of `/!` commands and of plan checks with shell syntax. See [Shell](#shell). |
| `theme` | theme name | `"twocrest"` | The color theme of the window: `"twocrest"` (also `"dark"`), `"twocrest-light"` (also `"light"`), a theme file of `<config dir>/themes/` by its name without `.lua`, or one defined with `app.theme.define`. It wins over the pick of `/theme`. An unknown or broken theme is a startup warning and TwoCrest dark is used. See [themes.md](themes.md). |
| `colors` | table `{ ui = {...}, syntax = {...}, palette = {...} }` | none | Colors over the chosen theme, with the format of a theme file ([themes.md](themes.md#overriding-a-theme-from-initlua)). A bad token or color is a warning and the rest applies. The workspace replaces the global table as a whole. |
| `window` | `"fullscreen"`, `"maximized"`, `"windowed"`, or `{ mode, width, height }` | `"fullscreen"` (Windows: `"maximized"`) | How the window opens; the size (at least 400 px, 1280×800 by default) is the windowed one, also the size full screen and maximized return to. **Global config only**: a workspace `app.opt.window` is ignored with a notice. See [themes.md](themes.md#window-mode). |

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
- Cost is added per response, including compaction summaries. It shows in the status bar: `$0.00` before the model is used, the session's tokens (`48.2k tok`) if a model without a price was used, and otherwise the running total (four decimals below 1 USD).
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

## Decision model (Cloudflare Clef)

A decision model answers typed questions (yes/no, a choice, a score) about a text with probabilities instead of writing an answer. It is much cheaper and faster than a model call, so crest uses it, when it is available, to:

- **judge the problem definitions** of the research flow's Definition stage (instead of a keyword heuristic and a judge call to the `plan` model), and read a free-text answer that accepts them as they are;
- **read free-text answers at the gates** of the research and implementation flows (approve, retry, skip, abort, back to the Spec, or instructions) when they are not one of the fixed phrases;
- **tell side questions from answers** at those gates: a line it reads as a side question with at least 0.8 confidence opens the side chat instead of redoing the stage, and in the side chat a line that says to go back to the flow ("listo, la respuesta es X, continúa") proposes the answer ([side questions](workflows.md#side-questions));
- **route each chat prompt**: offer only the optional tool groups it needs (charts and tables, web, simulation, subagent, each MCP server, each plugin tool; the file tools are always offered) and pick its model by difficulty (a trivial prompt goes to `models.trivial`, a hard one to `models.hard`, a normal one keeps the `main` role).

It is optional: without credentials, or when a call fails or takes longer than `timeout_ms`, everything works as without it.

The model is [Clef](https://developers.cloudflare.com/workers-ai/models/clef-flash/) on Cloudflare Workers AI. It needs an API token with Workers AI permission and the account id, from `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` or from the keyring (service `crest`, entries `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`), like the Anthropic key:

```bash
secret-tool store --label="crest" service crest username CLOUDFLARE_API_TOKEN
secret-tool store --label="crest" service crest username CLOUDFLARE_ACCOUNT_ID
```

```lua
app.opt.decision = {
  enabled = true,              -- false never uses it
  model = "clef-flash",        -- or "clef" (larger, slower, longer context)
  account_id = "…",            -- instead of CLOUDFLARE_ACCOUNT_ID (the token never goes here)
  timeout_ms = 1500,           -- per call (1-30000); a late answer falls back
  tools = true,                -- route the tool groups of each prompt
  model_routing = true,        -- route the model of each prompt
  models = { trivial = "haiku", hard = "opus" },  -- aliases, role names or model ids
}
-- Prices in USD per million tokens (take the current ones from Cloudflare); output is free.
app.opt.pricing = { ["clef-flash"] = { input = 0.038, output = 0 } }
```

- Prompts are routed only in the chat, and only when the turn has no `--model` override or pinned role. A notice says what the routing changed ("Decision model: haiku for a simple prompt; tools left out: web, subagent").
- A tool group the session already used is always offered again, so the history never names a tool the model lacks.
- `/help` shows the decision model in use, or why it is off.

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

Ids may carry a provider prefix (see [Model ids](#model-ids-and-appoptproviders)). With an OpenCode Go subscription, for example:

```lua
app.models.aliases = { glm = "opencode-go:glm-5.3", kimi = "opencode-go:kimi-k3" }
app.models.roles = {
  main    = { id = "glm", fallbacks = { "kimi" } },
  explore = "opencode-go:deepseek-v4.1-flash",
  compact = "opencode-go:deepseek-v4.1-flash",
}
```

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

Aliases are merged with the builtin ones (the workspace wins) and appear in the window's `/model` popup when a provider serves their model.

### Resolution order

1. **Turn override**: `--model`, or `/model <x>` sent as typed (`Esc` hides the popup first).
   - If the value is a **role name** (`plan`), that role's whole spec is used.
   - If it is an alias or an id, it only replaces the `id` and keeps the role's `max_tokens`, `thinking` and `fallbacks`.
   - Aliases are checked before role names.
2. **Role pinned** by a tool or a workflow (for example, the `task` subagent asks for `explore`, and `app.send(..., { role = "plan" })` asks for `plan`).
3. Workspace config.
4. Global config, with its plugins.
5. **The selected model** (`Enter` in the popup, `/model set <id>`): `main` and `component` with Anthropic as the default provider, every role with OpenCode.
6. Builtin values (the table above), only with Anthropic as the default provider.

At startup, crest queries the model list of every provider with a key (`GET /v1/models`) and waits at most 5 s; models are listed with their prefix, and those of the default provider also without it. If a role points to a model that does not exist, a notice is shown and the first available fallback is used. If the catalog does not respond, the notice "could not validate the configured models: …" appears and nothing is validated.

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

## Privacy: secrets and personal data

Every tool result (`read_file`, `grep`, `run_command`, the web tools, MCP and plugin tools) is masked **before anyone reads it**: the model, the timeline, the saved session and the `after_tool` hooks only see `[REDACTED:<kind>]` placeholders. Your prompts are masked as well before they reach the router or the model, and a notice tells you what was masked.

- **Secrets, always on:** AWS, GitHub, Anthropic, OpenAI, Slack, Stripe and Google keys, JWTs, private key blocks (even cut ones), `Bearer`/`Basic` tokens, passwords in URLs (`postgres://user:[REDACTED:password]@db`) and literal values of `password`, `token`, `api_key`, `secret`… assignments. Code such as `let token = next_token()`, `${PASSWORD}`, environment variable names and filler values (`changeme`, `<your-key>`) is left alone.
- **Personal data, on by default:** emails (except `git@…`, `noreply` and `example.com`-style addresses), card numbers (Luhn check), IBANs (mod-97), Spanish DNI/NIE (control letter) and US SSNs, and international phone numbers (`+34 600 123 456`).
- A masked result ends with a note telling the model that the placeholders are not the real content. `write_file` refuses content that carries a placeholder, and so does `edit_file` when `new` adds one, so a masked value never overwrites the real one on disk.

```lua
app.opt.privacy = {
  enabled = true,       -- false masks nothing (global config only)
  prompts = true,       -- also mask your prompts
  pii = { email = true, card = true, iban = true, national_id = true, phone = true },
  patterns = {          -- your own: the whole match, or its first group
    { name = "ticket", regex = "ACME-[0-9]{6}" },
    { name = "pin", regex = "pin=([0-9]{4})" },
  },
  allow = { "me@company.com" },   -- exact values never masked (global config only)
}
```

- **A workspace can only make it stricter.** A `.crest/init.lua` may add `patterns` and set categories or `prompts` to `true`, but its `enabled = false`, its `false` values and its `allow` list are ignored, so a cloned repository cannot switch the protection off.
- An invalid table (unknown field, bad regex) shows a notice and every rule stays on.
- Images in tool results are not inspected. `/open` and `/!` commands show your files and output unmasked: they stay on your machine (the context note of a `/!` command does go to the model).

## Shell

`/!` commands and the checks of a plan that need a shell (pipes, `&&`, globs) run the whole line through one shell. The model's `run_command` never does: it starts programs directly.

```lua
app.opt.shell = "pwsh"                                -- a known shell by name
app.opt.shell = { program = "bash", args = { "-lc" } } -- anything else; the line goes last
```

- Unset: `sh -c` on Linux and macOS; on Windows `pwsh -NoProfile -NonInteractive -Command` when PowerShell 7 is installed, else the bundled Windows PowerShell (`powershell`).
- A name alone gets its usual arguments: `-NoProfile -NonInteractive -Command` for `pwsh` and `powershell`, `/C` for `cmd`, `-c` for anything else (`bash`, `zsh`, Git Bash's `sh`).
- A malformed value is a startup warning and the default is used.

## Compact command output with rtk

[rtk](https://github.com/rtk-ai/rtk) compacts the output of common commands (`git status/log/diff`, `cargo test/build/clippy`, `pytest`, `jest`, `ls`, `docker ps`…) before the model reads it, often by 60-90%. It is off by default:

```lua
app.opt.rtk = { enabled = true, path = "rtk" }   -- or app.opt.rtk = true
```

- Install it with `cargo install --git https://github.com/rtk-ai/rtk rtk` (or from its releases). When it is enabled but not found, crest warns at startup and runs commands as they are.
- Only the model's `run_command` goes through it: for each call crest asks `rtk rewrite "<command>"` and, when rtk has an equivalent, runs that instead. The approval dialog, the permission rules and the read-only classification still use the **original** command. A rewritten line with shell syntax, an rtk deny rule or a failure to start rtk all fall back to the original command.
- The result says `output compacted by rtk`; the model can call `run_command` again with `raw = true` for the full output.
- rtk records its own savings: run `rtk gain` to see them. Output still goes through the privacy layer after rtk.
- `/!` commands, `/git` status, and the commands of the implementation workflow (checks, commits) never go through rtk.

## MCP servers

They are declared in `app.mcp`, either as a table keyed by name or with `app.mcp.add`. All fields are described in [plugins.md](plugins.md#appmcp-mcp-servers).

```lua
app.mcp.add("docs", { command = "mcp-docs", args = { "--root", "." }, effect = "read", timeout = 30 })
app.mcp.add("fs", { command = "npx", args = { "-y", "@modelcontextprotocol/server-filesystem", "." } })
-- A server without login, or with a token in a header (read from the environment):
app.mcp.add("deepwiki", { url = "https://mcp.deepwiki.com/mcp" })
app.mcp.add("context7", { url = "https://mcp.context7.com/mcp", headers = { Authorization = "Bearer ${CONTEXT7_API_KEY}" } })
-- A server with an OAuth login (/mcp login linear):
app.mcp.add("linear", { url = "https://mcp.linear.app/mcp" })
```

- Servers from the **global config** (and its plugins) start when crest opens.
- Those from the **workspace** stay pending, with a notice like "workspace MCP server `x` not started: it would run `…`. If you trust it, start crest with `crest --trust-mcp x` (remembered for this workspace)". They start once you approve them with `crest --trust-mcp <name>` or with `/mcp trust <name>`, which starts it right away.
- The approval is stored per workspace in `mcp-trust.json`, together with the exact transport: if the command, arguments, environment, working directory, URL, headers or OAuth client change, crest asks again. To revoke an approval, delete its entry from the file.
- On startup, crest waits up to 5 s for the servers before the first turn. A slower server (the first `npx -y` downloads its package) keeps starting in the background, for up to `startup_timeout` (120 s by default), and its tools are offered as soon as it is ready; a notice says "MCP server `x` ready (N tools)" or why it failed. The per-call limit for a tool is `timeout` if you declare it, or 120 s otherwise.
- `${VAR}` and `${VAR:-default}` in `args`, `env`, `url` and `headers` are read from crest's environment when the server starts, so tokens stay out of `init.lua`. A variable that is not set keeps the server from starting, with a message naming it.
- Stdio servers run in the workspace root (or `cwd`). When one fails, the message includes the last lines it wrote on stderr.
- A server that sends `tools/list_changed` gets its tools listed again; one whose connection drops is reconnected once.

### `/mcp`

| Command | What it does |
| --- | --- |
| `/mcp` | A table of every server: state (starting, ready · N tools, failed, needs login, disabled, not trusted), transport, config layer, and the error or the next command. |
| `/mcp tools <name>` | The tools of a ready server, with the names the model sees. |
| `/mcp restart <name>` | Closes the session and connects again (after fixing a token, or a server that crashed). |
| `/mcp disable <name>`, `/mcp enable <name>` | Turns a server off or on for this session. `disabled = true` in `init.lua` keeps it off. |
| `/mcp login <name>` | OAuth login of an HTTP server: opens the browser, waits for it on a loopback port, stores the tokens in the system keyring (service `crest`, entry `mcp-oauth:<name>`) and reconnects. Later runs reuse and refresh them. |
| `/mcp logout <name>` | Deletes the stored tokens. |
| `/mcp trust <name>` | Approves a workspace server and starts it. |

### OAuth

Most hosted servers (Linear, Notion, Sentry, Atlassian, Supabase, Cloudflare…) ask for a login: their state is "needs login" and `/mcp login <name>` signs in. crest registers itself with the server (dynamic client registration). Servers that do not allow that, such as GitHub's remote server, need a client you register with them, declared in `oauth`:

```lua
app.mcp.add("github", {
  url = "https://api.githubcopilot.com/mcp/",
  oauth = { client_id = "Iv1.…", client_secret = "…", scopes = { "repo" }, callback_port = 33418 },
})
```

Register `http://127.0.0.1:<callback_port>/callback` as the redirect URI of that client. Most of these servers also take a personal token in an `Authorization` header instead.

What crest does not support: servers that only speak the old HTTP+SSE transport (URLs ending in `/sse`; crest says so when one answers 405), and Gemini-style or other non-MCP endpoints.

### `npx` from the app menu

An app opened from the desktop gets the PATH of a login shell (`~/.profile`, `~/.zprofile`), not of `~/.zshrc`, where asdf, nvm, fnm or volta usually add `node` and `npx`. If a server fails with "`npx` was not found on PATH", use the absolute path (`command = "/home/you/.nvm/versions/node/v24.0.0/bin/npx"`) or add the line that sets up node to `~/.zprofile` or `~/.profile`.

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
| `ANTHROPIC_API_KEY` | The Anthropic API key. Takes priority over the keyring. |
| `OPENCODE_API_KEY` | The OpenCode key (Go and Zen). Takes priority over the keyring. |
| `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` | Credentials of the decision model (see [Decision model](#decision-model-cloudflare-clef)). Take priority over the keyring. |
| `XDG_CONFIG_HOME` | Base of the global config and the plugins (`$XDG_CONFIG_HOME/crest/`). Only used if absolute; also honoured on Windows, where the default is `%APPDATA%`. |
| `XDG_DATA_HOME` | Base of `sessions.db`, `mcp-trust.json` and `plugin-trust.json` (`$XDG_DATA_HOME/crest/`). Only used if absolute; also honoured on Windows, where the default is `%LOCALAPPDATA%`. |
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

-- Look -------------------------------------------------------------------
app.opt.window = { mode = "windowed", width = 1440, height = 900 }
app.opt.theme = "twocrest"  -- or a file of ~/.config/crest/themes/, by name
app.opt.colors = { ui = { caret = "#ffae42" } }

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
app.keymap.set("<leader>X", function() app.cmd("clear") end)
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
