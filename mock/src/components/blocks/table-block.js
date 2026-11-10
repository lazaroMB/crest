// A table: "3 rows × 3 columns", a header row and striped rows (crates/ui/src/blocks/table.rs).
// Long tables show their first 12 rows until expanded.

import { h } from "../../lib/h.js";
import "./table-block.scss";

const COLLAPSED_ROWS = 12;
const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

export function Table({ columns, rows, numeric = [] }, { expanded = true, toggle } = {}) {
  const shown = expanded ? rows : rows.slice(0, COLLAPSED_ROWS);
  const cell = (tag, text, i) => h(tag, { class: numeric[i] && "table-block__num" }, text);
  return h(
    "div",
    { class: "table-block" },
    h("div", { class: "table-block__caption" }, `${plural(rows.length, "row")} × ${plural(columns.length, "column")}`),
    h(
      "table",
      { class: "table-block__table" },
      h("thead", h("tr", columns.map((c, i) => cell("th", c, i)))),
      h("tbody", shown.map((row) => h("tr", row.map((c, i) => cell("td", c, i))))),
    ),
    rows.length > shown.length &&
      h("button", { class: "table-block__more", onClick: toggle }, `${rows.length - shown.length} more rows · o expands`),
  );
}

export function TableBlock(block, view) {
  return Table(block, view.expanded || block.rows.length <= COLLAPSED_ROWS ? {} : { expanded: false, toggle: view.toggle });
}
