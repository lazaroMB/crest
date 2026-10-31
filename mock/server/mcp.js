// The MCP side of the mock backend: the servers of `/mcp`, managed without a restart.
//
//   GET    /api/mcp                   every server, with its tools
//   POST   /api/mcp                   { spec } -> a new server, started at once
//   POST   /api/mcp/test              { spec } -> tries a spec without saving it
//   PATCH  /api/mcp/:name             { spec } -> saved and restarted
//   DELETE /api/mcp/:name
//   POST   /api/mcp/:name/:action     test | restart | login | logout | trust | enable | disable

import { MCP_TOOLS, mcpServers as servers } from "./data.js";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const find = (name) => servers.find((s) => s.name === name);
const toolsOf = (server) => MCP_TOOLS[server.name] ?? MCP_TOOLS[server.toolsFrom] ?? [];

function log(server, level, text) {
  server.log.unshift({ at: Date.now(), level, text });
  server.log.length = Math.min(server.log.length, 30);
}

function view(server) {
  const tools = server.state === "ready" ? toolsOf(server) : [];
  return { ...server, tools, toolCount: tools.length };
}

/** What connecting to `spec` would do, decided from its fields so every outcome can be tried. */
function outcome(spec, authed) {
  const c = spec.config;
  const text = [c.command, ...(c.args ?? []), c.url].join(" ");
  if (/fail|broken|refused/.test(text)) {
    return { state: "failed", message: "the server exited during startup (code 1)", stderr: [`${c.command ?? c.url}: connection refused`] };
  }
  if (spec.transport === "http" && spec.config.oauth && !authed && !/token|bearer/i.test(JSON.stringify(c.headers))) {
    return { state: "needs-login", message: "the server answered 401: log in to authorize crest" };
  }
  return { state: "ready", message: undefined, stderr: undefined };
}

/** Starting → its outcome after a moment, like McpManager::run. */
function start(server) {
  server.state = "starting";
  server.message = undefined;
  server.stderr = undefined;
  const generation = (server.generation = (server.generation ?? 0) + 1);
  setTimeout(() => {
    if (server.generation !== generation || server.state !== "starting") return;
    Object.assign(server, outcome(server, server.authed));
    if (server.state === "ready") log(server, "info", `connected · ${toolsOf(server).length} tools`);
    else log(server, server.state === "failed" ? "error" : "warning", server.message);
  }, 1400);
}

function fromSpec(spec) {
  const known = MCP_TOOLS[spec.name] ? spec.name : spec.transport === "http" ? "github" : "filesystem";
  return { ...spec, source: "crest", toolsFrom: known, log: [], authed: false };
}

function validate(spec, previous) {
  if (!spec.name || !/^[\w-]+$/.test(spec.name)) return "the name takes letters, digits, - and _";
  if (spec.name.includes("__")) return "the name can't contain __ (tools are named mcp__<server>__<tool>)";
  if (spec.name !== previous && find(spec.name)) return `there is already a server named ${spec.name}`;
  if (spec.transport === "stdio" && !spec.config.command?.trim()) return "a stdio server needs a command";
  if (spec.transport === "http" && !/^https?:\/\//.test(spec.config.url ?? "")) return "an http server needs an http(s) url";
  return null;
}

async function test(spec, authed) {
  const started = Date.now();
  await sleep(700 + Math.random() * 600);
  const result = outcome(spec, authed);
  const ms = Date.now() - started;
  if (result.state === "ready") {
    const count = (MCP_TOOLS[spec.name] ?? MCP_TOOLS[spec.toolsFrom] ?? MCP_TOOLS[spec.transport === "http" ? "github" : "filesystem"]).length;
    return { ok: true, text: `connected in ${ms} ms · ${count} tools` };
  }
  if (result.state === "needs-login") return { ok: false, text: "reachable, but it needs a login (401)" };
  return { ok: false, text: result.stderr?.[0] ?? result.message };
}

const logins = new Map(); // server name -> cancel()

const actions = {
  async test(server) {
    const result = await test(server, server.authed);
    log(server, result.ok ? "info" : "error", `test: ${result.text}`);
    return result;
  },
  restart(server) {
    log(server, "info", "restarted");
    start(server);
  },
  async login(server) {
    let cancelled = false;
    logins.set(server.name, () => (cancelled = true));
    await sleep(2600);
    logins.delete(server.name);
    if (cancelled) return { ok: false, text: "login cancelled" };
    server.authed = true;
    log(server, "info", "logged in (oauth)");
    start(server);
    return { ok: true, text: "logged in" };
  },
  cancel(server) {
    logins.get(server.name)?.();
  },
  logout(server) {
    server.authed = false;
    log(server, "info", "logged out · token deleted from the keyring");
    start(server);
  },
  trust(server) {
    log(server, "info", "trusted for this workspace");
    start(server);
  },
  enable(server) {
    log(server, "info", "enabled");
    start(server);
  },
  disable(server) {
    server.generation = (server.generation ?? 0) + 1;
    server.state = "disabled";
    server.message = undefined;
    log(server, "info", "disabled");
  },
};

export function mcpRoutes({ send, readBody }) {
  return [
    ["GET", /^\/api\/mcp$/, (req, res) => send(res, servers.map(view))],
    [
      "POST",
      /^\/api\/mcp\/test$/,
      async (req, res) => {
        const { spec } = await readBody(req);
        send(res, await test(spec, false));
      },
    ],
    [
      "POST",
      /^\/api\/mcp$/,
      async (req, res) => {
        const { spec } = await readBody(req);
        const error = validate(spec);
        if (error) return send(res, { error }, 400);
        const server = fromSpec(spec);
        servers.push(server);
        log(server, "info", "added");
        start(server);
        send(res, view(server));
      },
    ],
    [
      "PATCH",
      /^\/api\/mcp\/([\w-]+)$/,
      async (req, res, name) => {
        const server = find(name);
        if (!server) return send(res, { error: "no such server" }, 404);
        const { spec } = await readBody(req);
        const error = validate(spec, name);
        if (error) return send(res, { error }, 400);
        Object.assign(server, spec);
        log(server, "info", "config saved · restarting");
        start(server);
        send(res, view(server));
      },
    ],
    [
      "DELETE",
      /^\/api\/mcp\/([\w-]+)$/,
      (req, res, name) => {
        const at = servers.findIndex((s) => s.name === name);
        if (at !== -1) servers.splice(at, 1);
        send(res, { ok: true });
      },
    ],
    [
      "POST",
      /^\/api\/mcp\/([\w-]+)\/(\w+)$/,
      async (req, res, name, action) => {
        const server = find(name);
        if (!server || !actions[action]) return send(res, { error: "no such server or action" }, 404);
        const result = await actions[action](server);
        send(res, { ...(result ?? { ok: true }), server: view(server) });
      },
    ],
  ];
}
