//! Subagents: run while the caller waits, or in the background while the
//! chat goes on.
//!
//! A job ([`AgentJob`]) is a prompt for one of two kinds of subagent:
//!
//! - **explore** (or **plan**): the read-only `task` subagent. It only
//!   reads and searches, never asks the user anything, and its own model
//!   role (`explore`, `plan`) runs it.
//! - **general**: an agent with the chat's tools, permissions and
//!   instructions. Its edits and commands ask through the chat's approval
//!   dialog, which names it (`agent #2 · …`), as the chat's own calls do.
//!
//! In the background ([`AgentRunner::spawn`]) a job gets an
//! [`Agent`](BlockKind::Agent) block in the timeline that follows its tool
//! calls and ends with its report. The report then waits in the
//! [`AgentInbox`]: a running turn of the chat reads it at its next model
//! call, an idle chat starts a turn for it (the window watches the block).
//! Background jobs need a [`Spawner`] ([`AgentRunner::enable_background`]):
//! without one (headless runs, which end with their turn) jobs run while
//! the caller waits.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use async_trait::async_trait;
use domain::block::{AgentStatus, Block, BlockId, BlockKind, ToolStatus};
use domain::conversation::Session;
use domain::model::{ModelPolicy, ModelRole};
use futures::FutureExt;
use futures::future::{AbortHandle, Abortable, BoxFuture};

use crate::flows::research::agents::NodeEvents;
use crate::ports::approval::{ApprovalError, ApprovalPort, ApprovalRequest, ApprovalResponse};
use crate::ports::events::{DomainEvent, EventSink};
use crate::ports::instructions::InstructionsPort;
use crate::ports::skills::SkillsPort;
use crate::tools::registry::ToolRegistry;
use crate::tools::task::{TaskTool, final_text, with_note};
use crate::usecases::agents::{AgentInbox, AgentReport};
use crate::usecases::permissions::ProjectRules;
use crate::usecases::run_turn::{AgentDeps, AgentService, SharedPermissions, TurnOptions};

/// Background agents running at once, at most.
pub const MAX_RUNNING: usize = 4;
/// Tool calls an agent's block shows.
const ACTIVITY_LINES: usize = 8;
/// Characters of the prompt that name a job when nothing else does.
const NAME_CHARS: usize = 48;

/// Runs a future on the app's runtime, detached.
pub type Spawner = Arc<dyn Fn(BoxFuture<'static, ()>) + Send + Sync>;

/// Which subagent runs a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    /// The read-only subagent, with the `explore` role.
    Explore,
    /// The read-only subagent, with the `plan` role.
    Plan,
    /// An agent with the chat's tools and approvals.
    General,
}

impl AgentKind {
    /// The kind a skill's `agent` (or `/agent <kind>`) names: `Explore`
    /// and `Plan` are read-only, anything else is general.
    #[must_use]
    pub fn named(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "explore" => Self::Explore,
            "plan" => Self::Plan,
            _ => Self::General,
        }
    }

    /// The word shown for it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::General => "general",
        }
    }
}

/// Who started a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentOrigin {
    /// The user (`/agent`, `&`, a forked skill they ran).
    User,
    /// The chat's agent (`task`, `skill` with `background`).
    Model,
}

impl AgentOrigin {
    const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Model => "model",
        }
    }
}

/// What a subagent is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentJob {
    /// A short title (`Find permission checks`).
    pub name: String,
    /// Everything the subagent needs: it does not see the chat.
    pub prompt: String,
    /// Which subagent runs it.
    pub kind: AgentKind,
    /// The model to use instead of its role's (alias, role or id).
    pub model: Option<String>,
    /// Who started it.
    pub origin: AgentOrigin,
}

impl AgentJob {
    /// A job of `kind` for `prompt`, named after its first words.
    #[must_use]
    pub fn new(kind: AgentKind, prompt: impl Into<String>, origin: AgentOrigin) -> Self {
        let prompt = prompt.into();
        Self {
            name: title_of(&prompt),
            prompt,
            kind,
            model: None,
            origin,
        }
    }

    /// The same job named `name` (when not empty).
    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        if !name.trim().is_empty() {
            self.name = name.trim().to_owned();
        }
        self
    }

    /// The same job run by `model`, if any.
    #[must_use]
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }
}

