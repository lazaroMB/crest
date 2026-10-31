// A workflow stage: "Stage 3/14 · Data" between two rules, over a progress bar
// (crates/ui/src/blocks/stage.rs).

import { h } from "../../lib/h.js";
import "./stage-block.scss";

export function StageBlock(block) {
  return h(
    "div",
    { class: "stage-block" },
    h(
      "div",
      { class: "stage-block__title" },
      h("span", { class: "stage-block__rule" }),
      h("strong", `Stage ${block.index}/${block.total} · ${block.name}`),
      h("span", { class: "stage-block__rule" }),
    ),
    h("div", { class: "stage-block__track" }, h("div", { class: "stage-block__fill", style: { width: `${(block.index / block.total) * 100}%` } })),
  );
}
