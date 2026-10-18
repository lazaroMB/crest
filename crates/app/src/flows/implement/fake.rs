//! A scripted [`ImplementNode`] for tests of the frame and of graphs.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;

use super::node::{ImplementNode, NodeError, StepContext, StepOutcome};
use super::state::{EntryKind, Step};

/// A step that logs every run and answers a script, then `Next`.
#[derive(Debug)]
pub struct FakeStep {
    step: Step,
    script: Mutex<VecDeque<Result<StepOutcome, String>>>,
    replies: Mutex<Vec<Option<String>>>,
}

impl FakeStep {
    /// A node of `step` that always answers `Next`.
    #[must_use]
    pub fn new(step: Step) -> Self {
        Self {
            step,
            script: Mutex::new(VecDeque::new()),
            replies: Mutex::new(Vec::new()),
        }
    }

    /// Answers `outcome` on the next unscripted run.
    #[must_use]
    pub fn then(self, outcome: StepOutcome) -> Self {
        self.lock_script().push_back(Ok(outcome));
        self
    }

    /// Fails with [`NodeError::Failed`] on the next unscripted run.
    #[must_use]
    pub fn then_fail(self, message: impl Into<String>) -> Self {
        self.lock_script().push_back(Err(message.into()));
        self
    }

    /// Number of runs so far.
    #[must_use]
    pub fn runs(&self) -> usize {
        self.lock_replies().len()
    }

    /// The user reply each run saw.
    #[must_use]
    pub fn replies(&self) -> Vec<Option<String>> {
        self.lock_replies().clone()
    }

    fn lock_script(&self) -> std::sync::MutexGuard<'_, VecDeque<Result<StepOutcome, String>>> {
        self.script
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_replies(&self) -> std::sync::MutexGuard<'_, Vec<Option<String>>> {
        self.replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl ImplementNode for FakeStep {
    fn step(&self) -> Step {
        self.step
    }

    async fn run(&self, ctx: &mut StepContext) -> Result<StepOutcome, NodeError> {
        self.lock_replies()
            .push(ctx.user_reply().map(str::to_owned));
        ctx.log(EntryKind::Note, format!("{} ran", self.step.name_es()));
        match self.lock_script().pop_front() {
            Some(Ok(outcome)) => Ok(outcome),
            Some(Err(message)) => Err(NodeError::Failed(message)),
            None => Ok(StepOutcome::Next),
        }
    }
}
