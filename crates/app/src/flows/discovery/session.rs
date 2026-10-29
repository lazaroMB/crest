//! [`DiscoverySession`]: the engine as the chat (window or headless)
//! drives it. One active discovery per workspace at a time; while it waits
//! for a person every plain line of the chat answers it.

use std::sync::Arc;

use domain::discovery::{Level, NextStep, Section, Status};

use super::engine::{DiscoveryEngine, StepOutcome};
use super::error::DiscoveryError;
use crate::flows::aside::{FlowKind, GateInfo};
use crate::ports::discovery::{DiscoveryId, DiscoverySummary};

/// The discovery flow of one workspace.
#[derive(Debug, Clone)]
pub struct DiscoverySession {
    engine: Arc<DiscoveryEngine>,
    active: Option<DiscoveryId>,
    waiting: Option<Waiting>,
    /// The discovery this session drove last, kept after it froze.
    last: Option<DiscoveryId>,
}

/// What the active discovery waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Waiting {
    section: Option<Section>,
    prompt: String,
}

impl DiscoverySession {
    /// A session over `engine` with nothing active.
    #[must_use]
    pub fn new(engine: Arc<DiscoveryEngine>) -> Self {
        Self {
            engine,
            active: None,
            waiting: None,
            last: None,
        }
    }

    /// The discovery this session drove last (started, resumed, reopened,
    /// accepted or aborted), whether or not it is still open.
    #[must_use]
    pub fn last(&self) -> Option<&DiscoveryId> {
        self.last.as_ref()
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &Arc<DiscoveryEngine> {
        &self.engine
    }

    /// The discovery the chat's lines go to, if one waits.
    #[must_use]
    pub fn active(&self) -> Option<&DiscoveryId> {
        self.active.as_ref()
    }

    /// Whether a discovery waits for a person's line.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.is_some() && self.waiting.is_some()
    }

    /// The gate the chat shows while waiting.
    #[must_use]
    pub fn gate_info(&self) -> Option<GateInfo> {
        let waiting = self.waiting.as_ref()?;
        Some(GateInfo {
            flow: FlowKind::Research,
            stage: waiting
                .section
                .map_or_else(|| "accept".to_owned(), |s| s.to_string()),
            prompt: waiting.prompt.clone(),
            topic: None,
        })
    }

    /// Starts a discovery of `problem` at `level` and runs it to its
    /// first stop.
    ///
    /// # Errors
    ///
    /// The engine's.
    pub async fn start(&mut self, problem: &str, level: Level) -> Result<String, DiscoveryError> {
        let id = self.engine.start(problem, level).await?;
        self.active = Some(id.clone());
        self.last = Some(id);
        self.waiting = None;
        self.run().await
    }

    /// Runs the active discovery to its next stop and describes it.
    async fn run(&mut self) -> Result<String, DiscoveryError> {
        let id = self.active.clone().ok_or(DiscoveryError::NotActive)?;
        let outcomes = self.engine.run(&id).await?;
        let last = outcomes.last().cloned();
        self.waiting = match &last {
            Some(StepOutcome::NeedsHuman { section, prompt }) => Some(Waiting {
                section: Some(*section),
                prompt: prompt.clone(),
            }),
            Some(StepOutcome::ReadyToAccept) => Some(Waiting {
                section: None,
                prompt: StepOutcome::ReadyToAccept.to_string(),
            }),
            _ => None,
        };
        if matches!(last, Some(StepOutcome::Frozen(_))) {
            self.active = None;
        }
        Ok(describe(&id, &outcomes))
    }

    /// A line of the chat for the waiting discovery.
    ///
    /// # Errors
    ///
    /// [`DiscoveryError::NotActive`] when nothing waits, or the engine's.
    pub async fn handle_line(&mut self, line: &str) -> Result<String, DiscoveryError> {
        let id = self.active.clone().ok_or(DiscoveryError::NotActive)?;
        if self.waiting.is_none() {
            return Err(DiscoveryError::NotActive);
        }
        let answered = self.engine.answer(&id, line).await?;
        if matches!(answered, StepOutcome::Frozen(Status::Accepted)) {
            self.active = None;
            self.waiting = None;
            return Ok(format!("{id}: accepted and frozen"));
        }
        let rest = self.run().await?;
        Ok(format!("{answered}\n{rest}"))
    }

