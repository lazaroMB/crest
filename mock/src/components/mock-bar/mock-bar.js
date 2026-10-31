// The mock's own toolbar, outside the crest window: theme, window width and
// panels, to look at the UI in different conditions. Ctrl+Shift+M hides it.

import { h } from "../../lib/h.js";
import { setUi } from "../../store.js";
import "./mock-bar.scss";

const WIDTHS = ["fill", 1440, 1024, 720];

const toggle = (label, on, onClick) => h("button", { class: ["mockbar__button", on && "mockbar__button--on"], onClick }, label);

export function MockBar(state) {
  const { ui } = state;
  if (!ui.mockBar) return null;
  return h(
    "div",
    { class: "mockbar" },
    h("strong", "crest UI mock"),
    h("span", { class: "mockbar__group" }, "theme", [
      toggle("dark", ui.theme === "twocrest", () => setUi({ theme: "twocrest" })),
      toggle("light", ui.theme === "twocrest-light", () => setUi({ theme: "twocrest-light" })),
    ]),
    h(
      "span",
      { class: "mockbar__group" },
      "width",
      WIDTHS.map((w) => toggle(String(w), ui.width === w, () => setUi({ width: w }))),
    ),
    h("span", { class: "mockbar__group" }, "panels", [
      toggle("chats", ui.chatsShown, () => setUi({ chatsShown: !ui.chatsShown })),
      toggle("tree", ui.treeShown, () => setUi({ treeShown: !ui.treeShown })),
    ]),
    h("span", { class: "mockbar__hint" }, "try: fix the bug · rename add · /research … · is it faster · fail"),
    h("button", { class: "mockbar__button", onClick: () => setUi({ mockBar: false }) }, "hide (Ctrl+Shift+M)"),
  );
}
