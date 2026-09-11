//! Append-only cache progress for pipes and agents using a terminal.
use crate::session;
use eyre::Result;
use mbx_cache_core::AgentStats;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(super) fn run(
    cargo: &OsStr,
    arguments: &[String],
    environment: BTreeMap<String, String>,
    stats: impl Fn() -> AgentStats + Sync,
) -> Result<ExitCode> {
    with_progress(
        Duration::from_secs(15),
        stats,
        |line| crate::logging::note(&line),
        || super::cargo::run_cargo(cargo, arguments, environment),
    )
}

fn with_progress<T>(
    interval: Duration,
    stats: impl Fn() -> AgentStats + Sync,
    report: impl Fn(String) + Sync,
    operation: impl FnOnce() -> T,
) -> T {
    std::thread::scope(|scope| {
        let (stop, stopped) = mpsc::channel::<()>();
        let stats = &stats;
        let report = &report;
        scope.spawn(move || {
            let started = Instant::now();
            while matches!(
                stopped.recv_timeout(interval),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                report(line(started.elapsed(), &stats()));
            }
        });
        // Cargo retains its inherited streams, signals, arguments and runners.
        // Dropping the sender also interrupts the wait when operation unwinds.
        let result = operation();
        drop(stop);
        result
    })
}

fn line(elapsed: Duration, stats: &AgentStats) -> String {
    format!(
        "mbx[progress]: {}s elapsed; {} hits, {} misses, {} bypassed, {} not looked up; ~{:.1}s compiler work saved",
        elapsed.as_secs(),
        stats.hits,
        session::cache_misses(stats),
        session::unexpected_bypasses(stats),
        stats.unconsulted,
        stats.avoided_compiler_duration_ns as f64 / 1e9,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_commands_report_without_changing_the_result_or_using_terminal_controls() {
        let (send, receive) = mpsc::channel();
        let result = with_progress(
            Duration::from_millis(5),
            || {
                let mut stats = AgentStats::default();
                stats.hits = 1;
                stats.lookups = 3;
                stats.avoided_compiler_duration_ns = 1_500_000_000;
                stats
            },
            |line| send.send(line).unwrap(),
            || {
                let line = receive.recv_timeout(Duration::from_secs(2)).unwrap();
                assert!(line.contains("1 hits, 2 misses"));
                assert!(line.contains("~1.5s compiler work saved"));
                assert!(!line.contains(['\r', '\x1b']));
                101
            },
        );
        assert_eq!(result, 101);
    }

    #[test]
    fn short_commands_stop_reporting_immediately() {
        let started = Instant::now();
        with_progress(
            Duration::from_secs(60),
            AgentStats::default,
            |_| panic!("short command reported"),
            || (),
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
