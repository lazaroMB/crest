//! The agents the research nodes run turns with.
//!
//! [`explore_agent`] builds the read-only agent of the stages that only
//! explore (Observación, Datos, Investigación), the same way the `task`
//! tool builds its subagent:
//!
//! - its registry holds only the read tools (`list_files`, `find_file`,
//!   `tree`, `grep`, `read_file`), plus `run_command` when a shell is given:
//!   the policy lets through only the commands the shell classifies as
//!   reads (`git log`, `git blame`…), plus the MCP tools given;
//! - its permissions are [`PermissionPolicy::read_only`], which denies any
//!   call that is not a read whatever the mode;
//! - its approval port denies everything, so nothing reaches the user;
//! - its events are dropped: the node shows what it found in its own
//!   blocks, never the agent's streaming text or its final JSON.
//!
//! [`node_agent`] builds the agent of the stages that act (Experimentos,
//! Prototipo): the chat's tools, permissions and approvals, but its events
//! go through [`NodeEvents`], which keeps the tool calls and approvals in
//! the timeline and drops the streamed text (the final JSON of the turn)
//! and the turn framing, which belong to the chat's own turns.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::model::ModelPolicy;
use domain::permission::PermissionPolicy;

use crate::ports::approval::{ApprovalError, ApprovalPort, ApprovalRequest, ApprovalResponse};
use crate::ports::events::{DomainEvent, EventSink};
use crate::ports::fs::FileSystemPort;
use crate::ports::llm::LlmPort;
use crate::ports::search::{ContentSearchPort, FileSearchPort};
use crate::ports::shell::ShellPort;
use crate::ports::telemetry::TelemetryPort;
use crate::tools::budget::ResultBudget;
use crate::tools::files::{
    FindFileTool, GrepTool, ListFilesTool, ReadFileTool, RunCommandTool, TreeTool,
};
use crate::tools::registry::{RegisterError, Tool, ToolRegistry};
use crate::usecases::run_turn::{AgentDeps, AgentService, SharedPermissions};

/// Most model calls of one explore turn.
pub const EXPLORE_MAX_ITERATIONS: u32 = 30;

/// System prompt of the read-only agent.
pub const EXPLORE_SYSTEM_PROMPT: &str = "\
You are the read-only research agent of crest, a coding assistant. You investigate the \
workspace for one stage of a research flow. You can only read: list_files, find_file, grep, \
tree, read_file and, when offered, run_command for read-only git commands and read-only MCP \
tools. You cannot edit, write or run other commands, and you cannot ask the user anything.
- Search broadly first (grep, find_file, tree), then read only the parts that matter.
- Cite what you read with its workspace path and line numbers.
- End with exactly the answer format the task asks for.";

/// The ports the read-only agent is built from.
#[derive(Clone)]
pub struct ExplorePorts {
    /// The model.
    pub llm: Arc<dyn LlmPort>,
    /// Telemetry of its model and tool calls.
    pub telemetry: Arc<dyn TelemetryPort>,
    /// Model selection (each node passes its role).
    pub models: ModelPolicy,
    /// File listing and fuzzy finding.
    pub file_search: Arc<dyn FileSearchPort>,
    /// Content search.
    pub content_search: Arc<dyn ContentSearchPort>,
    /// Reading files (its tools never call the write methods).
    pub fs: Arc<dyn FileSystemPort>,
    /// For read-only git commands; `None` leaves `run_command` out.
    pub shell: Option<Arc<dyn ShellPort>>,
    /// Extra read-only tools (MCP queries registered with a read effect).
    pub extra_tools: Vec<Arc<dyn Tool>>,
}

/// The read tools of `ports` in one registry cut to `budget`.
///
/// # Errors
///
/// [`RegisterError`] when an extra tool repeats a name.
pub fn read_only_tools(
    ports: &ExplorePorts,
    budget: ResultBudget,
) -> Result<ToolRegistry, RegisterError> {
    let mut tools = ToolRegistry::new().with_budget(budget);
    let search = &ports.file_search;
    tools.register_typed(ListFilesTool::new(Arc::clone(search), budget))?;
    tools.register_typed(FindFileTool::new(Arc::clone(search), budget))?;
    tools.register_typed(TreeTool::new(Arc::clone(search), budget))?;
    tools.register_typed(GrepTool::new(Arc::clone(&ports.content_search), budget))?;
    tools.register_typed(ReadFileTool::new(Arc::clone(&ports.fs), budget))?;
    if let Some(shell) = &ports.shell {
        tools.register_typed(RunCommandTool::new(Arc::clone(shell), budget))?;
    }
    for tool in &ports.extra_tools {
        tools.register(Arc::clone(tool))?;
    }
    Ok(tools)
}

/// The read-only agent over `ports` (see the module docs).
///
/// # Errors
///
/// As [`read_only_tools`].
pub fn explore_agent(ports: ExplorePorts) -> Result<AgentService, RegisterError> {
    let tools = read_only_tools(&ports, ResultBudget::default())?;
    let deps = AgentDeps {
        llm: ports.llm,
        events: Arc::new(DiscardEvents),
        approval: Arc::new(DenyApprovals),
        telemetry: ports.telemetry,
    };
    let permissions = Arc::new(Mutex::new(PermissionPolicy::read_only()));
    Ok(AgentService::new(deps, tools, ports.models, permissions)
        .with_system_prompt(EXPLORE_SYSTEM_PROMPT)
        .with_max_iterations(EXPLORE_MAX_ITERATIONS))
}

