// The sidebar (crates/ui/src/chats.rs), in three widths (src/modes.js):
//
//   full    New chat · Search, then Projects (each with its directories and
//           chats), Chats (the standalone ones), and, if anything is
//           archived, a folded "Archived" section at the bottom
//   strip   icons only: expand, new chat, search, projects, one initial per project
//   hidden  nothing; a small button at the top left brings it back
//
// A click opens a chat; a right click or ⋯ opens its menu: New chat (in the
// same project), Rename, Move to project (or a new one: promoting the chat),
// Remove from project, Archive, Delete. A project's menu adds a chat, adds
// directories from anywhere, renames it, pins it, and archives it. Archiving
// hides a chat or a project from the lists above without deleting it; the
// Archived section is the only place to see it again and restore it.

import { h } from "../../lib/h.js";
import {
  addProjectRoot,
  archiveChat,
  branchChat,
  archiveProject,
  createProject,
  deleteChat,
  moveChat,
  newChat,
  openChat,
  pinProject,
  removeProjectRoot,
  renameChat,
  renameProject,
  setSearch,
  setSidebar,
  toggleArchived,
  toggleProject,
  unarchiveChat,
  unarchiveProject,
} from "../../actions.js";
import { setUi } from "../../store.js";
import { archivedModel, chatRoots, sidebarModel } from "../../organization.js";
import { icon } from "../icons.js";
import { Brand } from "../brand/brand.js";
import "./chats-panel.scss";

const BADGE = { running: "●", waiting: "⏳" };

const close = () => setUi({ menu: null });

const iconButton = (name, title, onClick, extra = {}) =>
  h("button", { class: ["icon-button", extra.class], title, "aria-label": title, onClick }, icon(name));

/** The Crest mark. Hovering it shows the panel icon, so the button still reads as collapse or expand. */
function logoButton(title, onClick) {
  return h(
    "button",
    { class: "icon-button chats__logo", title, "aria-label": title, onClick },
    h("span", { class: "chats__logo-mark" }, Brand({ height: 13 })),
    h("span", { class: "chats__logo-hover" }, icon("panel")),
  );
}

/** A text field that runs `onSubmit(value)` on Enter and closes the menu on Esc. */
function field(value, placeholder, onSubmit, { autofocus = true, select = true } = {}) {
  const input = h("input", {
    class: "chat-menu__input",
    value: value ?? "",
    placeholder,
    onKeydown: (e) => {
      e.stopPropagation();
      if (e.key === "Enter") onSubmit(input.value);
      if (e.key === "Escape") close();
    },
  });
  if (autofocus) requestAnimationFrame(() => (select ? input.select() : input.focus()));
  return input;
}

const row = (label, onClick, kind) =>
  h("button", { class: ["chat-menu__row", kind && `chat-menu__row--${kind}`], onClick }, label);

const hint = (text) => h("div", { class: "chat-menu__hint" }, text);

function NewProjectForm(chat) {
  const root = field("", "~/dev/my-repo", () => submit(), { autofocus: false });
  const name = field(chat?.title ?? "", "Project name", () => root.focus());
  const submit = () => createProject(name.value.trim(), root.value.trim(), chat?.id);
  return h(
    "div",
    { class: "chat-menu" },
    h("div", { class: "chat-menu__label" }, chat ? "Promote to a new project" : "New project"),
    name,
    root,
    hint("A directory anywhere on disk; add more later"),
    row("Create", submit),
    row("Cancel", close),
  );
}

function ChatMenu(chat, menu, org) {
  if (menu.mode === "rename") {
    return h("div", { class: "chat-menu" }, field(chat.title, "Chat name", (v) => renameChat(chat.id, v)), hint("Enter saves · Esc cancels"));
  }
  if (menu.mode === "delete") {
    return h(
      "div",
      { class: "chat-menu" },
      h("div", { class: "chat-menu__row chat-menu__row--danger chat-menu__row--static" }, "Delete this chat?"),
      row("Delete", () => deleteChat(chat.id), "danger"),
      row("Cancel", close),
    );
  }
  if (menu.mode === "move") {
    const others = org.projects.filter((p) => p.id !== chat.projectId);
    return h(
      "div",
      { class: "chat-menu" },
      h("div", { class: "chat-menu__label" }, chat.projectId ? "Move to project" : "Add to project"),
      others.map((p) => row([icon("folder", 14), " ", p.name], () => moveChat(chat.id, p.id))),
      row([icon("plus", 14), " New project…"], () => setUi({ menu: { chatId: chat.id, mode: "new-project" } })),
      row("Cancel", close),
    );
  }
  if (menu.mode === "new-project") return NewProjectForm(chat);
  return h(
    "div",
    { class: "chat-menu" },
    row([icon("plus", 14), " New chat"], () => newChat(chat.projectId ?? null)),
    row([icon("branch", 14), " Branch chat"], () => branchChat(chat.id)),
    row("Rename", () => setUi({ menu: { chatId: chat.id, mode: "rename" } })),
    row(chat.projectId ? "Move to project…" : "Add to project…", () => setUi({ menu: { chatId: chat.id, mode: "move" } })),
    chat.projectId && row("Remove from project", () => moveChat(chat.id, null)),
    row("Archive", () => archiveChat(chat.id)),
    row("Delete", () => setUi({ menu: { chatId: chat.id, mode: "delete" } }), "danger"),
    row("Cancel", close),
  );
}

