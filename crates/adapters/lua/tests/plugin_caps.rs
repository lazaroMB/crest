//! Plugin capabilities end to end in a real Lua host: `app.http` against a
//! local server, `app.shell.run` with real programs and `app.secret` from
//! the environment, before and after the declaration is granted.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use adapter_lua::{HostOptions, LuaHandle, LuaHost};
use application::ports::plugin::{
    ConfigOrigin, ConfigSources, LoadedConfig, PluginError, PluginHostPort, PluginOutput,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A config home with one plugin (`jira`) and a workspace.
struct Fixture {
    _dir: tempfile::TempDir,
    config: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new(plugin: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let plugin_dir = config.join("plugins").join("jira");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("init.lua"), plugin).unwrap();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(workspace.join("sub")).unwrap();
        let workspace = dunce::canonicalize(&workspace).unwrap();
        Self {
            _dir: dir,
            config,
            workspace,
        }
    }

    async fn load(&self) -> (LuaHandle, LoadedConfig) {
        let host = LuaHost::spawn(HostOptions {
            workspace: Some(self.workspace.clone()),
            keyring: false,
            environment: Some(vec![
                ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
                ("ANTHROPIC_API_KEY".into(), "sk-ant-secret".into()),
                ("CREST_PLUGIN_JIRA_TOKEN".into(), "jira-token".into()),
                ("CREST_PROBE".into(), "visible".into()),
            ]),
            ..HostOptions::default()
        })
        .unwrap();
        let config = host
            .load_config(ConfigSources {
                global: Some(self.config.join("init.lua")),
                workspace: None,
            })
            .await
            .unwrap();
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        (host, config)
    }

    /// Loads and grants every declaration.
    async fn granted(&self) -> LuaHandle {
        let (host, config) = self.load().await;
        host.grant_capabilities(config.plugin_capabilities)
            .await
            .unwrap();
        host
    }
}

