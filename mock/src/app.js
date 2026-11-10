// The window: lays out the components once, then re-renders each region when
// the part of the state it shows changes.
//
//   mock bar (outside crest)
//   ┌ window ──────────────────────────────────────────────┐
//   │ chats │ tree │ timeline                              │
//   │       │      │ activity line                         │
//   │ prompt                                                │
//   │ status bar                                            │
//   └──────────────────────────────────────────────────────┘
//   approval dialog, MCP servers modal (over everything)

import { h } from "./lib/h.js";
import { subscribe, state } from "./store.js";
import { ActivityLine } from "./components/activity-line/activity-line.js";
import { InfoCorner } from "./components/info-corner/info-corner.js";
import { ApprovalDialog } from "./components/approval-dialog/approval-dialog.js";
import { ChatsPanel } from "./components/chats-panel/chats-panel.js";
import { McpManager } from "./components/mcp-manager/mcp-manager.js";
import { MockBar } from "./components/mock-bar/mock-bar.js";
import { mountPromptInput } from "./components/prompt-input/prompt-input.js";
import { StatusBar } from "./components/status-bar/status-bar.js";
import { mountTimeline } from "./components/timeline/timeline.js";
import { TreePanel } from "./components/tree-panel/tree-panel.js";
import { bindKeys } from "./keys.js";

/** Calls `fn(state)` only when `watch(state)` changed since the last call. */
function whenChanged(watch, fn) {
  let last;
  return (s) => {
    const key = JSON.stringify(watch(s));
    if (key === last) return;
    last = key;
    fn(s);
  };
}

/** A slot that `render` fills again whenever `watch(state)` changes. */
const region = (slot, watch, render) => whenChanged(watch, (s) => slot.replaceChildren(render(s) || ""));

/**
 * A region whose content has inputs and scrolled panes: keeps the focused
 * `[data-key]` input (with its caret) and the `[data-scroll]` offsets across renders.
 */
const keptRegion = (slot, watch, render) =>
  whenChanged(watch, (s) => {
    const active = document.activeElement;
    const key = slot.contains(active) ? active.dataset.key : null;
    const caret = key && "selectionStart" in active ? [active.selectionStart, active.selectionEnd] : null;
    const scrolls = Object.fromEntries([...slot.querySelectorAll("[data-scroll]")].map((el) => [el.dataset.scroll, el.scrollTop]));
    slot.replaceChildren(render(s) || "");
    for (const el of slot.querySelectorAll("[data-scroll]")) el.scrollTop = scrolls[el.dataset.scroll] ?? 0;
    const again = key && slot.querySelector(`[data-key="${key}"]`);
    if (again) {
      again.focus();
      if (caret) again.setSelectionRange(...caret);
    }
  });

const slot = () => h("div", { class: "slot" });

export function mountApp(root) {
  const mockBar = slot();
  const chats = slot();
  const tree = slot();
  const timeline = h("div");
  const activity = slot();
  const prompt = h("div");
  const status = slot();
  const overlay = slot();
  const info = slot();
  const mcp = slot();

  const win = h(
    "div",
    { class: "window" },
    h(
      "div",
      { class: "workbench" },
      h("div", { class: "workbench__body" }, chats, h("div", { class: "root" }, tree, h("div", { class: "session" }, timeline, activity))),
      prompt,
      status,
    ),
    overlay,
    info,
    mcp,
  );
  root.append(mockBar, h("div", { class: "stage" }, win));

  const timelineView = mountTimeline(timeline);
  const promptView = mountPromptInput(prompt);

  const regions = [
    region(mockBar, (s) => s.ui, MockBar),
    region(
      chats,
      (s) => [
        s.organization,
        s.openChatId,
        s.ui.sidebar,
        s.ui.search,
        s.ui.menu,
        s.ui.folded,
        s.ui.focus,
        s.ui.archivedOpen,
        s.mcp.map((m) => m.state),
        s.ui.mcp.open,
      ],
      ChatsPanel,
    ),
    region(tree, (s) => [s.ui.treeShown, s.ui.focus], (s) => s.ui.treeShown && TreePanel(s)),
    region(activity, (s) => [s.turn.activity, s.turn.queue, Math.floor(Date.now() / 100)], (s) => ActivityLine(s)),
    region(status, (s) => [s.status, s.notice, s.turn.busy, s.organization, s.openChatId], StatusBar),
    region(info, (s) => [s.notices, s.notice, s.turn.steps, s.toast, s.ui.infoOpen], InfoCorner),
    region(overlay, (s) => [s.turn.approval, s.turn.chatId, s.chat?.id, s.ui.approvalIndex], ApprovalDialog),
    keptRegion(mcp, (s) => [s.ui.mcp.open && s.mcp, s.ui.mcp], McpManager),
  ];
  const renderTimeline = whenChanged(
    (s) => [s.chat, s.ui.selected, s.ui.expanded, s.ui.altView, s.ui.outlineOpen, s.ui.focus],
    timelineView.update,
  );

  function render(s) {
    win.setAttribute("data-theme", s.ui.theme);
    win.style.maxWidth = s.ui.width === "fill" ? "" : `${s.ui.width}px`;
    win.classList.toggle("window--framed", s.ui.width !== "fill");
    for (const update of regions) update(s);
    renderTimeline(s);
    promptView.update(s);
  }

  subscribe(render);
  setInterval(() => state.turn.activity && render(state), 100);
  bindKeys(promptView);
  render(state);
  promptView.focus();
}
