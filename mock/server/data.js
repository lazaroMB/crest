// The mock data: projects, chats and their timelines, the status bar and the / commands.

/** A diff block from a unified diff. */
export function diffBlock(id, path, unified) {
  const lines = unified
    .replace(/^\n/, "")
    .split("\n")
    .map((text) => {
      if (text.startsWith("---") || text.startsWith("+++")) return { kind: "header", text };
      if (text.startsWith("@@")) return { kind: "hunk", text };
      if (text.startsWith("+")) return { kind: "add", text };
      if (text.startsWith("-")) return { kind: "del", text };
      return { kind: "ctx", text };
    });
  return {
    id,
    kind: "diff",
    path,
    added: lines.filter((l) => l.kind === "add").length,
    removed: lines.filter((l) => l.kind === "del").length,
    lines,
  };
}

export const ADD_FIX_DIFF = `
--- a/src/math.rs
+++ b/src/math.rs
@@ -1,7 +1,7 @@
 /// Adds two integers.
 pub fn add(a: i64, b: i64) -> i64 {
-    if b < 0 { a - b.abs() } else { a + b }
+    a + b
 }`;

export const RENAME_DIFF = `
--- a/src/math.rs
+++ b/src/math.rs
@@ -1,4 +1,4 @@
-/// Adds two integers.
-pub fn add(a: i64, b: i64) -> i64 {
+/// Sums two integers.
+pub fn sum(a: i64, b: i64) -> i64 {
     a + b
 }`;

export const SLOWEST_TESTS = {
  kind: "chart",
  title: "Slowest tests (ms)",
  chart: "hbar",
  labels: ["parse_config", "add_negative", "load_session", "render_board", "add_mixed"],
  series: [{ name: "ms", values: [41, 33, 27, 18, 6] }],
};

export const EVIDENCE = {
  kind: "table",
  columns: ["evidence", "source", "confidence"],
  rows: [
    ["add(-2, -3) returns -8", "cargo test", "high"],
    ["the branch on b < 0 subtracts", "src/math.rs:3", "high"],
    ["no test covers two negatives", "tests/math.rs", "medium"],
  ],
};

export const WELCH = {
  kind: "computed",
  method: "Welch's t-test",
  provenance: "crest-stats 0.15.0 · blake3:7f3a…c21e · 3 ms",
  verdict: "verified",
  why: "Why: two independent samples with unequal variances (Levene p = 0.004).",
  values: [
    { label: "mean before", value: "18.42 ms" },
    { label: "mean after", value: "12.97 ms" },
    { label: "t", value: "6.81" },
    { label: "df", value: "71.3" },
    { label: "p", value: "< 0.001" },
    { label: "Cohen's d", value: "1.52" },
  ],
};

export const FIX_REPLY =
  "**Found it.** `add` special-cased negative numbers and subtracted `b.abs()` from `a`, so `add(5, -3)` gave `2`, but `add(-2, -3)` gave `-5 - 3`.\n\n- The branch is gone: `add` is now `a + b`.\n- A regression test covers both signs (`tests/math.rs`).\n- `cargo test` passes: **24 passed**, the slowest test takes 41 ms.";

const bugfix = [
  {
    id: "b1",
    kind: "user",
    text: "add() returns the difference for negative numbers: find out why, fix it and chart how long each test takes",
  },
  {
    id: "b2",
    kind: "thinking",
    summary: "The tests fail only with negative numbers; look at add first.",
    text: "The tests fail only with negative numbers; look at add first.\nadd_negative expects -5 and gets 1, so the sign handling is the suspect.",
  },
  {
    id: "b3",
    kind: "tool",
    name: "grep",
    summary: '"fn add" in src/',
    status: "done",
    input: '{ "pattern": "fn add", "path": "src/" }',
    output: "src/math.rs:2: pub fn add(a: i64, b: i64) -> i64 {",
  },
  { id: "b4", kind: "tool", name: "read_file", summary: "src/math.rs", status: "done" },
  diffBlock("b5", "src/math.rs", ADD_FIX_DIFF),
  {
    id: "b6",
    kind: "tool",
    name: "run_command",
    summary: "cargo test",
    status: "done",
    output: "test result: ok. 24 passed; 0 failed; 0 ignored; finished in 0.41s",
  },
  { id: "b7", ...SLOWEST_TESTS },
  { id: "b8", kind: "text", markdown: FIX_REPLY },
];

