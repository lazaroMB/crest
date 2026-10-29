//! [`DiscoveryEngine`]: the use case that drives one discovery. It has no
//! state of its own: every command loads the log, replays it, asks the
//! state for the one legal next step and appends one event.

use std::sync::Arc;

use domain::block::{Block, BlockKind};
use domain::discovery::{
    Actor, DiscoveryEvent, DiscoveryState, Level, Logged, NextStep, Section, Status, Verdict,
    VerdictPolicy, replay,
};
use domain::model::ModelRole;

use super::audit::{AuditReport, audit};
use super::error::DiscoveryError;
use super::evaluator::{MAX_MODEL_ROUNDS, evaluate, human_prompt, read_human_verdict};
use super::lock::DiscoveryLock;
use super::model::Model;
use super::packet::{Packet, Span, citable_items};
use super::records::{self, ClaimRecord, EvidenceRecord};
use super::render::{discovery_markdown, events_jsonl, section_artifact};
use super::writers::{Draft, findings_text, write_section};
use crate::decide::Decider;
use crate::flows::research::node::system_now_ms;
use crate::flows::research::nodes::spec::render::{folder_number, slug};
use crate::flows::research::nodes::spec::sink::SpecSink;
use crate::flows::research::usage::StageUsage;
use crate::ports::discovery::{
    DiscoveryId, DiscoveryLogError, DiscoveryLogPort, SourceStorePort, StoredSource, text_hash,
};
use crate::ports::events::{DomainEvent, EventSink, NoticeLevel};
use crate::ports::fs::FileSystemPort;
use crate::ports::shell::ShellPort;
use crate::ports::web::WebPort;
use crate::usecases::run_turn::AgentService;

/// Consecutive failing audits of one section before a person is asked.
pub const MAX_FIX_ROUNDS: u32 = 2;

/// What the engine is built from.
#[derive(Clone)]
pub struct DiscoveryDeps {
    pub(crate) log: Arc<dyn DiscoveryLogPort>,
    pub(crate) sources: Arc<dyn SourceStorePort>,
    pub(crate) model: Model,
    pub(crate) events: Arc<dyn EventSink>,
    pub(crate) decider: Decider,
    pub(crate) explorer: Option<Arc<AgentService>>,
    pub(crate) worker: Option<Arc<AgentService>>,
    pub(crate) fs: Option<Arc<dyn FileSystemPort>>,
    pub(crate) web: Option<Arc<dyn WebPort>>,
    pub(crate) shell: Option<Arc<dyn ShellPort>>,
    pub(crate) sink: Option<Arc<dyn SpecSink>>,
    pub(crate) workspace_root: String,
    pub(crate) human: String,
    pub(crate) clock: fn() -> u64,
    pub(crate) lock: DiscoveryLock,
}

impl std::fmt::Debug for DiscoveryDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryDeps")
            .field("workspace_root", &self.workspace_root)
            .field("explorer", &self.explorer.is_some())
            .field("worker", &self.worker.is_some())
            .field("web", &self.web.is_some())
            .field("sink", &self.sink.is_some())
            .finish_non_exhaustive()
    }
}

impl DiscoveryDeps {
    /// Deps with the two stores, the model and the event sink.
    #[must_use]
    pub fn new(
        log: Arc<dyn DiscoveryLogPort>,
        sources: Arc<dyn SourceStorePort>,
        model: Model,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            log,
            sources,
            model,
            events,
            decider: Decider::none(),
            explorer: None,
            worker: None,
            fs: None,
            web: None,
            shell: None,
            sink: None,
            workspace_root: ".".to_owned(),
            human: "user".to_owned(),
            clock: system_now_ms,
            lock: DiscoveryLock::new(),
        }
    }

    /// The decision model that reads free-text answers.
    #[must_use]
    pub fn with_decider(mut self, decider: Decider) -> Self {
        self.decider = decider;
        self
    }

    /// The read-only agent that explores the repository and the web.
    #[must_use]
    pub fn with_explorer(mut self, agent: Arc<AgentService>) -> Self {
        self.explorer = Some(agent);
        self
    }

    /// The acting agent that runs spikes.
    #[must_use]
    pub fn with_worker(mut self, agent: Arc<AgentService>) -> Self {
        self.worker = Some(agent);
        self
    }

    /// The workspace files, to read repository sources.
    #[must_use]
    pub fn with_fs(mut self, fs: Arc<dyn FileSystemPort>) -> Self {
        self.fs = Some(fs);
        self
    }

    /// The web, to fetch sources.
    #[must_use]
    pub fn with_web(mut self, web: Arc<dyn WebPort>) -> Self {
        self.web = Some(web);
        self
    }

    /// The shell, for spike worktrees.
    #[must_use]
    pub fn with_shell(mut self, shell: Arc<dyn ShellPort>) -> Self {
        self.shell = Some(shell);
        self
    }

    /// Where the document and the derived files are written.
    #[must_use]
    pub fn with_sink(mut self, sink: Arc<dyn SpecSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// The workspace root the logs are listed by.
    #[must_use]
    pub fn with_workspace_root(mut self, root: impl Into<String>) -> Self {
        self.workspace_root = root.into();
        self
    }

    /// The name recorded for the person's verdicts (`human:<name>`).
    #[must_use]
    pub fn with_human(mut self, name: impl Into<String>) -> Self {
        self.human = name.into();
        self
    }

    /// Replaces the wall clock, for tests.
    #[must_use]
    pub fn with_clock(mut self, clock: fn() -> u64) -> Self {
        self.clock = clock;
        self
    }

    /// The lock the tools' file port consults: the folder of an open
    /// discovery is locked while the engine works on it.
    #[must_use]
    pub fn with_lock(mut self, lock: DiscoveryLock) -> Self {
        self.lock = lock;
        self
    }
}

