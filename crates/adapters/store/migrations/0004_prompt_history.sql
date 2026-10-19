-- 0004: the lines sent from the prompt, which `Up` brings back on resume.
--
-- A JSON array of strings, oldest first; sessions saved before it have none.

ALTER TABLE sessions ADD COLUMN prompt_history_json TEXT NOT NULL DEFAULT '[]';
