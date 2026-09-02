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
    let reported_shim = std::env::var_os(CARGO_SHIM_PATH_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            invoked_as_cargo()
                .then(|| std::env::current_exe().ok())
                .flatten()
        });
    // mise removes its wrapper directory before launching mbx, so there is no
    // shim path to exclude. Treating the first remaining Cargo as a shim would
    // instead remove the real tool from PATH.
    let invoked_via_mise_wrapper = !invoked_as_cargo() && reported_shim.is_none();
    // The Windows launcher uses this only to select dispatch in the target mbx.
    // Do not leak it into Cargo, build scripts, or nested mbx commands.
    unsafe { std::env::remove_var(CARGO_SHIM_MODE_ENV) };
    unsafe { std::env::remove_var(CARGO_SHIM_PATH_ENV) };
    #[cfg(windows)]
    if invoked_as_cargo()
        && let Some(code) = forward_to_current_mbx(reported_shim.as_deref())?
    {
        return Ok(code);
    }
    let shim_dir = super::setup_install_dir()
        .ok_or_else(|| eyre::eyre!("the platform data directory could not be located"))?;
    let configured_shim = shim_dir.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    let real_cargo = if invoked_via_mise_wrapper {
        resolve_real_cargo(&configured_shim)?
    } else {
        let shim = resolve_active_cargo_shim(
            &configured_shim,
            reported_shim.as_deref().map(Path::as_os_str),
            std::env::var_os("PATH").as_deref(),
        );
        exclude_shim_from_path(&shim)?;
        resolve_real_cargo(&shim)?
    };
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if mbx_disabled() {
        preserve_native_compiler_paths();
        return run_real_cargo(&real_cargo, &arguments);
    }
    if enclosing_session(std::env::var_os(crate::session::SOCKET_ENV).as_deref())
        || cargo_proxy_passthrough(&arguments)
    {
        return run_real_cargo(&real_cargo, &arguments);
    }
    let Ok(string_arguments) = super::strings(&arguments) else {
        return run_real_cargo(&real_cargo, &arguments);
    };
    let Some(roots) = cargo_roots(
        &real_cargo,
        &string_arguments,
        std::env::var_os(CARGO_TARGET_DIR_ENV).as_deref(),
    ) else {
        return run_real_cargo(
            &real_cargo,
            &string_arguments
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>(),
        );
    };
    // Every probe and the final child must name Cargo directly.
    unsafe { std::env::set_var("CARGO", &real_cargo) };
    let (config, settings) = Config::load_for_cli()?;
    super::cargo::run_with_roots(&config, &settings, &string_arguments, roots)
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
fn forward_to_current_mbx(reported_shim: Option<&Path>) -> Result<Option<ExitCode>> {
    let Some(install_dir) = super::setup_install_dir() else {
        return Ok(None);
    };
    let Some(target) = super::setup::cargo_shim_target(&install_dir) else {
        return Ok(None);
    };
    let mut command = Command::new(&target);
    command
        .args(std::env::args_os().skip(1))
        .env(CARGO_SHIM_MODE_ENV, "1");
    if let Some(reported_shim) = reported_shim {
        command.env(CARGO_SHIM_PATH_ENV, reported_shim);
    }
    let status = command
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

/// Keep native build-system configuration reusable while bypassing the cache.
///
/// CMake records the absolute compiler path it sees. An mbx build points host
/// C and C++ compilation at persistent shims, so handing a later disabled
/// build the platform compiler directly makes CMake invalidate and reconfigure
/// the same build directory. The persistent shim is already transparent
/// without a session; preserve its path here without starting an agent or
/// caching any compilation.
fn preserve_native_compiler_paths() {
    if crate::session::CC_CRATE_ENV
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return;
    }
    let Ok((config, _)) = Config::load_for_cli() else {
        return;
    };
    if !config.cc {
        return;
    }
    let shims = config.cache_dir.join("shims");
    for (variable, stem) in [
        ("HOST_CC", crate::session::CC_SHIM_STEM),
        ("HOST_CXX", crate::session::CXX_SHIM_STEM),
    ] {
        let shim = shims.join(crate::session::shim_file_name(stem));
        if shim.is_file() {
            // SAFETY: Cargo shim dispatch is single-threaded before its child
            // process starts.
            unsafe { std::env::set_var(variable, shim) };
        }
    }
}

