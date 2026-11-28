# Spec 0003: chat documents, a doc panel with a page editor, a diagram editor, and `/doc` commands

Status: **proposed**. Nothing here is built in the crates. Source: the browser prototype in `mock/`
(`mock/src/docs.js`, `mock/src/doc-actions.js`, `mock/src/components/doc-panel/`,
`mock/src/lib/markdown.js`, `mock/src/lib/mermaid-view.js`) and the requirements in
[R03-chat-documents.md](../requirements/R03-chat-documents.md). There is no Specky3 discovery behind
it; the prototype is the evidence. The five decisions in R03 are confirmed (2026-11-11). This spec is
the plan to port the prototype into the crates, and three of its parts depend on spikes that have not
run (see Open work).

The requirements this implements, in plain terms and without crest's source, are in
[R03-chat-documents.md](../requirements/R03-chat-documents.md).

## Problem

Seven facts in the crates, read from the source on 2026-11-11:

1. A markdown document exists only as a block. `BlockKind::Markdown { title, source }`
   (`crates/domain/src/block.rs:336`) is drawn by `markdown::render` (`crates/ui/src/blocks/markdown.rs:27`)
   through gpui-component's `TextView` (`crates/ui/src/blocks/math.rs:403`). `TextView` has a
   `selectable` switch and no editing (gpui-component 0.7.3, `src/text/compat.rs:95`).
2. A fenced `mermaid` block inside markdown is not drawn as a diagram by that path. Diagrams come from
   their own block, `BlockKind::Mermaid(MermaidSpec)` (`block.rs:867`), through
   `mermaid::render_source` (`crates/ui/src/blocks/mermaid.rs:471`), which a plugin's
   `CustomNode::Mermaid` also calls (`crates/ui/src/blocks/custom.rs:201`). That view already zooms
   (`ZOOM_STEP` 1.25, `mermaid.rs:45`) and cuts its source at `MAX_SOURCE_CHARS` 8,000 (`mermaid.rs:47`).
3. The renderer is `rusty-mermaid` 0.2.0 (`Cargo.toml:218`). Its diagram crate has parsers for flowchart,
   sequence, pie, timeline, gantt, class, state, er, journey, mindmap, and others, and none for
   `usecase-beta`, which the prototype offers as a template (`rusty-mermaid-diagrams-0.2.0/src/lib.rs:157-175`).
4. Nothing keeps a person's own files for a chat. A session has a `workspace_root` and a `project_id`
   (`crates/domain/src/conversation.rs:401`, `:405`) and its history lives in the store. The store's
   migrations end at `0008_organize` (`crates/adapters/store/src/migrations.rs:27-68`), and its ports are
   `SessionStorePort`, `ProjectStorePort`, and `OrganizeStorePort`
   (`crates/app-core/src/ports/store.rs:113`, `:147`, `:183`).
5. The only file port, `FileSystemPort` (`crates/app-core/src/ports/fs.rs:341`), is the agent's: it
   enforces roots and sensitive paths, returns `PathError::NoRoots` for a standalone chat, and writes
   through `prepare_write` and `apply` (`fs.rs:384`, `:391`), which the permission modes gate. A person's
   own saves cannot go through an approval dialog.
6. The window has one side slot, the workspace tree (`RootView` in `crates/ui/src/app.rs:296`, filled by
   `root.set_side_panel` in `crates/ui/src/session/mod.rs:662`), and the chats sidebar over the top
   left. There is no slot for a document.
7. `CommandGroup` has six groups (`Chat`, `Flows`, `Stages`, `ModelAndPermissions`, `Workspace`, `App`;
   `crates/app-core/src/usecases/commands/catalog.rs:18`), and every `CATALOG` entry (`catalog.rs:76`)
   names one. A tool's effect is `ToolEffect::Read | Write | Execute | Network`
   (`crates/domain/src/permission.rs:90`); `Write` is what the permission modes ask about.

## Non-goals

Carried over from R03's "What this does not cover":

- No collaboration, comments, sharing, or version history beyond the editor's undo.
- No Word export, no folder export with `assets/`, and no PDF written without the print dialog.
- No table editor on the toolbar, no doc-to-doc links, no table of contents.
- No docs without a chat, and no moving a doc from one chat to another.
- A branched chat (R02 FR-20) does not copy its docs.
- No change to the Lua plugin API.

## Proposed approach

