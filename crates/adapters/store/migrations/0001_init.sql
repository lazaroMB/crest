-- 0001: sessions, their timeline and their "allow always" rules.
--
-- `data_json` holds the full serialized value; the other columns are copies
-- used for listing and ordering without decoding it. Timestamps are Unix
-- epoch milliseconds. Child rows go away with their session.

CREATE TABLE sessions (
    id             TEXT    PRIMARY KEY NOT NULL,
    workspace_root TEXT    NOT NULL,
    title          TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    data_json      TEXT    NOT NULL
);

CREATE INDEX sessions_by_workspace
    ON sessions (workspace_root, updated_at DESC, created_at DESC, id DESC);

CREATE TABLE timeline_blocks (
    session_id TEXT    NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    block_id   TEXT    NOT NULL,
    kind       TEXT    NOT NULL,
    data_json  TEXT    NOT NULL,
    PRIMARY KEY (session_id, seq)
) WITHOUT ROWID;

-- Rules are stored per session because `StoredSession` carries them; the
-- workspace-wide view is the union over the sessions of a workspace.
CREATE TABLE permission_rules (
    session_id TEXT    NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    rule_json  TEXT    NOT NULL,
    PRIMARY KEY (session_id, seq)
) WITHOUT ROWID;
