//! `AGENTS.md` and `CLAUDE.md` through the real composition root: the
//! root's files reach the system prompt, a subdirectory's the first tool
//! result in its subtree, and an edited file the next request.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::sync::Arc;

use application::ports::plugin::ConfigSources;
use application::usecases::cancel::CancelToken;
use application_testing::llm::{ScriptedLlm, ScriptedResponse};
use crest::headless::run::frontend;
use crest::headless::{StdinApproval, Terminal, run_turn};
use crest::root::{Config, Overrides, build_app};
use domain::conversation::{ContentPart, ToolResultContent};
use domain::permission::PermissionMode;
use serde_json::json;

struct Sink;

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn project_files_reach_the_prompt_and_nested_ones_the_tool_results() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("pkg")).unwrap();
    std::fs::write(ws.join("AGENTS.md"), "Always run make test.").unwrap();
    std::fs::write(ws.join("CLAUDE.md"), "@AGENTS.md\n").unwrap();
    std::fs::write(ws.join("pkg/AGENTS.md"), "No unwrap in pkg.").unwrap();
    std::fs::write(ws.join("pkg/lib.rs"), "pub fn f() {}\n").unwrap();

    let llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedResponse::tool_use("toolu_1", "read_file", json!({ "path": "pkg/lib.rs" })),
        ScriptedResponse::text("Leído."),
        ScriptedResponse::text("Otra."),
    ]));
    let terminal = Terminal::new(Box::new(Sink), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let config = Config {
        workspace: ws.clone(),
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
        session_id: None,
        project: None,
    };
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources::default()),
        db_path: Some(dir.path().join("sessions.db")),
        ..Overrides::default()
    };
    let mut app = build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap();
    run_turn(&mut app, "lee pkg/lib.rs", CancelToken::new()).await;

    let requests = llm.requests();
    let system: String = requests[0].system.iter().map(|b| b.text.clone()).collect();
    assert!(system.starts_with("You are crest"), "{system}");
    assert!(system.contains("# Project instructions"), "{system}");
    assert!(
        system.contains("## AGENTS.md\nAlways run make test."),
        "{system}"
    );
    assert!(
        !system.contains("## CLAUDE.md"),
        "folded into AGENTS.md: {system}"
    );
    assert!(!system.contains("No unwrap"), "nested files wait: {system}");

    let result: Vec<String> = requests[1]
        .messages
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|part| match part {
            ContentPart::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|item| match item {
            ToolResultContent::Text { text } => Some(text),
            ToolResultContent::Image { .. } => None,
        })
        .collect();
    assert_eq!(result.len(), 2, "{result:?}");
    assert!(result[1].contains("<instructions path=\"pkg/AGENTS.md\">\nNo unwrap in pkg."));

    // An edit (as `/init` makes) is in the next request.
    std::fs::write(ws.join("AGENTS.md"), "Always run cargo test, twice over.").unwrap();
    run_turn(&mut app, "otra", CancelToken::new()).await;
    let system: String = llm.requests()[2]
        .system
        .iter()
        .map(|b| b.text.clone())
        .collect();
    assert!(
        system.contains("Always run cargo test, twice over."),
        "{system}"
    );
}