/// What one step did, or why it stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// The open section was written and audited.
    Written {
        /// The section.
        section: Section,
        /// Blocking findings.
        errors: u32,
        /// Non-blocking findings.
        warnings: u32,
        /// Whether the isolated reviewer ran.
        isolated: bool,
    },
    /// The open section was approved; the next one opens.
    Approved {
        /// The section.
        section: Section,
        /// Who approved.
        by: Actor,
        /// The note, if any.
        note: Option<String>,
    },
    /// The open section was rejected and will be rewritten.
    Rejected {
        /// The section.
        section: Section,
        /// Who rejected.
        by: Actor,
        /// What to fix.
        note: String,
    },
    /// A person must answer.
    NeedsHuman {
        /// The section.
        section: Section,
        /// What is asked.
        prompt: String,
    },
    /// The records the gate needs were produced.
    GateOpened {
        /// The section.
        section: Section,
        /// What was produced.
        produced: Vec<String>,
    },
    /// The gate is closed and the engine could not open it.
    GateClosed {
        /// The section.
        section: Section,
        /// What is missing.
        missing: Vec<String>,
        /// What the attempt produced anyway (records checked, a spike
        /// scored), so the trail is visible even when it fell short.
        produced: Vec<String>,
    },
    /// Every section is approved: a person may accept.
    ReadyToAccept,
    /// The log is frozen.
    Frozen(Status),
}

impl StepOutcome {
    /// Whether the engine must stop and wait after this outcome.
    #[must_use]
    pub fn stops(&self) -> bool {
        matches!(
            self,
            Self::NeedsHuman { .. }
                | Self::GateClosed { .. }
                | Self::ReadyToAccept
                | Self::Frozen(_)
        )
    }
}

impl std::fmt::Display for StepOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Written {
                section,
                errors,
                warnings,
                isolated,
            } => write!(
                f,
                "`{section}` written and audited: {errors} error(s), {warnings} warning(s){}",
                if *isolated {
                    ", isolated reviewer ran"
                } else {
                    ""
                }
            ),
            Self::Approved { section, by, note } => match note {
                Some(note) => write!(f, "`{section}` approved by {by} with a note: {note}"),
                None => write!(f, "`{section}` approved by {by}"),
            },
            Self::Rejected { section, by, note } => {
                write!(f, "`{section}` rejected by {by}: {note}")
            }
            Self::NeedsHuman { prompt, .. } => f.write_str(prompt),
            Self::GateOpened { section, produced } => {
                write!(f, "gate of `{section}` opened: {}", produced.join(", "))
            }
            Self::GateClosed {
                section,
                missing,
                produced,
            } => {
                if !produced.is_empty() {
                    writeln!(f, "{}", produced.join(", "))?;
                }
                write!(f, "gate of `{section}` closed: {}", missing.join("; "))
            }
            Self::ReadyToAccept => f.write_str(
                "every section is approved; `/research accept` freezes the discovery and \
                 derives the Spec",
            ),
            Self::Frozen(status) => write!(f, "the discovery is {status:?}"),
        }
    }
}

