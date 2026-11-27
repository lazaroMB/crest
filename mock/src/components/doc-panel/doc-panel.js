// The doc panel, to the right of the chat (or in its place):
//
//   ┌ ▤ v0.6.0 release notes ▾  · Release v0.6.0 in numbers   saved  ✎ edit  ⇪ export  ◧ ▣ × ┐
//   │                                                                                        │
//   │   the rendered doc is the editor; the format bar works on the selection. "source"      │
//   │   shows the markdown, which is what gets saved                                        │
//   │                                                                                        │
//   └ agent: Rewrote the selected passage…                                       show chat ┘  (doc view only)
//
// `state.ui.docView`: hidden | split | doc. In "doc" the chat's timeline is
// hidden but its prompt stays below, so the person keeps talking to the agent
// that edits the doc. Each doc belongs to one chat; ▾ lists that chat's docs.
//
// Mounted once (mountDocPanel) so the source keeps its text, caret and focus
// while the head and the rendered doc are drawn again.

import { h } from "../../lib/h.js";
import { markdown, toMarkdown } from "../../lib/markdown.js";
import { paintMermaid } from "../../lib/mermaid-view.js";
import {
  DIAGRAM_TEMPLATES,
  closeDiagramEditor,
  deleteDoc,
  diagramSource,
  editDocMarkdown,
  exportDoc,
  newDoc,
  openDiagramEditor,
  openDoc,
  renameDoc,
  saveDiagram,
  setDocEditing,
  setDocSplit,
  setDocView,
  toggleDocMenu,
} from "../../doc-actions.js";
import { docsOfChat, formatSelection, placeOf } from "../../docs.js";
import { state as store } from "../../store.js";
import { icon } from "../icons.js";
import "./doc-panel.scss";

const iconButton = (name, title, onClick, extra = {}) =>
  h("button", { class: ["doc-panel__icon", extra.on && "doc-panel__icon--on"], title, "aria-label": title, onClick }, icon(name, 14));

const menuRow = (iconName, label, onClick, kind) =>
  h("button", { class: ["popmenu__row", kind && `popmenu__row--${kind}`], onClick }, icon(iconName, 14), h("span", {}, label));

function ExportMenu() {
  return h(
    "div",
    { class: "popmenu doc-panel__menu doc-panel__menu--right", role: "menu" },
    h("div", { class: "popmenu__label" }, "Export"),
    menuRow("export", "Markdown (.md)", () => exportDoc("md")),
    menuRow("export", "Web page (.html)", () => exportDoc("html")),
    menuRow("export", "PDF (print dialog)", () => exportDoc("pdf")),
    h("div", { class: "popmenu__sep" }),
    menuRow("copy", "Copy as markdown", () => exportDoc("copy")),
  );
}

function SwitchMenu(state, doc) {
  const mine = docsOfChat(state.docs, state.chat?.id);
  return h(
    "div",
    { class: "popmenu doc-panel__menu", role: "menu" },
    h("div", { class: "popmenu__label" }, "Docs of this chat"),
    mine.map((d) => menuRow("doc", d.title, () => openDoc(d.id), d.id === doc?.id && "on")),
    h("div", { class: "popmenu__sep" }),
    menuRow("plus", "New doc", () => newDoc()),
    doc && menuRow("close", "Delete this doc", () => deleteDoc(doc.id), "danger"),
  );
}

