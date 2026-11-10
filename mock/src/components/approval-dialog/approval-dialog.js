// "Allow edit_file? src/math.rs": the diff, and five answers (crates/ui/src/approval.rs).
// y/s/p/e/n answer at once; ↑↓ and Enter pick one; Esc rejects.

import { h } from "../../lib/h.js";
import { approve } from "../../actions.js";
import { setUi } from "../../store.js";
import { DiffLines } from "../blocks/diff-block.js";
import "./approval-dialog.scss";

export const APPROVAL_OPTIONS = [
  { key: "y", choice: "once", label: "Yes, once" },
  { key: "s", choice: "session", label: "Yes, this session for", chip: true },
  { key: "p", choice: "project", label: "Yes, always in this project for", chip: true },
  { key: "e", choice: "all-edits", label: "Yes, allow all edits this session", note: "(auto-edit · <C-g>p to undo)" },
  { key: "n", choice: "reject", label: "No, tell crest why…", danger: true },
];

function Option(option, index, { selected, pattern }) {
  return h(
    "div",
    {
      class: ["approval__option", selected && "approval__option--selected"],
      onMouseenter: () => setUi({ approvalIndex: index }),
      onClick: () => approve(option.choice),
    },
    h("span", { class: "approval__cursor" }, selected ? "›" : ""),
    h("span", { class: ["approval__key", option.danger && "approval__key--danger"] }, option.key),
    h("span", option.label),
    option.chip && h("span", { class: "approval__chip" }, pattern),
    option.note && h("span", { class: "muted" }, option.note),
  );
}

export function ApprovalDialog(state) {
  const request = state.turn.approval;
  if (!request || state.turn.chatId !== state.chat?.id) return null;
  const preview = request.preview;
  const pattern = `${request.tool} ${request.pattern}`;
  return h(
    "div",
    { class: "approval-overlay" },
    h(
      "div",
      { class: "approval", role: "dialog" },
      h("div", { class: "approval__title" }, h("strong", `Allow ${request.tool}?`), h("span", { class: "muted" }, request.subject)),
      h(
        "div",
        { class: "approval__preview" },
        h(
          "div",
          { class: "approval__file" },
          h("strong", preview.path),
          h("span", { class: "diff-block__added" }, `+${preview.added}`),
          h("span", { class: "diff-block__removed" }, `-${preview.removed}`),
        ),
        DiffLines(preview.lines),
      ),
      h(
        "div",
        { class: "approval__options" },
        APPROVAL_OPTIONS.map((option, i) => Option(option, i, { selected: i === state.ui.approvalIndex, pattern })),
      ),
      h("div", { class: "approval__footer" }, "↑↓ choose · Tab/⇧Tab pattern · Enter confirm · Esc reject"),
    ),
  );
}
