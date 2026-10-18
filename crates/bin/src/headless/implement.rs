//! `crest --headless --implement <ruta>`: the implementation flow of a
//! Spec in the terminal. The gates are answered on stdin, one line each,
//! on the same input as the approvals.

use application::flows::implement::flow::FlowReply;
use application::flows::implement::graph::status_notice;
use application::flows::implement::state::FlowStatus;
use application::ports::events::{DomainEvent, NoticeLevel};

use super::run::frontend;
use super::terminal::{Style, Terminal};
use super::{Exit, StdinApproval};
use crate::args::Args;
use crate::root::{App, Config, Overrides, build_app};

/// Runs the implementation flow of `target` (a Spec folder, its name or a
/// `plan.json`) until it stops, reading the answer to each gate from
/// `input`. When the input closes the flow stays at its gate, stored, and
/// the same command resumes it.
///
/// Exits with [`Exit::Success`] once the flow completes, with
/// [`Exit::TurnFailed`] when it is aborted, goes back to the Spec, fails
/// or is left at a gate, and with [`Exit::Config`] when it cannot open.
pub async fn run_flow(app: &App, target: &str, input: &StdinApproval, terminal: &Terminal) -> Exit {
    let session = match app.open_implementation(target).await {
        Ok(session) => session,
        Err(err) => {
            terminal
                .lock()
                .line(Some(Style::Red), &format!("error: {err:#}"));
            return Exit::Config;
        }
    };
    if session.resumed() {
        app.emit(DomainEvent::Notice {
            level: NoticeLevel::Info,
            message: format!("implementation flow of {} resumed", session.plan_path()),
        });
    }
    let mut status = session.run().await;
    loop {
        let current = match status {
            Ok(current) => current,
            Err(err) => {
                terminal
                    .lock()
                    .line(Some(Style::Red), &format!("error: {err}"));
                return Exit::TurnFailed;
            }
        };
        match current {
            FlowStatus::Running { .. } => status = session.run().await,
            FlowStatus::WaitingForInput { .. } => {
                terminal.lock().styled(Style::Bold, "› ");
                let line = match input.read_line().await {
                    Ok(Some(line)) => line,
                    Ok(None) | Err(_) => {
                        terminal.lock().line(
                            Some(Style::Yellow),
                            "no answer (stdin closed): the flow stays at this gate; \
                             the same command resumes it",
                        );
                        return Exit::TurnFailed;
                    }
                };
                status = match session.handle_line(&line).await {
                    Ok(FlowReply::Status(next)) => Ok(next),
                    Ok(FlowReply::Position(position)) => {
                        terminal.lock().line(None, &position.to_string());
                        session.status().await
                    }
                    Err(err) => Err(err),
                };
            }
            finished => {
                if let Some((level, message)) = status_notice(&finished) {
                    app.emit(DomainEvent::Notice { level, message });
                }
                return if finished == FlowStatus::Completed {
                    Exit::Success
                } else {
                    Exit::TurnFailed
                };
            }
        }
    }
}

/// `crest --headless --implement <target>` on stdin and stdout.
pub async fn run(args: &Args, target: &str) -> Exit {
    let terminal = Terminal::stdout();
    let input = StdinApproval::stdin(terminal.clone(), args.yes);
    let app = async {
        let config = Config::from_args(args)?;
        build_app(
            &config,
            frontend(&terminal, input.clone()),
            Overrides::default(),
        )
        .await
    };
    match app.await {
        Ok(app) => run_flow(&app, target, &input, &terminal).await,
        Err(err) => {
            eprintln!("error: {err:#}");
            Exit::Config
        }
    }
}
