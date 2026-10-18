//! [`SimExport`] adapter: `simulation/board.html`, a single self-contained
//! page that replays the recorded runs of a simulated board with three.js,
//! with the window's look (isometric preset, orbit/zoom/pan, glowing state
//! tops and packets, edge outlines) and no network access.
//!
//! The page does not simulate (v1 replays; a WASM build of the simulator
//! is for later). Everything it shows is computed here from the domain:
//! the scene geometry (`scene_of`, the same the window draws), the trips
//! of every message (send and arrival time on its trace) and, per chip,
//! a keyframe each time its state, variables or queue change, coloured by
//! the domain's `chip_look`.
//!
//! three.js r160 is vendored in `vendor/three/0.160.0/` (MIT, see its
//! `LICENSE`) and inlined as a classic script: its single trailing
//! `export{…}` becomes `window.THREE = {…}`, so the page also works from
//! `file://`.

use std::collections::BTreeMap;

use application::ports::sim::{SimExport, SimExportError};
use domain::sysmodel::Board;
use domain::sysmodel::scene::{PACKET_Y, chip_look, message_color, scene_of};
use domain::sysmodel::sim::initial_snapshot;
use domain::sysmodel::trace::EventKind;
use serde_json::{Value, json};

/// The vendored three.js build.
const THREE: &str = include_str!("../vendor/three/0.160.0/three.module.min.js");
/// The page.
const TEMPLATE: &str = include_str!("board.html");
/// The replay script.
const REPLAY: &str = include_str!("replay.js");

/// The exporter.
#[derive(Debug, Clone, Copy, Default)]
pub struct HtmlExporter;

