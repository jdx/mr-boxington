//! Sending dev builds typed as plain `cargo` through blaze.
//!
//! blaze (<https://github.com/indexable-inc/blaze>) keeps a workspace compiled
//! in a daemon and rebuilds an edited function without running Cargo. Its fast
//! path needs its own client in front of Cargo, which `cargo blaze build`
//! gives; a workspace wrapper alone does not. With the `blaze` setting on, the
//! Cargo shim runs that client for the commands it serves and names itself as
//! the client's `CARGO`, so the Cargo builds blaze still runs come back through
//! the shim and use the cache. The Cargo the shim selected travels beside it,
//! so that the build that comes back runs the same Cargo a build that never
//! left would have.

use eyre::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Set for blaze's client and everything it starts. The shim never routes a
/// command that carries it: that is blaze running Cargo, or a program blaze
/// ran, and either one has already been through blaze once.
pub(super) const ROUTED_ENV: &str = "MBX_BLAZE_ROUTED";

/// The Cargo the shim selected before it handed the command to blaze, which
/// the shim runs when blaze's Cargo call comes back. blaze's `CARGO` names the
/// shim, so the shim can't find that choice anywhere else.
const RETURN_CARGO_ENV: &str = "MBX_BLAZE_CARGO";

/// blaze's engine. Releases before the rename have no `blaze` program, and
/// every release has this one, which `cargo blaze` runs as `ra-jit cargo`.
const ENGINE: &str = "ra-jit";

/// Whether blaze could take this command, before any setting is read. Every
/// other command goes on without the settings `route` needs.
pub(super) fn wanted(arguments: &[String]) -> bool {
    std::env::var_os(ROUTED_ENV).is_none() && serves(arguments)
}

