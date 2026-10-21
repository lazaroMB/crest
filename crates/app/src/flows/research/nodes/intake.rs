//! Stage 1, Intake: restates the request, classifies it (feature, bug,
//! performance, refactor) and asks what is missing to investigate it.
//!
//! The result is an [`IntakeBrief`] stored under [`BRIEF_KEY`] and shown as
//! a `Text` block. The stage has a gate: an answer other than an approval
//! reruns the node with it, and the answer is added to the brief before it
//! is restated again.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::research::Stage;
use serde::{Deserialize, Serialize};

use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Flow data key of the [`IntakeBrief`].
pub const BRIEF_KEY: &str = "intake.brief";

const SYSTEM: &str = "You are the first step of an investigation into a software project. \
Restate the user's request in one or two precise sentences, classify it and ask only for \
what is missing to investigate it (steps to reproduce, expected behaviour, scope, metrics, \
environment). Do not make up data or propose solutions.
Reply only with a JSON object, with no text around it:
{\"kind\": \"feature\" | \"bug\" | \"performance\" | \"refactor\", \"restated\": \"…\", \
\"questions\": [\"…\"], \"size\": \"small\" | \"medium\" | \"large\", \"blocking\": false}
If nothing is missing, `questions` is []. `size` is how big the change looks: small (one \
module, a few files), medium or large (several modules, a new subsystem or a redesign). \
`blocking` is true only when the investigation cannot start without the answers; otherwise \
the questions are taken as assumptions.";

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// New functionality.
    #[serde(alias = "funcionalidad")]
    Feature,
    /// Something that does not work as expected.
    #[serde(alias = "error")]
    Bug,
    /// Something too slow or too costly.
    #[serde(alias = "rendimiento")]
    Performance,
    /// Restructuring without changing behaviour.
    Refactor,
}

impl RequestKind {
    /// Name shown to the user.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Feature => "New feature",
            Self::Bug => "Bug",
            Self::Performance => "Performance",
            Self::Refactor => "Refactor",
        }
    }
}

/// How big a change the request looks, as Intake judges it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    /// One module, a few files.
    #[serde(alias = "pequeño", alias = "pequeno")]
    Small,
    /// Several files or a couple of modules.
    #[serde(alias = "mediano")]
    Medium,
    /// Several modules, a new subsystem or a redesign.
    #[serde(alias = "grande")]
    Large,
}

impl Size {
    /// Name shown to the user.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

/// The request as Intake understood it, read by the later stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntakeBrief {
    /// Classification of the request.
    pub kind: RequestKind,
    /// The request restated in one or two sentences.
    pub restated: String,
    /// What is still missing, asked to the user.
    pub questions: Vec<String>,
    /// The user's answers at the gate, oldest first.
    #[serde(default)]
    pub answers: Vec<String>,
    /// How big the change looks; `None` in briefs saved before it existed
    /// or when the model left it out.
    #[serde(default)]
    pub size: Option<Size>,
    /// Whether the questions must be answered before investigating (Triage
    /// asks them); otherwise they are taken as assumptions.
    #[serde(default)]
    pub blocking: bool,
}

/// What the model answers.
#[derive(Debug, Deserialize)]
struct Answer {
    kind: RequestKind,
    restated: String,
    #[serde(default)]
    questions: Vec<String>,
    #[serde(default)]
    size: Option<Size>,
    #[serde(default)]
    blocking: bool,
}

impl Answer {
    fn validate(&self) -> Result<(), String> {
        if self.restated.trim().is_empty() {
            return Err("`restated` is empty".into());
        }
        if self.questions.iter().any(|q| q.trim().is_empty()) {
            return Err("`questions` has an empty question".into());
        }
        Ok(())
    }
}

/// The Intake node.
#[derive(Debug, Default)]
pub struct Intake;

