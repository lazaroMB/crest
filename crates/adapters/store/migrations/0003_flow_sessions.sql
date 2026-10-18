-- 0003: checkpoints of graph-flow sessions (the investigative flow).
--
-- One row per flow session. `data_json` holds the whole serialized
-- `graph_flow::Session` (its `Context` included); `graph_id`,
-- `current_task_id` and `status_message` are copies for listing and
-- inspection without decoding it. `version` is the optimistic-locking
-- counter graph-flow requires and is authoritative over the one inside the
-- JSON. `updated_at` is Unix epoch milliseconds. Independent of `sessions`:
-- a flow is linked to a chat session by the application, not by a key here.

CREATE TABLE flow_sessions (
    id              TEXT    PRIMARY KEY NOT NULL,
    graph_id        TEXT    NOT NULL,
    current_task_id TEXT    NOT NULL,
    status_message  TEXT,
    version         INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    data_json       TEXT    NOT NULL
);

CREATE INDEX flow_sessions_by_graph ON flow_sessions (graph_id, updated_at DESC);
