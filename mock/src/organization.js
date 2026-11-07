// Chats and projects, apart from directories. Pure: no DOM, no fetch, so the
// mock server and the spike scripts use it as is.
//
//   organization = { projects: [Project], chats: [Chat] }
//   Project = { id, name, roots: [path, …], pinned, archived? }   at least one root
//   Chat    = { id, title, projectId | null, updatedAt, activity?, archived?, branchedFrom? }
//
// `branchedFrom: { chatId, title, atBlock }` marks a chat that was branched from
// another: it started as a copy of that chat's blocks up to `atBlock`, and from
// there the two conversations diverge. The original chat is untouched.
//
// A chat with `projectId: null` is standalone: it has no directory until it is
// promoted into a project. Every function returns a new organization and
// throws an Error with a message the UI can show when the change is refused.
//
// Archiving takes a chat or a project out of `sidebarModel`'s lists without
// deleting it; `archivedModel` is the only place it still shows, so it can be
// restored. Archiving a project leaves its chats where they are: they are
// still that project's chats, just hidden with it until it comes back.

const normalize = (path) => path.trim().replace(/\/+$/, "") || "/";

/** The organization for today's catalog: each directory becomes a project of one root. */
export function fromDirectoryCatalog(catalog, now = 0) {
  const projects = catalog.map((dir) => ({
    id: dir.id ?? `p-${dir.name}`,
    name: dir.name,
    roots: [normalize(dir.path)],
    pinned: false,
  }));
  const chats = catalog.flatMap((dir, d) =>
    dir.chats.map((chat, i) => ({
      id: chat.id,
      title: chat.title,
      projectId: projects[d].id,
      updatedAt: chat.updatedAt ?? now - i,
      activity: chat.activity,
    })),
  );
  return { projects, chats };
}

const project = (org, id) => {
  const found = org.projects.find((p) => p.id === id);
  if (!found) throw new Error(`no project ${id}`);
  return found;
};

const withProject = (org, id, change) => ({
  ...org,
  projects: org.projects.map((p) => (p.id === id ? change(p) : p)),
});

const withChat = (org, id, change) => {
  if (!org.chats.some((c) => c.id === id)) throw new Error(`no chat ${id}`);
  return { ...org, chats: org.chats.map((c) => (c.id === id ? change(c) : c)) };
};

export function addChat(org, { id, title = "New chat", projectId = null, now = Date.now(), branchedFrom = null }) {
  if (projectId) project(org, projectId);
  return { ...org, chats: [{ id, title, projectId, updatedAt: now, ...(branchedFrom ? { branchedFrom } : {}) }, ...org.chats] };
}

/**
 * A branch of a chat: a new chat in the same project, titled "<title> (branch)",
 * carrying where it came from. The original chat stays as it is. The caller
 * copies the blocks up to the branch point into the new chat's own history.
 */
export function branchChat(org, chatId, newId, now = Date.now()) {
  const chat = org.chats.find((c) => c.id === chatId);
  if (!chat) throw new Error(`no chat ${chatId}`);
  const title = `${chat.title || "New chat"} (branch)`;
  return addChat(org, { id: newId, title, projectId: chat.projectId, now, branchedFrom: { chatId, title: chat.title, atBlock: null } });
}

export function createProject(org, { id, name, roots }) {
  const clean = [...new Set((roots ?? []).map(normalize).filter(Boolean))];
  if (!name?.trim()) throw new Error("a project needs a name");
  if (!clean.length) throw new Error("a project needs at least one directory");
  if (org.projects.some((p) => p.id === id)) throw new Error(`project ${id} exists`);
  return { ...org, projects: [...org.projects, { id, name: name.trim(), roots: clean, pinned: false }] };
}

/** Moves a chat into a project, or back out of it with `projectId: null`. Its id and history stay. */
export function moveChat(org, chatId, projectId) {
  if (projectId) project(org, projectId);
  return withChat(org, chatId, (c) => ({ ...c, projectId }));
}

export const promoteChat = moveChat;

export function addRoot(org, projectId, path) {
  const root = normalize(path);
  return withProject(org, projectId, (p) => (p.roots.includes(root) ? p : { ...p, roots: [...p.roots, root] }));
}

