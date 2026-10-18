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

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
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
use crate::usecases::run_turn::{AgentDeps, AgentService};

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
