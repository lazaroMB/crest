// The / command palette above the prompt (crates/ui/src/input.rs), grouped
// (src/commands.js): "/" alone lists the groups, a group lists its commands,
// and typing more filters every command, each row labeled with its group.

import { h } from "../../lib/h.js";
import { invocation } from "../../commands.js";
import "./command-palette.scss";

export const PALETTE_ROWS = 8;

const FOOTER = {
  groups: "↑↓ choose · Enter or → open a group · type to filter · Esc hide",
  group: "↑↓ choose · Tab complete · Enter run · ← all groups · Esc hide",
  commands: "↑↓ choose · Tab complete · Enter run · Esc hide",
};

function GroupRow(group) {
  const names = [...new Set(group.commands.map((c) => invocation(c.usage)))];
  return [
    h("span", { class: "palette__group" }, group.label),
    h("span", { class: "palette__description" }, names.join(" ")),
    h("span", { class: "palette__count" }, String(group.commands.length)),
  ];
}

function CommandRow(command, labeled) {
  return [
    h("span", { class: "palette__usage" }, command.usage),
    h("span", { class: "palette__description" }, command.description),
    labeled && command.group && h("span", { class: "palette__tag" }, command.group.label),
  ];
}

/** `view` is what src/commands.js `palette()` returned. */
export function CommandPalette(view, selected, onPick) {
  const rows = view.rows;
  const first = Math.max(0, Math.min(selected - PALETTE_ROWS + 1, rows.length - PALETTE_ROWS));
  const shown = rows.slice(first, first + PALETTE_ROWS);
  const more = rows.length - PALETTE_ROWS;
  const footer = view.kind === "groups" ? FOOTER.groups : view.group ? FOOTER.group : FOOTER.commands;
  return h(
    "div",
    { class: "palette", "data-kind": view.group ? "group" : view.kind },
    view.group && h("div", { class: "palette__title" }, "← ", view.group.label),
    shown.map((row, i) =>
      h(
        "div",
        {
          class: ["palette__row", first + i === selected && "palette__row--selected"],
          onMousedown: (e) => (e.preventDefault(), onPick(first + i)),
        },
        view.kind === "groups" ? GroupRow(row) : CommandRow(row, !view.group),
      ),
    ),
    h("div", { class: "palette__footer" }, footer, more > 0 && ` · ${first + 1}–${first + shown.length} of ${rows.length}`),
  );
}
