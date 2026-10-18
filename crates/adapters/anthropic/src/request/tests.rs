use application::ports::llm::{ServerTool, SystemBlock, ToolChoice, ToolDefinition};
use domain::conversation::Message;
use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
use serde_json::json;

use super::*;

fn request(model: &str, thinking: ThinkingConfig) -> LlmRequest {
    let model = ResolvedModel {
        role: ModelRole::Main,
        spec: ModelSpec::new(model, 16_000).with_thinking(thinking),
        source: SpecSource::Default,
        warnings: vec![],
    };
    LlmRequest::new(model, vec![Message::user_text("Find the bug")])
}

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(name, "A tool", json!({"type": "object"}))
}

fn build_for(req: &LlmRequest) -> (MessagesRequest, Vec<RequestWarning>) {
    build_with_warnings(req, &req.model.spec.id)
}

#[test]
fn rules_match_by_longest_prefix() {
    assert_eq!(rules_for("claude-opus-5-5"), NO_FORCED);
    assert_eq!(rules_for("claude-opus-5"), ADAPTIVE_ONLY);
    assert_eq!(rules_for("claude-opus-5-20260101"), ADAPTIVE_ONLY);
    assert!(!rules_for("claude-haiku-4-5-20251001").programmatic_tool_calling);
    assert_eq!(rules_for("claude-opus-55"), ModelRules::PERMISSIVE);
    assert_eq!(rules_for("gpt-x"), ModelRules::PERMISSIVE);
}

#[test]
fn opus_5_5_degrades_forced_tool_choice_and_budget_thinking() {
    let mut req = request(
        "claude-opus-5-5",
        ThinkingConfig::Enabled {
            budget_tokens: 8000,
        },
    );
    req.tools = vec![tool("read_file")];
    req.tool_choice = ToolChoice::Tool("read_file".into());
    let (body, warnings) = build_for(&req);
    insta::assert_json_snapshot!(body);
    assert_eq!(
        warnings,
        vec![RequestWarning::BudgetToAdaptive {
            model: "claude-opus-5-5".into()
        }]
    );

    // Without thinking a forced choice is not hidden by the thinking rule,
    // so the model rule is what removes it.
    req.model.spec.thinking = ThinkingConfig::Disabled;
    let (body, warnings) = build_for(&req);
    assert_eq!(body.tool_choice, None);
    assert_eq!(
        warnings,
        vec![RequestWarning::ForcedToolChoice {
            model: "claude-opus-5-5".into()
        }]
    );
}

#[test]
fn opus_5_keeps_forced_tool_choice() {
    let mut req = request("claude-opus-5", ThinkingConfig::Disabled);
    req.tools = vec![tool("read_file")];
    req.tool_choice = ToolChoice::Any;
    let (body, warnings) = build_for(&req);
    assert_eq!(body.tool_choice, Some(WireToolChoice::Any));
    assert!(warnings.is_empty());
}

#[test]
fn haiku_4_5_calls_web_tools_directly_and_drops_adaptive_thinking() {
    let mut req = request("claude-haiku-4-5", ThinkingConfig::Adaptive);
    req.server_tools = vec![
        ServerTool::WebSearch { max_uses: Some(3) },
        ServerTool::WebFetch { max_uses: None },
    ];
    let (body, warnings) = build_for(&req);
    insta::assert_json_snapshot!(body);
    assert_eq!(
        warnings,
        vec![RequestWarning::AdaptiveDropped {
            model: "claude-haiku-4-5".into()
        }]
    );
}

#[test]
fn sonnet_5_5_web_tools_keep_the_default_callers() {
    let mut req = request("claude-sonnet-5-5", ThinkingConfig::Adaptive);
    req.server_tools = vec![ServerTool::WebSearch { max_uses: None }];
    let (body, _) = build_for(&req);
    let value = serde_json::to_value(&body).unwrap();
    assert_eq!(value["tools"][0].get("allowed_callers"), None);
    assert_eq!(value["thinking"], json!({"type": "adaptive"}));
}

#[test]
fn thinking_budget_is_kept_within_api_limits() {
    let mut req = request(
        "claude-opus-4-6",
        ThinkingConfig::Enabled {
            budget_tokens: 50_000,
        },
    );
    let (body, warnings) = build_for(&req);
    insta::assert_json_snapshot!(body);
    assert_eq!(
        warnings,
        vec![RequestWarning::BudgetAdjusted {
            from: 50_000,
            to: 15_999
        }]
    );

    req.model.spec.thinking = ThinkingConfig::Enabled { budget_tokens: 10 };
    let (body, _) = build_for(&req);
    assert_eq!(
        body.thinking,
        Some(crate::wire::WireThinking::Enabled {
            budget_tokens: 1024
        })
    );

    req.max_tokens = Some(1024);
    let (body, warnings) = build_for(&req);
    assert_eq!(body.thinking, None);
    assert_eq!(
        warnings,
        vec![RequestWarning::BudgetDropped { max_tokens: 1024 }]
    );
}

#[test]
fn cache_plan_caps_breakpoints_and_caches_the_conversation() {
    let mut req = request("claude-sonnet-5-5", ThinkingConfig::Adaptive);
    req.tools = vec![tool("grep").cached(), tool("read_file")];
    req.system = vec![
        SystemBlock::cached("You are crest, a coding agent."),
        SystemBlock::cached("Project conventions: ..."),
        SystemBlock::cached("Repository map: ..."),
        SystemBlock::new("Today is 2026-10-17."),
    ];
    let (body, warnings) = build_for(&req);
    insta::assert_json_snapshot!(body);
    // Tool marker + 3 system markers = 4 explicit; the earliest (tools) goes.
    assert_eq!(
        warnings,
        vec![RequestWarning::CacheBreakpointsDropped { count: 1 }]
    );
    assert_eq!(body.cache_control, Some(CacheControl::EPHEMERAL));
}

#[test]
fn no_automatic_cache_without_messages() {
    let mut req = request("claude-sonnet-5-5", ThinkingConfig::Adaptive);
    req.messages.clear();
    let (body, warnings) = build_for(&req);
    assert_eq!(body.cache_control, None);
    assert!(warnings.is_empty());
}