/// Where a discovery is, as `/research` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusReport {
    /// The discovery.
    pub id: DiscoveryId,
    /// The level.
    pub level: Level,
    /// Open, accepted or aborted.
    pub status: Status,
    /// The open section.
    pub open: Option<Section>,
    /// Sections approved.
    pub approved: usize,
    /// Sections of the level.
    pub total: usize,
    /// The next legal step.
    pub next: NextStep,
    /// Model calls and tokens of this engine so far.
    pub usage: StageUsage,
}

impl std::fmt::Display for StatusReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "discovery {} · {:?} · {}/{} approved",
            self.id, self.level, self.approved, self.total
        )?;
        match &self.next {
            NextStep::Write(s) => write!(f, " · next: write `{s}`"),
            NextStep::OpenGate { section, missing } => {
                write!(f, " · gate of `{section}` closed: {}", missing.join("; "))
            }
            NextStep::FixAudit { section, errors } => {
                write!(f, " · `{section}` has {errors} audit error(s) to fix")
            }
            NextStep::Verdict(s) => write!(f, " · `{s}` waits for a verdict"),
            NextStep::Accept => f.write_str(" · ready to accept"),
            NextStep::Frozen => write!(f, " · {:?}", self.status),
        }
    }
}

/// The log of a discovery with its replayed state.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The events.
    pub events: Vec<Logged>,
    /// The state.
    pub state: DiscoveryState,
}

/// The discovery engine.
#[derive(Debug, Clone)]
pub struct DiscoveryEngine {
    deps: DiscoveryDeps,
}

impl DiscoveryEngine {
    /// An engine over `deps`.
    #[must_use]
    pub fn new(deps: DiscoveryDeps) -> Self {
        Self { deps }
    }

    /// The deps, for the records and spikes helpers.
    pub(crate) fn deps(&self) -> &DiscoveryDeps {
        &self.deps
    }

    pub(crate) fn now(&self) -> u64 {
        (self.deps.clock)()
    }

