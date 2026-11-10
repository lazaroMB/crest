// The / commands under named groups. Pure, like organization.js.
//
// A built-in command gets its group from GROUP_OF (in crest, a `group` field on
// each CATALOG row); a plugin command gets a group named after its plugin.
// Typing "/" alone lists the groups; a group opens its commands; typing more
// filters every command and labels each row with its group.

export const GROUPS = [
  { id: "chat", label: "Chat", hint: "this conversation" },
  { id: "flows", label: "Flows", hint: "research, implement, simulate" },
  { id: "stages", label: "Stages", hint: "steer the running flow" },
  { id: "model", label: "Model & permissions", hint: "how the agent runs" },
  { id: "workspace", label: "Workspace", hint: "projects, MCP, plugins" },
  { id: "app", label: "App", hint: "help, theme, account, quit" },
];

export const GROUP_OF = {
  clear: "chat",
  open: "chat",
  "!": "chat",
  aside: "chat",
  research: "flows",
  implement: "flows",
  simulate: "flows",
  stage: "stages",
  back: "stages",
  skip: "stages",
  retry: "stages",
  abort: "stages",
  flow: "stages",
  reopen: "stages",
  model: "model",
  mode: "model",
  permissions: "model",
  project: "workspace",
  mcp: "workspace",
  plugin: "workspace",
  help: "app",
  theme: "app",
  login: "app",
  logout: "app",
  q: "app",
};

/** "/research --deep <problem>" -> "research" */
export const commandName = (usage) => usage.replace(/^\//, "").split(/\s/)[0];

/** How a command is typed: "/research", or "!" for the shell, which takes no slash. */
export const invocation = (usage) => (usage.startsWith("!") ? "!" : `/${commandName(usage)}`);

/** The group of one command row: { id, label }. */
export function groupOf(command) {
  if (command.source && command.source !== "crest") {
    return { id: `plugin:${command.source}`, label: `Plugin · ${command.source}`, hint: "commands it registered" };
  }
  const id = GROUP_OF[commandName(command.usage)];
  return id ? GROUPS.find((g) => g.id === id) : null;
}

/** Every group that has commands, in GROUPS order, then the plugin groups. */
export function groupedCommands(commands) {
  const groups = new Map();
  for (const command of commands) {
    const group = groupOf(command) ?? { id: "other", label: "Other", hint: "" };
    if (!groups.has(group.id)) groups.set(group.id, { ...group, commands: [] });
    groups.get(group.id).commands.push(command);
  }
  const order = (id) => {
    const at = GROUPS.findIndex((g) => g.id === id);
    return at === -1 ? GROUPS.length : at;
  };
  return [...groups.values()].sort((a, b) => order(a.id) - order(b.id));
}

/**
 * The palette for the text typed so far:
 *   "/"                -> { kind: "groups", rows: [group, …] }
 *   group opened       -> { kind: "commands", group, rows: [command, …] }
 *   "/res…"            -> { kind: "commands", rows: [command with .group, …] }
 *   anything else      -> null
 */
export function palette(commands, text, openGroup = null) {
  if (!text.startsWith("/") || text.includes("\n")) return null;
  const typed = text.trim().toLowerCase();
  const groups = groupedCommands(commands);
  if (typed === "/" && openGroup) {
    const group = groups.find((g) => g.id === openGroup);
    if (group) return { kind: "commands", group, rows: group.commands.map((c) => ({ ...c, group })) };
  }
  if (typed === "/") return { kind: "groups", rows: groups };
  const rows = commands
    .filter((c) => c.usage.toLowerCase().startsWith(typed) || typed.startsWith(c.usage.split(" ")[0]))
    .map((c) => ({ ...c, group: groupOf(c) }));
  return rows.length ? { kind: "commands", rows } : null;
}
