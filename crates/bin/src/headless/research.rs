//! `crest --headless --research "<problem>"`: the research flow in the
//! terminal, from the problem to the Spec folder. Each gate is answered on
//! stdin, one line each, on the same input as the approvals: "yes" moves on,
//! any other text redoes the stage, and `/stage`, `/back <stage>`, `/skip`
//! `/retry` and `/abort` work as in the chat. `resume` (or `retry`) as the
//! problem continues the flow left halfway in the workspace, also after a
//! failed stage. `/aside [question]` chats aside with the gate as context
//! until `/flow [answer]` (or "listo, continúa") goes back, as in the
//! window (see [`crate::aside`]); a side question is never guessed here.

use application::flows::aside::{last_answer, preamble};
use application::ports::events::{DomainEvent, NoticeLevel};

use super::run::frontend;
use super::terminal::{Style, Terminal};
use super::{Exit, StdinApproval};
use crate::args::Args;
use crate::aside::{Aside, Route, opened_notice};
use crate::discovery::Engine;
use crate::gui::{command_turn, run_turn_in};
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
            "error: the research flow is unavailable (the crest database did not open)",
        );
        return Exit::Config;
    };
    let mut aside = Aside::default();
    let mut result =
        Some(research::command(&mut session, ResearchCommand::parse(problem), now_ms()).await);
    let exit = loop {
        match result {
            Some(Ok(message)) => app.emit(DomainEvent::Notice {
                level: NoticeLevel::Info,
                message,
            }),
            Some(Err(message)) => {
                terminal
                    .lock()
                    .line(Some(Style::Red), &format!("error: {message}"));
            }
            None => {}
        }
        if !session.is_active() {
            break match session.finished().await {
                Some(true) => Exit::Success,
                Some(false) => {
                    // A failed or interrupted stage: the flow stays stored
                    // at its last checkpoint.
                    terminal
                        .lock()
                        .line(Some(Style::Yellow), open_hint(session.engine()));
                    Exit::TurnFailed
                }
                _ => Exit::TurnFailed,
            };
        }
        terminal.lock().styled(Style::Bold, "› ");
        let line = match input.read_line().await {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => {
                terminal.lock().line(
                    Some(Style::Yellow),
                    "no answer (stdin closed): the flow stays at this gate; \
                     `crest --headless --research resume` resumes it",
                );
                break Exit::TurnFailed;
            }
        };
        result = side_line(app, &mut session, &mut aside, &line).await;
    };
    if let Err(err) = app.save().await {
        app.warn(format!("could not save the session: {err}"));
    }
    app.research = Some(session);
    exit
}

/// A line read at a gate: `/aside`, `/flow`, a line of the side chat (a
/// chat turn, `None`) or the answer to the gate.
async fn side_line(
    app: &mut App,
    session: &mut crate::research::Research,
    aside: &mut Aside,
    line: &str,
) -> Option<Result<String, String>> {
    let gate = session.gate_info().await;
    let (command, args) = match line.trim().strip_prefix('/') {
        Some(rest) => rest.split_once(' ').unwrap_or((rest, "")),
        None => ("", ""),
    };
    let route = match (command, gate) {
        ("aside" | "aparte", Some(gate)) => {
            let notice = opened_notice(&gate);
            let context = preamble(&gate);
            aside.open(gate);
            if args.trim().is_empty() {
                return Some(Ok(notice));
            }
            app.emit(DomainEvent::Notice {
                level: NoticeLevel::Info,
                message: notice,
            });
            run_turn_in(app, command_turn(args.trim()), Some(context)).await;
            return None;
        }
        ("flow" | "volver", gate) if !args.trim().is_empty() || !aside.is_open() => {
            aside.close();
            return match gate {
                Some(_) if !args.trim().is_empty() => {
                    Some(research::line(session, args.trim()).await)
                }
                Some(gate) => Some(Ok(format!("{gate}: {}", gate.prompt.trim()))),
                None => Some(Err("no flow waits for an answer".to_owned())),
            };
        }
        ("flow" | "volver", _) => aside.propose().map(Route::Propose),
        // `/back`, `/stage`…: the flow takes them, and the side chat closes.
        (command, _) if !command.is_empty() => {
            aside.close();
            None
        }
        (_, gate) => Some(aside.route(line, gate, &app.decider.clone(), false).await.0),
    };
    match route {
        Some(Route::Aside(context)) => {
            run_turn_in(app, command_turn(line), Some(context)).await;
            None
        }
        Some(Route::Propose(context)) => {
            let prompt = if line.trim_start().starts_with('/') {
                "back to the flow"
            } else {
                line
            };
            let before = app.session.turns.len();
            run_turn_in(app, command_turn(prompt), Some(context)).await;
            let answer = app.session.turns.get(before..).and_then(last_answer);
            Some(Ok(aside.proposed(answer)))
        }
        Some(Route::Answer(text)) => Some(research::line(session, &text).await),
        Some(Route::Chat) | None => Some(research::line(session, line).await),
    }
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

/// What to tell the user about a flow left open (saved, not waiting): the
/// graph engine reruns the failed stage with `retry`, the discovery engine
/// rejects `/retry` and continues from its log with `resume`.
fn open_hint(engine: Engine) -> &'static str {
    match engine {
        Engine::Graph => {
            "the flow is saved; `crest --headless --research retry` runs the stage again"
        }
        Engine::Discovery => {
            "the discovery is saved; `crest --headless --research resume` continues it"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_discovery_is_told_to_resume_not_retry() {
        let hint = open_hint(Engine::Discovery);
        assert!(hint.contains("--research resume"), "{hint}");
        assert!(!hint.contains("retry"), "{hint}");
    }

    #[test]
    fn an_open_graph_flow_keeps_the_retry_hint() {
        assert!(open_hint(Engine::Graph).contains("--research retry"));
    }
}
