// A failed turn: the message, and a hint on what to do next (crates/ui/src/timeline/render.rs).

import { h } from "../../lib/h.js";
import "./error-block.scss";

export function ErrorBlock(block) {
  return h(
    "div",
    { class: "error-block" },
    h("div", { class: "error-block__message" }, block.message),
    block.hint && h("div", { class: "error-block__hint" }, `→ ${block.hint}`),
  );
}
