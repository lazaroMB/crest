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

  const [workbench, commands] = await Promise.all([api.workbench(), api.commands()]);
  setState({ ...workbench, commands });

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
