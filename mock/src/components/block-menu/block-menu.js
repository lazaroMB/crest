// A block's copy button, split in two: "⧉ copy" does the block's usual copy
// (a chart as an image, the rest as markdown), "▾" opens the other ways out
// of the chat, "Add to doc" among them.
//
//   ⧉ copy image │ ▾      Copy image
//                         Copy as markdown table
//                         Copy as CSV
//                         ─────────────
//                         Add to "v0.6.0 release notes"
//                         Add to a new doc

import { h } from "../../lib/h.js";
import { addBlockToDoc, copyBlock, toggleBlockMenu } from "../../doc-actions.js";
import { docsOfChat } from "../../docs.js";
import { icon } from "../icons.js";
import "./block-menu.scss";

const COPIES = {
  chart: [
    ["image", "image", "Copy image"],
    ["copy", "markdown", "Copy as markdown table"],
    ["copy", "csv", "Copy as CSV"],
  ],
  table: [
    ["copy", "markdown", "Copy as markdown"],
    ["copy", "csv", "Copy as CSV"],
  ],
};
const DEFAULT_COPIES = [["copy", "markdown", "Copy as markdown"]];

const item = (iconName, label, onClick) =>
  h("button", { class: "popmenu__row", onMousedown: (e) => e.preventDefault(), onClick: (e) => (e.stopPropagation(), onClick()) }, icon(iconName, 14), h("span", {}, label));

/** The doc "Add to doc" writes to: the one in the panel when it is this chat's, else the chat's latest. */
function targetDoc(state) {
  const mine = docsOfChat(state.docs, state.chat?.id);
  return mine.find((d) => d.id === state.ui.docId) ?? mine[0] ?? null;
}

export function BlockMenu(block, state, { overlay = false } = {}) {
  const open = state.ui.blockMenu === block.id;
  const copies = COPIES[block.kind] ?? DEFAULT_COPIES;
  const [, primaryFormat] = copies[0];
  const target = targetDoc(state);
  return h(
    "div",
    { class: ["block-menu", overlay && "block-menu--overlay", open && "block-menu--open"], onMousedown: (e) => e.stopPropagation() },
    h(
      "button",
      { class: "block-menu__main", title: copies[0][2], onClick: () => copyBlock(block.id, primaryFormat) },
      "⧉ ",
      block.kind === "chart" ? "copy image" : "copy",
    ),
    h("button", { class: "block-menu__more", title: "More: copy as…, add to doc", "aria-label": "More actions", onClick: () => toggleBlockMenu(block.id) }, icon("chevron", 12)),
    open &&
      h(
        "div",
        { class: "popmenu block-menu__menu", role: "menu" },
        copies.map(([iconName, format, label]) => item(iconName, label, () => copyBlock(block.id, format))),
        h("div", { class: "popmenu__sep" }),
        target
          ? [
              item("doc", `Add to "${target.title}"`, () => addBlockToDoc(block.id)),
              item("plus", "Add to a new doc", () => addBlockToDoc(block.id, { fresh: true })),
            ]
          : item("doc", "Add to doc", () => addBlockToDoc(block.id)),
      ),
  );
}
