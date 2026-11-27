// A small markdown renderer: paragraphs, headings, lists, quotes, tables,
// rules, fenced code, images and inline `code`, **bold** and *italic*. Enough
// for the mock's replies and its docs.
//
// `assets` resolves `![alt](asset:name)` to the doc's own images (src/docs.js).

import { h } from "./h.js";

const INLINE = /(!\[[^\]]*\]\([^)]+\)|`[^`]+`|\*\*[^*]+\*\*|\*[^*]+\*)/g;

export function inline(text, assets = {}) {
  return text.split(INLINE).map((part) => {
    const image = part.match(/^!\[([^\]]*)\]\(([^)]+)\)$/);
    if (image) {
      const [, alt, src] = image;
      const asset = src.startsWith("asset:") ? src.slice(6) : null;
      const url = asset ? assets[asset] : src;
      return url
        ? h("img", { class: "md-image", src: url, alt, "data-asset": asset || undefined })
        : h("span", { class: "md-missing" }, `[missing image: ${alt}]`);
    }
    if (part.startsWith("`") && part.endsWith("`") && part.length > 1) return h("code", part.slice(1, -1));
    if (part.startsWith("**") && part.endsWith("**")) return h("strong", part.slice(2, -2));
    if (part.startsWith("*") && part.endsWith("*") && part.length > 2) return h("em", part.slice(1, -1));
    return part;
  });
}

function list(lines, ordered, assets) {
  const items = lines.map((line) => h("li", inline(line.replace(/^(\s*[-*]|\s*\d+\.)\s+/, ""), assets)));
  return h(ordered ? "ol" : "ul", items);
}

const cells = (line) => line.trim().replace(/^\||\|$/g, "").split("|").map((c) => c.trim());

function table(lines, assets) {
  const [head, , ...body] = lines;
  const numeric = cells(lines[1]).map((c) => c.endsWith(":"));
  const cell = (tag, text, i) => h(tag, { class: numeric[i] && "md-num" }, inline(text, assets));
  return h(
    "table",
    { class: "md-table" },
    h("thead", h("tr", cells(head).map((c, i) => cell("th", c, i)))),
    h("tbody", body.map((row) => h("tr", cells(row).map((c, i) => cell("td", c, i))))),
  );
}

/** A fence's language and body. The language is the first word of the info string. */
function fenceOf(fence) {
  const open = fence.match(/^```([^\n]*)\n?/);
  const lang = (open?.[1] ?? "").trim().split(/\s+/)[0]?.toLowerCase() ?? "";
  const body = fence.slice(open ? open[0].length : 0).replace(/\n?```\s*$/, "");
  return { lang, body };
}

function codeBlock(lang, body) {
  return h("pre", { class: "md-code", "data-lang": lang || undefined }, h("code", body));
}

/** A diagram the editor draws. The source stays on the figure so saving writes the fence back. */
export function mermaidBlock(body) {
  return h(
    "figure",
    { class: "md-mermaid", contenteditable: "false", "data-source": body },
    h("div", { class: "md-mermaid__canvas" }, "drawing diagram…"),
    h("button", { class: "md-mermaid__edit", type: "button", title: "Edit the diagram", tabindex: "-1" }, "edit"),
  );
}

export function markdown(source, options = {}) {
  const { assets = {}, diagrams = false } = options;
  const out = [];
  const chunks = source.split(/\n{2,}/);
  for (let i = 0; i < chunks.length; i++) {
    const chunk = chunks[i];
    if (chunk.startsWith("```")) {
      // A fence may span blank lines: join chunks until it closes.
      let fence = chunk;
      while (!/\n```\s*$/.test(fence) && i + 1 < chunks.length) fence += `\n\n${chunks[++i]}`;
      const { lang, body } = fenceOf(fence);
      out.push(diagrams && lang === "mermaid" ? mermaidBlock(body) : codeBlock(lang, body));
      continue;
    }
    const lines = chunk.split("\n");
    if (!chunk.trim()) continue;
    if (/^(-{3,}|\*{3,})$/.test(chunk.trim())) out.push(h("hr", { class: "md-rule" }));
    else if (lines.every((l) => /^\s*[-*]\s+/.test(l))) out.push(list(lines, false, assets));
    else if (lines.every((l) => /^\s*\d+\.\s+/.test(l))) out.push(list(lines, true, assets));
    else if (lines.every((l) => l.startsWith(">"))) {
      out.push(h("blockquote", { class: "md-quote" }, markdown(lines.map((l) => l.replace(/^>\s?/, "")).join("\n"), options)));
    } else if (lines.length >= 2 && lines.every((l) => l.trim().startsWith("|")) && /^\s*\|?[\s:|-]+\|?\s*$/.test(lines[1])) {
      out.push(table(lines, assets));
    } else if (/^#{1,3}\s/.test(chunk)) {
      const level = chunk.match(/^#+/)[0].length;
      out.push(h(`h${level}`, inline(lines[0].replace(/^#+\s/, ""), assets)));
      if (lines.length > 1) out.push(...markdown(lines.slice(1).join("\n"), options));
    } else out.push(h("p", inline(chunk, assets)));
  }
  return out;
}

const blockTag = (node) => (node.nodeType === Node.ELEMENT_NODE ? node.tagName.toLowerCase() : "");

/** The marks around inline text, dropped when the text is empty so a toggle leaves nothing behind. */
const marked = (text, mark) => (text ? `${mark}${text}${mark}` : "");

/** The inline content of an element, as markdown. What contenteditable produces (`b`, `i`, `div`) counts too. */
function inlineMd(el) {
  let out = "";
  for (const node of el.childNodes) {
    if (node.nodeType === Node.TEXT_NODE) {
      out += node.textContent.replace(/\u00a0/g, " ");
      continue;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) continue;
    const tag = blockTag(node);
    const style = node.style;
    if (tag === "br") out += "\n";
    else if (tag === "strong" || tag === "b" || style?.fontWeight === "bold" || Number(style?.fontWeight) >= 600) out += marked(inlineMd(node), "**");
    else if (tag === "em" || tag === "i" || style?.fontStyle === "italic") out += marked(inlineMd(node), "*");
    else if (tag === "code") out += `\`${node.textContent}\``;
    else if (tag === "a") out += `[${inlineMd(node)}](${node.getAttribute("href") || ""})`;
    else if (tag === "img") out += imageMd(node);
    else out += inlineMd(node);
  }
  return out;
}

