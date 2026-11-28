// What the person (and the agent's turn events) can do with docs. Like
// actions.js, these talk to the mock server and update the state.

import { toBlob, toPng } from "html-to-image";
import { api } from "./api.js";
import { notice, openChat } from "./actions.js";
import { setState, setUi, state } from "./store.js";
import { markdown as renderMarkdown, mermaidBlock, toMarkdown } from "./lib/markdown.js";
import { allDocs, appendMarkdown, blockMarkdown, docsOfChat, fileName, inlineAssets, tableCsv, tableMarkdown, titleFor } from "./docs.js";

const VIEWS = ["hidden", "split", "doc"];
const SAVE_AFTER_MS = 400;
let saveTimer;

export const currentDoc = () => state.docs.find((d) => d.id === state.ui.docId) ?? null;

/** Adds or replaces a doc in the state. */
export function upsertDoc(doc) {
  const at = state.docs.findIndex((d) => d.id === doc.id);
  const docs = at === -1 ? [...state.docs, doc] : state.docs.map((d) => (d.id === doc.id ? { ...d, ...doc } : d));
  setState({ docs });
}

/** After a chat opens: its doc stays in the panel, or its latest doc takes the panel's place; no doc closes the panel. */
export function docForChat(chatId) {
  const mine = docsOfChat(state.docs, chatId);
  const keep = mine.find((d) => d.id === state.ui.docId);
  const docId = keep?.id ?? mine[0]?.id ?? null;
  setUi({
    docId,
    docView: docId ? state.ui.docView : "hidden",
    docEditing: false,
    diagramEdit: null,
    docMenu: null,
    blockMenu: null,
    selectionBar: null,
    promptContext: [],
  });
}

// The panel.

export function setDocView(docView) {
  const docId = state.ui.docId ?? docsOfChat(state.docs, state.chat?.id)[0]?.id ?? null;
  setUi({ docView, docId, docMenu: null, focus: docView === "doc" && state.ui.focus === "timeline" ? "prompt" : state.ui.focus });
}

export const cycleDocView = () => setDocView(VIEWS[(VIEWS.indexOf(state.ui.docView) + 1) % VIEWS.length]);

export const setDocSplit = (docSplit) => setUi({ docSplit: Math.min(0.75, Math.max(0.25, docSplit)) });

export const toggleDocMenu = (name) => setUi({ docMenu: state.ui.docMenu === name ? null : name, blockMenu: null });

export const setDocEditing = (docEditing) => setUi({ docEditing, diagramEdit: null, selectionBar: null });

// The diagram editor: code on the right, preview on the left. Saving writes the
// ```mermaid fence into the doc and goes back to the document.

export const DIAGRAM_TEMPLATES = {
  flowchart: "flowchart LR\n  start[Idea] --> draft[Draft]\n  draft --> review{Review}\n  review -->|ok| publish[Publish]\n  review -->|changes| draft",
  sequence: "sequenceDiagram\n  participant P as Person\n  participant A as Agent\n  P->>A: Write the summary\n  A-->>P: Draft ready",
  pie: 'pie title Time spent\n  "Research" : 40\n  "Writing" : 35\n  "Review" : 25',
  timeline: "timeline\n  title Release\n  Week 1 : Research\n  Week 2 : Draft : Review\n  Week 3 : Publish",
  usecase: 'usecase-beta\ndirection LR\nactor Customer("Customer")\nsystemBoundary "Order system"\n  Checkout("Place order")\nend\nCustomer --> Checkout',
};

/** Opens the editor on the `index`-th diagram of the doc, or on a new one inserted before top-level block `at`. */
export function openDiagramEditor(spec) {
  if (!currentDoc()) return;
  setUi({ diagramEdit: spec, docEditing: false, selectionBar: null, docMenu: null, docView: state.ui.docView === "hidden" ? "split" : state.ui.docView });
}

export const closeDiagramEditor = () => state.ui.diagramEdit && setUi({ diagramEdit: null });

/** The doc as the editor shows it, built off screen so a diagram can be put back by position. */
function renderedDoc(doc) {
  const page = document.createElement("article");
  if (doc.markdown.trim()) page.append(...renderMarkdown(doc.markdown, { assets: doc.assets, diagrams: true }));
  return page;
}

/** The source of the diagram being edited, or the starting text of a new one. */
export function diagramSource(spec) {
  const doc = currentDoc();
  if (!doc || !spec) return "";
  if (spec.mode === "new") return DIAGRAM_TEMPLATES.flowchart;
  return renderedDoc(doc).querySelectorAll(".md-mermaid")[spec.index]?.dataset.source ?? "";
}

