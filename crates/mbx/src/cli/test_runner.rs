//! Test binaries under the machine-wide permit pool.
//!
//! The pool paces compilers, but `cargo test` spends much of its time running
//! test binaries, and libtest sizes itself to the whole machine. Three
//! worktrees testing at once are three suites each starting a thread per CPU.
//! With `scheduler.tests`, mbx installs itself as Cargo's target runner for
//! the command; each test binary then waits for permits like a compiler does,
//! and chains to whatever runner was configured before.
//!
//! Work a test starts is charged to the test's own permits: its children see
//! scheduling turned off. Otherwise a test that builds something -- trybuild,
//! compile-fail suites, mbx's own integration tests -- would hold permits
//! while its compilers waited for more, and two of them could hold the whole
//! pool between them and wait on each other forever.
use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const SHIM: &str = "mbx-test-runner";
/// The runner each target had before this one, by target triple.
const RUNNERS: &str = "MBX_TEST_RUNNERS";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Runner {
    path: PathBuf,
    args: Vec<OsString>,
}

pub(super) struct TestRunner {
    /// Runner environment keys, each paired with the triple it names.
    keys: Vec<(String, String)>,
    runners: BTreeMap<String, Option<Runner>>,
    shim: PathBuf,
}

impl TestRunner {
    /// Wrap this command's test binaries, when it is a `cargo test` and
    /// scheduling them is on. `None` leaves Cargo's runners alone.
    pub(super) fn prepare(
        config: &crate::config::Config,
        arguments: &[String],
        directory: &Path,
    ) -> Result<Option<Self>> {
        if !config.scheduler.enabled || !config.scheduler.tests || !wraps(arguments) {
            return Ok(None);
        }
        // Resolution is best-effort, like the pool itself: a configuration
        // mbx cannot read runs the tests unscheduled rather than failing.
        let resolved = (|| -> Result<_> {
            let cargo = cargo_config2::Config::load()?;
            let inherited = inherited_runners();
            let mut keys = Vec::new();
            let mut runners = BTreeMap::new();
            for target in super::launch::requested_targets(&cargo, arguments)? {
                let triple = target.triple().to_owned();
                let runner = cargo.runner(&target)?.map(|runner| Runner {
                    path: runner.path,
                    args: runner.args,
                });
                // A nested mbx sees the outer one's shim as the configured
                // runner. Chaining to it would loop; chain to what it wraps.
                let runner = match runner {
                    Some(runner) if is_shim(&runner.path) => {
                        inherited.get(&triple).cloned().flatten()
                    }
                    runner => runner,
                };
                keys.push((super::launch::runner_key(&target), triple.clone()));
                runners.insert(triple, runner);
            }
            Ok((keys, runners))
        })();
        let (keys, runners) = match resolved {
            Ok(resolved) => resolved,
            Err(error) => {
                log::debug!("running tests without machine-wide permits: {error:#}");
                return Ok(None);
            }
        };
        let shim = crate::session::install_shim_named(
            &std::env::current_exe()?,
            directory,
            SHIM,
            crate::session::ShimLink::Tracking,
        )?;
        Ok(Some(Self {
            keys,
            runners,
            shim,
        }))
    }

    pub(super) fn environment(&self, environment: &mut BTreeMap<String, String>) -> Result<()> {
        // Cargo splits a runner from the environment on whitespace, so the
        // shim is named through PATH, as `cargo run`'s launch shim is, and
        // the triple rides along as its first argument.
        let path = environment
            .get("PATH")
            .map(OsString::from)
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(self.shim.parent().unwrap().to_path_buf())
                .chain(std::env::split_paths(&path)),
        )?;
        environment.insert("PATH".into(), path.to_string_lossy().into_owned());
        for (key, triple) in &self.keys {
            environment.insert(key.clone(), format!("{SHIM} {triple}"));
        }
        environment.insert(RUNNERS.into(), serde_json::to_string(&self.runners)?);
        Ok(())
    }
}

/// Whether a Cargo command line runs test binaries mbx can wrap.
///
/// Configuration overrides on the command line are out of reach for the same
/// reason as in `cargo run`'s launch: cargo-config2 cannot resolve them, and
/// an unknown runner must never be replaced.
fn wraps(arguments: &[String]) -> bool {
    let args: Vec<_> = arguments
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .collect();
    matches!(
        super::launch::cargo_subcommand(arguments),
        Some("test" | "t")
    ) && !args.iter().any(|arg| {
        arg.starts_with('+')
            || matches!(
                arg.as_str(),
                "--config" | "-C" | "--help" | "-h" | "--no-run"
            )
            || arg.starts_with("--config=")
    })
}

fn is_shim(path: &Path) -> bool {
    path.file_stem() == Some(OsStr::new(SHIM))
}

fn inherited_runners() -> BTreeMap<String, Option<Runner>> {
    std::env::var(RUNNERS)
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
}

