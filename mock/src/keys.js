// crest's keys, as far as a browser lets us have them (docs/usage.md).
//
//   Esc            back to the prompt (closes the outline and menus first)
//   Ctrl+C         interrupt the turn, or clear the prompt
//   Ctrl+Tab / F6  move the focus: prompt → timeline → tree → chats
//   Ctrl+G …       the leader: s chats · e tree · O outline · p permission mode
//   in the timeline: j/k ↑/↓ move · gg/G ends · o/Enter expand · t chart/table · y copy
//   in an approval: y s p e n answer · ↑/↓ + Enter · Esc rejects
//
// Browsers keep Ctrl+Tab for their own tabs, so F6 does the same.

import { approve, copySelected, cycleMode, interrupt, moveSelection, notice, selectEdge, toggleAltView, toggleExpanded } from "./actions.js";
import { APPROVAL_OPTIONS } from "./components/approval-dialog/approval-dialog.js";
import { setUi, state } from "./store.js";

let leader = false;
let leaderTimer;
let lastKey = null;

function focusOrder() {
  return ["prompt", "timeline", state.ui.treeShown && "tree", state.ui.chatsShown && "chats"].filter(Boolean);
}

function focus(target, prompt) {
  setUi({ focus: target });
  if (target === "prompt") prompt.focus();
  else document.activeElement?.blur();
}

function onApproval(e) {
  const options = APPROVAL_OPTIONS;
  const direct = options.find((o) => o.key === e.key);
  if (direct) return approve(direct.choice);
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    const step = e.key === "ArrowDown" ? 1 : -1;
    return setUi({ approvalIndex: (state.ui.approvalIndex + step + options.length) % options.length });
  }
  if (e.key === "Enter") return approve(options[state.ui.approvalIndex].choice);
  if (e.key === "Escape") return approve("reject");
}

function onLeader(e) {
  const actions = {
    s: () => setUi({ chatsShown: !state.ui.chatsShown }),
    e: () => setUi({ treeShown: !state.ui.treeShown }),
    O: () => setUi({ outlineOpen: !state.ui.outlineOpen }),
    p: cycleMode,
  };
  (actions[e.key] ?? (() => notice("warning", `<C-g>${e.key} is not in the mock yet`)))();
}

function onTimeline(e) {
  const blockId = state.ui.selected;
  const block = state.chat?.blocks.find((b) => b.id === blockId);
  const keys = {
    j: () => moveSelection(1),
    ArrowDown: () => moveSelection(1),
    k: () => moveSelection(-1),
    ArrowUp: () => moveSelection(-1),
    G: () => selectEdge(true),
    g: () => lastKey === "g" && selectEdge(false),
    o: () => blockId && toggleExpanded(blockId),
    Enter: () => blockId && toggleExpanded(blockId),
    t: () => block?.kind === "chart" && toggleAltView(blockId),
    y: copySelected,
  };
  const action = keys[e.key];
  if (!action) return false;
  action();
  return true;
}

export function bindKeys(prompt) {
  window.addEventListener("keydown", (e) => {
    const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;

    if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "m") {
      e.preventDefault();
      return setUi({ mockBar: !state.ui.mockBar });
    }

    if (state.turn.approval && state.turn.chatId === state.chat?.id) {
      e.preventDefault();
      return onApproval(e);
    }

    if (e.ctrlKey && e.key === "c" && !window.getSelection()?.toString()) {
      e.preventDefault();
      if (!interrupt()) prompt.clear();
      return;
    }

    if (e.ctrlKey && e.key === "g") {
      e.preventDefault();
      leader = true;
      clearTimeout(leaderTimer);
      leaderTimer = setTimeout(() => (leader = false), 1500);
      return;
    }
    if (leader && !["Control", "Shift", "Meta", "Alt"].includes(e.key)) {
      e.preventDefault();
      leader = false;
      return onLeader(e);
    }

    if ((e.ctrlKey && e.key === "Tab") || e.key === "F6") {
      e.preventDefault();
      const order = focusOrder();
      const step = e.shiftKey ? -1 : 1;
      return focus(order[(order.indexOf(state.ui.focus) + step + order.length) % order.length], prompt);
    }

    if (e.key === "Escape") {
      if (state.ui.outlineOpen) return setUi({ outlineOpen: false });
      if (state.ui.menu) return setUi({ menu: null });
      return focus("prompt", prompt);
    }

    if (!typing && state.ui.focus === "timeline" && onTimeline(e)) e.preventDefault();
    lastKey = e.key;
  });
}
