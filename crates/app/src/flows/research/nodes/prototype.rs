//! Node 11, Prototype (optional, `/skip` jumps over it): a spike in a
//! temporary `git worktree` that proves the hard piece of the chosen
//! option can be built.
//!
//! # Where the spike lives
//!
//! In `.crest/spikes/<id>` inside the workspace, on the throwaway branch
//! `crest/spike-<id>`, where `<id>` is the decision id and the time (e.g.
//! `d1-1760000000000`). Not in `../.crest-spike-<id>`: the agent's file
//! tools (`read_file`, `edit_file`, `write_file`…) only take paths inside
//! the workspace root, so a worktree inside it is the only one they can
//! edit. While it exists, `git status` of the main tree shows `.crest/`
//! as untracked unless it is ignored.
//!
//! # Limitation
//!
//! The [`AgentService`](crate::usecases::run_turn::AgentService) of the
//! context is bound to the workspace root and `run_command` has no working
//! directory, so the agent is told to reach the worktree through paths
//! and flags (`git -C .crest/spikes/<id>`, `cargo test --manifest-path
//! .crest/spikes/<id>/Cargo.toml`); tools that need to run *inside* the
//! directory cannot. Nothing stops the agent from editing the main tree
//! beyond the prompt and the usual approval of every write.
//!
//! # Steps
//!
//! 1. Without a [`Decision`](domain::research::Decision) →
//!    `Back { to: Modeling }`.
//! 2. First run: asks the user (`NeedsInput`) to create the worktree,
//!    showing the exact commands. That answer is the approval of the git
//!    commands, which the node runs through the agent's `run_command` tool
//!    after checking that the permission policy does not deny them. Any
//!    answer other than a yes finishes without a spike.
//! 3. `git worktree add -b crest/spike-<id> .crest/spikes/<id> HEAD`, then
//!    an agent turn (role `main`) that writes the spike and runs one
//!    command that proves it; the agent emits its `ToolCall` and `Diff`
//!    blocks. It ends with a JSON verdict, asked again once if invalid.
//! 4. The verdict becomes an [`Evidence`](domain::research::Evidence) with
//!    a `Command` source and is stored under [`RESULT_KEY`].
//! 5. `git worktree remove --force` and `git branch -D` clean up whatever
//!    happened; a failed cleanup is a warning, not an error.
//! 6. A spike that shows the piece cannot be built goes
//!    `Back { to: Modeling }`, which now sees that evidence.
//!
//! The node emits a `ToolCall` block for each git command it runs and a
//! `Markdown` block with the verdict.

use std::sync::Arc;

use async_trait::async_trait;
use domain::block::{BlockKind, SourceRef, ToolStatus};
use domain::conversation::{SessionId, ToolUseId, TurnId};
use domain::permission::{ActionRequest, Decision as Verdict};
use domain::research::{Decision, EvidenceId, Stage};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::flows::research::gate::approves;
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::ports::events::NoticeLevel;
use crate::ports::shell::RunCommandRequest;
use crate::tools::registry::ToolContext;
use crate::usecases::approval::lock_permissions;
use crate::usecases::run_turn::AgentService;

/// Directory of the spikes, relative to the workspace root.
pub const SPIKES_DIR: &str = ".crest/spikes";

/// Flow-data key with the [`Spike`] the user is asked about.
pub const SPIKE_KEY: &str = "prototype.spike";

/// Flow-data key with the [`SpikeResult`] of the last spike.
pub const RESULT_KEY: &str = "prototype.result";

const RUN_COMMAND: &str = "run_command";

const SPIKE_PROMPT: &str = "You are the Prototype stage of a software investigation. \
Write a minimal spike that proves the hard piece of the chosen option can be built. \
Work ONLY inside the temporary worktree {dir}: every path you read or write starts \
with {dir}/. run_command runs at the workspace root and takes no working directory, \
so reach the worktree through paths and flags (git -C {dir}, \
cargo test --manifest-path {dir}/Cargo.toml…). Do not touch files outside the worktree. \
Run one command that shows whether the piece works and finish by answering ONLY with \
JSON:\n{\"feasible\": true, \"summary\": \"one sentence on what you tried\", \
\"command\": \"exact line of the verification command\", \"output_excerpt\": \
\"relevant part of its output\"}";

/// A spike: its id, worktree and branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spike {
    /// `<decision id>-<time>`, e.g. `d1-1760000000000`.
    pub id: String,
    /// Worktree path relative to the workspace root.
    pub path: String,
    /// Throwaway branch of the worktree.
    pub branch: String,
}

