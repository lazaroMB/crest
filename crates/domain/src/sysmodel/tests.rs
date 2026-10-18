use serde_json::json;

use super::check::{Origin, flow_paths};
use super::layout::{CELL, layout};
use super::lint::{Severity, uncovered};
use super::model::*;
use super::scene::{build_scene, frame_at};
use super::sim::{SplitMix64, simulate_with_seed};
use super::trace::{DropReason, EventKind};
use super::*;

/// client → api → db and back: api allows 2 at a time and queues 1.
fn model_json() -> serde_json::Value {
    json!({
        "version": "crest.sysmodel/v1",
        "spec_ref": { "solution": "0001-demo", "evidence": ["E1"] },
        "components": [
            {
                "id": "client", "kind": "client", "cites": ["black-box: user"],
                "ports": [
                    { "id": "out", "dir": "out", "emits": ["Req"] },
                    { "id": "in", "dir": "in", "accepts": ["Resp"] }
                ],
                "vars": { "done": 0 },
                "machine": {
                    "initial": "waiting", "states": ["waiting", "done"],
                    "transitions": [
                        { "from": "*", "on": "Resp", "to": "done",
                          "effects": [{ "set": "done", "expr": "done + 1" }] }
                    ]
                }
            },
            {
                "id": "api", "kind": "service", "cites": ["E1"],
                "ports": [
                    { "id": "in", "dir": "in", "accepts": ["Req"] },
                    { "id": "db", "dir": "out", "emits": ["Query"] },
                    { "id": "rows", "dir": "in", "accepts": ["Rows"] },
                    { "id": "out", "dir": "out", "emits": ["Resp"] }
                ],
                "capacity": { "concurrency": 2, "queue": 1, "on_full": "reject" },
                "latency": { "dist": "uniform", "ms": [1, 3] },
                "vars": { "inflight": 0 },
                "machine": {
                    "initial": "idle", "states": ["idle", "busy"],
                    "transitions": [
                        { "from": "*", "on": "Req", "to": "busy",
                          "effects": [
                              { "set": "inflight", "expr": "inflight + 1" },
                              { "emit": "Query", "port": "db" }
                          ] },
                        { "from": "busy", "on": "Rows", "guard": "inflight > 1", "to": "busy",
                          "effects": [
                              { "set": "inflight", "expr": "inflight - 1" },
                              { "emit": "Resp", "port": "out" }
                          ] },
                        { "from": "busy", "on": "Rows", "to": "idle",
                          "effects": [
                              { "set": "inflight", "expr": "inflight - 1" },
                              { "emit": "Resp", "port": "out" }
                          ] }
                    ]
                }
            },
            {
                "id": "db", "kind": "db", "cites": ["E1"],
                "ports": [
                    { "id": "in", "dir": "in", "accepts": ["Query"] },
                    { "id": "out", "dir": "out", "emits": ["Rows"] }
                ],
                "latency": { "dist": "fixed", "ms": 5 },
                "machine": {
                    "initial": "up", "states": ["up"],
                    "transitions": [
                        { "from": "up", "on": "Query", "effects": [{ "emit": "Rows", "port": "out" }] }
                    ]
                }
            }
        ],
        "connections": [
            { "id": "c1", "from": "client.out", "to": "api.in", "latency_ms": 1 },
            { "id": "c2", "from": "api.db", "to": "db.in", "latency_ms": 1 },
            { "id": "c3", "from": "db.out", "to": "api.rows", "latency_ms": 1 },
            { "id": "c4", "from": "api.out", "to": "client.in", "latency_ms": 1 }
        ],
        "invariants": ["api.inflight >= 0"],
        "scenarios": [
            {
                "id": "one_request", "seed": 7, "until_ms": 100,
                "inject": [{ "at_ms": 0, "to": "client.out", "msg": "Req" }],
                "expect": [
                    { "eventually": "client.state == 'done'", "within_ms": 50 },
                    { "path": ["client", "api", "db", "api", "client"] },
                    { "count": "Resp", "at": "client", "min": 1, "max": 1 },
                    { "always": "api.inflight <= 2" }
                ],
                "covers": ["T1"]
            },
            {
                "id": "burst", "seed": 42, "until_ms": 200,
                "inject": [{ "at_ms": 0, "to": "client.out", "msg": "Req",
                             "repeat": { "every_ms": 0, "count": 1 } },
                           { "at_ms": 0, "to": "client.out", "msg": "Req",
                             "repeat": { "every_ms": 1, "count": 5 } }],
                "expect": [
                    { "always": "api.inflight <= 1" },
                    { "count": "Req", "at": "api", "event": "rejected", "max": 0 },
                    { "never": "api.queue > 1" }
                ],
                "covers": ["T2"]
            }
        ]
    })
}

