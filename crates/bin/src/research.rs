//! The research flow in the binary: how the composition root builds it and
//! how the chat (window and headless) drives it.
//!
//! # Commands
//!
//! - `/research <problem description>` starts a flow that runs to its
//!   first gate: the five stages of the fast pipeline (Triage, Discovery,
//!   Problem definition, Solution, Spec, plan and review), at the depth
//!   its triage picks.
//! - `/research --quick <problem>` (`-q`) runs the fast pipeline at quick
//!   depth; `/research --deep <problem>` (`-d`) runs the 14 nodes of the
//!   X-Workflow.
//! - `/research resume` continues the flow left halfway in this workspace
//!   (crest says so when it opens), with the pipeline it started with.
//! - `/research list` shows the stored flows with their pipeline and cost
//!   so far; `/research` alone says where the flow is and what it cost.
//!
//! While a flow is active every line typed in the chat goes to it: the
//! answer to its gate ("yes" to go on, anything else to redo the stage) or
//! `/stage`, `/back <stage>`, `/skip` and `/abort`. The `:` commands of the
//! UI (`/model`, `/mode`…) are handled by the UI first. When the flow ends
//! the chat answers turns again.
//!
//! Headless: `crest --headless --research "<problem>"` runs the flow in the
//! terminal and reads each gate's answer from stdin (`resume` as the problem
//! continues the pending flow; `--quick` or `--deep` before the problem
//! work as in the chat).

use application::cost::PricingTable;
use application::decide::Decider;
use std::sync::Arc;

use adapter_store::SqliteFlowStorage;
use application::flows::research::agents::{ExplorePorts, explore_agent, node_agent};
use application::flows::research::error::FlowError;
use application::flows::research::flow::FlowReply;
use application::flows::research::gate::FlowStatus;
use application::flows::research::graph::{ResearchWorkspace, research_graphs};
use application::flows::research::node::NodeDeps;
use application::flows::research::session::{
    ResearchSession, StartOptions, describe_reply, describe_status,
};
use application::flows::research::usage::StageUsage;
use application::ports::llm::LlmPort;
use application::tools::registry::ToolRegistry;
use application::usecases::permissions::ProjectRules;
use application::usecases::run_turn::{AgentDeps, SharedPermissions};
use domain::model::ModelPolicy;
use domain::permission::ToolEffect;
use domain::research::Pipeline;
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
    /// Where the acting nodes' "always in project" answers are written.
    pub project_rules: ProjectRules,
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
    /// Prices of the models, for the cost of each stage (maybe empty).
    pub pricing: PricingTable,
}

/// The MCP tools of `tools` that only read (servers declared with a read
/// effect), for the read-only agent of the Datos node.
fn read_only_mcp(tools: &ToolRegistry) -> Vec<Arc<dyn application::tools::registry::Tool>> {
    tools
        .names()
        .into_iter()
        .filter(|name| name.starts_with("mcp__"))
        .filter_map(|name| tools.get(&name))
        .filter(|tool| tool.effect(&serde_json::json!({})) == ToolEffect::Read)
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
        project_rules,
        models,
        mut explore,
        workspace,
        system_prompt,
        root,
        storage,
        decider,
        pricing,
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
    .with_system_prompt(system_prompt)
    .with_project_rules(project_rules);
    let root = root.as_path().display().to_string();
    let deps = NodeDeps::new(llm, models, events)
        .with_agent(Arc::new(acting))
        .with_explore_agent(Arc::new(explorer))
        .with_workspace_root(root.clone())
        .with_decider(decider)
        .with_pricing(pricing);
    let graphs = research_graphs(deps, Some(workspace))?;
    Ok(ResearchSession::new(graphs, storage, root))
}

/// How `/research <problem>` runs: the flag before the problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    /// No flag: the fast pipeline, at the depth its triage picks.
    Auto,
    /// `--quick` (`-q`): the fast pipeline at quick depth.
    Quick,
    /// `--deep` (`-d`): the 14-stage pipeline.
    Deep,
}

impl StartMode {
    /// The mode named by a leading flag, if `word` is one.
    fn from_flag(word: &str) -> Option<Self> {
        match word {
            "--quick" | "-q" | "--rapido" | "--rápido" => Some(Self::Quick),
            "--deep" | "-d" | "--profundo" => Some(Self::Deep),
            _ => None,
        }
    }

    /// How [`ResearchSession::start`] starts the flow.
    #[must_use]
    pub fn options(self) -> StartOptions {
        match self {
            Self::Auto => StartOptions::new(Pipeline::Fast),
            Self::Quick => StartOptions {
                pipeline: Pipeline::Fast,
                depth: Some("quick".to_owned()),
            },
            Self::Deep => StartOptions::deep(),
        }
    }
}

