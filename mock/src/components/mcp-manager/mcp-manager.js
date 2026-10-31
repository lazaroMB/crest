// "MCP servers": list, inspect, test, log in, enable, add, edit and remove the
// MCP servers, all live (no restart). Opened from the Chats footer, `/mcp` or <C-g>m.
//
//   ┌ MCP servers ─────────────────────────────────────────── × ┐
//   │ filter      │ ● github  ready · 8 tools        [enabled ●] │
//   │ GLOBAL      │ Test  Restart  Log in again  Log out  Edit … │
//   │ ● github    │ Overview  Tools 8  Log                       │
//   │ ● linear    │ …                                            │
//   │ WORKSPACE   │                                              │
//   │ + Add server│                                              │
//   └ ↑↓ server · Tab tabs · a add · e edit · … · Esc close ─────┘

import { h } from "../../lib/h.js";
import {
  askDeleteMcp,
  cancelDeleteMcp,
  cancelMcpLogin,
  closeMcp,
  deleteMcp,
  loginMcp,
  revealMcpSecret,
  runMcp,
  selectMcp,
  setMcpFilter,
  setMcpTab,
  setMcpToolFilter,
  startMcpForm,
  toggleMcpEnabled,
  toggleMcpTool,
  visibleMcp,
} from "../../actions.js";
import { McpForm } from "./mcp-form.js";
import "./mcp-manager.scss";

export const MCP_STATE = {
  starting: { label: "starting", tone: "busy" },
  ready: { label: "ready", tone: "ok" },
  failed: { label: "failed", tone: "error" },
  "needs-login": { label: "needs login", tone: "warn" },
  untrusted: { label: "untrusted", tone: "warn" },
  disabled: { label: "disabled", tone: "off" },
  stopped: { label: "stopped", tone: "off" },
};

export const Dot = (state) => h("span", { class: ["mcp-dot", `mcp-dot--${MCP_STATE[state]?.tone ?? "off"}`] });

const Chip = (text, kind) => h("span", { class: ["mcp-chip", kind && `mcp-chip--${kind}`] }, text);

const Button = (label, onClick, { primary, danger, disabled, title, key } = {}) =>
  h(
    "button",
    {
      class: ["mcp-btn", primary && "mcp-btn--primary", danger && "mcp-btn--danger"],
      disabled: disabled || null,
      title,
      onClick,
    },
    label,
    key && h("span", { class: "mcp-btn__key" }, key),
  );