/// A plugin with a `run` tool that evaluates `input.code` inside its own
/// file (so it is the plugin's code calling the APIs).
fn plugin(capabilities: &[String]) -> String {
    let list = capabilities
        .iter()
        .map(|c| format!("{c:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"app.plugin{{ name = "jira", capabilities = {{ {list} }} }}
app.tools.register({{ name = "run", handler = function(input)
  -- Run the snippet as code of this file: wrap it in a function defined here.
  local steps = {{
    get = function(url, opts) return app.http.get(url, opts) end,
    request = function(opts) return app.http.request(opts) end,
    shell = function(p, a, o) return app.shell.run(p, a, o) end,
    secret = function(n) return app.secret(n) end,
  }}
  local f = steps[input.step]
  return f(table.unpack(input.args or {{}}))
end }})
"#
    )
}

async fn call(host: &LuaHandle, step: &str, args: Value) -> Result<Value, PluginError> {
    match host
        .call_tool("run", json!({ "step": step, "args": args }))
        .await?
    {
        PluginOutput::Value(value) => Ok(value),
        other => panic!("unexpected output {other:?}"),
    }
}

fn err_text(result: Result<Value, PluginError>) -> String {
    match result {
        Err(err) => err.to_string(),
        Ok(value) => panic!("expected an error, got {value}"),
    }
}

fn local_cap(server: &MockServer) -> String {
    format!("network:http://127.0.0.1:{}", server.address().port())
}

// Runs POSIX tools (`echo`, `cat`, `env`, `sleep`).
#[cfg(unix)]
#[tokio::test]
async fn the_declaration_is_reported_and_inactive_until_granted() {
    let fx = Fixture::new(&plugin(&[
        "shell:echo".into(),
        "network:api.example.com".into(),
    ]));
    let (host, config) = fx.load().await;
    let decl = &config.plugin_capabilities[0];
    assert_eq!(decl.plugin, "jira");
    assert_eq!(decl.origin, ConfigOrigin::Global);
    assert!(decl.source.ends_with("plugins/jira/init.lua"));
    assert_eq!(decl.summary(), "network: api.example.com; shell: echo");

    let denied = err_text(call(&host, "shell", json!(["echo", ["hi"]])).await);
    assert!(denied.contains("are not approved"), "{denied}");
    assert!(denied.contains("crest --trust-plugin jira"), "{denied}");
    assert!(denied.contains("/plugin trust jira"), "{denied}");

    host.grant_capabilities(config.plugin_capabilities.clone())
        .await
        .unwrap();
    let out = call(&host, "shell", json!(["echo", ["hi"]])).await.unwrap();
    assert_eq!(out["stdout"], "hi\n");

    // A different list is a different declaration: not granted.
    let mut changed = config.plugin_capabilities[0].clone();
    changed.capabilities.pop();
    host.grant_capabilities(vec![changed]).await.unwrap();
    let again = err_text(call(&host, "shell", json!(["echo", ["hi"]])).await);
    assert!(again.contains("are not approved"), "{again}");
}

#[tokio::test]
async fn http_reaches_a_granted_host() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issue/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "key": "CR-1" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/echo"))
        .respond_with(|req: &wiremock::Request| {
            ResponseTemplate::new(201).set_body_bytes(req.body.clone())
        })
        .mount(&server)
        .await;
    let fx = Fixture::new(&plugin(&[local_cap(&server)]));
    let host = fx.granted().await;

    let url = format!("{}/issue/1", server.uri());
    let out = call(&host, "get", json!([url])).await.unwrap();
    assert_eq!(out["status"], 200);
    assert_eq!(out["json"], json!({ "key": "CR-1" }));
    assert_eq!(out["headers"]["content-type"], "application/json");

    let out = call(
        &host,
        "request",
        json!([{ "method": "post", "url": format!("{}/echo", server.uri()), "json": { "a": 1 } }]),
    )
    .await
    .unwrap();
    assert_eq!(out["status"], 201);
    assert_eq!(out["body"], r#"{"a":1}"#);
}

#[tokio::test]
async fn http_refuses_other_hosts_and_redirects_to_them() {
    let server = MockServer::start().await;
    let port = server.address().port();
    Mock::given(path("/away"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("http://localhost:{port}/x").as_str()),
        )
        .mount(&server)
        .await;
    let fx = Fixture::new(&plugin(&[local_cap(&server)]));
    let host = fx.granted().await;

    let other = err_text(call(&host, "get", json!([format!("http://localhost:{port}/x")])).await);
    assert!(
        other.contains(&format!("did not declare network:http://localhost:{port}")),
        "{other}"
    );
    let https = err_text(call(&host, "get", json!(["https://example.com/"])).await);
    assert!(
        https.contains("did not declare network:example.com"),
        "{https}"
    );

    let redirect = err_text(call(&host, "get", json!([format!("{}/away", server.uri())])).await);
    assert!(redirect.contains("redirect from"), "{redirect}");
    assert!(redirect.contains("denied"), "{redirect}");
}

#[tokio::test]
async fn http_times_out() {
    let server = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let fx = Fixture::new(&plugin(&[local_cap(&server)]));
    let host = fx.granted().await;
    let start = Instant::now();
    let err = err_text(
        call(
            &host,
            "get",
            json!([format!("{}/slow", server.uri()), { "timeout": 0.3 }]),
        )
        .await,
    );
    assert!(err.contains("timed out"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[cfg(unix)]
fn shell_plugin() -> String {
    plugin(&[
        "shell:cat".into(),
        "shell:env".into(),
        "shell:pwd".into(),
        "shell:sleep".into(),
    ])
}

// Runs POSIX tools (`echo`, `cat`, `env`, `sleep`).
#[cfg(unix)]
#[tokio::test]
async fn shell_runs_granted_programs_in_the_workspace() {
    let fx = Fixture::new(&shell_plugin());
    let host = fx.granted().await;

    let out = call(&host, "shell", json!(["cat", [], { "stdin": "piped" }]))
        .await
        .unwrap();
    assert_eq!(out["stdout"], "piped");
    assert_eq!(out["status"], 0);
    assert_eq!(out["timed_out"], false);

    let pwd = call(&host, "shell", json!(["pwd"])).await.unwrap();
    assert_eq!(
        pwd["stdout"].as_str().unwrap().trim_end(),
        path_str(&fx.workspace)
    );
    let sub = call(&host, "shell", json!(["pwd", [], { "cwd": "sub" }]))
        .await
        .unwrap();
    assert_eq!(
        sub["stdout"].as_str().unwrap().trim_end(),
        path_str(&fx.workspace.join("sub"))
    );

    let escape = err_text(call(&host, "shell", json!(["pwd", [], { "cwd": ".." }])).await);
    assert!(escape.contains("outside the workspace"), "{escape}");

    let denied = err_text(call(&host, "shell", json!(["ls"])).await);
    assert!(denied.contains("did not declare shell:ls"), "{denied}");
}

#[cfg(unix)]
fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

// Runs POSIX tools (`echo`, `cat`, `env`, `sleep`).
#[cfg(unix)]
#[tokio::test]
async fn shell_hides_credentials_from_programs() {
    let fx = Fixture::new(&shell_plugin());
    let host = fx.granted().await;
    let out = call(
        &host,
        "shell",
        json!(["env", [], { "env": { "EXTRA": "1" } }]),
    )
    .await
    .unwrap();
    let env = out["stdout"].as_str().unwrap();
    assert!(env.contains("CREST_PROBE=visible"), "{env}");
    assert!(env.contains("EXTRA=1"), "{env}");
    assert!(!env.contains("ANTHROPIC_API_KEY"), "{env}");
    assert!(!env.contains("CREST_PLUGIN_JIRA_TOKEN"), "{env}");
}

// Runs POSIX tools (`echo`, `cat`, `env`, `sleep`).
#[cfg(unix)]
#[tokio::test]
async fn shell_kills_a_program_on_timeout() {
    let fx = Fixture::new(&shell_plugin());
    let host = fx.granted().await;
    let start = Instant::now();
    let out = call(&host, "shell", json!(["sleep", ["30"], { "timeout": 0.3 }]))
        .await
        .unwrap();
    assert_eq!(out["timed_out"], true);
    assert_eq!(out.get("status"), None, "no exit status: {out}");
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn secrets_come_from_the_environment_once_granted() {
    let fx = Fixture::new(&shell_plugin());
    let (host, config) = fx.load().await;
    let denied = err_text(call(&host, "secret", json!(["token"])).await);
    assert!(denied.contains("are not approved"), "{denied}");
    host.grant_capabilities(config.plugin_capabilities)
        .await
        .unwrap();
    assert_eq!(
        call(&host, "secret", json!(["token"])).await.unwrap(),
        json!("jira-token")
    );
    assert_eq!(
        call(&host, "secret", json!(["missing"])).await.unwrap(),
        Value::Null
    );
}

#[tokio::test]
async fn capabilities_are_inactive_while_the_file_loads() {
    let fx = Fixture::new(
        r#"app.plugin{ capabilities = { "shell:echo" } }
           local ok, err = pcall(app.shell.run, "echo", { "x" })
           app.tools.register({ name = "run", handler = function() return tostring(err) end })"#,
    );
    let (host, _) = fx.load().await;
    let out = host.call_tool("run", json!({})).await.unwrap();
    let PluginOutput::Value(Value::String(err)) = out else {
        panic!("{out:?}")
    };
    assert!(err.contains("not active while init.lua loads"), "{err}");
}
