//! Fast 2, Discovery: what the repository, its history and prior art say
//! about the problem, gathered at once.
//!
//! Up to four read-only agent turns run concurrently (`futures::join!`,
//! never `tokio::spawn`, so cancelling the flow cancels them):
//!
//! | Part         | Gathers                                   | Required | Depth           |
//! | ------------ | ----------------------------------------- | -------- | --------------- |
//! | map          | [`Observation::gather`]: tree, key files  | yes      | always          |
//! | history      | [`Data::gather_repo`]: git log, logs      | no       | always          |
//! | prior art    | [`Research::gather_repo`]: repo patterns  | no       | standard        |
//! | web          | [`Research::gather_web`]: libraries, docs | no       | standard + web  |
//!
//! Only then are the results recorded, in that fixed order, so the blocks
//! never depend on which turn finished first. A failed optional part is a
//! warning; a failed map fails the stage. Each finished part is kept under
//! `discovery.done.<part>` until the stage succeeds, so a retry only runs
//! the parts that had not finished.
//!
//! The prior solutions are always stored under [`OPTIONS_KEY`] (maybe
//! empty), which Brainstorm needs. Each agent turn is limited to
//! [`Depth::max_iterations`] model calls.

use std::fmt::Write as _;
use std::future::Future;

use async_trait::async_trait;
use domain::block::{BlockKind, SourceRef};
use domain::research::Stage;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::flows::research::depth::Depth;
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::flows::research::nodes::data::Data;
use crate::flows::research::nodes::intake::{BRIEF_KEY, IntakeBrief};
use crate::flows::research::nodes::observation::{Observation, has_web};
use crate::flows::research::nodes::research::{OPTIONS_KEY, Research, table};
use crate::ports::events::NoticeLevel;

/// Prefix of the flow data keys of the finished parts.
pub const DONE_PREFIX: &str = "discovery.done.";

/// The parts of Discovery, in the order they are recorded.
const PARTS: [&str; 4] = ["map", "history", "prior_art", "web"];

/// A finished part, kept until the stage succeeds.
#[derive(Debug, Serialize, Deserialize)]
struct Done<T> {
    value: T,
}

fn done_key(part: &str) -> String {
    format!("{DONE_PREFIX}{part}")
}

/// The result of `part`: kept from an earlier attempt, else `gather`
/// (kept once it succeeds).
async fn cached<T, F>(ctx: &NodeContext, part: &str, gather: F) -> Result<T, NodeError>
where
    T: Serialize + DeserializeOwned,
    F: Future<Output = Result<T, NodeError>>,
{
    let key = done_key(part);
    if let Some(done) = ctx.data::<Option<Done<T>>>(&key).flatten() {
        return Ok(done.value);
    }
    let value = gather.await?;
    ctx.set_data(&key, Some(Done { value: &value }))?;
    Ok(value)
}

/// The Discovery node.
#[derive(Debug, Default)]
pub struct Discovery {
    observation: Observation,
    data: Data,
    research: Research,
}

impl Discovery {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Warns that the optional `part` failed and goes on without it.
    fn skipped(ctx: &mut NodeContext, part: &str, error: &NodeError) {
        let message =
            format!("Discovery: the {part} could not be gathered ({error}); going on without it.");
        ctx.notice(NoticeLevel::Warning, &message);
        ctx.log(message);
    }
}

/// What the prior-art turns are asked about: the brief, since the problem
/// statements come later in this pipeline.
fn prior_art_brief(problem: &str, brief: &IntakeBrief) -> String {
    let mut out = format!(
        "Problem ({}): {}\nOriginal request: {problem}\n",
        brief.kind.name(),
        brief.restated
    );
    for answer in &brief.answers {
        let _ = writeln!(out, "User clarification: {answer}");
    }
    out
}

