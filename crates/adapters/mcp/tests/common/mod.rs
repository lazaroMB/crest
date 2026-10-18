//! Helpers shared by the integration tests: the example echo server.

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;

use application::ports::plugin::{McpServerSpec, McpTransport};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Path of the `echo_server` example, which `cargo test` builds next to the
/// test binaries (`target/<profile>/examples/`).
pub fn echo_server_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(|deps| deps.parent()).unwrap();
    let path = profile_dir
        .join("examples")
        .join(format!("echo_server{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "{} is missing; run the tests with `cargo test -p adapter-mcp` so the example is built",
        path.display()
    );
    path
}

/// A stdio spec running the example echo server as `name`.
pub fn echo_spec(name: &str) -> McpServerSpec {
    McpServerSpec {
        name: name.into(),
        transport: McpTransport::Stdio {
            command: echo_server_path().display().to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
    }
}

/// Starts the example server on a free local port; returns it with its URL.
pub async fn http_server() -> (Child, String) {
    let mut child = Command::new(echo_server_path())
        .args(["--http", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut url = String::new();
    BufReader::new(stdout).read_line(&mut url).await.unwrap();
    (child, url.trim().to_owned())
}