const release = [
  { id: "r1", kind: "user", text: "summarize the last release with a chart and a table" },
  { id: "r2", kind: "tool", name: "run_command", summary: "git log v0.5.0..v0.6.0 --stat", status: "done" },
  { id: "r3", kind: "text", markdown: "**v0.6.0 in numbers**: 42 commits, 3 features, 1 fix." },
  {
    id: "r4",
    kind: "chart",
    title: "Lines changed by crate",
    chart: "bar",
    labels: ["ui", "app", "adapters", "domain", "docs"],
    series: [{ name: "lines", values: [2140, 1480, 760, 410, 230] }],
  },
  {
    id: "r5",
    kind: "table",
    columns: ["feature", "crate", "tests"],
    rows: [
      ["3D models in /open", "ui", "31"],
      ["decision model (Clef)", "app", "27"],
      ["prompt history", "ui", "9"],
    ],
    numeric: [false, false, true],
  },
  {
    id: "r6",
    kind: "table",
    columns: ["change", "why it matters"],
    rows: [
      [
        "charts on an own renderer",
        "Bars keep a sane width on wide screens, sit centered in their slot and group by category; lines and areas get a crosshair with one row per series.",
      ],
      [
        "tables that wrap",
        "Long cells wrap instead of being cut, each row takes the height it needs and the columns share the width of the block.",
      ],
      ["a centered column", "Prose stays readable at ~760px; rich blocks get ~1100px."],
    ],
  },
];

const research = [
  { id: "s1", kind: "user", text: "/research add() returns the difference for negative numbers" },
  { id: "s2", kind: "stage", index: 1, total: 14, name: "Intake" },
  { id: "s3", kind: "stage", index: 2, total: 14, name: "Observation" },
  { id: "s4", kind: "tool", name: "grep", summary: '"fn add" in src/', status: "done" },
  { id: "s5", kind: "stage", index: 3, total: 14, name: "Data" },
  { id: "s6", ...EVIDENCE },
  { id: "s7", kind: "stage", index: 4, total: 14, name: "Analysis" },
  {
    id: "s8",
    kind: "text",
    markdown:
      "The failure is local to `add`: the `b < 0` branch was meant for subtraction and leaked into addition. Callers never pass two negatives in production, which is why it went unnoticed.",
  },
  { id: "s9", kind: "stage", index: 5, total: 14, name: "Hypotheses" },
];

const rename = [
  { id: "n1", kind: "user", text: "rename add to sum everywhere" },
  { id: "n2", kind: "tool", name: "grep", summary: '"add(" in src/ tests/', status: "done" },
  { id: "n3", kind: "text", markdown: "`add` is used in two files; first `src/math.rs`:" },
  { id: "n4", kind: "tool", name: "edit_file", summary: "src/math.rs", status: "awaiting approval" },
];

const stats = [
  {
    id: "t1",
    kind: "user",
    text: "is the new parser faster? compare the two benchmark runs in bench/before.csv and bench/after.csv",
  },
  { id: "t2", kind: "tool", name: "show_dataset", summary: "bench/before.csv, bench/after.csv", status: "done" },
  { id: "t3", ...WELCH },
  {
    id: "t4",
    kind: "text",
    markdown:
      "Yes: the new parser is faster, by **5.45 ms** on average (p < 0.001, a large effect). That is roughly a 30% cut in parse time.",
    unverifiedNumbers: 1,
  },
];

const failed = [
  { id: "f1", kind: "user", text: "write the changelog for v0.18.0" },
  { id: "f2", kind: "tool", name: "run_command", summary: "git log v0.17.0..HEAD --oneline", status: "done" },
  {
    id: "f3",
    kind: "error",
    message: "Anthropic API: overloaded (529) after 3 retries",
    hint: "try again, or /model to pick another model",
  },
];

