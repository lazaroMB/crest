use std::sync::Arc;

use domain::model::ModelPolicy;
use domain::research::Evidence;
use domain::sysmodel::Board;
use domain::workspace::WorkspaceRoot;
use serde_json::json;

use super::*;
use crate::ports::sim::SimExportError;
use crate::testing::fs::InMemoryFs;
use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
use crate::testing::sim::{self, FOLDER, SOURCES_JSON, SPEC_MD};
use crate::testing::system::RecordingSink;

struct FakeExport;

impl SimExport for FakeExport {
    fn board_html(&self, board: &Board) -> Result<String, SimExportError> {
        Ok(format!("<html>{}</html>", board.spec))
    }
}

struct Harness {
    fs: Arc<InMemoryFs>,
    llm: Arc<ScriptedLlm>,
    sink: Arc<RecordingSink>,
    simulator: Simulator,
}

fn harness(script: Vec<ScriptedResponse>) -> Harness {
    let fs = Arc::new(
        InMemoryFs::new(WorkspaceRoot::new(std::env::temp_dir()).unwrap())
            .with_file(&format!(".crest/specs/{FOLDER}/spec.md"), SPEC_MD)
            .with_file(&format!(".crest/specs/{FOLDER}/sources.json"), SOURCES_JSON)
            .with_file(
                &format!(".crest/specs/{FOLDER}/journal.md"),
                "# Research journal\n\n## 12. Spec\n\n- `2026-10-01 10:00` Written.\n",
            )
            .with_file(".crest/specs/0002-other/spec.md", "# Spec 0002: Other\n"),
    );
    let llm = Arc::new(ScriptedLlm::new(script));
    let sink = Arc::new(RecordingSink::new());
    let simulator = Simulator::new(SimulateDeps {
        llm: llm.clone(),
        models: ModelPolicy::default(),
        fs: fs.clone(),
        search: fs.clone(),
        events: Some(sink.clone()),
        export: Some(Arc::new(FakeExport)),
    });
    Harness {
        fs,
        llm,
        sink,
        simulator,
    }
}

fn request(spec: &str) -> SimulateRequest {
    SimulateRequest {
        spec: spec.to_owned(),
        now_ms: 1_790_000_000_000,
        open_board: true,
    }
}

fn file(h: &Harness, name: &str) -> Option<String> {
    h.fs.file(&format!(".crest/specs/{FOLDER}/{name}"))
}

#[test]
fn folders_resolve_by_name_number_or_path() {
    let folders = vec!["0001-orders-api".to_owned(), "0012-cache".to_owned()];
    assert_eq!(
        resolve_folder(&folders, "0001-orders-api").as_deref(),
        Some("0001-orders-api")
    );
    assert_eq!(
        resolve_folder(&folders, "1").as_deref(),
        Some("0001-orders-api")
    );
    assert_eq!(
        resolve_folder(&folders, "0012").as_deref(),
        Some("0012-cache")
    );
    assert_eq!(
        resolve_folder(&folders, ".crest/specs/0012-cache/").as_deref(),
        Some("0012-cache")
    );
    assert_eq!(
        resolve_folder(&folders, ".crest/specs/0012-cache/spec.md").as_deref(),
        Some("0012-cache")
    );
    assert_eq!(
        resolve_folder(&folders, "cache").as_deref(),
        Some("0012-cache")
    );
    assert_eq!(resolve_folder(&folders, "7"), None);
    assert_eq!(resolve_folder(&folders, "0"), None);
}

#[tokio::test]
async fn a_spec_is_simulated_and_recorded() {
    let h = harness(sim::script());
    let report = h.simulator.run(request("1")).await.unwrap();
    assert_eq!(report.folder, FOLDER);
    assert_eq!(report.board.title, "Orders API");
    let verdicts: Vec<(&str, bool)> = report
        .board
        .runs
        .iter()
        .map(|r| (r.scenario.as_str(), r.passed))
        .collect();
    assert_eq!(verdicts, [("one_request", true), ("burst", false)]);
    assert_eq!(report.repairs, 0);
    assert!(report.loosening.is_empty());

    // The assertions were asked first, in a call of their own.
    let requests = h.llm.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tools[0].name, "submit_assertions");
    assert_eq!(requests[1].tools[0].name, "submit_sysmodel");
    let second = format!("{:?}", requests[1].messages);
    assert!(second.contains("Frozen plan"));

    for f in [
        "simulation/sysmodel.json",
        "simulation/assertions.json",
        "simulation/traces/one_request.json",
        "simulation/traces/burst.json",
        "simulation/board.html",
    ] {
        assert!(file(&h, f).is_some(), "{f} missing");
    }
    let model: SysModel =
        SysModel::from_json(&file(&h, "simulation/sysmodel.json").unwrap()).unwrap();
    assert_eq!(model, report.board.model);
    let run: ScenarioRun =
        serde_json::from_str(&file(&h, "simulation/traces/burst.json").unwrap()).unwrap();
    assert!(!run.passed);
    assert_eq!(
        report.board_html(),
        Some(".crest/specs/0001-orders-api/simulation/board.html")
    );

    let sources: Vec<Evidence> = serde_json::from_str(&file(&h, "sources.json").unwrap()).unwrap();
    let ids: Vec<String> = sources.iter().map(|e| e.id.to_string()).collect();
    assert_eq!(ids, ["E1", "E2", "E3"]);
    assert!(sources[1].claim.contains("`one_request`") && sources[1].claim.contains("passed"));
    assert!(
        sources[2]
            .claim
            .contains("failed: always api.inflight <= 1")
    );
    assert_eq!(
        sources[2].source,
        domain::block::SourceRef::File {
            path: ".crest/specs/0001-orders-api/simulation/traces/burst.json".into(),
            line_start: None,
            line_end: None,
        }
    );
    let journal = file(&h, "journal.md").unwrap();
    assert!(journal.starts_with("# Research journal\n\n## 12. Spec"));
    assert!(journal.contains("## Simulation"));
    assert!(journal.contains("Scenario `burst` failed [E3]."));

    let ready = h
        .sink
        .events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::SimulationReady { board } => Some(board),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ready.len(), 1);
    assert_eq!(*ready[0], report.board);
    let summary = report.summary();
    assert!(summary.contains("1 of 2 scenarios passed"), "{summary}");
    assert!(summary.contains("FAIL burst"));
    assert!(summary.contains("Board: .crest/specs/0001-orders-api/simulation/board.html"));
}

