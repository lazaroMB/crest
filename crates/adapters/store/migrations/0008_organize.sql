-- 0008: pin, archive and branch chats; archive projects.
--
-- Additive: every column has a default (or is NULL), so chats and projects
-- saved by the previous version keep working and nothing is rewritten.
-- Archiving never touches `timeline_blocks`. `branched_from` is cleared when
-- the original chat is deleted; `branched_at` is the number of blocks the
-- branch copied.

ALTER TABLE sessions ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN branched_from TEXT REFERENCES sessions (id) ON DELETE SET NULL;
ALTER TABLE sessions ADD COLUMN branched_at INTEGER;

ALTER TABLE projects ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
