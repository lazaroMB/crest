// Everything the UI can do. Components call these; they talk to the mock server and update the state.

import { toBlob } from "html-to-image";
import { api } from "./api.js";
import { setState, setTurn, setUi, state } from "./store.js";

const NOTICE_TTL = 6000; // crest's status bar notices last 6 s (status.rs)
let noticeTimer;

export function notice(level, text) {
  clearTimeout(noticeTimer);
  setState({ notice: { level, text } });
  noticeTimer = setTimeout(() => setState({ notice: null }), NOTICE_TTL);
}

/**
 * Loads the workbench. The URL can pick the starting point:
 *   ?chat=c-release  ?chat=new  ?theme=twocrest-light  ?width=1024  ?prompt=fix the bug
 */
export async function init() {
  const params = new URLSearchParams(location.search);
  const width = params.get("width");
  setUi({
    theme: params.get("theme") ?? state.ui.theme,
    width: width && width !== "fill" ? Number(width) : "fill",
  });

  const [workbench, commands, mcp] = await Promise.all([api.workbench(), api.commands(), api.mcpServers()]);
  setState({ ...workbench, commands, mcp });
  if (params.get("mcp") != null) openMcp({ name: params.get("mcp") || undefined, tab: params.get("tab") ?? undefined, add: params.get("mcp") === "add" });

  const chat = params.get("chat");
  if (chat === "new") await newChat();
  else await openChat(chat ?? workbench.openChatId);

  if (params.get("prompt")) send(params.get("prompt"));
}

async function refreshProjects() {
  const { projects } = await api.workbench();
  setState({ projects });
}

export async function openChat(id) {
  const chat = await api.chat(id);
  setState({ chat, openChatId: id });
  setUi({ selected: null, menu: null, outlineOpen: false });
}

export async function newChat(projectId = state.chat?.projectId) {
  const chat = await api.newChat(projectId);
  await refreshProjects();
  setState({ chat, openChatId: chat.id });
  setUi({ selected: null, focus: "prompt" });
}

export async function renameChat(id, title) {
  await api.renameChat(id, title.trim() || "New chat");
  if (state.chat?.id === id) state.chat.title = title;
  setUi({ menu: null });
  await refreshProjects();
}

export async function deleteChat(id) {
  await api.deleteChat(id);
  setUi({ menu: null });
  await refreshProjects();
  if (state.chat?.id === id) {
    const next = state.projects.flatMap((p) => p.chats)[0];
    if (next) await openChat(next.id);
    else setState({ chat: null });
  }
}

export function toggleProject(projectId) {
  setUi({ folded: { ...state.ui.folded, [projectId]: !state.ui.folded[projectId] } });
}

// Turns.

export function send(prompt) {
  if (!prompt.trim() || !state.chat) return;
  if (state.turn.busy) {
    setTurn({ queue: [...state.turn.queue, prompt] });
    return;
  }
  startTurn(prompt);
}

function startTurn(prompt) {
  const chatId = state.chat.id;
  setTurn({ chatId, busy: true, activity: { verb: "Working", startedAt: Date.now() } });
  markChat(chatId, "running");
  api.turn(chatId, prompt, (event) => onTurnEvent(chatId, event));
}

function onTurnEvent(chatId, event) {
  const here = state.chat?.id === chatId;
  switch (event.type) {
    case "block": {
      if (!here) return;
      const blocks = state.chat.blocks;
      const at = blocks.findIndex((b) => b.id === event.block.id);
      if (at === -1) blocks.push(event.block);
      else blocks[at] = event.block;
      setState({ chat: state.chat });
      return;
    }
    case "activity":
      return setTurn({ activity: event.activity });
    case "approval":
      markChat(chatId, event.request ? "waiting" : "running");
      setUi({ approvalIndex: 0 });
      return setTurn({ approval: event.request });
    case "status":
      return setState({ status: { ...state.status, ...event.status } });
    case "notice":
      return notice(event.notice.level, event.notice.text);
    case "chat":
      if (here) state.chat.title = event.title;
      return refreshProjects();
    case "done": {
      markChat(chatId, undefined);
      const [next, ...rest] = state.turn.queue;
      setTurn({ busy: false, activity: null, approval: null, queue: rest });
      if (next) startTurn(next);
    }
  }
}

