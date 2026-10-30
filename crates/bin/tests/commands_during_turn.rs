//! `/open`, `/!cmd` and the help answer while a turn runs (and holds the
//! [`App`](crest::root::App)) instead of waiting for it to end; the output
//! of the `/!` goes to the prompt after the turn.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::sync::Arc;
use std::time::Duration;

use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::usecases::cancel::CancelToken;
use crest::gui::UiController;
use crest::headless::run::frontend;
use crest::headless::{StdinApproval, Terminal};
use crest::root::{Config, Overrides, build_app};
use domain::conversation::StopReason;
use domain::permission::PermissionMode;
use ui::session::{Reply, SessionController as _, TurnEnd, TurnRequest};

struct Sink;

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Long enough for a loaded CI, far shorter than a turn that never ends.
const PROMPTLY: Duration = Duration::from_secs(10);

fn request(prompt: &str, cancel: CancelToken) -> TurnRequest {
    TurnRequest {
        prompt: prompt.into(),
        images: Vec::new(),
        model_override: None,
        role: None,
        cancel,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_shell_and_help_do_not_wait_for_the_running_turn() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("README.md"), "# crest\n").unwrap();

    // The first turn thinks forever; the second one answers.
    let llm = Arc::new(ScriptedLlm::new([
        ScriptedResponse::text("pensando").hang_after(1),
        ScriptedResponse::text("listo"),
    ]));
    let terminal = Terminal::new(Box::new(Sink), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let overrides = Overrides {
        llm: Some(llm.clone()),
        db_path: Some(dir.path().join("sessions.db")),
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

    let cancel = CancelToken::new();
    let turn = tokio::spawn(controller.run_turn(request("piensa", cancel.clone())));
    tokio::time::timeout(PROMPTLY, async {
        while llm.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the turn reaches the model");

    let open = controller.run_command("open", "@README.md").unwrap();
    let open = tokio::time::timeout(PROMPTLY, open).await;
    assert_eq!(open.expect("/open waited for the turn"), Reply::Done);

    let shell = controller.run_command("!", "echo desde-shell").unwrap();
    let shell = tokio::time::timeout(PROMPTLY, shell).await;
    assert_eq!(shell.expect("/! waited for the turn"), Reply::Done);

    let help = tokio::time::timeout(PROMPTLY, controller.help()).await;
    let help = help.expect("the help waited for the turn");
    assert!(help.commands.iter().any(|c| c.name == "open"), "{help:?}");

    cancel.cancel();
    let end = tokio::time::timeout(PROMPTLY, turn).await.unwrap().unwrap();
    assert_eq!(end, TurnEnd::Finished(StopReason::Interrupted));

    // The output of the `/!` run meanwhile goes before the next prompt.
    let next = controller.run_turn(request("sigue", CancelToken::new()));
    let end = tokio::time::timeout(PROMPTLY, next).await.unwrap();
    assert_eq!(end, TurnEnd::Finished(StopReason::EndTurn));
    let sent = format!("{:?}", llm.last_request().unwrap().messages.last());
    assert!(sent.contains("desde-shell"), "{sent}");
    assert!(sent.contains("sigue"), "{sent}");
    drop(dir);
}
