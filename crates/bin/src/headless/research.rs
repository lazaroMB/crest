//! `crest --headless --research "<problema>"`: the research flow in the
//! terminal, from the problem to the Spec folder. Each gate is answered on
//! stdin, one line each, on the same input as the approvals: "sí" moves on,
//! any other text redoes the stage, and `/stage`, `/back <nodo>`, `/skip`
//! and `/abort` work as in the chat. `resume` as the problem continues the
//! flow left halfway in the workspace.

use application::flows::research::gate::FlowStatus;
use application::ports::events::{DomainEvent, NoticeLevel};

use super::run::frontend;
use super::terminal::{Style, Terminal};
use super::{Exit, StdinApproval};
use crate::args::Args;
use crate::research::{self, ResearchCommand};
use crate::root::{App, Config, Overrides, build_app, now_ms};

/// Runs the research flow of `problem` (or resumes it, for `resume`) until
/// it ends, reading the answer to each gate from `input`. When the input
/// closes the flow stays at its gate, stored, and
/// `crest --headless --research resume` continues it.
///
/// Exits with [`Exit::Success`] once the flow completes, with
/// [`Exit::TurnFailed`] when it is aborted, fails or is left at a gate, and
/// with [`Exit::Config`] when there is no flow store.
pub async fn run_flow(
    app: &mut App,
    problem: &str,
    input: &StdinApproval,
    terminal: &Terminal,
) -> Exit {
    let Some(mut session) = app.research.take() else {
        terminal.lock().line(
            Some(Style::Red),
            "error: el flujo investigativo no está disponible (la base de datos de crest no abrió)",
        );
        return Exit::Config;
    };
    let mut result =
        research::command(&mut session, ResearchCommand::parse(problem), now_ms()).await;
    let exit = loop {
        match result {
            Ok(message) => app.emit(DomainEvent::Notice {
                level: NoticeLevel::Info,
                message,
            }),
            Err(message) => {
                terminal
                    .lock()
                    .line(Some(Style::Red), &format!("error: {message}"));
                if !session.is_active() {
                    break Exit::TurnFailed;
                }
            }
        }
        let Some(flow) = session.active() else {
            break match session.last_status().await {
                Ok(Some(FlowStatus::Completed)) => Exit::Success,
                _ => Exit::TurnFailed,
            };
        };
        if let Ok(position) = flow.position().await
            && position.status.is_running()
        {
            // A node failed: the flow stays at its last checkpoint.
            break Exit::TurnFailed;
        }
        terminal.lock().styled(Style::Bold, "› ");
        let line = match input.read_line().await {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => {
                terminal.lock().line(
                    Some(Style::Yellow),
                    "sin respuesta (stdin cerrado): el flujo queda en esta compuerta; \
                     `crest --headless --research resume` lo retoma",
                );
                break Exit::TurnFailed;
            }
        };
        result = research::line(&mut session, &line).await;
    };
    if let Err(err) = app.save().await {
        app.warn(format!("no pude guardar la sesión: {err}"));
    }
    app.research = Some(session);
    exit
}

/// `crest --headless --research <problem>` on stdin and stdout.
pub async fn run(args: &Args, problem: &str) -> Exit {
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
        Ok(mut app) => run_flow(&mut app, problem, &input, &terminal).await,
        Err(err) => {
            eprintln!("error: {err:#}");
            Exit::Config
        }
    }
}
