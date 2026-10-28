//! A plugin with capabilities through the real composition root: its tool
//! calls `app.http.get` against a local server once `--trust-plugin`
//! approved it, in a turn driven by a scripted model.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use application::ports::plugin::ConfigSources;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::usecases::cancel::CancelToken;
use crest::headless::run::frontend;
use crest::headless::{Exit, StdinApproval, Terminal, run_turn};
use crest::plugin_trust::PluginTrustStore;
use crest::root::{App, Config, Overrides, build_app};
use domain::permission::PermissionMode;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

struct Fixture {
    _dir: tempfile::TempDir,
    llm: Arc<ScriptedLlm>,
    output: Buffer,
    app: App,
    trust_file: PathBuf,
}

/// The `jira` plugin: a read-only tool that fetches an issue from `base`.
fn jira_plugin(server: &MockServer) -> String {
    format!(
        r#"app.plugin{{ name = "jira", capabilities = {{ "network:http://127.0.0.1:{port}" }} }}
app.tools.register({{
  name = "jira_issue",
  description = "Fetches a Jira issue",
  effect = "read",
  schema = {{ type = "object", properties = {{ key = {{ type = "string" }} }} }},
  handler = function(input)
    local res = app.http.get("{base}/issue/" .. input.key, {{ headers = {{ Accept = "application/json" }} }})
    if res.status ~= 200 then error("HTTP " .. res.status) end
    return res.json.key .. ": " .. res.json.summary
  end,
}})
"#,
        port = server.address().port(),
        base = server.uri(),
    )
}

async fn fixture(server: &MockServer, trust_plugins: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let config_dir = dir.path().join("config");
    let plugin_dir = config_dir.join("plugins").join("jira");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("init.lua"), jira_plugin(server)).unwrap();
    let trust_file = dir.path().join("data").join("plugin-trust.json");

    let output = Buffer::default();
    let terminal = Terminal::new(Box::new(output.clone()), false);
    // No answers: a read tool must not ask.
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedResponse::tool_use("toolu_1", "jira_issue", json!({ "key": "CR-7" })),
        ScriptedResponse::text("CR-7 is about the parser."),
    ]));
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources {
            global: Some(config_dir.join("init.lua")),
            workspace: None,
        }),
        db_path: Some(dir.path().join("sessions.db")),
        mcp_trust_path: Some(dir.path().join("data").join("mcp-trust.json")),
        plugin_trust_path: Some(trust_file.clone()),
        ..Overrides::default()
    };
    let config = Config {
        workspace: ws,
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: trust_plugins.iter().map(|s| (*s).to_owned()).collect(),
        resume: false,
        session_id: None,
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

async fn jira_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issue/CR-7"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "key": "CR-7", "summary": "Parser drops comments" })),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_trusted_plugin_tool_calls_the_api() {
    let server = jira_server().await;
    let mut fx = fixture(&server, &["jira"]).await;
    assert!(fx.app.warnings.is_empty(), "{:?}", fx.app.warnings);
    let store = PluginTrustStore::load(&fx.trust_file).unwrap();
    assert!(
        store.is_trusted(&fx.app.plugin_capabilities[0]),
        "the approval is saved"
    );

    let exit = run_turn(&mut fx.app, "what is CR-7?", CancelToken::new()).await;
    let out = fx.output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    let requests = fx.llm.requests();
    assert_eq!(requests.len(), 2);
    let result = format!("{:?}", requests[1].messages.last().unwrap());
    assert!(result.contains("CR-7: Parser drops comments"), "{result}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_unapproved_plugin_is_announced_and_its_calls_fail() {
    let server = jira_server().await;
    let mut fx = fixture(&server, &[]).await;
    let notice = format!(
        "plugin `jira` requests network: http://127.0.0.1:{} — approve with `crest \
         --trust-plugin jira` or `/plugin trust jira`",
        server.address().port()
    );
    assert_eq!(fx.app.warnings, [notice]);

    let exit = run_turn(&mut fx.app, "what is CR-7?", CancelToken::new()).await;
    let out = fx.output.text();
    assert_eq!(exit, Exit::Success, "{out}");
    let result = format!("{:?}", fx.llm.requests()[1].messages.last().unwrap());
    assert!(result.contains("are not approved"), "{result}");
    assert!(result.contains("crest --trust-plugin jira"), "{result}");
    assert!(server.received_requests().await.unwrap().is_empty());
}
