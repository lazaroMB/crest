// The status bar (crates/ui/src/status.rs):
//   ▲▲ │ claude-sonnet-5-5 ● │ ask │ $0.0735          notice · fix/add-negatives ~2 ↑1 │ crest

import { h } from "../../lib/h.js";
import { cycleMode } from "../../actions.js";
import { Brand } from "../brand/brand.js";
import "./status-bar.scss";

function gitText(git) {
  const parts = [
    git.staged && `+${git.staged}`,
    git.modified && `~${git.modified}`,
    git.untracked && `?${git.untracked}`,
    git.conflicts && `!${git.conflicts}`,
    git.ahead && `↑${git.ahead}`,
    git.behind && `↓${git.behind}`,
  ].filter(Boolean);
  return [git.branch, ...parts].join(" ");
}

function gitTone(git) {
  if (git.conflicts) return "error";
  return git.staged || git.modified || git.untracked ? "warning" : "success";
}

const bar = () => h("span", { class: "sep" }, "│");

export function StatusBar(state) {
  const { status, notice, turn } = state;
  if (!status) return null;
  return h(
    "footer",
    { class: "status" },
    h(
      "div",
      { class: "status__side" },
      Brand({ height: 14 }),
      bar(),
      h("span", status.model, turn.busy && h("span", { class: "status__busy" }, " ●")),
      bar(),
      h("button", { class: "status__mode", title: "Ctrl+G p cycles the mode", onClick: cycleMode }, status.mode),
      bar(),
      h("span", `$${status.cost.toFixed(4)}`),
    ),
    h(
      "div",
      { class: "status__side status__side--right" },
      notice && [h("span", { class: `status__notice status__notice--${notice.level}` }, notice.text), bar()],
      h("span", { class: `status__git status__git--${gitTone(status.git)}` }, gitText(status.git)),
      bar(),
      h("span", { class: "muted" }, status.workspace),
    ),
  );
}
