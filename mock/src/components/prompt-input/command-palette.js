// The / command palette above the prompt (crates/ui/src/input.rs): up to 8
// commands that start with what you typed.

import { h } from "../../lib/h.js";
import "./command-palette.scss";

export const PALETTE_ROWS = 8;

export function matchCommands(commands, text) {
  if (!text.startsWith("/") || text.includes("\n")) return [];
  const typed = text.trim().toLowerCase();
  return commands.filter((c) => c.usage.toLowerCase().startsWith(typed) || typed.startsWith(c.usage.split(" ")[0]));
}

export function CommandPalette(matches, selected, onPick) {
  const first = Math.max(0, Math.min(selected - PALETTE_ROWS + 1, matches.length - PALETTE_ROWS));
  const rows = matches.slice(first, first + PALETTE_ROWS);
  const more = matches.length - PALETTE_ROWS;
  return h(
    "div",
    { class: "palette" },
    rows.map((command, i) =>
      h(
        "div",
        {
          class: ["palette__row", first + i === selected && "palette__row--selected"],
          onMousedown: (e) => (e.preventDefault(), onPick(first + i)),
        },
        h("span", { class: "palette__usage" }, command.usage),
        h("span", { class: "palette__description" }, command.description),
      ),
    ),
    h(
      "div",
      { class: "palette__footer" },
      "↑↓ choose · Tab complete · Enter run · Esc hide",
      more > 0 && ` · ${first + 1}–${first + rows.length} of ${matches.length} · ${more} more`,
    ),
  );
}
