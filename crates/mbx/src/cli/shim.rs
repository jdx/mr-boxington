use super::cargo::{CARGO_TARGET_DIR_ENV, cargo_roots, exit_code};
use crate::config::Config;
use eyre::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;

const CARGO_SHIM_STEM: &str = "cargo";
const CARGO_SHIM_MODE_ENV: &str = "MBX_CARGO_SHIM_MODE";
const CARGO_SHIM_PATH_ENV: &str = "MBX_CARGO_SHIM_PATH";
static PATH_BEFORE_SHIM_EXCLUSION: OnceLock<OsString> = OnceLock::new();

/// Whether this process was installed under Cargo's name by `mbx setup`.
pub fn is_cargo_shim() -> bool {
    invoked_as_cargo() || std::env::var_os(CARGO_SHIM_MODE_ENV).is_some_and(|value| value == "1")
}

fn invoked_as_cargo() -> bool {
    std::env::args_os()
        .next()
        .as_deref()
        .is_some_and(is_cargo_shim_path)
}

fn is_cargo_shim_path(path: &OsStr) -> bool {
    Path::new(path).file_stem().is_some_and(|stem| {
        if cfg!(windows) {
            stem.to_string_lossy().eq_ignore_ascii_case(CARGO_SHIM_STEM)
        } else {
            stem == OsStr::new(CARGO_SHIM_STEM)
        }
    })
}

/// Run an invocation received through the persistent Cargo shim.
pub fn run_cargo_shim() -> Result<ExitCode> {
    let reported_shim = std::env::var_os(CARGO_SHIM_PATH_ENV);
    // The Windows launcher uses this only to select dispatch in the target mbx.
    // Do not leak it into Cargo, build scripts, or nested mbx commands.
    unsafe { std::env::remove_var(CARGO_SHIM_MODE_ENV) };
    unsafe { std::env::remove_var(CARGO_SHIM_PATH_ENV) };
    #[cfg(windows)]
    if invoked_as_cargo()
        && let Some(code) = forward_to_current_mbx()?
    {
        return Ok(code);
    }
    let shim_dir = super::setup_install_dir()
        .ok_or_else(|| eyre::eyre!("the platform data directory could not be located"))?;
    let configured_shim = shim_dir.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    let shim = resolve_active_cargo_shim(
        &configured_shim,
        reported_shim.as_deref(),
        std::env::var_os("PATH").as_deref(),
    );
    exclude_shim_from_path(&shim)?;
    let real_cargo = resolve_real_cargo(&shim)?;
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if mbx_disabled() || cargo_proxy_passthrough(&arguments) {
        return run_real_cargo(&real_cargo, &arguments);
    }
    let Ok(string_arguments) = super::strings(&arguments) else {
        return run_real_cargo(&real_cargo, &arguments);
    };
    if cargo_roots(
        &real_cargo,
        &string_arguments,
        std::env::var_os(CARGO_TARGET_DIR_ENV).as_deref(),
    )
    .is_none()
    {
        return run_real_cargo(
            &real_cargo,
            &string_arguments
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>(),
        );
    }
    // Every probe and the final child must name Cargo directly.
    unsafe { std::env::set_var("CARGO", &real_cargo) };
    let (config, settings) = Config::load_for_cli()?;
    super::cargo::run(&config, &settings, &string_arguments)
}

fn resolve_active_cargo_shim(
    configured: &Path,
    reported: Option<&OsStr>,
    path: Option<&OsStr>,
) -> PathBuf {
    reported
        .map(Path::new)
        .filter(|shim| is_cargo_shim_path(shim.as_os_str()) && shim.is_file())
        .map(Path::to_path_buf)
        .or_else(|| {
            let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
            std::env::split_paths(path?)
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
        })
        .unwrap_or_else(|| configured.to_path_buf())
}

#[cfg(windows)]
fn forward_to_current_mbx() -> Result<Option<ExitCode>> {
    let Some(install_dir) = super::setup_install_dir() else {
        return Ok(None);
    };
    let Some(target) = super::setup::cargo_shim_target(&install_dir) else {
        return Ok(None);
    };
    let status = Command::new(&target)
        .args(std::env::args_os().skip(1))
        .env(CARGO_SHIM_MODE_ENV, "1")
        .status()
        .wrap_err_with(|| format!("failed to run the current mbx at {}", target.display()))?;
    Ok(Some(exit_code(status)))
}

fn mbx_disabled() -> bool {
    mbx_disabled_value(std::env::var_os("MBX_DISABLE").as_deref())
}

