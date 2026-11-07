# Spec 0002: projects with several directories, standalone chats, a three-width sidebar, a prompt mode control, and grouped commands

Status: **proposed**. Source: Specky3 discovery `CREST-SIDEBAR` (`Organization of chats, projects
and chat modes in the crest sidebar`), section `## Solution proposal` and spikes `S01`, `S02`, `S03`,
`S05`, `S06` (`.aos/journal/CREST-SIDEBAR/person/01-discovery.md`, `agent/spikes/`). Prototyped, not
yet built, in `mock/` (see `mock/src/organization.js`, `mock/src/commands.js`, `mock/src/modes.js`).
This spec is the port of that prototype into the crates.

The requirements this implements, in plain terms and without crest's source, are in
[R02-chats-projects-and-modes.md](../requirements/R02-chats-projects-and-modes.md).

## Problem

Five facts in the crates, confirmed by claims `0001` to `0007` and the Structural Map of the discovery:

1. A session stores one directory: `Session::workspace_root: String`
   (`crates/domain/src/conversation.rs:392`). There is no project table: the sidebar's projects are
   `recent_workspaces`, a `GROUP BY workspace_root` over `sessions`
   (`crates/adapters/store/src/session.rs:345-356`), loaded with `RECENT_PROJECTS = 20`
   (`crates/bin/src/gui.rs:2202`).
2. `chat.new` creates the session in the directory that is open (claim 0001, `crates/ui/src/chats.rs`),
   so no chat exists without a directory and a cross-repository task needs one chat per directory
   (claim 0002).
3. The sidebar is `PANEL_WIDTH = 240.0` (`crates/ui/src/chats.rs:165`), and `panel.chats`
   (`chats.rs:124`) shows or hides it whole (claims 0006, 0007).
4. The palette shows `PALETTE_ROWS = 8` rows (`crates/ui/src/input/completion.rs:20`) over one flat
   `Vec<CommandRow>` built by `snapshot` (`crates/bin/src/help.rs:419`): the 38 `CommandInfo`
   entries of `CATALOG` (`crates/app-core/src/usecases/commands/catalog.rs:32`) followed by plugin
   `CommandSpec`s (`crates/app-core/src/ports/plugin.rs:91`). `CommandRow`
   (`crates/ui/src/help.rs:52`) carries `name`, `usage`, `description`, and `source`, and no group
   (claim 0004).
5. Research starts only from typed text: `/research` and `--deep | -d | --profundo`
   (`crates/app-core/src/usecases/commands/research.rs:21`, claim 0005).

Three more facts, found after the discovery while reviewing the prototype, read from the source and not
backed by a discovery claim:

6. Deleting is the only way to take a chat out of the list: `delete` runs `DELETE FROM sessions`
   (`crates/adapters/store/src/session.rs:373-375`), and `timeline_blocks` goes with it through
   `ON DELETE CASCADE` (`migrations/0001_init.sql`). Nothing archives a chat or a project.
7. Nothing copies a chat. `/aside` (`catalog.rs:266`) opens a side chat that lives in memory while a
   pipeline waits at a gate, and `/flow` returns from it; it is not saved and does not fork the history.
8. The shell runs only as `/!`: `parse` strips a `!` after the slash (`crates/app-core/src/usecases/commands.rs:50`),
   the catalog shows it as `/! <command>` (`catalog.rs:133`), and `classify_line`
   (`crates/ui/src/input.rs:159`) sends any line not starting with `/` to the agent, so `! cat ~/file`
   is a prompt today.

## Non-goals

Carried over from the Solution proposal's Future work and the discovery's Scope:

- No folders inside projects, no drag and drop, no renaming of commands (a rename needs an alias).
- No change to the timeline, activity line, status bar, questions rail ([0001](0001-visibility-and-reach.md)),
  the file tree, or the stages inside the research, implement, and simulate pipelines.
- No change to the Lua plugin API: a plugin's group comes from `CommandSpec`'s existing source field.

## Proposed approach

### 1. A projects table and an optional project on each session (spikes S01, S02)

Add a migration in `crates/adapters/store/src/migrations.rs`:

