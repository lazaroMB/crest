//! A tool written in Lua, called by the model through the agent loop.

#[cfg(test)]
mod common;

use application::ports::approval::ApprovalResponse;
use application::testing::llm::ScriptedResponse;
use common::Agent;
use domain::permission::ToolEffect;
use serde_json::json;

const INIT: &str = r#"
local app = require("app")

app.tools.register({
  name = "todo_list",
  description = "Shows the project's pending tasks",
  schema = { type = "object", properties = { items = { type = "array" } } },
  effect = "read",
  handler = function(input)
    local lines = {}
    for i, item in ipairs(input.items) do lines[i] = "- " .. item end
    return table.concat(lines, "\n")
  end,
})

app.tools.register({
  name = "todo_count",
  effect = "read",
  handler = function(input) return { count = #input.items } end,
})

app.tools.register({
  name = "todo_add",
  handler = function(input) return "added " .. input.item end,
})

app.tools.register({
  name = "broken",
  effect = "read",
  handler = function() error("kaput") end,
})

app.tools.register({
  name = "spin",
  effect = "read",
  handler = function() while true do end end,
})
"#;

#[tokio::test]
async fn the_model_calls_a_lua_tool_and_gets_what_it_returned() {
    let script = vec![
        ScriptedResponse::tool_use("toolu_1", "todo_list", json!({ "items": ["a", "b"] }))
            .then_tool_use("toolu_2", "todo_count", json!({ "items": ["a", "b", "c"] })),
        ScriptedResponse::text("Listo."),
    ];
    let mut agent = Agent::new(INIT, script, |_| {}).await;
    agent.run("¿qué falta?").await;

    assert_eq!(
        agent.tool_results(),
        [
            ("- a\n- b".to_owned(), false),
            (r#"{"count":3}"#.to_owned(), false)
        ]
    );
    // Read-only tools run without asking.
    assert!(agent.approval.requests().is_empty());
    let request = agent.llm.requests().remove(0);
    let offered: Vec<_> = request.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(offered.contains(&"todo_list"), "{offered:?}");
    let todo = request
        .tools
        .iter()
        .find(|t| t.name == "todo_list")
        .unwrap();
    assert_eq!(todo.description, "Shows the project's pending tasks");
    assert_eq!(todo.input_schema["properties"]["items"]["type"], "array");
}

#[tokio::test]
async fn an_undeclared_effect_asks_like_a_command() {
    let script = vec![
        ScriptedResponse::tool_use("toolu_1", "todo_add", json!({ "item": "c" })),
        ScriptedResponse::text("Listo."),
    ];
    let mut agent = Agent::new(INIT, script, |_| {}).await;
    agent.approval.push(ApprovalResponse::Approve);
    agent.run("agrega c").await;

    let requests = agent.approval.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].effect, ToolEffect::Execute);
    assert_eq!(agent.tool_results(), [("added c".to_owned(), false)]);
}

#[tokio::test]
async fn lua_errors_and_runaway_tools_are_error_results() {
    let script = vec![
        ScriptedResponse::tool_use("toolu_1", "broken", json!({}))
            .then_tool_use("toolu_2", "spin", json!({}))
            .then_tool_use("toolu_3", "todo_count", json!({ "items": [1] })),
        ScriptedResponse::text("Listo."),
    ];
    let mut agent = Agent::new(INIT, script, |_| {}).await;
    agent.run("prueba").await;

    let results = agent.tool_results();
    assert!(
        results[0].1 && results[0].0.contains("kaput"),
        "{results:?}"
    );
    assert!(
        results[1].1 && results[1].0.contains("instruction limit"),
        "{results:?}"
    );
    // The host keeps serving after both failures.
    assert_eq!(results[2], (r#"{"count":1}"#.to_owned(), false));
}