fn mbx_disabled_value(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy();
        value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn cargo_proxy_passthrough(arguments: &[OsString]) -> bool {
    let cargo_arguments = arguments
        .iter()
        .take_while(|argument| argument.as_os_str() != OsStr::new("--"))
        .collect::<Vec<_>>();
    if cargo_arguments.iter().any(|argument| {
        matches!(
            argument.to_str(),
            Some("--version" | "-V" | "--help" | "-h")
        )
    }) {
        return true;
    }
    let mut skip_value = false;
    let command = cargo_arguments.into_iter().find_map(|argument| {
        if skip_value {
            skip_value = false;
            return None;
        }
        let argument = argument.to_str()?;
        if matches!(argument, "--color" | "--config" | "-Z" | "-C") {
            skip_value = true;
            return None;
        }
        (!argument.starts_with('-') && !argument.starts_with('+')).then_some(argument)
    });
    command.is_none()
        || matches!(
            command,
            Some(
                "help"
                    | "new"
                    | "init"
                    | "add"
                    | "remove"
                    | "update"
                    | "fetch"
                    | "clean"
                    | "config"
                    | "fmt"
                    | "tree"
                    | "search"
                    | "info"
                    | "login"
                    | "logout"
                    | "owner"
                    | "package"
                    | "publish"
                    | "install"
                    | "uninstall"
                    | "yank"
                    | "locate-project"
                    | "metadata"
                    | "generate-lockfile"
                    | "pkgid"
                    | "read-manifest"
                    | "report"
                    | "vendor"
                    | "verify-project"
                    | "version"
                    | "git-checkout"
            )
        )
}

fn same_path(left: &Path, right: &Path) -> bool {
    let (left, right) = match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => (left, right),
        _ => (left.to_path_buf(), right.to_path_buf()),
    };
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Keep explicit `mbx build`-style commands from rediscovering the persistent
/// Cargo shim after setup has placed it first on PATH.
pub(super) fn prepare_explicit_cargo() -> Result<()> {
    let Some(shim_dir) = super::setup_install_dir() else {
        return Ok(());
    };
    let shim = shim_dir.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    if !shim.is_file() {
        return Ok(());
    }
    exclude_shim_from_path(&shim)?;
    let cargo = resolve_real_cargo(&shim)?;
    // SAFETY: CLI dispatch is single-threaded here, before the cache session
    // or any child process has started.
    unsafe { std::env::set_var("CARGO", cargo) };
    Ok(())
}

fn exclude_shim_from_path(shim: &Path) -> Result<()> {
    let Some(path) = std::env::var_os("PATH") else {
        return Ok(());
    };
    PATH_BEFORE_SHIM_EXCLUSION.get_or_init(|| path.clone());
    let path = path_without_shim(shim, &path)?;
    // SAFETY: CLI dispatch is single-threaded here, before any child process
    // or cache session has started.
    unsafe { std::env::set_var("PATH", path) };
    Ok(())
}

pub(super) fn activation_path() -> Option<OsString> {
    PATH_BEFORE_SHIM_EXCLUSION
        .get()
        .cloned()
        .or_else(|| std::env::var_os("PATH"))
}

fn path_without_shim(shim: &Path, path: &OsStr) -> Result<OsString> {
    let Some(shim_dir) = shim.parent() else {
        return Ok(path.to_owned());
    };
    std::env::join_paths(
        std::env::split_paths(path).filter(|directory| !same_path(directory, shim_dir)),
    )
    .map_err(Into::into)
}

fn resolve_real_cargo(current: &Path) -> Result<OsString> {
    let configured = std::env::var_os("CARGO");
    if let Some(cargo) = configured_real_cargo(current, configured.as_deref()) {
        return Ok(cargo);
    }
    let path = std::env::var_os("PATH").ok_or_else(|| eyre::eyre!("PATH is not set"))?;
    resolve_real_cargo_from(current, None, &path)
}

fn configured_real_cargo(current: &Path, configured: Option<&OsStr>) -> Option<OsString> {
    let configured = Path::new(configured?);
    let current_dir = current.parent();
    (configured.is_absolute()
        && configured.is_file()
        && current_dir.is_none_or(|current_dir| {
            configured
                .parent()
                .is_some_and(|parent| !same_path(parent, current_dir))
        }))
    .then(|| configured.as_os_str().to_owned())
}

fn resolve_real_cargo_from(
    current: &Path,
    configured: Option<&OsStr>,
    path: &OsStr,
) -> Result<OsString> {
    let current_dir = current.parent();
    if let Some(cargo) = configured_real_cargo(current, configured) {
        return Ok(cargo);
    }
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    for directory in std::env::split_paths(path) {
        if current_dir.is_some_and(|current_dir| same_path(&directory, current_dir)) {
            continue;
        }
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate.into_os_string());
        }
    }
    eyre::bail!(
        "could not find the real Cargo after excluding {}",
        current.display()
    )
}

