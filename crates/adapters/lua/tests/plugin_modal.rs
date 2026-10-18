//! A plugin opens, updates and closes modals from a tool handler and a
//! command; the events reach the host's sink at once.

use std::sync::Arc;

use adapter_lua::{HostOptions, LuaHost};
use application::ports::events::{DomainEvent, EventSink};
use application::ports::plugin::{ConfigSources, PluginHostPort, PluginOutput};
use application::testing::system::RecordingSink;
use domain::block::CustomNode;
use domain::modal::ModalId;
use serde_json::{Value, json};

const INIT: &str = r#"
local app = require("app")

app.tools.register({
  name = "show_report",
  effect = "read",
  handler = function(input)
    app.modal.open{
      id = "report", title = "Report", width = "50%", position = "bottom-left",
      content = app.ui.col{ app.ui.csv(input.csv), app.ui.json({ ok = true }) },
    }
    return "shown"
  end,
})

app.commands.register("report-done", function()
  app.modal.update("report", app.ui.markdown("**done**"))
  app.modal.close("report")
end)
"#;

#[tokio::test]
async fn tool_handlers_and_commands_push_modal_events() {
    let dir = tempfile::tempdir().unwrap();
    let init = dir.path().join("init.lua");
    std::fs::write(&init, INIT).unwrap();
    let sink = Arc::new(RecordingSink::new());
    let host = LuaHost::spawn(HostOptions {
        instruction_limit: 1_000_000,
        events: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
        ..HostOptions::default()
    })
    .unwrap();
    let config = host
        .load_config(ConfigSources {
            global: Some(init),
            workspace: None,
        })
        .await
        .unwrap();
    assert!(config.errors.is_empty(), "{:?}", config.errors);

    let out = host
        .call_tool("show_report", json!({ "csv": "a,b\n1,2\n" }))
        .await
        .unwrap();
    assert_eq!(out, PluginOutput::Value(json!("shown")));
    let events = sink.take();
    let [DomainEvent::ModalOpened { spec }] = events.as_slice() else {
        panic!("{events:?}")
    };
    assert_eq!(spec.id, ModalId::new("config/report"));
    assert_eq!(spec.position.to_string(), "bottom-left");
    let CustomNode::Col { children } = &spec.content else {
        panic!("{:?}", spec.content)
    };
    assert!(matches!(children[0], CustomNode::Table(_)));
    assert!(matches!(children[1], CustomNode::Json { .. }));

    let command = config.commands[0].callback;
    host.invoke_callback(command, Value::Null).await.unwrap();
    assert_eq!(
        sink.kinds(),
        ["modal_updated", "modal_closed"],
        "the command updated and closed it"
    );
}
