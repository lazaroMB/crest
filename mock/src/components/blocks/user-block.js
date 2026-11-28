// What the user asked. crest stores it as a markdown quote ("> …"). A prompt
// sent with a passage of the doc shows that passage above it.

import { h } from "../../lib/h.js";
import "./user-block.scss";

export function UserBlock(block) {
  return h(
    "div",
    { class: "user-block" },
    (block.context ?? []).map((c) =>
      c.kind === "doc"
        ? h("div", { class: "user-block__passage", title: c.title }, "↳ ", c.title || c.text)
        : h("div", { class: "user-block__passage", title: c.text }, "↳ doc: “", c.text.replace(/\s+/g, " "), "”"),
    ),
    block.text,
  );
}
