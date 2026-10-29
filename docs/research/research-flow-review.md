# Review: the research flow (`/research`)

Status: **findings, nothing fixed yet**, October 2026. Code review of the 14-node research
workflow at crest 0.9.0 (commit `b66f995`). How the flow is meant to work:
[workflows.md](../workflows.md#research-workflow).

## Scope and method

- The frame was read in full: `crates/app/src/flows/research/{task,flow,gate,session,commands,builder,node}.rs`,
  the drivers in `crates/bin/src/research.rs`, `crates/bin/src/headless/research.rs` and the routing in
  `crates/bin/src/gui.rs`, plus the graph-flow 0.8.0 runner they sit on.
- The 14 nodes under `crates/app/src/flows/research/nodes/` were reviewed in three groups
  (1–5, 6–10, 11–14) against the node contract in `mod.rs` and the behaviour documented in
  `workflows.md`. Every finding marked **verified** was confirmed by reading the code path, not inferred.
- Line numbers refer to commit `b66f995`.

## What is sound

- **The frame.** One checkpoint per finished node; gates (confirm and question) and their reruns with
  `user_reply`; two automatic retries of transient errors; `Failed` kept at the last checkpoint and
  `/retry` rerunning the node with the user's pending answer (graph-flow does not save the session when a
  task errors, so the reply survives); resume after a restart or a cancelled run; `/back`, `/skip` and
  `/abort`; the 3-go-backs-per-stage limit turning into a question. All of it is covered by
  `flows/research/tests.rs`, `task.rs` and `session.rs` tests.
- **Back targets** of every node match the table in `workflows.md`.
- **Model output handling.** No panics found: JSON is located with byte-safe `find`/`rfind`, ids are
  parsed through `FromStr`, slices are guarded, scores reject NaN through the range check. Invalid
  answers get one retry with the error, then a non-transient `NodeError::Failed`.
- **Spec writes** go through `FsSpecSink`, which refuses `/`, `.` and `..` in folder names and uses
  `WorkspacePath`: nothing is written outside `.crest/specs`.
- **Plan cycle detection** is Kahn's algorithm (O(V+E)) and catches self-loops.
- **The read-only agent** (`agents.rs`) has exactly the tools the docs list, the read-only command
  policy, denied approvals and the 30-call limit.

## Findings

Severity: **high** = wrong result or an unrecoverable state for the user; **medium** = lost work, a
loop until the go-back limit, or silently discarded data; **low** = cosmetic or documentation.

### High

#### H1. Prototype leaves worktrees behind and cannot be resumed after a cancel — verified

`nodes/prototype.rs:200-203`. The cleanup (`git worktree remove --force`, `git branch -D`) runs only
after `run_spike` returns. If the step is cancelled (`Ctrl+C` in the window drops the future,
`session.rs:220`) or the process dies while the agent writes the spike, `.crest/spikes/<id>` and the
branch `crest/spike-<id>` stay forever. On resume the saved answer and the spike id (`SPIKE_KEY`) are
still there, so the same `git worktree add -b` fails with "branch already exists" and the node fails;
there is no path that removes a leftover spike before adding. `workflows.md` promises that "the cleanup
always runs".

Suggested fix: before `add`, if `SPIKE_KEY` names a spike, run its cleanup commands first (ignoring
failures); and record the spike as "created" in the flow data so a rerun knows there is something to
remove.

#### H2. Observation, Data and Research duplicate evidence on every rerun — verified

`nodes/observation.rs:277-283`, `nodes/data.rs:300-323`, `nodes/research/mod.rs:244-246, 300-305`.
All three call `add_evidence` unconditionally; the frame writes the state back as is (`task.rs:244`),
`/back` only clears the gate, the header and the go-back counters (`flow.rs:374-402`), and
`ResearchState` has no way to remove evidence. Scenario: at the Hypotheses gate the user types
`/back observation`; E1..En stay and E(n+1)..E2n are added; Data then adds up to 20 more commit records.
Analysis and Hypotheses (whose prompts include *all* evidence, unbounded) and the Spec see stale
duplicates with different ids, and `ObservationReport.evidence` points only at the new ones. Modeling's
Back to Definition deliberately replays Research, so this also happens without any user action.