    pub(crate) fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        self.deps.events.emit(DomainEvent::Notice {
            level,
            message: message.into(),
        });
    }

    pub(crate) fn emit_markdown(&self, title: String, source: String) {
        let block = Block::new(BlockKind::Markdown {
            title: Some(title),
            source,
        });
        if block.validate().is_ok() {
            self.deps.events.emit(DomainEvent::BlockAdded { block });
        }
    }

    /// The workspace root.
    #[must_use]
    pub fn workspace_root(&self) -> &str {
        &self.deps.workspace_root
    }

    /// Model calls and tokens so far.
    #[must_use]
    pub fn usage(&self) -> StageUsage {
        self.deps.model.usage()
    }

    /// Appends `event` to `id` and keeps the document current.
    ///
    /// # Errors
    ///
    /// The log's.
    pub(crate) async fn append(
        &self,
        id: &DiscoveryId,
        event: DiscoveryEvent,
    ) -> Result<Logged, DiscoveryError> {
        let logged = self.deps.log.append(id, event, self.now()).await?;
        Ok(logged)
    }

    /// Loads and replays `id`.
    ///
    /// # Errors
    ///
    /// [`DiscoveryError::Unknown`] or the log's.
    pub async fn load(&self, id: &DiscoveryId) -> Result<Loaded, DiscoveryError> {
        let events = match self.deps.log.load(id).await {
            Ok(events) => events,
            Err(DiscoveryLogError::NotFound(_)) => return Err(DiscoveryError::Unknown(id.clone())),
            Err(e) => return Err(e.into()),
        };
        let state =
            replay(&events).map_err(|e| DiscoveryLogError::Corrupt(id.clone(), e.to_string()))?;
        Ok(Loaded { events, state })
    }

    /// The folder name of a new discovery about `problem`: the next
    /// number under `.crest/specs/` and a slug of the request.
    async fn next_id(&self, problem: &str) -> Result<DiscoveryId, DiscoveryError> {
        let mut number = 0;
        if let Some(sink) = &self.deps.sink {
            let folders = sink
                .folders()
                .await
                .map_err(|e| DiscoveryError::Files(e.to_string()))?;
            number = folders
                .iter()
                .filter_map(|f| folder_number(f))
                .max()
                .unwrap_or(0);
        }
        let listed = self.deps.log.list(&self.deps.workspace_root).await?;
        for summary in &listed {
            if let Some(n) = folder_number(summary.id.as_str()) {
                number = number.max(n);
            }
        }
        let words: String = problem
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" ");
        Ok(DiscoveryId::new(format!(
            "{:04}-{}",
            number + 1,
            slug(&words)
        )))
    }

    /// Starts a discovery of `problem` at `level`.
    ///
    /// # Errors
    ///
    /// The log's.
    pub async fn start(&self, problem: &str, level: Level) -> Result<DiscoveryId, DiscoveryError> {
        let id = self.next_id(problem).await?;
        self.deps
            .log
            .start(&id, &self.deps.workspace_root, problem, level, self.now())
            .await?;
        self.deps.lock.lock(&id);
        self.render(&id).await?;
        Ok(id)
    }

    /// Where `id` is.
    ///
    /// # Errors
    ///
    /// As [`DiscoveryEngine::load`].
    pub async fn status(&self, id: &DiscoveryId) -> Result<StatusReport, DiscoveryError> {
        let loaded = self.load(id).await?;
        Ok(StatusReport {
            id: id.clone(),
            level: loaded.state.level,
            status: loaded.state.status,
            open: loaded.state.open(),
            approved: loaded.state.approved.len(),
            total: loaded.state.level.sections().len(),
            next: loaded.state.next_step(),
            usage: self.usage(),
        })
    }

    /// The text of `section` as last written, if any.
    pub(crate) async fn section_text(
        &self,
        id: &DiscoveryId,
        section: Section,
    ) -> Result<Option<String>, DiscoveryError> {
        Ok(self
            .deps
            .log
            .artifact(id, &section_artifact(section))
            .await?)
    }

    pub(crate) async fn artifact_json<T: serde::de::DeserializeOwned + Default>(
        &self,
        id: &DiscoveryId,
        name: &str,
    ) -> Result<T, DiscoveryError> {
        match self.deps.log.artifact(id, name).await? {
            Some(text) => serde_json::from_str(&text)
                .map_err(|e| DiscoveryLogError::Corrupt(id.clone(), format!("{name}: {e}")).into()),
            None => Ok(T::default()),
        }
    }

    pub(crate) async fn put_json<T: serde::Serialize>(
        &self,
        id: &DiscoveryId,
        name: &str,
        value: &T,
    ) -> Result<(), DiscoveryError> {
        let text = serde_json::to_string_pretty(value)
            .map_err(|e| DiscoveryError::Files(format!("{name}: {e}")))?;
        Ok(self.deps.log.put_artifact(id, name, &text).await?)
    }

    /// The evidence, claims and method records of `id`.
    pub(crate) async fn records(
        &self,
        id: &DiscoveryId,
    ) -> Result<
        (
            Vec<EvidenceRecord>,
            Vec<ClaimRecord>,
            Vec<(domain::discovery::RecordRef, String)>,
        ),
        DiscoveryError,
    > {
        let evidence: Vec<EvidenceRecord> =
            self.artifact_json(id, records::names::EVIDENCE).await?;
        let claims: Vec<ClaimRecord> = self.artifact_json(id, records::names::CLAIMS).await?;
        let mut texts = Vec::new();
        for (record, name) in [
            (
                domain::discovery::RecordRef::Structure,
                records::names::STRUCTURE,
            ),
            (
                domain::discovery::RecordRef::Variables,
                records::names::VARIABLES,
            ),
            (domain::discovery::RecordRef::Cld, records::names::CLD),
            (domain::discovery::RecordRef::Risks, records::names::RISKS),
        ] {
            if let Some(text) = self.deps.log.artifact(id, name).await? {
                texts.push((record, text));
            }
        }
        Ok((evidence, claims, texts))
    }

    /// Builds the packet of `section`.
    pub(crate) async fn packet(
        &self,
        id: &DiscoveryId,
        state: &DiscoveryState,
        section: Section,
        correction: Option<String>,
    ) -> Result<Packet, DiscoveryError> {
        let mut approved = Vec::new();
        for s in state.level.sections() {
            if *s == section {
                break;
            }
            if state.approved.contains_key(s)
                && let Some(text) = self.section_text(id, *s).await?
            {
                approved.push((*s, text));
            }
        }
        let sources: Vec<StoredSource> = self.deps.sources.sources(id).await?;
        let (evidence, claims, texts) = self.records(id).await?;
        let evidence: Vec<(u32, String, Span)> = evidence
            .into_iter()
            .map(|e| {
                (
                    e.id,
                    e.claim,
                    Span {
                        source: e.source,
                        span: e.span,
                    },
                )
            })
            .collect();
        let claims: Vec<(u32, String, Span)> = claims
            .into_iter()
            .map(|c| {
                let summary = c.summary();
                (
                    c.claim.id.0,
                    summary,
                    Span {
                        source: c.claim.source,
                        span: c.claim.span,
                    },
                )
            })
            .collect();
        let citable = citable_items(state, section, &sources, &evidence, &claims, &texts);
        Ok(Packet {
            problem: state.problem.clone(),
            level: state.level,
            section,
            approved,
            citable,
            correction,
        })
    }

    /// Writes `discovery.md` and `events.jsonl` to the sink, when there
    /// is one.
    pub(crate) async fn render(&self, id: &DiscoveryId) -> Result<(), DiscoveryError> {
        let Some(sink) = &self.deps.sink else {
            return Ok(());
        };
        let loaded = self.load(id).await?;
        let mut texts = Vec::new();
        for s in loaded.state.level.sections() {
            if let Some(text) = self.section_text(id, *s).await? {
                texts.push((*s, text));
            }
        }
        let sources = self.deps.sources.sources(id).await?;
        let doc = discovery_markdown(id, &loaded.state, &texts, &sources, &loaded.events);
        sink.write(id.as_str(), "discovery.md", &doc)
            .await
            .map_err(|e| DiscoveryError::Files(e.to_string()))?;
        sink.write(id.as_str(), "events.jsonl", &events_jsonl(&loaded.events))
            .await
            .map_err(|e| DiscoveryError::Files(e.to_string()))?;
        Ok(())
    }

    /// One step of `id`: whatever the state says is legal next.
    ///
    /// # Errors
    ///
    /// The stores', the model's, or a refused event.
    pub async fn step(&self, id: &DiscoveryId) -> Result<StepOutcome, DiscoveryError> {
        let loaded = self.load(id).await?;
        let outcome = match loaded.state.next_step() {
            NextStep::Frozen => StepOutcome::Frozen(loaded.state.status),
            NextStep::Accept => StepOutcome::ReadyToAccept,
            NextStep::OpenGate { section, missing } => {
                super::gates::open_gate(self, id, &loaded, section, missing).await?
            }
            NextStep::Write(section) => {
                // The research level has no gates, but Findings still needs
                // something to cite: explore once before its first draft.
                if section == Section::Findings
                    && loaded.state.level == Level::Research
                    && loaded.state.sources.is_empty()
                    && self.deps.explorer.is_some()
                    && !explored_before(&loaded.events)
                {
                    return super::explore::open_problem_gate(
                        self,
                        id,
                        &loaded,
                        section,
                        Vec::new(),
                    )
                    .await;
                }
                let correction = reject_note(&loaded.events, section);
                self.write(id, &loaded, section, correction).await?
            }
            NextStep::FixAudit { section, .. } => {
                if fix_rounds(&loaded.events, section) >= MAX_FIX_ROUNDS {
                    let report = self.last_audit(id, &loaded.events, section).await?;
                    let findings = report
                        .map(|r| r.correction())
                        .unwrap_or_else(|| "the audit found errors".to_owned());
                    StepOutcome::NeedsHuman {
                        section,
                        prompt: format!(
                            "`{section}` still fails its audit after {MAX_FIX_ROUNDS} rewrites:\n{findings}\n\
                             Say what to change, or answer `yes` to approve it anyway."
                        ),
                    }
                } else {
                    let report = self.last_audit(id, &loaded.events, section).await?;
                    let correction = correction_for(&loaded.events, section, report.as_ref());
                    self.write(id, &loaded, section, correction).await?
                }
            }
            NextStep::Verdict(section) => self.verdict(id, &loaded, section).await?,
        };
        Ok(outcome)
    }

    /// Steps `id` until something stops the engine.
    ///
    /// # Errors
    ///
    /// As [`DiscoveryEngine::step`].
    pub async fn run(&self, id: &DiscoveryId) -> Result<Vec<StepOutcome>, DiscoveryError> {
        let mut outcomes = Vec::new();
        loop {
            let outcome = self.step(id).await?;
            let stop = outcome.stops();
            outcomes.push(outcome);
            if stop || outcomes.len() > 200 {
                return Ok(outcomes);
            }
        }
    }

    /// The last audit report of `section`, from its artifact.
    async fn last_audit(
        &self,
        id: &DiscoveryId,
        events: &[Logged],
        section: Section,
    ) -> Result<Option<AuditReport>, DiscoveryError> {
        let Some(seq) = events.iter().rev().find_map(|e| match &e.event {
            DiscoveryEvent::Audited { section: s, .. } if *s == section => Some(e.seq),
            _ => None,
        }) else {
            return Ok(None);
        };
        let name = audit_artifact(section, seq);
        match self.deps.log.artifact(id, &name).await? {
            Some(text) => Ok(serde_json::from_str(&text).ok()),
            None => Ok(None),
        }
    }

    /// Writes and audits `section`.
    async fn write(
        &self,
        id: &DiscoveryId,
        loaded: &Loaded,
        section: Section,
        correction: Option<String>,
    ) -> Result<StepOutcome, DiscoveryError> {
        let state = &loaded.state;
        let packet = self.packet(id, state, section, correction).await?;
        let draft: Draft = write_section(&self.deps.model, &packet).await?;
        if draft.text.trim().is_empty() {
            return Err(DiscoveryError::Contract {
                section,
                findings: draft.findings,
            });
        }
        let hash = text_hash(&draft.text);
        let written = self
            .append(id, DiscoveryEvent::SectionWritten { section, hash })
            .await?;
        self.deps
            .log
            .put_artifact(id, &section_artifact(section), &draft.text)
            .await?;
        self.deps
            .log
            .put_artifact(
                id,
                &format!(
                    "drafts/{}-{}.md",
                    super::render::section_key(section),
                    written.seq
                ),
                &draft.text,
            )
            .await?;
        if !draft.passes() {
            self.notice(
                NoticeLevel::Warning,
                format!(
                    "`{section}`: the writer could not meet the contract in {} attempts:\n{}",
                    draft.attempts,
                    findings_text(&draft.findings)
                ),
            );
        }
        let isolated = matches!(
            section.verdict_policy(state.level),
            VerdictPolicy::Evaluator
        ) || state.level == Level::Deep;
        let report = audit(&self.deps.model, &packet, &draft.text, isolated).await?;
        let (errors, warnings) = report.counts();
        let audited = self
            .append(
                id,
                DiscoveryEvent::Audited {
                    section,
                    errors,
                    warnings,
                    isolated: report.isolated(),
                    by: report.by.clone(),
                },
            )
            .await?;
        self.put_json(id, &audit_artifact(section, audited.seq), &report)
            .await?;
        self.render(id).await?;
        self.emit_markdown(
            format!("{id} · {section}"),
            format!(
                "## {section}\n\n{}\n\n---\n{}",
                draft.text.trim(),
                report.markdown()
            ),
        );
        Ok(StepOutcome::Written {
            section,
            errors,
            warnings,
            isolated: report.isolated(),
        })
    }

    /// The verdict step of `section`, by its policy.
    async fn verdict(
        &self,
        id: &DiscoveryId,
        loaded: &Loaded,
        section: Section,
    ) -> Result<StepOutcome, DiscoveryError> {
        let state = &loaded.state;
        let report = self
            .last_audit(id, &loaded.events, section)
            .await?
            .unwrap_or(AuditReport {
                section,
                findings: Vec::new(),
                reviewer: None,
                by: Actor::Process("audit".into()),
            });
        match section.verdict_policy(state.level) {
            VerdictPolicy::Human => Ok(StepOutcome::NeedsHuman {
                section,
                prompt: human_prompt(section, &report, false),
            }),
            VerdictPolicy::AuditOnly => {
                let by = Actor::Process("audit".into());
                self.append(
                    id,
                    DiscoveryEvent::Verdict {
                        section,
                        choice: Verdict::ApproveAsIs,
                        by: by.clone(),
                        note: None,
                    },
                )
                .await?;
                self.after_verdict(id, section, Verdict::ApproveAsIs, by, None)
                    .await
            }
            VerdictPolicy::Evaluator => {
                if model_rejects(&loaded.events, section) >= MAX_MODEL_ROUNDS {
                    return Ok(StepOutcome::NeedsHuman {
                        section,
                        prompt: human_prompt(section, &report, true),
                    });
                }
                let Some(draft) = self.section_text(id, section).await? else {
                    return Err(DiscoveryError::Files(format!(
                        "{section} has no stored text"
                    )));
                };
                let packet = self.packet(id, state, section, None).await?;
                let evaluation = evaluate(&self.deps.model, &packet, &draft, &report).await?;
                let by = Actor::Model(self.deps.model.id_of(super::evaluator::EVALUATOR_ROLE));
                let note =
                    (!evaluation.note.trim().is_empty()).then(|| evaluation.note.trim().to_owned());
                self.append(
                    id,
                    DiscoveryEvent::Verdict {
                        section,
                        choice: evaluation.verdict,
                        by: by.clone(),
                        note: note.clone(),
                    },
                )
                .await?;
                self.after_verdict(id, section, evaluation.verdict, by, note)
                    .await
            }
        }
    }

    async fn after_verdict(
        &self,
        id: &DiscoveryId,
        section: Section,
        choice: Verdict,
        by: Actor,
        note: Option<String>,
    ) -> Result<StepOutcome, DiscoveryError> {
        self.render(id).await?;
        let outcome = if choice.approves() {
            StepOutcome::Approved { section, by, note }
        } else {
            StepOutcome::Rejected {
                section,
                by,
                note: note.unwrap_or_else(|| "rewrite".to_owned()),
            }
        };
        self.notice(NoticeLevel::Info, outcome.to_string());
        Ok(outcome)
    }

    /// A person's answer at a verdict (or at an escalated audit).
    ///
    /// # Errors
    ///
    /// [`DiscoveryError::NotWaiting`] when the state does not wait for one.
    pub async fn answer(
        &self,
        id: &DiscoveryId,
        text: &str,
    ) -> Result<StepOutcome, DiscoveryError> {
        let loaded = self.load(id).await?;
        let section = match loaded.state.next_step() {
            NextStep::Verdict(section) | NextStep::FixAudit { section, .. } => section,
            NextStep::Accept => {
                return if domain_is_yes(text) {
                    self.accept(id).await
                } else {
                    Err(DiscoveryError::NotWaiting(
                        "ready to accept; answer `yes` or `/research accept`".to_owned(),
                    ))
                };
            }
            other => return Err(DiscoveryError::NotWaiting(format!("{other:?}"))),
        };
        let Some((choice, note)) = read_human_verdict(text, &self.deps.decider).await else {
            return Err(DiscoveryError::NotWaiting(
                "an empty line is not a verdict; answer `yes`, `yes, but …` or a correction"
                    .to_owned(),
            ));
        };
        let by = Actor::Human(self.deps.human.clone());
        self.append(
            id,
            DiscoveryEvent::Verdict {
                section,
                choice,
                by: by.clone(),
                note: note.clone(),
            },
        )
        .await?;
        self.after_verdict(id, section, choice, by, note).await
    }

    /// Reopens `section`; returns the sections whose verdicts were dropped.
    ///
    /// # Errors
    ///
    /// A refused event (nothing to reopen) or the log's.
    pub async fn reopen(
        &self,
        id: &DiscoveryId,
        section: Section,
        reason: &str,
    ) -> Result<Vec<Section>, DiscoveryError> {
        let loaded = self.load(id).await?;
        let dropped: Vec<Section> = loaded
            .state
            .approved
            .keys()
            .copied()
            .filter(|s| *s >= section)
            .collect();
        self.append(
            id,
            DiscoveryEvent::Reopened {
                section,
                reason: reason.to_owned(),
            },
        )
        .await?;
        self.render(id).await?;
        Ok(dropped)
    }

    /// Abandons `id`. Once `Aborted` is in the log the folder is unlocked
    /// and the document rendered on a best-effort basis: nothing after
    /// the event can fail the command.
    ///
    /// # Errors
    ///
    /// The log's.
    pub async fn abort(&self, id: &DiscoveryId) -> Result<(), DiscoveryError> {
        self.append(id, DiscoveryEvent::Aborted).await?;
        self.deps.lock.unlock(id);
        self.render_or_warn(id).await;
        Ok(())
    }

    /// Renders the document, warning instead of failing.
    async fn render_or_warn(&self, id: &DiscoveryId) {
        if let Err(err) = self.render(id).await {
            self.notice(
                NoticeLevel::Warning,
                format!("discovery.md of {id} could not be written: {err}"),
            );
        }
    }

    /// Derives the Spec of an accepted `id`, reporting the outcome as a
    /// notice: a failure here leaves the log frozen and is retried with
    /// `/research accept`.
    async fn derive_or_warn(&self, id: &DiscoveryId) {
        match super::derive::derive(self, id).await {
            Ok(()) => self.notice(
                NoticeLevel::Info,
                format!("discovery {id} accepted; spec.md and plan.json derived"),
            ),
            Err(err) => self.notice(
                NoticeLevel::Warning,
                format!(
                    "discovery {id} is accepted but its Spec could not be derived: {err}; \
                     `/research accept` derives it again"
                ),
            ),
        }
    }

    /// Locks the folder of `id` again (on resume).
    pub fn lock(&self, id: &DiscoveryId) {
        self.deps.lock.lock(id);
    }

    /// Accepts `id` as the person, freezes the log and derives the Spec.
    /// On an already accepted discovery it derives the Spec again, so a
    /// derivation that failed (a sink or model error) can be retried.
    ///
    /// # Errors
    ///
    /// A refused event (a section is still open) or the log's. Nothing
    /// after the `Accepted` event fails the command: the folder is
    /// unlocked and the rendering and derivation report through notices.
    pub async fn accept(&self, id: &DiscoveryId) -> Result<StepOutcome, DiscoveryError> {
        let loaded = self.load(id).await?;
        if loaded.state.status != Status::Accepted {
            self.append(
                id,
                DiscoveryEvent::Accepted {
                    by: Actor::Human(self.deps.human.clone()),
                },
            )
            .await?;
            self.deps.lock.unlock(id);
            self.render_or_warn(id).await;
        }
        self.derive_or_warn(id).await;
        Ok(StepOutcome::Frozen(Status::Accepted))
    }

    /// The discoveries of the workspace, newest first.
    ///
    /// # Errors
    ///
    /// The log's.
    pub async fn list(
        &self,
    ) -> Result<Vec<crate::ports::discovery::DiscoverySummary>, DiscoveryError> {
        Ok(self.deps.log.list(&self.deps.workspace_root).await?)
    }

    /// The model, for the helpers.
    pub(crate) fn model(&self) -> &Model {
        &self.deps.model
    }

    /// The role the engine uses for `what`, for provenance.
    #[must_use]
    pub fn actor_for(&self, role: ModelRole) -> Actor {
        Actor::Model(self.deps.model.id_of(role))
    }
}

