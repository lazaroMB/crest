// The marks rail on the right: one mark per question asked. Ctrl+G O opens
// the list of questions (crates/ui/src/timeline/view.rs, outline.rs).

import { h } from "../../lib/h.js";
import { select } from "../../actions.js";
import { setUi } from "../../store.js";
import "./outline.scss";

export function Outline(state) {
  const questions = (state.chat?.blocks ?? []).filter((b) => b.kind === "user");
  if (!questions.length) return null;

  const blocks = state.chat.blocks;
  const selectedAt = blocks.findIndex((b) => b.id === state.ui.selected);
  const active = [...questions].reverse().find((q) => selectedAt === -1 || blocks.indexOf(q) <= selectedAt) ?? questions[0];
  const jump = (id) => select(id);

  return h(
    "div",
    { class: "outline" },
    h(
      "div",
      { class: "outline__rail", onClick: () => setUi({ outlineOpen: !state.ui.outlineOpen }) },
      questions.map((q) =>
        h(
          "div",
          { class: "outline__mark", title: q.text, onClick: (e) => (e.stopPropagation(), jump(q.id)) },
          h("span", { class: ["outline__pill", q === active && "outline__pill--active"] }),
        ),
      ),
    ),
    state.ui.outlineOpen &&
      h(
        "div",
        { class: "outline__menu" },
        h(
          "div",
          { class: "outline__menu-head" },
          h("span", "Esc to close"),
          h("button", { onClick: () => setUi({ outlineOpen: false }) }, "✕"),
        ),
        questions.map((q, i) =>
          h(
            "button",
            { class: ["outline__item", q === active && "outline__item--active"], onClick: () => jump(q.id) },
            `${i + 1}. ${q.text}`,
          ),
        ),
      ),
  );
}
