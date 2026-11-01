// Chat modes and sidebar widths. Pure, like organization.js.

/** What the prompt's + button offers. The mode stays chosen until changed back. */
export const CHAT_MODES = [
  { id: "chat", label: "Chat", hint: "talk to the agent", placeholder: "Ask crest…" },
  { id: "research", label: "Research", hint: "5 stages, gates where you decide", placeholder: "Describe the problem to research…" },
  { id: "deep", label: "Deep research", hint: "14 stages, experiments and a prototype", placeholder: "Describe the problem for a deep research…" },
];

export const modeOf = (id) => CHAT_MODES.find((m) => m.id === id) ?? CHAT_MODES[0];

/** The line the prompt sends for `text` in `mode`: the same command a person could type. */
export function modeCommand(mode, text) {
  const body = text.trim();
  if (!body || body.startsWith("/") || body.startsWith("!")) return body;
  if (mode === "research") return `/research ${body}`;
  if (mode === "deep") return `/research --deep ${body}`;
  return body;
}

export const nextMode = (id) => CHAT_MODES[(CHAT_MODES.findIndex((m) => m.id === id) + 1) % CHAT_MODES.length].id;

/** The sidebar's widths in pixels; the strip keeps only icons. */
export const SIDEBAR_WIDTH = { full: 240, strip: 48, hidden: 0 };

/** `<C-g>s`: full → strip → hidden → full. */
export const nextSidebar = (state) => ({ full: "strip", strip: "hidden", hidden: "full" })[state] ?? "full";
