# Proposal: the discovery flow, a governed replacement for `/research`

Status: **proposal, nothing built**, October 2026. Companion to
[research-flow-as-is.md](research-flow-as-is.md) (what exists),
[research-flow-review.md](research-flow-review.md) (what is wrong with it) and
[graph-vs-state-machine.md](graph-vs-state-machine.md) (why the engine changes). Roadmap item 5 in
[vision.md](../vision.md) (pipelines in Lua) should run on this engine later.

## 1. Why replace the research flow

The current flow is a **pipeline of nodes** whose state is *which node ran last*. That is the
root of most findings of the review: reruns duplicate or renumber records (H2, H3, M6) because
nothing ties a record to the step that produced it; producer and reviewer disagree on what a valid
artifact is (M2, M3) because the check lives at the end; a cancelled spike cannot be resumed (H1)
because the flow records the answer, not the fact; and an approval is a "yes" that records neither
who gave it nor what version of the evidence it was given on.

A governed, event-sourced discovery attacks exactly these problems. With a strong model it does
not fail: the agent gathers enough context and does not invent. What it costs is **tokens**, and
what the corporate variants of the idea carry is ceremony: tickets, Jira, Confluence, executive
summaries for directors. This proposal takes the engine and drops the ceremony.

What is worth taking:

1. **The deliverable is a document with a section contract; the state is which sections hold a
   verdict.**
2. **An append-only event log is the source of truth.** State, gates and the open section are
   replayed from it; gates compare sequence numbers.
3. **Audit and verdict are separate steps**, and every verdict carries who gave it.
4. **Reopening cascades**: a changed section drops everything after it, records included.
5. **Claims carry verbatim spans** checked against the stored text of the source. The causal
   diagram is computed, not drawn.
6. **Frozen criteria for spikes**: hypothesis and pass criteria before the run.
7. **Human verdicts become goldens**; a judge from another model family measures the evaluator.

What is not: editor hooks (crest is the runtime), Python, the OKF bundle, tickets and a ticket
queue, Jira/Confluence/Feature Ledger as mandatory sources, a role manifest file, a worktree per
discovery, the executive summary as a required artifact.

## 2. Two levels, one engine

| | `/research <problem>` | `/research --deep <problem>` |
| --- | --- | --- |
| For | a bug, a refactor, a feature in this repo | a design question, a performance or scaling problem, a choice of algorithm or library, anything where the answer is in the literature |
| Sources | the repository, its history, library docs, a handful of web pages | the repository **and the literature**: papers, books, standards, reference docs, source of other projects |
| Sections | 6 | 10 |
| Records required | evidence with spans | evidence, claims, structural map, computed causal diagram, risks, scored spikes |
| Verdicts | you, at two gates; the rest by audit only | you on framing and `accept`; an evaluator in between, after a clean audit |
| Cost | comparable to today's fast flow | high by design; shown per section, capped by `app.opt.research.budget` |

Same event log, same audits, same document format. The deep level adds sections, records and the
literature search; it does not change the rules.

## 3. The discovery in one paragraph

A **discovery** is a Markdown document in `.crest/specs/NNNN-slug/discovery.md`, written one
section at a time, in order. A section opens only when its gate is satisfied, is written by the
model from confirmed records, is **audited** (format, citations, spans, numbers) and then receives
a **verdict** with provenance. Every write, audit, verdict, record check, source fetch and spike
score is an event in SQLite; the document, the status line and the gates are projections of that
log. `accept` freezes the discovery and derives `spec.md`, `plan.json` and `review.md` from it, so
`/implement` and `/simulate` are unchanged.

## 4. Sections