export function saveDiagram(source) {
  const doc = currentDoc();
  const spec = state.ui.diagramEdit;
  if (!doc || !spec) return;
  const body = source.trim();
  const page = renderedDoc(doc);
  if (spec.mode === "edit") {
    const figure = page.querySelectorAll(".md-mermaid")[spec.index];
    if (!figure) return notice("warning", "that diagram is no longer in the doc");
    if (body) figure.dataset.source = body;
    else figure.remove();
  } else if (body) {
    page.insertBefore(mermaidBlock(body), page.children[spec.at] ?? null);
  }
  editDocMarkdown(toMarkdown(page));
  setUi({ diagramEdit: null });
  notice("info", !body ? (spec.mode === "edit" ? "diagram removed" : "nothing to add") : spec.mode === "new" ? "diagram added" : "diagram saved");
}

/** The next message references `id`: a chip on the prompt, and that doc is the one the agent edits. */
export function referenceDoc(id) {
  const doc = state.docs.find((d) => d.id === id);
  if (!doc) return notice("warning", "no such document");
  const item = { id: `ctx-${Date.now()}`, kind: "doc", docId: doc.id, title: doc.title, text: doc.title };
  const promptContext = [...state.ui.promptContext.filter((c) => !(c.kind === "doc" && c.docId === doc.id)), item];
  const here = doc.chatId === state.chat?.id;
  setUi({
    promptContext,
    focus: "prompt",
    ...(here ? { docId: doc.id, docView: state.ui.docView === "hidden" ? "split" : state.ui.docView } : {}),
  });
  notice("info", `referencing “${doc.title}”`);
}

const oneDoc = (query) => {
  const hits = allDocs(state.docs, query);
  if (!hits.length) return notice("warning", `no document matches “${query}”`);
  if (hits.length > 1) return notice("warning", `${hits.length} documents match “${query}”; pick one`);
  return hits[0];
};

/** Opens this chat's document, creating one when the chat has none. */
async function openEditor() {
  if (!state.chat) return;
  const mine = docsOfChat(state.docs, state.chat.id);
  const doc = mine.find((d) => d.id === state.ui.docId) ?? mine[0];
  if (doc) return openDoc(doc.id, state.ui.docView === "hidden" ? "split" : state.ui.docView);
  return newDoc();
}

async function showDoc(view) {
  if (state.chat && !docsOfChat(state.docs, state.chat.id).length) await newDoc();
  setDocView(view);
}

/** `/doc` and its subcommands. `args` is everything after the name. */
export async function docCommand(args) {
  const [sub, ...rest] = args;
  const query = rest.join(" ").trim();
  switch (sub) {
    case undefined:
      return openEditor();
    case "new":
      return newDoc(query || undefined);
    case "find":
    case "open": {
      const doc = query ? oneDoc(query) : notice("warning", "usage: /doc find <text>");
      if (doc) return openDoc(doc.id, "split");
      return;
    }
    case "use": {
      if (!query) {
        const doc = (currentDoc()?.chatId === state.chat?.id && currentDoc()) || docsOfChat(state.docs, state.chat?.id)[0];
        return doc ? referenceDoc(doc.id) : notice("warning", "no document to reference");
      }
      const doc = oneDoc(query);
      if (doc) return referenceDoc(doc.id);
      return;
    }
    case "add":
      return state.ui.selected ? addBlockToDoc(state.ui.selected) : notice("warning", "select a block in the chat first");
    case "split":
      return showDoc("split");
    case "only":
      return showDoc("doc");
    case "close":
      return setDocView("hidden");
    case "source":
      return currentDoc() ? setDocEditing(!state.ui.docEditing) : notice("warning", "no document open");
    case "export":
      if (!currentDoc()) return notice("warning", "no document open");
      return ["md", "html", "pdf", "copy"].includes(query) ? exportDoc(query) : notice("warning", "usage: /doc export md|html|pdf|copy");
    default: {
      const doc = oneDoc([sub, ...rest].join(" "));
      if (doc) return openDoc(doc.id, "split");
    }
  }
}

/** Opens a doc: its own chat first, then the doc, in `view` (from the sidebar: the doc alone). */
export async function openDoc(id, view) {
  const doc = state.docs.find((d) => d.id === id);
  if (!doc) return;
  if (doc.chatId !== state.openChatId) await openChat(doc.chatId);
  setUi({ docId: id, docView: view ?? (state.ui.docView === "hidden" ? "split" : state.ui.docView), docEditing: false, diagramEdit: null, docMenu: null });
}

