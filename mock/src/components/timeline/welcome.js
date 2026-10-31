// The empty chat: the mark, "crest" and the keys to start with (crates/ui/src/timeline/view.rs).

import { h } from "../../lib/h.js";
import { Brand } from "../brand/brand.js";
import "./welcome.scss";

const KEYS = [
  ["/", "commands: /research, /implement, /!cmd, /help…"],
  ["<C-g>", "shortcuts: e tree · ff files · fg grep · m model · j/k blocks · h help"],
  ["<C-Tab>", "move the focus: prompt → timeline → tree"],
  ["<Esc>", "back to the prompt"],
  ["<C-c>", "clear the prompt or interrupt the turn"],
  ["<C-S-c>", "copy the selected text, from the prompt or the chat"],
];

export function Welcome() {
  return h(
    "div",
    { class: "welcome" },
    h("div", { class: "welcome__hero" }, Brand({ height: 48 }), h("span", { class: "welcome__word" }, "crest")),
    h(
      "div",
      { class: "welcome__keys" },
      KEYS.map(([keys, text]) => h("div", { class: "welcome__row" }, h("span", { class: "welcome__key" }, keys), h("span", { class: "muted" }, text))),
    ),
  );
}
