//! A real Lua host loaded from a temporary `init.lua`, wired into the agent
//! loop with a scripted model.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use adapter_lua::{HostOptions, LuaHandle, LuaHost};
use application::ports::plugin::{ConfigSources, LoadedConfig, PluginHostPort};
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
use application::tools::plugin::PluginTool;
use application::tools::registry::ToolRegistry;
use application::usecases::run_turn::{AgentDeps, AgentService, TurnOptions};
use domain::conversation::{ContentPart, Session, ToolResultContent};
use domain::model::ModelPolicy;
use domain::permission::{PermissionMode, PermissionPolicy};

pub struct Agent {
    pub host: LuaHandle,
    pub config: LoadedConfig,
    pub llm: Arc<ScriptedLlm>,
    pub sink: Arc<RecordingSink>,
    pub approval: Arc<ScriptedApproval>,
    pub service: AgentService,
    pub session: Session,
    _dir: tempfile::TempDir,
}

impl Agent {
    /// Loads `init_lua` as the global config, offers its tools (and the
    /// ones `register` adds) to the model, runs its hooks around every
    /// call and answers with `script`.
    pub async fn new(
        init_lua: &str,
        script: Vec<ScriptedResponse>,
        register: impl FnOnce(&mut ToolRegistry),
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let init = dir.path().join("init.lua");
        std::fs::write(&init, init_lua).unwrap();
        let host = LuaHost::spawn(HostOptions {
            instruction_limit: 1_000_000,
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

        let port: Arc<dyn PluginHostPort> = Arc::new(host.clone());
        let mut tools = ToolRegistry::new();
        for spec in &config.tools {
            tools
                .register(Arc::new(PluginTool::new(spec.clone(), Arc::clone(&port))))
                .unwrap();
        }
        register(&mut tools);

        let llm = Arc::new(ScriptedLlm::new(script));
        let sink = Arc::new(RecordingSink::new());
        let approval = Arc::new(ScriptedApproval::new());
        let deps = AgentDeps {
            llm: llm.clone(),
            events: sink.clone(),
            approval: approval.clone(),
            telemetry: Arc::new(RecordingTelemetry::new()),
        };
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
        let service = AgentService::new(deps, tools, ModelPolicy::default(), permissions)
            .with_plugin_host(port);
        Self {
            host,
            config,
            llm,
            sink,
            approval,
            service,
            session: Session::new("/ws", 0),
            _dir: dir,
        }
    }

    pub async fn run(&mut self, input: &str) {
        self.service
            .run_turn(&mut self.session, input, TurnOptions::default())
            .await
            .unwrap();
    }

    /// Every `tool_result` of the last turn, as `(text, is_error)`.
    pub fn tool_results(&self) -> Vec<(String, bool)> {
        let turn = self.session.turns.last().unwrap();
        turn.messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|part| match part {
                ContentPart::ToolResult {
                    content, is_error, ..
                } => {
                    let text = content
                        .iter()
                        .filter_map(|c| match c {
                            ToolResultContent::Text { text } => Some(text.as_str()),
                            ToolResultContent::Image { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some((text, *is_error))
                }
                _ => None,
            })
            .collect()
    }
}
