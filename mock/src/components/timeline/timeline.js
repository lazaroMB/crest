// The timeline: every block of the chat in a centered column (crates/ui/src/timeline/).
//
// It keeps its scroll container between renders, so it is mounted once
// (mountTimeline) and then updated: it follows new blocks while you are at
// the bottom, and keeps your place when you scrolled up.

import { h } from "../../lib/h.js";
import { select, syncSelected, toggleAltView, toggleExpanded } from "../../actions.js";
import { isWide, renderBlock } from "../blocks/index.js";
import { Outline } from "./outline.js";
import { Welcome } from "./welcome.js";
import "./timeline.scss";

function BlockRow(block, state) {
  const view = {
    expanded: !!state.ui.expanded[block.id],
    altView: !!state.ui.altView[block.id],
    toggle: () => toggleExpanded(block.id),
    toggleAlt: () => toggleAltView(block.id),
  };
  const selected = state.ui.selected === block.id;
  return h(
    "div",
    { class: "tblock", "data-id": block.id },
    h(
      "div",
      {
        class: ["tblock__inner", isWide(block) && "tblock__inner--wide", selected && "tblock__inner--selected"],
        onMousedown: () => select(block.id),
      },
      renderBlock(block, view),
    ),
  );
}

export function mountTimeline(root) {
  const scroll = h("div", { class: "timeline__scroll" });
  const overlay = h("div", { class: "timeline__overlay" });
  root.classList.add("timeline");
  root.append(scroll, overlay);
  let lastChatId = null;
  let syncScheduled = false;
  let suppressSync = false;

  // Scrollspy: the block just under the top edge becomes the selected one,
  // so the outline's active question follows the scroll, not only the keys.
  // Skipped once right after we scroll a block into view ourselves (a click
  // or a key), so that jump does not get overridden back to the last block.
  function syncToScroll() {
    if (suppressSync) {
      suppressSync = false;
      return;
    }
    const rows = scroll.querySelectorAll(".tblock");
    if (!rows.length) return;
    // At the bottom, the last block may be too short to reach the top edge
    // below; treat the bottom itself as "on" the last block, as the keys do.
    if (scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 2) {
      syncSelected(rows[rows.length - 1].dataset.id);
      return;
    }
    const edge = scroll.getBoundingClientRect().top + 24;
    let current = rows[0];
    for (const row of rows) {
      if (row.getBoundingClientRect().top <= edge) current = row;
      else break;
    }
    syncSelected(current.dataset.id);
  }
  scroll.addEventListener("scroll", () => {
    if (syncScheduled) return;
    syncScheduled = true;
    requestAnimationFrame(() => {
      syncScheduled = false;
      syncToScroll();
    });
  });

  function update(state) {
    const blocks = state.chat?.blocks ?? [];
    const switched = state.chat?.id !== lastChatId;
    const atBottom = scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 48;
    const top = scroll.scrollTop;

    root.classList.toggle("timeline--focused", state.ui.focus === "timeline");
    scroll.replaceChildren(blocks.length ? h("div", { class: "timeline__list" }, blocks.map((b) => BlockRow(b, state))) : Welcome());
    overlay.replaceChildren(Outline(state) ?? "");

    scroll.scrollTop = switched || atBottom ? scroll.scrollHeight : top;
    lastChatId = state.chat?.id;

    if (state.ui.selected) {
      const el = scroll.querySelector(`[data-id="${state.ui.selected}"]`);
      if (el) {
        suppressSync = true;
        el.scrollIntoView({ block: "nearest" });
      }
    }
  }

  return { update };
}
