// The whole UI state, in one object. Components read it; only actions.js changes it.

export const state = {
  organization: { projects: [], chats: [] }, // src/organization.js
  openChatId: null,
  chat: null, // { id, projectId, title, blocks }
  status: null, // { model, mode, cost, git, workspace }
  commands: [],
  notice: null, // { id, level, text } on the status bar, gone after 6 s
  notices: [], // errors and warnings kept until dismissed
  toast: null, // { id, level, text } in the corner, gone after 3 s
  mcp: [], // the MCP servers: { name, transport, origin, source, state, message, config, tools, log, … }

  turn: {
    chatId: null,
    busy: false,
    activity: null, // { verb, stage, detail, startedAt }
    steps: [], // finished steps; the short ones stay in the log
    approval: null, // { id, tool, subject, preview, pattern }
    queue: [],
  },

  ui: {
    theme: "twocrest",
    width: "fill", // the mock bar's window width: fill | 1440 | 1024 | 720
    mockBar: true,
    focus: "prompt", // prompt | timeline | chats | tree
    sidebar: "hidden", // full | strip | hidden (src/modes.js); closed until the person opens it
    search: null, // the sidebar's search text while its field is open
    treeShown: false,
    folded: {}, // project id -> true when folded
    standaloneFolded: false,
    archivedOpen: false, // the "Archived" section, folded by default
    menu: null, // { chatId | projectId, mode: "menu" | "rename" | "delete" | "move" | "new-project" | "add-root" }
    chatMode: "chat", // chat | research | deep (src/modes.js)
    modeMenu: false, // the prompt's + menu is open
    selected: null, // the selected block id
    expanded: {}, // block id -> true
    altView: {}, // block id -> true when a chart shows its table
    outlineOpen: false,
    infoOpen: false,
    approvalIndex: 0,
    mcp: {
      open: false,
      selected: null, // server name
      tab: "overview", // overview | tools | log
      filter: "",
      toolFilter: "",
      openTool: null, // the tool name whose arguments are shown
      form: null, // { mode: "add" | "edit", name, draft, error, test, testing, oauthOpen }
      confirmDelete: false,
      pending: {}, // server name -> "test" | "login" | "restart" | …
      results: {}, // server name -> { ok, text } of its last test or login
      revealed: {}, // "server/key" -> true when a secret value is shown
    },
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
