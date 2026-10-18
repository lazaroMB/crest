//! Helpers shared by the integration tests: the example echo server.

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use application::ports::plugin::{McpServerSpec, McpTransport};

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