function ProjectMenu(project, menu) {
  if (menu.mode === "rename") {
    return h("div", { class: "chat-menu" }, field(project.name, "Project name", (v) => renameProject(project.id, v)), hint("Enter saves · Esc cancels"));
  }
  if (menu.mode === "add-root") {
    return h(
      "div",
      { class: "chat-menu" },
      h("div", { class: "chat-menu__label" }, "Add a directory"),
      field("", "/any/path/on/disk", (v) => addProjectRoot(project.id, v)),
      hint("Every chat of the project reaches it"),
    );
  }
  const last = project.roots.length === 1;
  return h(
    "div",
    { class: "chat-menu" },
    row([icon("plus", 14), " New chat here"], () => newChat(project.id)),
    h("div", { class: "chat-menu__label" }, "Directories"),
    project.roots.map((root) =>
      h(
        "div",
        { class: "chat-menu__root", title: root },
        h("span", { class: "chat-menu__path" }, h("bdi", {}, root)),
        h(
          "button",
          {
            class: "chat-menu__remove",
            title: last ? "A project keeps at least one directory" : "Remove from the project",
            disabled: last,
            onClick: () => removeProjectRoot(project.id, root),
          },
          icon("close", 12),
        ),
      ),
    ),
    row("Add directory…", () => setUi({ menu: { projectId: project.id, mode: "add-root" } })),
    row("Rename", () => setUi({ menu: { projectId: project.id, mode: "rename" } })),
    row(project.pinned ? "Unpin" : "Pin to top", () => pinProject(project.id)),
    row("Archive", () => archiveProject(project.id)),
    row("Cancel", close),
  );
}

function ChatRow(chat, state, nested) {
  const menu = state.ui.menu;
  const open = chat.id === state.openChatId;
  const openMenu = (e) => {
    e.preventDefault();
    e.stopPropagation();
    setUi({ menu: { chatId: chat.id, mode: "menu" } });
  };
  return [
    h(
      "div",
      {
        class: ["chat-row", nested && "chat-row--nested", open && "chat-row--open"],
        title: chat.title,
        onClick: () => openChat(chat.id),
        onContextmenu: openMenu,
      },
      h("span", { class: "chat-row__title" }, chat.title || "New chat"),
      chat.branchedFrom && h("span", { class: "chat-row__branch", title: `Branched from "${chat.branchedFrom.title}"` }, icon("branch", 12)),
      chat.activity && h("span", { class: "chat-row__badge" }, BADGE[chat.activity]),
      h("button", { class: "chat-row__more", title: "Chat actions", onClick: openMenu }, icon("more", 14)),
    ),
    menu?.chatId === chat.id && ChatMenu(chat, menu, state.organization),
  ];
}

function ProjectGroup(project, state) {
  const folded = state.ui.folded[project.id] && state.ui.search === null;
  const menu = state.ui.menu;
  const openMenu = (e) => {
    e.preventDefault();
    e.stopPropagation();
    setUi({ menu: { projectId: project.id, mode: "menu" } });
  };
  const dirs = project.roots.length;
  return h(
    "div",
    { class: "project" },
    h(
      "div",
      { class: "project__row", title: project.roots.join("\n"), onClick: () => toggleProject(project.id), onContextmenu: openMenu },
      h("span", { class: "project__marker" }, folded ? "▸" : "▾"),
      h("span", { class: "project__name" }, project.name),
      project.pinned && h("span", { class: "project__pin", title: "Pinned" }, icon("pin", 12)),
      dirs > 1 && h("span", { class: "project__dirs", title: project.roots.join("\n") }, `${dirs} dirs`),
      h("button", { class: "project__action", title: "New chat in this project", onClick: (e) => (e.stopPropagation(), newChat(project.id)) }, icon("plus", 14)),
      h("button", { class: "project__action", title: "Project actions", onClick: openMenu }, icon("more", 14)),
    ),
    menu?.projectId === project.id && ProjectMenu(project, menu),
    !folded &&
      (project.chats.length
        ? project.chats.map((chat) => ChatRow(chat, state, true))
        : h("div", { class: "chats__empty chats__empty--nested" }, state.ui.search ? "No match" : "No chats yet")),
  );
}

function Section(title, action, body) {
  return h("section", { class: "chats__section" }, h("div", { class: "chats__heading" }, h("span", {}, title), action), body);
}

