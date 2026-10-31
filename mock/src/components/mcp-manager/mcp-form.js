// The add/edit form of an MCP server: the same fields `app.mcp.add` takes in init.lua,
// without having to know Lua. Typing writes into the draft in place; only a
// change of shape (type, a row added or removed) re-renders.

import { h } from "../../lib/h.js";
import { MCP_TEMPLATES, addMcpRow, applyMcpTemplate, cancelMcpForm, editMcpDraft, removeMcpRow, saveMcpForm, testMcpForm } from "../../actions.js";

const EFFECTS = [
  ["read", "only reads"],
  ["write", "changes files or data"],
  ["execute", "runs things (default)"],
  ["network", "talks to the internet"],
];

function Text(draft, path, { placeholder, key, type = "text", wide } = {}) {
  const [a, b] = path;
  const target = b == null ? draft : draft[a];
  const field = b == null ? a : b;
  return h("input", {
    class: ["mcp-input", wide && "mcp-input--wide"],
    type,
    "data-key": key ?? `mcp-form-${path.join("-")}`,
    placeholder,
    value: target[field],
    spellcheck: "false",
    onInput: (e) => (target[field] = e.target.value),
  });
}

const Row = (label, hint, ...control) =>
  h("div", { class: "mcp-form__row" }, h("div", { class: "mcp-form__label" }, label, hint && h("div", { class: "mcp-form__hint" }, hint)), h("div", { class: "mcp-form__control" }, ...control));

function Segmented(options, value, onPick) {
  return h(
    "div",
    { class: "mcp-seg" },
    options.map(([id, label, title]) => h("button", { class: ["mcp-seg__item", value === id && "mcp-seg__item--on"], title, onClick: (e) => (e.preventDefault(), onPick(id)) }, label)),
  );
}

function Pairs(draft, list, keyHolder, valueHolder) {
  return h(
    "div",
    { class: "mcp-rows" },
    draft[list].map((row, i) =>
      h(
        "div",
        { class: "mcp-rows__row" },
        h("input", { class: "mcp-input mcp-input--key", "data-key": `mcp-${list}-${i}-k`, placeholder: keyHolder, value: row.key, spellcheck: "false", onInput: (e) => (row.key = e.target.value) }),
        h("span", { class: "muted" }, list === "env" ? "=" : ":"),
        h("input", {
          class: "mcp-input",
          "data-key": `mcp-${list}-${i}-v`,
          placeholder: valueHolder,
          value: row.value,
          spellcheck: "false",
          onInput: (e) => (row.value = e.target.value),
        }),
        h("button", { class: "mcp-rows__remove", title: "Remove", onClick: (e) => (e.preventDefault(), removeMcpRow(list, i)) }, "×"),
      ),
    ),
    h("button", { class: "mcp-link", onClick: (e) => (e.preventDefault(), addMcpRow(list)) }, list === "env" ? "+ variable" : "+ header"),
  );
}

function Args(draft) {
  return h(
    "div",
    { class: "mcp-rows" },
    draft.args.map((arg, i) =>
      h(
        "div",
        { class: "mcp-rows__row" },
        h("input", {
          class: "mcp-input",
          "data-key": `mcp-args-${i}`,
          placeholder: i === 0 ? "-y" : "argument",
          value: arg,
          spellcheck: "false",
          onInput: (e) => (draft.args[i] = e.target.value),
        }),
        h("button", { class: "mcp-rows__remove", title: "Remove", onClick: (e) => (e.preventDefault(), removeMcpRow("args", i)) }, "×"),
      ),
    ),
    h("button", { class: "mcp-link", onClick: (e) => (e.preventDefault(), addMcpRow("args")) }, "+ argument"),
  );
}