/// The first line of `prompt`, cut to a title.
fn title_of(prompt: &str) -> String {
    let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim().trim_start_matches('#').trim();
    match line.char_indices().nth(NAME_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

/// What a general agent is built from: the chat's parts.
#[derive(Clone)]
pub struct GeneralKit {
    /// The chat's model, approvals and telemetry (its events are replaced).
    pub deps: AgentDeps,
    /// The chat's tools.
    pub tools: ToolRegistry,
    /// The chat's models.
    pub models: ModelPolicy,
    /// The chat's permissions.
    pub permissions: SharedPermissions,
    /// The chat's system prompt.
    pub system_prompt: String,
    /// The project's instructions.
    pub instructions: Option<Arc<dyn InstructionsPort>>,
    /// The skills, for the nested ones it finds.
    pub skills: Option<Arc<dyn SkillsPort>>,
    /// Where "always in project" answers are written.
    pub project_rules: Option<ProjectRules>,
}

impl GeneralKit {
    fn service(&self, events: Arc<dyn EventSink>, approval: Arc<dyn ApprovalPort>) -> AgentService {
        let mut deps = self.deps.clone();
        deps.events = events;
        deps.approval = approval;
        let mut service = AgentService::new(
            deps,
            self.tools.clone(),
            self.models.clone(),
            Arc::clone(&self.permissions),
        )
        .with_system_prompt(self.system_prompt.clone());
        if let Some(port) = &self.instructions {
            service = service.with_instructions(Arc::clone(port));
        }
        if let Some(port) = &self.skills {
            service = service.with_skills(Arc::clone(port));
        }
        if let Some(rules) = &self.project_rules {
            service = service.with_project_rules(rules.clone());
        }
        service
    }
}

/// One of the session's background agents, as `/agents` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSummary {
    /// Its number.
    pub id: u64,
    /// Its title.
    pub name: String,
    /// Its kind.
    pub kind: AgentKind,
    /// Where it is.
    pub status: AgentStatus,
    /// Who started it.
    pub origin: AgentOrigin,
}

/// A background job and its block.
struct Job {
    block_id: BlockId,
    name: String,
    kind: AgentKind,
    origin: AgentOrigin,
    prompt: String,
    status: AgentStatus,
    /// Its tool calls: the block of each, its line and its status.
    activity: Vec<(BlockId, String, ToolStatus)>,
    report: Option<String>,
    abort: Option<AbortHandle>,
}

impl Job {
    fn block(&self, id: u64) -> Block {
        let activity = self
            .activity
            .iter()
            .map(|(_, line, status)| format!("{line}{}", status_mark(*status)))
            .collect();
        Block {
            id: self.block_id,
            kind: BlockKind::Agent {
                agent_id: id,
                name: self.name.clone(),
                kind: self.kind.label().to_owned(),
                origin: self.origin.label().to_owned(),
                prompt: self.prompt.clone(),
                status: self.status,
                activity,
                report: self.report.clone(),
            },
        }
    }
}

/// What follows a tool call's line in an agent's block.
fn status_mark(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Pending | ToolStatus::Running => " …",
        ToolStatus::AwaitingApproval => " (waiting for your approval)",
        ToolStatus::Succeeded => " ✓",
        ToolStatus::Failed => " ✗",
        ToolStatus::Denied => " (denied)",
        ToolStatus::Cancelled => " (cancelled)",
    }
}

/// Runs subagent jobs, waited for or in the background (see the module
/// docs).
pub struct AgentRunner {
    explore: Arc<TaskTool>,
    general: OnceLock<GeneralKit>,
    events: Arc<dyn EventSink>,
    approval: Arc<dyn ApprovalPort>,
    inbox: AgentInbox,
    spawner: OnceLock<Spawner>,
    jobs: Mutex<BTreeMap<u64, Job>>,
    next: AtomicU64,
    /// The workspace root, for the agents' sessions.
    root: String,
}