fn model() -> SysModel {
    SysModel::from_value(model_json()).unwrap()
}

#[test]
fn the_schema_rejects_unknown_fields_and_versions() {
    let mut v = model_json();
    v["components"][0]["colour"] = json!("red");
    assert!(SysModel::from_value(v).is_err());
    let mut v = model_json();
    v["version"] = json!("crest.sysmodel/v2");
    assert!(SysModel::from_value(v).is_err());
    let mut v = model_json();
    v["components"][0]["kind"] = json!("mainframe");
    assert!(SysModel::from_value(v).is_err());
    let mut v = model_json();
    v["components"][1]["machine"]["transitions"][0]["effects"][0] =
        json!({ "set": "x", "to": "y" });
    assert!(SysModel::from_value(v).is_err());
    let schema = schema();
    assert_eq!(schema["title"], "SysModel");
    let text = schema.to_string();
    assert!(text.contains("crest.sysmodel/v1"));
    assert!(text.contains("additionalProperties"));
}

#[test]
fn the_model_round_trips() {
    let m = model();
    let back = SysModel::from_json(&m.to_json()).unwrap();
    assert_eq!(back, m);
    assert_eq!(back.hash(), m.hash());
    assert_eq!(m.hash().len(), 16);
}

#[test]
fn a_clean_model_has_no_lint_errors() {
    let issues = lint(&model());
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn lint_reports_broken_references_and_types() {
    let mut v = model_json();
    v["components"][1]["machine"]["transitions"][0]["guard"] = json!("inflight + 1");
    v["components"][1]["machine"]["transitions"][0]["effects"][1]["port"] = json!("nope");
    v["components"][1]["machine"]["transitions"][1]["guard"] = json!("db.x > 1");
    v["components"][1]["machine"]["transitions"][2]["to"] = json!("gone");
    v["components"][1]["machine"]["states"] = json!(["idle", "busy", "lonely"]);
    v["components"][1]["vars"]["state"] = json!(1);
    v["connections"][1]["to"] = json!("db.out");
    v["scenarios"][0]["expect"][0] = json!({ "always": "inflight < 2" });
    v["scenarios"][0]["expect"][1] = json!({ "path": ["client", "ghost"] });
    v["components"][2]["cites"] = json!([]);
    let issues = lint(&SysModel::from_value(v).unwrap());
    let codes: Vec<(&str, Severity)> = issues
        .iter()
        .map(|i| (i.code.as_str(), i.severity))
        .collect();
    for want in [
        ("type", Severity::Error),
        ("unknown-port", Severity::Error),
        ("foreign-name", Severity::Error),
        ("unknown-state", Severity::Error),
        ("reserved", Severity::Error),
        ("port-direction", Severity::Error),
        ("unknown-component", Severity::Error),
        ("unreachable-state", Severity::Warning),
        ("no-citation", Severity::Warning),
    ] {
        assert!(codes.contains(&want), "{want:?} missing in {issues:#?}");
    }
    assert_eq!(issues[0].severity, Severity::Error, "errors come first");
    let assertion = issues
        .iter()
        .find(|i| i.at == "scenarios[one_request].expect[0]")
        .unwrap();
    assert!(assertion.message.contains("component.var"), "{assertion}");
}

#[test]
fn lint_warns_about_unhandled_messages_and_uncovered_items() {
    let mut v = model_json();
    v["components"][2]["machine"]["transitions"][0]["on"] = json!("Other");
    let issues = lint(&SysModel::from_value(v).unwrap());
    assert!(issues.iter().any(|i| i.code == "unhandled-message"));
    let items = vec!["T1".to_owned(), "T9".to_owned()];
    let gaps = uncovered(&model(), &items);
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].message.contains("T9"));
}

fn render(trace: &Trace) -> String {
    let mut out = format!(
        "scenario {} seed {} model {} events {}\n",
        trace.scenario,
        trace.seed,
        trace.model_hash,
        trace.events.len()
    );
    for e in &trace.events {
        out.push_str(&format!("{:>7} {}\n", e.at_us, e.kind.describe()));
    }
    out
}

#[test]
fn the_same_seed_gives_the_same_trace() {
    let m = model();
    for s in &m.scenarios {
        let a = simulate(&m, s).unwrap();
        let b = simulate(&m, s).unwrap();
        assert_eq!(a.to_json(), b.to_json());
        assert_eq!(a.hash(), b.hash());
    }
    let s = &m.scenarios[1];
    let other = simulate_with_seed(&m, s, 43).unwrap();
    assert_ne!(other.hash(), simulate(&m, s).unwrap().hash());
}

