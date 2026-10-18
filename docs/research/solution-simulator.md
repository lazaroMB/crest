# Research: an isometric "motherboard" view and an executable simulation of each solution

Status: research and proposal, not implemented. October 2026.

## Goal

crest's research flow ends in a Spec (problem, hypotheses, black-box diagram, functional spec with
sequence and state diagrams, technical spec, constraints, test protocol) **before** anything is
implemented. The idea is to turn each solution into:

1. **A board**: an isometric, circuit-board-like view of the proposed system. Components are chips,
   connections are traces, data stores and queues are visible parts.
2. **A simulation**: inputs enter the board, travel along traces, components change state, and
   different inputs drive different paths and states. Scenarios carry assertions, so the run either
   passes or shows exactly where the design breaks.

Developers use it to validate a solution before deciding to implement it, and to share an
easy-to-understand picture of what they are about to build.

## Reference: pgrun.dev/postgres-internals

The raw page shows the reference is a **three.js 0.160** scene built with the "ISO-GLOW" template:

- An orthographic camera at an isometric angle (30°, with an "Iso view" toggle back to perspective);
  `MeshStandardMaterial` parts, edge outlines (`EdgesGeometry` / `LineSegments2`), post-processing
  for glow, and glTF models.
- The scene models Postgres internals: postmaster, backends, shared buffers, WAL buffer, walwriter,
  walsender, checkpointer, vacuum, heap pages and tuples, locks, crash and recovery.
- Interaction: click any part, orbit, zoom and pan, scenario tabs with play/stop/reset, keyboard
  toggles, light and dark theme.
- The scene is hand-authored per demo, it does not check anything.

## Prior art

Nothing found combines the three ingredients. Each of them exists separately:

| Ingredient | Best references | What to take |
| --- | --- | --- |
| Isometric look | Cloudcraft, Isoflow / FossFLOW (JSON tile-grid model), pgrun / ISO-GLOW | Chips on a grid, shaded faces, glowing traces |
| Step-through over one model | RaftScope (raft.github.io), IcePanel Flows, Ilograph sequence perspectives, Structurizr dynamic views | Timeline with play/step/scrub over a deterministic run; Ilograph's `steps` grammar for scenarios |
| Checked behaviour | P language (communicating state machines + monitors), Quint / TLA+ (invariants, ITF JSON traces), Jepsen (checkers over histories), XState / Stately (statecharts, path generation) | Semantics: P-style machines exchanging messages, DEVS-style delays, ITF-like traces, invariant checks |

System-design simulators (paperdraw.dev and similar hobby tools) show live metrics under chaos
toggles, but have no assertions and no link to a spec. Rust discrete-event libraries (NeXosim, desim,
sim/DEVS) and deterministic-simulation testing (madsim, turmoil, FoundationDB) confirm the pattern
to use: single thread, virtual clock, seeded randomness.

## Proposed model format

A custom, versioned JSON schema (`crest.sysmodel/v1`) generated from Rust types with `serde` +
`schemars`, like the rest of crest's domain types. Existing formats were rejected: SCXML and XState
have no topology, latency or queues; C4, Ilograph and Structurizr have topology but no behaviour;
TLA+, Quint and P are hard for an LLM to write reliably and need external toolchains.

```jsonc
{
  "version": "crest.sysmodel/v1",
  "spec_ref": { "solution": "0007-session-cache", "evidence": ["E3"] },
  "components": [{
    "id": "api", "kind": "service",            // client|service|queue|db|cache|lb|external|timer
    "ports": [{ "id": "in", "dir": "in", "accepts": ["Req"] },
              { "id": "db", "dir": "out", "emits": ["Query"] }],
    "capacity": { "concurrency": 4, "queue": 16, "on_full": "reject" },
    "latency": { "dist": "uniform", "ms": [5, 20] },
    "vars": { "inflight": 0 },
    "machine": {
      "initial": "idle", "states": ["idle", "busy", "degraded"],
      "transitions": [{ "from": "idle", "on": "Req", "guard": "inflight < 4", "to": "busy",
                        "effects": [{ "set": "inflight", "expr": "inflight + 1" },
                                    { "emit": "Query", "port": "db", "after_ms": 2 }] }]
    },
    "layout": { "col": 3, "row": 1 }           // optional hint; auto-layout otherwise
  }],
  "connections": [{ "id": "c1", "from": "api.db", "to": "pg.in", "latency_ms": 1 }],
  "invariants": ["pg.conns <= 20"],
  "scenarios": [{
    "id": "cache_miss_storm", "seed": 42, "until_ms": 1000,
    "inject": [{ "at_ms": 0, "to": "client.out", "msg": "Req", "repeat": { "every_ms": 10, "count": 50 } }],
    "faults": [{ "at_ms": 300, "kill": "cache", "for_ms": 100 }],
    "expect": [
      { "always": "api.inflight <= 4" },
      { "eventually": "client.state == 'done'", "within_ms": 200 },
      { "path": ["client", "lb", "api", "pg", "api", "client"] }
    ],
    "covers": ["H1", "T3"]                      // hypotheses and test-protocol items of the Spec
  }]
}
```

Keeping it trustworthy:

- **Schema + lint** (pure domain): references resolve, message types match across connections, every
  state is reachable, every emitted message has a handler, guards use declared variables.