### 1. The model and the pure functions (domain)

New `crates/domain/src/doc.rs` with `DocId` and
`Doc { id, session_id, title, file_name, dir, created_at, updated_at }`. The text itself stays in the
file, so `Doc` carries no markdown.

The prototype's pure functions in `mock/src/docs.js` become Rust functions with the same cases as
tests. The mock's tests are its uses in the browser, so each function gets its first unit tests here.

| Prototype (`mock/src/docs.js`) | Rust (`domain::doc`) | Rule to keep |
|---|---|---|
| `appendMarkdown` | `append_markdown` | A blank line between the old text and the addition; an empty doc starts with the addition. |
| `findSpan`, `replaceSpan` | `find_span`, `replace_span` | Text copied from the page has lost its marks and line breaks; marks and any run of whitespace may sit between its characters. A passage that is not there leaves the text unchanged. |
| `formatSelection` (`bold`, `italic`, `code`, `heading`, `list`, `quote`, `link`, `image`) | `format_selection` | Toggles: a span that already has the marks loses them; a line format toggles its prefix on every selected line; `**x**` is bold and not italic; a link with no address returns `None`. |
| `fileName`, `docFile`, `assetFile` | `file_name`, `unique_file_name` | The title in lower-case with hyphens, `doc` when empty. Two docs of one chat with the same title get `-2`, `-3` (the prototype has no such rule). |
| `docDir`, `placeOf` | `doc_dir(session, project, data_dir)` | `<first root>/.crest/docs/<session id>` for a project chat, `<data dir>/docs/<session id>` otherwise. |
| `blockMarkdown`, `canAddToDoc` | `block_markdown(&BlockKind) -> Option<String>` | `Some` only for the kinds R03 FR-1 lists; a chart becomes `![title](assets/<name>)` when an image is given and its table otherwise. |
| `inlineAssets` | `inline_assets(markdown, assets)` | Every `](assets/<name>)` whose bytes are known becomes a `data:` URL; an unknown name stays. |
| `tableMarkdown`, `tableCsv` | `table_markdown`, `table_csv` | Pipes in cells are escaped; numeric columns right-align. |

On disk an image link is `assets/<name>`, a relative path that every markdown reader resolves (NFR-4).
The prototype's `asset:<name>` is an in-memory form, and `domain::doc` maps between the two.

`BlockView` (`crates/ui/src/blocks/mod.rs`, registered at `:126`) gains `fn doc_markdown(&self, kind) -> Option<String>`
next to `copy_formats` (`mermaid.rs:461`), so each block kind says whether and how it joins a doc.

### 2. The store and the port (R03 FR-19, FR-23 to FR-25)

Migration `0009_docs` in `crates/adapters/store/src/migrations.rs`:

- `docs (id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions ON DELETE CASCADE, title TEXT NOT NULL, file_name TEXT NOT NULL, dir TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, UNIQUE (dir, file_name))`.

`ON DELETE CASCADE` removes the rows when a chat is deleted and touches no file (R03 NFR-2). The
folder is stored as `dir` so a doc stays where it was written when a project's first directory changes;
section 3 moves it on purpose.

New `DocStorePort` in `crates/app-core/src/ports/store.rs`, next to the three store ports:
`docs()`, `docs_of(session)`, `create_doc`, `rename_doc` (title and file name), `touch_doc`,
`delete_doc` (the row only), and `set_doc_dir`. The fake in
`crates/app-testing/src/fakes/system.rs` gets the same behavior.

New `DocFilesPort` in `crates/app-core/src/ports/docs.rs`, for the files:

- `read(dir, file)`, `write(dir, file, text)`, `write_asset(dir, name, bytes)`, `read_asset(dir, name, max)`,
  and `relocate(from, to)`.
- `write` and `write_asset` create the folder, write to a temporary file in it, and rename, so a crash
  leaves the old text.
- A path is accepted only when `dir` is `<root>/.crest/docs/<id>` of a root the session has, or
  `<data dir>/docs/<id>` (`platform::dirs::data_dir`, `crates/platform/src/dirs.rs:27`). Names are the
  output of `file_name`, so `..` and separators cannot appear.
- It writes without an approval, because the author is the person. It is a separate port from
  `FileSystemPort` for that reason (Problem 5), and the agent never receives it.
- `relocate` renames the folder and, when that crosses a filesystem, copies and then removes. A failure
  leaves the old folder and the old `dir` in place and is reported.

