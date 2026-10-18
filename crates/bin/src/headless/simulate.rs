//! `crest --headless --simulate <spec>`: simulates the solution of a Spec
//! in the terminal. Prints the verdict of every scenario (and each failed
//! check with its counterexample time) and the path of `board.html`.
//! Exits with 0 when every scenario passes and with 1 otherwise, or when
//! the simulation could not be produced.

use application::usecases::simulate::SimulateRequest;

use super::run::frontend;
use super::terminal::{Style, Terminal};
use super::{Exit, StdinApproval};
use crate::args::Args;
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
            terminal.lock().line(
                Some(Style::Red),
                &format!("error: simulation failed: {err}"),
            );
            return Exit::TurnFailed;
        }
    };
    let mut w = terminal.lock();
    w.break_line();
    let runs = &report.board.runs;
    let passed = runs.iter().filter(|r| r.passed).count();
    w.line(
        Some(Style::Bold),
        &format!(
            "Simulation of {}: {passed} of {} scenarios passed (model {})",
            report.folder,
            runs.len(),
            report.board.model.hash()
        ),
    );
    for run in runs {
        let ok = run.verdicts.iter().filter(|v| v.passed).count();
        let (style, label) = if run.passed {
            (Style::Green, "PASS")
        } else {
            (Style::Red, "FAIL")
        };
        w.line(
            Some(style),
            &format!(
                "{label} {} ({ok}/{} checks, seed {})",
                run.scenario,
                run.verdicts.len(),
                run.trace.seed
            ),
        );
        for v in run.failures() {
            w.line(
                Some(Style::Red),
                &format!("  ✗ {}: {}", v.property, v.detail),
            );
        }
    }
    if report.repairs > 0 {
        w.line(
            Some(Style::Dim),
            &format!("repair rounds: {}", report.repairs),
        );
    }
    for note in &report.loosening {
        w.line(
            Some(Style::Yellow),
            &format!("frozen assertions restored: {note}"),
        );
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
