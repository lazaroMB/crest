//! `crest`: parses the command line and starts the UI or the headless mode.

use std::process::ExitCode;
use std::time::Duration;

use application::usecases::cancel::CancelToken;
use clap::Parser;
use crest::args::Args;
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
