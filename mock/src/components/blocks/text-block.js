// The assistant's reply, as markdown. A chip under it counts the numbers the
// model typed that no engine computed (crates/ui/src/timeline/numbers.rs).

import { h } from "../../lib/h.js";
import { markdown } from "../../lib/markdown.js";
import "./text-block.scss";

export function TextBlock(block) {
  const n = block.unverifiedNumbers;
  return h(
    "div",
    { class: "text-block" },
    markdown(block.markdown),
    n > 0 && h("span", { class: "numbers-chip" }, `⚠ ${n} ${n === 1 ? "number" : "numbers"} not from an engine`),
  );
}
