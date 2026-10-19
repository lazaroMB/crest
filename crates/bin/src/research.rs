//! The research flow in the binary: how the composition root builds it and
//! how the chat (window and headless) drives it.
//!
//! # Commands
//!
//! - `/research <problem description>` starts a flow (the 14 nodes of
//!   the X-Workflow) that runs to its first gate.
//! - `/research resume` continues the flow left halfway in this workspace
//!   (crest says so when it opens).
//! - `/research` alone says where the flow is.
//!
//! While a flow is active every line typed in the chat goes to it: the
//! answer to its gate ("yes" to go on, anything else to redo the stage) or
//! `/stage`, `/back <stage>`, `/skip` and `/abort`. The `:` commands of the
//! UI (`/model`, `/mode`…) are handled by the UI first. When the flow ends
//! the chat answers turns again.
//!
//! Headless: `crest --headless --research "<problem>"` runs the flow in the
//! terminal and reads each gate's answer from stdin (`resume` as the problem
//! continues the pending flow).

use application::decide::Decider;
use std::sync::Arc;

use adapter_store::SqliteFlowStorage;
use application::flows::research::agents::{ExplorePorts, explore_agent, node_agent};
use application::flows::research::error::FlowError;
use application::flows::research::flow::FlowReply;
use application::flows::research::gate::FlowStatus;
use application::flows::research::graph::{ResearchWorkspace, research_graph};
use application::flows::research::node::NodeDeps;
use application::flows::research::session::{ResearchSession, describe_reply, describe_status};
use application::ports::llm::LlmPort;
use application::tools::registry::ToolRegistry;
use application::usecases::run_turn::{AgentDeps, SharedPermissions};
use domain::model::ModelPolicy;
use domain::permission::ToolEffect;
use domain::workspace::WorkspaceRoot;

/// What the research flow is built from, all shared with the chat.
pub struct ResearchParts {
    /// Ports of the chat's agent: its events, approvals and telemetry are
    /// those of the chat.
    pub agent: AgentDeps,
    /// The chat's tools (the acting nodes use them, with approvals).
    pub tools: ToolRegistry,
    /// The chat's permissions.
    pub permissions: SharedPermissions,
    /// Model selection.
    pub models: ModelPolicy,
    /// Ports of the read-only agent (with the web tools, when configured);
    /// its MCP tools are taken from `tools`.
    pub explore: ExplorePorts,
    /// Where the Specs are written.
    pub workspace: ResearchWorkspace,
    /// System prompt of the acting nodes' agent (the chat's).
    pub system_prompt: String,
    /// The workspace root.
    pub root: WorkspaceRoot,
    /// The flow checkpoints (the chat's database).
    pub storage: Arc<SqliteFlowStorage>,
    /// The decision model of the judges and gates (maybe none).
    pub decider: Decider,
}

/// The MCP tools of `tools` that only read (servers declared with a read
/// effect), for the read-only agent of the Datos node.
fn read_only_mcp(tools: &ToolRegistry) -> Vec<Arc<dyn application::tools::registry::Tool>> {
    tools
        .names()
        .filter(|name| name.starts_with("mcp__"))
        .filter_map(|name| tools.get(name))
        .filter(|tool| tool.effect(&serde_json::json!({})) == ToolEffect::Read)
        .cloned()
        .collect()
}

/// The research flow of the workspace over `parts`.
///
/// # Errors
///
/// When the read-only agent or the graph cannot be built.
pub fn build(parts: ResearchParts) -> anyhow::Result<ResearchSession> {
    let ResearchParts {
        agent,
        tools,
        permissions,
        models,
        mut explore,
        workspace,
        system_prompt,
        root,
        storage,
        decider,
    } = parts;
    let llm: Arc<dyn LlmPort> = Arc::clone(&agent.llm);
    explore.llm = Arc::clone(&llm);
    explore.extra_tools.extend(read_only_mcp(&tools));
    let explorer = explore_agent(explore)?;
    let events = Arc::clone(&agent.events);
    let acting = node_agent(
        AgentDeps {
            llm: Arc::clone(&llm),
            ..agent
        },
        tools,
        models.clone(),
        permissions,
    )
    .with_system_prompt(system_prompt);
    let root = root.as_path().display().to_string();
    let deps = NodeDeps::new(llm, models, events)
        .with_agent(Arc::new(acting))
        .with_explore_agent(Arc::new(explorer))
        .with_workspace_root(root.clone())
        .with_decider(decider);
    let graph = research_graph(deps, Some(workspace))?;
    Ok(ResearchSession::new(graph, storage, root))
}

/// What `/research <args>` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchCommand {
    /// `/research <problem>`.
    Start(String),
    /// `/research resume [n|id]` (or `retry`): continues the last flow, or
    /// the one numbered `n` in `/research list`.
    Resume(Option<String>),
    /// `/research list`.
    List,
    /// `/research stage|back <stage>|skip|abort`: a command for the last flow.
    Line(String),
    /// `/research`.
    Status,
}