impl Intake {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// The user prompt: the request, the previous brief and the answers.
fn prompt(problem: &str, previous: Option<&IntakeBrief>, answers: &[String]) -> String {
    let mut out = format!("Original request:\n{problem}\n");
    if let Some(brief) = previous {
        let _ = write!(
            out,
            "\nPrevious restatement ({}): {}\n",
            brief.kind.name(),
            brief.restated
        );
        if !brief.questions.is_empty() {
            out.push_str("Questions asked:\n");
            for q in &brief.questions {
                let _ = writeln!(out, "- {q}");
            }
        }
    }
    if !answers.is_empty() {
        out.push_str("\nUser answers:\n");
        for a in answers {
            let _ = writeln!(out, "- {a}");
        }
        out.push_str(
            "\nIncorporate the answers, restate again and ask only for what is still missing.\n",
        );
    }
    out
}

/// The `Text` block shown to the user.
fn render(brief: &IntakeBrief) -> String {
    let mut out = format!(
        "**Kind:** {}\n\n**Restated request:** {}\n",
        brief.kind.name(),
        brief.restated
    );
    if brief.questions.is_empty() {
        out.push_str("\nNo open questions.\n");
    } else {
        out.push_str("\n**Questions:**\n\n");
        for (i, q) in brief.questions.iter().enumerate() {
            let _ = writeln!(out, "{}. {q}", i + 1);
        }
    }
    out
}

#[async_trait]
impl ResearchNode for Intake {
    fn stage(&self) -> Stage {
        Stage::Intake
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let previous: Option<IntakeBrief> = ctx.data(BRIEF_KEY);
        let mut answers = previous
            .as_ref()
            .map(|b| b.answers.clone())
            .unwrap_or_default();
        if let Some(reply) = ctx.user_reply().map(str::trim).filter(|r| !r.is_empty()) {
            answers.push(reply.to_owned());
        }
        let prompt = prompt(ctx.problem(), previous.as_ref(), &answers);
        let answer: Answer = ctx.ask_json(SYSTEM, &prompt, Answer::validate).await?;
        let brief = IntakeBrief {
            kind: answer.kind,
            restated: answer.restated.trim().to_owned(),
            questions: answer
                .questions
                .iter()
                .map(|q| q.trim().to_owned())
                .collect(),
            answers,
            size: answer.size,
            blocking: answer.blocking,
        };
        ctx.set_data(BRIEF_KEY, &brief)?;
        ctx.emit(BlockKind::Text {
            markdown: render(&brief),
        })?;
        let action = if ctx.user_reply().is_some() {
            "Request restated with the user's answer"
        } else {
            "Request restated"
        };
        ctx.log(format!(
            "{action} and classified as {}: {} ({} open questions).",
            brief.kind.name(),
            brief.restated,
            brief.questions.len()
        ));
        Ok(NodeOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::model::{ModelPolicy, ModelRole};
    use domain::research::ResearchState;

    use super::*;
    use crate::flows::research::json::RETRY_PREFIX as RETRY;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    fn context(llm: &Arc<ScriptedLlm>, sink: &Arc<RecordingSink>) -> NodeContext {
        let deps =
            NodeDeps::new(llm.clone(), ModelPolicy::default(), sink.clone()).with_clock(|| 5);
        NodeContext::new(
            Stage::Intake,
            deps,
            "login is very slow",
            ResearchState::default(),
        )
    }

    fn texts(sink: &RecordingSink) -> Vec<String> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => match block.kind {
                    BlockKind::Text { markdown } => Some(markdown),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    const ANSWER: &str = r#"```json
{"kind": "performance", "restated": "Login takes more than 5 s.",
 "questions": ["Since which version?", "In which environment?"]}
```"#;

    #[tokio::test]
    async fn classifies_restates_and_asks() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(ANSWER)]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink);

        assert_eq!(
            Intake::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );

        let brief: IntakeBrief = ctx.data(BRIEF_KEY).unwrap();
        assert_eq!(brief.kind, RequestKind::Performance);
        assert_eq!(brief.restated, "Login takes more than 5 s.");
        assert_eq!(brief.questions.len(), 2);
        assert!(brief.answers.is_empty());
        let shown = texts(&sink);
        assert_eq!(shown.len(), 1);
        assert!(shown[0].contains("**Kind:** Performance"));
        assert!(shown[0].contains("1. Since which version?"));
        let req = llm.last_request().unwrap();
        assert_eq!(req.model.role, ModelRole::Main);
        let journal = &ctx.state().journal;
        assert_eq!((journal.len(), journal[0].stage), (1, Stage::Intake));
        assert!(journal[0].text.contains("2 open questions"));
    }

    #[tokio::test]
    async fn a_gate_reply_is_incorporated_and_restated() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(
            r#"{"kind": "bug", "restated": "Since v2, login on Linux takes 5 s.", "questions": []}"#,
        )]));
        let sink = Arc::new(RecordingSink::new());
        let ctx = context(&llm, &sink);
        let previous = IntakeBrief {
            kind: RequestKind::Performance,
            restated: "Login takes more than 5 s.".into(),
            questions: vec!["Since which version?".into()],
            answers: vec![],
            size: None,
            blocking: false,
        };
        ctx.set_data(BRIEF_KEY, &previous).unwrap();
        let mut ctx = ctx.with_reply("since v2, on Linux");

        Intake::new().run(&mut ctx).await.unwrap();

        let brief: IntakeBrief = ctx.data(BRIEF_KEY).unwrap();
        assert_eq!(brief.kind, RequestKind::Bug);
        assert_eq!(brief.answers, ["since v2, on Linux"]);
        assert!(brief.questions.is_empty());
        let sent = format!("{:?}", llm.last_request().unwrap().messages);
        assert!(sent.contains("since v2, on Linux"));
        assert!(sent.contains("Since which version?"));
        assert!(texts(&sink)[0].contains("No open questions"));
        assert!(ctx.state().journal[0].text.contains("user's answer"));
    }

    #[test]
    fn briefs_saved_before_size_and_blocking_still_parse() {
        let old = r#"{"kind": "bug", "restated": "x", "questions": ["q"], "answers": []}"#;
        let brief: IntakeBrief = serde_json::from_str(old).unwrap();
        assert_eq!((brief.size, brief.blocking), (None, false));
        let new: Answer = serde_json::from_str(
            r#"{"kind": "bug", "restated": "x", "size": "small", "blocking": true}"#,
        )
        .unwrap();
        assert_eq!((new.size, new.blocking), (Some(Size::Small), true));
    }

    #[tokio::test]
    async fn an_invalid_answer_is_retried_once() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("it is a bug"),
            ScriptedResponse::text(r#"{"kind": "refactor", "restated": "Split the module."}"#),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink);
        Intake::new().run(&mut ctx).await.unwrap();
        let brief: IntakeBrief = ctx.data(BRIEF_KEY).unwrap();
        assert_eq!(brief.kind, RequestKind::Refactor);
        let retry = llm.last_request().unwrap();
        assert_eq!(retry.messages.len(), 3);
        assert!(format!("{:?}", retry.messages[2]).contains(RETRY));
    }

    #[tokio::test]
    async fn two_invalid_answers_fail_without_state() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text(r#"{"kind": "bug", "restated": ""}"#),
            ScriptedResponse::text(r#"{"kind": "other", "restated": "x"}"#),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink);
        let err = Intake::new().run(&mut ctx).await.unwrap_err();
        assert!(matches!(err, NodeError::Failed(_)), "{err}");
        assert!(ctx.data::<IntakeBrief>(BRIEF_KEY).is_none());
        assert!(texts(&sink).is_empty());
    }
}
