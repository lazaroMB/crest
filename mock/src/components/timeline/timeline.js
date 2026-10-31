// The timeline: every block of the chat in a centered column (crates/ui/src/timeline/).
//
// It keeps its scroll container between renders, so it is mounted once
// (mountTimeline) and then updated: it follows new blocks while you are at
// the bottom, and keeps your place when you scrolled up.

import { h } from "../../lib/h.js";
import { select, toggleAltView, toggleExpanded } from "../../actions.js";
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
      scroll.querySelector(`[data-id="${state.ui.selected}"]`)?.scrollIntoView({ block: "nearest" });
    }
  }

  return { update };
}
