// Draws ```mermaid fences in the doc editor. The source stays on the figure
// (`data-source`) so saving writes the fence back. Loaded on the first diagram.

import { h } from "./h.js";

let loading;
let themeName = "";
let seq = 0;

function load() {
  loading ??= import("mermaid").then((mod) => mod.default);
  return loading;
}

function themeOf() {
  return document.querySelector(".window")?.dataset.theme === "twocrest-light" ? "neutral" : "dark";
}

async function paintOne(mermaid, node, theme) {
  const source = node.dataset.source ?? "";
  const key = `${theme}\0${source}`;
  if (node.dataset.painted === key || node.dataset.painting === key) return;
  node.dataset.painting = key;
  const canvas = node.querySelector(".md-mermaid__canvas");
  try {
    const { svg } = await mermaid.render(`crest-mmd-${++seq}`, source);
    if (node.dataset.painting !== key) return;
    canvas.innerHTML = svg;
    node.classList.remove("md-mermaid--error");
    node.dataset.painted = key;
  } catch (error) {
    if (node.dataset.painting !== key) return;
    node.dataset.painted = key;
    node.classList.add("md-mermaid--error");
    const message = String(error?.message || "This diagram could not be drawn").split("\n")[0];
    canvas.replaceChildren(h("div", { class: "md-mermaid__error" }, message), h("pre", { class: "md-code" }, h("code", source)));
  }
}

/** Draws every `.md-mermaid` under `root`. A diagram already drawn for this theme is left as it is. */
export async function paintMermaid(root) {
  if (!root) return;
  const nodes = [...root.querySelectorAll(".md-mermaid")];
  if (!nodes.length) return;
  const theme = themeOf();
  const mermaid = await load();
  if (themeName !== theme) {
    themeName = theme;
    mermaid.initialize({ startOnLoad: false, securityLevel: "strict", suppressErrorRendering: true, theme, fontFamily: "inherit" });
  }
  await Promise.all(nodes.map((node) => paintOne(mermaid, node, theme)));
}
