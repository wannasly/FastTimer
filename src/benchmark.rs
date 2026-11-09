use crate::{
    Result,
    timer::{self, Case, Interval, Lease, Request, Snapshot},
};
use serde::Serialize;
use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::System::{
    Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
    Threading::Sleep,
};

pub const DEFAULT_CASES: &str = "baseline,fastest,1.0000,winmm:1";

pub struct Plan {
    pub cases: Vec<Case>,
    pub samples: usize,
    pub warmup: usize,
    pub rounds: usize,
}

impl Plan {
    pub fn new(cases: &str, samples: usize, warmup: usize, rounds: usize) -> Result<Self> {
        let cases = cases
            .split(',')
            .map(str::parse)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if cases.len() > 32
            || !(1..=10_000).contains(&samples)
            || warmup > 1_000
            || !(1..=20).contains(&rounds)
            || cases.len() * (samples + warmup) * rounds > 200_000
        {
            return Err("Use 1–32 cases, 1–10000 samples, 0–1000 warmup, 1–20 rounds and at most 200000 total waits".into());
        }
        Ok(Self {
            cases,
            samples,
            warmup,
            rounds,
        })
    }

    pub fn resolve(&self, range: Snapshot) -> Result<Vec<Request>> {
        self.cases
            .iter()
            .map(|&case| Request::resolve(case, range))
            .collect()
    }
}

#[derive(Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub started_unix_ms: u64,
    pub windows_build: u32,
    pub qpc_frequency_hz: i64,
    pub sleep_ms: u32,
    pub samples_per_round: usize,
    pub warmup_per_round: usize,
    pub rounds: usize,
    pub background_policy_honor_requests: bool,
    pub initial_timer: Snapshot,
    pub final_timer: Snapshot,
    pub cases: Vec<Measurement>,
}

#[derive(Serialize)]
pub struct Measurement {
    pub case: String,
    pub request: Request,
    pub observations: Vec<Observation>,
    pub stats: Statistics,
    pub samples_ms: Vec<f64>,
}

#[derive(Serialize)]
pub struct Observation {
    pub round: usize,
    pub order_in_round: usize,
    pub set_return_100ns: Option<Interval>,
    pub query_before_100ns: Interval,
    pub query_after_100ns: Interval,
}

#[derive(Serialize)]
pub struct Statistics {
    pub count: usize,
    pub min_ms: f64,
    pub mean_ms: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub population_stdev_ms: f64,
    pub mean_absolute_error_ms: f64,
    pub early_count: usize,
    pub outlier_count: usize,
    pub tail_jitter_ms: f64,
    pub total_sleep_ms: f64,
}

impl Statistics {
    fn from_samples(samples: &[f64]) -> Self {
        let mut ordered = samples.to_vec();
        ordered.sort_unstable_by(f64::total_cmp);
        let count = ordered.len();
        let total_sleep_ms = ordered.iter().sum::<f64>();
        let mean_ms = total_sleep_ms / count as f64;
        let variance = ordered.iter().map(|ms| (ms - mean_ms).powi(2)).sum::<f64>() / count as f64;
        let absolute_error = ordered.iter().map(|ms| (ms - 1.0).abs()).sum::<f64>() / count as f64;
        let nearest_rank = |percent: usize| ordered[(count * percent).div_ceil(100) - 1];
        let median_ms = (ordered[(count - 1) / 2] + ordered[count / 2]) / 2.0;
        let (q1, q3) = (nearest_rank(25), nearest_rank(75));
        let fence = 1.5 * (q3 - q1);
        Self {
            count,
            min_ms: ordered[0],
            mean_ms,
            median_ms,
            p95_ms: nearest_rank(95),
            p99_ms: nearest_rank(99),
            max_ms: ordered[count - 1],
            population_stdev_ms: variance.sqrt(),
            mean_absolute_error_ms: absolute_error,
            early_count: ordered.iter().take_while(|&&ms| ms < 1.0).count(),
            outlier_count: ordered
                .iter()
                .filter(|&&ms| ms < q1 - fence || ms > q3 + fence)
                .count(),
            tail_jitter_ms: nearest_rank(99) - median_ms,
            total_sleep_ms,
        }
    }
}