export const WIDE_CHART = {
  kind: "chart",
  title: "Requests by endpoint (last 7d)",
  chart: "bar",
  labels: ["/turn", "/chats", "/workbench", "/approval", "/commands", "/status", "/interrupt", "/health"],
  series: [{ name: "requests", values: [18400, 9200, 7100, 4300, 2600, 1800, 640, 120] }],
};

export const BIG_TABLE = {
  kind: "table",
  columns: ["route", "method", "median ms", "p99 ms", "calls"],
  rows: [
    ["/api/turn", "GET", "12", "340", "18400"],
    ["/api/chats", "GET", "3", "22", "9200"],
    ["/api/workbench", "GET", "4", "19", "7100"],
    ["/api/approval/:id", "POST", "2", "11", "4300"],
    ["/api/commands", "GET", "1", "6", "2600"],
    ["/api/status", "PATCH", "2", "9", "1800"],
    ["/api/interrupt/:chatId", "POST", "2", "8", "640"],
    ["/api/chats/:id", "PATCH", "3", "14", "410"],
    ["/api/chats/:id", "DELETE", "3", "15", "205"],
    ["/api/chats", "POST", "4", "18", "190"],
    ["/healthz", "GET", "1", "4", "120"],
    ["/api/workbench", "HEAD", "1", "3", "88"],
    ["/api/turn", "OPTIONS", "1", "2", "40"],
    ["/api/chats/:id", "OPTIONS", "1", "2", "19"],
    ["/api/approval/:id", "OPTIONS", "1", "2", "7"],
  ],
  numeric: [false, false, true, true, true],
};

const kitchen = [
  { id: "k01", kind: "user", text: "show me every block kind crest can render, then let's scroll through a bigger table and chart" },
  {
    id: "k02",
    kind: "thinking",
    summary: "One of each block, then a wide table and chart to exercise scroll and the outline rail.",
    text: "One of each block, then a wide table and chart to exercise scroll and the outline rail.\nKeep the statuses varied: done, running, awaiting approval, failed, denied.",
  },
  { id: "k03", kind: "tool", name: "grep", summary: '"kind:" in src/components/blocks/', status: "done", input: '{ "pattern": "kind:" }', output: "10 files, one block kind each" },
  { id: "k04", kind: "tool", name: "read_file", summary: "src/components/blocks/index.js", status: "done" },
  diffBlock("k05", "src/components/blocks/index.js", RENAME_DIFF),
  { id: "k06", kind: "tool", name: "run_command", summary: "npm run dev", status: "running" },
  { id: "k07", kind: "stage", index: 1, total: 5, name: "Intake" },
  { id: "k08", ...EVIDENCE },
  { id: "k09", kind: "stage", index: 3, total: 5, name: "Data" },
  { id: "k10", ...SLOWEST_TESTS },
  { id: "k11", ...WELCH },
  {
    id: "k12",
    kind: "text",
    markdown:
      "## Block kinds\n\nCrest renders **ten** kinds of block: `user`, `text`, `thinking`, `tool`, `diff`, `chart`, `table`, `stage`, `error` and `computed`.\n\n- Prose blocks stay at *~760px*.\n- Rich blocks (`diff`, `chart`, `table`, `computed`) get the wide column, *~1100px*.\n\n1. One of each above this paragraph.\n2. A bigger table and chart below, to scroll through.\n\n```rust\npub fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\n```",
  },
  { id: "k13", kind: "tool", name: "edit_file", summary: "src/components/blocks/index.js", status: "awaiting approval" },
  { id: "k14", kind: "error", message: "crest-stats 0.15.0: sample size too small for Welch's t-test (n=4)", hint: "collect at least 10 runs, or /back to Data" },
  { id: "k15", kind: "user", text: "now show a long table and a tall chart so the outline marks are far apart" },
  { id: "k16", ...BIG_TABLE },
  { id: "k17", ...WIDE_CHART },
  { id: "k18", kind: "tool", name: "run_command", summary: "cargo test --test negatives", status: "failed", output: "test add_negative ... FAILED\nassertion failed: `(left == right)`" },
  { id: "k19", kind: "tool", name: "edit_file", summary: "src/math.rs (outside the workspace)", status: "denied" },
  {
    id: "k20",
    kind: "text",
    markdown:
      "That covers every block kind, both table sizes (3 rows and 15 rows, which collapses past 12), both chart orientations (`hbar` and `bar`), and five tool statuses (`done`, `running`, `awaiting approval`, `failed`, `denied`).",
  },
  { id: "k21", kind: "user", text: "last one: a short reply near the bottom, to check the outline jumps here too" },
  { id: "k22", kind: "text", markdown: "Done — this is the third mark on the rail." },
];