Hypotheses does it right (`hypotheses.rs:221-226`: takes the old list, keeps ids and statuses of the
ones the model names, rebuilds the rest). The previous ids are already available in
`observation.report`, `data.report` and the Research data, so each node can retire its own records.

#### H3. Spec keeps drafts of problem statements that no longer exist — verified

`nodes/spec.rs:217-219`. `retain` only drops the drafts whose folders are being rewritten. When the
critical review goes back to Definition and Definition rewrites `problem_statements` wholesale
(`define/mod.rs:405`), Spec writes new `000N-…` folders but keeps the old drafts in `DRAFTS_KEY`. Plan
(`plan.rs:83`) and Review (`review/mod.rs:149`) then plan and review the dead Spec too, and its gaps can
send the flow back again.

Suggested fix: retain only the drafts whose `problem` is still in `state.problem_statements`.

#### H4. Modeling accepts `chosen: null` while viable options are scored — verified

`nodes/modeling.rs:167-172`, validation at `303-319`. The validator checks `chosen` only when it is
present. If the model scores options but declines to pick one, the node returns
`Back { Brainstorm, "every option violates some constraint" }`: a false reason, repeated until the
3-go-back question. Fix: require `chosen` when `scores` is non-empty (or `scores` empty when `chosen`
is null). Minor, same place: fractional scores such as 3.5 pass the 1..=5 check; the docs say 1-5.

#### H5. Experiments cannot be rerun once every hypothesis has a result — verified

`nodes/experiments.rs:159-171`. Only `Open | Untested` hypotheses are targeted. On `/back experiments`,
or a Back into Experiments, with every hypothesis already Supported or Refuted, the node logs "every
hypothesis already has a result" and returns `Done` with no blocks. Experiments has no gate, so the
user can never ask to retest. Fix: when `user_reply` is present (or when nothing is open), retest the
hypotheses the reply names, or all of them.

### Medium

#### M1. A hard, undocumented 30-minute limit per node

`builder.rs:19` (`DEFAULT_NODE_TIMEOUT`); `research_graph` never calls `with_node_timeout`, so it is
not configurable from the composition root. Experiments and Prototype run the chat agent with the
user's approvals: if the user walks away from an approval prompt, the task times out, the error is a
`GraphError` (not a transient `NodeError`, so no automatic retry), the flow becomes `Failed` and the
agent's tool calls and diffs are lost. Document it, raise it for the acting stages, or make it an option.

#### M2. Plan and Review disagree on what a valid plan is

Plan accepts a plan with empty `hypothesis_tests` or constraints without a `constraint_tests` entry
(`nodes/plan.rs:138-156`); Review flags both as major and goes back to Plan
(`nodes/review/checks.rs:240-263`). `task.rs::back` only logs and notices the reason; the rerun Plan node
gets no `user_reply`, so the model tends to repeat the omission until the 3-go-back question. Adding
those two checks to Plan's `check()` fixes them in its own retry loop, with the error in the prompt.
Same class: "constraint has no test → Definition" (`checks.rs:226`) reruns Definition blind.

#### M3. Spec and Review disagree on background citations

Spec accepts a background with at least one citation anywhere (`nodes/spec/render.rs:338`); Review
requires every paragraph and list item of `## 2. Background` to cite (`checks.rs:214-222`). An uncited
lead-in such as "**Previous attempts**" is a major gap that sends the flow back to Data, which cannot fix
wording. Align the two (the Spec check should be the stricter one, so the retry fixes the text).

#### M4. Data swallows transient web failures — verified

`nodes/data.rs:329-333`. Any error of `explore_web`, including rate limits and overloads (test
`a_failed_web_search_is_a_warning`), becomes a warning notice plus a journal line and the node returns
`Done`. The frame would have retried a transient error. The only way to get the documentation evidence
back is `/back data`, which triggers H2. Retry when `err.is_transient()`, or at least show it in the
Data table rather than only in the status bar.

#### M5. Experiments discards real results when the command text does not match

`nodes/experiments.rs:330-333` matches a run by `normalize(display()) == normalize(model "command")`.
`display()` (`ports/shell.rs:96-115`) single-quotes any argument with spaces or shell characters
(`sh -c 'cargo test 2>&1 | tail'`); the model's reproduction (double quotes, or the bare shell line)
does not match, so a command that really ran yields `Untested` and no evidence. Safe direction, but it
silently loses work. Match on the tool-call id or on the parsed argv instead of the rendered string.

