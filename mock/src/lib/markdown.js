// A small markdown renderer: paragraphs, headings, lists, fenced code and
// inline `code`, **bold** and *italic*. Enough for the mock's replies.

import { h } from "./h.js";

const INLINE = /(`[^`]+`|\*\*[^*]+\*\*|\*[^*]+\*)/g;

export function inline(text) {
  return text.split(INLINE).map((part) => {
    if (part.startsWith("`") && part.endsWith("`") && part.length > 1) return h("code", part.slice(1, -1));
    if (part.startsWith("**") && part.endsWith("**")) return h("strong", part.slice(2, -2));
    if (part.startsWith("*") && part.endsWith("*") && part.length > 2) return h("em", part.slice(1, -1));
    return part;
  });
}

function list(lines, ordered) {
  const items = lines.map((line) => h("li", inline(line.replace(/^(\s*[-*]|\s*\d+\.)\s+/, ""))));
  return h(ordered ? "ol" : "ul", items);
}

export function markdown(source) {
  const out = [];
  const chunks = source.split(/\n{2,}/);
  for (let i = 0; i < chunks.length; i++) {
    const chunk = chunks[i];
    if (chunk.startsWith("```")) {
      // A fence may span blank lines: join chunks until it closes.
      let fence = chunk;
      while (!/\n```\s*$/.test(fence) && i + 1 < chunks.length) fence += `\n\n${chunks[++i]}`;
      const body = fence.replace(/^```\w*\n?/, "").replace(/\n?```\s*$/, "");
      out.push(h("pre", { class: "md-code" }, h("code", body)));
      continue;
    }
    const lines = chunk.split("\n");
    if (lines.every((l) => /^\s*[-*]\s+/.test(l))) out.push(list(lines, false));
    else if (lines.every((l) => /^\s*\d+\.\s+/.test(l))) out.push(list(lines, true));
    else if (/^#{1,3}\s/.test(chunk)) {
      const level = chunk.match(/^#+/)[0].length;
      out.push(h(`h${level}`, inline(chunk.replace(/^#+\s/, ""))));
    } else out.push(h("p", inline(chunk)));
  }
  return out;
}