/** Archived projects and chats, folded by default; the only place left to see or restore them. */
function ArchivedSection(state) {
  const { projects, chats } = archivedModel(state.organization);
  if (!projects.length && !chats.length) return null;
  const open = state.ui.archivedOpen;
  const restoreRow = (title, onRestore) =>
    h(
      "div",
      { class: "archived-row" },
      h("span", { class: "archived-row__title" }, title),
      h("button", { class: "archived-row__restore", title: "Restore", "aria-label": "Restore", onClick: onRestore }, icon("restore", 14)),
    );
  return h(
    "section",
    { class: "chats__section" },
    h(
      "button",
      { class: "chats__archived-toggle", onClick: () => toggleArchived() },
      h("span", { class: "project__marker" }, open ? "▾" : "▸"),
      icon("archive", 14),
      h("span", {}, `Archived (${projects.length + chats.length})`),
    ),
    open &&
      h(
        "div",
        { class: "chats__archived-list" },
        projects.map((p) => restoreRow(`${p.name} (${p.chats.length})`, () => unarchiveProject(p.id))),
        chats.map((c) => restoreRow(c.title || "New chat", () => unarchiveChat(c.id))),
      ),
  );
}

function SearchField(state) {
  const input = h("input", {
    class: "chats__search-input",
    value: state.ui.search,
    placeholder: "Search chats and projects",
    onInput: () => setUi({ search: input.value }),
    onKeydown: (e) => {
      e.stopPropagation();
      if (e.key === "Escape") setSearch(null);
    },
  });
  requestAnimationFrame(() => {
    if (document.activeElement !== input) {
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    }
  });
  return h("div", { class: "chats__search" }, icon("search", 14), input, iconButton("close", "Close search", () => setSearch(null)));
}

function Full(state) {
  const { projects, standalone } = sidebarModel(state.organization, { query: state.ui.search ?? "" });
  const menu = state.ui.menu;
  const newProject = menu?.mode === "new-project" && !menu.chatId;
  return h(
    "aside",
    { class: ["chats", state.ui.focus === "chats" && "chats--focused"], "data-sidebar": "full" },
    h(
      "div",
      { class: "chats__top" },
      logoButton("Collapse the sidebar (<C-g>s)", () => setSidebar("strip")),
      h("span", { class: "chats__brand" }, "Crest"),
    ),
    h(
      "nav",
      { class: "chats__nav" },
      h("button", { class: "chats__nav-row", onClick: () => newChat(null) }, icon("chat"), h("span", {}, "New chat")),
      h("button", { class: "chats__nav-row", onClick: () => setSearch(state.ui.search === null ? "" : null) }, icon("search"), h("span", {}, "Search")),
    ),
    state.ui.search !== null && SearchField(state),
    h(
      "div",
      { class: "chats__list" },
      Section(
        "Projects",
        iconButton("plus", "New project", () => setUi({ menu: { chatId: null, mode: "new-project" } })),
        [newProject && NewProjectForm(null), projects.length ? projects.map((p) => ProjectGroup(p, state)) : h("div", { class: "chats__empty" }, "No projects")],
      ),
      Section(
        "Chats",
        null,
        standalone.length ? standalone.map((c) => ChatRow(c, state, false)) : h("div", { class: "chats__empty" }, state.ui.search ? "No match" : "Chats without a project show here"),
      ),
      ArchivedSection(state),
    ),
  );
}

function Strip(state) {
  const { projects } = sidebarModel(state.organization);
  const openProject = (p) => (p.chats[0] ? openChat(p.chats[0].id) : newChat(p.id));
  const current = state.organization.chats.find((c) => c.id === state.openChatId)?.projectId;
  return h(
    "aside",
    { class: ["chats", "chats--strip", state.ui.focus === "chats" && "chats--focused"], "data-sidebar": "strip" },
    logoButton("Expand the sidebar (<C-g>s)", () => setSidebar("full")),
    iconButton("chat", "New chat", () => newChat(null), { class: "strip-new" }),
    iconButton("search", "Search", () => setSearch(""), { class: "strip-search" }),
    iconButton("folder", "Projects", () => setSidebar("full"), { class: "strip-projects" }),
    h("div", { class: "chats__strip-sep" }),
    projects.map((p) =>
      h(
        "button",
        {
          class: ["strip-project", p.id === current && "strip-project--open"],
          title: p.name,
          "aria-label": p.name,
          onClick: () => openProject(p),
        },
        h("span", { class: "strip-project__initial" }, p.name.slice(0, 1).toUpperCase()),
        h("span", { class: "strip-project__name" }, p.name),
      ),
    ),
  );
}

export function ChatsPanel(state) {
  if (state.ui.sidebar === "hidden") {
    return h("button", { class: "sidebar-reveal", title: "Show the sidebar (<C-g>s)", "aria-label": "Show the sidebar", onClick: () => setSidebar("full") }, icon("panel"));
  }
  return state.ui.sidebar === "strip" ? Strip(state) : Full(state);
}

/** The directories the open chat reaches, for the status bar: "crest +1" or "no directory". */
export function workspaceLabel(state) {
  const roots = chatRoots(state.organization, state.openChatId);
  const chat = state.organization.chats.find((c) => c.id === state.openChatId);
  const project = state.organization.projects.find((p) => p.id === chat?.projectId);
  if (!project) return "no directory";
  return roots.length > 1 ? `${project.name} +${roots.length - 1}` : project.name;
}
