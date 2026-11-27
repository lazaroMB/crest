// The prompt: a + button, the chosen chat mode as a chip, "❯" and a textarea
// that grows with its text (crates/ui/src/input.rs).
//
// + opens a menu: the chat mode (Chat, Research, Deep research), what to add
// (a workspace file with @, a command with /) and what to manage (the MCP
// servers, the same as /mcp). The mode stays until changed;
// in research it sends "/research <text>", in deep research
// "/research --deep <text>", the same lines a person can type. <C-g>m cycles it.
//
// A passage the person passed from the doc ("Ask agent") sits above the line
// as a chip and goes with the next prompt.
//
// Enter sends, Shift+Enter adds a line, ↑ on an empty prompt brings back the
// last one, and "/" opens the grouped command palette.
//
// Mounted once (mountPromptInput) so the textarea keeps its text, caret and focus.

import { h } from "../../lib/h.js";
import { runCommand, send, setChatMode, toggleModeMenu } from "../../actions.js";
import { setUi } from "../../store.js";
import { openDoc, referenceDoc, removePromptContext } from "../../doc-actions.js";
import { palette } from "../../commands.js";
import { CHAT_MODES, modeCommand, modeOf } from "../../modes.js";
import { icon } from "../icons.js";
import { CommandPalette } from "./command-palette.js";
import "./prompt-input.scss";

const KEYS_HINT = "  (/ for commands · <C-g>m mode · <C-g>ff files)";

const MODE_ICON = { chat: "chat", research: "research", deep: "deep" };

