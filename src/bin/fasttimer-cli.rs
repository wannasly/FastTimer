use clap::{Parser, Subcommand};
use fasttimer::{
    Result,
    benchmark::{self, DEFAULT_CASES, Plan},
    timer::{self, Case, Lease, Request},
};
use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};

#[derive(Parser)]
#[command(version, about = "Windows timer resolution and Sleep(1) diagnostics")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the current interval and hardware range reported by Windows.
    Status {
        /// Print machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Hold a request in this process until Ctrl+C or the time limit.
    Hold {
        /// fastest, a period in ms (e.g. 0.5100), or winmm:<whole ms>.
        #[arg(default_value = "fastest")]
        case: Case,
        /// Time limit in seconds. Zero holds until interrupted.
        #[arg(long, default_value_t = 0)]
        seconds: u32,
    },
    /// Compare real Sleep(1) waits in the requesting process.
    Benchmark {
        /// Comma-separated baseline, fastest, decimal ms or winmm:<whole ms>.
        #[arg(long, default_value = DEFAULT_CASES)]
        cases: String,
        #[arg(long, default_value_t = 200)]
        samples: usize,
        #[arg(long, default_value_t = 20)]
        warmup: usize,
        #[arg(long, default_value_t = 3)]
        rounds: usize,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        csv: Option<PathBuf>,
    },
}

static CANCELLED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn console_handler(event: u32) -> i32 {
    if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        CANCELLED.store(true, Ordering::Relaxed);
        1
    } else {
        0
    }
}

fn listen_for_cancel() -> io::Result<()> {
    if unsafe { SetConsoleCtrlHandler(Some(console_handler), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Status { json } => {
            let range = timer::query()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&range)?);
            } else {
                println!(
                    "Windows reports {} ms (range {}–{} ms)",
                    range.current_100ns, range.finest_100ns, range.coarsest_100ns
                );
            }
        }
        Command::Hold { case, seconds } => {
            let request = Request::resolve(case, timer::query()?)?;
            if request == Request::None {
                return Err("Hold requires a timer request".into());
            }
            listen_for_cancel()?;
            if let Err(error) = timer::honor_background_requests() {
                eprintln!("Windows background timer policy could not be set ({error})");
            }
            let mut lease = Lease::acquire(request)?;
            println!("Holding {request}. Ctrl+C releases FastTimer's request.");
            if let Some(reported) = lease.reported_100ns {
                println!("NtSetTimerResolution returned {reported} ms.");
            }
            io::stdout().flush()?;
            let start = Instant::now();
            while !CANCELLED.load(Ordering::Relaxed)
                && (seconds == 0 || start.elapsed() < Duration::from_secs(u64::from(seconds)))
            {
                std::thread::park_timeout(Duration::from_millis(100));
            }
            lease.release()?;
            println!(
                "Released. Windows now reports {} ms.",
                timer::query()?.current_100ns
            );
        }
        Command::Benchmark {
            cases,
            samples,
            warmup,
            rounds,
            json,
            csv,
        } => {
            let plan = Plan::new(&cases, samples, warmup, rounds)?;
            if json.is_some() && json == csv {
                return Err("Choose different paths for JSON and CSV".into());
            }
            listen_for_cancel()?;
            let report = benchmark::run(&plan, false, &CANCELLED)?;
            if let Some(path) = json {
                report.write_json(&path)?;
            }
            if let Some(path) = csv {
                report.write_csv(&path)?;
            }
            println!(
                "Sleep(1), elapsed ms. {} measured waits per case. Windows build {}.",
                report.rounds * report.samples_per_round,
                report.windows_build
            );
            print!("{}", report.table());
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FastTimer: {error}");
            ExitCode::FAILURE
        }
    }
}
