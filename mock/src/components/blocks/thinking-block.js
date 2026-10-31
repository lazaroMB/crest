// The model's thinking, collapsed to its first line (crates/ui/src/blocks/thinking.rs).

import { h } from "../../lib/h.js";
import "./thinking-block.scss";

export function ThinkingBlock(block, view) {
  return h(
    "div",
    { class: "thinking-block" },
    h(
      "div",
      { class: "thinking-block__head", onClick: view.toggle },
      h("span", { class: "thinking-block__marker" }, view.expanded ? "▾" : "▸"),
      h("em", { class: "thinking-block__label" }, "Thinking…"),
      !view.expanded && h("span", { class: "thinking-block__summary" }, block.summary.slice(0, 160)),
    ),
    view.expanded && h("div", { class: "thinking-block__body" }, block.text),
  );
}
