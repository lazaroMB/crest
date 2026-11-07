// The file tree (crates/ui/src/tree.rs): hidden by default, Ctrl+G e shows it.

import { h } from "../../lib/h.js";
import "./tree-panel.scss";

const FILES = [
  ["crates", 0, true],
  ["app", 1, true],
  ["domain", 1, true],
  ["ui", 1, true],
  ["src", 2, true],
  ["app.rs", 3],
  ["session.rs", 3],
  ["status.rs", 3],
  ["docs", 0, true],
  ["src", 0, true],
  ["math.rs", 1],
  ["tests", 0, true],
  ["math.rs", 1],
  ["Cargo.toml", 0],
  ["README.md", 0],
];

export function TreePanel(state) {
  return h(
    "aside",
    { class: ["tree", state.ui.focus === "tree" && "tree--focused"] },
    h("div", { class: "tree__header" }, state.status?.workspace ?? "workspace"),
    h(
      "div",
      { class: "tree__list" },
      FILES.map(([name, depth, folder]) =>
        h(
          "div",
          { class: "tree__row", style: { paddingLeft: `${8 + 12 * depth}px` } },
          h("span", { class: "tree__marker" }, folder ? "▾" : " "),
          name,
        ),
      ),
    ),
  );
}
