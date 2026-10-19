//! The implementation flow: a second graph-flow graph that takes an
//! approved `plan.json` and runs it task by task. Here the agent stops
//! being creative: it executes, verifies and writes every departure from
//! the plan to the journal.
//!
//! # Steps
//!
//! | # | Step | Node | Gate |
//! | - | ---- | ---- | ---- |
//! | 1 | Load plan | [`LoadNode`](load::LoadNode) | plan and branch, before starting |
//! | 2 | Implement task | [`ImplementTask`](nodes::implement::ImplementTask) | when the agent says the task is blocked |
//! | 3 | Test and fix | [`TestTask`](nodes::test::TestTask) | after 3 failed fixes |
//! | 4 | Commit | [`CommitTask`](nodes::commit::CommitTask) | when git refuses |
//! | 5 | Verify hypotheses | [`VerifyPlan`](nodes::verify::VerifyPlan) | |
//! | 6 | Report and next steps | [`WriteReport`](nodes::report::WriteReport) | before closing |
//!
//! Steps 2–4 repeat for each task in dependency order: Commit (or a
//! skipped task) moves to the next task and goes back to Implement,
//! which goes on to Verify once none is left. Every step is a
//! checkpoint. At the problem gates the user answers "retry" (or
//! instructions for the agent), "continue", "skip", "abort" or
//! "back to spec" ([`gate`]); the last one ends the flow with
//! [`FlowStatus::BackToSpec`](state::FlowStatus::BackToSpec) so the chat
//! can reopen the Spec node of the research flow.
//!
//! # Assembling
//!
//! ```ignore
//! let deps = ImplementDeps::new(events, fs, shell)
//!     .with_agent(AgentKit::new(agent_deps, tools, models, permissions));
//! let graph = ImplementGraphBuilder::new(deps)
//!     .node(LoadNode::new())
//!     .node(ImplementTask::new())
//!     .node(TestTask::new())
//!     .node(CommitTask::new())
//!     .node(VerifyPlan::new())
//!     .node(WriteReport::new())
//!     .build()?;
//! let flow = ImplementFlow::start(graph, storage, session_id, plan_path).await?;
//! flow.run().await?; // stops at the first gate
//! flow.handle_line("yes").await?;
//! ```
//!
//! [`implement_graph`](graph::implement_graph) chains exactly these six
//! steps, and [`ImplementSession`](graph::ImplementSession) opens (or
//! resumes) the flow of one plan, as `/implement` does.
//!
//! # Modules
//!
//! - [`builder`]: chains steps into an [`ImplementGraph`](builder::ImplementGraph).
//! - [`checks`]: running the plan's checks through the shell.
//! - [`flow`]: [`ImplementFlow`](flow::ImplementFlow), one session of the graph.
//! - [`gate`]: the answers every gate accepts.
//! - [`graph`]: the six steps assembled ([`implement_graph`](graph::implement_graph))
//!   and [`ImplementSession`](graph::ImplementSession), one implementation of a Spec.
//! - [`load`]: reading, validating and ordering `plan.json`; the branch.
//! - [`measure`]: expected against obtained in check outputs.
//! - [`node`]: the step contract ([`ImplementNode`](node::ImplementNode)).
//! - [`task`]: [`StepTask`](task::StepTask), the graph-flow task of a step.
//! - [`nodes`]: the steps after Load plan, one file each.
//! - [`scope`]: file tools restricted to the files of a task.
//! - [`state`]: the steps and what the session keeps
//!   ([`ImplementState`](state::ImplementState)).

pub mod builder;
pub mod checks;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod flow;
pub mod gate;
pub mod graph;
pub mod load;
pub mod measure;
pub mod node;
pub mod nodes;
pub mod scope;
pub mod state;
pub mod task;

#[cfg(test)]
mod tests;
