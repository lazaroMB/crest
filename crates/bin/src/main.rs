//! `crest`: parses the command line and starts the UI or the headless mode.

use std::process::ExitCode;
use std::time::Duration;

use application::usecases::cancel::CancelToken;
use clap::Parser;
use crest::args::{Args, HeadlessRun};
use crest::telemetry::Telemetry;
use crest::{gui, headless};

fn main() -> ExitCode {
    let args = Args::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: cannot start the tokio runtime: {err}");
            return ExitCode::from(2);
        }
    };
    let (telemetry, warnings) = {
        let _guard = runtime.enter();
        Telemetry::init()
    };
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    let code = match args.headless_run() {
        Some(Ok(HeadlessRun::Prompt(prompt))) => runtime
            .block_on(headless::run(&args, prompt, CancelToken::new()))
            .code(),
        Some(Ok(HeadlessRun::Implement(target))) => runtime
            .block_on(headless::implement::run(&args, target))
            .code(),
        Some(Ok(HeadlessRun::Research(problem))) => runtime
            .block_on(headless::research::run(&args, problem))
            .code(),
        Some(Err(message)) => {
            eprintln!("error: {message}");
            2
        }
        None => gui::run(&args, runtime.handle()),
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
