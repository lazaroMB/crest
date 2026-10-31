// One tool call: "▸ grep · "fn add" in src/ · done" (crates/ui/src/timeline/render.rs).
// A click shows its input and output.

import { h } from "../../lib/h.js";
import "./tool-block.scss";

const STATUS_CLASS = {
  pending: "muted",
  cancelled: "muted",
  "awaiting approval": "warning",
  running: "info",
  done: "success",
  failed: "error",
  denied: "error",
};

export function ToolBlock(block, view) {
  const hasDetail = block.input || block.output;
  return h(
    "div",
    { class: "tool-block" },
    h(
      "div",
      { class: "tool-block__head", onClick: hasDetail ? view.toggle : null },
      h("span", { class: "tool-block__marker" }, view.expanded ? "▾" : "▸"),
      h("strong", { class: "tool-block__name" }, block.name),
      h("span", { class: "sep" }, "·"),
      h("span", { class: "tool-block__summary" }, block.summary),
      h("span", { class: "sep" }, "·"),
      h("span", { class: `tool-block__status tool-block__status--${STATUS_CLASS[block.status]}` }, block.status),
    ),
    view.expanded &&
      hasDetail &&
      h(
        "div",
        { class: "tool-block__detail" },
        block.input && h("pre", { class: "tool-block__input" }, block.input),
        block.output && h("pre", { class: "tool-block__output" }, block.output),
      ),
  );
}