| # | Phase | Section | research | deep | Written from | Gate to open | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | framing | `Goal` | ● | ● | the request restated | — | you |
| 2 | framing | `Background` | ● | ● | repo map, history, what the literature already says | — | you |
| 3 | framing | `Constraints` | ● | ● | limits found in repo, docs and the request | — | you (deep); audit only (research) |
| 4 | framing | `Scope` (`### In` / `### Out`) | ● | ● | the request, the repo map | — | you |
| 5 | problem | `Findings` | ● | ● | confirmed evidence; in deep also claims and the structural map | deep: structural map, ≥ 1 claim and the causal diagram confirmed **after** the `Scope` verdict | audit only (research); evaluator (deep) |
| 6 | problem | `Current system` | | ● | structural map, causal diagram | same | evaluator |
| 7 | synthesis | `Desired state` | | ● | hypotheses, constraints | — | evaluator |
| 8 | synthesis | `Gap` | | ● | 6 and 7 | — | evaluator |
| 9 | synthesis | `Breakdown` | ● | ● | measurable problem statements (Definition's judge) | — | you (research gate 2); evaluator (deep) |
| 10 | solution | `Solution proposal` | ● | ● | decision matrix; in deep also scored spikes and risks | deep: `risks` confirmed and ≥ 1 spike `satisfactory` **after** the `Breakdown` verdict | you (research); evaluator (deep) |
| | | `accept` | ● | ● | | | you, always |

There is no `Guardrails` section: runtime rules live in `init.lua`. The problem space is two
plain sections rather than a container. The format contract per section (prose, bullets, numbered
list, one Mermaid block) is kept because it is what makes the audit deterministic.

## 5. Sources: the research happens on the web

This is the part a deep, multi-source reconnaissance needs most, and the part corporate tooling
anchors in tickets and wikis. crest anchors it in the open web and the literature.

### 5.1 Kinds of source

`SourceRef` today knows files, commits, URLs and commands. The discovery adds:

| Kind | Identifier | Found through | Text obtained from |
| --- | --- | --- | --- |
| `Paper` | DOI or arXiv id; authors, year, venue | arXiv API, OpenAlex, Crossref, Semantic Scholar | the open-access PDF when there is one, else the abstract and landing page |
| `Book` | ISBN or Open Library id; authors, year, publisher | Open Library, Google Books (search only) | the page the user points to, a public chapter, or the user's own PDF (`/research add @book.pdf`) |
| `Standard` | RFC number, ISO/W3C id | the web search | the HTML text |
| `Docs` | URL of a reference manual with a version | the web search, the repo's `Cargo.lock`/lockfile versions | the HTML text |
| `Repo` | `owner/name@rev` plus a path | GitHub search or a URL | the raw file |
| `Article` | URL, author, date | the web search | the HTML text |

All of them are `SourceRef` variants in `domain`, cited as `[S7]` in the document, rendered in the
`Sources` block and in `## Sources` as a reference list (author, year, title, venue, identifier,
URL, date accessed). An optional `sources.csl.json` export (CSL-JSON) lets the user drop them into
a reference manager; nothing in crest depends on it.

### 5.2 Scholar search is a backend of the web port

`WebPort` gains `search_scholar(query, filters) -> Vec<ScholarHit>` beside `search`. Backends,
all free and keyless unless noted, chosen in `app.opt.web.scholar`:

- **OpenAlex**: works, authors, venues, citation counts, open-access location. Default.
- **arXiv API**: Atom feed, preprints. Default for computer science questions.
- **Crossref**: DOI metadata, reference lists.
- **Semantic Scholar**: abstracts, citation graph, TLDRs (rate-limited without a key).
- **Open Library**: books by subject, title, author, ISBN.

The explorer agent gets two new read-only tools, `scholar_search` and `source_fetch`, and a
prompt that tells it what counts as a source for the question (a measured result, a proof, a
reference implementation, an authoritative manual) and what does not (a blog restating a paper,
a forum answer without a reference). The agent decides what to read; the engine records what was
read, from where, and when.

### 5.3 Stored text, not just a URL

Today the evidence keeps the URL. The discovery keeps the **text it read**: every `source_fetch`
stores the extracted text, its hash, the final URL after redirects and the fetch time in a
`sources` table of `sessions.db`, and writes a copy under
`.crest/specs/NNNN-slug/sources/S07-<slug>.txt`. Three things depend on it:

- **span checks** (5.4) run against the stored text, so a page that changes or disappears does
  not break the audit;
- **packets** for writers and auditors carry excerpts around the cited spans, not whole pages,
  which is where most of the token cost goes;
- `/open @.crest/specs/0007-x/sources/S07-….txt` lets you read what the model read.

### 5.4 PDFs

The web adapter refuses `application/pdf` today, so it cannot read a paper. The deep level needs
text extraction (not rendering: the modal roadmap in
[rich-content-modals.md](rich-content-modals.md) covers rendering with `hayro`). Options, pure
Rust, to decide at phase 3:

- `pdf-extract` (MIT): text with layout heuristics; good enough for most papers, weak on
  two-column layouts.
- `lopdf` plus a small text-ordering pass of our own: more work, more control.
- `hayro`'s text API, if it exposes one by then, to share one dependency with the viewer.

Scanned PDFs without a text layer are out of scope; the audit reports the source as unreadable
and the claim stays an open question.

### 5.5 Claims with verbatim spans

A `Claim` is `origin → destination`, one of four canonical directions, a stated sign, a verbatim
`span` and the source it comes from. `check_claim` (pure) requires the span to be a substring of
the stored text of that source, after whitespace normalisation. Claims with an empty span are
**open questions**: listed, never drawn. Sign comes from the direction; a stated sign that
disagrees is a rejection. A read-only checker agent then judges whether the span *supports* the
claim, which the substring test cannot.

`cld(claims)` (pure) builds the graph, finds cycles, labels loops reinforcing or balancing and
renders Mermaid. `Risks` is written per loop. Both are deep-only.

### 5.6 Rules of evidence the audit enforces

- Every factual sentence in `Findings`, `Current system` and `Background` cites a confirmed source.
- A number in the prose comes from a source or from the user; otherwise it is an audit error.
- A `Paper` cited for a result must have been *fetched*, not only found: a hit from the scholar
  search without stored text can be cited for its existence, not for what it says.
- Two sources that resolve to the same DOI or the same normalised URL are one source.
- The date accessed is recorded; the audit warns when the repo's lockfile pins a version other
  than the one the cited docs describe.

## 6. The engine

### 6.1 The event log is the state

```rust
// crates/domain/src/discovery/event.rs (pure)
pub enum DiscoveryEvent {
    Started { problem: String, level: Level },
    SourceFetched { source: SourceId, hash: [u8; 32], url: String, at: u64 },
    SectionWritten { section: Section, inputs: Vec<u64>, hash: [u8; 32] },
    Audited { section: Section, errors: u32, warnings: u32, isolated: bool, by: Actor },
    Verdict { section: Section, choice: Verdict, by: Actor, note: Option<String> },
    RecordChecked { record: RecordRef, verdict: RecordVerdict, by: Actor },
    SpikeStarted { spike: SpikeId, worktree: String, branch: String },   // fixes H1
    SpikeScored { spike: SpikeId, score: SpikeScore },
    Reopened { section: Section, reason: String },
    Accepted { by: Actor },
    Refused { what: String, why: String },
}

pub enum Actor { Human(String), Model(String), Process(&'static str) }
pub enum Verdict { ApproveAsIs, ApproveWithNotes, RejectAndRevise }
```

`replay(events) -> DiscoveryState` yields the phase, the open section, each verdict with its
`seq`, the confirmed records with theirs, the sources, the spikes, and
`gate_missing(section) -> Vec<String>`. Gates compare sequences: a record confirmed with
`seq < seq(Verdict(Scope))` does not count once `Scope` is reapproved, and `Reopened { Scope }`
hides every later verdict and record without deleting anything. This single rule removes H2, H3
and M6.

graph-flow is not used here: it models *which task runs next*; the discovery needs *which facts
are established, from which inputs, by whom*. The implementation flow keeps graph-flow.

### 6.2 The model never writes the file

The section writer returns text; the engine decides whether to append it. So the write lock is
trivial and lives in `application`: only the open section is written, `headings == verdicts + 1`
holds by construction, and the discovery folder is added to the workspace's **sensitive paths**
while the discovery is open so no chat tool or plugin can touch it.

### 6.3 Audit, then verdict

**Audit** writes `audits/NNNN-<section>.md` from two layers: deterministic checks in
`domain::discovery::contract` (format per section, citations resolve, spans found, numbers
sourced, no solution language in the problem phase) and an **isolated reviewer**, the clean-context
read-only subagent the Critical review already uses, with the packet as its only context. The
writer's retry loop calls the same deterministic functions, so producer and audit cannot disagree.

**Verdict** is a gate in the chat with three choices mapped onto crest's existing answers:

| You type | Verdict | Effect |
| --- | --- | --- |
| `yes`, `ok`, empty line… | `ApproveAsIs` | next section opens |
| `yes, but …` | `ApproveWithNotes` | approved, note recorded, no rerun |
| anything else | `RejectAndRevise` | rewritten with your text, audited, asked again |
| `/reopen <section>` | `Reopened` | cascade, shown before it happens |

Provenance on every verdict: `human:<git user>`, `model:<id>`, or `process:user-text` when you
pasted your own text for a framing section (`/research import @notes.md`), which is copied
untouched.

In **deep**, sections 5 to 10 get their verdict from an **evaluator**: read-only, clean context,
`plan` role, sees the packet and the audit, answers one of the three choices with a reason. It may
approve only after an audit with zero errors. You still give every framing verdict and `accept`.

### 6.4 Spikes

The Prototype node becomes the spike runner: `SpikeStarted` is appended **before**
`git worktree add` so a resume cleans up first (H1); hypothesis and `k of n` criteria are an
immutable record the runner cannot change; the orchestrator scores; only `satisfactory` counts
toward the `Solution proposal` gate. Experiments are diagnostic spikes allowed in the problem
phase; `/reopen findings` is how you ask to retest (H5).

### 6.5 Roles

No manifest file: `app.models.roles` already exists.

| Work | Role | Policy |
| --- | --- | --- |
| section writer, record writer | `plan` | writes through the engine |
| explorer (repo, web, scholar) | `explore` | read-only agent, `scholar_search`, `source_fetch` |
| claim checker, auditor | `explore` | read-only, clean context, packet only |
| evaluator | `plan` | read-only, clean context, packet plus audit |
| spike runner | `main` | the chat agent, your approvals |
| judge (evals) | new `judge` | a different family through the provider router, e.g. an OpenCode model when the evaluator is Claude |

### 6.6 Status is the interface

`/research` alone prints the state, the phase, the open section, what a closed gate is missing and
the one legal next step. `/stage`, `/back`, `/skip`, `/retry`, `/abort`, `/aside`, `/flow` keep
working; `/back <section>` is an alias of `/reopen`. A **Discovery block** renders the document as
it grows with a verdict badge per section.

## 7. Token cost

A naive governed discovery is expensive because every worker re-reads the world. The engine
keeps the cost down without changing what the model gets to know:

- **Packets, not transcripts.** A writer sees the confirmed records it may cite and the previous
  sections; an auditor sees one section, its contract and the excerpts around its citations. No
  one sees the event log or other sections' audits.
- **Stored text, excerpted.** A source is fetched once; writers get the span and a window around
  it, not the page.
- **Cheap roles where judgement is mechanical.** Fetching, span checking and the deterministic
  audit run on `explore`; `plan` is reserved for writing and evaluating.
- **Prompt caching** on the section contract and the records, as the chat already does for the
  system prompt and the tool definitions.
- **A budget per discovery** (`app.opt.research.budget`, tokens or money) that turns into a
  question at the next gate when crossed, as the spending cap does today, and a cost line per
  section as today's stage usage notice.

## 8. What it writes

```text
.crest/specs/0007-session-cache/
  discovery.md          the governed document (6 or 10 sections, one verdict each)
  sources/              S01-….txt … the text read, one file per source
  sources.json          the reference list; sources.csl.json on request
  records/
    claims/             0001.md … (origin, destination, direction, sign, span, source)
    structure.md  variables.md  cld.md  risks.md     (deep)
    spikes/             S01.md … (frozen criteria, facts, result, score)
  audits/               0001-goal.md …
  spec.md  plan.json  plan.md  review.md  evidence/   ← derived on accept, as today
```

`spec.md` and `plan.json` are derived from the accepted discovery, so the implementation flow and
`/simulate` read what they read today. The event log stays in SQLite; `journal.md` is its
rendering.

## 9. Where it lives in the code

| Crate | New | Reused |
| --- | --- | --- |
| `domain` | `discovery/{event, replay, gates, section, contract, claim, cld, spike}.rs`; `SourceRef::{Paper, Book, Standard, Docs, Repo, Article}` | `research::{Evidence, Hypothesis, Constraint, Decision, extract_citations}` |
| `application` | `flows/discovery/{engine, packet, writers, audit, evaluator, commands, session}.rs`; ports `DiscoveryLogPort`, `SourceStorePort`; `WebPort::search_scholar`; tools `scholar_search`, `source_fetch` | the explorer and filtered agents, `SpecSink`, `Decider`, `UsageMeter`, the measurability judge, the plan validator, the clean-context reviewer, the spending cap |
| `adapter-web` | scholar backends (OpenAlex, arXiv, Crossref, Semantic Scholar, Open Library); PDF text extraction | `curl`, `html2text`, domain rules |
| `adapter-store` | `discovery_events` and `sources` tables | the database and migrations |
| `ui` | the Discovery block, verdict badges, the reference list | `Markdown`, `Mermaid`, `Table`, `Sources`, `StageHeader` |
| `bin` | wiring, `/research` routing, headless driver | everything else |

`make arch` keeps the boundaries: the domain gains no I/O; the engine talks to ports only.

## 10. Phases

1. **Domain and log** (small). Events, replay, gates, the section contract, claims, the CLD; the
   new `SourceRef` kinds; `SqliteDiscoveryLog` and the source store. Exit: scripted event
   sequences replay to the expected state; reopen hides later records; property tests on replay.
2. **`/research` at the research level** (medium). Framing with your verdicts, `Findings` and
   `Breakdown` with audit-only approval, `Solution proposal` with your verdict, `accept` and the
   derivation of `spec.md`/`plan.json`; the Discovery block; `/research` status; headless parity.
   Sources: repo, history, docs and web pages, with stored text and span checks. Exit: the
   implementation flow runs a plan derived from an accepted discovery; today's fast flow can be
   switched off.
3. **Literature** (medium). Scholar backends, `scholar_search` and `source_fetch`, PDF text
   extraction, the reference list and CSL export, the evidence rules of 5.6. Exit: a deep question
   on a fixture repo cites fetched papers whose spans verify.
4. **Deep level** (large). `Current system`, `Desired state`, `Gap`; claims checker, computed CLD,
   risks; the isolated auditor and the evaluator's verdicts; spikes with frozen criteria on the
   fixed Prototype. Exit: a deep discovery written and approved autonomously on a fixture repo with
   every citation resolving.
5. **Evals and switch-over** (small). Human verdicts as goldens, `/research evals` with the
   `judge` role, `flows/research` removed, `app.pipeline.define` on this engine.

## 11. Decisions taken here (change them before phase 1)

- **Names**: `/research` and `/research --deep`. No `/discovery`, no tickets, no ticket queue.
- **No `Guardrails` section**; no executive summary as a required artifact (`/research summary`
  can render one on demand).
- **Sources are open-web first**: scholar backends keyless by default; MCP servers (an issue
  tracker, a wiki) are optional sources like any other, never required.
- **Autonomous verdicts only in deep**; you approve framing and `accept` in both levels.
- **No worktree per discovery**; spikes keep their temporary worktree.
- **graph-flow stays for the implementation flow.**

## 12. Risks

- **Scope.** Phase 4 touches most existing nodes. Mitigation: they already produce the records;
  phase 4 wraps them.
- **Strict audits make the model loop.** Mitigation: the writer's retry gets the errors verbatim;
  warnings never block.
- **Scholar APIs change or rate-limit.** Mitigation: several backends behind one port, each
  optional; the engine records which one answered.
- **PDF extraction quality.** Two-column papers and equations extract badly. Mitigation: spans are
  whitespace-normalised; an unreadable source becomes an open question, not an error.
- **Evaluator self-agreement.** Mitigation: the `judge` role defaults to another family when a
  second provider key exists, and the evals report says so when it cannot.