#[test]
fn golden_trace_of_one_request() {
    let m = model();
    let trace = simulate(&m, &m.scenarios[0]).unwrap();
    insta::assert_snapshot!(render(&trace));
}

#[test]
fn splitmix_matches_its_reference_values() {
    // Reference output of SplitMix64 seeded with 0.
    let mut rng = SplitMix64::new(0);
    assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
    assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    let mut rng = SplitMix64::new(9);
    for _ in 0..100 {
        let v = rng.between(3, 5);
        assert!((3..=5).contains(&v));
    }
    assert_eq!(rng.between(4, 4), 4);
}

#[test]
fn one_request_passes_every_check() {
    let m = model();
    let run = run_scenario(&m, &m.scenarios[0]).unwrap();
    assert!(run.passed, "{:#?}", run.verdicts);
    assert_eq!(run.verdicts.len(), 5);
    assert_eq!(run.verdicts[0].origin, Origin::Invariant(0));
    let paths = flow_paths(&run.trace);
    let names: Vec<&str> = paths[&1].iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(names, ["client", "api", "db", "api", "client"]);
}

#[test]
fn the_burst_fails_with_counterexamples() {
    let m = model();
    let run = run_scenario(&m, &m.scenarios[1]).unwrap();
    assert!(!run.passed);
    let always = &run.verdicts[1];
    assert_eq!(always.origin, Origin::Expect(0));
    assert!(!always.passed);
    let cx = always.counterexample.as_ref().unwrap();
    assert_eq!(cx.snapshot.components["api"].vars["inflight"], 2);
    assert!(cx.event > 0);
    let rejected = &run.verdicts[2];
    assert!(!rejected.passed, "{rejected:?}");
    assert!(rejected.detail.starts_with("counted "));
    assert!(
        run.trace
            .events
            .iter()
            .any(|e| matches!(&e.kind, EventKind::Enqueued { comp, .. } if comp == "api"))
    );
}

#[test]
fn snapshots_rebuild_any_instant() {
    let m = model();
    let mut s = m.scenarios[1].clone();
    s.inject[1].repeat = Some(Repeat {
        every_ms: 1,
        count: 200,
    });
    let trace = simulate(&m, &s).unwrap();
    assert!(!trace.checkpoints.is_empty());
    let mut snap = trace.initial.clone();
    for (i, e) in trace.events.iter().enumerate() {
        snap.apply(e);
        if i % 97 == 0 {
            assert_eq!(trace.snapshot_after(i + 1), snap);
        }
    }
    assert_eq!(trace.snapshot_after(trace.events.len()), snap);
    let t = trace.events[40].at_us;
    assert_eq!(
        trace.snapshot_at(t),
        trace.snapshot_after(trace.events_until(t))
    );
    assert!(trace.next_event_after(t).unwrap() > t);
    assert!(trace.prev_event_before(t).unwrap() < t);
}

#[test]
fn faults_drop_messages_until_the_restart() {
    let m = model();
    let mut s = m.scenarios[0].clone();
    s.faults = vec![Fault {
        at_ms: 0,
        kill: "db".into(),
        for_ms: Some(50),
    }];
    s.inject.push(Inject {
        at_ms: 60,
        to: "client.out".into(),
        msg: "Req".into(),
        repeat: None,
    });
    let trace = simulate(&m, &s).unwrap();
    let kinds: Vec<&EventKind> = trace.events.iter().map(|e| &e.kind).collect();
    assert!(matches!(kinds[0], EventKind::Killed { comp, .. } if comp == "db"));
    assert!(kinds.iter().any(|k| matches!(
        k,
        EventKind::Dropped { comp, reason: DropReason::Down, .. } if comp == "db"
    )));
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EventKind::Restarted { comp, .. } if comp == "db"))
    );
    let run = run_scenario(&m, &s).unwrap();
    assert!(
        run.verdicts
            .iter()
            .any(|v| v.property.starts_with("eventually") && !v.passed)
    );
}

#[test]
fn a_guard_error_stops_the_run() {
    let mut v = model_json();
    v["components"][1]["machine"]["transitions"][0]["guard"] = json!("1 / inflight > 0");
    let m = SysModel::from_value(v).unwrap();
    let run = run_scenario(&m, &m.scenarios[0]).unwrap();
    assert!(!run.passed);
    assert_eq!(run.verdicts[0].origin, Origin::Run);
    assert!(run.verdicts[0].detail.contains("division by zero"));
}