/// Run one test binary once the pool has room for it.
pub fn dispatch() -> Option<ExitCode> {
    let mut arguments = std::env::args_os();
    if arguments.next().is_none_or(|arg| !is_shim(Path::new(&arg))) {
        return None;
    }
    let result = (|| -> Result<ExitCode> {
        let triple = arguments
            .next()
            .ok_or_else(|| eyre::eyre!("missing target triple"))?;
        let executable = arguments
            .next()
            .ok_or_else(|| eyre::eyre!("Cargo supplied no test binary"))?;
        let rest: Vec<OsString> = arguments.collect();
        let runner = inherited_runners()
            .remove(&*triple.to_string_lossy())
            .flatten();
        let mut command = match &runner {
            Some(runner) => {
                let mut command = Command::new(&runner.path);
                command.args(&runner.args).arg(&executable);
                command
            }
            None => Command::new(&executable),
        };
        command
            .args(&rest)
            // Explicitly off for shims the test's own builds run, and for any
            // mbx command it starts: that work is charged to this permit.
            .env(crate::scheduler::SCHED_DIR_ENV, "")
            .env("MBX_SCHEDULER", "0");
        let demand = crate::scheduler::Demand::test(
            &binary_name(Path::new(&executable)),
            test_threads(&rest, std::env::var("RUST_TEST_THREADS").ok().as_deref()),
        );
        // Listing a suite starts no tests and costs nothing worth waiting for.
        let permit = if lists_tests(&rest) {
            None
        } else {
            crate::scheduler::pool().and_then(|pool| pool.admit(&demand))
        };
        let status = command
            .status()
            .wrap_err_with(|| format!("failed to run {}", Path::new(&executable).display()))?;
        if permit.is_some() {
            crate::scheduler::record_compiler_memory(&demand, &status);
        }
        drop(permit);
        Ok(super::cargo::exit_code(status))
    })();
    Some(result.unwrap_or_else(|error| {
        eprintln!("mbx[error]: failed to run test binary: {error:#}");
        ExitCode::FAILURE
    }))
}

/// A test binary's name without the metadata hash Cargo appends to it, so
/// its memory history survives the hash changing.
fn binary_name(executable: &Path) -> String {
    let stem = executable
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    match stem.rsplit_once('-') {
        Some((name, hash))
            if !name.is_empty()
                && hash.len() == 16
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            name.to_owned()
        }
        _ => stem,
    }
}

/// The thread count libtest will use, when the command or environment says.
fn test_threads(arguments: &[OsString], environment: Option<&str>) -> Option<u64> {
    let mut arguments = arguments.iter().filter_map(|arg| arg.to_str());
    let mut stated = None;
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            break;
        }
        if argument == "--test-threads" {
            stated = arguments.next().and_then(|value| value.parse().ok());
        } else if let Some(value) = argument.strip_prefix("--test-threads=") {
            stated = value.parse().ok();
        }
    }
    stated.or_else(|| environment?.parse().ok())
}

fn lists_tests(arguments: &[OsString]) -> bool {
    arguments
        .iter()
        .take_while(|arg| arg.as_os_str() != "--")
        .any(|arg| arg == "--list")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn os(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_plain_test_commands_are_wrapped() {
        assert!(wraps(&strings(&["test"])));
        assert!(wraps(&strings(&["t", "--workspace"])));
        assert!(wraps(&strings(&["--color", "always", "test"])));
        assert!(wraps(&strings(&["test", "--", "--config"])));
        assert!(!wraps(&strings(&["build"])));
        assert!(!wraps(&strings(&["run", "--", "test"])));
        assert!(!wraps(&strings(&["test", "--no-run"])));
        assert!(!wraps(&strings(&["+nightly", "test"])));
        assert!(!wraps(&strings(&["test", "--config", "a.toml"])));
        assert!(!wraps(&strings(&["test", "--config=a.toml"])));
    }

    #[test]
    fn binary_names_drop_only_cargo_s_hash() {
        assert_eq!(
            binary_name(Path::new("target/debug/deps/mbx-0123456789abcdef")),
            "mbx"
        );
        assert_eq!(
            binary_name(Path::new("deps/cache_tests-fedcba9876543210.exe")),
            "cache_tests"
        );
        assert_eq!(binary_name(Path::new("deps/my-tool")), "my-tool");
        assert_eq!(
            binary_name(Path::new("deps/-0123456789abcdef")),
            "-0123456789abcdef"
        );
    }

    #[test]
    fn thread_counts_come_from_arguments_before_the_environment() {
        assert_eq!(test_threads(&os(&[]), None), None);
        assert_eq!(test_threads(&os(&[]), Some("3")), Some(3));
        assert_eq!(
            test_threads(&os(&["--test-threads", "2"]), Some("3")),
            Some(2)
        );
        assert_eq!(test_threads(&os(&["--test-threads=5"]), None), Some(5));
        assert_eq!(test_threads(&os(&["--", "--test-threads=5"]), None), None);
        assert_eq!(test_threads(&os(&[]), Some("many")), None);
    }

    #[test]
    fn listing_is_recognised() {
        assert!(lists_tests(&os(&["--list", "--format", "terse"])));
        assert!(!lists_tests(&os(&["--exact", "list"])));
    }

    #[test]
    fn nested_shims_are_recognised_by_name() {
        assert!(is_shim(Path::new("/tmp/mbx-session-x/mbx-test-runner")));
        assert!(is_shim(Path::new("mbx-test-runner.exe")));
        assert!(!is_shim(Path::new("/usr/bin/qemu-aarch64")));
    }
}
