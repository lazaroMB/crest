// Everything the UI can do. Components call these; they talk to the mock server and update the state.

import { toBlob } from "html-to-image";
import { api } from "./api.js";
import { setState, setTurn, setUi, state } from "./store.js";
import { NOTICE_TTL_MS, SHOW_AFTER_MS, recordStep } from "./visibility.js";
import { nextMode, nextSidebar } from "./modes.js";

let noticeTimer;
let toastTimer;
const TOAST_MS = 3000;

/** Shows a corner note for 3 s. The entry itself stays behind the information mark. */
function flash(toast) {
  setState({ toast });
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    if (state.toast?.id === toast.id) setState({ toast: null });
  }, TOAST_MS);
}

/** The bar keeps the notice for 6 s. The list keeps it until the person dismisses it. */
export function notice(level, text) {
  clearTimeout(noticeTimer);
  const item = { id: `notice-${Date.now()}`, level, text };
  setState({ notice: item, notices: [...state.notices, item] });
  noticeTimer = setTimeout(() => {
    if (state.notice?.id === item.id) setState({ notice: null });
  }, NOTICE_TTL_MS);
}

export function dismissInfo(id) {
  const notices = state.notices.filter((n) => n.id !== id);
  const steps = state.turn.steps.filter((s) => s.id !== id);
  const left =
    notices.some((n) => n.level === "error" || n.level === "warning") || steps.some((s) => s.elapsedMs < SHOW_AFTER_MS);
  setState({
    notices,
    notice: state.notice?.id === id ? null : state.notice,
    toast: state.toast?.id === id ? null : state.toast,
  });
  setTurn({ steps });
  if (!left) setUi({ infoOpen: false });
}

export function toggleInfo() {
  const infoOpen = !state.ui.infoOpen;
  setUi({ infoOpen });
  if (infoOpen) setState({ toast: null });
}

/**
 * Loads the workbench. The URL can pick the starting point:
 *   ?chat=c-release  ?chat=new  ?theme=twocrest-light  ?width=1024  ?prompt=fix the bug
 *   ?sidebar=strip|hidden  ?mode=research|deep  ?mcp  ?mcp=add  ?mcp=github&tab=tools
 */
export async function init() {
  const params = new URLSearchParams(location.search);
  const width = params.get("width");
  setUi({
    theme: params.get("theme") ?? state.ui.theme,
    width: width && width !== "fill" ? Number(width) : "fill",
    sidebar: params.get("sidebar") ?? state.ui.sidebar,
    chatMode: params.get("mode") ?? state.ui.chatMode,
  });

  const [workbench, commands, mcp] = await Promise.all([api.workbench(), api.commands(), api.mcpServers()]);
  setState({ ...workbench, commands, mcp });
  if (params.get("mcp") != null) {
    const param = params.get("mcp");
    openMcp({ name: param && param !== "add" ? param : undefined, tab: params.get("tab") ?? undefined, add: param === "add" });
  }

  const chat = params.get("chat");
  if (chat === "new") await newChat();
  else await openChat(chat ?? workbench.openChatId);

  if (params.get("prompt")) send(params.get("prompt"));
}

async function refreshProjects() {
  const { organization } = await api.workbench();
  setState({ organization });
}

/** Runs an organization change on the server; a refusal shows as a warning and changes nothing. */
async function organize(call) {
  try {
    const { organization } = await call();
    setState({ organization });
    return true;
  } catch (error) {
    notice("warning", error.message);
    return false;
  }
}

export async function openChat(id) {
  const chat = await api.chat(id);
  setState({ chat, openChatId: id, notice: null, notices: [], toast: null });
  setTurn({ steps: [] });
  setUi({ selected: null, menu: null, outlineOpen: false, infoOpen: false, focus: "prompt" });
  if (id === "c-kitchen") showVisibility();
}

/** The gallery chat opens with a short step on screen and an error kept in the mark. */
function showVisibility() {
  const step = { id: "step-1", label: "grep", elapsedMs: 40 };
  setState({
    notice: null,
    notices: [{ id: "kept-1", level: "error", text: "Anthropic API: overloaded (529) after 3 retries" }],
  });
  setTurn({ steps: [step] });
  flash({ id: step.id, level: "info", text: "grep · 40ms" });
}

/** A new chat: in `projectId`, or standalone with `null`. "New chat" with no argument is standalone. */
export async function newChat(projectId = null) {
  const chat = await api.newChat(projectId);
  await refreshProjects();
  setState({ chat, openChatId: chat.id });
  setUi({ selected: null, focus: "prompt", menu: null });
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
    const next = state.organization.chats[0];
    if (next) await openChat(next.id);
    else setState({ chat: null });
  }
}

/** Promotes a chat into a project, or takes it out with `null`. The chat keeps its history. */
export async function moveChat(chatId, projectId) {
  if (await organize(() => api.moveChat(chatId, projectId))) {
    if (state.chat?.id === chatId) state.chat.projectId = projectId;
    const name = state.organization.projects.find((p) => p.id === projectId)?.name;
    notice("info", projectId ? `moved to ${name}` : "now a standalone chat");
    setUi({ menu: null });
  }
}

/** A new project of one directory, with `chatId` promoted into it when given. */
export async function createProject(name, root, chatId) {
  if (await organize(() => api.createProject({ name, roots: [root], chatId }))) {
    if (chatId && state.chat?.id === chatId) state.chat.projectId = state.organization.chats.find((c) => c.id === chatId)?.projectId;
    notice("info", `project ${name} created`);
    setUi({ menu: null });
  }
}

