// What the user asked. crest stores it as a markdown quote ("> …").

import { h } from "../../lib/h.js";
import "./user-block.scss";

export function UserBlock(block) {
  return h("div", { class: "user-block" }, block.text);
}