#[async_trait]
impl ResearchNode for Discovery {
    fn stage(&self) -> Stage {
        Stage::Discovery
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let Some(brief) = ctx.data::<IntakeBrief>(BRIEF_KEY) else {
            return Ok(NodeOutcome::Back {
                to: Stage::Intake,
                reason: "the problem is not defined: the restated request from Triage is missing"
                    .into(),
            });
        };
        let depth = ctx.depth();
        ctx.set_max_iterations(Some(depth.max_iterations()));
        let standard = depth == Depth::Standard;
        let web = standard && has_web(ctx);
        let about = prior_art_brief(ctx.problem(), &brief);
        let first_evidence = ctx.state().evidence.len();

        let shared: &NodeContext = ctx;
        let (map, history, prior_art, found_online) = futures::join!(
            cached(shared, PARTS[0], self.observation.gather(shared, &brief)),
            cached(
                shared,
                PARTS[1],
                self.data.gather_repo(shared, &brief, None)
            ),
            async {
                if standard {
                    Some(cached(shared, PARTS[2], self.research.gather_repo(shared, &about)).await)
                } else {
                    None
                }
            },
            async {
                if web {
                    Some(cached(shared, PARTS[3], self.research.gather_web(shared, &about)).await)
                } else {
                    None
                }
            },
        );
        // Without the map there is nothing to build on; the finished parts
        // stay cached for the retry.
        let map = map?;

        let observed = self.observation.apply(ctx, map)?;
        match history {
            Ok(history) => {
                self.data.apply(ctx, history, None)?;
            }
            Err(error) => Self::skipped(ctx, "history", &error),
        }
        let options_from = ctx.state().evidence.len();
        let mut options = Vec::new();
        match prior_art {
            Some(Ok(found)) => options.extend(self.research.apply_repo(ctx, found)),
            Some(Err(error)) => Self::skipped(ctx, "prior art in the repo", &error),
            None => {}
        }
        let in_repo = options.len();
        match found_online {
            Some(Ok(found)) => options.extend(self.research.apply_web(ctx, found)),
            Some(Err(error)) => Self::skipped(ctx, "web research", &error),
            None => {}
        }
        if !options.is_empty() {
            ctx.emit(BlockKind::Table(table(&options)))?;
            let sources: Vec<SourceRef> = ctx.state().evidence[options_from..]
                .iter()
                .map(|e| e.source.clone())
                .collect();
            if !sources.is_empty() {
                ctx.emit(BlockKind::Sources { sources })?;
            }
        }
        ctx.set_data(OPTIONS_KEY, &options)?;
        for part in PARTS {
            ctx.set_data(&done_key(part), None::<()>)?;
        }

        let new_evidence = ctx.state().evidence.len() - first_evidence;
        ctx.log(format!(
            "Discovery ({}): {} key files, {new_evidence} new piece(s) of evidence, {} prior \
             solution(s) ({in_repo} in the repo, {} on the web){}.",
            depth.name(),
            observed.key_files.len(),
            options.len(),
            options.len() - in_repo,
            if standard {
                ""
            } else {
                "; prior art skipped at quick depth"
            },
        ));
        Ok(NodeOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::block::BlockKind;
    use serde_json::json;

    use super::*;
    use crate::flows::research::depth::DEPTH_KEY;
    use crate::flows::research::nodes::observation::testing::{
        agent_over, blocks, brief, context_over, workspace,
    };
    use crate::flows::research::nodes::research::PriorSolution;
    use crate::ports::llm::LlmPort;
    use crate::testing::llm::{BarrierLlm, RoutedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    const MAP: &str = "Observation stage";
    const HISTORY: &str = "Data stage";
    const PRIOR_ART: &str = "You are the \"Research\" node";

    fn map() -> ScriptedResponse {
        ScriptedResponse::text(
            json!({
                "summary": "A crate with the login.",
                "architecture": "flowchart TD\n  login --> hash",
                "key_files": [{"path": "src/login.rs", "kind": "code", "role": "logs in"}],
                "observations": [{"claim": "login hashes on every attempt.",
                    "source": {"type": "file", "path": "src/login.rs", "line_start": 1}}]
            })
            .to_string(),
        )
    }

    fn history() -> ScriptedResponse {
        ScriptedResponse::text(
            json!({
                "commits": [{"sha": "abc123", "date": "2026-01-05", "summary": "use argon2"}],
                "findings": []
            })
            .to_string(),
        )
    }

    fn prior_art() -> ScriptedResponse {
        ScriptedResponse::text(
            json!({"findings": [{"name": "session cache", "summary": "caches sessions",
                "path": "src/cache.rs", "line_start": 1, "line_end": 9}]})
            .to_string(),
        )
    }

    fn routed() -> RoutedLlm {
        RoutedLlm::new()
            .route(MAP, [map()])
            .route(HISTORY, [history()])
            .route(PRIOR_ART, [prior_art()])
    }

    fn context(llm: Arc<dyn LlmPort>, sink: &Arc<RecordingSink>) -> NodeContext {
        let service = agent_over(Arc::clone(&llm), &workspace(), None, |_| {});
        let ctx = context_over(Stage::Discovery, llm, sink, Some(service));
        ctx.set_data(BRIEF_KEY, brief()).unwrap();
        ctx
    }

    fn options(ctx: &NodeContext) -> Vec<PriorSolution> {
        ctx.data(OPTIONS_KEY)
            .expect("the options are always stored")
    }

    #[tokio::test]
    async fn the_parts_run_at_once_and_are_shown_in_a_fixed_order() {
        let routed = Arc::new(routed());
        // Run one after another, the first call would time out here.
        let llm: Arc<dyn LlmPort> = Arc::new(BarrierLlm::new(routed.clone(), 3));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(llm, &sink);

        let outcome = Discovery::new().run(&mut ctx).await.unwrap();

        assert_eq!(outcome, NodeOutcome::Done);
        assert_eq!(routed.remaining(), 0);
        let shown = blocks(&sink);
        let kinds: Vec<&str> = shown
            .iter()
            .map(|b| match b {
                BlockKind::Mermaid(_) => "mermaid",
                BlockKind::Table(_) => "table",
                BlockKind::Sources { .. } => "sources",
                BlockKind::Chart(_) => "chart",
                _ => "other",
            })
            .collect();
        // Map (diagram, key files, sources), history (chart, table,
        // sources), prior art (table, sources).
        assert_eq!(
            kinds,
            [
                "mermaid", "table", "sources", "chart", "table", "sources", "table", "sources"
            ]
        );
        assert!(matches!(&shown[6], BlockKind::Table(t) if t.rows.len() == 1));
        assert_eq!(options(&ctx)[0].name, "session cache");
        // E1 from the map, E2 from the history, E3 from the prior art.
        assert_eq!(ctx.state().evidence.len(), 3);
        assert!(
            ctx.state()
                .journal
                .last()
                .unwrap()
                .text
                .contains("standard")
        );
        assert!(
            ctx.data::<Option<Done<serde_json::Value>>>(&done_key("map"))
                .flatten()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_failed_optional_part_is_a_warning() {
        // No route answers the history.
        let llm = Arc::new(
            RoutedLlm::new()
                .route(MAP, [map()])
                .route(PRIOR_ART, [prior_art()]),
        );
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(llm, &sink);

        assert_eq!(
            Discovery::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );

        let warnings: Vec<String> = sink
            .events()
            .into_iter()
            .filter_map(|e| match e {
                crate::ports::events::DomainEvent::Notice {
                    level: NoticeLevel::Warning,
                    message,
                } => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("history"));
        assert_eq!(options(&ctx).len(), 1);
    }

    #[tokio::test]
    async fn quick_depth_skips_prior_art_but_stores_the_options() {
        let llm = Arc::new(routed());
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(llm.clone(), &sink);
        ctx.set_data(DEPTH_KEY, Depth::Quick).unwrap();

        Discovery::new().run(&mut ctx).await.unwrap();

        assert!(llm.requests(PRIOR_ART).is_empty());
        assert_eq!(llm.requests(MAP).len(), 1);
        assert_eq!(llm.requests(HISTORY).len(), 1);
        assert!(options(&ctx).is_empty());
        assert!(ctx.state().journal.last().unwrap().text.contains("quick"));
    }

    #[tokio::test]
    async fn a_retry_only_runs_the_parts_that_had_not_finished() {
        // The map has no answer the first time.
        let llm = Arc::new(
            RoutedLlm::new()
                .route(MAP, [])
                .route(HISTORY, [history()])
                .route(PRIOR_ART, [prior_art()]),
        );
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(llm.clone(), &sink);
        assert!(Discovery::new().run(&mut ctx).await.is_err());
        assert!(blocks(&sink).is_empty());

        llm.push(MAP, map());
        assert_eq!(
            Discovery::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );
        assert_eq!(llm.requests(HISTORY).len(), 1);
        assert_eq!(llm.requests(PRIOR_ART).len(), 1);
        assert_eq!(ctx.state().evidence.len(), 3);
    }

    #[tokio::test]
    async fn without_the_brief_it_goes_back_to_triage() {
        let llm = Arc::new(RoutedLlm::new());
        let sink = Arc::new(RecordingSink::new());
        let service = agent_over(llm.clone(), &workspace(), None, |_| {});
        let mut ctx = context_over(Stage::Discovery, llm, &sink, Some(service));
        let outcome = Discovery::new().run(&mut ctx).await.unwrap();
        let NodeOutcome::Back { to, .. } = outcome else {
            panic!("expected a return, got {outcome:?}");
        };
        assert_eq!(to.owner_in(domain::research::Pipeline::Fast), Stage::Triage);
    }
}
