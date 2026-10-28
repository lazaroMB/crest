//! The help modal's snapshot through the real composition root: a plugin
//! with a tool, a described command, a described keymap and a capability,
//! the example MCP echo server and a held-back workspace server.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use application::ports::plugin::ConfigSources;
use application::testing::llm::ScriptedLlm;
use crest::gui::UiController;
use crest::headless::run::frontend;
use crest::headless::{StdinApproval, Terminal};
use crest::root::{Config, Overrides, build_app};
use domain::permission::PermissionMode;
use std::sync::Arc;
use ui::help::{HelpSnapshot, ToolSource};
use ui::session::SessionController as _;

struct Sink;

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The `echo_server` example of adapter-mcp (see `tests/mcp_servers.rs`).
fn echo_server_path() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.parent().and_then(Path::parent).unwrap();
        let path = profile_dir
            .join("examples")
            .join(format!("echo_server{}", std::env::consts::EXE_SUFFIX));
        if !path.exists() {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let mut build = Command::new(cargo);
            build.args(["build", "-p", "adapter-mcp", "--example", "echo_server"]);
            if profile_dir.file_name().is_some_and(|n| n == "release") {
                build.arg("--release");
            }
            assert!(
                build.status().unwrap().success(),
                "cannot build echo_server"
            );
        }
        path
    })
    .clone()
}

const JIRA: &str = r#"app.plugin{ name = "jira", capabilities = { "network:jira.example.com" } }
app.tools.register({
  name = "jira_issue",
  description = "Fetches a Jira issue",
  effect = "read",
  schema = { type = "object", properties = { key = { type = "string" } } },
  handler = function(input) return input.key end,
})
app.commands.register("issue", function(args) end, { desc = "Show a Jira issue" })
app.keymap.set("<leader>j", function() app.cmd("issue") end, { desc = "Open the issue" })
"#;

async fn snapshot() -> HelpSnapshot {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join(".crest")).unwrap();
    let config_dir = dir.path().join("config");
    let plugin_dir = config_dir.join("plugins").join("jira");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("init.lua"), JIRA).unwrap();
    let global = format!(
        "app.mcp.add(\"echo\", {{ command = {:?} }})\napp.keymap.set(\"<leader>k\", \"block.last\")\n",
        echo_server_path().display().to_string()
    );
    std::fs::write(config_dir.join("init.lua"), global).unwrap();
    std::fs::write(
        ws.join(".crest").join("init.lua"),
        "app.mcp.add(\"wsgit\", { command = \"mcp-git-missing\" })\n",
    )
    .unwrap();

    let terminal = Terminal::new(Box::new(Sink), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let overrides = Overrides {
        llm: Some(Arc::new(ScriptedLlm::new(Vec::new()))),
        config_sources: Some(ConfigSources {
            global: Some(config_dir.join("init.lua")),
            workspace: Some(ws.join(".crest").join("init.lua")),
        }),
        db_path: Some(dir.path().join("sessions.db")),
        mcp_trust_path: Some(dir.path().join("data").join("mcp-trust.json")),
        plugin_trust_path: Some(dir.path().join("data").join("plugin-trust.json")),
        ..Overrides::default()
    };
    let config = Config {
        workspace: ws,
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
        session_id: None,
    };
    let app = build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap();
    let controller = UiController::new(app, tokio::runtime::Handle::current());
    let snapshot = controller.help().await;
    drop(dir);
    snapshot
}

#[tokio::test(flavor = "multi_thread")]
async fn the_help_lists_plugins_tools_commands_keys_and_servers() {
    let help = snapshot().await;

    let issue = help.commands.iter().find(|c| c.name == "issue").unwrap();
    assert_eq!(issue.description, "Show a Jira issue");
    assert_eq!(issue.source, "jira");
    for name in ["implement", "research", "simulate", "mcp", "plugin"] {
        assert!(help.commands.iter().any(|c| c.name == name), "{name}");
    }

    let leader_j = help.keys.iter().find(|k| k.keys == "<C-g>j").unwrap();
    assert_eq!(leader_j.description, "Open the issue");
    assert_eq!(leader_j.source, "jira");
    let j = help.keys.iter().find(|k| k.keys == "<C-g>k").unwrap();
    assert!(j.overridden, "{j:?}");
    assert_eq!(
        (j.action.as_str(), j.source.as_str()),
        ("block.last", "config")
    );

    let tool = |name: &str| help.tools.iter().find(|t| t.name == name).unwrap();
    let jira = tool("jira_issue");
    assert_eq!(jira.source, ToolSource::Plugin("jira".into()));
    assert_eq!(jira.effect, "read");
    assert!(jira.schema.contains("\"key\""), "{}", jira.schema);
    assert_eq!(
        tool("mcp__echo__add").source,
        ToolSource::Mcp("echo".into())
    );
    assert_eq!(tool("mcp__echo__add").effect, "execute");
    assert_eq!(tool("read_file").source, ToolSource::Builtin);
    assert_eq!(tool("run_command").effect, "depends on input");
    let first_mcp = help
        .tools
        .iter()
        .position(|t| matches!(t.source, ToolSource::Mcp(_)))
        .unwrap();
    assert!(
        help.tools[..first_mcp]
            .iter()
            .all(|t| !matches!(t.source, ToolSource::Mcp(_))),
        "grouped"
    );

    let plugin = help.plugins.iter().find(|p| p.name == "jira").unwrap();
    assert_eq!(plugin.trust, "pending");
    assert_eq!(plugin.approve.as_deref(), Some("/plugin trust jira"));
    assert_eq!(plugin.capabilities, ["network:jira.example.com"]);
    assert_eq!(plugin.layer, "global");
    assert!(plugin.path.ends_with("init.lua"), "{plugin:?}");
    for add in ["tool jira_issue", "command :issue", "keymap <leader>j"] {
        assert!(plugin.adds.iter().any(|a| a == add), "{add}: {plugin:?}");
    }
    assert_eq!(
        help.plugins.first().map(|p| p.name.as_str()),
        Some("config")
    );

    let server = |name: &str| help.mcp.iter().find(|m| m.name == name).unwrap();
    let echo = server("echo");
    assert!(echo.state.starts_with("ready ("), "{echo:?}");
    assert_eq!(echo.origin, "global");
    assert!(echo.transport.starts_with("stdio: "), "{echo:?}");
    let wsgit = server("wsgit");
    assert_eq!(wsgit.origin, "workspace");
    assert_eq!(wsgit.trust_command.as_deref(), Some("/mcp trust wsgit"));
    assert!(wsgit.state.contains("held back"), "{wsgit:?}");

    assert!(
        help.models
            .iter()
            .any(|m| m.kind == "role" && m.name == "main")
    );
    assert!(help.cli.iter().any(|c| c.flag.starts_with("--trust-mcp")));
    assert!(help.paths.iter().any(|p| p.label == "global config"));
    assert!(
        help.diagnostics.iter().any(|d| d.contains("jira")),
        "the pending plugin warns: {:?}",
        help.diagnostics
    );
}
