// Runs the frozen checks for spike S01. Prints one line per criterion.

import { onKey, recordStep, retainNotice } from "../src/visibility.js";

const notice = { level: "error", text: "overloaded" };
const kept = retainNotice(notice, 7000, true);
const dropped = retainNotice(notice, 7000, false);
const logged = recordStep([], { name: "grep", elapsedMs: 40 }, true);
const hidden = recordStep([], { name: "grep", elapsedMs: 40 }, false);
const outlineUp = onKey("up", true);
const blocksUp = onKey("up", false);

const checks = [
  ["1", kept?.text === "overloaded" && dropped === null],
  ["2", logged.length === 1 && logged[0].elapsedMs === 40 && hidden.length === 0],
  ["3", outlineUp === "outline.prev" && blocksUp === "block.prev"],
];

let failed = 0;
for (const [id, ok] of checks) {
  console.log(`${id} ${ok ? "met" : "not-met"}`);
  if (!ok) failed += 1;
}
console.log(failed === 0 ? "3 of 3" : `${3 - failed} of 3`);
process.exit(failed === 0 ? 0 : 1);