/** "/doc find <text>" -> "/doc find "; "/doc" -> "/doc"; "/open @<path>" -> "/open @". */
const completion = (usage) => {
  const at = usage.indexOf("@");
  if (at !== -1) return usage.slice(0, at + 1);
  const cut = usage.search(/\s[<[]/);
  return cut === -1 ? usage : `${usage.slice(0, cut)} `;
};

export function mountPromptInput(root) {
  const plus = h("button", { class: "prompt__plus", title: "Mode and attachments", "aria-label": "Mode and attachments" }, icon("plus"));
  const chipSlot = h("span", { class: "prompt__chip-slot" });
  const glyph = h("span", { class: "prompt__glyph" }, "❯");
  const textarea = h("textarea", { class: "prompt__text", rows: 1, spellcheck: "false" });
  const paletteSlot = h("div");
  const menuSlot = h("div");
  const contextSlot = h("div", { class: "prompt__context" });
  root.classList.add("prompt");
  root.append(menuSlot, paletteSlot, contextSlot, h("div", { class: "prompt__row" }, plus, chipSlot, glyph, textarea));

  let commands = [];
  let docs = [];
  let view = null;
  let openGroup = null;
  let selected = 0;
  let paletteHidden = false;
  let mode = "chat";
  let menuOpen = false;
  let menuIndex = 0;
  let context = [];
  const history = [];

  const menuItems = () => [
    ...CHAT_MODES.map((m) => ({ kind: "mode", id: m.id, icon: MODE_ICON[m.id], label: m.label, hint: m.hint })),
    { kind: "insert", text: "@", icon: "file", label: "File from the workspace", hint: "@" },
    { kind: "insert", text: "/doc", icon: "doc", label: "Document", hint: "/doc" },
    { kind: "insert", text: "/", icon: "slash", label: "Command", hint: "/" },
    { kind: "command", command: "/mcp", icon: "panel", label: "MCP servers", hint: "/mcp" },
  ];

  function resize() {
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 240)}px`;
  }

  function renderPalette() {
    view = paletteHidden ? null : palette(commands, textarea.value, openGroup, docs);
    const count = view?.rows.length ?? 0;
    selected = Math.min(selected, Math.max(0, count - 1));
    paletteSlot.replaceChildren(view ? CommandPalette(view, selected, pick) : "");
  }

  function renderMenu() {
    plus.classList.toggle("prompt__plus--open", menuOpen);
    if (!menuOpen) return menuSlot.replaceChildren("");
    const items = menuItems();
    const item = (it, i) =>
      h(
        "div",
        {
          class: ["mode-menu__row", i === menuIndex && "mode-menu__row--selected", it.kind === "mode" && it.id === mode && "mode-menu__row--on"],
          onMousedown: (e) => (e.preventDefault(), choose(it)),
        },
        icon(it.icon),
        h("span", { class: "mode-menu__label" }, it.label),
        h("span", { class: "mode-menu__hint" }, it.hint),
        it.kind === "mode" && it.id === mode && h("span", { class: "mode-menu__check" }, "✓"),
      );
    menuSlot.replaceChildren(
      h(
        "div",
        { class: "mode-menu", role: "menu" },
        h("div", { class: "mode-menu__label-row" }, "Mode"),
        items.filter((it) => it.kind === "mode").map((it) => item(it, items.indexOf(it))),
        h("div", { class: "mode-menu__sep" }),
        h("div", { class: "mode-menu__label-row" }, "Add"),
        items.filter((it) => it.kind === "insert").map((it) => item(it, items.indexOf(it))),
        h("div", { class: "mode-menu__sep" }),
        h("div", { class: "mode-menu__label-row" }, "Manage"),
        items.filter((it) => it.kind === "command").map((it) => item(it, items.indexOf(it))),
      ),
    );
  }

  function choose(it) {
    if (it.kind === "mode") setChatMode(it.id);
    else if (it.kind === "command") {
      toggleModeMenu(false);
      return runCommand(it.command);
    } else {
      toggleModeMenu(false);
      setValue(textarea.value + it.text);
    }
    textarea.focus();
  }

  /** The passages passed from the doc ("Ask agent"), each a chip the person can drop before sending. */
  function renderContext(items) {
    contextSlot.replaceChildren(
      ...items.map((c) => {
        const whole = c.kind === "doc";
        const shown = (whole ? c.title : c.text).replace(/\s+/g, " ");
        return h(
          "span",
          { class: "prompt__passage", title: whole ? c.title : c.text },
          icon(whole ? "doc" : "agent", 12),
          h("span", { class: "prompt__passage-label" }, whole ? "doc" : "from the doc"),
          h("span", { class: "prompt__passage-text" }, whole ? shown : `“${shown.slice(0, 80)}${shown.length > 80 ? "…" : ""}”`),
          h("button", { class: "prompt__chip-close", title: "Remove", "aria-label": "Remove", onClick: () => removePromptContext(c.id) }, icon("close", 10)),
        );
      }),
    );
    renderChip();
  }

  function renderChip() {
    const m = modeOf(mode);
    const asking = context.some((c) => c.kind === "doc-selection")
      ? "What should the agent do with this passage? (e.g. make it shorter)"
      : context.some((c) => c.kind === "doc")
        ? "What should the agent do with this document?"
        : "";
    textarea.placeholder = asking || m.placeholder + KEYS_HINT;
    chipSlot.replaceChildren(
      mode === "chat"
        ? ""
        : h(
            "span",
            { class: `prompt__chip prompt__chip--${mode}`, title: `${m.hint} · Backspace on an empty prompt goes back to chat` },
            h("span", {}, m.label),
            h("button", { class: "prompt__chip-close", title: "Back to chat", "aria-label": "Back to chat", onClick: () => setChatMode("chat") }, icon("close", 10)),
          ),
    );
  }

  function setValue(value) {
    textarea.value = value;
    resize();
    renderPalette();
  }

  function pick(index) {
    if (view.kind === "groups") {
      openGroup = view.rows[index].id;
      selected = 0;
      return renderPalette();
    }
    const row = view.rows[index];
    if (row.inert) return;
    if (row.docId) {
      openGroup = null;
      setValue("");
      return row.docAction === "use" ? referenceDoc(row.docId) : openDoc(row.docId, "split");
    }
    const text = completion(row.usage);
    openGroup = null;
    if (text.endsWith(" ")) return setValue(text);
    setValue("");
    runCommand(text);
  }

  function submit() {
    const line = modeCommand(mode, textarea.value);
    if (!line) return;
    history.push(textarea.value);
    openGroup = null;
    setValue("");
    if (!((line.startsWith("/") || line.startsWith("!")) && runCommand(line))) send(line);
  }

  plus.addEventListener("mousedown", (e) => {
    e.preventDefault();
    menuIndex = Math.max(0, CHAT_MODES.findIndex((m) => m.id === mode));
    toggleModeMenu();
    textarea.focus();
  });

  textarea.addEventListener("input", () => {
    paletteHidden = false;
    if (textarea.value !== "/") openGroup = null;
    selected = 0;
    resize();
    renderPalette();
  });

  textarea.addEventListener("keydown", (e) => {
    if (menuOpen) {
      const items = menuItems();
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        menuIndex = (menuIndex + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
        return renderMenu();
      }
      if (e.key === "Enter") {
        e.preventDefault();
        return choose(items[menuIndex]);
      }
    }
    const paletteOpen = Boolean(view?.rows.length);
    if (paletteOpen && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      selected = (selected + (e.key === "ArrowDown" ? 1 : -1) + view.rows.length) % view.rows.length;
      return renderPalette();
    }
    if (paletteOpen && view.kind === "groups" && (e.key === "ArrowRight" || e.key === "Tab")) {
      e.preventDefault();
      return pick(selected);
    }
    if (paletteOpen && view.group && (e.key === "ArrowLeft" || (e.key === "Backspace" && textarea.value === "/"))) {
      e.preventDefault();
      openGroup = null;
      selected = 0;
      return renderPalette();
    }
    if (paletteOpen && e.key === "Tab") {
      e.preventDefault();
      return setValue(completion(view.rows[selected].usage));
    }
    if (paletteOpen && e.key === "Escape") {
      e.stopPropagation();
      paletteHidden = true;
      openGroup = null;
      return renderPalette();
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (!paletteOpen) return submit();
      if (view.kind === "groups") return pick(selected);
      const row = view.rows[selected];
      if (row.docId || row.inert) return pick(selected);
      const done = completion(row.usage).trim();
      const typed = textarea.value.trim();
      // An argument already started ("/doc new Notes") runs as typed. A shorter line completes.
      const started = typed === done || typed.startsWith(`${done} `);
      return started ? submit() : pick(selected);
    }
    if (e.key === "Backspace" && !textarea.value && mode !== "chat") {
      e.preventDefault();
      return setChatMode("chat");
    }
    if (e.key === "ArrowUp" && !textarea.value && history.length) {
      e.preventDefault();
      setValue(history[history.length - 1]);
    }
  });

  textarea.addEventListener("focus", () => setUi({ focus: "prompt" }));
  textarea.addEventListener("blur", () => menuOpen && toggleModeMenu(false));

  renderChip();

  return {
    focus: () => textarea.focus(),
    clear: () => setValue(""),
    hasText: () => textarea.value.length > 0,
    update(state) {
      commands = state.commands;
      docs = state.docs;
      glyph.classList.toggle("prompt__glyph--focused", state.ui.focus === "prompt");
      if (state.ui.chatMode !== mode) {
        mode = state.ui.chatMode;
        renderChip();
      }
      if (state.ui.modeMenu !== menuOpen) {
        menuOpen = state.ui.modeMenu;
        renderMenu();
      }
      if (state.ui.promptContext !== context) {
        context = state.ui.promptContext;
        renderContext(context);
      }
    },
  };
}