- **A tiny expression language** for guards, effects and assertions (integers, comparisons, `&&`,
  `||`, `!`, `component.var`, `component.state`) with a hand-written parser: no `eval`, and the HTML
  export can interpret it identically.
- **Deterministic engine**: event queue ordered by `(time, seq)`, integer microseconds, seeded RNG
  per scenario. Same model + seed → byte-identical trace (golden tests).
- **Assertions checked by the simulator**: `always`, `never`, `eventually … within`, counts, latency
  percentiles, expected path. A failure comes with a counterexample (event index + state snapshot).
- **Traceability**: scenarios list the hypotheses and tests they cover; each run becomes `Evidence`
  (source: model hash + seed + trace hash); lint warns about test-protocol items without a scenario.

## Rendering

### In crest (native GPUI)

Two options:

1. **2.5D vector isometric (recommended for v1).** The isometric look is a projection, not real 3D:
   `sx = (x − y)·tw/2`, `sy = (x + y)·th/2 − z·zh`. Chips are extruded boxes drawn as three shaded
   polygons; traces are Manhattan polylines routed on the grid (dark wide stroke + thin bright one for
   the PCB look); packets are glowing dots interpolated along a trace as a pure function of simulation
   time, so scrubbing is free. Painter's algorithm by `x + y + z` is enough for non-overlapping grid
   boxes. GPUI's `PathBuilder` (fill, stroke, dashes, béziers, transforms) covers it, and crest
   already has a canvas with pan / cursor-anchored zoom / fit in `crates/ui/src/blocks/mermaid/paint.rs`.
   Cache tessellated paths per component; only packets and state colours change per frame.
2. **Real 3D (later).** Lighting, glow and orbiting like pgrun need the wgpu offscreen renderer
   planned for the 3D viewer (phase 3 of [rich-content modals](rich-content-modals.md)): render to a
   texture, read back, show as a `RenderImage`. Same scene description, different renderer.

Screen layout: board in the centre; bottom timeline with play/pause, speed, step-to-next-event and
red markers for failed assertions; right panel with the selected component's state, variables, queue
and its machine as a Mermaid state diagram (already rendered by crest) with the current state lit;
top scenario picker, seed and pass/fail per assertion. Clicking an input chip or choosing a scenario
re-runs the simulation.

### Sharing

- **Self-contained HTML** (no CDN): the model JSON, the recorded traces and a small replay script.
  The static board is SVG generated by Rust with the same projection code; JS animates only the
  packets on a canvas overlay. v1 replays recorded traces (exact, no second simulator to keep in sync);
  v2 inlines the Rust simulator compiled to WASM so the reader can change inputs and re-run.
- **GIF / video**: render frames from the same scene with `tiny-skia`, encode with `gif`/`gifski`, or
  pipe to `ffmpeg` when present.

## Architecture in crest

| Layer | New pieces |
| --- | --- |
| `crates/domain` | `sysmodel/`: model types + schema, expression parser, lint, deterministic simulator, trace with checkpoints for fast seeking, assertion checker, grid auto-layout and trace routing, renderer-agnostic isometric scene |
| `crates/app` | Research node **Simulation** between Spec and Plan: generate the model from the Spec's diagrams and test protocol, validate, run every scenario, repair loop (max 2 rounds), record Evidence, gate for the user; `SimExport` port; a use case to re-run with new inputs or seeds |
| `crates/adapters` | HTML exporter, GIF exporter, persistence of `sysmodel.json` and traces in the Spec folder |
| `crates/ui` | `blocks/sysboard/`: isometric canvas, timeline, state inspector, scenario picker; a compact preview block in the timeline that opens the full view in a modal |

## Risks

- **A plausible but wrong model.** The simulation proves internal consistency, not fidelity. Every
  component must cite a Spec element or Evidence id; lint compares the model with the black-box
  diagram; each sequence diagram becomes an expected `path` assertion; the user reviews at the gate.
- **Assertion gaming.** The model must not weaken its own checks to pass: derive assertions from the
  test protocol first, in a separate call, freeze them, and show any loosening between repair rounds.
- **False confidence.** Label it a behavioural sketch; latencies are guesses unless backed by evidence
  (show provenance, allow sweeps across seeds and latency ranges).
- **Semantics creep.** v1: flat machines, timers as delayed self-messages, FIFO queues. Hierarchical
  and parallel states, retries and load generators later.
- **Two engines drifting.** Replay-only HTML in v1, WASM later.

## MVP and later

MVP: domain model + lint + expression language + deterministic simulator with `always`,
`eventually … within`, `count` and `path`; the Simulation research node with Evidence and a gate;
the 2.5D board with packets, play/pause/scrub, click-to-inspect and a scenario picker; HTML export
replaying recorded traces.

Later: latency percentiles and load generators, fault toggles in the UI, WASM re-simulation in the
HTML, GIF/MP4 export, bounded exhaustive exploration of small models ("P-lite" model checking),
real-3D rendering, exporters to Quint or Structurizr.

## Sources

pgrun.dev/postgres-internals (raw page), Cloudcraft, Isoflow / FossFLOW, IcePanel Flows, Ilograph
specs and sequence perspectives, Structurizr DSL, paperdraw.dev, RaftScope, The Secret Lives of Data,
Jepsen, Stately Studio and Inspector, SCXML, Quint and the ITF trace format, P language, Alloy,
NeXosim, desim, sim (DEVS), madsim, turmoil, tiny-skia, gifski.
