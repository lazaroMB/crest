-- 0007: projects with one or more directories, and an optional project on
-- each session.
--
-- `sessions.workspace_root` stays as it is and keeps being written; a
-- standalone session has `project_id` NULL and an empty `workspace_root`.
-- A project's directories are `project_roots` rows ordered by `position`.
-- Timestamps are Unix epoch milliseconds.
--
-- Nothing here touches `timeline_blocks` or a session's other columns. The
-- data step runs in Rust right after this script (see `migrations.rs`): each
-- distinct non-empty `workspace_root` becomes one project of one directory,
-- named after its last path component, and its sessions point at it.

CREATE TABLE projects (
    id         TEXT    PRIMARY KEY NOT NULL,
    name       TEXT    NOT NULL,
    pinned     INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE project_roots (
    project_id TEXT    NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    root       TEXT    NOT NULL,
    position   INTEGER NOT NULL,
    PRIMARY KEY (project_id, root)
) WITHOUT ROWID;

-- NULL by default, as `ADD COLUMN ... REFERENCES` requires with foreign keys
-- on. Deleting a project makes its sessions standalone, never deletes them.
ALTER TABLE sessions ADD COLUMN project_id TEXT REFERENCES projects (id) ON DELETE SET NULL;

CREATE INDEX sessions_by_project ON sessions (project_id);