function markChat(chatId, activity) {
  const chat = state.projects.flatMap((p) => p.chats).find((c) => c.id === chatId);
  if (chat) chat.activity = activity;
  setState({ projects: state.projects });
}

export function approve(choice) {
  const request = state.turn.approval;
  if (!request) return;
  setTurn({ approval: null });
  api.approve(request.id, choice);
}

export function interrupt() {
  if (!state.turn.busy) return false;
  setTurn({ queue: [] });
  api.interrupt(state.turn.chatId);
  return true;
}

/** The / commands the mock runs itself; the rest go to the server as prompts. Returns true when handled. */
export function runCommand(text) {
  const [name, ...args] = text.split(/\s+/);
  switch (name) {
    case "/clear":
      newChat();
      return true;
    case "/mode":
      if (["ask", "auto-edit", "plan"].includes(args[0])) setMode(args[0]);
      else notice("warning", "usage: /mode <ask|auto-edit|plan>");
      return true;
    case "/theme":
      setUi({ theme: state.ui.theme === "twocrest" ? "twocrest-light" : "twocrest" });
      return true;
    case "/research":
      return false;
    case "/mcp":
      mcpCommand(args);
      return true;
    default:
      notice("warning", `${name} is not in the mock yet`);
      return true;
  }
}

// Status bar.

function setMode(mode) {
  setState({ status: { ...state.status, mode } });
  api.setStatus({ mode });
  notice("info", `permission mode: ${mode}`);
}

const MODES = ["ask", "auto-edit", "plan"];

export function cycleMode() {
  setMode(MODES[(MODES.indexOf(state.status.mode) + 1) % MODES.length]);
}

// Timeline.

export function select(blockId) {
  setUi({ selected: blockId, focus: "timeline" });
}

export function moveSelection(delta) {
  const blocks = state.chat?.blocks ?? [];
  if (!blocks.length) return;
  const at = blocks.findIndex((b) => b.id === state.ui.selected);
  const next = at === -1 ? (delta > 0 ? 0 : blocks.length - 1) : at + delta;
  setUi({ selected: blocks[Math.max(0, Math.min(blocks.length - 1, next))].id });
}

export function selectEdge(last) {
  const blocks = state.chat?.blocks ?? [];
  if (blocks.length) setUi({ selected: blocks[last ? blocks.length - 1 : 0].id });
}

export function toggleExpanded(blockId) {
  setUi({ expanded: { ...state.ui.expanded, [blockId]: !state.ui.expanded[blockId] } });
}

export function toggleAltView(blockId) {
  setUi({ altView: { ...state.ui.altView, [blockId]: !state.ui.altView[blockId] } });
}

export function copySelected() {
  const block = state.chat?.blocks.find((b) => b.id === state.ui.selected);
  if (!block) return;
  const text = block.markdown ?? block.text ?? JSON.stringify(block, null, 2);
  navigator.clipboard?.writeText(text);
  notice("info", "copied");
}

/** Rasterizes `node` (a chart block) and puts it on the clipboard as a PNG. */
export async function copyChartImage(node) {
  if (!node) return;
  node.classList.add("chart--capturing");
  try {
    // `--background` lives on `.window[data-theme]`, not `:root`, but custom
    // properties inherit down from any ancestor so reading it off the node works.
    const bg = getComputedStyle(node).getPropertyValue("--background").trim();
    const blob = await toBlob(node, { pixelRatio: 2, backgroundColor: bg || undefined });
    if (!blob) throw new Error("rendering failed");
    if (window.ClipboardItem && navigator.clipboard?.write) {
      await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })]);
      notice("info", "chart copied as image");
    } else {
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = "chart.png";
      link.click();
      URL.revokeObjectURL(url);
      notice("warning", "clipboard images unsupported here · downloaded instead");
    }
  } catch (err) {
    notice("error", `couldn't copy chart image: ${err.message}`);
  } finally {
    node.classList.remove("chart--capturing");
  }
}