/** A new empty doc owned by the open chat, shown beside it. */
export async function newDoc(title) {
  if (!state.chat) return null;
  const doc = await api.newDoc({ chatId: state.chat.id, title: title ?? titleFor(state.chat.title) });
  upsertDoc(doc);
  setUi({ docId: doc.id, docView: state.ui.docView === "hidden" ? "split" : state.ui.docView, docMenu: null, docEditing: false, diagramEdit: null });
  return doc;
}

export async function renameDoc(id, title) {
  const doc = await api.patchDoc(id, { title });
  upsertDoc(doc);
}

export async function deleteDoc(id) {
  await api.deleteDoc(id);
  setState({ docs: state.docs.filter((d) => d.id !== id) });
  if (state.ui.docId === id) docForChat(state.chat?.id);
  notice("info", "doc deleted");
}

/** The person edited the source: kept at once in the state, saved shortly after. `assets` merges images the toolbar inserted. */
export function editDocMarkdown(markdown, assets) {
  const doc = currentDoc();
  if (!doc) return;
  doc.markdown = markdown;
  if (assets) doc.assets = { ...doc.assets, ...assets };
  doc.updatedAt = Date.now();
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => api.patchDoc(doc.id, { markdown: doc.markdown, assets }), SAVE_AFTER_MS);
}

// Adding to a doc.

/** Appends `markdown` (and its images) to the chat's current doc, or to a new one; opens the panel on it. */
async function addToDoc(markdown, assets = {}, { fresh = false } = {}) {
  if (!state.chat || !markdown.trim()) return;
  let doc = !fresh && (currentDoc()?.chatId === state.chat.id ? currentDoc() : docsOfChat(state.docs, state.chat.id)[0]);
  if (!doc) doc = await newDoc();
  const saved = await api.patchDoc(doc.id, { markdown: appendMarkdown(doc.markdown, markdown), assets });
  upsertDoc({ ...saved, justAdded: Date.now() });
  setUi({ docId: saved.id, docView: state.ui.docView === "hidden" ? "split" : state.ui.docView, blockMenu: null, selectionBar: null });
  notice("info", `added to "${saved.title}"`);
}

const nextFrame = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));

/** The chart block on screen, rasterized as a PNG data URL, with its controls hidden. */
async function chartImage(blockId, as = "png") {
  setUi({ blockMenu: null });
  await nextFrame();
  const node = document.querySelector(`.tblock[data-id="${blockId}"] .chart`);
  if (!node) return null;
  node.classList.add("chart--capturing");
  try {
    // `--background` lives on `.window[data-theme]`, not `:root`, but custom
    // properties inherit down from any ancestor so reading it off the node works.
    const bg = getComputedStyle(node).getPropertyValue("--background").trim();
    const options = { pixelRatio: 2, backgroundColor: bg || undefined };
    return as === "blob" ? await toBlob(node, options) : await toPng(node, options);
  } finally {
    node.classList.remove("chart--capturing");
  }
}

export async function addBlockToDoc(blockId, { fresh = false } = {}) {
  const block = state.chat?.blocks.find((b) => b.id === blockId);
  if (!block) return;
  if (block.kind === "chart") {
    const name = `chart-${blockId}.png`;
    const png = await chartImage(blockId).catch(() => null);
    const md = png ? blockMarkdown(block, { image: name }) : blockMarkdown(block);
    return addToDoc(md, png ? { [name]: png } : {}, { fresh });
  }
  return addToDoc(blockMarkdown(block), {}, { fresh });
}

export async function addSelectionToDoc() {
  const bar = state.ui.selectionBar;
  if (!bar?.text) return;
  window.getSelection()?.removeAllRanges();
  await addToDoc(bar.text);
}

// Copying a block.

async function writeClipboard(text, what) {
  await navigator.clipboard?.writeText(text);
  notice("info", `copied ${what}`);
}