/// The artifact name of the audit report at `seq`.
#[must_use]
pub fn audit_artifact(section: Section, seq: u64) -> String {
    format!("audits/{}-{seq}.json", super::render::section_key(section))
}

/// Whether an exploration already ran (a record was checked), so a
/// research-level `Findings` with no sources is not explored twice.
fn explored_before(events: &[Logged]) -> bool {
    events
        .iter()
        .any(|e| matches!(e.event, DiscoveryEvent::RecordChecked { .. }))
}

/// Whether `text` is a plain yes.
fn domain_is_yes(text: &str) -> bool {
    crate::flows::research::gate::is_approval(text.trim()) && !text.trim().is_empty()
}

/// The note of the last rejection of `section`, standing until its next
/// verdict or a reopen: every rewrite in between (the one the rejection
/// asks for, and the audit fixes that may follow it) carries it.
#[must_use]
pub fn reject_note(events: &[Logged], section: Section) -> Option<String> {
    for e in events.iter().rev() {
        match &e.event {
            DiscoveryEvent::Reopened { .. } => return None,
            DiscoveryEvent::Verdict {
                section: s,
                choice,
                note,
                ..
            } if *s == section => {
                return (*choice == Verdict::RejectAndRevise)
                    .then(|| note.clone().unwrap_or_else(|| "rewrite".to_owned()));
            }
            _ => {}
        }
    }
    None
}

