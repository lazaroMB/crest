-- 0006: the listing summary of a discovery, kept on its row.
--
-- A listing used to replay every discovery's whole event log to learn its
-- status. The adapter now maintains the replayed summary on the
-- `discoveries` row, in the transaction that appends the events:
-- `status` ('open', 'accepted' or 'aborted'), `open_section` (the open
-- section's snake_case name, NULL when none) and `event_count`.
-- `status` NULL means "not summarised": the log did not replay when this
-- migration ran, and a listing leaves such a discovery out. Existing rows
-- are backfilled in Rust right after this script (see `migrations.rs`).

ALTER TABLE discoveries ADD COLUMN status TEXT;
ALTER TABLE discoveries ADD COLUMN open_section TEXT;
ALTER TABLE discoveries ADD COLUMN event_count INTEGER NOT NULL DEFAULT 0;

CREATE INDEX discoveries_open ON discoveries (workspace_root, started_at DESC)
    WHERE status = 'open';
