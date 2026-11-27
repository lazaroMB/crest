// The floating bar over a text selection, above it (below when there is no
// room). What it offers depends on where the text is:
//
//   in the chat    ⧉ Copy · ＋ Add to doc
//   in the doc     ⧉ Copy · ✦ Ask agent   (the passage goes to the prompt as context)
//
// bindSelectionBar watches the mouse and Shift+arrows; the bar itself is a
// region of the window like any other, drawn from `state.ui.selectionBar`.

import { h } from "../../lib/h.js";
import { addSelectionToDoc, copySelection, hideSelectionBar, passSelectionToAgent, showSelectionBar } from "../../doc-actions.js";
import { state } from "../../store.js";
import { icon } from "../icons.js";
import "./selection-bar.scss";

const BAR_HEIGHT = 34;

const button = (iconName, label, onClick, primary) =>
  h(
    "button",
    { class: ["selection-bar__button", primary && "selection-bar__button--primary"], onMousedown: (e) => e.preventDefault(), onClick },
    icon(iconName, 14),
    h("span", {}, label),
  );

export function SelectionBar(s) {
  const bar = s.ui.selectionBar;
  if (!bar) return null;
  const below = bar.y < BAR_HEIGHT + 8;
  return h(
    "div",
    {
      class: ["selection-bar", below && "selection-bar--below"],
      style: { left: `${bar.x}px`, top: `${below ? bar.bottom : bar.y}px` },
      role: "toolbar",
      "aria-label": "Selection",
    },
    button("copy", "Copy", copySelection),
    bar.source === "chat"
      ? button("doc", "Add to doc", addSelectionToDoc, true)
      : button("agent", "Ask agent", passSelectionToAgent, true),
  );
}

/** Where the selection is: "chat", "doc" or null for anywhere else. */
function sourceOf(node) {
  const el = node?.nodeType === Node.ELEMENT_NODE ? node : node?.parentElement;
  if (el?.closest(".timeline__scroll .tblock")) return "chat";
  if (el?.closest(".doc-panel__preview")) return "doc";
  return null;
}

export function bindSelectionBar(win) {
  const relative = (x, y, bottom) => {
    const box = win.getBoundingClientRect();
    return { x: x - box.left, y: y - box.top, bottom: bottom - box.top };
  };

  function read(event) {
    const target = event?.target;
    if (target?.closest?.(".selection-bar")) return;
    if (target instanceof HTMLTextAreaElement && target.classList.contains("doc-panel__source")) {
      const text = target.value.slice(target.selectionStart, target.selectionEnd);
      if (!text.trim()) return hideSelectionBar();
      const box = target.getBoundingClientRect();
      const x = event.clientX ?? box.left + box.width / 2;
      const y = event.clientY ?? box.top + 24;
      return showSelectionBar({ source: "doc", text, ...relative(x, y - 12, y + 12) });
    }
    const selection = window.getSelection();
    const text = selection?.toString() ?? "";
    if (!selection || selection.isCollapsed || !text.trim()) return hideSelectionBar();
    const range = selection.getRangeAt(0);
    const source = sourceOf(range.commonAncestorContainer);
    if (!source) return hideSelectionBar();
    const rect = range.getBoundingClientRect();
    showSelectionBar({ source, text: text.trim(), ...relative(rect.left + rect.width / 2, rect.top, rect.bottom) });
  }

  // The selection settles after the event; read it on the next tick.
  document.addEventListener("mouseup", (e) => setTimeout(() => read(e)));
  document.addEventListener("keyup", (e) => e.shiftKey && setTimeout(() => read(e)));
  document.addEventListener("mousedown", (e) => {
    if (!e.target.closest?.(".selection-bar")) hideSelectionBar();
  });
  document.addEventListener("scroll", () => state.ui.selectionBar && hideSelectionBar(), true);
}
