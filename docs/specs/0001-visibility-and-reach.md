# Spec 0001: durable notices, a short-step log, and outline-only arrows

Status: **implemented** in `crates/ui` only (no CLI or headless change): sections 1 and 2 as the information mark (`crates/ui/src/info.rs`), section 3 as the outline arrows. Source: Specky3 epic `CREST-UX` (`Weaknesses of the crest UI against the
standards for agentic interfaces`), node `#18` (Solution proposal) and spike `S01`
(`.aos/tree/CREST-UX/nodes/17.md` and `18.md`). Prototyped, not yet built, in `mock/` (see
`mock/src/visibility.js`). This spec is the port of that prototype into `crates/ui`.

The requirements this implements, in plain terms and without crest's source, are in
[R01-visibility-and-reach.md](../requirements/R01-visibility-and-reach.md).

## Problem

Three timings in `crates/ui` were measured against the causal claims in node `#9`:

1. A status-bar notice clears after `NOTICE_TTL` (`crates/ui/src/status.rs:23`, 6 seconds). A longer
   notice lifetime lowers the unseen-error count (claim 0001); a lower count raises shared control
   (claim 0004). An error that scrolls out of the timeline and off the bar in the same minute is gone.
2. The activity line waits `SHOW_AFTER` (`crates/ui/src/activity.rs:36`, 150 ms) before it draws. A
   longer activity delay lowers shared control (claim 0002): a step that finishes under 150 ms never
   shows, so the person cannot see or redirect it.
3. `Up`/`Down` map to `"block.prev"`/`"block.next"` (`crates/ui/src/timeline/view.rs:345-346`)
   regardless of `outline_open` (`view.rs:77`); only `Escape` reads that flag (`view.rs:333-338`).
   Greater question reach raises shared control (claim 0003). Today the outline cannot be driven with
   the keyboard; only its mouse click (`choose_outline`, `view.rs:498`) selects a question.

## Non-goals

Carried over from node `#18`'s Future work, unchanged by this spec:

- No change to `NOTICE_TTL` or `SHOW_AFTER`. Both stay as they are; risks of raising either are in
  `.aos/tree/CREST-UX/nodes/16.md`.
- No resize of the 240 px chats panel (`crates/ui/src/chats.rs:165`) or the 280 px tree
  (`crates/ui/src/tree.rs:49`).
- No CLI or headless change, no telemetry, no user study of scientists and engineers.
- Scroll-driven outline selection and "click a mark that is already on screen" are mock-only
  refinements (see `mock/src/components/timeline/timeline.js`); this spec ports the three Spike S01
  behaviors, not the extra scrollspy work.

## Proposed approach

Port the three helpers Spike S01 met in `mock/src/visibility.js` into `crates/ui`, each keyed to one
claim and avoiding the side effect named for it in node `#16`:

### 1. Dismissible notices (claims 0001, 0004 / risk: notice lifetime)

Add a persistent notice list beside `StatusState`'s transient one (`crates/ui/src/status.rs`). The
status bar still clears a notice after `NOTICE_TTL`; an error or warning notice also goes into the
list and stays there until the person dismisses it. `NOTICE_TTL` itself is untouched, so the risk of
a notice crowding the one-line bar (node `#16`, "Notice lifetime to shared control") does not apply.

- Equivalent of `mock/src/visibility.js`'s `retainNotice(notice, elapsedMs, persistent)`: a kept
  notice is `persistent = true` and is never aged out by time; a transient one still expires at
  `NOTICE_TTL`.
- Surfaced the way the mock settled on after this spec's own review (toast 3 s, then parked behind a
  mark), not the first mock pass (an inline list above the status bar): see "UI placement" below.

### 2. Short-step log (claim 0002 / risk: activity delay)

Add a log of finished steps beside `ActivityState` (`crates/ui/src/activity.rs`). `SHOW_AFTER` still
gates the live line, so a step shorter than it never flashes the line (the reason the constant exists,
per its own doc comment at `activity.rs:34-35`). That same short step is appended to the log instead,
so the person can still see it and, if it is still running, interrupt it.

- Equivalent of `recordStep(steps, step, logShort)`: every finished step is recorded; one under
  `SHOW_AFTER` is marked short.

### 3. Outline-only arrows (claim 0003 / risk: question reach)

Change `on_key` (`view.rs:324`) so `"up"`/`"down"` consult `self.outline_open` before resolving to an
action, instead of always resolving to `"block.prev"`/`"block.next"`:

- `outline_open == true`: `"up"` -> an outline-prev action, `"down"` -> an outline-next action,
  moving `choose_outline`'s selection (`view.rs:498`) the way a mouse click on an outline row already
  does.
- `outline_open == false`: unchanged, `"block.prev"` / `"block.next"`.
- `Escape` still closes the outline first (`view.rs:333-338`); once closed, arrows fall back to block
  navigation on the next key, so the risk named in node `#16` ("outline takes arrows only while it is
  open") holds after this change, not just before it.
- Equivalent of `onKey(key, outlineOpen)` in the mock.

## UI placement

The mock's first pass put the kept-notice list and the short-step log as two rows pinned above the
status bar and the prompt. Asked whether that was legible, it was not: a plain information line next
to the prompt read as another status message, not as something to open. The mock was changed to a
single information mark (`info-corner`, bottom-right of the window) that: shows one entry as a toast
for 3 seconds when it is added, then holds every kept notice and short step behind the mark until
dismissed. `crates/ui` should follow the corner placement, not the pinned-rows one; `mock/src/
components/info-corner/` is the reference, not `mock/src/components/status-bar/`'s first revision.

## Acceptance

- A notice with `NoticeLevel::Error` or `NoticeLevel::Warning` still clears from the status bar after
  `NOTICE_TTL`, and is still readable afterward from the information mark, until dismissed.
- A step that finishes in under `SHOW_AFTER` never appears on the live activity line, and does appear
  in the step log / information mark.
- With the outline closed, `Up`/`Down` move timeline blocks exactly as today (no behavior change,
  covered by the existing `view.rs` tests).
- With the outline open, `Up`/`Down` move the outline's selection; `Escape` still closes it; closing it
  returns arrow keys to block navigation.
- `cargo test -p ui` passes, including a new case for the outline-open arrow mapping alongside
  `the_outline_menu_closes_every_way` and `the_outline_scrolls_to_the_current_question`
  (`crates/ui/src/timeline/view.rs`).

## Traceability

| Behavior | Claim | Risk avoided (node `#16`) | Spike criterion (node `#17`) |
| --- | --- | --- | --- |
| Dismissible notices | 0001, 0004 | Notice lifetime to shared control | 1 |
| Short-step log | 0002 | Activity delay to shared control | 2 |
| Outline-only arrows | 0003 | Question reach to shared control | 3 |