/// The agent of the nodes that act, over the chat's `deps`, `tools`,
/// `models` and `permissions`, with its events filtered by [`NodeEvents`].
#[must_use]
pub fn node_agent(
    mut deps: AgentDeps,
    tools: ToolRegistry,
    models: ModelPolicy,
    permissions: SharedPermissions,
) -> AgentService {
    deps.events = Arc::new(NodeEvents::new(deps.events));
    AgentService::new(deps, tools, models, permissions)
}

/// Forwards to `inner` only what the timeline should show of a node's
/// agent turn: tool call blocks (and diffs or other blocks it adds), their
/// status, approvals, notices and retries. Text and thinking blocks with
/// their deltas, usage and the turn framing are dropped.
pub struct NodeEvents {
    inner: Arc<dyn EventSink>,
}

impl std::fmt::Debug for NodeEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeEvents").finish_non_exhaustive()
    }
}

impl NodeEvents {
    /// Filters the events sent to `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn EventSink>) -> Self {
        Self { inner }
    }

    /// Whether `event` reaches the timeline.
    #[must_use]
    pub fn keeps(event: &DomainEvent) -> bool {
        match event {
            DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => !matches!(
                block.kind,
                BlockKind::Text { .. } | BlockKind::Thinking { .. }
            ),
            DomainEvent::ToolStatusChanged { .. }
            | DomainEvent::ApprovalRequested { .. }
            | DomainEvent::Notice { .. }
            | DomainEvent::Retrying { .. }
            | DomainEvent::ModelFallback { .. } => true,
            _ => false,
        }
    }
}

impl EventSink for NodeEvents {
    fn emit(&self, event: DomainEvent) {
        if Self::keeps(&event) {
            self.inner.emit(event);
        }
    }
}

/// Drops every event.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscardEvents;

impl EventSink for DiscardEvents {
    fn emit(&self, _event: DomainEvent) {}
}

/// Denies every approval: the read-only agent never asks the user.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyApprovals;

#[async_trait]
impl ApprovalPort for DenyApprovals {
    async fn request(&self, _req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError> {
        Err(ApprovalError::Unavailable(
            "the read-only research agent cannot ask for approval".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::Session;
    use domain::model::ModelRole;
    use domain::workspace::WorkspaceRoot;
    use serde_json::json;

    use super::*;
    use crate::testing::fs::{InMemoryFs, ScriptedShell};
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingTelemetry;
    use crate::usecases::run_turn::TurnOptions;

    fn ports(llm: Arc<ScriptedLlm>, fs: Arc<InMemoryFs>) -> ExplorePorts {
        ExplorePorts {
            llm,
            telemetry: Arc::new(RecordingTelemetry::new()),
            models: ModelPolicy::default(),
            file_search: fs.clone(),
            content_search: fs.clone(),
            fs,
            shell: Some(Arc::new(ScriptedShell::new())),
            extra_tools: Vec::new(),
        }
    }

    #[tokio::test]
    async fn the_node_agent_shows_tool_calls_but_not_its_answer() {
        use crate::testing::system::{RecordingSink, ScriptedApproval};
        let fs = Arc::new(
            InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap()).with_file("src/lib.rs", "x\n"),
        );
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("miro").then_tool_use(
                "t1",
                "read_file",
                json!({ "path": "src/lib.rs" }),
            ),
            ScriptedResponse::text("{\"ok\": true}"),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let p = ports(llm.clone(), fs);
        let deps = AgentDeps {
            llm,
            events: sink.clone(),
            approval: Arc::new(ScriptedApproval::new()),
            telemetry: p.telemetry.clone(),
        };
        let tools = read_only_tools(&p, ResultBudget::default()).unwrap();
        let agent = node_agent(
            deps,
            tools,
            ModelPolicy::default(),
            Arc::new(Mutex::new(PermissionPolicy::default())),
        );
        let mut session = Session::new("/ws", 0);
        agent
            .run_turn(&mut session, "lee", TurnOptions::new(ModelRole::Main))
            .await
            .unwrap();
        let kinds = sink.kinds();
        assert!(
            kinds.contains(&"tool_status_changed".to_owned()),
            "{kinds:?}"
        );
        for event in sink.events() {
            assert!(NodeEvents::keeps(&event), "{event:?}");
            if let DomainEvent::BlockAdded { block } = event {
                assert!(
                    matches!(block.kind, BlockKind::ToolCall { .. }),
                    "{block:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn reads_but_cannot_write_or_run() {
        let fs = Arc::new(
            InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap()).with_file("src/lib.rs", "x\n"),
        );
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::tool_use("t1", "read_file", json!({ "path": "src/lib.rs" }))
                .then_tool_use(
                    "t2",
                    "write_file",
                    json!({ "path": "src/lib.rs", "content": "y" }),
                )
                .then_tool_use(
                    "t3",
                    "run_command",
                    json!({ "command": "rm", "args": ["-rf", "."] }),
                ),
            ScriptedResponse::text("listo"),
        ]));
        let agent = explore_agent(ports(Arc::clone(&llm), Arc::clone(&fs))).unwrap();
        let names: Vec<&str> = agent.tools().names().collect();
        assert!(names.contains(&"run_command") && !names.contains(&"write_file"));
        let mut session = Session::new("/ws", 0);
        agent
            .run_turn(&mut session, "lee", TurnOptions::new(ModelRole::Explore))
            .await
            .unwrap();
        assert_eq!(fs.file("src/lib.rs").as_deref(), Some("x\n"));
        let sent = serde_json::to_string(&llm.last_request().unwrap().messages).unwrap();
        assert!(sent.contains("1\\tx"), "{sent}");
        assert!(sent.contains("read-only"), "{sent}");
        assert_eq!(sent.matches("\"is_error\":true").count(), 2, "{sent}");
    }
}
