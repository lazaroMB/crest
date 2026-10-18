//! Frame of the research flow (X-Workflow): the node contract, its
//! graph-flow adapter, gates in the chat and conditional returns.
//!
//! # Pieces
//!
//! - [`node`]: [`ResearchNode`](node::ResearchNode), the trait each stage
//!   implements, with [`NodeContext`](node::NodeContext) and
//!   [`NodeOutcome`](node::NodeOutcome).
//! - [`task`]: [`NodeTask`](task::NodeTask), the graph-flow `Task` that
//!   runs a node and turns its outcome into a `NextAction`.
//! - [`builder`]: chains nodes into a [`ResearchGraph`](builder::ResearchGraph).
//! - [`flow`]: [`ResearchFlow`](flow::ResearchFlow), one session of the
//!   graph: `start`, `resume`, `step`, `run`, `provide_input` and the chat
//!   commands of [`commands`] through `handle_line`.
//! - [`nodes`]: the registry, one file per node.
//!
//! # Node contract
//!
//! A node implements [`ResearchNode`](node::ResearchNode):
//!
//! - `stage()` names its [`Stage`](domain::research::Stage); the gate
//!   (`has_gate`) and the model role ([`NodeContext::role`]) come from it.
//! - `run(&self, ctx)` reads [`NodeContext::problem`], [`NodeContext::state`]
//!   and, when rerun after a gate, [`NodeContext::user_reply`]; it records
//!   what it learns with [`NodeContext::state_mut`] and
//!   [`NodeContext::log`], shows blocks with [`NodeContext::emit`] (the
//!   `StageHeader` and the gate prompt are emitted by the frame) and calls
//!   the model with [`NodeContext::ask`], [`NodeContext::complete`] or
//!   [`NodeContext::agent`].
//! - It returns `Done`; `NeedsInput { prompt }` to ask the user and run
//!   again with the answer; or `Back { to, reason }` when an earlier stage
//!   is missing something: the flow goes back there instead of inventing
//!   it. `to` must be an earlier stage of the graph; after three returns
//!   from the same stage the frame asks the user instead.
//! - Gated stages are asked to confirm after `Done`: "sí"/"ok" moves on,
//!   any other text reruns the node with it as `user_reply`.
//! - Errors ([`NodeError`](node::NodeError)) leave the flow at the last
//!   checkpoint and are shown as an `Error` block.
//! - Node-specific data that must survive a restart goes in
//!   [`NodeContext::set_data`] (any key not starting with `research.`).
//!
//! [`NodeContext::role`]: node::NodeContext::role
//! [`NodeContext::problem`]: node::NodeContext::problem
//! [`NodeContext::state`]: node::NodeContext::state
//! [`NodeContext::user_reply`]: node::NodeContext::user_reply
//! [`NodeContext::state_mut`]: node::NodeContext::state_mut
//! [`NodeContext::log`]: node::NodeContext::log
//! [`NodeContext::emit`]: node::NodeContext::emit
//! [`NodeContext::ask`]: node::NodeContext::ask
//! [`NodeContext::complete`]: node::NodeContext::complete
//! [`NodeContext::agent`]: node::NodeContext::agent
//! [`NodeContext::set_data`]: node::NodeContext::set_data
//!
//! ```
//! use application::flows::research::node::{
//!     NodeContext, NodeError, NodeOutcome, ResearchNode,
//! };
//! use async_trait::async_trait;
//! use domain::block::BlockKind;
//! use domain::research::Stage;
//!
//! #[derive(Debug, Default)]
//! pub struct Hypotheses;
//!
//! impl Hypotheses {
//!     pub fn new() -> Self {
//!         Self
//!     }
//! }
//!
//! #[async_trait]
//! impl ResearchNode for Hypotheses {
//!     fn stage(&self) -> Stage {
//!         Stage::Hypotheses
//!     }
//!
//!     async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
//!         if ctx.state().evidence.is_empty() {
//!             return Ok(NodeOutcome::Back {
//!                 to: Stage::Data,
//!                 reason: "no hay evidencia para formular hipótesis".into(),
//!             });
//!         }
//!         let feedback = ctx.user_reply().unwrap_or("ninguna");
//!         let prompt = format!("Problema: {}\nCorrecciones: {feedback}", ctx.problem());
//!         let text = ctx.ask("Escribe una hipótesis de una oración.", &prompt).await?;
//!         ctx.state_mut().add_hypothesis(text.clone(), vec![], 1);
//!         ctx.emit(BlockKind::Markdown { title: None, source: text })?;
//!         ctx.log("Una hipótesis nueva.");
//!         Ok(NodeOutcome::Done)
//!     }
//! }
//! ```
//!
//! A node is tested alone with [`NodeContext::new`](node::NodeContext::new)
//! over a `ScriptedLlm` (feature `testing`), and inside a graph with
//! [`fake::FakeNode`] neighbours.

pub mod agents;
pub mod builder;
pub mod commands;
pub mod error;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod flow;
pub mod gate;
pub mod json;
pub mod node;
pub mod nodes;
pub mod task;

#[cfg(test)]
mod tests;
