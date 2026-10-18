//! The frame end to end: three fake nodes A → B (gate) → C, where C
//! sends the flow back to A once.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::model::ModelPolicy;
use domain::research::Stage;
use graph_flow::{InMemorySessionStorage, Session, SessionStorage};

use super::builder::{ResearchGraph, ResearchGraphBuilder};
use super::error::FlowError;
use super::fake::FakeNode;
use super::flow::ResearchFlow;
use super::gate::FlowStatus;
use super::node::{NodeDeps, NodeOutcome};
use crate::ports::events::DomainEvent;
use crate::testing::llm::ScriptedLlm;
use crate::testing::system::RecordingSink;

const A: Stage = Stage::Observation;
const B: Stage = Stage::Hypotheses;
const C: Stage = Stage::Experiments;

/// Session storage that counts saves.
#[derive(Default)]
pub(super) struct CountingStorage {
    inner: InMemorySessionStorage,
    saves: AtomicUsize,
}

impl CountingStorage {
    pub(super) fn saves(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SessionStorage for CountingStorage {
    async fn save(&self, session: Session) -> graph_flow::Result<()> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.inner.save(session).await
    }

    async fn get(&self, id: &str) -> graph_flow::Result<Option<Session>> {
        self.inner.get(id).await
    }

    async fn delete(&self, id: &str) -> graph_flow::Result<()> {
        self.inner.delete(id).await
    }
}

/// The three fake nodes and what they see.
pub(super) struct Fixture {
    pub(super) a: Arc<FakeNode>,
    pub(super) b: Arc<FakeNode>,
    pub(super) c: Arc<FakeNode>,
    pub(super) sink: Arc<RecordingSink>,
    pub(super) storage: Arc<CountingStorage>,
}

impl Fixture {
    pub(super) fn new(b: FakeNode, c: FakeNode) -> Self {
        Self {
            a: Arc::new(FakeNode::new(A)),
            b: Arc::new(b),
            c: Arc::new(c),
            sink: Arc::new(RecordingSink::new()),
            storage: Arc::new(CountingStorage::default()),
        }
    }

    /// B finishes and gates; C goes back to A once, then finishes.
    pub(super) fn with_one_back() -> Self {
        Self::new(
            FakeNode::new(B),
            FakeNode::new(C).then(NodeOutcome::Back {
                to: A,
                reason: "falta el perfil de la consulta".into(),
            }),
        )
    }

    pub(super) fn graph(&self) -> ResearchGraph {
        let deps = NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::clone(&self.sink) as _,
        )
        .with_clock(|| 1);
        ResearchGraphBuilder::new(deps)
            .shared_node(Arc::clone(&self.a) as _)
            .shared_node(Arc::clone(&self.b) as _)
            .shared_node(Arc::clone(&self.c) as _)
            .build()
            .unwrap()
    }

    pub(super) async fn start(&self) -> ResearchFlow {
        let storage = Arc::clone(&self.storage) as Arc<dyn SessionStorage>;
        ResearchFlow::start(self.graph(), storage, "s1", "el login tarda 2 s")
            .await
            .unwrap()
    }

    pub(super) fn runs(&self) -> [usize; 3] {
        [self.a.runs(), self.b.runs(), self.c.runs()]
    }