- `projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, pinned INTEGER NOT NULL DEFAULT 0, created_at, updated_at)`.
- `project_roots (project_id TEXT REFERENCES projects ON DELETE CASCADE, root TEXT NOT NULL, position INTEGER, PRIMARY KEY (project_id, root))`.
- `sessions.project_id TEXT NULL REFERENCES projects ON DELETE SET NULL`.
- Data step: for each distinct non-empty `workspace_root`, insert one project named after the last
  path component, with that one root, and set `project_id` on its sessions. This is the mock's
  `fromDirectoryCatalog` (`mock/src/organization.js`); S02 criterion 1 checked that it keeps every id.

`workspace_root` stays as a column, so the queries that read it today keep working during the port. A
standalone session has `project_id NULL` and an empty `workspace_root`. In the domain, `Session`
gains `project_id: Option<ProjectId>`, and a new `Project { id, name, pinned, roots: Vec<String> }`
lives next to it in `crates/domain/src/conversation.rs`.

The store port gains `projects()`, `create_project(name, roots)`, `add_root`, `remove_root`,
`rename_project`, `pin_project`, and `move_session(session_id, Option<ProjectId>)`, with the mock's
rules:

- `create_project` refuses an empty name or an empty root list.
- `add_root` normalizes a trailing slash and ignores a duplicate.
- `remove_root` refuses to remove the last root.
- `move_session` keeps the session id and its blocks.

The fake in `crates/app-testing/src/fakes/system.rs` gets the same behavior, and the existing
`recent_workspaces_match_the_fake` pattern (`session.rs:721`) adds a projects case.

### 2. Every root of a project is a workspace (risk: pathway 2)

`WorkspaceRoot` (`crates/domain/src/workspace.rs:68`) and `resolve` (`workspace.rs:110`) check one
root. A project session builds a list of `WorkspaceRoot`s. Resolving a path tries each root in order,
and `PathError`'s "outside the workspace" (`workspace.rs:23`) means outside all of them. The file
tools (`crates/app/src/tools/files.rs:46`) and `open_file` (`crates/app-core/src/usecases/open_file.rs:82`)
take the list. A standalone session has no root: a tool that needs one returns a message asking the
person for a directory (S01 criterion 4) instead of resolving against the process's current directory.

Status: the file tools, `open_file`, the sandbox, and now the search tools (`list_files`, `find_file`,
`grep`, `tree`, plus the file tree panel and the `@` list, which read the same port) take the list.
The searches run in every root with the same ignore and sensitive-path rules, name the paths of the
first root relative and of the others absolute (`RootedPath::address`), share the limit between the
roots, and note a root that cannot be read while the others answer. `run_command` keeps running in the
first root (it has no directory argument). Not done: the tree panel watches only the first root for
changes.

### 3. The sidebar model and three widths (spike S05, risks: pathways 1, 3, 6, 7)

`crates/ui/src/chats.rs` replaces its per-directory catalog with the mock's `sidebarModel`:

- Projects come first, pinned and then by newest chat, each with its chats newest first and a directory count.
- The standalone chats follow under "Wild Chats".
- A search query filters chats by title.

A `SidebarWidth { Full, Strip, Hidden }` replaces the boolean behind `panel.chats`. The window opens
`Hidden`. `Full` keeps `PANEL_WIDTH` 240. `Strip` is 48 px and shows its rows all the time: the Crest
mark, new chat, search, projects, and one initial per project.

`Hidden` holds only the Crest mark, in the top left, and the rail ends where the prompt starts
(`mock/src/app.js` lays the prompt under the whole window). While it is closed the session uses the
full width and starts below the mark, so there is no empty column and the mark never paints over a
block. Pointer hover or keyboard focus on the mark reveals the strip's rows as a bar that floats over the session (the session does not move); the rows stay inside the body's height and never cross the prompt. Leaving hides them again
without changing the state. Clicking the mark sets `Full`. The mark in the full sidebar sets
`Hidden`, not `Strip`. A project initial shows the project's name beside it while the pointer is on it.
`<C-g>s` still cycles `Full`, `Strip`, `Hidden`, and back to `Full` (`nextSidebar`); the existing keymap
binding stays.

The chat and project menus follow `mock/src/components/chats-panel/chats-panel.js`:

- Chat: New chat, Branch chat, Rename, "Add to project" or "Move to project", Remove from project,
  Archive, Delete.
- Project: New chat here, Directories with remove, Add directory, Rename, Pin, Archive.

`chat.new` from a project row passes that project. `chat.new` elsewhere, and `/clear`, keep the
current chat's project, or create a standalone chat when the current chat has none.