function imageMd(img) {
  const asset = img.getAttribute("data-asset");
  const src = asset ? `asset:${asset}` : img.getAttribute("src") || "";
  return `![${img.getAttribute("alt") || ""}](${src})`;
}

function listMd(el) {
  const ordered = blockTag(el) === "ol";
  return [...el.children]
    .filter((child) => blockTag(child) === "li")
    .map((li, i) => `${ordered ? `${i + 1}. ` : "- "}${inlineMd(li).trim()}`)
    .join("\n");
}

function tableMd(table) {
  const rows = [...table.querySelectorAll("tr")].map((tr) =>
    [...tr.children].map((cell) => inlineMd(cell).replace(/\|/g, "\\|").replace(/\n/g, " ").trim()),
  );
  if (!rows.length) return null;
  const width = Math.max(...rows.map((row) => row.length));
  const pad = (row) => [...row, ...Array(Math.max(0, width - row.length)).fill("")];
  const body = rows.slice(1);
  const numeric = pad(rows[0]).map((_, i) => body.length > 0 && body.every((_, r) => rows[r + 1] && table.querySelectorAll("tr")[r + 1].children[i]?.classList.contains("md-num")));
  const line = (cells) => `| ${pad(cells).join(" | ")} |`;
  const rule = numeric.map((n) => (n ? "---:" : "---"));
  return [line(rows[0]), `| ${rule.join(" | ")} |`, ...rows.slice(1).map(line)].join("\n");
}

function blockMd(node) {
  if (node.nodeType === Node.TEXT_NODE) return node.textContent.trim() || null;
  if (node.nodeType !== Node.ELEMENT_NODE) return null;
  const tag = blockTag(node);
  if (node.classList.contains("md-mermaid")) {
    const source = (node.dataset.source ?? "").replace(/\n$/, "");
    return `\`\`\`mermaid\n${source}\n\`\`\``;
  }
  // A wrapper the editor put around a diagram or a code block still has to keep it.
  if ((tag === "p" || tag === "div") && node.querySelector(".md-mermaid, pre, table, blockquote, ul, ol")) {
    const parts = [...node.childNodes].map(blockMd).filter(Boolean);
    return parts.length ? parts.join("\n\n") : null;
  }
  if (tag === "p" || tag === "div") return inlineMd(node).trim() || null;
  if (tag === "h1" || tag === "h2" || tag === "h3") return `${"#".repeat(Number(tag[1]))} ${inlineMd(node).trim()}`;
  if (tag === "ul" || tag === "ol") return listMd(node);
  if (tag === "pre") {
    const lang = node.dataset.lang || "";
    const code = (node.querySelector("code") ?? node).textContent.replace(/\n$/, "");
    return `\`\`\`${lang}\n${code}\n\`\`\``;
  }
  if (tag === "blockquote") {
    const inner = [...node.childNodes].map(blockMd).filter(Boolean).join("\n\n");
    return inner.trim() ? inner.split("\n").map((line) => `> ${line}`).join("\n") : null;
  }
  if (tag === "table") return tableMd(node);
  if (tag === "hr") return "---";
  if (tag === "img") return imageMd(node);
  const text = inlineMd(node).trim();
  return text || null;
}

/**
 * The markdown a rendered doc edits back to. `root` is the article the person
 * writes in; only the tags the renderer emits, plus the ones contenteditable
 * adds (`b`, `i`, `div`), are read.
 */
export function toMarkdown(root) {
  const blocks = [...root.childNodes].map(blockMd).filter(Boolean);
  return blocks.length ? `${blocks.join("\n\n")}\n` : "";
}
