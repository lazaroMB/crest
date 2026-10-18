//! `before_tool` and `after_tool` hooks written in Lua, run by the agent
//! loop around a built-in tool.

#[cfg(test)]
mod common;

use application::ports::approval::ApprovalResponse;
use application::testing::llm::ScriptedResponse;
use application::tools::registry::{PreparedCall, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use common::Agent;
use domain::permission::ToolEffect;
use serde_json::{Value, json};

/// Behaves like `run_command` without running anything.
struct FakeRunCommand;

#[async_trait]
impl Tool for FakeRunCommand {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Runs a command."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "command": { "type": "string" } } })
    }

    fn effect(&self, _input: &Value) -> ToolEffect {
        ToolEffect::Execute
    }

    async fn prepare(&self, input: Value, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let command = input["command"].as_str().unwrap_or_default().to_owned();
        let run = Box::pin(async move { Ok(ToolOutput::text(format!("ran {command}"))) });
        Ok(PreparedCall::new(input.to_string(), run))
    }
}

const INIT: &str = r#"
local app = require("app")

app.on("before_tool", function(ev)
  if ev.name == "run_command" and ev.input.command == "rm" then return { deny = true } end
end)

app.on("before_tool", function(ev)
  if ev.name == "run_command" and ev.input.command == "git push" then
    return { input = { command = "git push --dry-run" } }
  end
end)

local seen = {}
app.on("after_tool", function(ev)
  seen[#seen + 1] = ev.name .. ": " .. ev.output.text
end)

app.tools.register({
  name = "seen",
  effect = "read",
  handler = function() return table.concat(seen, "\n") end,
})
"#;

fn command(id: &str, command: &str) -> ScriptedResponse {
    ScriptedResponse::tool_use(id, "run_command", json!({ "command": command }))
}

#[cfg(test)]
async fn agent(script: Vec<ScriptedResponse>) -> Agent {
    Agent::new(INIT, script, |tools| {
        tools.register(std::sync::Arc::new(FakeRunCommand)).unwrap();
    })
    .await
}

#[tokio::test]
async fn the_plan_hook_denies_rm_without_asking() {
    let script = vec![command("toolu_1", "rm"), ScriptedResponse::text("Vale.")];
    let mut agent = agent(script).await;
    assert_eq!(agent.config.hooks.len(), 3);
    agent.run("borra todo").await;

    assert_eq!(agent.tool_results(), [("denied by hook".to_owned(), true)]);
    assert!(agent.approval.requests().is_empty());
}

#[tokio::test]
async fn a_hook_rewrites_the_input_before_approval() {
    let script = vec![
        command("toolu_1", "git push"),
        ScriptedResponse::text("Listo."),
    ];
    let mut agent = agent(script).await;
    agent.approval.push(ApprovalResponse::Approve);
    agent.run("publica").await;

    assert_eq!(
        agent.tool_results(),
        [("ran git push --dry-run".to_owned(), false)]
    );
    // The user approves the rewritten call, not the original one.
    let requests = agent.approval.requests();
    assert_eq!(requests[0].subject, r#"{"command":"git push --dry-run"}"#);
}

#[tokio::test]
async fn after_tool_hooks_see_each_output() {
    let script = vec![
        command("toolu_1", "ls"),
        ScriptedResponse::tool_use("toolu_2", "seen", json!({})),
        ScriptedResponse::text("Listo."),
    ];
    let mut agent = agent(script).await;
    agent.approval.push(ApprovalResponse::Approve);
    agent.run("lista").await;

    let results = agent.tool_results();
    assert_eq!(results[0], ("ran ls".to_owned(), false));
    // `seen` itself is reported once it has run, so it sees only `ls`.
    assert_eq!(results[1], ("run_command: ran ls".to_owned(), false));
}