impl Spike {
    fn for_decision(decision: &Decision, now: u64) -> Self {
        let id = format!("{}-{now}", decision.id.to_string().to_lowercase());
        Self {
            path: format!("{SPIKES_DIR}/{id}"),
            branch: format!("crest/spike-{id}"),
            id,
        }
    }

    fn add(&self) -> RunCommandRequest {
        RunCommandRequest::new(
            "git",
            ["worktree", "add", "-b", &self.branch, &self.path, "HEAD"],
        )
    }

    fn cleanup(&self) -> [RunCommandRequest; 2] {
        [
            RunCommandRequest::new("git", ["worktree", "remove", "--force", &self.path]),
            RunCommandRequest::new("git", ["branch", "-D", &self.branch]),
        ]
    }
}

/// What a spike showed, as stored under [`RESULT_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpikeResult {
    /// The spike.
    pub spike: Spike,
    /// Whether the hard piece could be built.
    pub feasible: bool,
    /// What was tried, in one sentence.
    pub summary: String,
    /// Evidence recorded from the verification command.
    pub evidence: EvidenceId,
}

#[derive(Debug, Deserialize)]
struct Report {
    feasible: bool,
    summary: String,
    command: String,
    #[serde(default)]
    output_excerpt: String,
}

/// The node of [`Stage::Prototype`].
#[derive(Debug, Default)]
pub struct Prototype;

