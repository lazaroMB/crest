// The prompt: "❯" and a textarea that grows with its text (crates/ui/src/input.rs).
// Enter sends, Shift+Enter adds a line, ↑ on an empty prompt brings back the
// last one, and "/" opens the command palette.
//
// Mounted once (mountPromptInput) so the textarea keeps its text, caret and focus.

import { h } from "../../lib/h.js";
import { runCommand, send } from "../../actions.js";
import { setUi } from "../../store.js";
import { CommandPalette, matchCommands } from "./command-palette.js";
import "./prompt-input.scss";

const PLACEHOLDER = "Ask crest…  (/ for commands · <C-g>ff files · <C-g>fg text)";

/** "/mode <ask|auto-edit|plan>" -> "/mode " ; "/clear" -> "/clear" */
const completion = (usage) => usage.replace(/\s[<[].*$/, "").replace(/\s.*$/, "") + (/[<[]/.test(usage) ? " " : "");

export function mountPromptInput(root) {
  const glyph = h("span", { class: "prompt__glyph" }, "❯");
  const textarea = h("textarea", { class: "prompt__text", rows: 1, placeholder: PLACEHOLDER, spellcheck: "false" });
  const paletteSlot = h("div");
  root.classList.add("prompt");
  root.append(paletteSlot, h("div", { class: "prompt__row" }, glyph, textarea));

  let commands = [];
  let matches = [];
  let selected = 0;
  let paletteHidden = false;
  const history = [];

  function resize() {
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 240)}px`;
  }

  function renderPalette() {
    matches = paletteHidden ? [] : matchCommands(commands, textarea.value);
    selected = Math.min(selected, Math.max(0, matches.length - 1));
    paletteSlot.replaceChildren(matches.length ? CommandPalette(matches, selected, pick) : "");
  }

  function setValue(value) {
    textarea.value = value;
    resize();
    renderPalette();
  }

  function pick(index) {
    const usage = matches[index].usage;
    const text = completion(usage);
    if (text.endsWith(" ")) return setValue(text);
    setValue("");
    runCommand(text);
  }

  function submit() {
    const text = textarea.value;
    if (!text.trim()) return;
    history.push(text);
    setValue("");
    if (!(text.startsWith("/") && runCommand(text.trim()))) send(text);
  }

  textarea.addEventListener("input", () => {
    paletteHidden = false;
    selected = 0;
    resize();
    renderPalette();
  });

  textarea.addEventListener("keydown", (e) => {
    const paletteOpen = matches.length > 0;
    if (paletteOpen && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      selected = (selected + (e.key === "ArrowDown" ? 1 : -1) + matches.length) % matches.length;
      return renderPalette();
    }
    if (paletteOpen && e.key === "Tab") {
      e.preventDefault();
      return setValue(completion(matches[selected].usage));
    }
    if (paletteOpen && e.key === "Escape") {
      e.stopPropagation();
      paletteHidden = true;
      return renderPalette();
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      const stillTyping = paletteOpen && textarea.value.trim() !== completion(matches[selected].usage).trim();
      return stillTyping ? pick(selected) : submit();
    }
    if (e.key === "ArrowUp" && !textarea.value && history.length) {
      e.preventDefault();
      setValue(history[history.length - 1]);
    }
  });

  textarea.addEventListener("focus", () => setUi({ focus: "prompt" }));

  return {
    focus: () => textarea.focus(),
    clear: () => setValue(""),
    hasText: () => textarea.value.length > 0,
    update(state) {
      commands = state.commands;
      glyph.classList.toggle("prompt__glyph--focused", state.ui.focus === "prompt");
    },
  };
}