    pub(super) fn headers(&self) -> Vec<u32> {
        self.sink
            .events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => match block.kind {
                    BlockKind::StageHeader { index, .. } => Some(index),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }
}

fn waiting_at(status: &FlowStatus) -> Option<Stage> {
    match status {
        FlowStatus::WaitingForInput { stage, .. } => Some(*stage),
        _ => None,
    }
}

#[tokio::test]
async fn three_nodes_with_a_gate_and_a_back() {
    let fx = Fixture::with_one_back();
    let flow = fx.start().await;

    // A runs, B finishes and stops at its gate.
    let status = flow.run().await.unwrap();
    assert_eq!(waiting_at(&status), Some(B));
    assert_eq!(fx.runs(), [1, 1, 0]);

    // Confirming runs C, which goes back to A; A and B run again and B
    // asks again.
    let status = flow.provide_input("sí").await.unwrap();
    assert_eq!(waiting_at(&status), Some(B));
    assert_eq!(fx.runs(), [2, 2, 1]);

    // This time C finishes and the flow ends.
    let status = flow.provide_input("ok").await.unwrap();
    assert_eq!(status, FlowStatus::Completed);
    assert_eq!(fx.runs(), [2, 2, 2]);
    assert_eq!(fx.headers(), [2, 5, 6, 2, 5, 6]);

    let journal = flow.state().await.unwrap().journal;
    assert!(
        journal
            .iter()
            .any(|e| e.stage == C && e.text.contains("Regreso a Observación"))
    );
    assert_eq!(
        journal.last().map(|e| e.text.as_str()),
        Some("Flujo terminado.")
    );
    assert!(matches!(
        flow.provide_input("más").await,
        Err(FlowError::Completed)
    ));
}

#[tokio::test]
async fn every_finished_node_is_a_checkpoint() {
    let fx = Fixture::with_one_back();
    let flow = fx.start().await;
    assert_eq!(fx.storage.saves(), 1);

    let status = flow.step().await.unwrap();
    assert_eq!(status, FlowStatus::Running { next: B });
    assert_eq!(fx.storage.saves(), 2);

    let status = flow.step().await.unwrap();
    assert_eq!(waiting_at(&status), Some(B));
    assert_eq!(fx.storage.saves(), 3);

    // Waiting: stepping again neither runs nor saves.
    flow.step().await.unwrap();
    assert_eq!((fx.runs(), fx.storage.saves()), ([1, 1, 0], 3));
}

#[tokio::test]
async fn feedback_at_a_gate_reruns_the_node() {
    let fx = Fixture::with_one_back();
    let flow = fx.start().await;
    flow.run().await.unwrap();
    let status = flow.provide_input("agrega la hipótesis H2").await.unwrap();
    assert_eq!(waiting_at(&status), Some(B));
    assert_eq!(
        fx.b.replies(),
        [None, Some("agrega la hipótesis H2".into())]
    );
    assert_eq!(fx.runs(), [1, 2, 0]);
}

#[tokio::test]
async fn questions_rerun_the_node_with_the_answer() {
    let fx = Fixture::new(
        FakeNode::new(B),
        FakeNode::new(C).then(NodeOutcome::NeedsInput {
            prompt: "¿Qué versión de Postgres?".into(),
        }),
    );
    let flow = fx.start().await;
    flow.run().await.unwrap();
    let status = flow.provide_input("sí").await.unwrap();
    assert_eq!(
        status,
        FlowStatus::WaitingForInput {
            stage: C,
            prompt: "¿Qué versión de Postgres?".into()
        }
    );
    let status = flow.provide_input("la 16").await.unwrap();
    assert_eq!(status, FlowStatus::Completed);
    assert_eq!(fx.c.replies(), [None, Some("la 16".into())]);
}

#[tokio::test]
async fn resumes_from_storage_after_dropping_the_runner() {
    let fx = Fixture::with_one_back();
    let flow = fx.start().await;
    flow.step().await.unwrap();
    drop(flow);

    // A new graph (as after a restart) over the same storage.
    let storage = Arc::clone(&fx.storage) as Arc<dyn SessionStorage>;
    let flow = ResearchFlow::resume(fx.graph(), storage, "s1")
        .await
        .unwrap();
    let position = flow.position().await.unwrap();
    assert_eq!(position.stage, B);
    assert_eq!(flow.problem().await.unwrap(), "el login tarda 2 s");
    let status = flow.run().await.unwrap();
    assert_eq!(waiting_at(&status), Some(B));
    assert_eq!(fx.runs(), [1, 1, 0]);
}

#[tokio::test]
async fn a_failing_node_keeps_the_last_checkpoint() {
    let fx = Fixture::new(FakeNode::new(B).then_fail("sin red"), FakeNode::new(C));
    let flow = fx.start().await;
    let err = flow.run().await.unwrap_err();
    assert!(err.to_string().contains("sin red"), "{err}");
    assert_eq!(
        flow.status().await.unwrap(),
        FlowStatus::Running { next: B }
    );
    assert!(fx.sink.kinds().contains(&"block_added".to_owned()));

    // Retrying runs B again from the checkpoint.
    let status = flow.run().await.unwrap();
    assert_eq!(waiting_at(&status), Some(B));
    assert_eq!(fx.runs(), [1, 2, 0]);
}

#[tokio::test]
async fn start_and_resume_check_the_session() {
    let fx = Fixture::with_one_back();
    fx.start().await;
    let storage = Arc::clone(&fx.storage) as Arc<dyn SessionStorage>;
    let again = ResearchFlow::start(fx.graph(), Arc::clone(&storage), "s1", "x").await;
    assert!(matches!(again, Err(FlowError::SessionExists(_))));
    let missing = ResearchFlow::resume(fx.graph(), storage, "nope").await;
    assert!(matches!(missing, Err(FlowError::SessionNotFound(_))));
}