/// Run `arguments` through blaze when the setting, the command, the platform,
/// and the installation allow it. `None` leaves the command to mbx, which runs
/// `real_cargo` as it would have.
pub(super) fn route(
    enabled: bool,
    arguments: &[String],
    shim: Option<&Path>,
    real_cargo: &OsStr,
) -> Option<Result<ExitCode>> {
    if !enabled || !wanted(arguments) {
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
    run(&engine, shim, real_cargo, arguments)
}

/// The Cargo to run for a call blaze made through `shim`: the one the shim had
/// selected before routing. Both variables are inherited by everything blaze
/// starts, so the selection applies only while `CARGO` still names the shim,
/// as blaze set it; a program that names another Cargo keeps its choice.
/// `None` for any other call, and for a selection in the shim's own directory,
/// which would only come back here again.
pub(super) fn returning_cargo(shim: &Path) -> Option<OsString> {
    std::env::var_os(ROUTED_ENV)?;
    returning_cargo_from(
        shim,
        std::env::var_os("CARGO").as_deref(),
        std::env::var_os(RETURN_CARGO_ENV).as_deref(),
    )
}

fn returning_cargo_from(
    shim: &Path,
    cargo: Option<&OsStr>,
    selected: Option<&OsStr>,
) -> Option<OsString> {
    if Path::new(cargo?).parent() != shim.parent() {
        return None;
    }
    let selected = Path::new(selected?);
    (selected.is_absolute() && is_executable(selected) && selected.parent() != shim.parent())
        .then(|| selected.as_os_str().to_owned())
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
        .find(|candidate| is_executable(candidate))
}

/// A file a command search would run: a `ra-jit` without the execute bit,
/// such as a half-finished download, is passed over as the shell would.
pub(super) fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Run blaze's client for `arguments`. `None` when it could not be started,
/// such as a `ra-jit` built for another machine: blaze has done nothing yet, so
/// the command goes to Cargo. A failure once it has started is the build's own.
fn run(
    engine: &Path,
    shim: &Path,
    real_cargo: &OsStr,
    arguments: &[String],
) -> Option<Result<ExitCode>> {
    let mut child = match Command::new(engine)
        .arg("cargo")
        .args(arguments)
        .env("CARGO", shim)
        .env(RETURN_CARGO_ENV, real_cargo)
        .env(ROUTED_ENV, "1")
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            log::warn!(
                "blaze is on, but {} could not be started ({error}); building with Cargo",
                engine.display()
            );
            return None;
        }
    };
    Some(
        child
            .wait()
            .wrap_err_with(|| format!("failed to wait for {}", engine.display()))
            .map(super::cargo::exit_code),
    )
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

    fn executable(path: &Path) {
        std::fs::write(path, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn engine_is_found_on_path() {
        let root = tempfile::tempdir().unwrap();
        let empty = root.path().join("empty");
        let blaze = root.path().join("blaze");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&blaze).unwrap();
        executable(&blaze.join(ENGINE));
        let path = std::env::join_paths([&empty, &blaze]).unwrap();
        assert_eq!(engine(Some(&path)), Some(blaze.join(ENGINE)));
        let path = std::env::join_paths([&empty]).unwrap();
        assert_eq!(engine(Some(&path)), None);
        assert_eq!(engine(None), None);
    }

    #[cfg(unix)]
    #[test]
    fn engine_lookup_passes_over_a_file_it_cannot_run() {
        let root = tempfile::tempdir().unwrap();
        let broken = root.path().join("broken");
        let blaze = root.path().join("blaze");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::create_dir_all(&blaze).unwrap();
        std::fs::write(broken.join(ENGINE), "").unwrap();
        executable(&blaze.join(ENGINE));
        let path = std::env::join_paths([&broken, &blaze]).unwrap();
        assert_eq!(engine(Some(&path)), Some(blaze.join(ENGINE)));
    }

    #[test]
    fn the_returning_call_runs_the_cargo_selected_before_routing() {
        let root = tempfile::tempdir().unwrap();
        let shim_dir = root.path().join("mbx");
        let tools = root.path().join("tools");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        let shim = shim_dir.join("cargo");
        let selected = tools.join("cargo");
        executable(&shim);
        executable(&selected);
        let returning = |selected: Option<&OsStr>| {
            returning_cargo_from(&shim, Some(shim.as_os_str()), selected)
        };
        assert_eq!(
            returning(Some(selected.as_os_str())),
            Some(selected.clone().into_os_string())
        );
        // Naming the shim again, or nothing usable, leaves the usual search.
        assert_eq!(returning(Some(shim.as_os_str())), None);
        assert_eq!(returning(Some(OsStr::new("cargo"))), None);
        assert_eq!(returning(Some(tools.join("missing").as_os_str())), None);
        assert_eq!(returning(None), None);
    }

    #[test]
    fn a_program_that_names_another_cargo_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let shim = root.path().join("mbx").join("cargo");
        let selected = root.path().join("tools").join("cargo");
        let other = root.path().join("other").join("cargo");
        for path in [&shim, &selected, &other] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            executable(path);
        }
        assert_eq!(
            returning_cargo_from(&shim, Some(other.as_os_str()), Some(selected.as_os_str())),
            None
        );
        assert_eq!(
            returning_cargo_from(&shim, None, Some(selected.as_os_str())),
            None
        );
    }

    #[test]
    fn an_engine_that_cannot_start_leaves_the_build_to_cargo() {
        let root = tempfile::tempdir().unwrap();
        // Gone between the lookup and the start: every platform refuses to
        // start it, where what an unloadable file does differs (macOS starts
        // a file of zeros and reports the failure as its exit).
        let engine = root.path().join(ENGINE);
        assert!(
            run(
                &engine,
                Path::new("/shim/cargo"),
                OsStr::new("/usr/bin/cargo"),
                &arguments("build"),
            )
            .is_none()
        );
    }

    #[test]
    fn routing_needs_the_setting() {
        assert!(
            route(
                false,
                &arguments("build"),
                Some(Path::new("/shim/cargo")),
                OsStr::new("/usr/bin/cargo"),
            )
            .is_none()
        );
    }
}