impl ResearchCommand {
    /// Parses the arguments of `/research`.
    #[must_use]
    pub fn parse(args: &str) -> Self {
        let args = args.trim();
        let (name, rest) = args
            .split_once(char::is_whitespace)
            .map_or((args, ""), |(n, r)| (n, r.trim()));
        let which = (!rest.is_empty()).then(|| rest.to_owned());
        match name {
            "" => Self::Status,
            "resume" | "retomar" | "reanudar" | "continue" | "continuar" => Self::Resume(which),
            "retry" | "reintentar" if rest.is_empty() => Self::Resume(None),
            "list" | "ls" | "listar" if rest.is_empty() => Self::List,
            "stage" | "skip" | "abort" | "back" => Self::Line(format!("/{args}")),
            _ => Self::Start(args.to_owned()),
        }
    }
}

/// What the status bar says when `Ctrl+C` stopped the research flow.
pub const INTERRUPTED: &str = "research flow interrupted; it is saved at its last stage and \
                               `/research resume` continues it";

/// Usage of `/research`, shown when there is nothing to report.
pub const USAGE: &str = "no active research flow; `/research <problem description>` \
                         starts one and `/research list` shows the stored ones";

/// Runs `command` (`now` in Unix epoch milliseconds for the new flow's
/// id) and answers a line for the status bar (a markdown list for
/// [`ResearchCommand::List`]).
///
/// # Errors
///
/// The error of the flow, as text, also when a node failed.
pub async fn command(
    research: &mut ResearchSession,
    command: ResearchCommand,
    now: u64,
) -> Result<String, String> {
    let status = match command {
        ResearchCommand::Start(problem) => research.start(&problem, now).await,
        ResearchCommand::Resume(which) => research.resume(which.as_deref()).await,
        ResearchCommand::Line(line) => return self::line(research, &line).await,
        ResearchCommand::List => return list(research).await,
        ResearchCommand::Status => {
            return Ok(match research.active() {
                Some(flow) => flow
                    .position()
                    .await
                    .map(|p| p.to_string())
                    .map_err(|e| e.to_string())?,
                None => match research.pending().await {
                    Ok(Some(position)) => format!("{position}; {}", resume_hint(&position.status)),
                    Ok(None) => USAGE.to_owned(),
                    Err(err) => return Err(err.to_string()),
                },
            });
        }
    };
    match status {
        Ok(status @ FlowStatus::Failed { .. }) => Err(describe_status(&status)),
        Ok(status) => Ok(describe_status(&status)),
        Err(err) => Err(message(&err)),
    }
}

/// What to type to continue a flow left at `status`.
fn resume_hint(status: &FlowStatus) -> &'static str {
    match status {
        FlowStatus::Failed { .. } => "`/research retry` runs it again",
        _ => "`/research resume` to continue",
    }
}

/// `/research list`: the stored flows of the workspace, newest first.
async fn list(research: &ResearchSession) -> Result<String, String> {
    let flows = research.list().await.map_err(|e| message(&e))?;
    if flows.is_empty() {
        return Ok(USAGE.to_owned());
    }
    let mut out = String::new();
    for flow in flows {
        let problem: String = flow
            .problem
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
        out.push_str(&format!(
            "{}. **{problem}** — {}\n",
            flow.number, flow.position
        ));
    }
    out.push_str("\n`/research resume <n>` continues one.");
    Ok(out)
}

/// A line of the chat for the research flow: the answer to the active
/// gate, or a flow command (`/back`, `/retry`, …) for the last flow.
///
/// # Errors
///
/// The error of the flow, as text, also when a node failed.
pub async fn line(research: &mut ResearchSession, text: &str) -> Result<String, String> {
    match research.handle_line(text).await {
        Ok(FlowReply::Status(status @ FlowStatus::Failed { .. })) => Err(describe_status(&status)),
        Ok(reply) => Ok(describe_reply(&reply)),
        Err(err) => Err(message(&err)),
    }
}

/// The notice shown at start-up when a flow was left halfway.
pub async fn pending_notice(research: &ResearchSession) -> Option<String> {
    let position = research.pending().await.ok()??;
    Some(format!(
        "a research flow is pending ({position}); {}",
        resume_hint(&position.status)
    ))
}

fn message(err: &FlowError) -> String {
    match err {
        FlowError::SessionNotFound(_) => {
            "no such research flow in this workspace; `/research list` shows them".to_owned()
        }
        FlowError::Completed => "research flow already finished; `/back <stage>` reopens it, \
                                 `/research <problem>` starts another"
            .to_owned(),
        other => format!("research flow: {other}"),
    }
}

/// Whether `reply` left the flow waiting at a gate.
#[must_use]
pub fn waits(reply: &FlowReply) -> bool {
    matches!(
        reply,
        FlowReply::Status(FlowStatus::WaitingForInput { .. }) | FlowReply::Position(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_arguments() {
        assert_eq!(ResearchCommand::parse(""), ResearchCommand::Status);
        assert_eq!(
            ResearchCommand::parse(" resume "),
            ResearchCommand::Resume(None)
        );
        assert_eq!(
            ResearchCommand::parse("resume 2"),
            ResearchCommand::Resume(Some("2".into()))
        );
        assert_eq!(
            ResearchCommand::parse("retry"),
            ResearchCommand::Resume(None)
        );
        assert_eq!(ResearchCommand::parse("list"), ResearchCommand::List);
        assert_eq!(
            ResearchCommand::parse("back 3"),
            ResearchCommand::Line("/back 3".into())
        );
        assert_eq!(
            ResearchCommand::parse("add suma mal"),
            ResearchCommand::Start("add suma mal".into())
        );
    }
}
