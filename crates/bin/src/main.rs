//! `crest`: parses the command line and starts the UI or the headless mode.

use std::process::ExitCode;
use std::time::Duration;

use application::usecases::cancel::CancelToken;
use clap::Parser;
use crest::args::Args;
use crest::headless;
use crest::root::Config;
use crest::telemetry::Telemetry;

fn main() -> ExitCode {
    let args = Args::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: no puedo iniciar el runtime de tokio: {err}");
            return ExitCode::from(2);
        }
    };
    let (telemetry, warnings) = {
        let _guard = runtime.enter();
        Telemetry::init()
    };
    for warning in warnings {
        eprintln!("aviso: {warning}");
    }
    let code = match &args.headless {
        Some(prompt) => runtime
            .block_on(headless::run(&args, prompt, CancelToken::new()))
            .code(),
        None => run_ui(&args),
    };
    {
        let _guard = runtime.enter();
        telemetry.shutdown();
    }
    // A pending stdin read (an approval interrupted by Ctrl+C) must not
    // keep the process alive.
    runtime.shutdown_timeout(Duration::from_millis(200));
    ExitCode::from(code)
}

/// Opens the window. The agent is not connected to it yet (T3.3).
fn run_ui(args: &Args) -> u8 {
    let config = match Config::from_args(args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err:#}");
            return 2;
        }
    };
    let workspace_label = config.workspace.file_name().map_or_else(
        || config.workspace.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    match ui::app::run(ui::app::UiOptions {
        workspace_label,
        ..ui::app::UiOptions::default()
    }) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("error: {err}");
            1
        }
    }
}
