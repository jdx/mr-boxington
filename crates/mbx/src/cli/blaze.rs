//! Sending dev builds typed as plain `cargo` through blaze.
//!
//! blaze (<https://github.com/indexable-inc/blaze>) keeps a workspace compiled
//! in a daemon and rebuilds an edited function without running Cargo. Its fast
//! path needs its own client in front of Cargo, which `cargo blaze build`
//! gives; a workspace wrapper alone does not. With the `blaze` setting on, the
//! Cargo shim runs that client for the commands it serves and names itself as
//! the client's `CARGO`, so the Cargo builds blaze still runs come back through
//! the shim and use the cache.

use eyre::{Context, Result};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Set for blaze's client and everything it starts. The shim never routes a
/// command that carries it: that is blaze running Cargo, or a program blaze
/// ran, and either one has already been through blaze once.
pub(super) const ROUTED_ENV: &str = "MBX_BLAZE_ROUTED";

/// blaze's engine. Releases before the rename have no `blaze` program, and
/// every release has this one, which `cargo blaze` runs as `ra-jit cargo`.
const ENGINE: &str = "ra-jit";

/// Run `arguments` through blaze when the setting, the command, the platform,
/// and the installation allow it. `None` leaves the command to mbx.
pub(super) fn route(
    enabled: bool,
    arguments: &[String],
    shim: Option<&Path>,
) -> Option<Result<ExitCode>> {
    if !enabled || std::env::var_os(ROUTED_ENV).is_some() || !serves(arguments) {
        return None;
    }
    if !builds_here() {
        log::debug!(
            "blaze builds only on Linux and on macOS on Apple silicon; building with Cargo"
        );
        return None;
    }
    let Some(shim) = shim else {
        log::warn!(
            "blaze is on, but this Cargo call did not come through mbx's Cargo shim, which blaze needs to reach mbx again; building with Cargo. Run `mbx setup` to install the shim"
        );
        return None;
    };
    let Some(engine) = engine(std::env::var_os("PATH").as_deref()) else {
        log::warn!("blaze is on, but `{ENGINE}` is not on PATH; building with Cargo");
        return None;
    };
    log::debug!(
        "sending `cargo {}` through {}",
        arguments.join(" "),
        engine.display()
    );
    Some(run(&engine, shim, arguments))
}

/// Whether blaze's client serves this command line: a dev-profile build, run,
/// test, or check named first. A release or named profile, a `+toolchain`, and
/// flags ahead of the subcommand are left to Cargo, as is everything after
/// `--`, which belongs to the program or the test harness.
fn serves(arguments: &[String]) -> bool {
    let Some((command, rest)) = arguments.split_first() else {
        return false;
    };
    if !matches!(
        command.as_str(),
        "build" | "b" | "run" | "r" | "test" | "t" | "check" | "c"
    ) {
        return false;
    }
    !rest
        .iter()
        .take_while(|argument| argument.as_str() != "--")
        .any(|argument| {
            matches!(argument.as_str(), "--release" | "-r" | "--profile")
                || argument.starts_with("--profile=")
        })
}

fn builds_here() -> bool {
    cfg!(target_os = "linux") || cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

fn engine(path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .map(|directory| directory.join(ENGINE))
        .find(|candidate| candidate.is_file())
}

fn run(engine: &Path, shim: &Path, arguments: &[String]) -> Result<ExitCode> {
    let status = Command::new(engine)
        .arg("cargo")
        .args(arguments)
        .env("CARGO", shim)
        .env(ROUTED_ENV, "1")
        .status()
        .wrap_err_with(|| format!("failed to run {}", engine.display()))?;
    Ok(super::cargo::exit_code(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn dev_builds_go_to_blaze() {
        for line in [
            "build",
            "b -p app",
            "run --bin app",
            "r",
            "test --lib",
            "t",
            "check --all-targets",
            "c",
            "run -- --release",
            "test -- --profile x",
        ] {
            assert!(serves(&arguments(line)), "{line}");
        }
    }

    #[test]
    fn other_builds_stay_with_cargo() {
        for line in [
            "",
            "build --release",
            "run -r",
            "test --profile bench",
            "check --profile=ci",
            "+nightly build",
            "-q build",
            "--locked build",
            "clippy",
            "doc",
            "bench",
            "install --path .",
        ] {
            assert!(!serves(&arguments(line)), "{line}");
        }
    }

    #[test]
    fn engine_is_found_on_path() {
        let root = tempfile::tempdir().unwrap();
        let empty = root.path().join("empty");
        let blaze = root.path().join("blaze");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&blaze).unwrap();
        std::fs::write(blaze.join(ENGINE), "").unwrap();
        let path = std::env::join_paths([&empty, &blaze]).unwrap();
        assert_eq!(engine(Some(&path)), Some(blaze.join(ENGINE)));
        let path = std::env::join_paths([&empty]).unwrap();
        assert_eq!(engine(Some(&path)), None);
        assert_eq!(engine(None), None);
    }

    #[test]
    fn routing_needs_the_setting() {
        assert!(route(false, &arguments("build"), Some(Path::new("/shim/cargo"))).is_none());
    }
}
