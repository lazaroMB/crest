// The line above the prompt while crest works (crates/ui/src/activity.rs):
//   ⠋ Working · grep "add(" in src/ · 7s · Ctrl+C interrupts
// It shows up after 150 ms, so quick steps do not flicker. Prompts sent
// during a turn queue under it.

import { h } from "../../lib/h.js";
import "./activity-line.scss";

const FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
const SHOW_AFTER = 150;
const truncate = (s, n) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

export function ActivityLine(state, now = Date.now()) {
  const { activity, queue } = state.turn;
  const elapsed = activity ? now - activity.startedAt : 0;
  const shown = activity && elapsed >= SHOW_AFTER;
  if (!shown && !queue.length) return null;

  const rest = shown
    ? [activity.stage, activity.detail && truncate(activity.detail, 120), `${Math.floor(elapsed / 1000)}s`, "Ctrl+C interrupts"].filter(Boolean)
    : [];

  return h(
    "div",
    { class: "activity" },
    shown &&
      h(
        "div",
        { class: "activity__line" },
        h("span", { class: "activity__verb" }, `${FRAMES[Math.floor(now / 100) % FRAMES.length]} ${activity.verb}`),
        rest.map((part) => h("span", { class: "muted" }, ` · ${part}`)),
      ),
    queue.length > 0 && [
      h("div", { class: "activity__queue muted" }, `${queue.length} queued · ↑ edits the last · Ctrl+C discards`),
      queue.map((prompt) => h("div", { class: "activity__queued muted" }, `↳ ${truncate(prompt, 100)}`)),
    ],
  );
}