// MCP servers: the manager modal (components/mcp-manager). Every change is live,
// like McpManager in crest: no restart, the server goes Starting → Ready.

const mcpUi = () => state.ui.mcp;

function setMcpUi(patch) {
  setUi({ mcp: { ...state.ui.mcp, ...patch } });
}

let mcpPoll;

/** Reloads the servers, and keeps polling while one of them is starting. */
export async function loadMcp() {
  const mcp = await api.mcpServers();
  setState({ mcp });
  clearTimeout(mcpPoll);
  if (mcp.some((s) => s.state === "starting")) mcpPoll = setTimeout(loadMcp, 400);
}

function putServer(server) {
  const at = state.mcp.findIndex((s) => s.name === server.name);
  if (at === -1) state.mcp.push(server);
  else state.mcp[at] = server;
  setState({ mcp: state.mcp });
}

export const MCP_ACTIONS = ["test", "restart", "login", "logout", "trust", "enable", "disable", "remove"];

/** `/mcp`, `/mcp add`, `/mcp tools <name>`, `/mcp <action> <name>`, `/mcp <name>`. */
function mcpCommand([sub, name]) {
  if (!sub) return openMcp();
  if (sub === "add") return openMcp({ add: true });
  if (sub === "tools") return openMcp({ name, tab: "tools" });
  if (MCP_ACTIONS.includes(sub)) {
    if (!state.mcp.some((s) => s.name === name)) return notice("warning", `usage: /mcp ${sub} <name>`);
    openMcp({ name });
    if (sub === "remove") return setMcpUi({ confirmDelete: true });
    if (sub === "enable" || sub === "disable") return runMcp(name, sub);
    return sub === "login" ? loginMcp(name) : runMcp(name, sub);
  }
  if (state.mcp.some((s) => s.name === sub)) return openMcp({ name: sub });
  notice("warning", `no MCP server named ${sub}`);
}

export function openMcp({ name, tab, add } = {}) {
  const selected = name ?? mcpUi().selected ?? state.mcp[0]?.name ?? null;
  setMcpUi({ open: true, selected, tab: tab ?? mcpUi().tab, confirmDelete: false, form: null });
  document.activeElement?.blur();
  loadMcp();
  if (add) startMcpForm();
}

export function closeMcp() {
  setMcpUi({ open: false, form: null, confirmDelete: false });
}

export function toggleMcpModal() {
  if (mcpUi().open) closeMcp();
  else openMcp();
}

export function selectMcp(name) {
  setMcpUi({ selected: name, form: null, confirmDelete: false, openTool: null, toolFilter: "" });
}

/** The servers the list shows, in its order (global first, then this workspace). */
export function visibleMcp() {
  const filter = mcpUi().filter.trim().toLowerCase();
  const shown = state.mcp.filter((s) => !filter || s.name.toLowerCase().includes(filter));
  return [...shown.filter((s) => s.origin === "global"), ...shown.filter((s) => s.origin !== "global")];
}

export function moveMcp(delta) {
  const list = visibleMcp();
  if (!list.length) return;
  const at = list.findIndex((s) => s.name === mcpUi().selected);
  const next = at === -1 ? 0 : (at + delta + list.length) % list.length;
  selectMcp(list[next].name);
}

export function setMcpTab(tab) {
  setMcpUi({ tab });
}

export function cycleMcpTab(step) {
  const tabs = ["overview", "tools", "log"];
  setMcpTab(tabs[(tabs.indexOf(mcpUi().tab) + step + tabs.length) % tabs.length]);
}

export function setMcpFilter(filter) {
  setMcpUi({ filter });
}

export function setMcpToolFilter(toolFilter) {
  setMcpUi({ toolFilter });
}

