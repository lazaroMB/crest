//! `crest --headless --simulate <spec>`: simulates the solution of a Spec
//! in the terminal. Prints the verdicts as the window's `/simulate` shows
//! them ([`simulation_markdown`]: every scenario and each failed check with
//! its counterexample time), the lint warnings, the evidence recorded and
//! the path of `board.html`.
//! Exits with 0 when every scenario passes and with 1 otherwise, or when
//! the simulation could not be produced.

use application::usecases::simulate::SimulateRequest;

use super::run::frontend;
use super::terminal::{Style, Terminal};
use super::{Exit, StdinApproval};
use crate::args::Args;
use crate::gui::{simulate_failed_notice, simulation_markdown};
use crate::root::{App, Config, Overrides, build_app, now_ms};

/// Simulates `spec` with the app's simulator and prints the result.
pub async fn run_simulation(app: &App, spec: &str, terminal: &Terminal) -> Exit {
    let request = SimulateRequest {
        spec: spec.to_owned(),
        now_ms: now_ms(),
        open_board: false,
    };
    let report = match app.simulator.run(request).await {
        Ok(report) => report,
        Err(err) => {
            terminal
                .lock()
                .line(Some(Style::Red), &simulate_failed_notice(&err));
            return Exit::TurnFailed;
        }
    };
    let mut w = terminal.lock();
    w.break_line();
    // The verdicts as the window shows them, coloured; the board's path
    // follows in full, so its relative line is left out.
    let markdown = simulation_markdown(&report);
    for (ix, line) in markdown.lines().enumerate() {
        let style = if ix == 0 {
            Some(Style::Bold)
        } else if line.starts_with("- PASS") {
            Some(Style::Green)
        } else if line.starts_with("- FAIL") || line.starts_with("  - ✗") {
            Some(Style::Red)
        } else if line.starts_with("- Board: ") {
            continue;
        } else {
            None
        };
        w.line(style, line);
    }
    for warning in &report.warnings {
        w.line(Some(Style::Dim), &format!("lint: {warning}"));
    }
    if !report.evidence.is_empty() {
        let ids: Vec<String> = report.evidence.iter().map(ToString::to_string).collect();
        w.line(
            Some(Style::Dim),
            &format!("evidence recorded: {}", ids.join(", ")),
        );
    }
    if let Some(html) = report.board_html() {
        let path = app.workspace.as_path().join(html);
        w.line(None, &format!("board: {}", path.display()));
    }
    if report.board.passed() {
        Exit::Success
    } else {
        Exit::TurnFailed
    }
}

/// `crest --headless --simulate <spec>` on stdout.
pub async fn run(args: &Args, spec: &str) -> Exit {
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
        Ok(app) => run_simulation(&app, spec, &terminal).await,
        Err(err) => {
            eprintln!("error: {err:#}");
            Exit::Config
        }
    }
}
