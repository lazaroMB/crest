//! Background agents through the real composition root: `/agent` starts
//! one without blocking the chat, `/agents` lists it, and its report
//! reaches the chat's agent in the turn the window starts for it.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::sync::Arc;
use std::time::Duration;

use application::ports::plugin::ConfigSources;
use application::usecases::cancel::CancelToken;
use application::usecases::commands::Command;
use application_testing::llm::{ScriptedLlm, ScriptedResponse};
use crest::gui::UiController;
use crest::headless::run::frontend;
use crest::headless::{StdinApproval, Terminal};
use crest::root::{Config, Overrides, build_app};
use domain::block::{AgentStatus, BlockKind};
use domain::conversation::ContentPart;
use domain::permission::PermissionMode;
use ui::session::{Reply, SessionController as _, TurnEnd};

struct Sink;

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_works_in_the_background_and_its_report_wakes_the_chat() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn check() {}\n").unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        // The explore agent's answer, then the chat's in the wake turn.
        ScriptedResponse::text("`check` is defined in src/lib.rs:1."),
        ScriptedResponse::text("The agent found it: src/lib.rs:1."),
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
    let app = build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap();
    let agents = Arc::clone(&app.agents);
    assert!(
        !agents.background_available(),
        "headless runs have no background"
    );
    let controller = UiController::new(app, tokio::runtime::Handle::current());
    assert!(agents.background_available());

    let reply = controller
        .run_command(Command::parse("agent explore where is check defined?"))
        .unwrap()
        .await;
    assert_eq!(reply, Reply::Done);

    for _ in 0..500 {
        if !agents.inbox().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let list = agents.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].status, AgentStatus::Done);
    assert_eq!(list[0].name, "where is check defined?");

    let Reply::Block(block) = controller
        .run_command(Command::parse("agents"))
        .unwrap()
        .await
    else {
        panic!("a list");
    };
    let BlockKind::Markdown { source, .. } = &block.kind else {
        panic!("{block:?}");
    };
    assert!(
        source.contains("| 1 | where is check defined? | explore | you | done |"),
        "{source}"
    );
    let reply = controller
        .run_command(Command::parse("agents stop 1"))
        .unwrap()
        .await;
    assert!(matches!(reply, Reply::Notice(..)), "{reply:?}");

    let end = controller.wake(CancelToken::new()).await;
    assert!(matches!(end, TurnEnd::Finished(_)), "{end:?}");
    let requests = llm.requests();
    assert_eq!(requests.len(), 2);
    let last = requests[1].messages.last().unwrap();
    let text: String = last
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("<agent-report id=\"1\""), "{text}");
    assert!(text.contains("src/lib.rs:1"), "{text}");
    // Nothing left: a second wake runs no turn.
    let end = controller.wake(CancelToken::new()).await;
    assert!(matches!(end, TurnEnd::Finished(_)));
    assert_eq!(llm.requests().len(), 2);
}