### 4. The prompt mode control (spike S06; S04 failed on width; risk: pathway 5)

The prompt row in `crates/ui/src/input/` gains, before the text field:

- A round 26 px button that opens a popup with two parts: modes (Chat, Research, Deep research) and
  inserts (`@` file, `/` command). The popup reuses the popup machinery in `crates/ui/src/input/popups.rs`.
- A chip with the mode's label and a 14 px ×, in the extra-small text size, with no icon.

S04's chip with an icon measured 180 px and failed the 160 px limit; S06's compact chip measured
137 px. The input state holds `ChatMode { Chat, Research, Deep }`. On submit, `Research` and `Deep`
prefix the text with `/research ` or `/research --deep ` (`modeCommand`), and a line that already
starts with `/` is sent as typed. Backspace on an empty prompt, the chip's ×, or choosing Chat resets
the mode. A new action `prompt.mode` cycles the modes (the mock binds it to `<C-g>m`).

### 5. A group on every command row (spike S03, risk: pathway 4)

- `CommandInfo` (`catalog.rs`) gains `group: CommandGroup`, one of Chat, Flows, Stages, Model &
  permissions, Workspace, App. Every `CATALOG` entry sets it; a test asserts that none is missing.
- `CommandRow` (`crates/ui/src/help.rs:52`) gains `group: String`. `snapshot` (`crates/bin/src/help.rs:419`)
  fills it from `CommandInfo::group` for built-ins, and as `Plugin · <source>` for every
  `CommandSpec`, so plugins change nothing.
- The palette in `crates/ui/src/input/completion.rs` and `popups.rs` has two views. `/` alone lists
  group rows (label, the command names, count), and Enter, Right, or Tab opens one group, while Left,
  or Backspace on `/`, goes back. `/<prefix>` keeps today's prefix match across all commands and shows
  each row's group as a tag (`palette` in `mock/src/commands.js`).
- `PALETTE_ROWS` stays 8, and the group view fits in it: 6 built-in groups plus one per plugin, scrolling as today.

### 6. Archive and restore (R02 FR-18, FR-19)

The same migration adds `sessions.archived INTEGER NOT NULL DEFAULT 0` and
`projects.archived INTEGER NOT NULL DEFAULT 0`. The store port gains `set_session_archived(id, bool)`
and `set_project_archived(id, bool)`; neither touches `timeline_blocks`. The sidebar model leaves out
archived projects, their chats, and archived chats, and an `archived()` query feeds a folded
"Archived (N)" section after "Wild Chats" with one restore button per row (`sidebarModel` and
`archivedModel` in `mock/src/organization.js`). `delete` keeps its meaning and its cascade.

### 7. Branch a chat (R02 FR-20, FR-21)

The migration adds `sessions.branched_from TEXT NULL REFERENCES sessions ON DELETE SET NULL` and
`sessions.branched_at INTEGER NULL`, the number of blocks copied. A new store call
`branch_session(id) -> SessionId` runs in one transaction:

- Insert a session with a new id, the source's `project_id` and `workspace_root`, and the title `<title> (branch)`.
- Copy the source's `timeline_blocks` rows with `INSERT INTO timeline_blocks SELECT`, keeping `seq` and `kind` and
  setting the new `session_id`.
- Set `branched_from` and `branched_at`.

The source session is only read. A new action `chat.branch` calls it and opens the new chat. The row
shows a branch icon, and the timeline starts with `Branched from "<title>" after N blocks` (the
`.timeline__branch` banner in `mock/src/components/timeline/timeline.js`). Deleting the original
leaves the branch and clears its link.

### 8. A bare `!` runs the shell (R02 FR-22)

`classify_line` (`crates/ui/src/input.rs:159`) returns `Line::Command` for a line whose first
non-blank character is `!`, so `! cat ~/file` reaches `parse` as `!cat ~/file`, which already strips
the `!`. The catalog usage becomes `! <command>` under the Chat group, and the palette prints it as `!`
rather than `/!` (`invocation` in `mock/src/commands.js`). The prompt mode never wraps a `!` line
(`modeCommand` in `mock/src/modes.js`), and `/!cmd` keeps working.

## UI placement

