//! A graph-flow run paused at a `WaitForInput` gate resumes at that gate
//! after the process "restarts" (everything dropped, the database reopened).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use adapter_store::SqliteFlowStorage;
use async_trait::async_trait;
use graph_flow::{
    Context, ExecutionStatus, FlowRunner, Graph, GraphBuilder, NextAction, Session, SessionStorage,
    Task, TaskResult,
};

/// How many times each task ran, for one graph instance.
#[derive(Default)]
struct Runs {
    first: AtomicUsize,
    gate: AtomicUsize,
    last: AtomicUsize,
}

struct First(Arc<Runs>);
struct Gate(Arc<Runs>);
struct Last(Arc<Runs>);

#[async_trait]
impl Task for First {
    fn id(&self) -> &str {
        "first"
    }

    async fn run(&self, ctx: Context) -> graph_flow::Result<TaskResult> {
        self.0.first.fetch_add(1, Ordering::SeqCst);
        ctx.set("observed", vec!["a", "b"])?;
        Ok(TaskResult::new(None, NextAction::ContinueAndExecute))
    }
}

#[async_trait]
impl Task for Gate {
    fn id(&self) -> &str {
        "gate"
    }

    async fn run(&self, ctx: Context) -> graph_flow::Result<TaskResult> {
        self.0.gate.fetch_add(1, Ordering::SeqCst);
        if ctx.get::<String>("answer").is_none() {
            return Ok(TaskResult::new(
                Some("need an answer".into()),
                NextAction::WaitForInput,
            ));
        }
        Ok(TaskResult::new(None, NextAction::ContinueAndExecute))
    }
}

#[async_trait]
impl Task for Last {
    fn id(&self) -> &str {
        "last"
    }

    async fn run(&self, ctx: Context) -> graph_flow::Result<TaskResult> {
        self.0.last.fetch_add(1, Ordering::SeqCst);
        let observed: Vec<String> = ctx.get("observed").unwrap_or_default();
        let answer: String = ctx.get("answer").unwrap_or_default();
        ctx.set("summary", format!("{}+{answer}", observed.join(",")))?;
        Ok(TaskResult::new(Some("done".into()), NextAction::End))
    }
}

fn graph(runs: &Arc<Runs>) -> graph_flow::Result<Arc<Graph>> {
    let graph = GraphBuilder::new("toy")
        .add_task(Arc::new(First(Arc::clone(runs))))
        .add_task(Arc::new(Gate(Arc::clone(runs))))
        .add_task(Arc::new(Last(Arc::clone(runs))))
        .add_edge("first", "gate")
        .add_edge("gate", "last")
        .build()?;
    Ok(Arc::new(graph))
}

fn counts(runs: &Runs) -> [usize; 3] {
    [&runs.first, &runs.gate, &runs.last].map(|n| n.load(Ordering::SeqCst))
}

#[tokio::test]
async fn resumes_at_the_gate_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crest.db");

    // First process: run until the gate.
    {
        let storage = Arc::new(SqliteFlowStorage::open(&path).unwrap());
        let runs = Arc::new(Runs::default());
        let runner = FlowRunner::new(graph(&runs).unwrap(), storage.clone());
        storage
            .save(Session::new_from_task("flow-1".into(), "first").with_graph_id("toy"))
            .await
            .unwrap();

        let result = runner.run("flow-1").await.unwrap();
        assert!(matches!(result.status, ExecutionStatus::WaitingForInput));
        assert_eq!(result.response.as_deref(), Some("need an answer"));
        assert_eq!(counts(&runs), [1, 1, 0]);
    }

    // Second process: a new storage on the same file and a new, identical graph.
    let storage = Arc::new(SqliteFlowStorage::open(&path).unwrap());
    let runs = Arc::new(Runs::default());
    let runner = FlowRunner::new(graph(&runs).unwrap(), storage.clone());

    let session = storage.get("flow-1").await.unwrap().unwrap();
    assert_eq!(session.current_task_id, "gate");
    assert_eq!(
        session.context.get::<Vec<String>>("observed"),
        Some(vec!["a".to_owned(), "b".to_owned()])
    );
    session.context.set("answer", "yes").unwrap();
    storage.save(session).await.unwrap();

    let result = runner.run("flow-1").await.unwrap();
    assert!(matches!(result.status, ExecutionStatus::Completed));
    assert_eq!(counts(&runs), [0, 1, 1], "the first task must not rerun");

    let done = storage.get("flow-1").await.unwrap().unwrap();
    assert_eq!(done.current_task_id, "last");
    assert_eq!(
        done.context.get::<String>("summary").as_deref(),
        Some("a,b+yes")
    );
}