impl HtmlExporter {
    /// A new exporter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// three.js as a classic script defining `window.THREE`.
///
/// # Errors
///
/// When the build does not end in one `export{…}` statement.
pub fn classic_three() -> Result<String, SimExportError> {
    let start = THREE
        .rfind("export{")
        .ok_or_else(|| SimExportError("three.js: no export statement".into()))?;
    let body = &THREE[start + "export{".len()..];
    let end = body
        .find('}')
        .ok_or_else(|| SimExportError("three.js: unterminated export".into()))?;
    let mut fields = Vec::new();
    for item in body[..end].split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (local, name) = item.split_once(" as ").unwrap_or((item, item));
        fields.push(format!("{}:{}", name.trim(), local.trim()));
    }
    Ok(format!(
        "(function(){{\"use strict\";\n{}\nwindow.THREE={{{}}};}})();",
        &THREE[..start],
        fields.join(",")
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Fills `{{NAME}}` placeholders in one pass (values are not rescanned).
fn fill(template: &str, values: &BTreeMap<&str, String>) -> String {
    let mut out =
        String::with_capacity(template.len() + values.values().map(String::len).sum::<usize>());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        match after.find("}}") {
            Some(close) if values.contains_key(&after[..close]) => {
                out.push_str(&values[&after[..close]]);
                rest = &after[close + 2..];
            }
            _ => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What the page replays, as JSON.
#[must_use]
pub fn replay_data(board: &Board) -> Value {
    let model = &board.model;
    let scene = scene_of(model);
    let chip_ix: BTreeMap<&str, usize> = scene
        .chips
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    let ribbon_ix: BTreeMap<&str, usize> = scene
        .ribbons
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.as_str(), i))
        .collect();
    let scenarios: Vec<Value> = board
        .runs
        .iter()
        .map(|run| {
            let trace = &run.trace;
            let mut trips = Vec::new();
            for e in &trace.events {
                if let EventKind::Sent {
                    kind,
                    conn,
                    arrive_us,
                    ..
                } = &e.kind
                    && let Some(&r) = ribbon_ix.get(conn.as_str())
                {
                    trips.push(json!([
                        r,
                        e.at_us,
                        arrive_us,
                        kind,
                        message_color(model, kind)
                    ]));
                }
            }
            let mut snap = initial_snapshot(model);
            let key = |t: u64, id: &str, snap: &domain::sysmodel::Snapshot| {
                let state = snap.components.get(id);
                let look = chip_look(model, id, state);
                let vars = state.map(|s| s.vars.clone()).unwrap_or_default();
                let queue: Vec<&str> = state
                    .map(|s| s.queue.iter().map(|q| q.kind.as_str()).collect())
                    .unwrap_or_default();
                json!([
                    t,
                    chip_ix.get(id).copied().unwrap_or(0),
                    look.state,
                    look.emissive,
                    look.busy,
                    look.up,
                    look.queued,
                    vars,
                    queue
                ])
            };
            let mut keys: Vec<Value> = scene.chips.iter().map(|c| key(0, &c.id, &snap)).collect();
            let mut times: Vec<u64> = Vec::new();
            for e in &trace.events {
                snap.apply(e);
                if times.last() != Some(&e.at_us) {
                    times.push(e.at_us);
                }
                if !matches!(e.kind, EventKind::Sent { .. } | EventKind::Rejected { .. })
                    && chip_ix.contains_key(e.kind.comp())
                {
                    keys.push(key(e.at_us, e.kind.comp(), &snap));
                }
            }
            let verdicts: Vec<Value> = run
                .verdicts
                .iter()
                .map(|v| {
                    json!({
                        "property": v.property,
                        "passed": v.passed,
                        "detail": v.detail,
                        "at_us": v.counterexample.as_ref().map(|c| c.at_us),
                    })
                })
                .collect();
            json!({
                "id": run.scenario,
                "passed": run.passed,
                "seed": trace.seed,
                "until_us": trace.until_us.max(1),
                "verdicts": verdicts,
                "trips": trips,
                "keys": keys,
                "times": times,
            })
        })
        .collect();
    json!({
        "title": format!("Simulation · {} · {}", board.spec, board.title),
        "spec": board.spec,
        "model_hash": model.hash(),
        "packet_y": PACKET_Y,
        "scene": scene,
        "scenarios": scenarios,
    })
}

impl SimExport for HtmlExporter {
    fn board_html(&self, board: &Board) -> Result<String, SimExportError> {
        let data = serde_json::to_string(&replay_data(board))
            .map_err(|e| SimExportError(e.to_string()))?
            // Keep `</script>` (and comments) out of the inline JSON.
            .replace("</", "<\\/")
            .replace("<!--", "<\\!--");
        let values = BTreeMap::from([
            (
                "TITLE",
                html_escape(&format!("Simulation · {} · {}", board.spec, board.title)),
            ),
            ("DATA", data),
            ("THREE", classic_three()?),
            ("REPLAY", REPLAY.to_owned()),
        ]);
        Ok(fill(TEMPLATE, &values))
    }
}

#[cfg(test)]
mod tests {
    use domain::sysmodel::{SysModel, run_all};

    use super::*;

    fn board() -> Board {
        let model = SysModel::from_value(json!({
            "version": "crest.sysmodel/v1",
            "components": [
                { "id": "a", "kind": "client", "label": "A </script>",
                  "ports": [{ "id": "out", "dir": "out" }],
                  "machine": { "initial": "s", "states": ["s"] } },
                { "id": "b", "kind": "db", "ports": [{ "id": "in", "dir": "in" }],
                  "vars": { "n": 0 },
                  "machine": { "initial": "idle", "states": ["idle", "seen"], "transitions": [
                      { "from": "*", "on": "Ping", "to": "seen", "effects": [{ "set": "n", "expr": "n + 1" }] }
                  ] } }
            ],
            "connections": [{ "id": "c", "from": "a.out", "to": "b.in", "latency_ms": 4 }],
            "scenarios": [{ "id": "ping", "until_ms": 10,
                "inject": [{ "at_ms": 1, "to": "a.out", "msg": "Ping" }],
                "expect": [{ "always": "b.n < 1" }] }]
        }))
        .unwrap();
        let runs = run_all(&model).unwrap();
        Board {
            spec: "0001-x".into(),
            title: "Ping <b>".into(),
            model,
            runs,
        }
    }

    #[test]
    fn three_becomes_a_classic_script() {
        let js = classic_three().unwrap();
        assert!(js.starts_with("(function(){\"use strict\";"));
        assert!(js.contains("window.THREE={"));
        assert!(js.contains("WebGLRenderer:"));
        assert!(js.contains("OrthographicCamera:"));
        assert!(!js.contains("export{"));
        assert!(!js.to_lowercase().contains("</script"));
    }

    #[test]
    fn the_data_holds_trips_keyframes_and_verdicts() {
        let data = replay_data(&board());
        let s = &data["scenarios"][0];
        assert_eq!(s["id"], "ping");
        assert_eq!(s["passed"], false);
        assert_eq!(s["trips"].as_array().unwrap().len(), 1);
        let trip = &s["trips"][0];
        assert_eq!(trip[1], 1000);
        assert_eq!(trip[2], 5000);
        let keys = s["keys"].as_array().unwrap();
        // One key per chip at 0, then b starts and handles Ping.
        assert_eq!(keys[0][0], 0);
        assert!(keys.iter().any(|k| k[2] == "seen" && k[7]["n"] == 1));
        assert_eq!(s["verdicts"][0]["at_us"], 5000);
        assert_eq!(data["scene"]["chips"].as_array().unwrap().len(), 2);
        assert_eq!(s["times"], json!([1000, 5000]));
    }

    #[test]
    fn the_page_is_self_contained_and_escaped() {
        let html = HtmlExporter::new().board_html(&board()).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>Simulation · 0001-x · Ping &lt;b&gt;</title>"));
        assert!(!html.contains("{{"), "every placeholder filled");
        assert!(!html.contains("<script src"));
        // The label with `</script>` stays inside the JSON.
        assert_eq!(html.matches("</script>").count(), 3);
        assert!(html.contains("A <\\/script>"));
        let json_start = html
            .find("<script type=\"application/json\" id=\"data\">")
            .unwrap();
        let json = &html[json_start..];
        let json = &json[json.find('>').unwrap() + 1..json.find("</script>").unwrap()];
        let back: Value = serde_json::from_str(json).unwrap();
        assert_eq!(back["spec"], "0001-x");
    }

    #[test]
    fn placeholders_are_filled_once() {
        let values = BTreeMap::from([("A", "{{B}}".to_owned()), ("B", "b".to_owned())]);
        assert_eq!(fill("x{{A}}y{{B}}{{C}}", &values), "x{{B}}yb{{C}}");
    }
}
