// A bar chart, vertical ("bar") or horizontal ("hbar"), drawn with plain HTML
// (crates/ui/src/blocks/chart/). `t` swaps it for the table behind it.

import { h } from "../../lib/h.js";
import { copyChartImage } from "../../actions.js";
import { Table } from "./table-block.js";
import "./chart-block.scss";

/** Round the axis up to 1, 2 or 5 times a power of ten, in about 5 steps. */
function niceAxis(max) {
  const raw = max / 5;
  const power = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 5, 10].map((m) => m * power).find((s) => s >= raw);
  const top = Math.ceil(max / step) * step;
  const ticks = [];
  for (let t = 0; t <= top + step / 2; t += step) ticks.push(t);
  return { top, ticks };
}

const short = (n) => (n >= 1000 ? `${+(n / 1000).toFixed(1)}K` : String(n));
const pct = (v, top) => `${(v / top) * 100}%`;

function Grid(ticks, top, vertical) {
  return h(
    "div",
    { class: "chart__grid" },
    ticks.map((t) => h("span", { class: "chart__gridline", style: vertical ? { bottom: pct(t, top) } : { left: pct(t, top) } })),
  );
}

function HBars(labels, values) {
  const { top, ticks } = niceAxis(Math.max(...values));
  return h(
    "div",
    { class: "hbars" },
    h(
      "div",
      { class: "hbars__plot" },
      Grid(ticks, top, false),
      labels.map((label, i) => [
        h("div", { class: "hbars__label" }, label),
        h(
          "div",
          { class: "hbars__track" },
          h("div", { class: "hbars__bar", style: { width: pct(values[i], top) } }),
          h("span", { class: "hbars__value" }, short(values[i])),
        ),
      ]),
    ),
    h(
      "div",
      { class: "hbars__axis" },
      ticks.map((t) => h("span", { style: { left: pct(t, top) } }, short(t))),
    ),
  );
}

function VBars(labels, values) {
  const { top, ticks } = niceAxis(Math.max(...values));
  return h(
    "div",
    { class: "vbars" },
    h("div", { class: "vbars__axis" }, ticks.map((t) => h("span", { style: { bottom: pct(t, top) } }, short(t)))),
    h(
      "div",
      { class: "vbars__plot" },
      Grid(ticks, top, true),
      labels.map((label, i) =>
        h(
          "div",
          { class: "vbars__slot" },
          h(
            "div",
            { class: "vbars__bar", style: { height: pct(values[i], top) } },
            h("span", { class: "vbars__value" }, short(values[i])),
          ),
          h("span", { class: "vbars__label" }, label),
        ),
      ),
    ),
  );
}

export function ChartBlock(block, view) {
  const values = block.series[0].values;
  const kind = block.chart === "hbar" ? "bar" : block.chart;
  const hint = view.altView ? `${kind} chart · table · t: chart` : `${kind} chart · t: table`;
  const table = {
    columns: ["", ...block.series.map((s) => s.name)],
    rows: block.labels.map((label, i) => [label, ...block.series.map((s) => String(s.values[i]))]),
    numeric: [false, ...block.series.map(() => true)],
  };
  const root = h(
    "div",
    { class: "chart" },
    h(
      "div",
      { class: "chart__head" },
      h("strong", { class: "chart__title" }, block.title || "Chart"),
      h(
        "div",
        { class: "chart__actions" },
        h(
          "button",
          { class: "chart__copy", title: "Copy chart as image", onClick: () => copyChartImage(root) },
          "⧉ copy image",
        ),
        h("span", { class: "chart__hint" }, hint),
      ),
    ),
    view.altView ? Table(table) : block.chart === "hbar" ? HBars(block.labels, values) : VBars(block.labels, values),
  );
  return root;
}