pub fn run(plan: &Plan, honor_background: bool, cancel: &AtomicBool) -> Result<Report> {
    let started_unix_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let windows_build = timer::windows_build()?;
    let initial_timer = timer::query()?;
    let requests = plan.resolve(initial_timer)?;
    let mut frequency = 0;
    if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0 || frequency <= 0 {
        return Err("QueryPerformanceFrequency is unavailable".into());
    }
    let scale = 1_000.0 / frequency as f64;
    let mut measurements: Vec<_> = requests
        .iter()
        .map(|_| {
            (
                Vec::with_capacity(plan.rounds),
                Vec::with_capacity(plan.samples * plan.rounds),
            )
        })
        .collect();

    for round in 0..plan.rounds {
        for order in 0..requests.len() {
            let index = (round + order) % requests.len();
            let (observations, samples) = &mut measurements[index];
            check_cancel(cancel)?;
            let mut lease = Lease::acquire(requests[index])?;
            for _ in 0..plan.warmup {
                check_cancel(cancel)?;
                unsafe { Sleep(1) };
            }
            let before = timer::query()?;
            for _ in 0..plan.samples {
                check_cancel(cancel)?;
                let start = counter()?;
                // Use the actual Win32 Sleep API, not Rust's thread::sleep.
                unsafe { Sleep(1) };
                samples.push((counter()? - start) as f64 * scale);
            }
            let after = timer::query()?;
            lease.release()?;
            observations.push(Observation {
                round: round + 1,
                order_in_round: order + 1,
                set_return_100ns: lease.reported_100ns,
                query_before_100ns: before.current_100ns,
                query_after_100ns: after.current_100ns,
            });
        }
    }
    let final_timer = timer::query()?;
    let cases = plan
        .cases
        .iter()
        .zip(requests)
        .zip(measurements)
        .map(
            |((&case, request), (observations, samples_ms))| Measurement {
                case: case.to_string(),
                request,
                observations,
                stats: Statistics::from_samples(&samples_ms),
                samples_ms,
            },
        )
        .collect();
    Ok(Report {
        schema_version: 2,
        started_unix_ms,
        windows_build,
        qpc_frequency_hz: frequency,
        sleep_ms: 1,
        samples_per_round: plan.samples,
        warmup_per_round: plan.warmup,
        rounds: plan.rounds,
        background_policy_honor_requests: honor_background,
        initial_timer,
        final_timer,
        cases,
    })
}

fn counter() -> io::Result<i64> {
    let mut value = 0;
    if unsafe { QueryPerformanceCounter(&mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

fn check_cancel(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Benchmark cancelled",
        ));
    }
    Ok(())
}

impl Report {
    pub fn table(&self) -> String {
        let mut table = format!(
            "{:<12} {:>9} {:>9} {:>9} {:>9}\r\n",
            "CASE", "MEAN", "P50", "P95", "P99",
        );
        use std::fmt::Write as _;
        for case in &self.cases {
            let s = &case.stats;
            let _ = write!(
                table,
                "{:<12} {:>9.3} {:>9.3} {:>9.3} {:>9.3}\r\n",
                case.case, s.mean_ms, s.median_ms, s.p95_ms, s.p99_ms
            );
        }
        table
    }

    pub fn write_json(&self, path: &Path) -> Result<()> {
        let mut file = BufWriter::new(File::create(path)?);
        serde_json::to_writer_pretty(&mut file, self)?;
        writeln!(file)?;
        file.flush()?;
        Ok(())
    }

    pub fn write_csv(&self, path: &Path) -> Result<()> {
        let mut file = BufWriter::new(File::create(path)?);
        writeln!(
            file,
            "case,api,requested_100ns,windows_build,rounds,count,min_ms,mean_ms,median_ms,p95_ms,p99_ms,max_ms,population_stdev_ms,mean_absolute_error_ms,early_count,query_min_100ns,query_max_100ns,outlier_count,tail_jitter_ms,total_sleep_ms"
        )?;
        for case in &self.cases {
            let (api, requested) = match case.request {
                Request::None => ("none", String::new()),
                Request::Native { interval_100ns } => ("native", interval_100ns.0.to_string()),
                Request::WinMm { period_ms } => {
                    ("winmm", (u64::from(period_ms) * 10_000).to_string())
                }
            };
            let queries = case
                .observations
                .iter()
                .flat_map(|o| [o.query_before_100ns.0, o.query_after_100ns.0]);
            let first = case.observations[0].query_before_100ns.0;
            let (minimum, maximum) = queries.fold((first, first), |(min, max), value| {
                (min.min(value), max.max(value))
            });
            let s = &case.stats;
            // Case strings are normalized tokens/numbers and never contain CSV delimiters.
            writeln!(
                file,
                "{},{api},{requested},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{minimum},{maximum},{},{:.6},{:.6}",
                case.case,
                self.windows_build,
                self.rounds,
                s.count,
                s.min_ms,
                s.mean_ms,
                s.median_ms,
                s.p95_ms,
                s.p99_ms,
                s.max_ms,
                s.population_stdev_ms,
                s.mean_absolute_error_ms,
                s.early_count,
                s.outlier_count,
                s.tail_jitter_ms,
                s.total_sleep_ms
            )?;
        }
        file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_keep_outliers_and_early_wakes() {
        let stats = Statistics::from_samples(&[2.0, 0.5, 1.0, 5.0]);
        assert_eq!(stats.count, 4);
        assert_eq!(stats.median_ms, 1.5);
        assert_eq!(stats.p95_ms, 5.0);
        assert_eq!(stats.p99_ms, 5.0);
        assert_eq!(stats.early_count, 1);
        assert_eq!(stats.mean_absolute_error_ms, 1.375);
        assert_eq!(stats.outlier_count, 1);
        assert_eq!(stats.tail_jitter_ms, 3.5);
        assert_eq!(stats.total_sleep_ms, 8.5);
    }

    #[test]
    fn rejects_unbounded_or_malformed_plans() {
        assert!(Plan::new(DEFAULT_CASES, 100, 20, 3).is_ok());
        assert!(Plan::new("baseline,", 1, 0, 1).is_err());
        assert!(Plan::new("fastest", 0, 0, 1).is_err());
        assert!(Plan::new(DEFAULT_CASES, 10_000, 1_000, 20).is_err());
    }
}
