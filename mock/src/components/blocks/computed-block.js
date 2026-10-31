// An engine-computed card: the method, its provenance chip and verdict, why the
// method was chosen, and the values (crates/ui/src/blocks/computed/).

import { h } from "../../lib/h.js";
import "./computed-block.scss";

const VERDICT = {
  verified: ["success", "✓ Verified"],
  mismatch: ["error", "✗ Mismatch"],
  stale: ["warning", "Stale input"],
  recomputing: ["info", "Recomputing…"],
};

export function ComputedBlock(block) {
  const [tone, label] = VERDICT[block.verdict];
  return h(
    "div",
    { class: "computed" },
    h(
      "div",
      { class: "computed__head" },
      h("strong", block.method),
      h("button", { class: "computed__chip", title: "provenance" }, block.provenance),
      h("span", { class: `computed__pill computed__pill--${tone}` }, label),
    ),
    block.why && h("div", { class: "computed__why" }, block.why),
    h(
      "dl",
      { class: "computed__values" },
      block.values.map((v) => [h("dt", v.label), h("dd", v.value)]),
    ),
    h(
      "div",
      { class: "computed__foot" },
      h("button", { class: "computed__button" }, "Recompute"),
      h("span", { class: "muted" }, "reproduce in Python · R · Lua · shell"),
    ),
  );
}