const chat = (id, projectId, title, blocks = []) => ({ id, projectId, title, blocks });

export const chats = {
  "c-bugfix": chat("c-bugfix", "p-crest", "Fix add() for negative numbers", bugfix),
  "c-rename": chat("c-rename", "p-crest", "Rename add to sum", rename),
  "c-research": chat("c-research", "p-crest", "Research: negative numbers", research),
  "c-release": chat("c-release", "p-crest", "Release v0.6.0 in numbers", release),
  "c-failed": chat("c-failed", "p-crest", "Changelog for v0.18.0", failed),
  "c-stats": chat("c-stats", "p-lab", "Is the new parser faster?", stats),
  "c-kitchen": chat("c-kitchen", "p-crest", "Every block kind", kitchen),
  "c-site-1": chat("c-site-1", "p-site", "Hero section copy"),
  "c-site-2": chat("c-site-2", "p-site", "Lighthouse below 90"),
};

const summary = (id, activity) => ({ id, title: chats[id].title, activity });

export const projects = [
  {
    id: "p-crest",
    name: "crest",
    path: "~/dev/crest",
    chats: [
      summary("c-bugfix"),
      summary("c-rename", "waiting"),
      summary("c-research", "running"),
      summary("c-release"),
      summary("c-failed"),
      summary("c-kitchen"),
    ],
  },
  { id: "p-lab", name: "lab-notes", path: "~/dev/lab-notes", chats: [summary("c-stats")] },
  { id: "p-site", name: "twocrest-site", path: "~/dev/twocrest-site", chats: [summary("c-site-1"), summary("c-site-2")] },
];

export const status = {
  model: "claude-sonnet-5-5",
  mode: "ask",
  cost: 0.0735,
  git: { branch: "fix/add-negatives", staged: 0, modified: 2, untracked: 0, conflicts: 0, ahead: 1, behind: 0 },
  workspace: "crest",
};

export const commands = [
  { usage: "/help [section|search]", description: "Open the help on a section, or filtered by a search" },
  { usage: "/model [alias]", description: "Model of the next turn only (alias, role or id); no argument clears it" },
  { usage: "/mode <ask|auto-edit|plan>", description: "Change the permission mode" },
  { usage: "/clear", description: "Start a new, empty session (not while a turn runs)" },
  { usage: "/project", description: "Close this project and choose another one" },
  { usage: "/open @<path>", description: "Show a workspace file in a modal, 3D models included" },
  { usage: "/! <command>", description: "Run a shell command in the workspace; its output goes to the model" },
  { usage: "/research <problem>", description: "Start the research flow on a problem (5 stages, depth picked by the model)" },
  { usage: "/research --deep <problem>", description: "The full 14-stage flow, with experiments and a prototype" },
  { usage: "/research resume [n]", description: "Continue the last research flow" },
  { usage: "/research list [all]", description: "The open research flows of this workspace, newest first" },
  { usage: "/simulate <spec>", description: "Simulate the solution of a Spec and open the 3D board" },
  { usage: "/implement <spec>", description: "Run the plan of a Spec task by task, one commit per task" },
  { usage: "/stage", description: "Where the active flow is (research or implementation)" },
  { usage: "/back <stage>", description: "Take the research flow back to a stage, by number or name" },
  { usage: "/skip", description: "Skip an optional stage of the research flow" },
  { usage: "/retry", description: "Run the failed stage of the research flow again" },
  { usage: "/abort", description: "Stop the active flow for good (its journal is kept)" },
  { usage: "/aside [question]", description: "Chat aside while a flow waits at a gate" },
  { usage: "/flow [answer]", description: "Back to the waiting flow: answers it" },
  { usage: "/permissions", description: "List and remove the permission rules" },
  { usage: "/theme", description: "Pick a theme, previewed in the window" },
  { usage: "/mcp", description: "Manage the MCP servers: add, edit, test, log in, enable…" },
  { usage: "/mcp add", description: "Add an MCP server (stdio or http), applied without a restart" },
  { usage: "/mcp tools <name>", description: "The tools of an MCP server" },
  { usage: "/mcp [test|restart|login|logout|trust|enable|disable|remove] <name>", description: "Act on one MCP server" },
  { usage: "/plugin list", description: "List the plugins that declare capabilities" },
  { usage: "/login", description: "Store your API key in the system keyring" },
  { usage: "/logout", description: "Delete the API key from the system keyring" },
  { usage: "/q", description: "Quit crest (also /quit and /qa)" },
];