#[tokio::test]
async fn changed_assertions_are_restored_and_reported() {
    let mut model = sim::model();
    model["scenarios"][1]["expect"] = json!([{ "always": "api.inflight <= 100" }]);
    let h = harness(vec![
        ScriptedResponse::tool_use("a", "submit_assertions", sim::plan()),
        ScriptedResponse::tool_use("b", "submit_sysmodel", model),
    ]);
    let report = h.simulator.run(request(FOLDER)).await.unwrap();
    assert_eq!(report.loosening.len(), 1, "{:?}", report.loosening);
    assert!(report.loosening[0].contains("always api.inflight <= 1"));
    let burst = report.board.run("burst").unwrap();
    assert!(!burst.passed, "the frozen assertion still decides");
    assert!(
        file(&h, "journal.md")
            .unwrap()
            .contains("Attempt to change the frozen assertions")
    );
    assert!(report.summary().contains("Frozen assertions restored"));
}

#[tokio::test]
async fn invalid_models_are_repaired_with_their_errors() {
    let mut broken = sim::model();
    broken["components"][1]["machine"]["transitions"][0]["effects"][1]["port"] = json!("nowhere");
    let mut missing = sim::model();
    missing["scenarios"] = json!([missing["scenarios"][0].clone()]);
    let h = harness(vec![
        ScriptedResponse::tool_use("a", "submit_assertions", sim::plan()),
        ScriptedResponse::tool_use("b", "submit_sysmodel", broken),
        ScriptedResponse::tool_use("c", "submit_sysmodel", missing),
        ScriptedResponse::tool_use("d", "submit_sysmodel", sim::model()),
    ]);
    let report = h.simulator.run(request(FOLDER)).await.unwrap();
    assert_eq!(report.repairs, 2);
    let requests = h.llm.requests();
    let second = format!("{:?}", requests[2].messages);
    assert!(second.contains("unknown-port"), "{second}");
    let third = format!("{:?}", requests[3].messages);
    assert!(third.contains("scenario `burst` of the frozen plan is missing"));
}

#[tokio::test]
async fn too_many_invalid_models_fail() {
    let broken = json!({ "version": "crest.sysmodel/v1", "components": [], "extra": 1 });
    let h = harness(vec![
        ScriptedResponse::tool_use("a", "submit_assertions", sim::plan()),
        ScriptedResponse::tool_use("b", "submit_sysmodel", broken.clone()),
        ScriptedResponse::tool_use("c", "submit_sysmodel", broken.clone()),
        ScriptedResponse::tool_use("d", "submit_sysmodel", broken),
    ]);
    let err = h.simulator.run(request(FOLDER)).await.unwrap_err();
    assert!(
        matches!(err, SimulateError::Model { rounds: 2, .. }),
        "{err}"
    );
    assert!(file(&h, "simulation/sysmodel.json").is_none());
}

#[tokio::test]
async fn bad_plans_are_asked_again_once() {
    let mut bad = sim::plan();
    bad["scenarios"][1]["expect"] = json!([{ "always": "inflight <= 1" }]);
    let h = harness(vec![
        ScriptedResponse::tool_use("a", "submit_assertions", bad.clone()),
        ScriptedResponse::tool_use("b", "submit_assertions", bad),
    ]);
    let err = h.simulator.run(request(FOLDER)).await.unwrap_err();
    let SimulateError::Plan(problems) = err else {
        panic!("{err}")
    };
    assert!(problems[0].contains("component.inflight"), "{problems:?}");
    assert_eq!(h.llm.requests().len(), 2);
}

#[tokio::test]
async fn unknown_or_empty_specs_are_reported() {
    let h = harness(Vec::new());
    let err = h.simulator.run(request("9")).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("found: 0001-orders-api, 0002-other"),
        "{err}"
    );
    let err = h.simulator.run(request("0002")).await.unwrap_err();
    assert!(
        err.to_string().contains("no black-box or sequence diagram"),
        "{err}"
    );
}
