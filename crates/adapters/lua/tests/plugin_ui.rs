//! A Lua tool that answers with a `Custom` block built with `app.ui`.

#[cfg(test)]
mod common;

use application::ports::events::DomainEvent;
use application::testing::llm::ScriptedResponse;
use common::Agent;
use domain::block::{BlockKind, Cell, CustomNode};
use serde_json::json;

const INIT: &str = r#"
local app = require("app")

app.tools.register({
  name = "todo_list",
  description = "Shows the project's pending tasks",
  schema = { type = "object", properties = { items = { type = "array" } } },
  effect = "read",
  handler = function(input)
    return app.ui.block("todo", app.ui.list(input.items))
  end,
})

app.tools.register({
  name = "todo_table",
  effect = "read",
  handler = function(input)
    local rows = {}
    for i, item in ipairs(input.items) do rows[i] = { item, i % 2 == 0 } end
    return app.ui.block("todo", app.ui.table({ columns = { "tarea", "hecha" }, rows = rows }))
  end,
})

app.tools.register({
  name = "todo_broken",
  effect = "read",
  handler = function()
    return app.ui.block("todo", app.ui.table({ columns = {}, rows = {} }))
  end,
})
"#;

/// Trees of the `Custom` blocks the turn showed, in order.
fn custom_trees(agent: &Agent) -> Vec<CustomNode> {
    agent
        .sink
        .events()
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                match block.kind {
                    BlockKind::Custom { plugin, name, tree } => {
                        assert_eq!((plugin.as_str(), name.as_str()), ("config", "todo"));
                        Some(tree)
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn todo_list_shows_a_list_and_todo_table_a_table() {
    let script = vec![
        ScriptedResponse::tool_use("toolu_1", "todo_list", json!({ "items": ["a", "b"] }))
            .then_tool_use("toolu_2", "todo_table", json!({ "items": ["a", "b"] })),
        ScriptedResponse::text("Listo."),
    ];
    let mut agent = Agent::new(INIT, script, |_| {}).await;
    agent.run("¿qué falta?").await;

    let results = agent.tool_results();
    assert!(results.iter().all(|(_, is_error)| !is_error), "{results:?}");
    let trees = custom_trees(&agent);
    assert_eq!(trees.len(), 2, "{trees:?}");
    assert_eq!(
        trees[0],
        CustomNode::List {
            items: vec![
                CustomNode::Text { text: "a".into() },
                CustomNode::Text { text: "b".into() },
            ],
        }
    );
    let CustomNode::Table(table) = &trees[1] else {
        panic!("not a table: {:?}", trees[1]);
    };
    let columns: Vec<_> = table.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(columns, ["tarea", "hecha"]);
    assert_eq!(
        table.rows,
        [
            vec![Cell::Text("a".into()), Cell::Bool(false)],
            vec![Cell::Text("b".into()), Cell::Bool(true)],
        ]
    );
}

#[tokio::test]
async fn an_invalid_tree_fails_the_tool() {
    let script = vec![
        ScriptedResponse::tool_use("toolu_1", "todo_broken", json!({})),
        ScriptedResponse::text("Listo."),
    ];
    let mut agent = Agent::new(INIT, script, |_| {}).await;
    agent.run("prueba").await;

    let results = agent.tool_results();
    assert!(
        results[0].1 && results[0].0.contains("table has no columns"),
        "{results:?}"
    );
    assert!(custom_trees(&agent).is_empty());
}