export function McpForm(form) {
  const d = form.draft;
  const stdio = d.transport === "stdio";
  return h(
    "div",
    { class: "mcp-detail mcp-form" },
    h("div", { class: "mcp-detail__head" }, h("strong", form.mode === "add" ? "Add an MCP server" : `Edit ${form.name}`), h("span", { class: "muted" }, "saving starts it at once")),
    h(
      "div",
      { class: "mcp-detail__body", "data-scroll": "mcp-form" },
      form.mode === "add" &&
        h(
          "div",
          { class: "mcp-templates" },
          h("span", { class: "muted" }, "Start from"),
          MCP_TEMPLATES.map((t) => h("button", { class: "mcp-chip mcp-chip--button", onClick: () => applyMcpTemplate(t) }, t.label)),
        ),
      Row("Name", "letters, digits, - and _", Text(d, ["name"], { placeholder: "github" })),
      Row(
        "Type",
        stdio ? "a local program crest starts" : "a remote server crest connects to",
        Segmented(
          [
            ["stdio", "Local command (stdio)"],
            ["http", "Remote URL (http)"],
          ],
          d.transport,
          (t) => editMcpDraft((draft) => (draft.transport = t)),
        ),
      ),
      stdio
        ? [
            Row("Command", null, Text(d, ["command"], { placeholder: "npx", wide: true })),
            Row("Arguments", "one per row", Args(d)),
            Row("Environment", "${VAR} and ${VAR:-default} expand", Pairs(d, "env", "KEY", "value")),
            Row("Working dir", "empty: the workspace", Text(d, ["cwd"], { placeholder: ".", wide: true })),
          ]
        : [
            Row("URL", null, Text(d, ["url"], { placeholder: "https://example.com/mcp", wide: true })),
            Row("Headers", "e.g. Authorization", Pairs(d, "headers", "Header", "value or ${TOKEN}")),
            Row(
              "OAuth",
              "log in through the browser",
              h(
                "div",
                { class: "mcp-rows" },
                Segmented(
                  [
                    [false, "No"],
                    [true, "Yes"],
                  ],
                  d.oauth.enabled,
                  (on) => editMcpDraft((draft) => (draft.oauth.enabled = on)),
                ),
                d.oauth.enabled &&
                  h(
                    "div",
                    { class: "mcp-form__nested" },
                    h("div", { class: "mcp-form__hint" }, "Leave the client empty to register crest on its own (dynamic registration)."),
                    Row("Client id", null, Text(d, ["oauth", "clientId"], { placeholder: "optional" })),
                    Row("Client secret", null, Text(d, ["oauth", "clientSecret"], { placeholder: "optional", type: "password" })),
                    Row("Scopes", null, Text(d, ["oauth", "scopes"], { placeholder: "space separated" })),
                    Row("Callback port", null, Text(d, ["oauth", "callbackPort"], { placeholder: "any free port" })),
                  ),
              ),
            ),
          ],
      Row(
        "Available in",
        null,
        Segmented(
          [
            ["global", "Every project", "~/.config/crest/mcp.json"],
            ["workspace", "This workspace", ".crest/mcp.json"],
          ],
          d.scope,
          (s) => editMcpDraft((draft) => (draft.scope = s)),
        ),
      ),
      Row(
        "Effect",
        EFFECTS.find(([id]) => id === d.effect)?.[1],
        Segmented(
          EFFECTS.map(([id, hint]) => [id, id, hint]),
          d.effect,
          (e) => editMcpDraft((draft) => (draft.effect = e)),
        ),
      ),
      Row(
        "Timeouts",
        "seconds",
        h(
          "div",
          { class: "mcp-inline" },
          Text(d, ["timeout"], { placeholder: "60" }),
          h("span", { class: "muted" }, "per call"),
          Text(d, ["startupTimeout"], { placeholder: "30" }),
          h("span", { class: "muted" }, "to start"),
        ),
      ),
    ),
    form.error && h("div", { class: "mcp-result mcp-result--error" }, h("span", "✗"), h("span", form.error)),
    form.testing && h("div", { class: "mcp-result" }, h("span", { class: "mcp-spinner" }), h("span", "Connecting…")),
    form.test && h("div", { class: ["mcp-result", form.test.ok ? "mcp-result--ok" : "mcp-result--error"] }, h("span", form.test.ok ? "✓" : "✗"), h("span", form.test.text)),
    h(
      "div",
      { class: "mcp-actions mcp-form__buttons" },
      h("button", { class: "mcp-btn", disabled: form.testing || null, onClick: testMcpForm }, "Test connection"),
      h("span", { class: "mcp-actions__gap" }),
      h("button", { class: "mcp-btn", onClick: cancelMcpForm }, "Cancel"),
      h("button", { class: "mcp-btn mcp-btn--primary", onClick: saveMcpForm }, form.mode === "add" ? "Add and start" : "Save and restart"),
    ),
  );
}
