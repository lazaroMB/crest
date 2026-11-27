//! Skills in the agent loop: the user's `/name` loads the skill with a
//! `skill` call before the model answers, what a skill grants lasts one
//! turn, and a compaction names the skills it drops.

use std::sync::Arc;

use async_trait::async_trait;
use domain::conversation::{ContentPart, Message, Role, ToolResultContent, ToolUseId, Turn};
use domain::permission::{PermissionMode, PermissionRule, RuleDecision, RuleScope, ToolEffect};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::testing::llm::ScriptedResponse;
use crate::tools::registry::{PreparedCall, ToolContext, ToolOutput, TypedTool};
use crate::usecases::skills::{SKILL_TOOL, by_user};

#[derive(Debug, Deserialize, JsonSchema)]
struct SkillInput {
    skill: String,
    #[serde(default)]
    args: Option<String>,
}

/// A `skill` that returns the skill's name and arguments, says who called
/// it, and grants `run_command(git diff *)` for the turn.
struct FakeSkill {
    /// Set once the service exists.
    permissions: Arc<std::sync::OnceLock<SharedPermissions>>,
}

#[async_trait]
impl TypedTool for FakeSkill {
    type Input = SkillInput;

    fn name(&self) -> &str {
        SKILL_TOOL
    }

    fn description(&self) -> &str {
        "Loads a skill."
    }

    fn effect(&self, _input: &SkillInput) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: SkillInput,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let who = if by_user(&ctx.tool_use_id) {
            "user"
        } else {
            "model"
        };
        let text = format!(
            "skill {} ({}) for the {who}",
            input.skill,
            input.args.unwrap_or_default()
        );
        let permissions = Arc::clone(self.permissions.get().unwrap());
        Ok(PreparedCall::new(
            input.skill,
            async move {
                permissions.lock().unwrap().grant([PermissionRule::parse(
                    "run_command(git diff *)",
                    RuleDecision::AllowAlways,
                    RuleScope::Session,
                )
                .unwrap()]);
                Ok(ToolOutput::text(text))
            }
            .boxed(),
        ))
    }
}

fn harness(script: Vec<ScriptedResponse>) -> Harness {
    let permissions = Arc::new(std::sync::OnceLock::new());
    let tool = FakeSkill {
        permissions: Arc::clone(&permissions),
    };
    let harness = Harness::with(script, PermissionMode::Ask, |tools| {
        tools.register_typed(tool).unwrap();
    });
    let _ = permissions.set(Arc::clone(harness.service.permissions()));
    harness
}