#### M6. Definition renumbers constraints on every rerun

`nodes/define/mod.rs:404-409`. `save` clears the constraints and re-adds them from C1. After a Review
Back to Definition ("a constraint without a test"), earlier `Decision.discarded` strings ("violates
C1") and the Spec text reference stale ids. Keep the ids of constraints whose text did not change, as
Hypotheses does.

### Low

- **L1. Research limits not enforced.** `workflows.md` says "up to 5 searches and 3 pages"; the prompt
  says "up to 5 pages" and the only cap is the agent's 30 model calls (`research/mod.rs:44-52`,
  `agents.rs:46`). Also, the "30 calls" are model calls, not tool calls: one call may issue several.
- **L2. Journal overstates recorded commits.** `data.rs:396` reports `answer.commits.len()` (the prompt
  asks for 50) while `data.rs:300` keeps 20. Later stages read the journal as context.
- **L3. Decision model at gates not in `workflows.md`.** With a decider, free text the model reads as
  approval with p ≥ 0.7 moves on (`gate.rs:139`), while the gates table says any other text is a
  correction. `usage.md` mentions it; `workflows.md` does not.
- **L4. Ignoring `.crest/` renumbers *and overwrites*.** The Prototype section suggests ignoring
  `.crest/`; `FsSpecSink::folders()` honours ignore files (`sink.rs:124`), so numbering restarts at
  0001 and `write` overwrites an older `0001-<same-slug>/` (`spec.md`, `plan.json`, `review.md`). The
  docs warn about the renumbering, not the overwrite.
- **L5. Duplicate journal entry on a transient retry.** `task.rs:220` logs "User answer: …" on every
  attempt of the retry loop in `Task::run`.
- **L6. Drafts recorded only after all folders are written.** `spec.rs:201-220`: a crash between two
  Specs leaves `0004-…` on disk but unknown to the flow; the rerun writes `0005-…` (duplicate folder).
- **L7. Review prompt lists `Stage::ALL`, not the graph's stages.** `review/mod.rs:281-285, 330-333`.
  In a graph without Data (as in `review/tests.rs:160`) a `"missing_stage": "data"` answer becomes
  `Back { Data }`, which `task.rs:168` turns into a failed flow instead of a retry with the error. Only
  reduced graphs are affected.
- **L8. Repeated gaps with several Specs.** `review/mod.rs:198-203` pushes "constraint has no test"
  once per draft, so the same gap appears N times in `review.md`.
- **L9. Analysis emits empty charts.** `analysis.rs:216-226` relies on `ChartSpec::validate`, which
  accepts `labels: []` with empty series.
- **L10. `RequestKind` has no catch-all.** `intake.rs:33-45`: a `kind` outside the four values is a
  serde error; after the one retry the flow fails at node 1. `FindingKind` has `Other`.
- **L11. Measurable heuristic gaps.** `define/measurable.rs:55-170` lacks spelled-out units
  ("milliseconds", "percent", "milisegundos"); `446-458` matches outcomes with `starts_with` on broad
  stems ("error", "show"), so vague conditions can pass. Mitigated by the model's second opinion.
- **L12. Dead data.** `experiments.rs:31-33, 197` writes `KEY_PROTOCOLS`; nothing reads it, and it is
  overwritten with only the current run's targets.
- **L13. Doc nuances.** The Definition question gate accepts "ok" to keep an unmeasurable draft
  (`define/mod.rs:171-175`), not mentioned; Brainstorm goes back to Research when `OPTIONS_KEY` is
  missing (`brainstorm.rs:74-80`), not in the go-backs table.

## Missing tests worth adding

- A rerun of Observation, Data and Research (H2) asserting the evidence count.
- Spec after Definition rewrote the problem statements (H3).
- Modeling with `chosen: null` and non-empty `scores` (H4).
- Experiments rerun with every hypothesis already decided (H5).
- Prototype cancelled mid-spike, then resumed (H1).

## Suggested order

H1 to H5 first; H2, H3 and M6 share one cause (what each node must retire when it reruns) and can be
fixed with one rule in the node contract: *a node owns the records it created and replaces them on a
rerun*. Then M2 and M3 (make the producing node as strict as the review), then M1 and M4.
