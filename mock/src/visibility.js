// Prototype for spike S01. Not part of crest. It tries three changes
// against the timings measured in crates/ui: a notice that stays until
// dismissed, a log of steps too short to flash the activity line, and
// arrow keys that move the question outline while that outline is open.

export const NOTICE_TTL_MS = 6000;
export const SHOW_AFTER_MS = 150;

/** Returns the notice, or null once a transient notice has expired. */
export function retainNotice(notice, elapsedMs, persistent) {
  if (!notice) return null;
  if (persistent) return notice;
  return elapsedMs >= NOTICE_TTL_MS ? null : notice;
}

/** Records a step. Short steps are kept only when the log is on. */
export function recordStep(steps, step, logShort) {
  const next = steps.slice();
  if (step.elapsedMs >= SHOW_AFTER_MS || logShort) next.push(step);
  return next;
}

/** Where an arrow key goes. The outline takes it only while open. */
export function onKey(key, outlineOpen) {
  const outline = outlineOpen && (key === "up" || key === "down");
  if (key === "up") return outline ? "outline.prev" : "block.prev";
  if (key === "down") return outline ? "outline.next" : "block.next";
  return "none";
}