export async function addProjectRoot(projectId, root) {
  if (await organize(() => api.patchProject(projectId, { addRoot: root }))) setUi({ menu: { projectId, mode: "menu" } });
}

export const removeProjectRoot = (projectId, root) => organize(() => api.patchProject(projectId, { removeRoot: root }));

export async function renameProject(projectId, name) {
  if (await organize(() => api.patchProject(projectId, { name }))) setUi({ menu: null });
}

export function pinProject(projectId) {
  const pinned = !state.organization.projects.find((p) => p.id === projectId)?.pinned;
  organize(() => api.patchProject(projectId, { pinned }));
  setUi({ menu: null });
}

export function toggleProject(projectId) {
  setUi({ folded: { ...state.ui.folded, [projectId]: !state.ui.folded[projectId] } });
}

/** Takes a chat or a project out of the sidebar's lists without deleting it. */
export async function archiveChat(chatId) {
  if (await organize(() => api.patchChat(chatId, { archived: true }))) {
    notice("info", "chat archived");
    setUi({ menu: null });
  }
}

/** A branch of a chat: a new chat in the same project, with the history copied up to now. */
export async function branchChat(chatId) {
  const chat = await api.branchChat(chatId);
  await refreshProjects();
  setState({ chat, openChatId: chat.id });
  setUi({ selected: null, focus: "prompt", menu: null });
  notice("info", `branched from "${chat.branchedFrom?.title ?? "chat"}"`);
}

export async function unarchiveChat(chatId) {
  if (await organize(() => api.patchChat(chatId, { archived: false }))) notice("info", "chat restored");
}

export async function archiveProject(projectId) {
  if (await organize(() => api.patchProject(projectId, { archived: true }))) {
    notice("info", "project archived");
    setUi({ menu: null });
  }
}

export async function unarchiveProject(projectId) {
  if (await organize(() => api.patchProject(projectId, { archived: false }))) notice("info", "project restored");
}

export const toggleArchived = () => setUi({ archivedOpen: !state.ui.archivedOpen, menu: null });

// The sidebar's widths and its search.

export const setSidebar = (sidebar) => setUi({ sidebar, menu: null, focus: sidebar === "hidden" && state.ui.focus === "chats" ? "prompt" : state.ui.focus });

export const cycleSidebar = () => setSidebar(nextSidebar(state.ui.sidebar));

/** Opens the search field (from the strip it opens the full sidebar first); `null` closes it. */
export function setSearch(search) {
  setUi({ search, sidebar: search === null ? state.ui.sidebar : "full" });
}

// The prompt's chat mode.

export const setChatMode = (chatMode) => setUi({ chatMode, modeMenu: false });

export const cycleChatMode = () => setChatMode(nextMode(state.ui.chatMode));

export const toggleModeMenu = (open = !state.ui.modeMenu) => setUi({ modeMenu: open });

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
      return setTurn({ activity: event.activity, steps: rememberStep(state.turn.activity) });
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
      setTurn({ busy: false, activity: null, approval: null, queue: rest, steps: rememberStep(state.turn.activity) });
      if (next) startTurn(next);
    }
  }
}

function markChat(chatId, activity) {
  const chats = state.organization.chats.map((c) => (c.id === chatId ? { ...c, activity } : c));
  setState({ organization: { ...state.organization, chats } });
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

/** The / commands and `!` shell lines the mock runs itself; the rest go to the server as prompts. Returns true when handled. */
export function runCommand(text) {
  if (text.startsWith("!") || text.startsWith("/!")) {
    const shell = text.replace(/^\/?!/, "").trim();
    notice(shell ? "info" : "warning", shell ? `shell: ${shell} (the mock does not run it)` : "usage: ! <command>");
    return true;
  }
  const [name, ...args] = text.split(/\s+/);
  switch (name) {
    case "/clear":
      newChat(state.chat?.projectId ?? null);
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

function rememberStep(activity) {
  if (!activity?.startedAt) return state.turn.steps;
  const elapsedMs = Date.now() - activity.startedAt;
  const label = [activity.verb, activity.detail].filter(Boolean).join(" · ") || activity.verb;
  const step = { id: `step-${Date.now()}`, label, elapsedMs };
  if (elapsedMs < SHOW_AFTER_MS) flash({ id: step.id, level: "info", text: `${label} · ${elapsedMs}ms` });
  return recordStep(state.turn.steps, step, true);
}

/** Moves among the person's questions, starting from the one the outline marks. */
export function moveOutline(delta) {
  const blocks = state.chat?.blocks ?? [];
  const questions = blocks.filter((b) => b.kind === "user");
  if (!questions.length) return;
  const selectedAt = blocks.findIndex((b) => b.id === state.ui.selected);
  const active = [...questions].reverse().find((q) => selectedAt === -1 || blocks.indexOf(q) <= selectedAt) ?? questions[0];
  const at = questions.indexOf(active);
  const next = Math.max(0, Math.min(questions.length - 1, at + delta));
  setUi({ selected: questions[next].id, focus: "timeline" });
}

export function select(blockId) {
  setUi({ selected: blockId, focus: "timeline" });
}

/** Follows the scroll: the block nearest the top becomes selected, without
 * moving the focus away from wherever it was (typing stays typing). */
export function syncSelected(blockId) {
  if (blockId && blockId !== state.ui.selected) setUi({ selected: blockId });
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
  setUi({ menu: null, modeMenu: false });
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
