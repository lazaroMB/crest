// The Chats sidebar (crates/ui/src/chats.rs): every project and its chats.
// A click opens a chat, a right click opens its menu (rename, delete).
// Its footer, as tall as the prompt and status bar beside it, opens the MCP servers.

import { h } from "../../lib/h.js";
import { deleteChat, newChat, openChat, openMcp, renameChat, toggleProject } from "../../actions.js";
import { Dot } from "../mcp-manager/mcp-manager.js";
import { setUi } from "../../store.js";
import "./chats-panel.scss";

const BADGE = { running: "●", waiting: "⏳" };

function ChatMenu(chat, menu) {
  const close = () => setUi({ menu: null });

  if (menu.mode === "rename") {
    const input = h("input", {
      class: "chat-menu__input",
      value: chat.title,
      onKeydown: (e) => {
        e.stopPropagation();
        if (e.key === "Enter") renameChat(chat.id, input.value);
        if (e.key === "Escape") close();
      },
    });
    requestAnimationFrame(() => input.select());
    return h("div", { class: "chat-menu" }, input, h("div", { class: "chat-menu__hint" }, "Enter saves · Esc cancels"));
  }

  if (menu.mode === "delete") {
    return h(
      "div",
      { class: "chat-menu" },
      h("div", { class: "chat-menu__row chat-menu__row--danger chat-menu__row--static" }, "Delete this chat?"),
      h("button", { class: "chat-menu__row chat-menu__row--danger", onClick: () => deleteChat(chat.id) }, "Delete"),
      h("button", { class: "chat-menu__row", onClick: close }, "Cancel"),
    );
  }

  return h(
    "div",
    { class: "chat-menu" },
    h("button", { class: "chat-menu__row", onClick: () => setUi({ menu: { chatId: chat.id, mode: "rename" } }) }, "Rename"),
    h("button", { class: "chat-menu__row chat-menu__row--danger", onClick: () => setUi({ menu: { chatId: chat.id, mode: "delete" } }) }, "Delete"),
    h("button", { class: "chat-menu__row", onClick: close }, "Cancel"),
  );
}

function ChatRow(chat, { open, menu }) {
  return [
    h(
      "div",
      {
        class: ["chat-row", open && "chat-row--open"],
        title: chat.title,
        onClick: () => openChat(chat.id),
        onContextmenu: (e) => {
          e.preventDefault();
          setUi({ menu: { chatId: chat.id, mode: "menu" } });
        },
      },
      h("span", { class: "chat-row__title" }, chat.title || "New chat"),
      chat.activity && h("span", { class: "chat-row__badge" }, BADGE[chat.activity]),
    ),
    menu?.chatId === chat.id && ChatMenu(chat, menu),
  ];
}

function ProjectGroup(project, state) {
  const folded = state.ui.folded[project.id];
  return h(
    "div",
    { class: "project" },
    h(
      "div",
      { class: "project__row", title: project.path, onClick: () => toggleProject(project.id) },
      h("span", { class: "project__marker" }, folded ? "▸" : "▾"),
      h("span", { class: "project__name" }, project.name),
    ),
    !folded &&
      (project.chats.length
        ? project.chats.map((chat) => ChatRow(chat, { open: chat.id === state.openChatId, menu: state.ui.menu }))
        : h("div", { class: "chats__empty chats__empty--nested" }, "No chats yet")),
  );
}

/** The worst state of the enabled servers decides the footer's dot. */
function mcpSummary(servers) {
  const count = (state) => servers.filter((s) => s.state === state).length;
  const ready = count("ready");
  const login = count("needs-login") + count("untrusted");
  const failed = count("failed");
  const starting = count("starting");
  const dot = failed ? "failed" : login ? "needs-login" : starting ? "starting" : ready ? "ready" : "disabled";
  const parts = [
    ready && `${ready} ok`,
    starting && `${starting} starting`,
    login && h("span", { class: "chats-dock__warn" }, `${login} need you`),
    failed && h("span", { class: "chats-dock__error" }, `${failed} failed`),
  ].filter(Boolean);
  return { dot, parts };
}

function McpDock(state) {
  const { dot, parts } = mcpSummary(state.mcp);
  return h(
    "div",
    { class: "chats-dock" },
    h(
      "button",
      { class: ["chats-dock__button", state.ui.mcp.open && "chats-dock__button--open"], title: "MCP servers (<C-g>m · /mcp)", onClick: () => openMcp() },
      Dot(dot),
      h("span", { class: "chats-dock__label" }, "MCP servers"),
      h("span", { class: "chats-dock__count" }, state.mcp.length),
    ),
    h(
      "div",
      { class: "chats-dock__status" },
      parts.length ? parts.flatMap((p, i) => (i ? [h("span", { class: "sep" }, " · "), p] : [p])) : h("span", { class: "muted" }, "none yet · add one"),
    ),
  );
}

export function ChatsPanel(state) {
  return h(
    "aside",
    { class: ["chats", state.ui.focus === "chats" && "chats--focused"] },
    h("div", { class: "chats__header" }, "Chats"),
    h("button", { class: "chats__new", onClick: () => newChat() }, "New chat"),
    h(
      "div",
      { class: "chats__list" },
      state.projects.length
        ? state.projects.map((project) => ProjectGroup(project, state))
        : h("div", { class: "chats__empty" }, "No chats yet"),
    ),
    McpDock(state),
  );
}