fn enclosing_session(session_socket: Option<&OsStr>) -> bool {
    // Cargo subcommands such as watch and nextest can invoke Cargo again. The
    // outer session's compiler wrapper and agent socket are already inherited,
    // so starting another session here would pay the setup cost for every
    // nested invocation and replace the long-lived watch session's context.
    session_socket.is_some_and(|socket| !socket.is_empty())
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

/// Keep explicit `mbx build`-style commands from rediscovering a persistent
/// Cargo shim or mise command wrapper on PATH.
pub(super) fn prepare_explicit_cargo() -> Result<()> {
    let standalone_shim = super::setup_install_dir()
        .map(|directory| directory.join(if cfg!(windows) { "cargo.exe" } else { "cargo" }))
        .filter(|shim| shim.is_file());
    let Some(path) = std::env::var_os("PATH") else {
        return Ok(());
    };
    let (path, proxy) = path_without_cargo_proxies(&path, standalone_shim.as_deref())?;
    let Some(proxy) = proxy else {
        return Ok(());
    };
    let cargo = resolve_explicit_cargo(
        &proxy,
        standalone_shim.as_deref(),
        std::env::var_os("CARGO").as_deref(),
        &path,
    )?;
    PATH_BEFORE_SHIM_EXCLUSION.get_or_init(|| std::env::var_os("PATH").unwrap_or_default());
    // SAFETY: CLI dispatch is single-threaded here, before the cache session
    // or any child process has started.
    unsafe {
        std::env::set_var("PATH", path);
        std::env::set_var("CARGO", cargo);
    }
    Ok(())
}

fn path_without_cargo_proxies(
    path: &OsStr,
    standalone_shim: Option<&Path>,
) -> Result<(OsString, Option<PathBuf>)> {
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    let mut first_proxy = None;
    let directories = std::env::split_paths(path).filter(|directory| {
        let candidate = directory.join(name);
        let is_standalone = standalone_shim.is_some_and(|shim| same_path(&candidate, shim));
        let is_proxy =
            candidate.is_file() && (is_standalone || is_mise_command_wrapper_path(&candidate));
        if is_proxy {
            first_proxy.get_or_insert(candidate);
        }
        !is_proxy
    });
    Ok((std::env::join_paths(directories)?, first_proxy))
}

/// Resolve Cargo for an explicit command without accepting its standalone shim.
fn resolve_explicit_cargo(
    proxy: &Path,
    standalone_shim: Option<&Path>,
    configured: Option<&OsStr>,
    path: &OsStr,
) -> Result<OsString> {
    if let Some(configured) = configured.map(Path::new).filter(|candidate| {
        candidate.is_absolute()
            && candidate.is_file()
            && !is_mise_command_wrapper_path(candidate)
            && standalone_shim.is_none_or(|shim| !same_path(candidate, shim))
    }) {
        return Ok(configured.as_os_str().to_owned());
    }
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    for directory in std::env::split_paths(path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate.into_os_string());
        }
    }
    eyre::bail!(
        "could not find the real Cargo after excluding {}",
        proxy.display()
    )
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
        && !is_mise_cargo_proxy_path(configured)
        && current_dir.is_none_or(|current_dir| {
            configured
                .parent()
                .is_some_and(|parent| !same_path(parent, current_dir))
        }))
    .then(|| configured.as_os_str().to_owned())
}

pub(crate) fn is_mise_command_wrapper_path(path: &Path) -> bool {
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    path.ends_with(Path::new("command-wrappers").join("bin").join(name))
}

fn is_mise_cargo_proxy_path(path: &Path) -> bool {
    if is_mise_command_wrapper_path(path) {
        return true;
    }
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    let Some(shims) = path.parent().filter(|parent| {
        parent.file_name() == Some(OsStr::new("shims"))
            && path.file_name() == Some(OsStr::new(name))
    }) else {
        return false;
    };
    let Some(data_dir) = shims.parent() else {
        return false;
    };
    data_dir.file_name() == Some(OsStr::new("mise"))
        || std::env::var_os("MISE_DATA_DIR")
            .is_some_and(|configured| same_path(data_dir, Path::new(&configured)))
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
        if candidate.is_file() && !is_mise_cargo_proxy_path(&candidate) {
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
    fn cargo_shim_reuses_an_enclosing_session() {
        assert!(enclosing_session(Some(OsStr::new("session-socket"))));
        assert!(!enclosing_session(Some(OsStr::new(""))));
        assert!(!enclosing_session(None));
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

    #[cfg(windows)]
    #[test]
    fn reported_windows_shim_wins_when_real_cargo_precedes_it_on_path() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("new-home/bin/cargo.exe");
        let shim_dir = directory.path().join("shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let shim = shim_dir.join("cargo.exe");
        let real = real_dir.join("cargo.exe");
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&real_dir, &shim_dir]).unwrap();

        let active = resolve_active_cargo_shim(&configured, Some(shim.as_os_str()), Some(&path));
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
    fn cargo_resolution_rejects_mise_proxies_as_the_real_cargo() {
        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let wrapper_dir = directory.path().join("command-wrappers/bin");
        let mise_shims_dir = directory.path().join("mise/shims");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        std::fs::create_dir_all(&mise_shims_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let wrapper = wrapper_dir.join(name);
        let mise_shim = mise_shims_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&wrapper, b"wrapper").unwrap();
        std::fs::write(&mise_shim, b"mise shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&wrapper_dir, &mise_shims_dir, &real_dir]).unwrap();

        assert_eq!(
            configured_real_cargo(&shim, Some(wrapper.as_os_str())),
            None
        );
        assert_eq!(
            configured_real_cargo(&shim, Some(mise_shim.as_os_str())),
            None
        );
        assert_eq!(
            resolve_real_cargo_from(&shim, Some(mise_shim.as_os_str()), &path).unwrap(),
            real.into_os_string()
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

    #[test]
    fn explicit_cargo_excludes_mise_wrapper_from_child_path() {
        let directory = tempfile::tempdir().unwrap();
        let wrapper_dir = directory.path().join("command-wrappers/bin");
        let shim_dir = directory.path().join("mbx/bin");
        let mise_shims_dir = directory.path().join("mise/shims");
        let tool_dir = directory.path().join("tool/bin");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&mise_shims_dir).unwrap();
        std::fs::create_dir_all(&tool_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let wrapper = wrapper_dir.join(name);
        let shim = shim_dir.join(name);
        let mise_shim = mise_shims_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&wrapper, b"mise wrapper").unwrap();
        std::fs::write(&shim, b"mbx shim").unwrap();
        std::fs::write(&mise_shim, b"mise shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([
            &wrapper_dir,
            &shim_dir,
            &mise_shims_dir,
            &tool_dir,
            &real_dir,
        ])
        .unwrap();

        let (path, proxy) = path_without_cargo_proxies(&path, Some(&shim)).unwrap();

        assert_eq!(proxy, Some(wrapper.clone()));
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            vec![mise_shims_dir, tool_dir, real_dir]
        );
        assert_eq!(
            resolve_explicit_cargo(&wrapper, Some(&shim), Some(shim.as_os_str()), &path).unwrap(),
            mise_shim.into_os_string()
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
