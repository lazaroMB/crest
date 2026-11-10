// Picks the component of a block kind, and which column width it gets.

import { ChartBlock } from "./chart-block.js";
import { ComputedBlock } from "./computed-block.js";
import { DiffBlock } from "./diff-block.js";
import { ErrorBlock } from "./error-block.js";
import { StageBlock } from "./stage-block.js";
import { TableBlock } from "./table-block.js";
import { TextBlock } from "./text-block.js";
import { ThinkingBlock } from "./thinking-block.js";
import { ToolBlock } from "./tool-block.js";
import { UserBlock } from "./user-block.js";

const COMPONENTS = {
  user: UserBlock,
  text: TextBlock,
  thinking: ThinkingBlock,
  tool: ToolBlock,
  diff: DiffBlock,
  chart: ChartBlock,
  table: TableBlock,
  stage: StageBlock,
  error: ErrorBlock,
  computed: ComputedBlock,
};

/** Rich blocks take the wide column (1100px); prose stays at 760px. */
const WIDE = new Set(["diff", "chart", "table", "computed"]);

export const isWide = (block) => WIDE.has(block.kind);

/** `view`: { expanded, altView, toggle } */
export function renderBlock(block, view) {
  return COMPONENTS[block.kind](block, view);
}