export function removeRoot(org, projectId, path) {
  const p = project(org, projectId);
  const root = normalize(path);
  if (p.roots.length === 1 && p.roots[0] === root) throw new Error("a project keeps at least one directory");
  return withProject(org, projectId, (q) => ({ ...q, roots: q.roots.filter((r) => r !== root) }));
}

export function renameProject(org, projectId, name) {
  if (!name?.trim()) throw new Error("a project needs a name");
  return withProject(org, projectId, (p) => ({ ...p, name: name.trim() }));
}

export function togglePin(org, projectId) {
  return withProject(org, projectId, (p) => ({ ...p, pinned: !p.pinned }));
}

export function archiveProject(org, projectId) {
  project(org, projectId);
  return withProject(org, projectId, (p) => ({ ...p, archived: true }));
}

export function unarchiveProject(org, projectId) {
  project(org, projectId);
  return withProject(org, projectId, (p) => ({ ...p, archived: false }));
}

export function archiveChat(org, chatId) {
  return withChat(org, chatId, (c) => ({ ...c, archived: true }));
}

export function unarchiveChat(org, chatId) {
  return withChat(org, chatId, (c) => ({ ...c, archived: false }));
}

export function renameChat(org, chatId, title) {
  return withChat(org, chatId, (c) => ({ ...c, title }));
}

export function removeChat(org, chatId) {
  return { ...org, chats: org.chats.filter((c) => c.id !== chatId) };
}

export function touchChat(org, chatId, now = Date.now()) {
  return withChat(org, chatId, (c) => ({ ...c, updatedAt: now }));
}

/** The directories a chat's tools may use: its project's, or none for a standalone chat. */
export function chatRoots(org, chatId) {
  const chat = org.chats.find((c) => c.id === chatId);
  if (!chat?.projectId) return [];
  return project(org, chat.projectId).roots;
}

const newestFirst = (a, b) => b.updatedAt - a.updatedAt;
const byLastChat = (chats) => (a, b) =>
  Number(b.pinned) - Number(a.pinned) || last(chats, b.id) - last(chats, a.id) || a.name.localeCompare(b.name);
const last = (chats, projectId) => Math.max(-Infinity, ...chats.filter((c) => c.projectId === projectId).map((c) => c.updatedAt));

/**
 * What the sidebar lists: the projects (pinned first, then by their newest chat),
 * each with its chats newest first, then the standalone chats. A `query` keeps
 * the chats whose title matches and every chat of a project whose name matches.
 * An archived project, and an archived chat wherever it is, show here no more;
 * `archivedModel` is where they go instead.
 */
export function sidebarModel(org, { query = "" } = {}) {
  const q = query.trim().toLowerCase();
  const matches = (text) => !q || text.toLowerCase().includes(q);
  const projects = [...org.projects]
    .filter((p) => !p.archived)
    .sort(byLastChat(org.chats))
    .flatMap((p) => {
      const all = org.chats.filter((c) => c.projectId === p.id && !c.archived).sort(newestFirst);
      const chats = matches(p.name) ? all : all.filter((c) => matches(c.title));
      return !q || chats.length || matches(p.name) ? [{ ...p, chats }] : [];
    });
  const standalone = org.chats.filter((c) => !c.projectId && !c.archived && matches(c.title)).sort(newestFirst);
  return { projects, standalone };
}

/**
 * What the "Archived" section lists: the archived projects, each with every
 * chat it still has, and the archived chats of an active project or of none,
 * newest first. Unarchiving a project does not touch its chats' own flag;
 * unarchiving a chat does not touch its project's.
 */
export function archivedModel(org) {
  const archivedProjectIds = new Set(org.projects.filter((p) => p.archived).map((p) => p.id));
  const projects = org.projects
    .filter((p) => p.archived)
    .map((p) => ({ ...p, chats: org.chats.filter((c) => c.projectId === p.id).sort(newestFirst) }));
  const chats = org.chats.filter((c) => c.archived && !archivedProjectIds.has(c.projectId)).sort(newestFirst);
  return { projects, chats };
}
