// The window: lays out the components once, then re-renders each region when
// the part of the state it shows changes.
//
//   mock bar (outside crest)
//   ┌ window ──────────────────────────────────────────────┐
//   │ chats │ tree │ timeline                              │
//   │       │      │ activity line                         │
//   │       │      │ prompt                                │
//   │       │      │ status bar                            │
//   └──────────────────────────────────────────────────────┘
//   approval dialog (over everything)

import { h } from "./lib/h.js";
import { subscribe, state } from "./store.js";
import { ActivityLine } from "./components/activity-line/activity-line.js";
import { ApprovalDialog } from "./components/approval-dialog/approval-dialog.js";
import { ChatsPanel } from "./components/chats-panel/chats-panel.js";
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

  const win = h(
    "div",
    { class: "window" },
    h("div", { class: "workbench" }, chats, h("div", { class: "root" }, tree, h("div", { class: "session" }, timeline, activity, prompt, status))),
    overlay,
  );
  root.append(mockBar, h("div", { class: "stage" }, win));

  const timelineView = mountTimeline(timeline);
  const promptView = mountPromptInput(prompt);

  const regions = [
    region(mockBar, (s) => s.ui, MockBar),
    region(chats, (s) => [s.ui.chatsShown && s.projects, s.openChatId, s.ui.menu, s.ui.folded, s.ui.focus], (s) => s.ui.chatsShown && ChatsPanel(s)),
    region(tree, (s) => [s.ui.treeShown, s.ui.focus], (s) => s.ui.treeShown && TreePanel(s)),
    region(activity, (s) => [s.turn.activity, s.turn.queue, Math.floor(Date.now() / 100)], (s) => ActivityLine(s)),
    region(status, (s) => [s.status, s.notice, s.turn.busy], StatusBar),
    region(overlay, (s) => [s.turn.approval, s.turn.chatId, s.chat?.id, s.ui.approvalIndex], ApprovalDialog),
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