/// What `/research <args>` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchCommand {
    /// `/research [--quick|--deep] <problem>`.
    Start {
        /// The problem to research.
        problem: String,
        /// The pipeline and depth.
        mode: StartMode,
    },
    /// `/research resume [n|id]` (or `retry`): continues the last flow, or
    /// the one numbered `n` in `/research list`.
    Resume(Option<String>),
    /// `/research list [all]`: the open flows, or every one with `all`.
    List(bool),
    /// `/research cancel [n|id|all]`: aborts the last flow, the one
    /// numbered `n` in `/research list`, or every open one.
    Cancel(Option<String>),
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
            "list" | "ls" | "listar" if rest.is_empty() => Self::List(false),
            "list" | "ls" | "listar" if matches!(rest, "all" | "todos") => Self::List(true),
            "cancel" | "cancelar" | "descartar" | "drop" => Self::Cancel(which),
            "stage" | "skip" | "abort" | "back" => Self::Line(format!("/{args}")),
            flag => match StartMode::from_flag(flag) {
                Some(mode) => Self::Start {
                    problem: rest.to_owned(),
                    mode,
                },
                None => Self::Start {
                    problem: args.to_owned(),
                    mode: StartMode::Auto,
                },
            },
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
        ResearchCommand::Start { problem, .. } if problem.is_empty() => {
            return Err(
                "`/research --quick <problem>` and `/research --deep <problem>` need a problem \
                 description"
                    .to_owned(),
            );
        }
        ResearchCommand::Start { problem, mode } => {
            research.start(&problem, now, mode.options()).await
        }
        ResearchCommand::Resume(which) => research.resume(which.as_deref()).await,
        ResearchCommand::Line(line) => return self::line(research, &line).await,
        ResearchCommand::List(all) => return list(research, all).await,
        ResearchCommand::Cancel(which) => {
            return match research.cancel(which.as_deref()).await {
                Ok(0) => Ok("nothing to cancel: that research flow already stopped".to_owned()),
                Ok(1) => Ok("research flow cancelled; its journal is kept".to_owned()),
                Ok(n) => Ok(format!(
                    "{n} research flows cancelled; their journals are kept"
                )),
                Err(err) => Err(message(&err)),
            };
        }
        ResearchCommand::Status => {
            return Ok(match research.active() {
                Some(flow) => {
                    let position = flow.position().await.map_err(|e| e.to_string())?;
                    let used = flow.usage().await.unwrap_or_default();
                    format!("{position}{}", so_far(&used))
                }
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

/// " · $0.41 so far" when the cost of `used` is known, else nothing.
fn so_far(used: &StageUsage) -> String {
    used.cost_usd
        .map(|cost| format!(" · ${cost:.2} so far"))
        .unwrap_or_default()
}

/// What to type to continue a flow left at `status`.
fn resume_hint(status: &FlowStatus) -> &'static str {
    match status {
        FlowStatus::Failed { .. } => "`/research retry` runs it again",
        _ => "`/research resume` to continue",
    }
}

/// `/research list [all]`: the stored flows of the workspace that are
/// still open (every one with `all`), newest first, with the numbers
/// `/research resume` and `/research cancel` take.
async fn list(research: &ResearchSession, all: bool) -> Result<String, String> {
    let flows = research.list().await.map_err(|e| message(&e))?;
    let total = flows.len();
    let flows: Vec<_> = flows
        .into_iter()
        .filter(|flow| all || flow.position.status.is_open())
        .collect();
    if flows.is_empty() {
        return Ok(if total == 0 {
            USAGE.to_owned()
        } else {
            format!("no open research flow; `/research list all` shows the {total} stored")
        });
    }
    let hidden = total - flows.len();
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
            "{}. **{problem}** — {} · {}{}\n",
            flow.number,
            flow.pipeline.name(),
            flow.position,
            so_far(&flow.usage)
        ));
    }
    out.push_str("\n`/research resume <n>` continues one, `/research cancel <n>` drops it.");
    if hidden > 0 {
        out.push_str(&format!(
            " {hidden} finished or cancelled: `/research list all`."
        ));
    }
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
        assert_eq!(ResearchCommand::parse("list"), ResearchCommand::List(false));
        assert_eq!(
            ResearchCommand::parse("list all"),
            ResearchCommand::List(true)
        );
        assert_eq!(
            ResearchCommand::parse("cancel 2"),
            ResearchCommand::Cancel(Some("2".into()))
        );
        assert_eq!(
            ResearchCommand::parse("cancelar"),
            ResearchCommand::Cancel(None)
        );
        assert_eq!(
            ResearchCommand::parse("cancel all"),
            ResearchCommand::Cancel(Some("all".into()))
        );
        assert_eq!(
            ResearchCommand::parse("back 3"),
            ResearchCommand::Line("/back 3".into())
        );
        let start = |problem: &str, mode| ResearchCommand::Start {
            problem: problem.into(),
            mode,
        };
        assert_eq!(
            ResearchCommand::parse("add suma mal"),
            start("add suma mal", StartMode::Auto)
        );
        assert_eq!(
            ResearchCommand::parse("--quick the button does nothing"),
            start("the button does nothing", StartMode::Quick)
        );
        assert_eq!(ResearchCommand::parse("-q x"), start("x", StartMode::Quick));
        assert_eq!(
            ResearchCommand::parse("--deep  login is slow "),
            start("login is slow", StartMode::Deep)
        );
        assert_eq!(ResearchCommand::parse("-d"), start("", StartMode::Deep));
        // A flag later in the text is part of the problem.
        assert_eq!(
            ResearchCommand::parse("why does -q fail"),
            start("why does -q fail", StartMode::Auto)
        );
    }

    #[test]
    fn modes_pick_the_pipeline_and_depth() {
        assert_eq!(StartMode::Auto.options(), StartOptions::new(Pipeline::Fast));
        let quick = StartMode::Quick.options();
        assert_eq!(
            (quick.pipeline, quick.depth.as_deref()),
            (Pipeline::Fast, Some("quick"))
        );
        assert_eq!(StartMode::Deep.options(), StartOptions::deep());
    }
}