fn run_real_cargo(cargo: &OsStr, arguments: &[OsString]) -> Result<ExitCode> {
    let status = Command::new(cargo)
        .args(arguments)
        .status()
        .wrap_err_with(|| format!("failed to run {}", cargo.to_string_lossy()))?;
    Ok(exit_code(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_shim_passthrough_preserves_cargo_namespace_and_toolchains() {
        assert!(cargo_proxy_passthrough(&[]));
        assert!(cargo_proxy_passthrough(&["--verbose".into()]));
        assert!(cargo_proxy_passthrough(&[
            "--color".into(),
            "always".into()
        ]));
        assert!(!cargo_proxy_passthrough(&[
            "--color".into(),
            "always".into(),
            "build".into()
        ]));
        assert!(cargo_proxy_passthrough(&[
            "-C".into(),
            "project".into(),
            "clean".into()
        ]));
        assert!(!cargo_proxy_passthrough(&["cache".into(), "stats".into()]));
        assert!(cargo_proxy_passthrough(&[
            "install".into(),
            "ripgrep".into()
        ]));
        assert!(cargo_proxy_passthrough(&[
            "+nightly".into(),
            "--version".into()
        ]));
        assert!(!cargo_proxy_passthrough(&[
            "+nightly".into(),
            "build".into()
        ]));
        assert!(!cargo_proxy_passthrough(&[
            "test".into(),
            "--workspace".into()
        ]));
        assert!(!cargo_proxy_passthrough(&[
            "run".into(),
            "--".into(),
            "--help".into()
        ]));
        for command in [
            "clean",
            "config",
            "fmt",
            "pkgid",
            "read-manifest",
            "report",
            "vendor",
            "verify-project",
        ] {
            assert!(
                cargo_proxy_passthrough(&[command.into()]),
                "{command} should not start an mbx session"
            );
        }
        for command in [
            "build", "check", "test", "bench", "run", "rustc", "rustdoc", "doc", "clippy", "fix",
        ] {
            assert!(
                !cargo_proxy_passthrough(&[command.into()]),
                "{command} should start an mbx session"
            );
        }
    }

    #[test]
    fn cargo_shim_identification_and_disable_values_are_explicit() {
        assert!(is_cargo_shim_path(OsStr::new("/tmp/cargo")));
        assert!(is_cargo_shim_path(OsStr::new("cargo.exe")));
        assert!(!is_cargo_shim_path(OsStr::new("/tmp/mbx")));
        assert!(mbx_disabled_value(Some(OsStr::new("1"))));
        assert!(!mbx_disabled_value(Some(OsStr::new("0"))));
        assert!(!mbx_disabled_value(Some(OsStr::new("false"))));
        assert!(!mbx_disabled_value(None));
    }

    #[cfg(windows)]
    #[test]
    fn cargo_shim_paths_are_case_insensitive_on_windows() {
        assert!(is_cargo_shim_path(OsStr::new(r"C:\Tools\CARGO.EXE")));
        assert!(same_path(
            Path::new(r"C:\Users\Example\mbx\bin"),
            Path::new(r"c:\users\example\MBX\BIN")
        ));
    }

    #[test]
    fn cargo_resolution_excludes_the_shim_directory_and_prevents_recursion() {
        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&shim_dir, &real_dir]).unwrap();

        assert_eq!(
            resolve_real_cargo_from(&shim, Some(shim.as_os_str()), &path).unwrap(),
            real.into_os_string()
        );
    }

    #[test]
    fn cargo_resolution_uses_the_active_shim_when_the_configured_home_changed() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("new-home/bin/cargo");
        let shim_dir = directory.path().join("original-home/bin");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&shim_dir, &real_dir]).unwrap();

        let active = resolve_active_cargo_shim(&configured, None, Some(&path));
        assert_eq!(active, shim);
        let path = path_without_shim(&active, &path).unwrap();
        assert_eq!(
            resolve_real_cargo_from(&active, None, &path).unwrap(),
            real.into_os_string()
        );
    }

    #[test]
    fn configured_real_cargo_does_not_need_a_path_search() {
        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();

        assert_eq!(
            configured_real_cargo(&shim, Some(real.as_os_str())),
            Some(real.into_os_string())
        );
    }

    #[test]
    fn cargo_resolution_removes_the_shim_directory_from_child_path() {
        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let path = std::env::join_paths([&shim_dir, &real_dir]).unwrap();

        assert_eq!(
            std::env::split_paths(&path_without_shim(&shim, &path).unwrap()).collect::<Vec<_>>(),
            vec![real_dir]
        );
    }

    #[cfg(unix)]
    #[test]
    fn cargo_resolution_excludes_a_symlink_to_the_shim_directory() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let shim_link = directory.path().join("linked-shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        symlink(&shim_dir, &shim_link).unwrap();
        let shim = shim_dir.join("cargo");
        let real = real_dir.join("cargo");
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&shim_link, &real_dir]).unwrap();

        assert_eq!(
            resolve_real_cargo_from(&shim, None, &path).unwrap(),
            real.into_os_string()
        );
    }
}