/** `format`: "markdown" | "csv" | "image". */
export async function copyBlock(blockId, format) {
  const block = state.chat?.blocks.find((b) => b.id === blockId);
  if (!block) return;
  setUi({ blockMenu: null });
  if (format === "image") {
    try {
      const blob = await chartImage(blockId, "blob");
      if (!blob) throw new Error("rendering failed");
      if (window.ClipboardItem && navigator.clipboard?.write) {
        await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })]);
        notice("info", "chart copied as image");
      } else {
        const url = URL.createObjectURL(blob);
        Object.assign(document.createElement("a"), { href: url, download: "chart.png" }).click();
        URL.revokeObjectURL(url);
        notice("warning", "clipboard images unsupported here · downloaded instead");
      }
    } catch (err) {
      notice("error", `couldn't copy chart image: ${err.message}`);
    }
    return;
  }
  if (format === "csv") {
    const t = block.kind === "chart" ? chartRows(block) : block;
    return writeClipboard(tableCsv(t.columns, t.rows), "as CSV");
  }
  if (block.kind === "chart") {
    const t = chartRows(block);
    return writeClipboard(tableMarkdown(t.columns, t.rows, t.numeric), "as a markdown table");
  }
  return writeClipboard(blockMarkdown(block), "as markdown");
}

const chartRows = (block) => ({
  columns: ["", ...block.series.map((s) => s.name)],
  rows: block.labels.map((label, i) => [label, ...block.series.map((s) => String(s.values[i]))]),
  numeric: [false, ...block.series.map(() => true)],
});

export const toggleBlockMenu = (blockId) => setUi({ blockMenu: state.ui.blockMenu === blockId ? null : blockId, docMenu: null });

// The selection bar.

export const showSelectionBar = (selectionBar) => setUi({ selectionBar, blockMenu: null, docMenu: null });

export const hideSelectionBar = () => state.ui.selectionBar && setUi({ selectionBar: null });

export async function copySelection() {
  const bar = state.ui.selectionBar;
  if (bar?.text) await writeClipboard(bar.text, "");
  setUi({ selectionBar: null });
}

/** The selected passage of the doc goes to the prompt as context; the next prompt says what to do with it. */
export function passSelectionToAgent() {
  const bar = state.ui.selectionBar;
  if (!bar?.text) return;
  const item = { id: `ctx-${Date.now()}`, kind: "doc-selection", docId: state.ui.docId, text: bar.text };
  setUi({ promptContext: [...state.ui.promptContext, item], selectionBar: null, focus: "prompt" });
  window.getSelection()?.removeAllRanges();
  document.querySelector(".prompt__text")?.focus();
}

export const removePromptContext = (id) => setUi({ promptContext: state.ui.promptContext.filter((c) => c.id !== id) });

// Export.

const PRINT_CSS = `
  body { font: 15px/1.6 -apple-system, "Segoe UI", sans-serif; color: #222; max-width: 760px; margin: 40px auto; padding: 0 24px; }
  h1, h2, h3 { line-height: 1.25; } h1 { font-size: 1.8em; } h2 { font-size: 1.35em; margin-top: 1.6em; }
  code { font: 0.9em "JetBrains Mono", Menlo, monospace; background: #f3f1ed; padding: 0 4px; border-radius: 3px; }
  pre { background: #f3f1ed; padding: 12px; border-radius: 6px; overflow-x: auto; } pre code { background: none; padding: 0; }
  table { border-collapse: collapse; margin: 12px 0; } th, td { border: 1px solid #ddd; padding: 4px 10px; } .md-num { text-align: right; }
  blockquote { margin: 0; padding-left: 12px; border-left: 3px solid #ddd; color: #555; } img { max-width: 100%; }
`;

function docHtml(doc) {
  const body = document.createElement("div");
  body.append(...renderMarkdown(doc.markdown, { assets: doc.assets }));
  return `<!doctype html><html><head><meta charset="utf-8"><title>${doc.title}</title><style>${PRINT_CSS}</style></head><body>${body.innerHTML}</body></html>`;
}

function download(name, type, text) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const link = Object.assign(document.createElement("a"), { href: url, download: name });
  link.click();
  URL.revokeObjectURL(url);
}

/** `format`: "md" (one file, images inlined) | "html" | "pdf" (the browser's print dialog) | "copy". */
export async function exportDoc(format) {
  const doc = currentDoc();
  setUi({ docMenu: null });
  if (!doc) return;
  if (format === "md") download(fileName(doc, "md"), "text/markdown", inlineAssets(doc));
  else if (format === "html") download(fileName(doc, "html"), "text/html", docHtml(doc));
  else if (format === "copy") await writeClipboard(doc.markdown, "the doc as markdown");
  else if (format === "pdf") {
    const win = window.open("", "_blank");
    if (!win) return notice("warning", "the browser blocked the print window");
    win.document.write(docHtml(doc));
    win.document.close();
    win.onload = () => win.print();
  }
}
