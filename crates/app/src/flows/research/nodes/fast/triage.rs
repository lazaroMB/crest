//! Fast 1, Triage: Intake plus how deep to dig.
//!
//! Runs [`Intake`] inside the stage (its brief now also carries the size
//! of the change and whether its questions block the investigation), then
//! picks the [`Depth`] with [`depth::choose`](crate::flows::research::depth::choose)
//! and stores it under [`DEPTH_KEY`]. One line tells the user the depth
//! and how to change it; a large request also gets a notice suggesting
//! `/research --deep`.
//!
//! There is no confirmation gate. Only blocking questions stop the flow
//! (a `Question` gate): the answer reruns Intake with it, and "ok" goes on
//! with the brief as it is. Non-blocking questions are written to the
//! journal as assumptions.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::research::Stage;

use crate::flows::research::depth::{DEPTH_KEY, Depth, DepthChoice, choose};
use crate::flows::research::gate::{KEY_DEPTH, is_approval};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::flows::research::nodes::intake::{BRIEF_KEY, Intake, IntakeBrief};
use crate::ports::events::NoticeLevel;

/// Flow data key set while Triage waits for answers to blocking
/// questions (`bool`).
pub const ASKED_KEY: &str = "triage.asked";

/// Notice for requests that look too big for the fast pipeline.
pub const DEEP_NOTICE: &str =
    "This looks large; `/research --deep <problem>` runs the 14-stage flow.";

/// The Triage node.
#[derive(Debug, Default)]
pub struct Triage;