### 3. Docs follow the chat (R03 FR-25)

A new use case `crates/app-core/src/usecases/docs.rs` owns create, add, rename, save, and relocate. It
is called from the places that already move a chat or change a project's directories:

- `move_session` (`store.rs:172`): when the session's project changes, `doc_dir` gives the new folder and
  every doc of the session moves with `relocate`, then `set_doc_dir`.
- `remove_root` (`store.rs:159`, `Project::remove_root` at `crates/domain/src/project.rs:129`): when the
  removed root was the first, the docs of the project's sessions move to the new first root.
- `branch_session` (`store.rs:199`): copies no doc.

### 4. The agent's tools (R03 FR-16, FR-17, NFR-7)

New tools in `crates/app/src/tools/`, beside the `show_*` tools in `components.rs`:

| Tool | Effect | What it does |
|---|---|---|
| `doc_list` | `Read` | The chat's docs: id, title, size. |
| `doc_read` | `Read` | A doc's markdown, by id or title. |
| `doc_write` | `Write` | Creates a doc or replaces all of one. |
| `doc_edit` | `Write` | Replaces one passage (`replace_span`), the tool the selected-passage flow uses. |

`ToolEffect::Write` makes the existing permission modes apply (NFR-7). The write goes through the
`DocFilesPort` after the approval, and the approval dialog shows the diff of the markdown as it does
for `edit_file`. A tool refuses a doc of another chat. After each write the panel reloads that doc and
shows "agent" as the author of the change.

The prompt gains two context kinds next to `@` files: a doc reference (id, title) and a doc selection
(doc id, passage). The turn passes them to the agent, and a message that carries a doc reference names
that doc as the one to change (R03 FR-22).

### 5. The doc panel and its views (R03 FR-5, FR-19, FR-20)

New module `crates/ui/src/docs/` with `panel.rs`, `page.rs`, `source.rs`, `toolbar.rs`,
`diagram_editor.rs`, and `export.rs`.

- `RootView` (`app.rs:296`) gains a `doc: Option<Entity<DocPanel>>` slot and a `DocView { Hidden, Split, Alone }`.
  Split lays the timeline and the panel side by side with a draggable divider clamped to 25-75 percent
  (the mock's `setDocSplit`). Alone gives the body to the panel and keeps the prompt and the status bar
  at the bottom, with a one-line strip of the agent's last message and "show chat".
- The tree slot and the doc panel can both be open. The tree keeps its slot, and the doc panel takes the
  timeline's area.
- New actions `panel.doc` (cycles the three views; the keymap at `crates/app-core/src/keymap.rs:703-705`
  has no `<leader>d`), `doc.new`, `doc.source`, and `doc.export`. Every button has an action name, so
  every one is reachable from the keyboard (R03 NFR-3).
- The head shows the title (a click renames), the chat's title, the saved state, "source" or "preview",
  and the export menu. A path line shows `doc_dir/file_name`.
- The sidebar gets the Docs section (`crates/ui/src/chats.rs`, `chats/rows.rs`): every doc newest first
  with its chat's title, filtered by the sidebar's search; a chat row with docs shows a document mark and
  a count. A click calls `doc.open` with view `Alone`.

### 6. The page and the source (R03 FR-6 to FR-9) — depends on spike S1

The source view is a multi-line editor over gpui-component's `InputState` (the editing engine the prompt
already uses, `crates/ui/src/input/`; the `gpui-base` dependency at `Cargo.toml:204-206`), with soft wrap and no line numbers. The toolbar
calls `format_selection` on its selection range and sets the new text and selection, as the mock does.

The rendered page, the editor the target person uses, has no counterpart in GPUI today (Problem 1).
The page splits the markdown into top-level blocks and draws each through `TextView`, with a fenced
`mermaid` block drawn by `mermaid::render_source` (Problem 2) and an image by `blocks/image.rs`. Spike S1
decides how a block is edited in place; the candidates are:

- **Block edit.** A click on a block turns that one block into a small editor over its own markdown, with
  the toolbar acting on it; Enter on a block's end or Esc leaves it and redraws. The rest of the page
  stays rendered.
- **Source beside page.** The page is read-only and sits next to the source editor, which edits.

The acceptance for R03 NFR-1 (no markdown knowledge) holds only for the first. If S1 fails it, this spec
returns to the approver before section 6 is built.

