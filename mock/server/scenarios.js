// Scripted turns. A prompt picks one by its words (see pickScenario).
//
// Each scenario gets a `turn` with:
//   push(block)       add or replace a block in the timeline
//   activity(...)     the line above the prompt ("⠋ Working · grep … · 3s")
//   notice(level, s)  a status bar notice
//   spend(dollars)    add to the session cost
//   approve(request)  wait for the approval dialog; resolves to a choice
//   sleep(ms)         wait; throws Interrupted on Ctrl+C

import { ADD_FIX_DIFF, EVIDENCE, FIX_REPLY, RENAME_DIFF, SLOWEST_TESTS, WELCH, diffBlock } from "./data.js";

let seq = 0;
const id = (prefix) => `${prefix}-${Date.now().toString(36)}-${seq++}`;

async function say(turn, markdown, unverifiedNumbers) {
  const blockId = id("text");
  const words = markdown.split(/(?<=\s)/);
  let shown = "";
  for (let i = 0; i < words.length; i++) {
    shown += words[i];
    if (i % 3 === 0) {
      turn.push({ id: blockId, kind: "text", markdown: shown });
      await turn.sleep(45);
    }
  }
  turn.push({ id: blockId, kind: "text", markdown, unverifiedNumbers });
}

async function tool(turn, name, summary, ms, output) {
  const blockId = id("tool");
  turn.activity("Working", `${name} ${summary}`);
  turn.push({ id: blockId, kind: "tool", name, summary, status: "running" });
  await turn.sleep(ms);
  turn.push({ id: blockId, kind: "tool", name, summary, status: "done", output });
}

async function think(turn, summary) {
  const blockId = id("think");
  turn.activity("Working", "thinking");
  turn.push({ id: blockId, kind: "thinking", summary: "", text: "" });
  await turn.sleep(600);
  turn.push({ id: blockId, kind: "thinking", summary, text: summary });
  await turn.sleep(300);
}

/** An edit_file that waits for the approval dialog. Returns false when rejected. */
async function edit(turn, path, unified) {
  const toolId = id("tool");
  const block = (status) => ({ id: toolId, kind: "tool", name: "edit_file", summary: path, status });
  const pattern = path.replace(/[^/]+$/, "**");
  turn.push(block("awaiting approval"));
  turn.activity(null);

  const preview = diffBlock(id("diff"), path, unified);
  const choice = await turn.approve({ id: id("approval"), tool: "edit_file", subject: path, preview, pattern });

  if (choice === "reject") {
    turn.push(block("denied"));
    return false;
  }
  if (choice === "session") turn.notice("info", `edit_file ${pattern} allowed for this session`);
  if (choice === "project") turn.notice("info", "rule saved in .crest/permissions.local.toml");
  if (choice === "all-edits") {
    turn.setStatus({ mode: "auto-edit" });
    turn.notice("warning", "auto-edit: edits run without asking (Ctrl+G p to undo)");
  }
  turn.push(block("running"));
  turn.activity("Working", `edit_file ${path}`);
  await turn.sleep(300);
  turn.push(block("done"));
  turn.push(preview);
  return true;
}

async function bugfix(turn) {
  await think(turn, "The tests fail only with negative numbers; look at add first.");
  await tool(turn, "grep", '"fn add" in src/', 700, "src/math.rs:2: pub fn add(a: i64, b: i64) -> i64 {");
  await tool(turn, "read_file", "src/math.rs", 400);
  turn.spend(0.0121);
  if (!(await edit(turn, "src/math.rs", ADD_FIX_DIFF))) {
    return say(turn, "OK, I left `src/math.rs` as it was. Tell me what to change instead.");
  }
  await tool(turn, "run_command", "cargo test", 1800, "test result: ok. 24 passed; 0 failed");
  turn.push({ id: id("chart"), ...SLOWEST_TESTS });
  turn.activity("Working", "writing");
  turn.spend(0.0187);
  await say(turn, FIX_REPLY);
}

async function rename(turn) {
  await tool(turn, "grep", '"add(" in src/ tests/', 900);
  await say(turn, "`add` is used in two files; first `src/math.rs`:");
  if (!(await edit(turn, "src/math.rs", RENAME_DIFF))) {
    return say(turn, "Stopped: nothing was renamed.");
  }
  turn.spend(0.0094);
  await tool(turn, "edit_file", "tests/math.rs", 500);
  await tool(turn, "run_command", "cargo test", 1400, "test result: ok. 24 passed");
  await say(turn, "`add` is now `sum` in `src/math.rs` and `tests/math.rs`; the 24 tests pass.");
}

async function research(turn) {
  const stages = ["Intake", "Observation", "Data", "Analysis", "Hypotheses"];
  for (const [i, name] of stages.entries()) {
    const stage = `Stage ${i + 1}/${stages.length} ${name}`;
    turn.activity("Researching", `${name}: asking claude-opus-5-5`, stage);
    turn.push({ id: id("stage"), kind: "stage", index: i + 1, total: stages.length, name });
    await turn.sleep(900);
    if (name === "Observation") await tool(turn, "grep", '"fn add" in src/', 600);
    if (name === "Data") turn.push({ id: id("table"), ...EVIDENCE });
    turn.spend(0.006);
  }
  turn.activity(null);
  await say(
    turn,
    "**Gate · Hypotheses.** Three hypotheses from 3 pieces of evidence:\n\n1. The `b < 0` branch was meant for `sub` and was copied into `add`.\n2. A caller relies on the wrong result.\n3. The test suite never covered two negatives.\n\nApprove them with an empty line or `yes`, answer `no` with a note, or `/aside` to look something up first.",
  );
}

async function stats(turn) {
  await tool(turn, "show_dataset", "bench/before.csv, bench/after.csv", 900);
  turn.activity("Working", "compute: welch_t_test");
  await turn.sleep(600);
  turn.push({ id: id("computed"), ...WELCH });
  await say(turn, "Yes: the new parser is faster, by **5.45 ms** on average (p < 0.001). That is roughly a 30% cut.", 1);
}

async function failure(turn) {
  await tool(turn, "run_command", "git log v0.17.0..HEAD --oneline", 700);
  turn.activity("Working", "claude-sonnet-5-5");
  for (let i = 1; i <= 3; i++) {
    await turn.sleep(900);
    turn.notice("warning", `Anthropic overloaded (529): retry ${i}/3`);
  }
  await turn.sleep(600);
  turn.push({
    id: id("error"),
    kind: "error",
    message: "Anthropic API: overloaded (529) after 3 retries",
    hint: "try again, or /model to pick another model",
  });
  turn.notice("error", "turn failed: overloaded");
}

async function help(turn) {
  await think(turn, "This is the mock: list the scripted turns.");
  await say(
    turn,
    "This is the **crest UI mock**: every turn is scripted by the mock server. A prompt picks a script by its words:\n\n- `fix`, `bug` or `negative`: grep, read, an edit to approve, tests and a chart.\n- `rename`: an edit to approve, then two more tools.\n- `/research <problem>`: five stages with their progress and a gate.\n- `faster`, `stats` or `t-test`: a computed card with its provenance.\n- `fail` or `error`: three retries in the status bar, then an error block.\n\n`Ctrl+C` interrupts any of them.",
  );
}

export function pickScenario(prompt) {
  const p = prompt.toLowerCase();
  if (p.startsWith("/research")) return research;
  if (/rename/.test(p)) return rename;
  if (/\b(fail|error)/.test(p)) return failure;
  if (/faster|stats|t-test/.test(p)) return stats;
  if (/fix|bug|negative/.test(p)) return bugfix;
  return help;
}