impl std::fmt::Debug for AgentRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRunner")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl AgentRunner {
    /// A runner of read-only jobs on `explore`, showing background agents
    /// through `events`, asking through `approval` and leaving their
    /// reports in `inbox`. General jobs need [`Self::set_general`].
    #[must_use]
    pub fn new(
        explore: Arc<TaskTool>,
        events: Arc<dyn EventSink>,
        approval: Arc<dyn ApprovalPort>,
        inbox: AgentInbox,
        root: String,
    ) -> Self {
        Self {
            explore,
            general: OnceLock::new(),
            events,
            approval,
            inbox,
            spawner: OnceLock::new(),
            jobs: Mutex::default(),
            next: AtomicU64::new(1),
            root,
        }
    }

    /// Builds general agents from `kit` (the chat's parts, ready after
    /// the runner).
    pub fn set_general(&self, kit: GeneralKit) {
        let _ = self.general.set(kit);
    }

    /// Runs background jobs with `spawner`.
    pub fn enable_background(&self, spawner: Spawner) {
        let _ = self.spawner.set(spawner);
    }

    /// Whether jobs can run in the background here.
    #[must_use]
    pub fn background_available(&self) -> bool {
        self.spawner.get().is_some()
    }

    /// The inbox the reports go to.
    #[must_use]
    pub fn inbox(&self) -> &AgentInbox {
        &self.inbox
    }

    /// Runs `job` while the caller waits and returns its report. A general
    /// agent's tool calls show in the timeline as the research flow's do.
    pub fn run(&self, job: AgentJob) -> BoxFuture<'static, Result<String, String>> {
        match job.kind {
            AgentKind::Explore | AgentKind::Plan => self
                .explore
                .report(job.prompt, role_of(job.kind), job.model)
                .map(|r| r.map_err(|e| e.to_string()))
                .boxed(),
            AgentKind::General => {
                let events: Arc<dyn EventSink> =
                    Arc::new(NodeEvents::new(Arc::clone(&self.events)));
                self.general_run(job, events, Arc::clone(&self.approval))
            }
        }
    }

    fn general_run(
        &self,
        job: AgentJob,
        events: Arc<dyn EventSink>,
        approval: Arc<dyn ApprovalPort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let Some(kit) = self.general.get() else {
            return futures::future::ready(Err(
                "the general agent is not available here".to_owned()
            ))
            .boxed();
        };
        let service = kit.service(events, approval);
        let root = self.root.clone();
        let budget = kit.tools.budget();
        async move {
            let mut session = Session::new(root, 0);
            let mut opts = TurnOptions::new(ModelRole::Main);
            opts.turn_override = job.model;
            match service.run_turn(&mut session, job.prompt, opts).await {
                Ok(outcome) => {
                    Ok(budget.truncate_text(&with_note(final_text(&session), outcome.stop_reason)))
                }
                Err(err) => Err(format!("the agent failed: {err}")),
            }
        }
        .boxed()
    }

    /// Starts `job` in the background and returns its number.
    ///
    /// # Errors
    ///
    /// When background jobs are not available here, or
    /// [`MAX_RUNNING`] are running already.
    pub fn spawn(self: &Arc<Self>, job: AgentJob) -> Result<u64, String> {
        let Some(spawner) = self.spawner.get().cloned() else {
            return Err("background agents are not available here".to_owned());
        };
        if job.kind == AgentKind::General && self.general.get().is_none() {
            return Err("the general agent is not available here".to_owned());
        }
        let running = self
            .lock()
            .values()
            .filter(|job| job.status == AgentStatus::Running)
            .count();
        if running >= MAX_RUNNING {
            return Err(format!(
                "{MAX_RUNNING} agents are running already; wait for one or stop it (/agents)"
            ));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let entry = Job {
            block_id: BlockId::new(),
            name: job.name.clone(),
            kind: job.kind,
            origin: job.origin,
            prompt: job.prompt.clone(),
            status: AgentStatus::Running,
            activity: Vec::new(),
            report: None,
            abort: None,
        };
        let block = entry.block(id);
        self.lock().insert(id, entry);
        self.events.emit(DomainEvent::BlockAdded { block });
        let sink: Arc<dyn EventSink> = Arc::new(Activity {
            runner: Arc::clone(self),
            id,
        });
        let work = match job.kind {
            AgentKind::Explore | AgentKind::Plan => self
                .explore
                .report_to(job.prompt, role_of(job.kind), job.model, sink)
                .map(|r| r.map_err(|e| e.to_string()))
                .boxed(),
            AgentKind::General => {
                let approval: Arc<dyn ApprovalPort> = Arc::new(Labeled {
                    inner: Arc::clone(&self.approval),
                    label: format!("agent #{id} · {}", job.name),
                });
                self.general_run(job, sink, approval)
            }
        };
        let (handle, registration) = AbortHandle::new_pair();
        if let Some(entry) = self.lock().get_mut(&id) {
            entry.abort = Some(handle);
        }
        let runner = Arc::clone(self);
        spawner(
            async move {
                let result = Abortable::new(work, registration).await;
                runner.finish(id, result.ok());
            }
            .boxed(),
        );
        Ok(id)
    }

    /// Stops the agent `id`; `false` when it is not running.
    pub fn stop(&self, id: u64) -> bool {
        let abort = self
            .lock()
            .get_mut(&id)
            .filter(|job| job.status == AgentStatus::Running)
            .and_then(|job| job.abort.take());
        match abort {
            Some(abort) => {
                abort.abort();
                // The task may not run again to see it: end it here.
                self.finish(id, None);
                true
            }
            None => false,
        }
    }

    /// Stops every running agent and returns how many.
    pub fn stop_all(&self) -> usize {
        let ids: Vec<u64> = self
            .lock()
            .iter()
            .filter(|(_, job)| job.status == AgentStatus::Running)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter().filter(|id| self.stop(*id)).count()
    }

    /// The session's background agents, oldest first.
    #[must_use]
    pub fn list(&self) -> Vec<AgentSummary> {
        self.lock()
            .iter()
            .map(|(id, job)| AgentSummary {
                id: *id,
                name: job.name.clone(),
                kind: job.kind,
                status: job.status,
                origin: job.origin,
            })
            .collect()
    }

    /// Ends the job `id` with `result` (`None`: stopped), once.
    fn finish(&self, id: u64, result: Option<Result<String, String>>) {
        let (status, text) = match result {
            Some(Ok(report)) => (AgentStatus::Done, report),
            Some(Err(error)) => (AgentStatus::Failed, error),
            None => (
                AgentStatus::Cancelled,
                "stopped by the user before it finished".to_owned(),
            ),
        };
        let mut jobs = self.lock();
        let Some(job) = jobs.get_mut(&id) else {
            return;
        };
        if job.status.is_terminal() {
            return;
        }
        job.status = status;
        job.report = Some(text.clone());
        job.abort = None;
        // A job the user stopped needs nobody's attention.
        let tell = !(status == AgentStatus::Cancelled && job.origin == AgentOrigin::User);
        let report = AgentReport {
            id,
            name: job.name.clone(),
            status,
            text,
        };
        let block = job.block(id);
        drop(jobs);
        if tell {
            self.inbox.push(report);
        }
        self.events.emit(DomainEvent::BlockUpdated { block });
    }

    /// Records what the agent `id`'s tool call in `block` shows, and
    /// redraws its block.
    fn activity(&self, id: u64, block: BlockId, line: Option<String>, status: ToolStatus) {
        let mut jobs = self.lock();
        let Some(job) = jobs.get_mut(&id) else {
            return;
        };
        if job.status.is_terminal() {
            return;
        }
        match job.activity.iter_mut().find(|(b, ..)| *b == block) {
            Some(entry) => {
                if let Some(line) = line {
                    entry.1 = line;
                }
                entry.2 = status;
            }
            None => {
                let Some(line) = line else {
                    return;
                };
                job.activity.push((block, line, status));
                if job.activity.len() > ACTIVITY_LINES {
                    job.activity.remove(0);
                }
            }
        }
        let block = job.block(id);
        drop(jobs);
        self.events.emit(DomainEvent::BlockUpdated { block });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Job>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The role of a read-only job.
const fn role_of(kind: AgentKind) -> ModelRole {
    match kind {
        AgentKind::Plan => ModelRole::Plan,
        AgentKind::Explore | AgentKind::General => ModelRole::Explore,
    }
}

/// What a background agent's events become: its tool calls, lines of its
/// block; its notices, the chat's. The rest (its text, its usage) stays
/// out of the chat.
struct Activity {
    runner: Arc<AgentRunner>,
    id: u64,
}

impl EventSink for Activity {
    fn emit(&self, event: DomainEvent) {
        match event {
            DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                if let BlockKind::ToolCall {
                    name,
                    summary,
                    status,
                    ..
                } = &block.kind
                {
                    let line = if summary.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} {summary}")
                    };
                    self.runner.activity(self.id, block.id, Some(line), *status);
                }
            }
            DomainEvent::ToolStatusChanged {
                block_id, status, ..
            } => self.runner.activity(self.id, block_id, None, status),
            event @ DomainEvent::Notice { .. } => self.runner.events.emit(event),
            _ => {}
        }
    }
}

