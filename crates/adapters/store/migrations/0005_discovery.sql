-- 0005: the discovery engine (`/research` on the event-sourced engine).
--
-- `discoveries` names each discovery of a workspace with its request and
-- times; a listing replays each one's events for its status, which stays
-- cheap while logs hold tens of events. `discovery_events` is the
-- append-only log: one row per event, `seq` 1-based and dense, the whole
-- `Logged` as JSON in `event_json` (its `event` tag copied to `kind` for
-- inspection). The
-- adapter replays and judges before every insert, so no row the domain
-- rejects is ever written. `discovery_artifacts` holds the texts the engine
-- produced (sections, records, spikes, drafts) by name;
-- `discovery_sources` the text of every source read, with its hash.
-- Times are Unix epoch milliseconds.

CREATE TABLE discoveries (
    id             TEXT    PRIMARY KEY NOT NULL,
    workspace_root TEXT    NOT NULL,
    problem        TEXT    NOT NULL,
    level          TEXT    NOT NULL,
    started_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE INDEX discoveries_by_workspace ON discoveries (workspace_root, started_at DESC);

CREATE TABLE discovery_events (
    discovery_id TEXT    NOT NULL REFERENCES discoveries (id) ON DELETE CASCADE,
    seq          INTEGER NOT NULL,
    at           INTEGER NOT NULL,
    kind         TEXT    NOT NULL,
    event_json   TEXT    NOT NULL,
    PRIMARY KEY (discovery_id, seq)
);

CREATE TABLE discovery_artifacts (
    discovery_id TEXT NOT NULL REFERENCES discoveries (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    text         TEXT NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (discovery_id, name)
);

CREATE TABLE discovery_sources (
    discovery_id TEXT    NOT NULL REFERENCES discoveries (id) ON DELETE CASCADE,
    source_id    INTEGER NOT NULL,
    hash         TEXT    NOT NULL,
    location     TEXT    NOT NULL,
    title        TEXT,
    kind         TEXT    NOT NULL,
    fetched_at   INTEGER NOT NULL,
    text         TEXT    NOT NULL,
    PRIMARY KEY (discovery_id, source_id)
);