impl Triage {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ResearchNode for Triage {
    fn stage(&self) -> Stage {
        Stage::Triage
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let asked = ctx.data::<bool>(ASKED_KEY).unwrap_or(false);
        let previous: Option<IntakeBrief> = ctx.data(BRIEF_KEY);
        let go_on = asked && ctx.user_reply().is_some_and(is_approval);
        let brief = match previous.filter(|_| go_on) {
            Some(brief) => {
                ctx.log("The user goes on without answering the open questions.");
                brief
            }
            None => {
                let outcome = ctx.run_as(&Intake::new()).await?;
                if outcome != NodeOutcome::Done {
                    return Ok(outcome);
                }
                ctx.data(BRIEF_KEY)
                    .ok_or_else(|| NodeError::Failed("Intake left no brief".into()))?
            }
        };
        if !go_on && brief.blocking && !brief.questions.is_empty() {
            ctx.set_data(ASKED_KEY, true)?;
            ctx.log("The open questions block the investigation; asking the user.");
            return Ok(NodeOutcome::NeedsInput {
                prompt: blocking_prompt(&brief.questions),
            });
        }
        ctx.set_data(ASKED_KEY, false)?;
        if !brief.questions.is_empty() {
            ctx.log(format!(
                "Open questions taken as assumptions: {}",
                brief.questions.join(" ")
            ));
        }

        let requested: Option<String> = ctx.data(KEY_DEPTH);
        let choice = choose(requested.as_deref(), &brief, ctx.problem());
        ctx.set_data(DEPTH_KEY, choice.depth)?;
        ctx.emit(BlockKind::Markdown {
            title: None,
            source: depth_line(&choice),
        })?;
        if choice.suggest_deep {
            ctx.notice(NoticeLevel::Info, DEEP_NOTICE);
        }
        ctx.log(format!(
            "Depth {} ({}).",
            choice.depth.name(),
            choice.reason
        ));
        Ok(NodeOutcome::Done)
    }
}

/// The question of a blocking brief.
fn blocking_prompt(questions: &[String]) -> String {
    let mut out = String::from("Before investigating I need to know:\n\n");
    for (i, q) in questions.iter().enumerate() {
        let _ = writeln!(out, "{}. {q}", i + 1);
    }
    out.push_str("\nAnswer them, or \"ok\" to go on taking them as assumptions.");
    out
}

/// The line that tells the user the depth and how to change it.
fn depth_line(choice: &DepthChoice) -> String {
    let change = match choice.depth {
        Depth::Quick => "`/research --deep <problem>` runs the 14-stage flow instead".to_owned(),
        Depth::Standard => "`/research --quick <problem>` runs a lighter pass and \
                            `/research --deep <problem>` the 14-stage flow"
            .to_owned(),
    };
    format!(
        "**Depth:** {} ({}). {change}.",
        choice.depth.name(),
        choice.reason
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::model::ModelPolicy;
    use domain::research::ResearchState;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    fn context(
        llm: &Arc<ScriptedLlm>,
        sink: &Arc<RecordingSink>,
        depth: Option<&str>,
    ) -> NodeContext {
        let deps =
            NodeDeps::new(llm.clone(), ModelPolicy::default(), sink.clone()).with_clock(|| 3);
        let flow = graph_flow::Context::new();
        if let Some(depth) = depth {
            flow.set(KEY_DEPTH, depth).unwrap();
        }
        NodeContext::in_flow(
            Stage::Triage,
            deps,
            ResearchState::default(),
            "the login button does nothing".into(),
            None,
            flow,
        )
    }

    fn answer(kind: &str, size: &str, questions: &[&str], blocking: bool) -> ScriptedResponse {
        ScriptedResponse::text(
            serde_json::json!({
                "kind": kind, "restated": "The login button does nothing.",
                "questions": questions, "size": size, "blocking": blocking,
            })
            .to_string(),
        )
    }

    fn markdown(sink: &RecordingSink) -> Vec<String> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => match block.kind {
                    BlockKind::Markdown { source, .. } => Some(source),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn notices(sink: &RecordingSink) -> Vec<String> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::Notice { message, .. } => Some(message),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_small_bug_goes_quick() {
        let llm = Arc::new(ScriptedLlm::new([answer(
            "bug",
            "small",
            &["Which browser?"],
            false,
        )]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, None);

        assert_eq!(
            Triage::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );

        assert_eq!(ctx.depth(), Depth::Quick);
        assert_eq!(ctx.stage(), Stage::Triage);
        let shown = markdown(&sink);
        assert_eq!(shown.len(), 1);
        assert!(
            shown[0].contains("**Depth:** quick (small bug)"),
            "{}",
            shown[0]
        );
        assert!(shown[0].contains("--deep"));
        assert!(notices(&sink).is_empty());
        let journal = &ctx.state().journal;
        assert_eq!(journal[0].stage, Stage::Intake);
        assert!(
            journal
                .iter()
                .any(|e| e.text.contains("assumptions: Which browser?"))
        );
        assert!(
            journal
                .iter()
                .any(|e| e.stage == Stage::Triage && e.text.contains("quick"))
        );
    }

    #[tokio::test]
    async fn a_large_request_suggests_the_deep_flow() {
        let llm = Arc::new(ScriptedLlm::new([answer("feature", "large", &[], false)]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, None);
        Triage::new().run(&mut ctx).await.unwrap();
        assert_eq!(ctx.depth(), Depth::Standard);
        assert_eq!(notices(&sink), [DEEP_NOTICE]);
    }

    #[tokio::test]
    async fn the_depth_asked_for_at_the_start_wins() {
        let llm = Arc::new(ScriptedLlm::new([answer("feature", "large", &[], false)]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, Some("quick"));
        Triage::new().run(&mut ctx).await.unwrap();
        assert_eq!(ctx.depth(), Depth::Quick);
        assert!(markdown(&sink)[0].contains("asked for at the start"));
    }

    #[tokio::test]
    async fn blocking_questions_are_asked_and_ok_goes_on() {
        let llm = Arc::new(ScriptedLlm::new([answer(
            "bug",
            "small",
            &["Which endpoint?"],
            true,
        )]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, None);

        let outcome = Triage::new().run(&mut ctx).await.unwrap();
        let NodeOutcome::NeedsInput { prompt } = outcome else {
            panic!("expected a question, got {outcome:?}");
        };
        assert!(prompt.contains("1. Which endpoint?") && prompt.contains("\"ok\""));
        assert!(ctx.data::<Depth>(DEPTH_KEY).is_none());

        // "ok" goes on with the brief as it is, without asking the model.
        let mut ctx = ctx.with_reply("ok");
        assert_eq!(
            Triage::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );
        assert_eq!(llm.requests().len(), 1);
        assert_eq!(ctx.depth(), Depth::Quick);
    }

    #[tokio::test]
    async fn an_answer_to_blocking_questions_reruns_intake() {
        let llm = Arc::new(ScriptedLlm::new([
            answer("bug", "small", &["Which endpoint?"], true),
            answer("bug", "small", &[], false),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, None);
        Triage::new().run(&mut ctx).await.unwrap();
        let mut ctx = ctx.with_reply("POST /login");
        assert_eq!(
            Triage::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );
        let brief: IntakeBrief = ctx.data(BRIEF_KEY).unwrap();
        assert_eq!(brief.answers, ["POST /login"]);
        assert!(format!("{:?}", llm.last_request().unwrap().messages).contains("POST /login"));
    }
}