The mock is the reference: `mock/src/components/chats-panel/` for the full and strip sidebar and
the menus, and `mock/src/components/prompt-input/` for the mode button, chip, and grouped palette.
Screenshots of each state are in `mock/spikes/evidence/` (`S05-full-1280.png`, `S05-strip-1280.png`,
`S05-hidden-1280.png`, `S05-strip-1024.png`, `S06-mode-menu.png`, `S06-deep-chip.png`,
`extra-promote-menu.png`, `extra-project-menu.png`, `extra-palette-groups.png`, `menu-check-chat.png`,
`menu-check-project.png`, `menu-check-archived.png`, `branch-chat.png`, `palette-shell-fix.png`,
`sidebar-rail-hidden.png`, `sidebar-rail-hover.png`). A strip icon is a
`gpui-component` icon button; nothing in the mock needs a layout GPUI cannot draw.

## Acceptance

- After the migration on a store with chats in N directories, `projects()` returns N projects of one
  root, and all session ids are still listed (S02 criterion 1).
- A new chat with no project has `project_id NULL`, is listed under "Chats", and resolves no path. A
  file tool on it returns the ask-for-a-directory message (S01 criteria 1 and 4).
- `move_session` into an existing or a new project keeps the id and the blocks (S01 criteria 2 and 3).
- `add_root` with two directories in different locations gives 3 roots, a duplicate leaves 3, and
  `remove_root` on the last root is an error (S02 criteria 2 and 3). A path under any root resolves,
  and a path under none fails with "outside the workspace".
- The window opens hidden. The hidden mark sits in the top left and the session uses the full width,
  starting below the mark. Hovering the mark shows new chat, search, and projects as a bar that floats
  over the session, which does not move; the rows end at the prompt, and leaving hides them. The
  Crest mark opens the full sidebar, and the mark in the full sidebar returns to hidden. `<C-g>s` still
  cycles full, strip, and hidden. The strip is at most 56 px, and at 1024 px the session keeps at least
  90 percent of the window (S05, refined in the mock after that spike).
- Choosing Deep research shows a "Deep research" chip, and submitting `why` sends
  `/research --deep why`. The button and the chip take at most 160 px at 1280 px (S06).
- Every `CATALOG` entry has a group. `/` alone shows at most 8 group rows, a plugin command lands in
  `Plugin · <source>`, and `/res` and `/mode plan` find the same commands as today (S03).
- Archiving a chat hides it and archiving a project hides it with its chats; both are listed under
  "Archived", restoring brings them back, and the block count of every session is unchanged.
- `branch_session` on a chat of 8 blocks gives a new session in the same project with 8 blocks,
  `branched_from` set, and the source still at 8 blocks with the same rows.
- `classify_line("! cat ~/file")` is a command that runs the shell, `/!cat ~/file` still is, and a
  `!` line sent in deep research mode is not prefixed with `/research`.
- In the prototype, `node mock/scripts/organization-spike.mjs` and `node mock/scripts/sidebar-extras-check.mjs`
  exit 0.
- `cargo test` passes for `store`, `domain`, `app`, and `ui`, including new cases for the migration,
  multi-root resolution, the sidebar width cycle, the grouped palette, archive, branch, and the `!` line.

## Traceability

| Behavior | Breakdown item | Claims | Risk avoided (`risks.md`) | Spike |
| --- | --- | --- | --- | --- |
| Standalone chats and promotion | 1 | 0001, 0002 | Pathway 1 | S01 (4 of 4) |
| Projects with several directories | 2 | Structural Map | Pathways 2, 3 | S02 (4 of 4) |
| Grouped commands | 3 | 0004 | Pathway 4 | S03 (4 of 4) |
| Prompt mode control | 4 | 0005 | Pathway 5 | S04 (3 of 4, not satisfactory), S06 (4 of 4) |
| Three sidebar widths | 5 | 0006, 0007 | Pathways 6, 7 | S05 (4 of 4) |
| Hidden mark, no empty column | After the mock review | FR-23 | An empty column beside the chat | `sidebar-rail-hidden.png`, `sidebar-rail-hover.png` |
| Archive and restore | After review | Source facts 6 | None recorded | `sidebar-extras-check.mjs` (3 checks) |
| Branch a chat | After review | Source facts 7 | None recorded | `sidebar-extras-check.mjs` (2 checks) |
| Bare `!` shell line | After review | Source facts 8 | None recorded | `sidebar-extras-check.mjs` (4 checks) |
