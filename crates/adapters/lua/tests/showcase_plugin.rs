//! The demo plugin of the repository (`examples/plugins/showcase`) loads
//! without diagnostics, and its command, tool and keymap work.

use std::path::PathBuf;
use std::sync::Arc;

use adapter_lua::{HostOptions, LuaHost};
use application::ports::events::{DomainEvent, EventSink};
use application::ports::plugin::{ConfigSources, PluginAction, PluginHostPort, PluginOutput};
use application::testing::system::RecordingSink;
use domain::block::{BlockKind, CustomNode};
use serde_json::json;

fn plugin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/plugins/showcase/init.lua")
}

#[tokio::test]
async fn the_showcase_plugin_shows_every_kind_of_node() {
    let sink = Arc::new(RecordingSink::new());
    let host = LuaHost::spawn(HostOptions {
        events: Some(Arc::clone(&sink) as Arc<dyn EventSink>),
        ..HostOptions::default()
    })
    .unwrap();
    let config = host
        .load_config(ConfigSources {
            global: Some(plugin()),
            workspace: None,
        })
        .await
        .unwrap();
    assert!(config.errors.is_empty(), "{:?}", config.errors);
    assert_eq!(config.keymaps.len(), 1);
    assert_eq!(config.keymaps[0].lhs, "<leader>d");
    assert_eq!(config.tools[0].name, "demo_dashboard");
    assert_eq!(config.hooks.len(), 1);
    let demo = config
        .commands
        .iter()
        .find(|c| c.name == "demo")
        .unwrap()
        .callback;

    // The tour and every single section open a modal with valid content.
    for section in [
        "", "chart", "table", "diagram", "json", "diff", "code", "html", "stats",
    ] {
        host.invoke_callback(demo, json!(section)).await.unwrap();
        let events = sink.take();
        let [DomainEvent::ModalOpened { spec }] = events.as_slice() else {
            panic!("/demo {section}: {events:?}")
        };
        spec.content.validate().unwrap();
    }

    // `block` and the tool answer with the dashboard block.
    let is_dashboard = |kind: &BlockKind| matches!(kind, BlockKind::Custom { name, tree: CustomNode::Col { .. }, .. } if name == "dashboard");
    let PluginOutput::Block(kind) = host.invoke_callback(demo, json!("block")).await.unwrap()
    else {
        panic!("/demo block returned no block");
    };
    assert!(is_dashboard(&kind));
    let PluginOutput::Block(kind) = host.call_tool("demo_dashboard", json!({})).await.unwrap()
    else {
        panic!("the tool returned no block");
    };
    assert!(is_dashboard(&kind));

    // `ask` sends a prompt with the plan role.
    let PluginOutput::Actions(actions) = host.invoke_callback(demo, json!("ask")).await.unwrap()
    else {
        panic!("no actions");
    };
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PluginAction::Send { .. })),
        "{actions:?}"
    );
}
