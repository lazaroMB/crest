//! MCP servers declared in `init.lua`, through the real composition root:
//! the example echo server of adapter-mcp over stdio, with the model
//! replaced by a script.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use application::ports::mcp::McpServerState;
use application::ports::plugin::ConfigSources;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::usecases::cancel::CancelToken;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal, run_turn};
use crest::root::{App, Config, Overrides, build_app};
use domain::permission::PermissionMode;
use serde_json::json;

/// Captured terminal output.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The `echo_server` example of adapter-mcp, next to the test binaries
/// (`target/<profile>/examples/`). `cargo test --workspace` builds it; a
/// run limited to this crate builds it here.
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
            let status = build.status().unwrap();
            assert!(status.success(), "cannot build the echo_server example");
        }
        assert!(path.exists(), "{} is missing", path.display());
        path
    })
    .clone()
}

/// Lua string literal of `path`.
fn lua_string(path: &Path) -> String {
    format!("{:?}", path.display().to_string())
}

struct Fixture {
    _dir: tempfile::TempDir,
    llm: Arc<ScriptedLlm>,
    output: Buffer,
    app: App,
    trust_file: PathBuf,
}

/// Builds the app with `global` and `workspace` as the two `init.lua`
/// files, `answers` on the simulated stdin and `trust_mcp` on the command
/// line.
async fn fixture(
    global: &str,
    workspace: &str,
    answers: &str,
    trust_mcp: &[&str],
    script: Vec<ScriptedResponse>,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join(".crest")).unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let global_init = config_dir.join("init.lua");
    let ws_init = ws.join(".crest").join("init.lua");
    std::fs::write(&global_init, global).unwrap();
    std::fs::write(&ws_init, workspace).unwrap();
    let trust_file = dir.path().join("data").join("mcp-trust.json");

    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    let input = Box::new(Cursor::new(answers.as_bytes().to_vec()));
    let approval = StdinApproval::new(terminal.clone(), input, false);
    let llm = Arc::new(ScriptedLlm::new(script));
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources {
            global: Some(global_init),
            workspace: Some(ws_init),
        }),
        db_path: Some(dir.path().join("sessions.db")),
        mcp_trust_path: Some(trust_file.clone()),
        ..Overrides::default()
    };
    let config = Config {
        workspace: ws,
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: trust_mcp.iter().map(|s| (*s).to_owned()).collect(),
        resume: false,
    };
    let app = build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap();
    Fixture {
        _dir: dir,
        llm,
        output,
        app,
        trust_file,
    }
}

fn echo_decl() -> String {
    format!(
        "app.mcp.add(\"echo\", {{ command = {} }})\n",
        lua_string(&echo_server_path())
    )
}

fn add_script() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::tool_use("toolu_1", "mcp__echo__add", json!({ "a": 2, "b": 3 })),
        ScriptedResponse::text("2 + 3 = 5"),
    ]
}

fn tool_names(app: &App) -> Vec<String> {
    app.service.tools().names().map(str::to_owned).collect()
}

#[tokio::test]
async fn a_global_server_is_offered_and_used_after_approval() {
    let mut fx = fixture(&echo_decl(), "", "y\n", &[], add_script()).await;
    assert!(fx.app.warnings.is_empty(), "{:?}", fx.app.warnings);
    let names = tool_names(&fx.app);
    assert!(names.iter().any(|n| n == "mcp__echo__add"), "{names:?}");
    assert!(names.iter().any(|n| n == "mcp__echo__echo"), "{names:?}");

    let exit = run_turn(&mut fx.app, "suma 2 y 3", CancelToken::new()).await;
    let out = fx.output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    assert!(out.contains("Allow mcp__echo__add"), "{out}");
    assert!(out.contains("✓ mcp__echo__add"), "{out}");

    let requests = fx.llm.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].tools.iter().any(|t| t.name == "mcp__echo__add"),
        "the model is offered the MCP tool"
    );
    let result = format!("{:?}", requests[1].messages.last().unwrap());
    assert!(result.contains('5'), "{result}");
}

#[tokio::test]
async fn a_denied_call_does_not_reach_the_server() {
    let mut fx = fixture(&echo_decl(), "", "n\n", &[], add_script()).await;
    let exit = run_turn(&mut fx.app, "suma 2 y 3", CancelToken::new()).await;
    let out = fx.output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    assert!(out.contains("⊘ mcp__echo__add: denied by user"), "{out}");
}

#[tokio::test]
async fn a_read_server_runs_without_asking() {
    let decl = echo_decl().replace("})", ", effect = \"read\" })");
    // No answers on stdin: an approval prompt would fail the turn.
    let mut fx = fixture(&decl, "", "", &[], add_script()).await;
    let exit = run_turn(&mut fx.app, "suma 2 y 3", CancelToken::new()).await;
    let out = fx.output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    assert!(!out.contains("Allow"), "{out}");
    assert!(out.contains("✓ mcp__echo__add"), "{out}");
}

#[tokio::test]
async fn a_workspace_server_does_not_start_until_trusted() {
    let fx = fixture("", &echo_decl(), "", &[], Vec::new()).await;
    let names = tool_names(&fx.app);
    assert!(!names.iter().any(|n| n.starts_with("mcp__")), "{names:?}");
    assert!(fx.app.mcp.is_none());
    assert_eq!(fx.app.warnings.len(), 1, "{:?}", fx.app.warnings);
    let warning = &fx.app.warnings[0];
    assert!(
        warning.contains("workspace MCP server `echo` not started"),
        "{warning}"
    );
    assert!(warning.contains("crest --trust-mcp echo"), "{warning}");
    assert!(fx.output.text().contains("crest --trust-mcp echo"));
    assert!(!fx.trust_file.exists());
}

#[tokio::test]
async fn trusting_a_workspace_server_starts_it_and_is_remembered() {
    let fx = fixture("", &echo_decl(), "", &["echo"], Vec::new()).await;
    assert!(fx.app.warnings.is_empty(), "{:?}", fx.app.warnings);
    assert!(tool_names(&fx.app).iter().any(|n| n == "mcp__echo__add"));
    let saved = std::fs::read_to_string(&fx.trust_file).unwrap();
    assert!(saved.contains("echo_server"), "{saved}");
}

#[tokio::test]
async fn a_failing_server_becomes_a_warning() {
    let decl = "app.mcp.add('gone', { command = '/nonexistent/crest-mcp-server' })";
    let fx = fixture(decl, "", "", &[], Vec::new()).await;
    assert_eq!(fx.app.warnings.len(), 1, "{:?}", fx.app.warnings);
    assert!(
        fx.app.warnings[0].contains("MCP server `gone` unavailable"),
        "{:?}",
        fx.app.warnings
    );
    let mcp = fx.app.mcp.as_ref().unwrap();
    assert!(matches!(
        mcp.statuses[0].state,
        McpServerState::Failed { .. }
    ));
}
