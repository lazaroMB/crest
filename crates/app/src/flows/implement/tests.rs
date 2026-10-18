//! The six steps chained in one flow over fakes: gates, the loop over the
//! tasks and the report.

use std::sync::Arc;

use graph_flow::{InMemorySessionStorage, SessionStorage};
use serde_json::json;

use super::builder::ImplementGraphBuilder;
use super::flow::ImplementFlow;
use super::load::LoadNode;
use super::load::tests::plan_json;
use super::nodes::commit::CommitTask;
use super::nodes::implement::ImplementTask;
use super::nodes::report::WriteReport;
use super::nodes::test::TestTask;
use super::nodes::tests::{Harness, exit};
use super::nodes::verify::VerifyPlan;
use super::state::{FlowStatus, Step, TaskStatus};
use crate::testing::fs::ScriptedShell;
use crate::testing::llm::ScriptedResponse;

fn write(id: &str, path: &str) -> ScriptedResponse {
    ScriptedResponse::tool_use(
        id,
        "write_file",
        json!({ "path": path, "content": "// ok\n" }),
    )
}

fn git_status(file: &str) -> Vec<String> {
    [
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--",
        file,
    ]
    .map(str::to_owned)
    .to_vec()
}

#[tokio::test]
async fn implements_a_plan_task_by_task_and_reports() {
    let mut shell = ScriptedShell::new()
        .on(
            "git",
            ["switch", "-c", "spec/0007-cache-de-sesiones"],
            exit(0, ""),
        )
        .on(
            "git",
            ["rev-parse", "--short", "HEAD"],
            exit(0, "abc1234\n"),
        );
    for id in ["t1", "t2"] {
        let file = format!("src/{id}.rs");
        let upper = id.to_uppercase();
        let message = format!("feat(src): Hacer {upper}\n\nTask: {upper}");
        shell = shell
            .on("cargo", ["test", id], exit(0, "ok"))
            .on("git", git_status(&file), exit(0, &format!("?? {file}\0")))
            .on("git", ["add", "-A", "--", file.as_str()], exit(0, ""))
            .on("git", ["commit", "-m", message.as_str()], exit(0, ""));
    }
    let shell = Arc::new(shell);
    let h = Harness::new(
        [
            write("toolu_1", "src/t1.rs"),
            ScriptedResponse::text("STATUS: done"),
            write("toolu_2", "src/t2.rs"),
            ScriptedResponse::text("STATUS: done"),
        ],
        Arc::clone(&shell) as _,
    );
    h.fs.set_file(
        ".crest/specs/0007/plan.json",
        plan_json(&[("T2", &["T1"]), ("T1", &[])]),
    );
    let graph = ImplementGraphBuilder::new(h.deps())
        .node(LoadNode::new())
        .node(ImplementTask::new())
        .node(TestTask::new())
        .node(CommitTask::new())
        .node(VerifyPlan::new())
        .node(WriteReport::new())
        .build()
        .unwrap();
    let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());
    let flow = ImplementFlow::start(graph, storage, "impl-1", ".crest/specs/0007/plan.json")
        .await
        .unwrap();

    // Gate before starting: plan and branch.
    let status = flow.run().await.unwrap();
    assert!(matches!(
        status,
        FlowStatus::WaitingForInput {
            step: Step::Load,
            ..
        }
    ));
    // On through both tasks to the gate before closing.
    let status = flow.provide_input("sí").await.unwrap();
    assert!(
        matches!(
            status,
            FlowStatus::WaitingForInput {
                step: Step::Report,
                ..
            }
        ),
        "{status:?}"
    );
    assert_eq!(
        flow.provide_input("sí").await.unwrap(),
        FlowStatus::Completed
    );

    let state = flow.state().await.unwrap();
    assert_eq!(state.branch.as_deref(), Some("spec/0007-cache-de-sesiones"));
    for id in &state.order {
        assert_eq!(state.record(id).unwrap().status, TaskStatus::Committed);
    }
    let commits = shell
        .calls()
        .iter()
        .filter(|c| c.args.first().is_some_and(|a| a == "commit"))
        .count();
    assert_eq!(commits, 2);
    let report = h.fs.file(".crest/specs/0007/report.md").unwrap();
    assert!(report.contains("- Tareas hechas: 2 de 2"), "{report}");
}
