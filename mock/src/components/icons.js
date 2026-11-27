// Line icons, 16 px, drawn with the text color. Native crest would use
// gpui-component's IconName for the same shapes.

import { svg } from "../lib/h.js";

const PATHS = {
  panel: ["M3 4h18v16H3z", "M9 4v16"],
  plus: ["M12 5v14", "M5 12h14"],
  search: ["M10.5 4a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13z", "M15.5 15.5 20 20"],
  folder: ["M3 6h6l2 2h10v11H3z"],
  chat: ["M4 5h16v11H9l-5 4z"],
  more: ["M5 12h.01", "M12 12h.01", "M19 12h.01"],
  close: ["M6 6l12 12", "M18 6 6 18"],
  research: ["M10.5 4a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13z", "M15.5 15.5 20 20", "M8 10.5h5"],
  deep: ["M12 3 3 8l9 5 9-5z", "M3 13l9 5 9-5", "M3 18l9 5 9-5"],
  file: ["M6 3h8l4 4v14H6z", "M14 3v4h4"],
  slash: ["M15 4 9 20"],
  pin: ["M9 4h6l-1 6 3 3H7l3-3z", "M12 13v7"],
  branch: ["M6 3v12", "M6 6a3 3 0 0 0 3 3h6a3 3 0 0 1 3 3", "M18 15v6", "M6 21a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z"],
  archive: ["M3 4h18v4H3z", "M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8", "M10 12h4"],
  restore: ["M4 4v5h5", "M4.5 13a8 8 0 1 0 2.4-6.6"],
  doc: ["M6 3h8l4 4v14H6z", "M14 3v4h4", "M9 12h6", "M9 16h6"],
  split: ["M3 4h18v16H3z", "M12 4v16"],
  full: ["M3 4h18v16H3z", "M7 9h10", "M7 13h10", "M7 17h6"],
  copy: ["M9 9h11v11H9z", "M5 15H4V4h11v1"],
  image: ["M3 5h18v14H3z", "M3 16l5-5 4 4 3-3 6 6", "M15.5 9a1 1 0 1 0 0-.01"],
  export: ["M12 3v12", "M7 8l5-5 5 5", "M5 15v5h14v-5"],
  agent: ["M12 3l2 5 5 2-5 2-2 5-2-5-5-2 5-2z", "M19 16l1 2 2 1-2 1-1 2-1-2-2-1 2-1z"],
  edit: ["M4 20h4L19 9l-4-4L4 16z", "M13 7l4 4"],
  eye: ["M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z", "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z"],
  chevron: ["M6 9l6 6 6-6"],
  list: ["M9 6h12", "M9 12h12", "M9 18h12", "M4 6h.01", "M4 12h.01", "M4 18h.01"],
  quote: ["M5 7h6", "M5 12h14", "M5 17h10"],
  code: ["M9 8 5 12l4 4", "M15 8l4 4-4 4"],
  diagram: ["M3 4h7v5H3z", "M14 15h7v5h-7z", "M14 4h7v5h-7z", "M6.5 9v8.5H14", "M10 6.5h4"],
  link: ["M10 13a5 5 0 0 0 7.1 0l2-2a5 5 0 0 0-7.1-7.1l-1.2 1.2", "M14 11a5 5 0 0 0-7.1 0l-2 2a5 5 0 0 0 7.1 7.1l1.2-1.2"],
};

export function icon(name, size = 16) {
  return svg(
    "svg",
    {
      class: "icon",
      width: size,
      height: size,
      viewBox: "0 0 24 24",
      fill: "none",
      stroke: "currentColor",
      "stroke-width": 1.6,
      "stroke-linecap": "round",
      "stroke-linejoin": "round",
      "aria-hidden": "true",
    },
    (PATHS[name] ?? []).map((d) => svg("path", { d })),
  );
}
