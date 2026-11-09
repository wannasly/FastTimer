#![windows_subsystem = "windows"]

#[path = "../gui/mod.rs"]
mod gui;

use clap::Parser;
use fasttimer::timer::Case;
use std::process::ExitCode;

#[derive(Parser)]
#[command(version, about = "FastTimer timer resolution utility")]
struct Args {
    /// Start in the notification area without showing the window.
    #[arg(long)]
    hidden: bool,
    /// Initial request: baseline, fastest, decimal ms, or winmm:<whole ms>.
    #[arg(long, default_value = "baseline")]
    request: Case,
    /// Exit the running desktop instance and release its request.
    #[arg(long, conflicts_with_all = ["hidden", "request"])]
    exit: bool,
}

fn main() -> ExitCode {
    match Args::try_parse() {
        Ok(args) => {
            let result = if args.exit {
                gui::exit()
            } else {
                gui::run(args.hidden, args.request)
            };
            if let Err(error) = result {
                gui::message(&error.to_string(), true);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            gui::message(&error.to_string(), error.use_stderr());
            if error.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
    }
}
