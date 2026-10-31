// A unified diff: "src/math.rs +1 -1" and its colored lines (crates/ui/src/blocks/diff.rs).

import { h } from "../../lib/h.js";
import "./diff-block.scss";

export function DiffLines(lines) {
  return h(
    "div",
    { class: "diff-lines" },
    lines.map((line) => h("div", { class: `diff-line diff-line--${line.kind}` }, line.text || " ")),
  );
}

export function DiffBlock(block) {
  return h(
    "div",
    { class: "diff-block" },
    h(
      "div",
      { class: "diff-block__head" },
      h("strong", block.path),
      h("span", { class: "diff-block__added" }, `+${block.added}`),
      h("span", { class: "diff-block__removed" }, `-${block.removed}`),
      block.newFile && h("span", { class: "muted" }, "new file"),
    ),
    block.lines.length ? DiffLines(block.lines) : h("div", { class: "muted" }, "(no changes)"),
  );
}