/// The correction a rewrite of `section` carries: the note of a rejection
/// not yet acted on, the findings of the last audit, or both (a person's
/// note on a draft that also fails its audit must not be lost).
#[must_use]
pub fn correction_for(
    events: &[Logged],
    section: Section,
    report: Option<&AuditReport>,
) -> Option<String> {
    let parts: Vec<String> = reject_note(events, section)
        .into_iter()
        .chain(report.map(AuditReport::correction))
        .filter(|s| !s.trim().is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Consecutive audits with errors of `section` since its last verdict or
/// reopen.
#[must_use]
pub fn fix_rounds(events: &[Logged], section: Section) -> u32 {
    let mut rounds = 0;
    for e in events.iter().rev() {
        match &e.event {
            DiscoveryEvent::Audited {
                section: s, errors, ..
            } if *s == section => {
                if *errors > 0 {
                    rounds += 1;
                } else {
                    break;
                }
            }
            DiscoveryEvent::Verdict { section: s, .. } if *s == section => break,
            DiscoveryEvent::Reopened { .. } => break,
            _ => {}
        }
    }
    rounds
}

/// Rejections by a model of `section` since its last approval or reopen.
#[must_use]
pub fn model_rejects(events: &[Logged], section: Section) -> u32 {
    let mut rejects = 0;
    for e in events.iter().rev() {
        match &e.event {
            DiscoveryEvent::Verdict {
                section: s,
                choice,
                by,
                ..
            } if *s == section => {
                if choice.approves() {
                    break;
                }
                if matches!(by, Actor::Model(_)) {
                    rejects += 1;
                }
            }
            DiscoveryEvent::Reopened { .. } => break,
            _ => {}
        }
    }
    rejects
}
