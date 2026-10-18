-- 0002: number of turns as a column, so listings need not decode data_json.
--
-- Existing rows are backfilled from the JSON; a row whose JSON is not valid
-- gets 0 here and is reported as corrupt when loaded.

ALTER TABLE sessions ADD COLUMN turn_count INTEGER NOT NULL DEFAULT 0;

UPDATE sessions
SET turn_count = CASE
    WHEN json_valid(data_json) THEN coalesce(json_array_length(data_json, '$.turns'), 0)
    ELSE 0
END;
