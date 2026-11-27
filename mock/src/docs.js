// Docs: markdown documents that belong to a chat. Pure functions, no DOM.
//
//   { id, chatId, title, markdown, assets: { name: dataUrl }, createdAt, updatedAt }
//
// A chat may own several docs; the panel shows one at a time and "Add to doc"
// goes to the chat's current one (the last it opened or wrote to). The file is
// not stored on the doc: it follows the chat (docDir). A chat in a project
// keeps its docs under that project's first directory; a chat with no project
// keeps them in crest's data directory. Images live in `assets` and the
// markdown points at them as `![alt](asset:name)`; on disk they sit in
// `assets/` beside the file. An export inlines them into one file.

/** The chat's docs, the most recently changed first. */
export function docsOfChat(docs, chatId) {
  return docs.filter((d) => d.chatId === chatId).sort((a, b) => b.updatedAt - a.updatedAt);
}

/** Every doc, the most recently changed first, for the sidebar; `query` filters by title. */
export function allDocs(docs, query = "") {
  const q = query.trim().toLowerCase();
  return [...docs].filter((d) => !q || d.title.toLowerCase().includes(q)).sort((a, b) => b.updatedAt - a.updatedAt);
}

/** `markdown` with `addition` appended as its own section, a blank line between them. */
export function appendMarkdown(markdown, addition) {
  const body = markdown.trimEnd();
  return body ? `${body}\n\n${addition.trim()}\n` : `${addition.trim()}\n`;
}

