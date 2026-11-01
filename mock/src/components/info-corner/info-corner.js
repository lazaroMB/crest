// Session notes that should not sit in the prompt or the status bar.
// A short step, or an error that has left the 6 s bar, appears as a toast
// for 3 s and then stays behind the information mark.

import { h } from "../../lib/h.js";
import { dismissInfo, toggleInfo } from "../../actions.js";
import { SHOW_AFTER_MS } from "../../visibility.js";
import "./info-corner.scss";

function entries(state) {
  const notes = state.notices
    .filter((n) => n.id !== state.notice?.id && (n.level === "error" || n.level === "warning"))
    .map((n) => ({ id: n.id, level: n.level, text: n.text }));
  const steps = state.turn.steps
    .filter((s) => s.elapsedMs < SHOW_AFTER_MS)
    .map((s) => ({ id: s.id, level: "info", text: `${s.label} · ${s.elapsedMs}ms` }));
  return [...notes, ...steps];
}

export function InfoCorner(state) {
  const items = entries(state);
  const toast = state.toast && items.some((item) => item.id === state.toast.id) ? state.toast : null;
  if (!items.length && !toast) return null;

  return h(
    "div",
    { class: "info" },
    state.ui.infoOpen &&
      h(
        "ul",
        { class: "info__panel" },
        items.map((item) =>
          h(
            "li",
            { class: `info__row info__row--${item.level}` },
            h("span", { class: "info__text" }, item.text),
            h("button", { class: "info__dismiss", title: "Dismiss", onClick: () => dismissInfo(item.id) }, "×"),
          ),
        ),
      ),
    !state.ui.infoOpen &&
      toast &&
      h("div", { class: `info__toast info__toast--${toast.level}` }, toast.text),
    h(
      "button",
      { class: ["info__mark", state.ui.infoOpen && "info__mark--open"], title: "Information", onClick: toggleInfo },
      "I",
    ),
  );
}