const ago = (at) => {
  const s = Math.round((Date.now() - at) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
};

const where = (server) =>
  server.source === "init.lua"
    ? server.origin === "global"
      ? "~/.config/crest/init.lua"
      : ".crest/init.lua"
    : server.origin === "global"
      ? "~/.config/crest/mcp.json"
      : ".crest/mcp.json";

// The list.

function ServerRow(server, selected) {
  const info = MCP_STATE[server.state];
  return h(
    "div",
    {
      class: ["mcp-row", selected && "mcp-row--selected", server.state === "disabled" && "mcp-row--off"],
      onClick: () => selectMcp(server.name),
    },
    Dot(server.state),
    h("span", { class: "mcp-row__name" }, server.name),
    h("span", { class: "mcp-row__meta" }, server.state === "ready" ? `${server.toolCount}` : info.tone === "ok" ? "" : info.label),
  );
}

function ServerList(state) {
  const ui = state.ui.mcp;
  const list = visibleMcp();
  const group = (title, servers) =>
    servers.length > 0 && [h("div", { class: "mcp-list__group" }, title), servers.map((s) => ServerRow(s, s.name === ui.selected))];
  return h(
    "div",
    { class: "mcp-list" },
    h("input", {
      class: "mcp-input mcp-list__filter",
      "data-key": "mcp-filter",
      placeholder: "Filter servers",
      value: ui.filter,
      onInput: (e) => setMcpFilter(e.target.value),
    }),
    h(
      "div",
      { class: "mcp-list__items", "data-scroll": "mcp-list" },
      group(
        "Global",
        list.filter((s) => s.origin === "global"),
      ),
      group(
        "This workspace",
        list.filter((s) => s.origin !== "global"),
      ),
      !list.length && h("div", { class: "mcp-empty" }, state.mcp.length ? "No server matches" : "No MCP servers yet"),
    ),
    h("button", { class: "mcp-list__add", onClick: () => startMcpForm() }, "+ Add server", h("span", { class: "mcp-btn__key" }, "a")),
  );
}

// The detail of one server.

function StateBanner(server, ui) {
  const pending = ui.pending[server.name];
  if (pending === "login") {
    return h(
      "div",
      { class: "mcp-banner mcp-banner--busy" },
      h("span", { class: "mcp-spinner" }),
      h(
        "div",
        { class: "mcp-banner__text" },
        h("strong", "Waiting for the browser…"),
        h("div", { class: "muted" }, `crest opened ${server.config.url?.replace(/\/mcp\/?$/, "")}/authorize — finish the login there.`),
      ),
      Button("Cancel", () => cancelMcpLogin(server.name)),
    );
  }
  switch (server.state) {
    case "starting":
      return h("div", { class: "mcp-banner mcp-banner--busy" }, h("span", { class: "mcp-spinner" }), h("span", "Starting… its tools appear when it is ready"));
    case "needs-login":
      return h(
        "div",
        { class: "mcp-banner mcp-banner--warn" },
        h("div", { class: "mcp-banner__text" }, h("strong", "Needs a login"), h("div", { class: "muted" }, server.message)),
        Button("Log in", () => loginMcp(server.name), { primary: true, key: "l" }),
      );
    case "untrusted":
      return h(
        "div",
        { class: "mcp-banner mcp-banner--warn mcp-banner--column" },
        h("div", { class: "mcp-banner__text" }, h("strong", "Not trusted yet"), h("div", { class: "muted" }, server.message)),
        h("pre", { class: "mcp-code" }, [server.config.command, ...server.config.args].join(" ")),
        h("div", { class: "mcp-actions" }, Button("Trust and start", () => runMcp(server.name, "trust"), { primary: true })),
      );
    case "failed":
      return h(
        "div",
        { class: "mcp-banner mcp-banner--error mcp-banner--column" },
        h("div", { class: "mcp-banner__text" }, h("strong", "Failed"), h("div", { class: "muted" }, server.message)),
        server.stderr && h("pre", { class: "mcp-code mcp-code--error" }, server.stderr.join("\n")),
        h(
          "div",
          { class: "mcp-actions" },
          Button("Retry", () => runMcp(server.name, "restart"), { primary: true }),
          server.source !== "init.lua" && Button("Edit config", () => startMcpForm(server.name)),
        ),
      );
    case "disabled":
      return h("div", { class: "mcp-banner" }, h("span", { class: "muted" }, "Disabled — the model doesn't see its tools. Turn it on to start it."));
    default:
      return null;
  }
}

function Actions(server, ui) {
  const pending = ui.pending[server.name];
  const busy = Boolean(pending) || server.state === "starting";
  const off = server.state === "disabled" || server.state === "untrusted";
  const oauth = server.transport === "http" && server.config.oauth;
  const locked = server.source === "init.lua";
  const lockedHint = "Declared in init.lua — edit it there";
  return h(
    "div",
    { class: "mcp-actions" },
    Button(pending === "test" ? "Testing…" : "Test", () => runMcp(server.name, "test"), { disabled: busy || off, key: "t" }),
    Button("Restart", () => runMcp(server.name, "restart"), { disabled: busy || off, key: "r" }),
    oauth && server.authed && Button("Log in again", () => loginMcp(server.name), { disabled: busy || off, key: "l" }),
    oauth && server.authed && Button("Log out", () => runMcp(server.name, "logout"), { disabled: busy || off }),
    h("span", { class: "mcp-actions__gap" }),
    Button("Edit", () => startMcpForm(server.name), { disabled: locked, title: locked ? lockedHint : null, key: "e" }),
    Button("Delete", askDeleteMcp, { danger: true, disabled: locked, title: locked ? lockedHint : null, key: "d" }),
  );
}

function Result(result) {
  if (!result) return null;
  return h(
    "div",
    { class: ["mcp-result", result.ok ? "mcp-result--ok" : "mcp-result--error"] },
    h("span", result.ok ? "✓" : "✗"),
    h("span", result.text),
    h("span", { class: "muted" }, ago(result.at)),
  );
}

function ConfirmDelete(server) {
  return h(
    "div",
    { class: "mcp-confirm" },
    h("div", h("strong", `Delete ${server.name}?`), " ", h("span", { class: "muted" }, `It is removed from ${where(server)} and its tools are gone from every chat.`)),
    h("div", { class: "mcp-actions" }, Button("Delete", () => deleteMcp(server.name), { danger: true, primary: true, key: "↵" }), Button("Cancel", cancelDeleteMcp, { key: "esc" })),
  );
}

function Secret(server, key, value, ui) {
  const id = `${server.name}/${key}`;
  const secret = !/^\$\{/.test(value) && /key|token|secret|pass|auth/i.test(key);
  if (!secret) return h("span", value);
  return h(
    "span",
    { class: "mcp-secret" },
    ui.revealed[id] ? value : "•".repeat(Math.min(12, value.length || 8)),
    h("button", { class: "mcp-link", onClick: () => revealMcpSecret(id) }, ui.revealed[id] ? "hide" : "show"),
  );
}

const Field = (label, ...value) => h("div", { class: "mcp-field" }, h("div", { class: "mcp-field__label" }, label), h("div", { class: "mcp-field__value" }, ...value));

function Overview(server, ui) {
  const c = server.config;
  const pairs = (rows) =>
    rows.length ? rows.map((r) => h("div", { class: "mcp-pair" }, h("span", { class: "mcp-pair__key" }, r.key), "=", Secret(server, r.key, r.value, ui))) : h("span", { class: "muted" }, "none");
  return h(
    "div",
    { class: "mcp-overview" },
    server.transport === "stdio"
      ? [
          Field("Command", h("code", { class: "mcp-code mcp-code--inline" }, [c.command, ...c.args].join(" "))),
          Field("Environment", pairs(c.env)),
          Field("Working dir", c.cwd || h("span", { class: "muted" }, "the workspace")),
        ]
      : [
          Field("URL", h("code", { class: "mcp-code mcp-code--inline" }, c.url)),
          Field("Headers", pairs(c.headers)),
          Field(
            "OAuth",
            c.oauth
              ? [
                  c.oauth.clientId ? `client ${c.oauth.clientId}` : "dynamic registration",
                  c.oauth.scopes && h("span", { class: "muted" }, ` · scopes ${c.oauth.scopes}`),
                  h("span", { class: "muted" }, server.authed ? " · token in the keyring" : " · not logged in"),
                ]
              : h("span", { class: "muted" }, "no"),
          ),
        ],
    Field("Effect", server.effect, h("span", { class: "muted" }, " · how the permission rules treat its tools")),
    Field("Timeouts", `${server.timeout} s per call · ${server.startupTimeout} s to start`),
    Field("Tool names", h("code", { class: "mcp-code mcp-code--inline" }, `mcp__${server.name}__<tool>`)),
    Field("Saved in", h("code", { class: "mcp-code mcp-code--inline" }, where(server)), server.source === "init.lua" && h("span", { class: "muted" }, " · read-only here")),
  );
}

function Tools(server, ui) {
  if (server.state !== "ready") return h("div", { class: "mcp-empty" }, "The tools show once the server is ready.");
  const filter = ui.toolFilter.trim().toLowerCase();
  const tools = server.tools.filter((t) => !filter || t.name.includes(filter) || t.description.toLowerCase().includes(filter));
  return h(
    "div",
    { class: "mcp-tools" },
    h("input", {
      class: "mcp-input",
      "data-key": "mcp-tool-filter",
      placeholder: `Filter ${server.tools.length} tools`,
      value: ui.toolFilter,
      onInput: (e) => setMcpToolFilter(e.target.value),
    }),
    tools.map((t) => {
      const open = ui.openTool === t.name;
      return h(
        "div",
        { class: ["mcp-tool", open && "mcp-tool--open"] },
        h(
          "div",
          { class: "mcp-tool__head", onClick: () => toggleMcpTool(t.name) },
          h("span", { class: "mcp-tool__marker" }, open ? "▾" : "▸"),
          h("span", { class: "mcp-tool__name" }, h("span", { class: "muted" }, `mcp__${server.name}__`), t.name),
          h("span", { class: "mcp-tool__desc" }, t.description),
        ),
        open &&
          h(
            "div",
            { class: "mcp-tool__params" },
            t.params.length
              ? t.params.map((p) =>
                  h(
                    "div",
                    { class: "mcp-param" },
                    h("span", { class: "mcp-param__name" }, p.name, p.required ? "" : "?"),
                    h("span", { class: "mcp-param__type" }, p.type),
                    h("span", { class: "muted" }, p.description),
                  ),
                )
              : h("span", { class: "muted" }, "no arguments"),
          ),
      );
    }),
    !tools.length && h("div", { class: "mcp-empty" }, "No tool matches"),
  );
}

function Log(server) {
  if (!server.log.length) return h("div", { class: "mcp-empty" }, "Nothing yet.");
  return h(
    "div",
    { class: "mcp-log" },
    server.log.map((entry) =>
      h(
        "div",
        { class: "mcp-log__row" },
        h("span", { class: "mcp-log__at" }, ago(entry.at)),
        h("span", { class: `mcp-log__level mcp-log__level--${entry.level}` }, entry.level),
        h("span", entry.text),
      ),
    ),
  );
}

function Detail(server, ui) {
  const info = MCP_STATE[server.state];
  const tab = ui.tab;
  const tabs = [
    ["overview", "Overview"],
    ["tools", `Tools${server.state === "ready" ? ` ${server.toolCount}` : ""}`],
    ["log", "Log"],
  ];
  const enabled = server.state !== "disabled";
  return h(
    "div",
    { class: "mcp-detail" },
    h(
      "div",
      { class: "mcp-detail__head" },
      Dot(server.state),
      h("strong", { class: "mcp-detail__name" }, server.name),
      h("span", { class: `mcp-state mcp-state--${info.tone}` }, server.state === "ready" ? `ready · ${server.toolCount} tools` : info.label),
      Chip(server.transport),
      Chip(server.origin === "global" ? "global" : "workspace"),
      server.source === "init.lua" && Chip("init.lua", "lua"),
      h("span", { class: "mcp-actions__gap" }),
      h(
        "button",
        {
          class: ["mcp-switch", enabled && "mcp-switch--on"],
          role: "switch",
          "aria-checked": String(enabled),
          disabled: ui.pending[server.name] || server.state === "untrusted" || null,
          title: "Space",
          onClick: () => toggleMcpEnabled(server.name),
        },
        h("span", { class: "mcp-switch__track" }, h("span", { class: "mcp-switch__thumb" })),
        enabled ? "Enabled" : "Disabled",
      ),
    ),
    StateBanner(server, ui),
    ui.confirmDelete ? ConfirmDelete(server) : Actions(server, ui),
    Result(ui.results[server.name]),
    h(
      "div",
      { class: "mcp-tabs" },
      tabs.map(([id, label]) => h("button", { class: ["mcp-tab", tab === id && "mcp-tab--on"], onClick: () => setMcpTab(id) }, label)),
    ),
    h("div", { class: "mcp-detail__body", "data-scroll": `mcp-body-${tab}` }, tab === "overview" ? Overview(server, ui) : tab === "tools" ? Tools(server, ui) : Log(server)),
  );
}

export function McpManager(state) {
  const ui = state.ui.mcp;
  if (!ui.open) return null;
  const server = state.mcp.find((s) => s.name === ui.selected);
  const keys = ui.form
    ? "Tab next field · Ctrl+Enter save · Esc cancel"
    : "↑↓ server · Tab tabs · a add · e edit · d delete · t test · r restart · l log in · Space on/off · Esc close";
  return h(
    "div",
    {
      class: "mcp-overlay",
      onMousedown: (e) => e.target === e.currentTarget && closeMcp(),
    },
    h(
      "div",
      { class: "mcp", role: "dialog", "aria-label": "MCP servers" },
      h(
        "div",
        { class: "mcp__title" },
        h("strong", "MCP servers"),
        h("span", { class: "muted" }, "changes apply live — no restart"),
        h("span", { class: "mcp-actions__gap" }),
        h("button", { class: "mcp__close", title: "Esc", onClick: closeMcp }, "×"),
      ),
      h(
        "div",
        { class: "mcp__body" },
        ServerList(state),
        ui.form ? McpForm(ui.form) : server ? Detail(server, ui) : h("div", { class: "mcp-detail mcp-empty" }, "Pick a server, or add one."),
      ),
      h("div", { class: "mcp__footer" }, keys),
    ),
  );
}
