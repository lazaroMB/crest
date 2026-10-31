// The crest mark: the two peaks of the TwoCrest logo (crates/ui/src/brand.rs).
// The back peak takes the foreground color, the front one the brand coral,
// cut out of the back one by a gap in the background color.

import { svg } from "../../lib/h.js";

const BACK = "4,74 37,24 70,74";
const FRONT = "46,74 86,6 124,74";

export function Brand({ height = 14 } = {}) {
  const width = (124 / 76) * height;
  return svg(
    "svg",
    { class: "brand", width, height, viewBox: "2 2 124 76", "aria-label": "crest" },
    svg("polygon", { points: BACK, fill: "var(--foreground)" }),
    svg("polygon", { points: FRONT, fill: "none", stroke: "var(--background)", "stroke-width": 6, "stroke-linejoin": "miter" }),
    svg("polygon", { points: FRONT, fill: "#d9604a" }),
  );
}