fn result_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|item| match item {
            ToolResultContent::Text { text } => Some(text.as_str()),
            ToolResultContent::Image { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn the_users_skill_is_loaded_before_the_model_answers() {
    let mut h = harness(vec![ScriptedResponse::text("Reviewed.")]);
    let opts = TurnOptions::default().with_skill("review", "src/lib.rs");
    h.service
        .run_turn(&mut h.session, "/review src/lib.rs", opts)
        .await
        .unwrap();
    let turn = h.session.turns.last().unwrap();
    assert_eq!(turn.messages.len(), 4, "{:#?}", turn.messages);
    assert_eq!(
        turn.messages[0].content,
        [ContentPart::Text {
            text: "/review src/lib.rs".into()
        }]
    );
    let ContentPart::ToolUse { id, name, input } = &turn.messages[1].content[0] else {
        panic!("{:?}", turn.messages[1]);
    };
    assert_eq!(turn.messages[1].role, Role::Assistant);
    assert!(by_user(id));
    assert_eq!(name, SKILL_TOOL);
    assert_eq!(input, &json!({ "skill": "review", "args": "src/lib.rs" }));
    assert_eq!(
        result_text(&turn.messages[2]),
        "skill review (src/lib.rs) for the user"
    );
    // The model's one request already has the skill's result.
    let requests = h.llm.requests();
    assert_eq!(requests.len(), 1);
    let sent = &requests[0].messages;
    assert_eq!(sent.len(), 3);
    assert!(matches!(sent[2].content[0], ContentPart::ToolResult { .. }));
    let trace = h.trace();
    assert!(trace.contains(&"added:tool_call".to_owned()), "{trace:?}");
    assert!(trace.contains(&"status:succeeded".to_owned()), "{trace:?}");
}

#[tokio::test]
async fn the_models_skill_call_is_not_the_users() {
    let mut h = harness(vec![
        ScriptedResponse::tool_use("toolu_1", SKILL_TOOL, json!({ "skill": "review" })),
        ScriptedResponse::text("ok"),
    ]);
    h.run("review my change").await.unwrap();
    let turn = h.session.turns.last().unwrap();
    assert_eq!(
        result_text(&turn.messages[2]),
        "skill review () for the model"
    );
}

#[tokio::test]
async fn what_a_skill_grants_ends_with_its_turn() {
    let mut h = harness(vec![
        ScriptedResponse::tool_use("toolu_1", SKILL_TOOL, json!({ "skill": "review" })),
        ScriptedResponse::text("ok"),
    ]);
    h.run("review").await.unwrap();
    assert!(
        h.service.permissions().lock().unwrap().grants.is_empty(),
        "the grants are dropped when the turn ends"
    );
}

#[tokio::test]
async fn a_compaction_names_the_skills_it_drops() {
    let mut old = Turn::new(Message::user_text("/review"));
    old.messages
        .push(Message::assistant(vec![ContentPart::ToolUse {
            id: ToolUseId::new("skill_user_1"),
            name: SKILL_TOOL.into(),
            input: json!({ "skill": "review" }),
        }]));
    let names = crate::usecases::skills::invoked([&old]);
    assert_eq!(names.into_iter().collect::<Vec<_>>(), ["review"]);
    let reminder =
        crate::usecases::skills::compaction_reminder(&["review".to_owned()].into_iter().collect())
            .unwrap();
    assert!(reminder.contains("review"));
    assert!(crate::usecases::skills::compaction_reminder(&Default::default()).is_none());
}

mod inbox {
    use domain::block::AgentStatus;

    use super::*;
    use crate::usecases::agents::{AgentInbox, AgentReport};

    fn report(id: u64) -> AgentReport {
        AgentReport {
            id,
            name: "Find x".into(),
            status: AgentStatus::Done,
            text: format!("report {id}"),
        }
    }

    fn texts(message: &Message) -> Vec<String> {
        message
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_waiting_report_joins_the_prompt() {
        let inbox = AgentInbox::new();
        inbox.push(report(1));
        let mut h = harness(vec![ScriptedResponse::text("ok")]);
        h.service = h.service.with_inbox(inbox.clone());
        h.run("hola").await.unwrap();
        let first = &h.session.turns.last().unwrap().messages[0];
        let texts = texts(first);
        assert_eq!(texts[0], "hola");
        assert!(texts[1].contains("<agent-report id=\"1\""), "{texts:?}");
        assert!(inbox.is_empty());
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    struct Nothing {}

    /// A tool during whose run a background agent ends.
    struct Slow {
        inbox: AgentInbox,
    }

    #[async_trait]
    impl TypedTool for Slow {
        type Input = Nothing;

        fn name(&self) -> &str {
            "slow"
        }

        fn description(&self) -> &str {
            "Takes a while."
        }

        fn effect(&self, _input: &Nothing) -> ToolEffect {
            ToolEffect::Read
        }

        async fn prepare(
            &self,
            _input: Nothing,
            _ctx: &ToolContext,
        ) -> Result<PreparedCall, ToolError> {
            let inbox = self.inbox.clone();
            Ok(PreparedCall::new(
                "slow",
                async move {
                    inbox.push(report(7));
                    Ok(ToolOutput::text("slow done"))
                }
                .boxed(),
            ))
        }
    }

    #[tokio::test]
    async fn a_report_that_arrives_during_the_turn_goes_with_the_next_results() {
        let inbox = AgentInbox::new();
        let tool = Slow {
            inbox: inbox.clone(),
        };
        let mut h = Harness::with(
            vec![
                ScriptedResponse::tool_use("toolu_1", "slow", json!({})),
                ScriptedResponse::text("ok"),
            ],
            PermissionMode::Ask,
            |tools| tools.register_typed(tool).unwrap(),
        );
        h.service = h.service.with_inbox(inbox.clone());
        h.run("go").await.unwrap();
        let turn = h.session.turns.last().unwrap();
        assert_eq!(
            texts(&turn.messages[0]),
            ["go"],
            "nothing waited at the start"
        );
        let results = &turn.messages[2];
        assert!(matches!(results.content[0], ContentPart::ToolResult { .. }));
        let reports = texts(results);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].contains("<agent-report id=\"7\""), "{reports:?}");
        // The second model call read it.
        let sent = &h.llm.requests()[1].messages;
        assert!(sent.last().unwrap().content.iter().any(|part| matches!(
            part,
            ContentPart::Text { text } if text.contains("report 7")
        )));
        assert!(inbox.is_empty());
    }
}