const escapeRegex = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * Where `text` sits in `markdown`, as [start, end], or null. Text selected in
 * the rendered doc has lost its markup (`**`, `` ` ``, `- `, `#`) and its line
 * breaks, so markup and any run of whitespace may sit between its characters.
 */
export function findSpan(markdown, text) {
  const exact = markdown.indexOf(text);
  if (exact !== -1) return [exact, exact + text.length];
  const words = text.trim().split(/\s+/).filter(Boolean);
  if (!words.length) return null;
  const markup = "[*_`]*";
  const gap = `${markup}(?:\\s+(?:[-*]\\s+|\\d+\\.\\s+|#{1,3}\\s+|>\\s?)?)${markup}`;
  const word = (w) => [...w].map(escapeRegex).join(markup);
  const match = new RegExp(`${markup}${words.map(word).join(gap)}${markup}`).exec(markdown);
  return match ? [match.index, match.index + match[0].length] : null;
}

/** `markdown` with the passage `from` replaced (found as findSpan finds it); unchanged when it is not there. */
export function replaceSpan(markdown, from, to) {
  const span = findSpan(markdown, from);
  return span ? markdown.slice(0, span[0]) + to + markdown.slice(span[1]) : markdown;
}

const escapeCell = (text) => String(text).replace(/\|/g, "\\|");

export function tableMarkdown(columns, rows, numeric = []) {
  const head = `| ${columns.map(escapeCell).join(" | ")} |`;
  const rule = `| ${columns.map((_, i) => (numeric[i] ? "---:" : "---")).join(" | ")} |`;
  const body = rows.map((row) => `| ${row.map(escapeCell).join(" | ")} |`);
  return [head, rule, ...body].join("\n");
}

export function tableCsv(columns, rows) {
  const quote = (c) => (/[",\n]/.test(c) ? `"${String(c).replace(/"/g, '""')}"` : String(c));
  return [columns, ...rows].map((row) => row.map(quote).join(",")).join("\n");
}

function chartTable(block) {
  return {
    columns: ["", ...block.series.map((s) => s.name)],
    rows: block.labels.map((label, i) => [label, ...block.series.map((s) => String(s.values[i]))]),
    numeric: [false, ...block.series.map(() => true)],
  };
}

const fence = (body, lang = "") => `\`\`\`${lang}\n${body}\n\`\`\``;

/**
 * A block as markdown for a doc. A chart becomes an image when `image` (its
 * asset name) is given, and its table otherwise.
 */
export function blockMarkdown(block, { image } = {}) {
  switch (block.kind) {
    case "user":
      return block.text
        .split("\n")
        .map((l) => `> ${l}`)
        .join("\n");
    case "text":
      return block.markdown;
    case "table":
      return tableMarkdown(block.columns, block.rows, block.numeric);
    case "chart": {
      const title = block.title || "Chart";
      if (image) return `![${title}](asset:${image})`;
      const t = chartTable(block);
      return `**${title}**\n\n${tableMarkdown(t.columns, t.rows, t.numeric)}`;
    }
    case "computed": {
      const values = block.values.map((v) => `- ${v.label}: **${v.value}**`).join("\n");
      return `**${block.method}** · ${block.provenance}${block.why ? `\n\n${block.why}` : ""}\n\n${values}`;
    }
    case "diff":
      return `\`${block.path}\`\n\n${fence(block.lines.map((l) => l.text).join("\n"), "diff")}`;
    case "tool":
      return `\`${block.name}\` · ${block.summary}${block.output ? `\n\n${fence(block.output)}` : ""}`;
    case "thinking":
      return `> *${block.summary || block.text}*`;
    case "error":
      return `> **Error:** ${block.message}${block.hint ? `\n> ${block.hint}` : ""}`;
    case "stage":
      return `**Stage ${block.index}/${block.total} · ${block.name}**`;
    default:
      return "";
  }
}

/** The blocks "Add to doc" offers: the ones a person would want in a document. */
const ADDABLE = new Set(["text", "table", "chart", "computed", "diff", "user"]);
export const canAddToDoc = (block) => ADDABLE.has(block.kind);

/** A doc's title from the chat it comes from. */
export const titleFor = (chatTitle) => (chatTitle && chatTitle !== "New chat" ? `${chatTitle} · notes` : "Untitled doc");

/** The markdown of a doc with its assets inlined as data URLs: one self-contained .md file. */
export function inlineAssets(doc) {
  return doc.markdown.replace(/\]\(asset:([^)]+)\)/g, (match, name) => (doc.assets?.[name] ? `](${doc.assets[name]})` : match));
}

/**
 * Applies a toolbar format to the selection [start, end) of `markdown`.
 * Returns the new text and where the selection sits in it, or null when a
 * link has no address. Wrapping a span that already has the marks removes
 * them; a line format toggles its prefix on every selected line.
 *
 * `format`: bold, italic, code, heading, list, quote, link, image.
 * `extra.url` for a link; `extra.alt` and `extra.name` for an image.
 */
export function formatSelection(markdown, start, end, format, extra = {}) {
  const from = Math.max(0, Math.min(start, end));
  const to = Math.min(markdown.length, Math.max(start, end));
  switch (format) {
    case "bold":
      return wrap(markdown, from, to, "**", "**");
    case "italic":
      return wrap(markdown, from, to, "*", "*");
    case "code":
      return markdown.slice(from, to).includes("\n") ? wrap(markdown, from, to, "```\n", "\n```") : wrap(markdown, from, to, "`", "`");
    case "heading":
      return prefixLines(markdown, from, to, "## ");
    case "list":
      return prefixLines(markdown, from, to, "- ");
    case "quote":
      return prefixLines(markdown, from, to, "> ");
    case "link":
      return extra.url ? insertLink(markdown, from, to, extra.url) : null;
    case "image":
      return insertBlock(markdown, from, to, `![${extra.alt || extra.name}](asset:${extra.name})`);
    default:
      return null;
  }
}

/** Wraps the selection in `before`/`after`, or removes that wrap when it is already there. An empty selection leaves the caret between the marks. */
function wrap(markdown, start, end, before, after) {
  const selected = markdown.slice(start, end);
  const around = markdown.slice(start - before.length, start) === before && markdown.slice(end, end + after.length) === after;
  // A single star each side of the selection is the inner pair of **bold**, not italic.
  const boldPair = before === "*" && markdown[start - 2] === "*" && markdown[end + 1] === "*";
  const inside =
    selected.startsWith(before) &&
    selected.endsWith(after) &&
    selected.length >= before.length + after.length &&
    !(before === "*" && selected.startsWith("**") && selected.endsWith("**"));
  if (around && !boldPair) return span(markdown.slice(0, start - before.length) + selected + markdown.slice(end + after.length), start - before.length, selected.length);
  if (inside) return span(markdown.slice(0, start) + selected.slice(before.length, -after.length) + markdown.slice(end), start, selected.length - before.length - after.length);
  const at = start + before.length;
  return span(markdown.slice(0, start) + before + selected + after + markdown.slice(end), selected ? at : at, selected ? selected.length : 0);
}

/** Toggles `prefix` on each line the selection touches. An empty line gains the prefix so the caret can start typing it. */
function prefixLines(markdown, start, end, prefix) {
  const lineStart = markdown.lastIndexOf("\n", Math.max(0, start - 1)) + 1;
  const lineEnd = markdown.indexOf("\n", end);
  const to = lineEnd === -1 ? markdown.length : lineEnd;
  const lines = markdown.slice(lineStart, to).split("\n");
  const on = lines.some((line) => line.trim()) && lines.every((line) => !line.trim() || line.startsWith(prefix));
  const next = lines
    .map((line) => (on ? line.slice(prefix.length) : line.trim() ? prefix + line : line))
    .join("\n");
  const body = next || prefix;
  return span(markdown.slice(0, lineStart) + body + markdown.slice(to), lineStart, body.length);
}

function insertLink(markdown, start, end, url) {
  const selected = markdown.slice(start, end);
  const linked = selected.match(/^\[([\s\S]*)\]\([^)]*\)$/);
  if (linked) return span(markdown.slice(0, start) + linked[1] + markdown.slice(end), start, linked[1].length);
  const label = selected || "link";
  const insert = `[${label}](${url})`;
  return span(markdown.slice(0, start) + insert + markdown.slice(end), start + 1, label.length);
}

/** Replaces the selection with `block`, on a line of its own. */
function insertBlock(markdown, start, end, block) {
  const lead = start > 0 && !markdown.slice(0, start).endsWith("\n\n") ? (markdown[start - 1] === "\n" ? "\n" : "\n\n") : "";
  const tail = end < markdown.length && markdown[end] !== "\n" ? "\n\n" : "";
  const at = start + lead.length;
  return span(markdown.slice(0, start) + lead + block + tail + markdown.slice(end), at, block.length);
}

const span = (markdown, start, length) => ({ markdown, start, end: start + length });

/** A file name for the doc: its title in kebab case. */
export function fileName(doc, ext) {
  const base = doc.title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "doc";
  return `${base}.${ext}`;
}

/** Where crest keeps its own files when a chat has no directory. */
export const CREST_DATA_DIR = "~/.local/share/crest";

/**
 * The directory a chat's docs live in. A project chat uses the project's
 * first directory (`<root>/.crest/docs/<chat id>`); a chat with no project
 * uses crest's data directory.
 */
export function docDir(chat, project) {
  const root = project?.roots?.[0];
  const base = root ? `${root}/.crest` : CREST_DATA_DIR;
  return `${base}/docs/${chat.id}`;
}

/** The markdown file of a doc, and where one of its images sits beside it. */
export const docFile = (doc, chat, project) => `${docDir(chat, project)}/${fileName(doc, "md")}`;
export const assetFile = (doc, chat, project, name) => `${docDir(chat, project)}/assets/${name}`;

/** `docFile` resolved from an organization: the chat the doc belongs to, and that chat's project. */
export function placeOf(doc, organization) {
  const chat = organization?.chats?.find((c) => c.id === doc.chatId) ?? { id: doc.chatId };
  const project = chat.projectId ? organization?.projects?.find((p) => p.id === chat.projectId) : null;
  return { chat, project, dir: docDir(chat, project), file: docFile(doc, chat, project) };
}
