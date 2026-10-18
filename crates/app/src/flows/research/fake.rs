//! A scripted [`ResearchNode`] for tests of the frame and of graphs.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use domain::research::Stage;

use super::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// What a [`FakeNode`] answers on one run.
#[derive(Debug, Clone)]
pub enum FakeStep {
    /// Return this outcome.
    Outcome(NodeOutcome),
    /// Fail with [`NodeError::Failed`] and this message.
    Fail(String),
}

/// A node that logs every run and answers a script, then `Done`.
#[derive(Debug)]
pub struct FakeNode {
    stage: Stage,
    script: Mutex<VecDeque<FakeStep>>,
    replies: Mutex<Vec<Option<String>>>,
}

impl FakeNode {
    /// A node of `stage` that always finishes.
    #[must_use]
    pub fn new(stage: Stage) -> Self {
        Self {
            stage,
            script: Mutex::new(VecDeque::new()),
            replies: Mutex::new(Vec::new()),
        }
    }

    /// Answers `outcome` on the next unscripted run.
    #[must_use]
    pub fn then(self, outcome: NodeOutcome) -> Self {
        self.push(FakeStep::Outcome(outcome));
        self
    }

    /// Fails on the next unscripted run.
    #[must_use]
    pub fn then_fail(self, message: impl Into<String>) -> Self {
        self.push(FakeStep::Fail(message.into()));
        self
    }

    fn push(&self, step: FakeStep) {
        if let Ok(mut script) = self.script.lock() {
            script.push_back(step);
        }
    }

    /// Number of runs so far.
    #[must_use]
    pub fn runs(&self) -> usize {
        self.replies.lock().map_or(0, |r| r.len())
    }

    /// The `user_reply` seen by each run.
    #[must_use]
    pub fn replies(&self) -> Vec<Option<String>> {
        self.replies.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl ResearchNode for FakeNode {
    fn stage(&self) -> Stage {
        self.stage
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if let Ok(mut replies) = self.replies.lock() {
            replies.push(ctx.user_reply().map(str::to_owned));
        }
        let step = self.script.lock().ok().and_then(|mut s| s.pop_front());
        match step {
            Some(FakeStep::Fail(message)) => Err(NodeError::Failed(message)),
            Some(FakeStep::Outcome(outcome)) => {
                ctx.log(format!("{outcome:?}"));
                Ok(outcome)
            }
            None => {
                ctx.log("Done");
                Ok(NodeOutcome::Done)
            }
        }
    }
}
