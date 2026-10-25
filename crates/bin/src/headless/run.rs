//! Running the headless turn and its exit code.

use std::sync::Arc;

use application::usecases::cancel::CancelToken;
use application::usecases::run_turn::TurnError;
use domain::conversation::StopReason;

use super::{HeadlessSink, StdinApproval, StdinForm, Terminal};
use crate::args::Args;
use crate::root::{App, Config, Frontend, Overrides, build_app};

/// How `crest --headless` ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The turn finished.
    Success,
    /// The turn failed (model error, iteration limit).
    TurnFailed,
    /// crest could not start (workspace, API key).
    Config,
    /// The user interrupted the turn.
    Interrupted,
}

impl Exit {
    /// The process exit code: 0, 1, 2 or 130.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::TurnFailed => 1,
            Self::Config => 2,
            Self::Interrupted => 130,
        }
    }
}

/// The terminal front end: events, approvals and forms over `terminal`.
/// Forms read through the approval's input; without a terminal on stdin
/// they are cancelled.
#[must_use]
pub fn frontend(terminal: &Terminal, approval: StdinApproval) -> Frontend {
    Frontend {
        events: Arc::new(HeadlessSink::new(terminal.clone())),
        form: Arc::new(StdinForm::stdin(terminal.clone(), approval.clone())),
        approval: Arc::new(approval),
    }
}

/// Runs one turn answering `prompt` and saves the session, including the
/// "allow always" rules granted during it.
pub async fn run_turn(app: &mut App, prompt: &str, cancel: CancelToken) -> Exit {
    let opts = app.turn_options.clone().with_cancel(cancel);
    let result = app.service.run_turn(&mut app.session, prompt, opts).await;
    if let Err(err) = app.save().await {
        app.warn(format!("could not save the session: {err}"));
    }
    match result {
        Ok(outcome) if outcome.stop_reason == StopReason::Interrupted => Exit::Interrupted,
        Ok(_) => Exit::Success,
        // Nothing to answer with: a configuration problem, like a missing key.
        Err(TurnError::NoModelSelected { .. }) => Exit::Config,
        Err(_) => Exit::TurnFailed,
    }
}

/// `crest --headless <prompt>` on stdin and stdout.
pub async fn run(args: &Args, prompt: &str, cancel: CancelToken) -> Exit {
    let terminal = Terminal::stdout();
    let approval = StdinApproval::stdin(terminal.clone(), args.yes);
    let app = async {
        let config = Config::from_args(args)?;
        build_app(&config, frontend(&terminal, approval), Overrides::default()).await
    };
    match app.await {
        Ok(mut app) => {
            let watcher = super::interrupt::watch_ctrl_c(cancel.clone(), terminal);
            let exit = run_turn(&mut app, prompt, cancel).await;
            watcher.abort();
            exit
        }
        Err(err) => {
            eprintln!("error: {err:#}");
            Exit::Config
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        let codes = [
            Exit::Success,
            Exit::TurnFailed,
            Exit::Config,
            Exit::Interrupted,
        ]
        .map(Exit::code);
        assert_eq!(codes, [0, 1, 2, 130]);
    }
}