    /// Where the active (or the latest) discovery is.
    ///
    /// # Errors
    ///
    /// The engine's.
    pub async fn status(&self) -> Result<String, DiscoveryError> {
        let id = match &self.active {
            Some(id) => id.clone(),
            None => match self.pending().await? {
                Some(summary) => summary.id,
                None => {
                    return Ok(
                        "no discovery is active; `/research <problem>` starts one and `/research list` shows the stored ones"
                            .to_owned(),
                    );
                }
            },
        };
        let report = self.engine.status(&id).await?;
        let hint = match (&self.waiting, &report.next) {
            (Some(w), _) => format!("\n{}", w.prompt),
            (None, NextStep::Frozen) => String::new(),
            (None, _) => "\n`/research resume` continues it".to_owned(),
        };
        Ok(format!("{report}{hint}"))
    }

    /// The stored discoveries of the workspace, newest first.
    ///
    /// # Errors
    ///
    /// The engine's.
    pub async fn list(&self) -> Result<Vec<DiscoverySummary>, DiscoveryError> {
        self.engine.list().await
    }

    /// Locks the folders of every open discovery of the workspace, as at
    /// start-up: a discovery stays open across sessions and its folder
    /// must not be edited by the tools before it is resumed. Returns how
    /// many were locked.
    ///
    /// # Errors
    ///
    /// The engine's.
    pub async fn lock_open(&self) -> Result<usize, DiscoveryError> {
        let open: Vec<DiscoveryId> = self
            .list()
            .await?
            .into_iter()
            .filter(|s| s.status == Status::Open)
            .map(|s| s.id)
            .collect();
        for id in &open {
            self.engine.lock(id);
        }
        Ok(open.len())
    }

