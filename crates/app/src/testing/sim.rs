//! A small Spec and the model answers that simulate it: client → api → db
//! with one scenario that passes (`one_request`) and one that fails
//! (`burst`: the api holds two requests while the assertion allows one).

use serde_json::{Value, json};

use super::llm::ScriptedResponse;

/// Folder of the fixture Spec.
pub const FOLDER: &str = "0001-orders-api";

/// `spec.md` of the fixture.
pub const SPEC_MD: &str = "# Spec 0001: Orders API\n\n\
## 1. Problem\n\nOrders take too long to answer [E1].\n\n\
## 4. Proposed solution\n\nAn API in front of the database.\n\n\
```mermaid\nflowchart LR\n  client[Client] --> api[Orders API]\n  api --> db[(Postgres)]\n```\n\n\
## 5. Functional specification\n\nThe API reads the order and answers.\n\n### Sequence\n\n\
```mermaid\nsequenceDiagram\n  Client->>API: Req\n  API->>DB: Query\n  DB-->>API: Rows\n  API-->>Client: Resp\n```\n\n\
### States\n\n```mermaid\nstateDiagram-v2\n  [*] --> idle\n  idle --> busy: Req\n  busy --> idle: Rows\n```\n\n\
## 8. Test protocol\n\n- T1: one request is answered within 50 ms through the database (H1).\n\
- T2: a burst of six requests never has more than one in flight.\n\n\
## 10. Sources\n\n- **[E1]** Orders are slow: `src/orders.rs`\n";

/// `sources.json` of the fixture: one evidence record.
pub const SOURCES_JSON: &str = "[\n  {\n    \"id\": \"E1\",\n    \"claim\": \"Orders are slow.\",\n    \"source\": { \"type\": \"file\", \"path\": \"src/orders.rs\" },\n    \"collected_at\": 1\n  }\n]\n";

/// The frozen plan the first call answers.
#[must_use]
pub fn plan() -> Value {
    json!({
        "components": [
            { "id": "client", "kind": "client", "stands_for": "Client" },
            { "id": "api", "kind": "service", "stands_for": "Orders API" },
            { "id": "db", "kind": "db", "stands_for": "Postgres" }
        ],
        "scenarios": [
            {
                "id": "one_request",
                "description": "One request goes through the database and back.",
                "covers": ["T1", "H1"],
                "expect": [
                    { "eventually": "client.state == 'done'", "within_ms": 50 },
                    { "path": ["client", "api", "db", "api", "client"] }
                ]
            },
            {
                "id": "burst",
                "description": "Six requests at once.",
                "covers": ["T2"],
                "expect": [ { "always": "api.inflight <= 1" } ]
            }
        ]
    })
}

/// The model the second call answers.
#[must_use]
pub fn model() -> Value {
    json!({
        "version": "crest.sysmodel/v1",
        "spec_ref": { "solution": FOLDER, "evidence": ["E1"] },
        "components": [
            {
                "id": "client", "kind": "client", "cites": ["Client"],
                "ports": [
                    { "id": "out", "dir": "out", "emits": ["Req"] },
                    { "id": "in", "dir": "in", "accepts": ["Resp"] }
                ],
                "machine": { "initial": "waiting", "states": ["waiting", "done"],
                    "transitions": [ { "from": "*", "on": "Resp", "to": "done" } ] }
            },
            {
                "id": "api", "kind": "service", "cites": ["Orders API", "E1"],
                "ports": [
                    { "id": "in", "dir": "in", "accepts": ["Req"] },
                    { "id": "db", "dir": "out", "emits": ["Query"] },
                    { "id": "rows", "dir": "in", "accepts": ["Rows"] },
                    { "id": "out", "dir": "out", "emits": ["Resp"] }
                ],
                "capacity": { "concurrency": 2, "queue": 8 },
                "latency": { "dist": "uniform", "ms": [1, 3] },
                "vars": { "inflight": 0 },
                "machine": { "initial": "idle", "states": ["idle", "busy"],
                    "transitions": [
                        { "from": "*", "on": "Req", "to": "busy", "effects": [
                            { "set": "inflight", "expr": "inflight + 1" },
                            { "emit": "Query", "port": "db" } ] },
                        { "from": "busy", "on": "Rows", "guard": "inflight > 1", "effects": [
                            { "set": "inflight", "expr": "inflight - 1" },
                            { "emit": "Resp", "port": "out" } ] },
                        { "from": "busy", "on": "Rows", "to": "idle", "effects": [
                            { "set": "inflight", "expr": "inflight - 1" },
                            { "emit": "Resp", "port": "out" } ] }
                    ] }
            },
            {
                "id": "db", "kind": "db", "cites": ["Postgres"],
                "ports": [
                    { "id": "in", "dir": "in", "accepts": ["Query"] },
                    { "id": "out", "dir": "out", "emits": ["Rows"] }
                ],
                "latency": { "dist": "fixed", "ms": 5 },
                "machine": { "initial": "up", "states": ["up"],
                    "transitions": [ { "from": "up", "on": "Query",
                        "effects": [ { "emit": "Rows", "port": "out" } ] } ] }
            }
        ],
        "connections": [
            { "id": "req", "from": "client.out", "to": "api.in", "latency_ms": 1 },
            { "id": "query", "from": "api.db", "to": "db.in", "latency_ms": 1 },
            { "id": "rows", "from": "db.out", "to": "api.rows", "latency_ms": 1 },
            { "id": "resp", "from": "api.out", "to": "client.in", "latency_ms": 1 }
        ],
        "scenarios": [
            { "id": "one_request", "seed": 7, "until_ms": 100,
              "inject": [ { "at_ms": 0, "to": "client.out", "msg": "Req" } ],
              "expect": [
                  { "eventually": "client.state == 'done'", "within_ms": 50 },
                  { "path": ["client", "api", "db", "api", "client"] }
              ] },
            { "id": "burst", "seed": 42, "until_ms": 200,
              "inject": [ { "at_ms": 0, "to": "client.out", "msg": "Req",
                            "repeat": { "every_ms": 1, "count": 6 } } ],
              "expect": [ { "always": "api.inflight <= 1" } ] }
        ]
    })
}

/// The two answers of a simulation of the fixture, as forced tool calls.
#[must_use]
pub fn script() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::tool_use("sim1", "submit_assertions", plan()),
        ScriptedResponse::tool_use("sim2", "submit_sysmodel", model()),
    ]
}
