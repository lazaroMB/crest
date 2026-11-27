-- 0009: documents that belong to a chat.
--
-- Additive: a new table, nothing existing is rewritten. The text of a doc is
-- a file in `dir`; the row only says where. Deleting a chat deletes its rows
-- (and no file); a doc stays where it was written until its folder is moved
-- on purpose, so `dir` is stored.

CREATE TABLE docs (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    file_name  TEXT NOT NULL,
    dir        TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (dir, file_name)
);

CREATE INDEX docs_by_session ON docs (session_id);
