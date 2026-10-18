# Workflows: research and implementation

crest ships two guided workflows that run inside the chat:

- The **research workflow** follows the stages of the [Hashlabs X-Workflow](https://www.hashlabs.com/guides/en/methodology/X-Workflow.html). It starts from a problem described in natural language and ends in a **Spec** with its implementation plan.
- The **implementation workflow** takes a Spec's `plan.json` and runs it task by task, with one commit per task.

Both are built with [graph-flow](https://crates.io/crates/graph-flow). Every step is a checkpoint in SQLite, in `~/.local/share/crest/sessions.db`. At **gates**, the workflow stops and waits for your answer in the same chat input (in the window) or on stdin (in headless mode).

While a workflow is active, **every line you send goes to the workflow** and does not start a normal turn. The window's `:` commands (`:model`, `:mode`, `:clear`, `:q`...) keep working. If an implementation workflow and a research workflow are both active, lines go to the implementation workflow first.

> Limitation: `Ctrl+C` does not stop a running step of either workflow. In the window, the cancellation token does not reach the workflow. In headless mode it kills the process, and on resume the interrupted step runs again. To stop a workflow in an orderly way, answer `/abort` at a gate.

## Research workflow

### Starting, querying and resuming

| Where | Command | What it does |
| --- | --- | --- |
| Window | `:research <problem description>` | Starts a new workflow and runs it up to the first gate. If there was another one, it **replaces** it without asking: the old one stays saved but can no longer be resumed. |
| Window | `:research resume` (also `retomar` or `reanudar`) | Resumes the workspace's last workflow. |
| Window | `:research` | Tells you which stage the workflow is at. |
| Headless | `crest --headless --research "<problem>"` | Runs the workflow in the terminal and reads the answers from stdin. |
| Headless | `crest --headless --research resume` | Resumes the pending workflow. |

When you open crest with a workflow halfway through, the notice «hay un flujo investigativo pendiente (…); `:research resume` para seguir» appears. The workflow does **not** activate on its own: until you type `:research resume`, your lines go to the normal chat.

On resume:

- if the workflow was waiting for an answer, crest shows the gate's question again;
- if it had stopped because of an error, it continues from the last checkpoint;
- if it had finished, it stays active only so that you can use `/back <node>`, and any other line releases it;
- if it was aborted, it cannot be resumed.

Only one workflow is tracked per workspace: the last one you started.

### The 14 nodes

In the "Node" and "Stage" columns, the name in quotes is the one the app shows.

| # | Node | Stage | Role | Gate | What it does | Blocks |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Intake («Intake») | Discovery («Descubrimiento») | `main` | yes | Restates the request, classifies it (feature, bug, performance, refactor) and lists open questions. | Text |
| 2 | Observation («Observación») | Discovery | `explore` | | Walks the repo with a read-only agent (tree, key files, `git log`) and draws the current architecture. | Mermaid, Table, Sources |
| 3 | Data («Datos») | Discovery | `explore` | | Gathers evidence: commits of the files involved (up to 20), findings and, if libraries are involved, documentation from the web. **Optional.** | Chart (commits per week), Table, Markdown, Sources |
| 4 | Analysis («Análisis») | Discovery | `main` | | Tests ideas against the evidence; every finding cites existing evidence. | Table, Chart |
| 5 | Hypotheses («Hipótesis») | Discovery | `plan` | yes | A one-sentence hypothesis, a cause diagram (Ishikawa) and a prioritized table. | Mermaid, Table |
| 6 | Experiments («Experimentos») | Discovery | `main` | | Designs and runs experiments with the chat agent (with its tools and approvals). A hypothesis is only marked supported or refuted if the command actually ran. | Table, Chart (plus the agent's ToolCall and Diff) |
| 7 | Definition and constraints («Definición y restricciones») | Definition («Definición») | `plan` | yes | Defines the problem in one **measurable** sentence (there may be several; each one yields a Spec) and the constraints table. | Markdown, Table |
| 8 | Research («Investigación») | Solution search («Búsqueda de soluciones») | `explore` | | Looks for prior solutions in the repo and on the web (up to 5 searches and 3 pages). | Sources, Table |
| 9 | Brainstorm («Lluvia de ideas») | Solution search | `plan` | | Generates unfiltered options. | Table |
| 10 | Modeling («Modelado») | Solution search | `plan` | yes | Discards the options that violate constraints and builds the decision matrix (scores 1-5) and the black box of the chosen option. | Table, Mermaid, Markdown |
| 11 | Prototype («Prototipo») | Solution search | `main` | question | Optional spike in a temporary `git worktree`. **Optional.** | ToolCall, Diff, Markdown |
| 12 | Spec («Spec») | Specification («Especificación») | `plan` | | Writes the Spec (`spec.md`). | Markdown, Sources |
| 13 | Plan («Plan») | Specification | `plan` | | Splits the Spec into tasks (`plan.json` and `plan.md`) and checks that there are no cycles. | Mermaid, Table |
| 14 | Critical review («Revisión crítica») | Specification | `plan` | yes | First runs deterministic checks; then a subagent with a clean context looks for gaps. If there are serious gaps, it goes back to the stage where information is missing. | Markdown, Table |

Each time you enter a node, a StageHeader block (`Stage 5/14 · Hipótesis`) appears with a progress bar. The models for each role are configured in [configuration.md](configuration.md#models-by-role). The Anthropic server-side web search and fetch tools are added to calls for the `explore` and `plan` roles, except in Spec, Plan and Critical review.

**Agents**:

- Observation, Data and Research use a **read-only** agent. It has `list_files`, `find_file`, `tree`, `grep`, `read_file`, `run_command` (with the read-only policy, so only read-only `git` commands get through) and the MCP tools declared with `effect = "read"`. It never asks for approval and is limited to 30 calls.
- Experiments and Prototype use the chat agent, with your permissions and approvals.

### Gates and answers

When a node with a gate finishes, the workflow asks:

> **Hipótesis** terminó. Responde «sí» para seguir con Experimentos (6/14), escribe correcciones para rehacer esta etapa o usa `/back <nodo>` para regresar.

| Answer | Effect |
| --- | --- |
| Approval: `sí`/`si`, `s`, `ok`, `okay`, `vale`, `dale`, `listo`, `sigue`, `seguir`, `continua`/`continuar`, `adelante`, `de acuerdo`, `aprobado`, `y`, `yes`, `go`, `lgtm`, or **an empty line** | Moves on to the next node. Case, accents and a trailing `.`, `!` or `,` are ignored. |
| Any other text, for example "add a hypothesis about the cache" | Redoes the stage with that text as a correction and asks again. «sí, pero agrega H3» counts as a correction, not an approval. |
| `/stage`, `/back`, `/skip`, `/abort` | See the next table. |

If Intake leaves open questions, answer them by typing at the gate: your text is added to the answers and Intake is redone.

There are also **question** gates: any text you type, including «sí», is taken as the answer and the node runs again with it. They appear in three cases:

- **Definition**, when after a retry the definition is still not measurable. It asks what is observed and how much (for example, "at 500 requests per second the response time rises from 10 ms to 500 ms") or a testable condition ("running X makes test Y fail").
- **Prototype**, before creating the worktree.
- When a stage has asked to **go back more than 3 times** to another one. The workflow asks you what is missing.

### Commands inside the workflow

| Command | What it does |
| --- | --- |
| `/stage` | Shows the position, for example «Etapa 5/14: Hipótesis (Descubrimiento) · esperando tu respuesta». |
| `/back <node>` | Goes back to that node and runs it immediately. `<node>` can be the number (1-14), the Spanish name (accents and case don't matter: `hipótesis`, `observacion`, `Definición y restricciones`), the id (`intake`, `observation`, `data`, `analysis`, `hypotheses`, `experiments`, `definition`, `research`, `brainstorm`, `modeling`, `prototype`, `spec`, `plan`, `critical_review`) or a unique prefix of at least 3 letters of the Spanish name (`hip`, `defin`, `lluvia`). Only nodes before the current one, or the current one, are accepted, except when the workflow has finished, in which case any node is. |
| `/skip` | Skips the current node. Only valid for **Data (3)** and **Prototype (11)**. In practice, Data can only be skipped after it fails, because it has no gate. |
| `/abort` | Aborts the workflow. The state and the journal stay saved and the chat goes back to normal. |

An unknown `/` command (for example `/model`) does **not** produce an error: it is taken as the answer to the gate. The error messages of these commands are in English, for example `usage: /back <stage>, by number (1-14) or name, e.g. /back 5 or /back hipótesis`.

**If a node fails**, the timeline shows an `Error` block «<Stage>: <error>» and the workflow stays at its last checkpoint. Free text is no longer accepted (it returns «the research flow is not waiting for input»). To retry, use `/back <current stage>` or `:research resume`.

### Automatic go-backs

A node goes back to an earlier stage when it lacks information, instead of making it up. Each go-back appears as a notice («Spec regresa a Modelado: …») and is recorded in the journal. The main ones are:

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

1. If you approve the spike, crest runs `git worktree add -b crest/spike-<id> .crest/spikes/<id> HEAD` inside the workspace, with the `run_command` tool and its approvals. Beforehand it shows you the exact commands, cleanup included, and asks «¿Lo creo? Responde «sí» para seguir, cualquier otra cosa para seguir sin prototipo, o /skip.»
2. The chat agent writes the spike in the worktree and gives a verdict (feasible or not), which is recorded as evidence and shown in a «Resultado del spike» block.
3. The cleanup (`git worktree remove --force` and `git branch -D`) always runs. If the spike is not feasible, the workflow goes back to Modeling.

It requires `git` and a repository. If `git worktree add` fails, the node fails, and `/skip` lets you continue without a prototype. While the worktree exists, `.crest/` shows up as untracked in `git status`, unless you ignore it.

### What it writes: `.crest/specs/NNNN-slug/`

One folder is created per problem definition. The files are written without asking for approval.

```text
.crest/specs/0007-cache-de-sesiones/
  spec.md        Spec for people (10 sections)
  plan.md        readable plan, generated from plan.json (do not edit by hand)
  plan.json      plan consumed by the implementation workflow
  journal.md     research journal (rewritten by the Plan node)
  sources.json   cited evidence: id, claim, source (file, commit, URL or command) and date
  review.md      result of the critical review
  evidence/
    chart-1-commits-por-semana.csv   data of each chart (label,<series...>)
```

- **`NNNN`** is the largest 4-digit prefix already present in `.crest/specs/`, plus 1. If `.crest/` is in `.gitignore`, crest does not see the existing folders and numbering starts again at 0001.
- **`slug`** comes from the title: lowercase, without accents, with `-` in place of any other character and at most 48 characters. If it ends up empty, it is `spec`.
- Only the charts' CSV files are written to `evidence/`. No images or command outputs are saved.
- The journal is rewritten for the last time in the Plan node, so the Critical review entries never reach `journal.md`.

The sections of `spec.md` (headings as crest writes them) are:

1. «Problema»
2. «Antecedentes» (with citations)
3. «Hipótesis»
4. «Propuesta de solución» (with the black box in Mermaid)
5. «Especificación funcional» (with `### Secuencia` and `### Estados`)
6. «Especificación técnica»
7. «Restricciones» (a table with the columns Id, Tipo, Restricción, Prueba)
8. «Protocolo de pruebas»
9. «Fuera de alcance y riesgos»
10. «Fuentes» (`[E1]`, ..., and links to the CSV files)

#### `plan.json`

```json
{
  "spec": "0007-cache-de-sesiones",
  "tasks": [
    {
      "id": "T1",
      "title": "Agregar SessionCache al puerto de storage",
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
| Window | `:implement <target>` |
| Headless | `crest --headless --implement <target>` |

`<target>` can be:

| Form | Example | Plan used |
| --- | --- | --- |
| A name without `/` | `0007-cache-de-sesiones` | `.crest/specs/0007-cache-de-sesiones/plan.json` |
| A path with `/` | `.crest/specs/0007-cache-de-sesiones/` | `<path>/plan.json` |
| A path to a `.json` file | `otro/plan.json` | that file |
| `.` | `.` | `plan.json` at the root |

There is no prefix matching: `0007` only works if a folder named exactly `0007` exists. Absolute paths must be inside the workspace.

Opening the same plan again **resumes** the workflow if it had not finished, with the notice «flujo de implementación de … reanudado». A finished or aborted workflow starts from scratch. On resume, the pending question is **not** shown again: answer directly, or type `/stage` to see it. In the window only one implementation workflow can be active at a time.

Requirements:

- `git` and a repository. The branch is created from the current `HEAD`. crest does not check that the working tree is clean, so uncommitted changes carry over to the branch.
- The crest database.
- The API key, because each task is implemented by the agent.

### Steps

The step names in parentheses are the ones `/stage` prints.

| # | Step | What it does |
| --- | --- | --- |
| 1 | Load plan («Cargar plan») | Validates `plan.json` and orders the tasks by dependencies (ties broken by natural id order: T2 before T10). Shows a table and the graph in Mermaid. **Gate**, and then creates the `spec/<spec>` branch with `git switch -c` (if it already exists, it switches to it). If the plan is not valid, it goes back to the Spec without asking. |
| 2 | Implement task («Implementar tarea») | One agent turn with the **task's role**. The prompt lists the allowed files and the acceptance criteria. The agent ends with `STATUS: done` or `STATUS: blocked: <reason>`, and `IDEA: …` lines go to the report. |
| 3 | Tests and fixes («Pruebas y corrección») | Runs the acceptance criteria. If any fails, it asks the agent to fix it, **up to 3 times**. If they still fail, **gate**. |
| 4 | Commit («Commit») | Stages **only** the task's files that changed (plus those approved out of scope) and commits. If nothing changed, there is no commit and a deviation is recorded in the journal. |
| 5 | Verify hypotheses («Verificar hipótesis») | When all tasks are done, runs `hypothesis_tests` and `constraint_tests` once and shows a «Prueba / Comando / Esperado / Obtenido / Cumple» table and, if there are measurements, an «Esperado contra obtenido» chart. |
| 6 | Report and next steps («Reporte y futuro») | Writes `report.md` in the Spec folder. **Final gate.** |

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

Answers are interpreted ignoring case and accents:

| Kind | Words |
| --- | --- |
| continue | The research workflow approvals (`sí`, `ok`, `continuar`, ...) and **also an empty line**. |
| retry | `reintentar`, `reintenta`, `otra vez`, `retry` |
| skip | `saltar`, `salta`, `saltar tarea`, `saltar la tarea`, `skip` |
| abort | `abortar`, `aborta`, `abort`, `cancelar`, `detener` |
| back to the spec | `volver al spec`, `volver a spec`, `volver al nodo spec`, `spec`, `back spec`, `back` |
| other text | Free-form instructions for the agent (where accepted). |

In addition, `/stage` (the position, for example «Paso 3/6: Pruebas y corrección · tarea T1 (1/3) · esperando tu respuesta») and `/abort` work at any gate.

| Gate | When | Answers |
| --- | --- | --- |
| Plan | Before creating the branch | **continue**: creates the branch and starts. **abort**. **back to the spec**. Anything else repeats the question. |
| Branch | `git switch -c` failed | **continue** retries (**retry** does not). **abort**. |
| Blocked task | The agent answered `STATUS: blocked` | **retry**: another attempt. **other text**: another attempt with your instructions. **continue**: treats it as implemented and moves on to tests. **skip**: no commit. **abort**. **back to the spec**. |
| Failing tests | The 3 fix attempts are used up | **retry** or **other text**: 3 more attempts (with your instructions, if you gave any). **continue**: commits with the criteria failing. **skip**. **abort**. **back to the spec**. |
| Failed commit | `git commit` failed | **retry** or **other text**: retries the commit. **continue** or **skip**: moves on without a commit. **abort**. **back to the spec**. |
| Report | After writing `report.md` | **continue**: closes the workflow. **other text**: adds it to the report under «Notas del usuario» and asks again. **abort**. **back to the spec**. |

> Careful: an empty line (just `Enter`) counts as **continue** at every gate. At "Failing tests" that commits with the criteria failing, and at "Report" it closes the workflow.

In headless mode, each gate shows `› ` and reads a line from stdin. Approvals for out-of-scope files use the same stdin (`[y]es / [a]lways / [n]o`). `--yes` approves those actions, but does **not** answer the gates. If stdin closes, the workflow stays at that gate («el mismo comando lo reanuda») and crest exits with 1.

### File restriction

Each task can only touch its `files` (a directory with `/` covers its contents) with `edit_file` and `write_file`:

- Writing **outside** those files always requires approval, in any permission mode, with the text «La tarea X quiere cambiar P, que no está entre sus archivos (…)».
- If you approve, the path stays allowed for the rest of the task, is included in its commit and is recorded as a deviation in the journal.
- If you deny, the agent gets the rejection and it is recorded too.

`run_command` is **not** restricted per task. A command can modify any file, subject only to the normal permission policy, but the commit only includes the task's files.

### `report.md`

It is written next to `plan.json` and shown in the timeline. It contains:

- **Header** («Reporte de implementación»): plan, branch, tasks done (X of N) and tests that pass.
- **Tasks**: a table with the status and commit of each one.
- **Achievements** («Logros»), **Failures** («Fallos») and **Deviations from the plan** («Desvíos del plan»).
- **Verification** («Verificación»): the hypotheses and constraints table.
- **Ideas for next Specs** («Ideas para siguientes Specs»): the `IDEA:` lines, the tasks left undone and the tests that do not pass.
- **User notes** («Notas del usuario»): what you wrote at the final gate.

### Back to the Spec

If the plan turns out not to be viable, because you answered «volver al spec» or because `plan.json` is invalid, the workflow **does not improvise**:

1. It records the deviation and stops with the message «el plan resultó inviable (…)».
2. It does **not** switch workflows automatically. To fix the plan:
   1. `:research resume` reopens the workspace's last research workflow (it stays active even if it has finished).
   2. `/back spec` goes back to the Spec node, which regenerates the Spec and then the plan.
   3. `:implement <spec>` starts the implementation again.

In headless mode, leaving through «volver al spec» or «abortar» exits with code 1.