    /// The latest open discovery of the workspace, if any.
    ///
    /// # Errors
    ///
    /// The engine's.
    pub async fn pending(&self) -> Result<Option<DiscoverySummary>, DiscoveryError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .find(|s| s.status == Status::Open))
    }

    /// Resumes the latest open discovery, or the one named by `which`
    /// (a 1-based number of [`DiscoverySession::list`] or an id), and runs
    /// it to its next stop.
    ///
    /// # Errors
    ///
    /// [`DiscoveryError::Unknown`] or the engine's.
    pub async fn resume(&mut self, which: Option<&str>) -> Result<String, DiscoveryError> {
        let id = self.resolve(which).await?;
        let loaded = self.engine.load(&id).await?;
        if loaded.state.status != Status::Open {
            return Err(DiscoveryError::Frozen(loaded.state.status));
        }
        let cleaned = super::spikes::clean_unfinished(&self.engine, &id, &loaded).await?;
        self.engine.lock(&id);
        self.active = Some(id.clone());
        self.last = Some(id.clone());
        self.waiting = None;
        let mut out = self.run().await?;
        if !cleaned.is_empty() {
            out = format!("cleaned up {} unfinished spike(s)\n{out}", cleaned.len());
        }
        Ok(out)
    }

    /// The discovery `which` names: the latest open one when `None`, else
    /// a 1-based number of [`DiscoverySession::list`] or an id.
    ///
    /// # Errors
    ///
    /// [`DiscoveryError::NotActive`] when nothing is open,
    /// [`DiscoveryError::Unknown`] when nothing matches.
    async fn resolve(&self, which: Option<&str>) -> Result<DiscoveryId, DiscoveryError> {
        let list = self.list().await?;
        match which {
            None => list
                .iter()
                .find(|s| s.status == Status::Open)
                .map(|s| s.id.clone())
                .ok_or(DiscoveryError::NotActive),
            Some(which) => {
                let which = which.trim();
                match which.parse::<usize>() {
                    Ok(n) if n >= 1 => list.get(n - 1).map(|s| s.id.clone()),
                    _ => list
                        .iter()
                        .find(|s| s.id.as_str() == which)
                        .map(|s| s.id.clone()),
                }
                .ok_or_else(|| DiscoveryError::Unknown(DiscoveryId::new(which)))
            }
        }
    }

    /// Aborts the discoveries `which` names: the active (or latest open)
    /// one when `None`, every open one for `all`, else the one numbered or
    /// named. Returns how many were open and are now aborted.
    ///
    /// # Errors
    ///
    /// [`DiscoveryError::Unknown`] for a number or id that matches nothing,
    /// or the engine's.
    pub async fn cancel(&mut self, which: Option<&str>) -> Result<usize, DiscoveryError> {
        let targets: Vec<DiscoveryId> = match which.map(str::trim) {
            Some("all") => self
                .list()
                .await?
                .into_iter()
                .filter(|s| s.status == Status::Open)
                .map(|s| s.id)
                .collect(),
            Some(which) => vec![self.resolve(Some(which)).await?],
            None => match &self.active {
                Some(id) => vec![id.clone()],
                None => self.pending().await?.map(|s| s.id).into_iter().collect(),
            },
        };
        let mut aborted = 0;
        for id in &targets {
            if self.engine.load(id).await?.state.status != Status::Open {
                continue;
            }
            self.engine.abort(id).await?;
            aborted += 1;
            self.last = Some(id.clone());
            if self.active.as_ref() == Some(id) {
                self.active = None;
                self.waiting = None;
            }
        }
        Ok(aborted)
    }

    /// Reopens `section` of the active discovery and runs on.
    ///
    /// # Errors
    ///
    /// The engine's.
    pub async fn reopen(
        &mut self,
        section: Section,
        reason: &str,
    ) -> Result<String, DiscoveryError> {
        let id = self.active_or_pending().await?;
        let dropped = self.engine.reopen(&id, section, reason).await?;
        self.active = Some(id.clone());
        self.last = Some(id);
        self.waiting = None;
        let rest = self.run().await?;
        let dropped: Vec<String> = dropped.iter().map(ToString::to_string).collect();
        Ok(format!(
            "reopened `{section}`; verdicts dropped: {}\n{rest}",
            if dropped.is_empty() {
                "none".to_owned()
            } else {
                dropped.join(", ")
            }
        ))
    }

    /// Aborts the active (or pending) discovery.
    ///
    /// # Errors
    ///
    /// The engine's.
    pub async fn abort(&mut self) -> Result<String, DiscoveryError> {
        let id = self.active_or_pending().await?;
        self.engine.abort(&id).await?;
        self.active = None;
        self.waiting = None;
        self.last = Some(id.clone());
        Ok(format!("discovery {id} aborted; its log is kept"))
    }

    /// Accepts the active discovery when every section is approved; on
    /// the one accepted last, derives its Spec again.
    ///
    /// # Errors
    ///
    /// A refused event (a section is still open) or the engine's.
    pub async fn accept(&mut self) -> Result<String, DiscoveryError> {
        let id = match self.active_or_pending().await {
            Ok(id) => id,
            Err(DiscoveryError::NotActive) => self.last.clone().ok_or(DiscoveryError::NotActive)?,
            Err(err) => return Err(err),
        };
        self.engine.accept(&id).await?;
        self.active = None;
        self.waiting = None;
        self.last = Some(id.clone());
        Ok(format!(
            "discovery {id} accepted and frozen; its Spec is under .crest/specs/{id}/"
        ))
    }

    async fn active_or_pending(&self) -> Result<DiscoveryId, DiscoveryError> {
        if let Some(id) = &self.active {
            return Ok(id.clone());
        }
        self.pending()
            .await?
            .map(|s| s.id)
            .ok_or(DiscoveryError::NotActive)
    }
}

/// One line per outcome, the stop last.
#[must_use]
pub fn describe(id: &DiscoveryId, outcomes: &[StepOutcome]) -> String {
    if outcomes.is_empty() {
        return format!("{id}: nothing to do");
    }
    outcomes
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
