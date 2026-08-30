use super::cargo::{CARGO_TARGET_DIR_ENV, cargo_roots, exit_code};
use crate::config::Config;
use eyre::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, ExitCode};

const CARGO_SHIM_STEM: &str = "cargo";

/// Whether this process was installed under Cargo's name by `mbx setup`.
pub fn is_cargo_shim() -> bool {
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
    let current = std::env::current_exe().wrap_err("failed to locate the Cargo shim")?;
    let real_cargo = resolve_real_cargo(&current)?;
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
    // Every probe and the final child must name Cargo directly. Leaving the
    // shim on PATH is harmless once CARGO is absolute and avoids mutating the
    // environment inherited by build scripts.
    unsafe { std::env::set_var("CARGO", &real_cargo) };
    let (config, settings) = Config::load_for_cli()?;
    super::cargo::run(&config, &settings, &string_arguments)
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
    let mut cargo_arguments = arguments
        .iter()
        .take_while(|argument| argument.as_os_str() != OsStr::new("--"));
    if cargo_arguments.clone().any(|argument| {
        matches!(
            argument.to_str(),
            Some("--version" | "-V" | "--help" | "-h")
        )
    }) {
        return true;
    }
    let command = cargo_arguments.find_map(|argument| {
        let argument = argument.to_str()?;
        (!argument.starts_with('-') && !argument.starts_with('+')).then_some(argument)
    });
    matches!(
        command,
        Some(
            "help"
                | "new"
                | "init"
                | "add"
                | "remove"
                | "update"
                | "fetch"
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
        )
    )
}

fn same_path(left: &Path, right: &Path) -> bool {
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

fn resolve_real_cargo(current: &Path) -> Result<OsString> {
    let configured = std::env::var_os("CARGO");
    let path = std::env::var_os("PATH").ok_or_else(|| eyre::eyre!("PATH is not set"))?;
    resolve_real_cargo_from(current, configured.as_deref(), &path)
}

fn resolve_real_cargo_from(
    current: &Path,
    configured: Option<&OsStr>,
    path: &OsStr,
) -> Result<OsString> {
    let current_dir = current.parent();
    if let Some(configured) = configured {
        let configured_path = Path::new(&configured);
        if configured_path.is_absolute()
            && configured_path.is_file()
            && current_dir.is_none_or(|current_dir| {
                !same_path(configured_path.parent().unwrap(), current_dir)
            })
        {
            return Ok(configured.to_owned());
        }
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
}
