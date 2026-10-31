// The whole UI state, in one object. Components read it; only actions.js changes it.

export const state = {
  projects: [],
  openChatId: null,
  chat: null, // { id, projectId, title, blocks }
  status: null, // { model, mode, cost, git, workspace }
  commands: [],
  notice: null, // { level, text }

  turn: {
    chatId: null,
    busy: false,
    activity: null, // { verb, stage, detail, startedAt }
    approval: null, // { id, tool, subject, preview, pattern }
    queue: [],
  },

  ui: {
    theme: "twocrest",
    width: "fill", // the mock bar's window width: fill | 1440 | 1024 | 720
    mockBar: true,
    focus: "prompt", // prompt | timeline | chats | tree
    chatsShown: true,
    treeShown: false,
    folded: {}, // project id -> true when folded
    menu: null, // { chatId, mode: "menu" | "rename" | "delete" }
    selected: null, // the selected block id
    expanded: {}, // block id -> true
    altView: {}, // block id -> true when a chart shows its table
    outlineOpen: false,
    approvalIndex: 0,
  },
};

const listeners = new Set();

export function subscribe(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Merge `patch` into the state (or into `state.ui` / `state.turn` with the helpers) and re-render. */
export function setState(patch) {
  Object.assign(state, patch);
  changed();
}

export function setUi(patch) {
  Object.assign(state.ui, patch);
  changed();
}

export function setTurn(patch) {
  Object.assign(state.turn, patch);
  changed();
}

let scheduled = false;
function changed() {
  if (scheduled) return;
  scheduled = true;
  requestAnimationFrame(() => {
    scheduled = false;
    for (const listener of listeners) listener(state);
  });
}