/** The last thing the agent said, under the doc when the chat is out of sight. */
function Peek(state) {
  const last = [...(state.chat?.blocks ?? [])].reverse().find((b) => b.kind === "text");
  if (!last) return null;
  const line = last.markdown.replace(/[*`#>]/g, "").split("\n").find((l) => l.trim()) ?? "";
  return h(
    "div",
    { class: "doc-panel__peek" },
    h("span", { class: "doc-panel__peek-who" }, "agent"),
    h("span", { class: "doc-panel__peek-text" }, line),
    h("button", { class: "doc-panel__peek-show", onClick: () => setDocView("split") }, "show chat"),
  );
}

const FORMAT_BUTTONS = [
  ["bold", "Bold (Ctrl+B)", "B"],
  ["italic", "Italic (Ctrl+I)", "I"],
  ["heading", "Heading", "H"],
  ["list", "List", "list"],
  ["quote", "Quote", "quote"],
  ["code", "Code", "code"],
  ["link", "Link (Ctrl+K)", "link"],
  ["image", "Insert image", "image"],
  ["diagram", "New diagram", "diagram"],
];

/** A file name that does not collide with an asset the doc already has. */
function assetName(file, assets) {
  const clean = file.name.toLowerCase().replace(/[^a-z0-9.]+/g, "-").replace(/^-+|-+$/g, "") || "image.png";
  if (!assets[clean]) return clean;
  const dot = clean.lastIndexOf(".");
  const stem = dot === -1 ? clean : clean.slice(0, dot);
  const ext = dot === -1 ? "" : clean.slice(dot);
  let n = 2;
  while (assets[`${stem}-${n}${ext}`]) n += 1;
  return `${stem}-${n}${ext}`;
}

/** The format bar. In the document it formats the visible selection; in the source view it edits the markdown. */
function FormatBar(source, preview) {
  let range = [0, 0];
  let saved = null;
  const sourceMode = () => store.ui.docEditing;
  const article = () => preview.querySelector(".doc-panel__page");
  const remember = (e) => {
    e.preventDefault();
    if (sourceMode()) range = [source.selectionStart, source.selectionEnd];
    else rememberSelection();
  };
  const rememberSelection = () => {
    const sel = window.getSelection();
    saved = sel?.rangeCount && preview.contains(sel.anchorNode) ? sel.getRangeAt(0).cloneRange() : saved;
  };
  const restore = () => {
    if (!saved) return;
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(saved);
  };
  const saveArticle = (assets) => {
    const page = article();
    if (page) editDocMarkdown(toMarkdown(page), assets);
  };
  const applySource = (format, extra) => {
    const [start, end] = document.activeElement === source ? [source.selectionStart, source.selectionEnd] : range;
    const result = formatSelection(source.value, start, end, format, extra);
    if (!result) return;
    source.value = result.markdown;
    source.focus();
    source.setSelectionRange(result.start, result.end);
    editDocMarkdown(source.value, extra?.assets);
  };
  const run = (name, value) => document.execCommand(name, false, value);
  const formatBlock = (tag) => run("formatBlock", tag) || run("formatBlock", `<${tag}>`);
  const block = () => {
    const node = window.getSelection()?.anchorNode;
    const el = node?.nodeType === Node.ELEMENT_NODE ? node : node?.parentElement;
    return el;
  };
  const applyVisual = (format, extra) => {
    restore();
    article()?.focus();
    restore();
    try {
      document.execCommand("styleWithCSS", false, false);
    } catch {
      /* some browsers refuse the command outside a focused editing host */
    }
    if (format === "bold") run("bold");
    else if (format === "italic") run("italic");
    else     if (format === "heading") formatBlock(block()?.closest("h1, h2, h3") ? "p" : "h2");
    else if (format === "list") run("insertUnorderedList");
    else if (format === "quote") formatBlock(block()?.closest("blockquote") ? "p" : "blockquote");
    else if (format === "code") toggleCode();
    else if (format === "link") {
      if (block()?.closest("a")) run("unlink");
      else if (extra?.url) run("createLink", extra.url);
    } else if (format === "image" && extra?.name) {
      const alt = (extra.alt || extra.name).replace(/"/g, "");
      run("insertHTML", `<img class="md-image" data-asset="${extra.name}" alt="${alt}" src="${extra.assets[extra.name]}">`);
    }
    saveArticle(extra?.assets);
  };
  const toggleCode = () => {
    const sel = window.getSelection();
    if (!sel?.rangeCount) return;
    const existing = block()?.closest("code, pre");
    if (existing) {
      existing.replaceWith(document.createTextNode(existing.textContent));
      return;
    }
    const text = sel.toString();
    if (text.includes("\n")) return formatBlock("pre");
    const code = document.createElement("code");
    code.textContent = text;
    const span = sel.getRangeAt(0);
    span.deleteContents();
    span.insertNode(code);
  };
  const apply = (format, extra) => (sourceMode() ? applySource(format, extra) : applyVisual(format, extra));
  /** A new diagram goes after the block holding the caret, or at the end of the doc. */
  const newDiagram = () => {
    if (sourceMode()) {
      const caret = document.activeElement === source ? source.selectionEnd : range[1];
      return openDiagramEditor({ mode: "new", at: markdown(source.value.slice(0, caret), { diagrams: true }).length });
    }
    const page = article();
    const children = page ? [...page.children] : [];
    let node = saved?.startContainer ?? null;
    while (node && node.parentNode !== page) node = node.parentNode;
    const at = node ? children.indexOf(node) + 1 : children.length;
    const empty = children.length === 1 && children[0].hasAttribute?.("data-placeholder");
    openDiagramEditor({ mode: "new", at: empty ? 0 : at });
  };
  const askLink = () => {
    const url = window.prompt("Link address", "https://");
    if (url?.trim()) apply("link", { url: url.trim() });
    else (sourceMode() ? source : article())?.focus();
  };
  const file = h("input", { type: "file", accept: "image/*", hidden: true });
  file.addEventListener("change", () => {
    const picked = file.files?.[0];
    file.value = "";
    if (!picked) return;
    const reader = new FileReader();
    reader.onload = () => {
      const doc = store.docs.find((d) => d.id === store.ui.docId);
      const name = assetName(picked, doc?.assets ?? {});
      const alt = picked.name.replace(/\.[^.]+$/, "").replace(/[-_]+/g, " ");
      apply("image", { name, alt, assets: { [name]: reader.result } });
    };
    reader.readAsDataURL(picked);
  });
  preview.addEventListener("input", () => {
    if (!sourceMode() && !store.docs.find((d) => d.id === store.ui.docId)?.agentEditing) saveArticle();
  });
  preview.addEventListener("paste", (e) => {
    if (sourceMode()) return;
    const text = e.clipboardData?.getData("text/plain");
    if (text == null) return;
    e.preventDefault();
    run("insertText", text);
  });
  const editDiagram = (figure) => {
    const index = [...(article()?.querySelectorAll(".md-mermaid") ?? [])].indexOf(figure);
    if (index !== -1) openDiagramEditor({ mode: "edit", index });
  };
  preview.addEventListener("click", (e) => {
    if (e.target.closest?.("a")) e.preventDefault();
    const edit = e.target.closest?.(".md-mermaid__edit");
    if (edit) editDiagram(edit.closest(".md-mermaid"));
  });
  preview.addEventListener("dblclick", (e) => {
    const figure = e.target.closest?.(".md-mermaid");
    if (figure) editDiagram(figure);
  });
  const button = (format, title, glyph) =>
    h(
      "button",
      {
        class: ["doc-panel__format-btn", glyph.length === 1 && `doc-panel__format-btn--${format}`],
        title,
        "aria-label": title,
        onMousedown: remember,
        onClick: () => (format === "link" ? askLink() : format === "image" ? file.click() : format === "diagram" ? newDiagram() : apply(format)),
      },
      glyph.length === 1 ? glyph : icon(glyph, 14),
    );
  const bar = h(
    "div",
    { class: "doc-panel__format", hidden: true },
    FORMAT_BUTTONS.slice(0, 3).map(([id, title, glyph]) => button(id, title, glyph)),
    h("span", { class: "doc-panel__format-sep" }),
    FORMAT_BUTTONS.slice(3, 6).map(([id, title, glyph]) => button(id, title, glyph)),
    h("span", { class: "doc-panel__format-sep" }),
    FORMAT_BUTTONS.slice(6).map(([id, title, glyph]) => button(id, title, glyph)),
    file,
  );
  return {
    bar,
    onKey(e) {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      const key = e.key.toLowerCase();
      const id = key === "b" ? "bold" : key === "i" ? "italic" : key === "k" ? "link" : null;
      if (!id) return;
      e.preventDefault();
      if (sourceMode()) range = [source.selectionStart, source.selectionEnd];
      else rememberSelection();
      if (id === "link") askLink();
      else apply(id);
    },
  };
}

const TEMPLATE_LABELS = { flowchart: "Flowchart", sequence: "Sequence", pie: "Pie", timeline: "Timeline", usecase: "Use case" };
const LIVE_AFTER_MS = 250;
const ZOOM_STEPS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
const ZOOM_MIN = 0.1;
const ZOOM_MAX = 8;
const FIT_MAX = 3;
const SPLIT_MIN = 15;
const SPLIT_MAX = 85;
// Kept between openings: a person who widened the code wants it wide next time.
let diagramSplit = 50;

const clamp = (value, min, max) => Math.min(max, Math.max(min, value));

/** Follows the pointer until it is released, wherever it goes. */
function drag(onMove, onEnd) {
  const up = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", up);
    onEnd();
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", up);
}

/** The drawing's zoom: "fit" fills the pane; a number is the scale of the drawing's own size. */
function diagramZoom(figure, preview) {
  let zoom = "fit";
  const level = h("button", { class: "doc-panel__zoom-level", type: "button", title: "Actual size" }, "–");
  const drawing = () => {
    const svg = figure.querySelector(".md-mermaid__canvas svg");
    const box = svg?.viewBox?.baseVal;
    return svg && box?.width ? { svg, w: box.width, h: box.height } : null;
  };
  const scale = () => {
    const d = drawing();
    if (!d) return 1;
    if (zoom !== "fit") return zoom;
    const pad = 24;
    return clamp(Math.min((preview.clientWidth - pad) / d.w, (preview.clientHeight - pad) / d.h), ZOOM_MIN, FIT_MAX);
  };
  const apply = () => {
    const d = drawing();
    preview.classList.toggle("is-zoomed", zoom !== "fit");
    if (!d) {
      level.textContent = "–";
      return;
    }
    const s = scale();
    Object.assign(d.svg.style, { maxWidth: "none", maxHeight: "none", width: `${d.w * s}px`, height: `${d.h * s}px` });
    level.textContent = `${Math.round(s * 100)}%`;
  };
  // Keeps the point at the middle of the pane where it was.
  const set = (next) => {
    const cx = (preview.scrollLeft + preview.clientWidth / 2) / Math.max(preview.scrollWidth, 1);
    const cy = (preview.scrollTop + preview.clientHeight / 2) / Math.max(preview.scrollHeight, 1);
    zoom = next === "fit" ? "fit" : clamp(next, ZOOM_MIN, ZOOM_MAX);
    apply();
    preview.scrollLeft = cx * preview.scrollWidth - preview.clientWidth / 2;
    preview.scrollTop = cy * preview.scrollHeight - preview.clientHeight / 2;
  };
  const step = (dir) => {
    const s = scale();
    const next = dir > 0 ? ZOOM_STEPS.find((z) => z > s + 0.001) : [...ZOOM_STEPS].reverse().find((z) => z < s - 0.001);
    set(next ?? (dir > 0 ? ZOOM_MAX : ZOOM_MIN));
  };
  level.addEventListener("click", () => set(1));

  preview.addEventListener(
    "wheel",
    (e) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      set(scale() * Math.exp(-e.deltaY * 0.002));
    },
    { passive: false },
  );
  preview.addEventListener("dblclick", () => set("fit"));
  // Drag to move around a drawing larger than the pane.
  preview.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || zoom === "fit") return;
    e.preventDefault();
    const start = { x: e.clientX, y: e.clientY, left: preview.scrollLeft, top: preview.scrollTop };
    preview.classList.add("is-panning");
    drag(
      (ev) => {
        preview.scrollLeft = start.left - (ev.clientX - start.x);
        preview.scrollTop = start.top - (ev.clientY - start.y);
      },
      () => preview.classList.remove("is-panning"),
    );
  });
  new ResizeObserver(() => zoom === "fit" && apply()).observe(preview);

  const button = (label, title, onClick) => h("button", { class: "doc-panel__zoom-btn", type: "button", title, "aria-label": title, onClick }, label);
  const controls = h(
    "span",
    { class: "doc-panel__zoom" },
    button("−", "Zoom out (Ctrl+−)", () => step(-1)),
    level,
    button("+", "Zoom in (Ctrl+=)", () => step(1)),
    h("button", { class: "doc-panel__zoom-btn doc-panel__zoom-fit", type: "button", title: "Fit to the pane (Ctrl+0)", onClick: () => set("fit") }, "fit"),
  );
  return { controls, apply, step, fit: () => set("fit") };
}

/** The bar between the preview and the code: drag it, or arrows on it; double-click puts it back in the middle. */
function diagramSplitter(body) {
  const splitter = h("div", {
    class: "doc-panel__diagram-split",
    role: "separator",
    tabindex: "0",
    "aria-label": "Resize the preview and the code",
    title: "Drag to resize · double-click to reset",
  });
  const set = (pct) => {
    diagramSplit = clamp(pct, SPLIT_MIN, SPLIT_MAX);
    body.style.setProperty("--diagram-split", `${diagramSplit}%`);
    splitter.setAttribute("aria-valuenow", String(Math.round(diagramSplit)));
  };
  // Stacked panes (narrow panel) turn the bar horizontal; the stylesheet says which through the cursor.
  const stacked = () => getComputedStyle(splitter).cursor === "row-resize";
  splitter.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    body.classList.add("is-resizing");
    drag(
      (ev) => {
        const rect = body.getBoundingClientRect();
        set(stacked() ? ((ev.clientY - rect.top) / rect.height) * 100 : ((ev.clientX - rect.left) / rect.width) * 100);
      },
      () => body.classList.remove("is-resizing"),
    );
  });
  splitter.addEventListener("dblclick", () => set(50));
  splitter.addEventListener("keydown", (e) => {
    const delta = { ArrowLeft: -5, ArrowUp: -5, ArrowRight: 5, ArrowDown: 5 }[e.key];
    if (!delta) return;
    e.preventDefault();
    e.stopPropagation();
    set(diagramSplit + delta);
  });
  set(diagramSplit);
  return splitter;
}

/**
 * The diagram editor, in place of the doc: the drawing on the left, its code on
 * the right, redrawn as the person types. Save puts it in the doc; Esc leaves it as it was.
 */
function DiagramEditor(spec, initial) {
  const figure = h("figure", { class: "md-mermaid md-mermaid--live", "data-source": initial }, h("div", { class: "md-mermaid__canvas" }, "drawing diagram…"));
  const preview = h("div", { class: "doc-panel__diagram-preview" }, figure);
  const code = h("textarea", { class: "doc-panel__diagram-code", spellcheck: "false", "aria-label": "Diagram code" });
  code.value = initial;
  const zoom = diagramZoom(figure, preview);
  let timer;
  const redraw = () => {
    figure.dataset.source = code.value;
    paintMermaid(preview).then(zoom.apply);
  };
  const setCode = (text) => {
    code.value = text;
    redraw();
    code.focus();
  };
  code.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(redraw, LIVE_AFTER_MS);
  });
  code.addEventListener("keydown", (e) => {
    e.stopPropagation();
    const mod = e.metaKey || e.ctrlKey;
    if (e.key === "Escape") return closeDiagramEditor();
    if (mod && (e.key === "Enter" || e.key.toLowerCase() === "s")) {
      e.preventDefault();
      return saveDiagram(code.value);
    }
    if (mod && (e.key === "=" || e.key === "+" || e.key === "-" || e.key === "0")) {
      e.preventDefault();
      return e.key === "0" ? zoom.fit() : zoom.step(e.key === "-" ? -1 : 1);
    }
    if (e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      document.execCommand("insertText", false, "  ");
    }
  });
  const isNew = spec.mode === "new";
  const body = h(
    "div",
    { class: "doc-panel__diagram-body" },
    h(
      "div",
      { class: "doc-panel__diagram-pane" },
      h("div", { class: "doc-panel__diagram-bar" }, h("span", { class: "doc-panel__diagram-label" }, "preview"), zoom.controls),
      preview,
    ),
  );
  body.append(
    diagramSplitter(body),
    h("div", { class: "doc-panel__diagram-pane" }, h("div", { class: "doc-panel__diagram-bar" }, h("span", { class: "doc-panel__diagram-label" }, "mermaid code")), code),
  );
  const el = h(
    "div",
    { class: "doc-panel__diagram" },
    h(
      "div",
      { class: "doc-panel__diagram-head" },
      icon("diagram", 14),
      h("strong", { class: "doc-panel__diagram-title" }, isNew ? "New diagram" : "Edit diagram"),
      h(
        "span",
        { class: "doc-panel__diagram-templates" },
        h("span", { class: "doc-panel__diagram-hint" }, "start from"),
        Object.entries(DIAGRAM_TEMPLATES).map(([id, text]) =>
          h("button", { class: "doc-panel__chip", type: "button", onClick: () => setCode(text) }, TEMPLATE_LABELS[id] ?? id),
        ),
      ),
      h("span", { class: "doc-panel__spacer" }),
      h("button", { class: "doc-panel__toggle", type: "button", title: "Leave without saving (Esc)", onClick: closeDiagramEditor }, "cancel"),
      h(
        "button",
        { class: "doc-panel__button doc-panel__button--solid", type: "button", title: "Save to the doc (Ctrl+Enter)", onClick: () => saveDiagram(code.value) },
        isNew ? "Add to doc" : "Save",
      ),
    ),
    body,
  );
  requestAnimationFrame(() => {
    redraw();
    code.focus();
  });
  return el;
}

function Empty() {
  return h(
    "div",
    { class: "doc-panel__empty" },
    icon("doc", 28),
    h("strong", {}, "No doc in this chat yet"),
    h("p", {}, "Select text in the chat and choose Add to doc, or open ▾ on a block's copy button. You can also start one empty and write it yourself, or ask the agent to write it."),
    h("button", { class: "doc-panel__button", onClick: () => newDoc() }, icon("plus", 14), " New doc"),
  );
}

export function mountDocPanel(root) {
  const resizer = h("div", { class: "doc-panel__resizer", title: "Drag to resize" });
  const head = h("div", { class: "doc-panel__head" });
  const preview = h("div", { class: "doc-panel__preview" });
  const source = h("textarea", { class: "doc-panel__source", spellcheck: "false", placeholder: "# Title\n\nWrite markdown here." });
  const format = FormatBar(source, preview);
  const peek = h("div");
  const diagram = h("div", { class: "doc-panel__diagram-slot", hidden: true });
  root.classList.add("doc-panel");
  root.append(resizer, head, format.bar, preview, source, diagram, peek);
  let diagramKey = null;

  let renaming = false;
  let lastHead = null;
  let lastBody = null;
  let lastJustAdded = 0;
  let current = null;

  source.addEventListener("input", () => editDocMarkdown(source.value));
  source.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") return setDocEditing(false);
    format.onKey(e);
  });
  preview.addEventListener("keydown", (e) => {
    e.stopPropagation();
    format.onKey(e);
  });
  preview.addEventListener("mousedown", (e) => {
    if (e.target === preview) preview.querySelector(".doc-panel__page")?.focus();
  });

  resizer.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const panes = root.parentElement.getBoundingClientRect();
    const move = (m) => root.style.setProperty("flex-basis", `${((panes.right - m.clientX) / panes.width) * 100}%`);
    const up = (m) => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      root.style.removeProperty("flex-basis");
      setDocSplit((panes.right - m.clientX) / panes.width);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });

  function titleField(doc) {
    const input = h("input", {
      class: "doc-panel__title-input",
      value: doc.title,
      onKeydown: (e) => {
        e.stopPropagation();
        if (e.key === "Enter") input.blur();
        if (e.key === "Escape") {
          input.value = doc.title;
          input.blur();
        }
      },
      onBlur: () => {
        renaming = false;
        lastHead = null;
        if (input.value.trim() && input.value !== doc.title) renameDoc(doc.id, input.value);
        else renderHead(store);
      },
    });
    requestAnimationFrame(() => input.select());
    return input;
  }

  function renderHead(state) {
    const doc = current;
    const view = state.ui.docView;
    const menu = state.ui.docMenu;
    const place = doc && placeOf(doc, state.organization);
    head.replaceChildren(
      h(
        "div",
        { class: "doc-panel__lead" },
        icon("doc", 14),
        doc
          ? [
              renaming
                ? titleField(doc)
                : h("button", { class: "doc-panel__title", title: "Rename", onClick: () => ((renaming = true), renderHead(state)) }, doc.title),
              h(
                "span",
                { class: "doc-panel__anchor" },
                iconButton("chevron", "Docs of this chat", () => toggleDocMenu("switch"), { on: menu === "switch" }),
                menu === "switch" && SwitchMenu(state, doc),
              ),
              h("span", { class: "doc-panel__chat", title: "The chat this doc belongs to" }, "· ", state.chat?.title ?? ""),
            ]
          : h("span", { class: "doc-panel__title doc-panel__title--empty" }, "Docs"),
      ),
      h(
        "div",
        { class: "doc-panel__tools" },
        doc && h("span", { class: ["doc-panel__state", doc.agentEditing && "doc-panel__state--agent"] }, doc.agentEditing ? "✦ agent writing…" : "saved"),
        doc &&
          h(
            "button",
            { class: "doc-panel__toggle", title: state.ui.docEditing ? "Back to the document (Esc)" : "Show the markdown source", onClick: () => setDocEditing(!state.ui.docEditing) },
            icon(state.ui.docEditing ? "eye" : "code", 14),
            h("span", { class: "doc-panel__label" }, state.ui.docEditing ? "preview" : "source"),
          ),
        doc &&
          h(
            "span",
            { class: "doc-panel__anchor" },
            h("button", { class: ["doc-panel__toggle", menu === "export" && "doc-panel__toggle--on"], title: "Export", onClick: () => toggleDocMenu("export") }, icon("export", 14), h("span", { class: "doc-panel__label" }, "export")),
            menu === "export" && ExportMenu(),
          ),
        h("span", { class: "doc-panel__divider" }),
        iconButton("split", "Chat and doc side by side (<C-g>d)", () => setDocView("split"), { on: view === "split" }),
        iconButton("full", "Only the doc; the prompt stays (<C-g>d)", () => setDocView("doc"), { on: view === "doc" }),
        iconButton("close", "Hide the doc panel (<C-g>d)", () => setDocView("hidden")),
      ),
      place && h("span", { class: "doc-panel__path", title: place.file }, place.file),
    );
  }

  function renderBody(state) {
    const doc = current;
    const spec = doc && state.ui.diagramEdit;
    diagram.hidden = !spec;
    if (spec) {
      preview.hidden = true;
      source.hidden = true;
      format.bar.hidden = true;
      const key = JSON.stringify([doc.id, spec]);
      if (key !== diagramKey) {
        diagramKey = key;
        diagram.replaceChildren(DiagramEditor(spec, diagramSource(spec)));
      }
      return;
    }
    diagramKey = null;
    diagram.replaceChildren();
    const editing = !!doc && state.ui.docEditing;
    preview.hidden = editing;
    source.hidden = !editing;
    format.bar.hidden = !doc;
    if (!doc) return preview.replaceChildren(Empty());
    if (editing) {
      if (document.activeElement !== source || doc.agentEditing) source.value = doc.markdown;
      if (document.activeElement !== source) requestAnimationFrame(() => source.focus());
      return;
    }
    const article = preview.querySelector(".doc-panel__page");
    if (article?.dataset.id === doc.id && article.contains(document.activeElement) && !doc.agentEditing) return;
    const page = h(
      "article",
      { class: "doc-panel__page", contenteditable: "true", spellcheck: "true", role: "textbox", "aria-multiline": "true", "aria-label": doc.title, "data-id": doc.id },
      doc.markdown.trim() ? markdown(doc.markdown, { assets: doc.assets, diagrams: true }) : h("p", { "data-placeholder": "Start writing…" }, h("br")),
    );
    preview.replaceChildren(page);
    if (doc.justAdded && doc.justAdded !== lastJustAdded) {
      lastJustAdded = doc.justAdded;
      const added = page.lastElementChild;
      added?.classList.add("doc-panel__added");
      requestAnimationFrame(() => added?.scrollIntoView({ block: "nearest", behavior: "smooth" }));
    }
  }

  return {
    update(state) {
      current = state.docs.find((d) => d.id === state.ui.docId && d.chatId === state.chat?.id) ?? null;
      root.classList.toggle("doc-panel--doc", state.ui.docView === "doc");
      root.style.setProperty("--doc-split", state.ui.docSplit);

      const headKey = JSON.stringify([
        current && [current.id, current.title, !!current.agentEditing, placeOf(current, state.organization).file],
        state.ui.docMenu,
        state.ui.docEditing,
        state.ui.docView,
        state.chat?.title,
        docsOfChat(state.docs, state.chat?.id).map((d) => [d.id, d.title]),
      ]);
      if (headKey !== lastHead && !renaming) {
        lastHead = headKey;
        renderHead(state);
      }
      const bodyKey = JSON.stringify([current?.id, current?.markdown, current && Object.keys(current.assets ?? {}), current?.justAdded, state.ui.docEditing, state.ui.diagramEdit]);
      if (bodyKey !== lastBody) {
        lastBody = bodyKey;
        renderBody(state);
      }
      paintMermaid(preview);
      peek.replaceChildren((state.ui.docView === "doc" && Peek(state)) || "");
    },
  };
}