#[test]
fn timers_send_to_themselves() {
    let m = SysModel::from_value(json!({
        "version": "crest.sysmodel/v1",
        "components": [{
            "id": "clock", "kind": "timer",
            "ports": [{ "id": "start", "dir": "in", "accepts": ["Tick"] }],
            "vars": { "ticks": 0 },
            "machine": { "initial": "on", "states": ["on"], "transitions": [
                { "from": "on", "on": "Tick", "guard": "ticks < 4",
                  "effects": [{ "set": "ticks", "expr": "ticks + 1" },
                              { "emit": "Tick", "port": "self", "after_ms": 10 }] }
            ] }
        }],
        "scenarios": [{ "id": "s", "until_ms": 1000,
            "inject": [{ "at_ms": 0, "to": "clock.start", "msg": "Tick" }],
            "expect": [{ "eventually": "clock.ticks == 4", "within_ms": 35 },
                       { "count": "Tick", "at": "clock", "event": "unhandled", "min": 1 }] }]
    }))
    .unwrap();
    let run = run_scenario(&m, &m.scenarios[0]).unwrap();
    assert!(run.passed, "{:#?}", run.verdicts);
    let mut late = m.scenarios[0].clone();
    late.expect = vec![Expectation::Eventually(EventuallyExpect {
        eventually: "clock.ticks == 4".into(),
        within_ms: 25,
    })];
    let run = run_scenario(&m, &late).unwrap();
    assert!(!run.passed);
    assert_eq!(
        run.verdicts[0].counterexample.as_ref().unwrap().at_us,
        25_000
    );
}

#[test]
fn layout_places_columns_by_flow_and_routes_manhattan() {
    let m = model();
    let l = layout(&m);
    assert_eq!(l.cells["client"].col, 0);
    assert_eq!(l.cells["api"].col, 1);
    assert_eq!(l.cells["db"].col, 2);
    assert_eq!(l.rows, 1);
    assert_eq!(l.routes.len(), 4);
    for (id, route) in &l.routes {
        assert!(route.len() >= 2, "{id}");
        for w in route.windows(2) {
            let straight = (w[0].0 - w[1].0).abs() < 1e-4 || (w[0].1 - w[1].1).abs() < 1e-4;
            assert!(straight, "{id}: {route:?}");
        }
        assert_eq!(
            route.first(),
            l.pins
                .get(&m.connections.iter().find(|c| &c.id == id).unwrap().from)
        );
    }
    // c3 (db → api) goes back, under the row.
    assert!(l.routes["c3"].iter().any(|p| p.1 > CELL / 2.0));
    let mut hinted = m.clone();
    hinted.components[2].layout = Some(GridPos { col: 0, row: 3 });
    let l = layout(&hinted);
    assert_eq!(l.cells["db"], GridPos { col: 0, row: 3 });
    assert_eq!(l.rows, 4);
}

#[test]
fn the_scene_and_frames_follow_the_trace() {
    let m = model();
    let scene = build_scene(&m, &layout(&m));
    assert_eq!(scene.chips.len(), 3);
    assert_eq!(scene.ribbons.len(), 4);
    assert_eq!(scene.labels.len(), 3);
    assert!(scene.board.min[0] < scene.chips[0].min()[0]);
    let trace = simulate(&m, &m.scenarios[0]).unwrap();
    let sent = trace
        .events
        .iter()
        .find(|e| matches!(e.kind, EventKind::Sent { .. }))
        .unwrap();
    let EventKind::Sent { arrive_us, .. } = sent.kind else {
        unreachable!()
    };
    let mid = (sent.at_us + arrive_us) / 2;
    let frame = frame_at(&m, &scene, &trace, mid);
    assert_eq!(frame.packets.len(), 1);
    let start = scene.ribbons[0].points[0];
    let end = *scene.ribbons[0].points.last().unwrap();
    let p = frame.packets[0].at;
    assert!(p[0] > start[0] && p[0] < end[0], "{p:?}");
    assert_eq!(frame, frame_at(&m, &scene, &trace, mid), "pure in time");
    let end_frame = frame_at(&m, &scene, &trace, trace.until_us);
    assert!(end_frame.packets.is_empty());
    assert_eq!(end_frame.chips[0].state, "done");
}

#[test]
fn boards_report_overall_results() {
    let m = model();
    let runs = run_all(&m).unwrap();
    let board = Board {
        spec: "0001-demo".into(),
        title: "Demo".into(),
        model: m,
        runs,
    };
    assert!(!board.passed());
    assert!(board.run("one_request").unwrap().passed);
    let json = serde_json::to_string(&board).unwrap();
    let back: Board = serde_json::from_str(&json).unwrap();
    assert_eq!(back, board);
}