/// Asks through `inner`, naming the agent that asks.
struct Labeled {
    inner: Arc<dyn ApprovalPort>,
    label: String,
}

#[async_trait]
impl ApprovalPort for Labeled {
    async fn request(&self, mut req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError> {
        req.origin = Some(self.label.clone());
        self.inner.request(req).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::ports::approval::ApprovalResponse;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
    use crate::tools::budget::ResultBudget;
    use crate::tools::registry::{ToolContext, TypedTool};
    use crate::tools::task::{TaskInput, TaskPorts};

    struct Fixture {
        sink: Arc<RecordingSink>,
        inbox: AgentInbox,
        runner: Arc<AgentRunner>,
        explore: Arc<TaskTool>,
    }

    fn fixture(script: Vec<ScriptedResponse>, background: bool) -> Fixture {
        let llm = Arc::new(ScriptedLlm::new(script));
        let fs =
            Arc::new(InMemoryFs::new(test_root()).with_file("src/lib.rs", "pub fn check() {}\n"));
        let ports = TaskPorts {
            llm,
            telemetry: Arc::new(RecordingTelemetry::new()),
            models: ModelPolicy::default(),
            file_search: fs.clone(),
            content_search: fs.clone(),
            fs,
            web: None,
            instructions: None,
        };
        let explore = Arc::new(TaskTool::new(ports, ResultBudget::default()).unwrap());
        let sink = Arc::new(RecordingSink::new());
        let inbox = AgentInbox::new();
        let runner = Arc::new(AgentRunner::new(
            Arc::clone(&explore),
            sink.clone(),
            Arc::new(ScriptedApproval::always(ApprovalResponse::Approve)),
            inbox.clone(),
            "/ws".into(),
        ));
        if background {
            runner.enable_background(Arc::new(|work| {
                drop(tokio::spawn(work));
            }));
        }
        Fixture {
            sink,
            inbox,
            runner,
            explore,
        }
    }

    fn explore(prompt: &str, origin: AgentOrigin) -> AgentJob {
        AgentJob::new(AgentKind::Explore, prompt, origin)
    }

    /// The agent blocks the sink got, as (status, activity, report).
    fn agent_blocks(sink: &RecordingSink) -> Vec<(AgentStatus, Vec<String>, Option<String>)> {
        sink.events()
            .into_iter()
            .filter_map(|event| match event {
                DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                    match block.kind {
                        BlockKind::Agent {
                            status,
                            activity,
                            report,
                            ..
                        } => Some((status, activity, report)),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    async fn until(mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out");
    }

    #[test]
    fn jobs_are_named_after_their_first_line() {
        let job = AgentJob::new(
            AgentKind::General,
            "\n# Find where the permission checks happen in the whole codebase please\nmore",
            AgentOrigin::User,
        );
        assert_eq!(
            job.name,
            "Find where the permission checks happen in the w…"
        );
        assert_eq!(job.clone().named("  ").name, job.name);
        assert_eq!(job.named("Short").name, "Short");
        assert_eq!(AgentKind::named("Explore"), AgentKind::Explore);
        assert_eq!(AgentKind::named("general-purpose"), AgentKind::General);
    }

    #[tokio::test]
    async fn without_a_spawner_nothing_goes_to_the_background() {
        let f = fixture(Vec::new(), false);
        assert!(!f.runner.background_available());
        let err = f.runner.spawn(explore("x", AgentOrigin::User)).unwrap_err();
        assert!(err.contains("not available"), "{err}");
        assert!(f.sink.events().is_empty());
    }

    #[tokio::test]
    async fn a_background_agent_shows_its_calls_and_leaves_its_report() {
        let f = fixture(
            vec![
                ScriptedResponse::tool_use("toolu_1", "read_file", json!({ "path": "src/lib.rs" })),
                ScriptedResponse::text("`check` is in src/lib.rs:1."),
            ],
            true,
        );
        let id = f
            .runner
            .spawn(explore("Where is check?", AgentOrigin::User))
            .unwrap();
        assert_eq!(id, 1);
        let inbox = f.inbox.clone();
        until(|| !inbox.is_empty()).await;
        let reports = f.inbox.drain();
        assert_eq!(reports[0].status, AgentStatus::Done);
        assert!(reports[0].text.contains("src/lib.rs:1"));
        let blocks = agent_blocks(&f.sink);
        assert_eq!(blocks[0].0, AgentStatus::Running);
        assert!(
            blocks.iter().any(|(_, activity, _)| activity
                .iter()
                .any(|l| l.starts_with("read_file src/lib.rs"))),
            "{blocks:?}"
        );
        let (status, _, report) = blocks.last().unwrap();
        assert_eq!(*status, AgentStatus::Done);
        assert!(report.as_deref().unwrap().contains("src/lib.rs:1"));
        assert_eq!(f.runner.list()[0].status, AgentStatus::Done);
    }

    #[tokio::test]
    async fn stopping_an_agent_tells_the_model_only_when_it_started_it() {
        let f = fixture(Vec::new(), true);
        // Nothing runs before the test yields: both are still at the start.
        let mine = f.runner.spawn(explore("a", AgentOrigin::User)).unwrap();
        let its = f.runner.spawn(explore("b", AgentOrigin::Model)).unwrap();
        assert!(f.runner.stop(mine));
        assert!(!f.runner.stop(mine), "already stopped");
        assert_eq!(f.runner.stop_all(), 1);
        let reports = f.inbox.drain();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].id, its);
        assert_eq!(reports[0].status, AgentStatus::Cancelled);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(f.inbox.is_empty(), "the aborted tasks do not report again");
        let statuses: Vec<_> = f.runner.list().iter().map(|a| a.status).collect();
        assert_eq!(statuses, [AgentStatus::Cancelled, AgentStatus::Cancelled]);
    }

    #[tokio::test]
    async fn only_so_many_run_at_once() {
        let f = fixture(Vec::new(), true);
        for _ in 0..MAX_RUNNING {
            f.runner.spawn(explore("x", AgentOrigin::User)).unwrap();
        }
        let err = f.runner.spawn(explore("x", AgentOrigin::User)).unwrap_err();
        assert!(err.contains("running already"), "{err}");
        f.runner.stop_all();
    }

    #[tokio::test]
    async fn a_general_job_needs_the_chats_parts() {
        let f = fixture(Vec::new(), true);
        let err = f
            .runner
            .spawn(AgentJob::new(AgentKind::General, "x", AgentOrigin::User))
            .unwrap_err();
        assert!(err.contains("general agent"), "{err}");
        let err = f
            .runner
            .run(AgentJob::new(AgentKind::General, "x", AgentOrigin::User))
            .await
            .unwrap_err();
        assert!(err.contains("general agent"), "{err}");
    }

    #[tokio::test]
    async fn task_sends_its_job_to_the_background_when_asked() {
        let f = fixture(vec![ScriptedResponse::text("found")], true);
        let cell: Arc<OnceLock<Arc<AgentRunner>>> = Arc::default();
        let _ = cell.set(Arc::clone(&f.runner));
        // A `task` tool over the same ports, with the runner.
        let task = TaskTool::clone_for_test(&f.explore).with_runner(cell);
        let ctx = ToolContext::new(
            domain::conversation::SessionId::new(),
            domain::conversation::TurnId::new(),
            domain::conversation::ToolUseId::new("toolu_1"),
        );
        let input: TaskInput = serde_json::from_value(json!({
            "description": "Find check",
            "prompt": "Where is check?",
            "background": true,
        }))
        .unwrap();
        let prepared = task.prepare(input, &ctx).await.unwrap();
        let output = prepared.run.await.unwrap();
        assert!(
            output.text_content().starts_with("Started agent #1"),
            "{}",
            output.text_content()
        );
        assert_eq!(f.runner.list()[0].name, "Find check");
        let inbox = f.inbox.clone();
        until(|| !inbox.is_empty()).await;
        assert_eq!(f.inbox.drain()[0].text, "found");
    }
}