// MCP servers (crates/adapters/mcp): what `/mcp` lists, with their tools.
// `source` says who declared it: "init.lua" (read-only here) or "crest" (made in the UI).

const tool = (name, description, params = []) => ({ name, description, params });
const param = (name, type, description, required = true) => ({ name, type, description, required });

export const MCP_TOOLS = {
  github: [
    tool("search_repositories", "Search GitHub repositories", [param("query", "string", "GitHub search syntax"), param("per_page", "number", "Results per page", false)]),
    tool("get_file_contents", "Read a file or directory from a repository", [param("owner", "string", "Repository owner"), param("repo", "string", "Repository name"), param("path", "string", "Path in the repository")]),
    tool("create_issue", "Open an issue", [param("owner", "string", "Repository owner"), param("repo", "string", "Repository name"), param("title", "string", "Issue title"), param("body", "string", "Markdown body", false)]),
    tool("list_issues", "List the issues of a repository", [param("owner", "string", "Repository owner"), param("repo", "string", "Repository name"), param("state", "open | closed | all", "Filter by state", false)]),
    tool("create_pull_request", "Open a pull request", [param("owner", "string", "Repository owner"), param("repo", "string", "Repository name"), param("head", "string", "Branch with the changes"), param("base", "string", "Branch to merge into")]),
    tool("get_pull_request_diff", "The unified diff of a pull request", [param("owner", "string", "Repository owner"), param("repo", "string", "Repository name"), param("number", "number", "Pull request number")]),
    tool("add_issue_comment", "Comment on an issue or pull request", [param("number", "number", "Issue number"), param("body", "string", "Markdown body")]),
    tool("list_commits", "Commits of a branch", [param("sha", "string", "Branch or commit", false)]),
  ],
  filesystem: [
    tool("read_file", "Read a file as text", [param("path", "string", "Path inside an allowed directory")]),
    tool("write_file", "Create or overwrite a file", [param("path", "string", "Path"), param("content", "string", "New contents")]),
    tool("list_directory", "Entries of a directory", [param("path", "string", "Directory")]),
    tool("search_files", "Find files by glob", [param("path", "string", "Where to start"), param("pattern", "string", "Glob")]),
    tool("get_file_info", "Size, times and permissions", [param("path", "string", "Path")]),
    tool("move_file", "Move or rename", [param("source", "string", "From"), param("destination", "string", "To")]),
  ],
  linear: [
    tool("list_issues", "Issues assigned to you or matching a filter", [param("filter", "string", "Linear filter", false)]),
    tool("create_issue", "Create an issue in a team", [param("team", "string", "Team key"), param("title", "string", "Title")]),
    tool("update_issue", "Change status, assignee or estimate", [param("id", "string", "Issue id")]),
  ],
  postgres: [
    tool("query", "Run a read-only SQL query", [param("sql", "string", "SQL")]),
    tool("list_tables", "Tables of a schema", [param("schema", "string", "Schema", false)]),
  ],
  "repo-tools": [
    tool("run_migrations", "Apply the pending database migrations"),
    tool("seed", "Load the development fixtures", [param("profile", "string", "Fixture profile", false)]),
  ],
  browser: [
    tool("navigate", "Open a URL", [param("url", "string", "URL")]),
    tool("screenshot", "Capture the page", [param("full_page", "boolean", "Whole page", false)]),
    tool("click", "Click an element", [param("selector", "string", "CSS selector")]),
  ],
};