impl Prototype {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ResearchNode for Prototype {
    fn stage(&self) -> Stage {
        Stage::Prototype
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let Some(decision) = ctx.state().decisions.last().cloned() else {
            return Ok(NodeOutcome::Back {
                to: Stage::Modeling,
                reason: "there is no chosen option to prototype".into(),
            });
        };
        let agent = Arc::clone(ctx.agent()?);
        if agent.tools().get(RUN_COMMAND).is_none() {
            return Err(NodeError::Unavailable("the run_command tool"));
        }
        let (spike, reply) = match (ctx.user_reply(), ctx.data::<Spike>(SPIKE_KEY)) {
            (Some(reply), Some(spike)) => (spike, reply.to_owned()),
            _ => {
                let spike = Spike::for_decision(&decision, ctx.now());
                let prompt = ask_prompt(&decision, &spike);
                ctx.set_data(SPIKE_KEY, &spike)?;
                return Ok(NodeOutcome::NeedsInput { prompt });
            }
        };
        if !approves(&reply, ctx.decider()).await {
            ctx.log(format!(
                "No prototype: the user did not authorize the spike ({reply})."
            ));
            return Ok(NodeOutcome::Done);
        }
        for request in std::iter::once(spike.add()).chain(spike.cleanup()) {
            check_policy(&agent, &request)?;
        }
        let (ok, output) = git(ctx, &agent, &spike, "add", spike.add()).await;
        if !ok {
            ctx.log(format!("Could not create the worktree {}.", spike.path));
            return Err(NodeError::Failed(format!(
                "git worktree add failed: {output}"
            )));
        }
        let report = run_spike(ctx, &agent, &decision, &spike).await;
        cleanup(ctx, &agent, &spike).await;
        let report = report?;
        record(ctx, &decision, spike, &report)
    }
}

fn ask_prompt(decision: &Decision, spike: &Spike) -> String {
    let [remove, branch] = spike.cleanup();
    format!(
        "To test \"{}\" I will create a temporary worktree and run a spike in it:\n\
         - `{}`\n- the agent writes and tests the hard piece inside `{}`\n\
         - when done: `{}` and `{}`\n\
         Create it? Answer \"yes\" to go on, anything else to continue without a \
         prototype, or /skip.",
        decision.chosen,
        spike.add().display(),
        spike.path,
        remove.display(),
        branch.display(),
    )
}

/// Refuses commands the permission policy denies; the user's answer to the
/// question stands for the approval of the rest.
fn check_policy(agent: &AgentService, request: &RunCommandRequest) -> Result<(), NodeError> {
    let effect = agent
        .tools()
        .get(RUN_COMMAND)
        .map(|tool| tool.effect(&json!(request)))
        .ok_or(NodeError::Unavailable("the run_command tool"))?;
    let action = ActionRequest::new(RUN_COMMAND, effect, request.display());
    match lock_permissions(agent.permissions()).decide(&action) {
        Verdict::Deny { reason } => Err(NodeError::Failed(format!(
            "the permission policy denies `{}`: {reason}",
            request.display()
        ))),
        Verdict::Allow | Verdict::Ask => Ok(()),
    }
}

/// Runs a git command through `run_command` and shows it as a `ToolCall`.
/// Returns whether it exited with 0, and its rendered output.
async fn git(
    ctx: &NodeContext,
    agent: &AgentService,
    spike: &Spike,
    step: &str,
    request: RunCommandRequest,
) -> (bool, String) {
    let tool_use_id = format!("prototype-{}-{step}", spike.id);
    let tool_ctx = ToolContext::new(
        SessionId::new(),
        TurnId::new(),
        ToolUseId::new(tool_use_id.clone()),
    );
    let input = json!(request);
    let (ok, text) = match agent
        .tools()
        .dispatch(RUN_COMMAND, input.clone(), &tool_ctx)
        .await
    {
        Ok(output) => {
            let text = output.text_content();
            (!output.is_error && text.starts_with("exit code: 0"), text)
        }
        Err(err) => (false, err.to_string()),
    };
    let status = if ok {
        ToolStatus::Succeeded
    } else {
        ToolStatus::Failed
    };
    // Best effort: the block only fails on bad payloads.
    let _ = ctx.emit(BlockKind::ToolCall {
        tool_use_id,
        name: RUN_COMMAND.into(),
        summary: request.display(),
        input,
        status,
        output: Some(text.clone()),
    });
    (ok, text)
}

async fn cleanup(ctx: &mut NodeContext, agent: &AgentService, spike: &Spike) {
    let [remove, branch] = spike.cleanup();
    let mut failed = Vec::new();
    for (step, request) in [("remove", remove), ("branch", branch)] {
        let line = request.display();
        if !git(ctx, agent, spike, step, request).await.0 {
            failed.push(line);
        }
    }
    if failed.is_empty() {
        ctx.log(format!("Worktree {} removed.", spike.path));
    } else {
        let message = format!("Could not clean up the spike: {}", failed.join("; "));
        ctx.notice(NoticeLevel::Warning, message.clone());
        ctx.log(message);
    }
}

/// The agent turn of the spike, with one retry when its verdict is not
/// valid JSON.
async fn run_spike(
    ctx: &NodeContext,
    agent: &AgentService,
    decision: &Decision,
    spike: &Spike,
) -> Result<Report, NodeError> {
    let mut session = ctx.new_session();
    let system = SPIKE_PROMPT.replace("{dir}", &spike.path);
    let input = format!(
        "{system}\n\nRequest: {}\nChosen option: {}\nWhy: {}",
        ctx.problem(),
        decision.chosen,
        decision.rationale
    );
    ctx.agent_json(agent, &mut session, input, |report: &Report| {
        if report.command.trim().is_empty() || report.summary.trim().is_empty() {
            return Err("the command or the summary is missing".into());
        }
        Ok(())
    })
    .await
}

fn record(
    ctx: &mut NodeContext,
    decision: &Decision,
    spike: Spike,
    report: &Report,
) -> Result<NodeOutcome, NodeError> {
    let verdict = if report.feasible {
        "can be built"
    } else {
        "could not be built"
    };
    let claim = format!(
        "Spike of \"{}\": {verdict}. {}",
        decision.chosen, report.summary
    );
    let now = ctx.now();
    let evidence = ctx.state_mut().add_evidence(
        claim,
        SourceRef::Command {
            command: report.command.clone(),
            output_excerpt: report.output_excerpt.clone(),
        },
        now,
    );
    ctx.emit(BlockKind::Markdown {
        title: Some("Spike result".into()),
        source: format!(
            "**{}** {verdict} [{evidence}].\n\n{}\n\n```text\n$ {}\n{}\n```",
            decision.chosen, report.summary, report.command, report.output_excerpt
        ),
    })?;
    ctx.log(format!(
        "Spike {} in {}: {verdict} ({evidence}).",
        spike.id, spike.path
    ));
    ctx.set_data(
        RESULT_KEY,
        SpikeResult {
            spike,
            feasible: report.feasible,
            summary: report.summary.clone(),
            evidence,
        },
    )?;
    if report.feasible {
        Ok(NodeOutcome::Done)
    } else {
        Ok(NodeOutcome::Back {
            to: Stage::Modeling,
            reason: format!(
                "the spike showed that \"{}\" cannot be built [{evidence}]",
                decision.chosen
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use domain::model::ModelPolicy;
    use domain::permission::{PermissionMode, PermissionPolicy};
    use domain::research::ResearchState;
    use domain::workspace::WorkspaceRoot;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::approval::ApprovalResponse;
    use crate::ports::events::DomainEvent;
    use crate::ports::shell::CommandOutput;
    use crate::testing::fs::{InMemoryFs, ScriptedShell};
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
    use crate::tools::files::{FileToolPorts, register_file_tools};
    use crate::tools::registry::ToolRegistry;
    use crate::usecases::run_turn::AgentDeps;

    const DIR: &str = ".crest/spikes/d1-9";
    const ADD: [&str; 6] = ["worktree", "add", "-b", "crest/spike-d1-9", DIR, "HEAD"];
    const REMOVE: [&str; 4] = ["worktree", "remove", "--force", DIR];
    const BRANCH: [&str; 3] = ["branch", "-D", "crest/spike-d1-9"];
    const TEST: [&str; 3] = ["test", "--manifest-path", ".crest/spikes/d1-9/Cargo.toml"];

    fn ok(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: Some(0),
            stdout: stdout.into(),
            ..CommandOutput::default()
        }
    }

    fn shell() -> ScriptedShell {
        ScriptedShell::new()
            .on("git", ADD, ok("Preparing worktree"))
            .on("git", REMOVE, ok(""))
            .on("git", BRANCH, ok("Deleted branch"))
            .on("cargo", TEST, ok("test cache ... ok"))
    }

    struct Fixture {
        llm: Arc<ScriptedLlm>,
        shell: Arc<ScriptedShell>,
        sink: Arc<RecordingSink>,
        ctx: NodeContext,
    }

    fn fixture(
        script: Vec<ScriptedResponse>,
        shell: ScriptedShell,
        state: ResearchState,
    ) -> Fixture {
        let llm = Arc::new(ScriptedLlm::new(script));
        let shell = Arc::new(shell);
        let sink = Arc::new(RecordingSink::new());
        let fs = Arc::new(InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap()));
        let mut tools = ToolRegistry::new();
        register_file_tools(
            &mut tools,
            FileToolPorts {
                file_search: fs.clone(),
                content_search: fs.clone(),
                fs,
                shell: shell.clone(),
                rewriter: None,
            },
        )
        .unwrap();
        let deps = AgentDeps {
            llm: llm.clone(),
            events: sink.clone(),
            approval: Arc::new(ScriptedApproval::always(ApprovalResponse::Approve)),
            telemetry: Arc::new(RecordingTelemetry::new()),
        };
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
        let agent = AgentService::new(deps, tools, ModelPolicy::default(), permissions);
        let deps = NodeDeps::new(llm.clone(), ModelPolicy::default(), sink.clone())
            .with_agent(Arc::new(agent))
            .with_clock(|| 9);
        let ctx = NodeContext::new(Stage::Prototype, deps, "login is slow", state);
        Fixture {
            llm,
            shell,
            sink,
            ctx,
        }
    }

    fn decided() -> ResearchState {
        let mut state = ResearchState::default();
        state.add_decision("In-memory cache", vec![], "avoids the disk", vec![]);
        state
    }

    fn spike_turn(feasible: bool) -> Vec<ScriptedResponse> {
        vec![
            ScriptedResponse::tool_use(
                "toolu_write",
                "write_file",
                json!({"path": format!("{DIR}/src/cache.rs"), "content": "pub struct Cache;\n"}),
            ),
            ScriptedResponse::tool_use(
                "toolu_test",
                "run_command",
                json!({"command": "cargo", "args": TEST}),
            ),
            ScriptedResponse::text(
                json!({
                    "feasible": feasible,
                    "summary": "an LRU cache behind SessionStore",
                    "command": "cargo test --manifest-path .crest/spikes/d1-9/Cargo.toml",
                    "output_excerpt": "test cache ... ok"
                })
                .to_string(),
            ),
        ]
    }

    fn commands(shell: &ScriptedShell) -> Vec<String> {
        shell
            .calls()
            .iter()
            .map(RunCommandRequest::display)
            .collect()
    }

    fn blocks(sink: &RecordingSink) -> Vec<BlockKind> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => Some(block.kind),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn asks_then_runs_the_spike_in_a_worktree_and_cleans_up() {
        let Fixture {
            llm,
            shell,
            sink,
            mut ctx,
        } = fixture(spike_turn(true), shell(), decided());
        let node = Prototype::new();

        let NodeOutcome::NeedsInput { prompt } = node.run(&mut ctx).await.unwrap() else {
            panic!("expected a question");
        };
        assert!(prompt.contains("`git worktree add -b crest/spike-d1-9 .crest/spikes/d1-9 HEAD`"));
        assert!(prompt.contains("git branch -D crest/spike-d1-9"));
        assert!(shell.calls().is_empty());
        assert_eq!(llm.requests().len(), 0);

        let mut ctx = ctx.with_reply("yes");
        assert_eq!(node.run(&mut ctx).await.unwrap(), NodeOutcome::Done);

        assert_eq!(
            commands(&shell),
            [
                "git worktree add -b crest/spike-d1-9 .crest/spikes/d1-9 HEAD",
                "cargo test --manifest-path .crest/spikes/d1-9/Cargo.toml",
                "git worktree remove --force .crest/spikes/d1-9",
                "git branch -D crest/spike-d1-9",
            ]
        );
        let first = &llm.requests()[0];
        assert_eq!(first.model.role, Stage::Prototype.default_role());
        assert!(
            format!("{:?}", first.messages)
                .contains("ONLY inside the temporary worktree .crest/spikes/d1-9")
        );

        let blocks = blocks(&sink);
        assert!(blocks.iter().any(|b| matches!(b, BlockKind::Diff(_))));
        let git_calls: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                BlockKind::ToolCall {
                    tool_use_id,
                    status,
                    ..
                } if tool_use_id.starts_with("prototype-") => Some((tool_use_id.as_str(), *status)),
                _ => None,
            })
            .collect();
        assert_eq!(
            git_calls,
            [
                ("prototype-d1-9-add", ToolStatus::Succeeded),
                ("prototype-d1-9-remove", ToolStatus::Succeeded),
                ("prototype-d1-9-branch", ToolStatus::Succeeded),
            ]
        );
        assert!(matches!(blocks.last(), Some(BlockKind::Markdown { .. })));

        let result: SpikeResult = ctx.data(RESULT_KEY).unwrap();
        assert!(result.feasible);
        let state = ctx.into_state();
        let evidence = &state.evidence[0];
        assert_eq!(evidence.id, result.evidence);
        assert_eq!(
            evidence.source,
            SourceRef::Command {
                command: "cargo test --manifest-path .crest/spikes/d1-9/Cargo.toml".into(),
                output_excerpt: "test cache ... ok".into(),
            }
        );
        assert!(evidence.claim.contains("can be built"));
        assert!(state.journal.iter().any(|e| e.text.contains("removed")));
    }

    #[tokio::test]
    async fn an_unbuildable_piece_goes_back_to_modeling_after_cleaning_up() {
        let Fixture { shell, ctx, .. } = fixture(spike_turn(false), shell(), decided());
        ctx.set_data(SPIKE_KEY, Spike::for_decision(&decided().decisions[0], 9))
            .unwrap();
        let mut ctx = ctx.with_reply("ok");
        let outcome = Prototype::new().run(&mut ctx).await.unwrap();
        assert!(
            matches!(&outcome, NodeOutcome::Back { to: Stage::Modeling, reason } if reason.contains("[E1]")),
            "{outcome:?}"
        );
        assert_eq!(commands(&shell).len(), 4);
        assert!(
            ctx.into_state().evidence[0]
                .claim
                .contains("could not be built")
        );
    }

    #[tokio::test]
    async fn a_failed_worktree_add_stops_before_the_agent() {
        let shell = ScriptedShell::new().on(
            "git",
            ADD,
            CommandOutput {
                status: Some(128),
                stderr: "fatal: not a git repository".into(),
                ..CommandOutput::default()
            },
        );
        let Fixture {
            llm, shell, ctx, ..
        } = fixture(spike_turn(true), shell, decided());
        ctx.set_data(SPIKE_KEY, Spike::for_decision(&decided().decisions[0], 9))
            .unwrap();
        // The Spanish "sí" still approves.
        let mut ctx = ctx.with_reply("sí");
        let err = Prototype::new().run(&mut ctx).await.unwrap_err();
        assert!(err.to_string().contains("not a git repository"), "{err}");
        assert_eq!(commands(&shell).len(), 1);
        assert!(llm.requests().is_empty());
    }

    #[tokio::test]
    async fn goes_back_without_a_decision_and_skips_when_declined() {
        let node = Prototype::new();
        let Fixture { mut ctx, .. } = fixture(vec![], shell(), ResearchState::default());
        assert!(matches!(
            node.run(&mut ctx).await.unwrap(),
            NodeOutcome::Back {
                to: Stage::Modeling,
                ..
            }
        ));

        let Fixture { shell, mut ctx, .. } = fixture(vec![], shell(), decided());
        assert!(matches!(
            node.run(&mut ctx).await.unwrap(),
            NodeOutcome::NeedsInput { .. }
        ));
        let mut ctx = ctx.with_reply("no, no spike");
        assert_eq!(node.run(&mut ctx).await.unwrap(), NodeOutcome::Done);
        assert!(shell.calls().is_empty());
        let state = ctx.into_state();
        assert!(state.evidence.is_empty());
        assert!(state.journal[0].text.starts_with("No prototype"));
    }
}