### 7. The diagram editor (R03 FR-10 to FR-15)

`diagram_editor.rs` replaces the page, as the mock's `DiagramEditor` replaces the doc area.

- Left, a `MermaidView` over the code; right, an `InputState` editor. The drawing redraws 250 ms after
  the last keystroke (`LIVE_AFTER_MS`). Code that does not parse shows the renderer's reason and the
  code, and Save still keeps it.
- Save replaces the nth diagram's source, or inserts a new block before top-level block `at`
  (`diagramEdit: { mode: "edit", index } | { mode: "new", at }` in the mock), then returns to the page.
  An empty edit removes the diagram and an empty new one adds nothing.
- The splitter is a 7 px bar between the panes, clamped to 15-85 percent, moved by the arrow keys at 5
  percent a step, reset by a double-click, and remembered in the window's state between openings. A
  panel narrower than 560 px stacks the panes.
- Zoom reuses `MermaidView`'s zoom (`mermaid.rs:45`, `:209-222`) and adds the "fit" mode as the default,
  the percentage button, the steps 25-400 percent, the wheel up to 800, and drag to pan. The prototype's
  `diagramZoom` in `doc-panel.js` is the reference for the arithmetic.
- The starting points are `DIAGRAM_TEMPLATES` in `mock/src/doc-actions.js`. Each template is offered only
  when the renderer parses it; with `rusty-mermaid` 0.2.0 that leaves out Use case (Problem 3), and
  spike S2 decides whether to wait for the renderer or draw it ourselves.
- A source over `MAX_SOURCE_CHARS` is cut for drawing and the editor says so (R03 NFR-6).

### 8. Adding, selecting, and exporting (R03 FR-1 to FR-4, FR-16, FR-18)

- The block menu next to copy gains "Add to doc" and "Add to a new doc" for the kinds whose
  `doc_markdown` returns `Some`. A chart's picture is made by the chart block's own drawing code and
  saved to `assets/` through `DocFilesPort::write_asset`. The crates have not been checked for a
  render-to-image path for charts; where there is none, the chart goes in as its table (R03 FR-3), and
  the picture waits for one.
- Selecting text in the timeline or on the page shows the bar, with Copy and "Add to doc" in the
  timeline, and Copy and "Ask agent" on the page. "Ask agent" adds the doc-selection context of section 4.
- Export: Markdown writes `inline_assets` to the file the person chooses; Web page writes the HTML the
  prototype's `docHtml` builds (the same print stylesheet, `PRINT_CSS` in `doc-actions.js`) with the
  diagrams as inline SVG from `rusty-mermaid`; PDF writes that HTML to a temporary file with a script
  that calls `window.print()` and opens it in the default browser (spike S3 confirms); Copy writes the
  markdown to the clipboard.

### 9. The `/doc` commands (R03 FR-21, FR-22)

- `CommandGroup` gains `Docs` after `Chat`, with the label "Docs" and the hint "open, search, reference"
  (`catalog.rs:18-58`; `ALL` goes from six to seven, and the palette's group view scrolls past its
  eight rows with the plugin groups, as R02 FR-13 already allows).
- `CATALOG` gains `doc` (usage `/doc [new|find|open|use|add|split|only|close|source|export] [text]`).
  The handler is the use case of section 3, and it parses the subcommands as `docCommand` does in
  `mock/src/doc-actions.js`.
- For `find`, `open`, and `use`, and for a bare `/doc <text>`, the palette lists matching docs as rows
  (`docChoices` in `mock/src/commands.js`): title, chat, and the action; "No document matches" as an
  inert row; and for `export`, the four formats. A query matched by several docs says how many and asks
  for one, and no match says so.
- The prompt's "+" menu gains "Document", which inserts `/doc `. `/doc use` adds the reference chip
  (section 4) with the document mark.

## UI placement

The mock is the reference: `mock/src/components/doc-panel/` for the panel, the toolbar, the diagram
editor, and the zoom; `mock/src/components/chats-panel/` for the Docs section; and
`mock/src/components/prompt-input/` for the chip and the palette rows. Screenshots are in
`mock/spikes/evidence/` (`docs-split.png`, `docs-toolbar.png`, `docs-selchat.png`, `docs-seldoc.png`,
`docs-commands.png`, `docs-diagram-hover.png`, `docs-diagram-resize.png`, `docs-diagram-narrow.png`,
`docs-diagram-new.png`, `docs-diagram-usecase.png`). The page editor is the one part of the mock that
GPUI cannot draw as it stands (Problem 1), which is why section 6 waits for S1.