const at = (minutesAgo) => Date.now() - minutesAgo * 60_000;

export const mcpServers = [
  {
    name: "github",
    transport: "http",
    origin: "global",
    source: "crest",
    effect: "network",
    timeout: 60,
    startupTimeout: 30,
    config: {
      url: "https://api.githubcopilot.com/mcp/",
      headers: [],
      oauth: { clientId: "", clientSecret: "", scopes: "repo read:org", callbackPort: "" },
    },
    state: "ready",
    authed: true,
    log: [
      { at: at(42), level: "info", text: "connected · 8 tools" },
      { at: at(43), level: "info", text: "logged in as lazaro (oauth)" },
    ],
  },
  {
    name: "filesystem",
    transport: "stdio",
    origin: "global",
    source: "init.lua",
    effect: "write",
    timeout: 30,
    startupTimeout: 10,
    config: {
      command: "npx",
      args: ["-y", "@modelcontextprotocol/server-filesystem", "~/Projects"],
      env: [],
      cwd: "",
    },
    state: "ready",
    log: [{ at: at(42), level: "info", text: "connected · 6 tools" }],
  },
  {
    name: "linear",
    transport: "http",
    origin: "global",
    source: "crest",
    effect: "network",
    timeout: 60,
    startupTimeout: 30,
    config: { url: "https://mcp.linear.app/mcp", headers: [], oauth: { clientId: "", clientSecret: "", scopes: "", callbackPort: "" } },
    state: "needs-login",
    message: "the server answered 401: log in to authorize crest",
    log: [{ at: at(42), level: "warning", text: "401 Unauthorized · needs login" }],
  },
  {
    name: "postgres",
    transport: "stdio",
    origin: "workspace",
    source: "crest",
    effect: "read",
    timeout: 30,
    startupTimeout: 10,
    config: {
      command: "uvx",
      args: ["mcp-server-postgres", "${DATABASE_URL}"],
      env: [{ key: "DATABASE_URL", value: "${DATABASE_URL:-postgres://localhost:5432/app}" }],
      cwd: "",
    },
    state: "failed",
    message: "the server exited during startup (code 1)",
    stderr: [
      "Traceback (most recent call last):",
      '  File "server.py", line 31, in connect',
      "psycopg.OperationalError: connection to server at \"localhost\" (::1), port 5432 failed:",
      "Connection refused. Is the server running on that host and accepting TCP/IP connections?",
    ],
    log: [
      { at: at(41), level: "error", text: "retry failed: exited with code 1" },
      { at: at(42), level: "error", text: "exited with code 1 · connection refused" },
    ],
  },
  {
    name: "repo-tools",
    transport: "stdio",
    origin: "workspace",
    source: "init.lua",
    effect: "execute",
    timeout: 120,
    startupTimeout: 20,
    config: { command: "./scripts/mcp-server", args: ["--stdio"], env: [{ key: "APP_ENV", value: "development" }], cwd: "." },
    state: "untrusted",
    message: "declared by this workspace (.crest/init.lua): approve the command before it runs",
    log: [],
  },
  {
    name: "browser",
    transport: "stdio",
    origin: "global",
    source: "crest",
    effect: "network",
    timeout: 60,
    startupTimeout: 20,
    config: { command: "npx", args: ["-y", "@playwright/mcp@latest", "--headless"], env: [], cwd: "" },
    state: "disabled",
    log: [{ at: at(60 * 24), level: "info", text: "disabled" }],
  },
];
