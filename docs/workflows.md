# Workflows: research, simulation and implementation

crest ships two guided workflows that run inside the chat:

- The **research workflow** follows the stages of the [Hashlabs X-Workflow](https://www.hashlabs.com/guides/en/methodology/X-Workflow.html). It starts from a problem described in natural language and ends in a **Spec** with its implementation plan.
- The **implementation workflow** takes a Spec's `plan.json` and runs it task by task, with one commit per task.

Between the two, on demand, you can **simulate a solution** ([below](#simulating-a-solution)): crest turns the Spec into an executable model, runs its scenarios and shows the result on a 3D board, before anything is implemented.

Both are built with [graph-flow](https://crates.io/crates/graph-flow). Every step is a checkpoint in SQLite, in `~/.local/share/crest/sessions.db`. At **gates**, the workflow stops and waits for your answer in the same chat input (in the window) or on stdin (in headless mode).

While a workflow waits at a gate, **every plain line you send answers it** and does not start a normal turn, unless you step aside to ask something else ([side questions](#side-questions)). The window's `/` commands (`/model`, `/mode`, `/clear`, `/q`...) keep working, and `/stage`, `/back`, `/skip`, `/retry` and `/abort` go to the workflow (the implementation workflow first, if both are active).

### Side questions

Sometimes you need to ask or look something up before you can answer a gate. A **side chat** lets you do that without the stage running again:

- `/aside [question]` (or `/aparte`) opens it. From then on your lines go to the normal chat, with all its tools and pasted images, and the model gets the gate's question as context. The workflow keeps waiting.
- With the [decision model](configuration.md#decision-model-cloudflare-clef), a line it is sure is a side question (and not an answer) opens the side chat by itself, with a notice. Pasting an image at a gate also opens it.
- To go back, type `/flow <answer>` (or `/volver`) to answer the gate directly. You can also type `/flow`, or a line such as "listo, continúa", "volvamos al flujo" or "back to the flow": the model then **proposes the answer** from the side conversation. The next line decides what to do with it. `Enter` or a "yes" sends it, and any other text is sent instead. With the decision model, a line like "listo, la respuesta es 200 ms, continúa" also goes back.
- `/flow` with no side chat open shows the gate's question again. `/stage`, `/back`, `/skip`, `/retry` and `/abort` act on the workflow and close the side chat.

The side chat lives in memory. After a restart, `/research resume` (or `/implement resume`) shows the gate again. In headless mode `/aside` and `/flow` work the same, except that a side question is never guessed.

**Failures never trap the chat.** A step whose model call fails for a passing reason (rate limit, overload, a dropped connection, an empty or unparsable answer) is retried twice by itself. If it still fails, the workflow is saved as **failed** at its last checkpoint, the timeline shows an `Error` block with what to do, and the chat goes back to normal. `/research retry` (or `/retry`, or `/research resume`) runs the failed step again; `/implement resume` does the same for an implementation. The same holds after a restart: the pending or failed workflow is announced when crest opens.

`Ctrl+C` stops a running step in the window: the workflow stays at its last checkpoint and resumes the same way. In headless mode it kills the process, and on resume the interrupted step runs again.

## Research workflow

### Starting, querying and resuming

| Where | Command | What it does |
| --- | --- | --- |
| Window | `/research <problem description>` | Starts a new workflow and runs it up to the first gate. The previous one stays saved and listed. |
| Window | `/research resume [n]` (also `continue`, `retomar` or `reanudar`) | Resumes the workspace's last workflow, or number `n` of `/research list`. A failed or interrupted step runs again. |
| Window | `/research retry`, `/retry` | Runs the failed step of the last workflow again. |
| Window | `/research list` | The workflows of the workspace, newest first, with their problem and where each one is. |
| Window | `/research` | Tells you which stage the workflow is at. |
| Headless | `crest --headless --research "<problem>"` | Runs the workflow in the terminal and reads the answers from stdin. |
| Headless | `crest --headless --research resume` (or `retry`) | Resumes the pending or failed workflow. |

When you open crest with a workflow halfway through (or failed), the notice "a research flow is pending (…); `/research resume` to continue" (or "`/research retry` runs it again") appears. The workflow does **not** activate on its own: until you resume it, your lines go to the normal chat.

On resume:

- if the workflow was waiting for an answer, crest shows the gate's question again;
- if it had failed or been interrupted, the step runs again from the last checkpoint;
- if it had finished, it cannot be resumed, but `/back <node>` reopens it at that node;
- if it was aborted, it cannot be resumed.

### The 14 nodes

The node and stage names are the ones the app shows.

| # | Node | Stage | Role | Gate | What it does | Blocks |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Intake | Discovery | `main` | yes | Restates the request, classifies it (feature, bug, performance, refactor) and lists open questions. | Text |
| 2 | Observation | Discovery | `explore` | | Walks the repo with a read-only agent (tree, key files, `git log`) and draws the current architecture. | Mermaid, Table, Sources |
| 3 | Data | Discovery | `explore` | | Gathers evidence: commits of the files involved (up to 20), findings and, if libraries are involved, documentation from the web. **Optional.** | Chart (commits per week), Table, Markdown, Sources |
| 4 | Analysis | Discovery | `main` | | Tests ideas against the evidence; every finding cites existing evidence. | Table, Chart |
| 5 | Hypotheses | Discovery | `plan` | yes | A one-sentence hypothesis, a cause diagram (Ishikawa) and a prioritized table. | Mermaid, Table |
| 6 | Experiments | Discovery | `main` | | Designs and runs experiments with the chat agent (with its tools and approvals). A hypothesis is only marked supported or refuted if the command actually ran. | Table, Chart (plus the agent's ToolCall and Diff) |
| 7 | Definition and constraints | Definition | `plan` | yes | Defines the problem in one **measurable** sentence (there may be several; each one yields a Spec) and the constraints table. | Markdown, Table |
| 8 | Research | Solution search | `explore` | | Looks for prior solutions in the repo and on the web (up to 5 searches and 3 pages). | Sources, Table |
| 9 | Brainstorm | Solution search | `plan` | | Generates unfiltered options. | Table |
| 10 | Modeling | Solution search | `plan` | yes | Discards the options that violate constraints and builds the decision matrix (scores 1-5) and the black box of the chosen option. | Table, Mermaid, Markdown |
| 11 | Prototype | Solution search | `main` | question | Optional spike in a temporary `git worktree`. **Optional.** | ToolCall, Diff, Markdown |
| 12 | Spec | Specification | `plan` | | Writes the Spec (`spec.md`). | Markdown, Sources |
| 13 | Plan | Specification | `plan` | | Splits the Spec into tasks (`plan.json` and `plan.md`) and checks that there are no cycles. | Mermaid, Table |
| 14 | Critical review | Specification | `plan` | yes | First runs deterministic checks; then a subagent with a clean context looks for gaps. If there are serious gaps, it goes back to the stage where information is missing. | Markdown, Table |

Each time you enter a node, a StageHeader block (`Stage 5/14 · Hypotheses`) appears with a progress bar. The models for each role are configured in [configuration.md](configuration.md#models-by-role). Research and Data search the web through the read-only agent with the local `web_search` and `web_fetch` tools (see [configuration.md](configuration.md#web-search)); the pages it reads become evidence with their URL. Without curl, or with `app.opt.web.enabled = false`, those stages skip the web and say so in the journal.

**Agents**:

- Observation, Data and Research use a **read-only** agent. It has `list_files`, `find_file`, `tree`, `grep`, `read_file`, `web_search` and `web_fetch` (when the web is configured), `run_command` (with the read-only policy, so only read-only `git` commands get through) and the MCP tools declared with `effect = "read"`. It never asks for approval and is limited to 30 calls.
- Experiments and Prototype use the chat agent, with your permissions and approvals.

### Gates and answers

When a node with a gate finishes, the workflow asks:

> **Hypotheses** is done. Answer "yes" to go on to Experiments (6/14), write corrections to redo this stage or use `/back <stage>` to go back.

After the last node the question ends in "to finish the flow" instead.

| Answer | Effect |
| --- | --- |
| Approval: `yes`, `y`, `yep`, `sure`, `ok`, `okay`, `go`, `go ahead`, `next`, `continue`, `proceed`, `approve`, `approved`, `agreed`, `done`, `lgtm`; the Spanish aliases `sí`/`si`, `s`, `vale`, `dale`, `listo`, `sigue`, `seguir`, `continua`/`continuar`, `adelante`, `de acuerdo`, `aprobado`; or **an empty line** | Moves on to the next node. Case, accents and a trailing `.`, `!` or `,` are ignored. |
| Any other text, for example "add a hypothesis about the cache" | Redoes the stage with that text as a correction and asks again. "yes, but add hypothesis H3" counts as a correction, not an approval. |
| `/stage`, `/back`, `/skip`, `/abort` | See the next table. |

If Intake leaves open questions, answer them by typing at the gate: your text is added to the answers and Intake is redone.

There are also **question** gates: any text you type, including "yes", is taken as the answer and the node runs again with it. They appear in three cases:

- **Definition**, when after a retry the definition is still not measurable. It asks what is observed and how much (for example, "at 500 requests per second the response time rises from 10 ms to 500 ms") or a testable condition ("running X makes test Y fail").
- **Prototype**, before creating the worktree.
- When a stage has asked to **go back more than 3 times** to another one. The workflow asks you what is missing.

### Commands inside the workflow

| Command | What it does |
| --- | --- |
| `/stage` | Shows the position, for example "Stage 5/14: Hypotheses (Discovery) · waiting for your answer". The status at the end can also be "running", "failed: …", "flow finished" or "flow aborted". |
| `/back <node>` | Goes back to that node and runs it immediately. `<node>` can be the number (1-14), the name (accents and case don't matter: `hypotheses`, `Definition and constraints`, `critical review`), the Spanish name kept as an alias (`hipótesis`, `observacion`, `Definición y restricciones`, `lluvia de ideas`, `revisión crítica`), the id (`intake`, `observation`, `data`, `analysis`, `hypotheses`, `experiments`, `definition`, `research`, `brainstorm`, `modeling`, `prototype`, `spec`, `plan`, `critical_review`) or a unique prefix of at least 3 letters of the English or Spanish name (`hyp`, `defin`, `crit`, `brain`, `hip`, `lluvia`, `revis`). Only nodes before the current one, or the current one, are accepted, except when the workflow has finished, in which case any node is. |
| `/skip` | Skips the current node. Only valid for **Data (3)** and **Prototype (11)**. In practice, Data can only be skipped after it fails, because it has no gate. |
| `/retry` | Runs a failed (or interrupted) node again. |
| `/abort` | Aborts the workflow. The state and the journal stay saved and the chat goes back to normal. |

These commands work on the workspace's last workflow even when it is not waiting at a gate (for example after a failure). A malformed command produces an error, for example `usage: /back <stage>, by number (1-14) or name, e.g. /back 5 or /back hypotheses`.

**If a node fails** after its automatic retries, the timeline shows an `Error` block "<Stage>: <error>" with the hint to retry, the workflow is saved as failed at its last checkpoint and the chat is free again. `/research retry` runs the node again; `/back <stage>` redoes an earlier one; `/skip` skips it if it is optional.

### Automatic go-backs

A node goes back to an earlier stage when it lacks information, instead of making it up. Each go-back appears as a notice ("Spec goes back to Modeling: …") and is recorded in the journal. The main ones are:

| Node | Goes back to | When |
| --- | --- | --- |
| Observation, Data | Intake / Observation | There is no brief or no observation report. |
| Analysis, Hypotheses, Definition | Data or Observation | There is no evidence. |
| Experiments, Definition | Hypotheses | There are no hypotheses. |
| Research, Brainstorm | Definition | There is no problem definition. |
| Modeling | Definition / Brainstorm | There are no constraints, no options, or every option violates some constraint. |
| Prototype | Modeling | There is no decision, or the spike turned out not to be feasible. |
| Spec | Definition / Observation / Modeling | The definition, the evidence or the decision is missing. |
| Plan | Spec | There is no Spec draft. |
| Critical review | the stage that is missing | There are unknown `[E…]` citations, empty sections, background without citations or cited files that do not exist (Observation); a constraint without a test (Definition); an invalid plan, one without `hypothesis_tests` or without a test for a constraint (Plan); or serious gaps found by the subagent. |

After 3 go-backs from the same stage, the fourth becomes a question for you.

### Prototype (spike)

1. If you approve the spike, crest runs `git worktree add -b crest/spike-<id> .crest/spikes/<id> HEAD` inside the workspace, with the `run_command` tool and its approvals. Beforehand it shows you the exact commands, cleanup included, and asks `Create it? Answer "yes" to go on, anything else to continue without a prototype, or /skip.`
2. The chat agent writes the spike in the worktree and gives a verdict (feasible or not), which is recorded as evidence and shown in a "Spike result" block.
3. The cleanup (`git worktree remove --force` and `git branch -D`) always runs. If the spike is not feasible, the workflow goes back to Modeling.

It requires `git` and a repository. If `git worktree add` fails, the node fails, and `/skip` lets you continue without a prototype. While the worktree exists, `.crest/` shows up as untracked in `git status`, unless you ignore it.

### What it writes: `.crest/specs/NNNN-slug/`

One folder is created per problem definition. The files are written without asking for approval.

```text
.crest/specs/0007-session-cache/
  spec.md        Spec for people (10 sections)
  simulation/    written by /simulate (see Simulating a solution)
  plan.md        readable plan, generated from plan.json (do not edit by hand)
  plan.json      plan consumed by the implementation workflow
  journal.md     research journal (rewritten by the Plan node)
  sources.json   cited evidence: id, claim, source (file, commit, URL or command) and date
  review.md      result of the critical review
  evidence/
    chart-1-commits-per-week-in-the-files-involved.csv   data of each chart (label,<series...>)
```

- **`NNNN`** is the largest 4-digit prefix already present in `.crest/specs/`, plus 1. If `.crest/` is in `.gitignore`, crest does not see the existing folders and numbering starts again at 0001.
- **`slug`** comes from the title: lowercase, without accents, with `-` in place of any other character and at most 48 characters. If it ends up empty, it is `spec`.
- Only the charts' CSV files are written to `evidence/`. No images or command outputs are saved.
- The journal is rewritten for the last time in the Plan node, so the Critical review entries never reach `journal.md`.

The sections of `spec.md` (headings as crest writes them) are:

- `## 1. Problem`
- `## 2. Background` (with citations)
- `## 3. Hypothesis`
- `## 4. Proposed solution` (with the black box in Mermaid)
- `## 5. Functional specification` (with `### Sequence` and `### States`)
- `## 6. Technical specification`
- `## 7. Constraints` (a table with the columns Id, Kind, Constraint, Test)
- `## 8. Test protocol`
- `## 9. Out of scope and risks` (with `### Out of scope` and `### Risks`)
- `## 10. Sources` (`[E1]`, ..., and links to the CSV files)

The critical review also accepts the Spanish headings written by earlier versions (`1. Problema`, `2. Antecedentes`, ..., `10. Fuentes`) when it reads an existing `spec.md`.

#### `plan.json`

```json
{
  "spec": "0007-session-cache",
  "tasks": [
    {
      "id": "T1",
      "title": "Add SessionCache to the storage port",
      "description": "...",
      "depends_on": [],
      "files": ["crates/app/src/ports/store.rs"],
      "acceptance": [
        { "kind": "test", "ref": "cargo test -p app session_cache" }
      ],
      "role": "main",
      "risk": "low"
    }
  ],
  "hypothesis_tests": [{ "kind": "command", "ref": "make bench-sessions", "expect": "p95 < 20ms" }],
  "constraint_tests": [{ "constraint": "C2", "kind": "test", "ref": "cargo test -p store offline" }]
}
```

| Field | Required | Values |
| --- | --- | --- |
| `spec` | yes | The folder name (set by the Plan node). |
| `tasks[].id`, `title`, `description` | yes | Strings. Ids must be unique. |
| `tasks[].depends_on` | yes | Ids of other tasks, without cycles. |
| `tasks[].files` | yes | At least one path. A directory ending in `/` covers everything under it. |
| `tasks[].acceptance` | yes | `{ kind: "test" \| "command", ref: "<command line>" }`. |
| `tasks[].role` | no (`main`) | `main`, `plan`, `explore`, `compact`, `title`, `component` or `deep`. |
| `tasks[].risk` | yes | `low`, `medium` or `high`. |
| `hypothesis_tests` | no (`[]`) | `{ kind, ref, expect }`. `expect` is text, for example `"p95 < 20ms"`. |
| `constraint_tests` | no (`[]`) | `{ constraint: "C<n>", kind, ref }`. |

Unknown fields are not allowed. A plan with no tasks, duplicate ids, unknown dependencies, tasks without files or cycles is rejected (for example, `tasks form a dependency cycle: T1 -> T2 -> T1`).

## Implementation workflow

### Starting

| Where | Command |
| --- | --- |
| Window | `/implement <target>` |
| Headless | `crest --headless --implement <target>` |

`<target>` can be:

| Form | Example | Plan used |
| --- | --- | --- |
| A name without `/` | `0007-session-cache` | `.crest/specs/0007-session-cache/plan.json` |
| A path with `/` | `.crest/specs/0007-session-cache/` | `<path>/plan.json` |
| A path to a `.json` file | `other/plan.json` | that file |
| `.` | `.` | `plan.json` at the root |

There is no prefix matching: `0007` only works if a folder named exactly `0007` exists. Absolute paths must be inside the workspace.

Opening the same plan again **resumes** the workflow if it had not finished, with the notice "implementation flow of … resumed". `/implement resume` (or `/implement` alone) does it for the last plan opened in the workspace, which crest also announces at start-up while it is pending. A step that failed or was interrupted runs again. A finished or aborted workflow starts from scratch. On resume, the pending question is **not** shown again: answer directly, or type `/stage` to see it. In the window only one implementation workflow can be active at a time.

Requirements:

- `git` and a repository. The branch is created from the current `HEAD`. crest does not check that the working tree is clean, so uncommitted changes carry over to the branch.
- The crest database.
- The API key, because each task is implemented by the agent.

### Steps

The step names are the ones `/stage` prints.

| # | Step | What it does |
| --- | --- | --- |
| 1 | Load plan | Validates `plan.json` and orders the tasks by dependencies (ties broken by natural id order: T2 before T10). Shows a table and the graph in Mermaid. **Gate**, and then creates the `spec/<spec>` branch with `git switch -c` (if it already exists, it switches to it). If the plan is not valid, it goes back to the Spec without asking. |
| 2 | Implement task | One agent turn with the **task's role**. The prompt lists the allowed files and the acceptance criteria. The agent ends with `STATUS: done` or `STATUS: blocked: <reason>`, and `IDEA: …` lines go to the report. |
| 3 | Test and fix | Runs the acceptance criteria. If any fails, it asks the agent to fix it, **up to 3 times**. If they still fail, **gate**. |
| 4 | Commit | Stages **only** the task's files that changed (plus those approved out of scope) and commits. If nothing changed, there is no commit and a deviation is recorded in the journal. |
| 5 | Verify hypotheses | When all tasks are done, runs `hypothesis_tests` and `constraint_tests` once and shows a "Test / Command / Expected / Obtained / Holds" table and, if there are measurements, an "Expected against obtained" chart. |
| 6 | Report and next steps | Writes `report.md` in the Spec folder. **Final gate.** |

Steps 2 to 4 repeat for each task. Each step has a 1-hour limit.

**Commit message**:

```text
<type>(<scope>): <task title>

Task: <id>
```

- The `type` comes from the first word of the title (the stems are Spanish and English):
  - `fix` for correg-, corrig-, arregl-, fix, repar-;
  - `refactor` for refactor, mover, move, renombr-, rename, extra-;
  - `test` for test, prueb-, prob-;
  - `docs` for document-, doc;
  - `feat` otherwise.
- The `scope` is the package name of the `Cargo.toml` closest to the task's first file. If there is none, it is the top-level directory, and if the file is at the root there is no scope.
- The commit uses your git configuration, hooks included.

**Criteria and tests**:

- Each `ref` runs at the workspace root, directly, without going through approvals.
- If the line contains `| & ; < > $ \` * ? ( ) ~` outside quotes, it runs with `sh -c`.
- It passes if it exits with code 0. The limit is 30 minutes per test.
- `kind` (`test` or `command`) does not change how it runs; it is only used as a label.
- In `hypothesis_tests`, `expect` can be a comparison with a number and a unit (`p95 < 20ms`, `throughput >= 500 req/s`). In that case the hypothesis is met if the command exits with 0 **and** its output contains a value that satisfies the comparison. crest looks for the number that follows the last mention of the metric, converting between `ns`, `us`, `ms`, `s` and `min`. If `expect` is free text, the command exiting with 0 is enough.

### Gates and answers

Answers are interpreted ignoring case, accents, a leading `/` and a trailing `.`, `!` or `,`. The Spanish words are kept as aliases:

| Kind | Words |
| --- | --- |
| continue | The research workflow approvals (`yes`, `ok`, `continue`, `proceed`, ..., and the aliases `sí`, `continuar`, ...), `go on`, and **also an empty line**. |
| retry | `retry`, `try again`; aliases `reintentar`, `reintenta`, `otra vez` |
| skip | `skip`, `skip task`, `skip the task`; aliases `saltar`, `salta`, `saltar tarea`, `saltar la tarea` |
| abort | `abort`, `cancel`, `stop`; aliases `abortar`, `aborta`, `cancelar`, `detener` |
| back to the spec | `back to spec`, `back to the spec`, `go back to spec`, `go back to the spec`, `spec`, `back spec`, `back`; aliases `volver al spec`, `volver a spec`, `volver al nodo spec` |
| other text | Free-form instructions for the agent (where accepted). |

In addition, `/stage` (the position, for example "Step 3/6: Test and fix · task T1 (1/3) · waiting for your answer") and `/abort` work at any gate.

| Gate | When | Answers |
| --- | --- | --- |
| Plan | Before creating the branch | **continue**: creates the branch and starts. **abort**. **back to the spec**. Anything else repeats the question. |
| Branch | `git switch -c` failed | **continue** retries (**retry** does not). **abort**. |
| Blocked task | The agent answered `STATUS: blocked` | **retry**: another attempt. **other text**: another attempt with your instructions. **continue**: treats it as implemented and moves on to tests. **skip**: no commit. **abort**. **back to the spec**. |
| Failing tests | The 3 fix attempts are used up | **retry** or **other text**: 3 more attempts (with your instructions, if you gave any). **continue**: commits with the criteria failing. **skip**. **abort**. **back to the spec**. |
| Failed commit | `git commit` failed | **retry** or **other text**: retries the commit. **continue** or **skip**: moves on without a commit. **abort**. **back to the spec**. |
| Report | After writing `report.md` | **continue**: closes the workflow. **other text**: adds it to the report under "User notes" and asks again. **abort**. **back to the spec**. |

> Careful: an empty line (just `Enter`) counts as **continue** at every gate. At "Failing tests" that commits with the criteria failing, and at "Report" it closes the workflow.

In headless mode, each gate shows `› ` and reads a line from stdin. Approvals for out-of-scope files use the same stdin (`[y]es / [a]lways / [n]o`). `--yes` approves those actions, but does **not** answer the gates. If stdin closes, the workflow stays at that gate ("no answer (stdin closed): the flow stays at this gate; the same command resumes it") and crest exits with 1.

### File restriction

Each task can only touch its `files` (a directory with `/` covers its contents) with `edit_file` and `write_file`:

- Writing **outside** those files always requires approval, in any permission mode, with the text "Task X wants to change P, which is not one of its files (…)."
- If you approve, the path stays allowed for the rest of the task, is included in its commit and is recorded as a deviation in the journal.
- If you deny, the agent gets the rejection and it is recorded too.

`run_command` is **not** restricted per task. A command can modify any file, subject only to the normal permission policy, but the commit only includes the task's files.

### `report.md`

It is written next to `plan.json` and shown in the timeline. It contains:

- **Header** (`# Implementation report: <spec>`): plan, branch, tasks done (X of N) and tests that pass.
- **Tasks**: a table with the status and commit of each one.
- **Achievements**, **Failures** and **Deviations from the plan**.
- **Verification**: the hypotheses and constraints table.
- **Ideas for later Specs**: the `IDEA:` lines, the tasks left undone and the tests that do not pass.
- **User notes**: what you wrote at the final gate.

### Back to the Spec

If the plan turns out not to be viable, because you answered "back to spec" or because `plan.json` is invalid, the workflow **does not improvise**:

1. It records the deviation and stops with the message "the plan turned out to be unworkable (…); go back to the Spec node of the research flow with `/research resume` and then `/back spec` to fix it, and run /implement again".
2. It does **not** switch workflows automatically. To fix the plan:
   1. `/research resume` reopens the workspace's last research workflow (it stays active even if it has finished).
   2. `/back spec` goes back to the Spec node, which regenerates the Spec and then the plan.
   3. `/implement <spec>` starts the implementation again.

In headless mode, leaving through "back to spec" or "abort" exits with code 1.

## Simulating a solution

A simulation validates the solution of a Spec **before** implementing it, and gives a picture of it that is easy to share. It is optional and on demand, not a stage of the research workflow: when the research workflow finishes, its last message suggests `/simulate <spec>`.

| Where | Command |
| --- | --- |
| Window | `/simulate <spec>`: the board opens in a full-size modal and the verdicts land in the timeline ([usage.md](usage.md#simulation-board) lists the keys). |
| Terminal | `crest --headless --simulate <spec>`: prints the verdicts and the path of `board.html`; exits with 0 only when every scenario passes. |
| Chat | Ask the agent ("simulate the session-cache Spec"); it calls the `simulate_solution` tool, which writes into the Spec folder (a `write` action: in `ask` mode it asks first). |

`<spec>` is a folder of `.crest/specs/` (`0007-session-cache`), its number (`7`), a unique part of its name or its path.

### What happens

1. **Read the Spec.** `spec.md` gives the title, the black-box diagram (`flowchart`), the sequence and state diagrams, the design sections and the test protocol (with its `T1`, `T2`... items); `sources.json` the known evidence.
2. **Assertions first, frozen.** A call of its own (role `plan`) turns the test protocol into scenarios with their assertions, before any model exists, and names the components of the black box. These assertions are frozen.
3. **The model.** A second call writes an executable model in the `crest.sysmodel/v1` format from the diagrams: components (client, service, queue, db, cache, lb, external, timer) with ports, capacity, latency, integer variables and a flat state machine; connections; the scenarios with their inputs (injections, repeats, faults). Whatever assertions the model writes, the frozen ones are put back; any change is reported ("frozen assertions restored") and written to the journal.
4. **Validation and repair.** The model is checked against the JSON Schema (unknown fields and values are rejected), linted (references resolve, message types match across connections, guards use declared variables, states are reachable, emitted messages have handlers, components cite the Spec) and dry-run. Errors go back to the model in up to **2 repair rounds**. Failing assertions are **not** repaired: they are findings.
5. **Run.** Every scenario runs in a deterministic simulator: a virtual clock in integer microseconds, events ordered by time and sequence, random latencies drawn from the scenario's seed. Same model and seed, same trace. The checks are `always`, `never`, `eventually … within`, `count` (handled, ignored, rejected or dropped messages at a component) and `path` (the components a request visits, as in a sequence diagram), plus the model's invariants after every event. A failure comes with a counterexample: the event, its time and the state of every component.
6. **Record.** In the Spec folder:

```text
.crest/specs/0007-session-cache/
  simulation/
    sysmodel.json        the model (crest.sysmodel/v1)
    assertions.json      the frozen scenarios and assertions
    traces/<scenario>.json  the run: every event, checkpoints of the state, the verdicts
    board.html           self-contained replay (three.js inlined, no network)
  sources.json           + one evidence record per scenario (model hash, seed, trace hash, verdicts)
  journal.md             + a "Simulation" section
```

### The board

The board is an isometric circuit board of the solution: components are chips (colour by kind, the state glowing on top), connections are traces, messages are glowing packets that move along them, and queued messages stack on their chip. In the window it is drawn by crest's own offscreen GPU renderer (lit bevelled chips, edge outlines, bloom; isometric preset plus orbit, zoom and pan); without a GPU adapter it falls back to a 2.5D drawing. The timeline plays, steps from event to event and scrubs, with a red mark at each failed assertion; clicking a chip shows its state, variables, queue and machine.

`board.html` replays the recorded runs with the same geometry and controls in any browser, offline; a link such as `board.html#scenario=burst&t=0.4&chip=api` opens a scenario at a moment with a chip inspected.

### Reading the result

A simulation is a **behavioural sketch**: it proves that the design is consistent with its own assertions, not that the real system behaves so. Latencies are guesses unless the model cites evidence for them. A passing scenario is evidence that the design holds under those inputs; a failing one shows where it breaks, with the state that breaks it. Lint warnings (for example a test-protocol item no scenario covers) are listed with the verdicts.