## Open work: spikes

| Spike | Question | Done when |
|---|---|---|
| S1 | Can a rendered block be edited in place in GPUI (block edit), with the toolbar working on it and the page keeping its position? | Four of four: a click opens a block editor, bold and heading toggle on it, Esc redraws the block with the change, and a 300-block doc still scrolls at the frame rate of the chat. |
| S2 | How does crest draw `usecase-beta`? | Either `rusty-mermaid` gains a parser (version and upgrade named) or a crest-side drawing of actors, boundaries, and links is measured against the five examples on Mermaid's page. |
| S3 | Does opening an HTML file with `window.print()` in the default browser give the person a PDF on macOS, Linux, and Windows? | A PDF with the doc's text, images, and diagrams from each platform. |

## Acceptance

- After migration `0009_docs` on a store with chats, `docs()` is empty and no session changed.
- `append_markdown`, `find_span`, `replace_span`, `format_selection`, `file_name`, `unique_file_name`,
  `doc_dir`, `block_markdown`, and `inline_assets` pass the cases in the table of section 1, including a
  doc with two diagrams, a passage with marks and line breaks, and two docs titled the same.
- `create_doc` for a project chat writes `<first root>/.crest/docs/<session id>/<title>.md` and for a
  standalone chat `<data dir>/docs/<session id>/<title>.md`. `write` leaves the old text when the process
  stops between the temporary file and the rename.
- `DocFilesPort` refuses a `dir` outside those two bases and a name with a separator or `..`.
- Moving a session to another project, or removing a project's first root, moves its docs and updates
  `dir`; a failed `relocate` leaves both unchanged. Deleting the session removes the rows and leaves the
  files. Branching copies no doc.
- With the permission mode "ask", `doc_write` and `doc_edit` show a diff and wait; with accept-edits they
  apply; `doc_list` and `doc_read` never ask. A tool refuses a doc of another chat.
- `/doc` appears under a Docs group; `/doc find rel` lists the release notes; `/doc use` adds a chip and
  the next turn carries the doc; an ambiguous title asks for one and an unknown one says so.
- "Add to doc" appears for text, table, chart, computed, diff, and user blocks and for no other kind. A
  chart adds an image in `assets/` and falls back to its table.
- The diagram editor saves in place for an existing diagram, inserts after the block where the cursor was
  for a new one, removes an emptied one, and leaves the doc unchanged on Esc. The splitter stays within
  15-85 percent and survives closing and reopening the editor; the zoom steps and "fit" match R03 FR-14.
- Split keeps at least 25 percent of the width for each side at 720, 1024, and 1440 pixels.
- Spikes S1 to S3 are scored, and section 6 and the Use case template follow their results.
- `cargo test` passes for `domain`, `store`, `app-core`, `app`, and `ui`, including the cases above.

## Traceability

| Requirement | Section | Prototype evidence | Risk |
|---|---|---|---|
| FR-1 to FR-4 add from the chat | 1, 8 | `docs-selchat.png`, `docs-seldoc.png` | Chart image needs a render path in `ui` |
| FR-5, FR-19, FR-20 panel, views, sidebar | 5 | `docs-split.png` | A second side area beside the tree |
| FR-6 to FR-9 page, toolbar, source, image | 6 | `docs-toolbar.png` | `TextView` has no editing: spike S1 |
| FR-10 to FR-15 diagram editor | 7 | `docs-diagram-hover.png`, `docs-diagram-resize.png`, `docs-diagram-narrow.png`, `docs-diagram-new.png` | No `usecase-beta` in the renderer: spike S2 |
| FR-16, FR-17, NFR-7 the agent | 4 | `mock/src/doc-actions.js` (`passSelectionToAgent`) | Confirmed: the permission mode asks; the mock applies at once |
| FR-18 export | 8 | `exportDoc` in `mock/src/doc-actions.js` | PDF through a browser: spike S3 |
| FR-21, FR-22 commands and reference | 9 | `docs-commands.png`, `docs-diagram-usecase.png` | One more palette group |
| FR-23 to FR-25 files and moving | 2, 3 | `docDir` in `mock/src/docs.js` | The prototype never moves a doc; a failed move must not lose one |