export function toggleMcpTool(tool) {
  setMcpUi({ openTool: mcpUi().openTool === tool ? null : tool });
}

export function revealMcpSecret(key) {
  setMcpUi({ revealed: { ...mcpUi().revealed, [key]: !mcpUi().revealed[key] } });
}

function setPending(name, action) {
  const pending = { ...mcpUi().pending };
  if (action) pending[name] = action;
  else delete pending[name];
  setMcpUi({ pending });
}

function setResult(name, result) {
  setMcpUi({ results: { ...mcpUi().results, [name]: result && { ...result, at: Date.now() } } });
}

/** test | restart | logout | trust | enable | disable, on one server. */
export async function runMcp(name, action) {
  if (mcpUi().pending[name]) return;
  setPending(name, action);
  if (action === "test") setResult(name, null);
  try {
    const result = await api.mcpAction(name, action);
    putServer(result.server);
    if (action === "test") setResult(name, result);
    if (action === "enable" || action === "disable") notice("info", `${name} ${action}d · saved, no restart needed`);
    if (action === "trust") notice("info", `${name} trusted for this workspace`);
  } catch (error) {
    notice("error", error.message);
  } finally {
    setPending(name, null);
    loadMcp();
  }
}

export function toggleMcpEnabled(name) {
  const server = state.mcp.find((s) => s.name === name);
  if (server) runMcp(name, server.state === "disabled" ? "enable" : "disable");
}

/** Log in (or again): crest opens the browser and waits for the OAuth callback. */
export async function loginMcp(name) {
  const server = state.mcp.find((s) => s.name === name);
  if (mcpUi().pending[name] || server?.transport !== "http" || !server.config.oauth) return;
  setPending(name, "login");
  setResult(name, null);
  try {
    const result = await api.mcpAction(name, "login");
    putServer(result.server);
    setResult(name, result);
  } catch (error) {
    notice("error", error.message);
  } finally {
    setPending(name, null);
    loadMcp();
  }
}

export function cancelMcpLogin(name) {
  api.mcpAction(name, "cancel");
}

export function askDeleteMcp() {
  const server = state.mcp.find((s) => s.name === mcpUi().selected);
  if (server && server.source !== "init.lua") setMcpUi({ confirmDelete: true });
}

export function cancelDeleteMcp() {
  setMcpUi({ confirmDelete: false });
}

export async function deleteMcp(name) {
  const list = visibleMcp();
  const at = list.findIndex((s) => s.name === name);
  await api.mcpRemove(name);
  const rest = list.filter((s) => s.name !== name);
  state.mcp = state.mcp.filter((s) => s.name !== name);
  setMcpUi({ confirmDelete: false, selected: rest[Math.min(at, rest.length - 1)]?.name ?? null });
  setState({ mcp: state.mcp });
  notice("info", `${name} removed · its tools are gone from every chat`);
}

// The add/edit form. Typing changes the draft in place (no re-render, so the
// input keeps its caret); adding or removing a row re-renders.

const row = () => ({ key: "", value: "" });

function emptyDraft() {
  return {
    name: "",
    transport: "stdio",
    scope: "global",
    effect: "execute",
    timeout: "60",
    startupTimeout: "30",
    command: "",
    args: [""],
    env: [],
    cwd: "",
    url: "",
    headers: [],
    oauth: { enabled: false, clientId: "", clientSecret: "", scopes: "", callbackPort: "" },
  };
}

function draftFrom(server) {
  const c = server.config;
  return {
    ...emptyDraft(),
    name: server.name,
    transport: server.transport,
    scope: server.origin,
    effect: server.effect,
    timeout: String(server.timeout),
    startupTimeout: String(server.startupTimeout),
    command: c.command ?? "",
    args: c.args?.length ? [...c.args] : [""],
    env: (c.env ?? []).map((r) => ({ ...r })),
    cwd: c.cwd ?? "",
    url: c.url ?? "",
    headers: (c.headers ?? []).map((r) => ({ ...r })),
    oauth: c.oauth ? { enabled: true, ...c.oauth } : emptyDraft().oauth,
  };
}

/** Starting points for the add form, so nobody has to know the commands by heart. */
export const MCP_TEMPLATES = [
  { label: "Filesystem", draft: { name: "files", command: "npx", args: ["-y", "@modelcontextprotocol/server-filesystem", "~/Documents"], effect: "write" } },
  { label: "Playwright", draft: { name: "playwright", command: "npx", args: ["-y", "@playwright/mcp@latest"], effect: "network" } },
  { label: "Postgres", draft: { name: "db", command: "uvx", args: ["mcp-server-postgres", "${DATABASE_URL}"], effect: "read" } },
  { label: "Sentry (http)", draft: { name: "sentry", transport: "http", url: "https://mcp.sentry.dev/mcp", effect: "network", oauth: { ...emptyDraft().oauth, enabled: true } } },
  { label: "Notion (http)", draft: { name: "notion", transport: "http", url: "https://mcp.notion.com/mcp", effect: "network", oauth: { ...emptyDraft().oauth, enabled: true } } },
];

export function startMcpForm(name) {
  const server = name && state.mcp.find((s) => s.name === name);
  if (server?.source === "init.lua") return notice("warning", `${name} is declared in init.lua: edit it there`);
  setMcpUi({
    form: server ? { mode: "edit", name, draft: draftFrom(server) } : { mode: "add", draft: emptyDraft() },
    confirmDelete: false,
  });
}

export function applyMcpTemplate(template) {
  setMcpUi({ form: { ...mcpUi().form, draft: { ...emptyDraft(), ...structuredClone(template.draft) }, error: null, test: null } });
}

export function cancelMcpForm() {
  setMcpUi({ form: null });
}

/** A structural change to the draft (type, rows, a toggle): re-renders the form. */
export function editMcpDraft(change) {
  const form = mcpUi().form;
  change(form.draft);
  setMcpUi({ form: { ...form, error: null } });
}

export const addMcpRow = (list) => editMcpDraft((d) => d[list].push(list === "args" ? "" : row()));
export const removeMcpRow = (list, index) => editMcpDraft((d) => d[list].splice(index, 1));

function specFrom(draft) {
  const pairs = (rows) => rows.filter((r) => r.key.trim()).map((r) => ({ key: r.key.trim(), value: r.value }));
  const { enabled, ...oauth } = draft.oauth;
  return {
    name: draft.name.trim(),
    transport: draft.transport,
    origin: draft.scope,
    effect: draft.effect,
    timeout: Number(draft.timeout) || 60,
    startupTimeout: Number(draft.startupTimeout) || 30,
    config:
      draft.transport === "stdio"
        ? { command: draft.command.trim(), args: draft.args.map((a) => a.trim()).filter(Boolean), env: pairs(draft.env), cwd: draft.cwd.trim() }
        : { url: draft.url.trim(), headers: pairs(draft.headers), oauth: enabled ? oauth : null },
  };
}

export async function testMcpForm() {
  const form = mcpUi().form;
  setMcpUi({ form: { ...form, testing: true, test: null, error: null } });
  const test = await api.mcpTestSpec(specFrom(form.draft)).catch((e) => ({ ok: false, text: e.message }));
  if (mcpUi().form) setMcpUi({ form: { ...mcpUi().form, testing: false, test } });
}

export async function saveMcpForm() {
  const form = mcpUi().form;
  const spec = specFrom(form.draft);
  try {
    const server = form.mode === "add" ? await api.mcpAdd(spec) : await api.mcpUpdate(form.name, spec);
    if (form.mode === "edit" && form.name !== spec.name) state.mcp = state.mcp.filter((s) => s.name !== form.name);
    putServer(server);
    setMcpUi({ form: null, selected: server.name, tab: "overview" });
    notice("info", `${server.name} ${form.mode === "add" ? "added" : "saved"} · starting, no restart needed`);
    loadMcp();
  } catch (error) {
    setMcpUi({ form: { ...mcpUi().form, error: error.message } });
  }
}
