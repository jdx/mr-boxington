//! Cargo owns executable selection; mbx owns the lifetime of its build context.
use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const SHIM: &str = "mbx-launch";
const CAPTURE: &str = "MBX_LAUNCH_CAPTURE";
const FORWARD: &str = "MBX_LAUNCH_FORWARD";
const RESTORE: &str = "MBX_SESSION_RESTORE";
const LEASE: &str = "MBX_SESSION_LEASE";
const BUILD_PATH: &str = "MBX_SESSION_PATH";

type Environment = BTreeMap<OsString, OsString>;

fn current_environment() -> Environment {
    let variables = std::env::vars_os();
    // Windows names are case-insensitive: an inherited `Path` must match the
    // `PATH` override instead of being mistaken for absent.
    #[cfg(windows)]
    let variables = variables.map(|(name, value)| (name.to_ascii_uppercase(), value));
    variables.collect()
}

/// Snapshot before CLI dispatch changes PATH or CARGO.
static CALLER: std::sync::OnceLock<Environment> = std::sync::OnceLock::new();

/// Remember the environment to restore when a build later launches an application.
pub fn remember_caller() {
    CALLER.get_or_init(|| {
        let mut environment = restored_environment().unwrap_or_else(|_| current_environment());
        environment.remove(OsStr::new("MBX_CARGO_SHIM_MODE"));
        environment.remove(OsStr::new("MBX_CARGO_SHIM_PATH"));
        environment
    });
}

/// `rustc` is the compiler of the toolchain `cargo` belongs to, when mbx found
/// one; see [`super::cargo::toolchain_rustc`]. A plain launch still builds, so
/// it needs the same compiler a managed build gets.
pub(super) fn plain_launch(
    cargo: &OsStr,
    arguments: &[String],
    rustc: Option<&std::path::Path>,
) -> Result<ExitCode> {
    let mut command = Command::new(cargo);
    command.args(arguments);
    if let Some(caller) = CALLER.get() {
        command.env_clear().envs(caller);
        // The caller's snapshot predates this shim entry. Dropping the loop
        // guard's counters would restart them in every re-entry.
        for name in [
            super::shim::CARGO_SHIM_DEPTH_ENV,
            super::shim::CARGO_SHIM_CHAIN_ENV,
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
    // A `+toolchain` argument asks rustup for another toolchain, and the
    // caller's RUSTC or RUSTUP_TOOLCHAIN already settles which compiler runs;
    // any of them wins over Cargo's own.
    let toolchain_argument = arguments
        .iter()
        .take_while(|argument| argument.as_str() != "--")
        .any(|argument| argument.starts_with('+'));
    let caller_chose = ["RUSTC", "CARGO_BUILD_RUSTC", "RUSTUP_TOOLCHAIN"]
        .iter()
        .any(|name| {
            CALLER.get().map_or_else(
                || std::env::var_os(name).is_some(),
                |caller| caller.contains_key(OsStr::new(name)),
            )
        });
    if let Some(rustc) = rustc
        && !toolchain_argument
        && !caller_chose
    {
        command.env("RUSTC", rustc);
    }
    Ok(super::cargo::exit_code(command.status()?))
}

/// Re-enter from the original environment when an application launch starts a
/// new build, or when a previous session has ended. Compiler shims never call
/// this: their diagnostic streams still belong to the compiler.
pub fn recover_cli() -> Result<Option<ExitCode>> {
    if std::env::var_os(RESTORE).is_none() {
        return Ok(None);
    }
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let launching = matches!(cargo_subcommand(&arguments), Some("run" | "r"));
    let live = std::env::var_os(LEASE)
        .and_then(|path| std::fs::File::open(path).ok())
        .is_some_and(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    // Only the shim selected for this Cargo command may keep its launch
    // runner. A build script's explicit shim run needs its own capture.
    if live && (!launching || forwarding_launch()) {
        return Ok(None);
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(std::env::args_os().skip(1))
        .env_clear()
        .envs(restored_environment()?);
    if super::is_cargo_shim() {
        command.env("MBX_CARGO_SHIM_MODE", "1");
    }
    let status = command.status()?;
    Ok(Some(super::cargo::exit_code(status)))
}

/// Bind the handoff to the selected executable: when it is real Cargo, an
/// inherited marker must not authorize a build script's unrelated shim run.
fn forwarding_launch() -> bool {
    if !super::is_cargo_shim() {
        return false;
    }
    let Some(expected) = std::env::var(FORWARD)
        .ok()
        .and_then(|value| serde_json::from_str::<OsString>(&value).ok())
    else {
        return false;
    };
    let shim = std::env::var_os("MBX_CARGO_SHIM_PATH")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    shim.and_then(|path| path.canonicalize().ok())
        .is_some_and(|path| path.as_os_str() == expected)
}

/// Config overrides cannot be resolved by cargo-config2. Let Cargo handle
/// these launches with the caller's environment until it offers a stable
/// resolved-config API. In particular, never replace an unknown runner.
pub(super) fn needs_plain_launch(arguments: &[String]) -> bool {
    let args: Vec<_> = arguments
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .collect();
    matches!(cargo_subcommand(arguments), Some("run" | "r"))
        && args.iter().any(|arg| {
            arg.starts_with('+')
                || arg.as_str() == "--config"
                || arg.starts_with("--config=")
                || arg.as_str() == "-C"
        })
}

/// Locate the command without interpreting option values or program arguments
/// as subcommands. Cargo globals may precede a command through the Cargo shim.
pub(super) fn cargo_subcommand<T: AsRef<OsStr>>(arguments: &[T]) -> Option<&str> {
    cargo_subcommand_at(arguments).map(|(_, command)| command)
}

/// The subcommand and where it sits, so a caller can put an alias expansion in
/// its place and keep the rest of the command line.
pub(super) fn cargo_subcommand_at<T: AsRef<OsStr>>(arguments: &[T]) -> Option<(usize, &str)> {
    let mut arguments = arguments.iter().enumerate();
    while let Some((index, argument)) = arguments.next() {
        let argument = argument.as_ref().to_str()?;
        match argument {
            "--" => return None,
            "--color" | "--config" | "-Z" | "-C" | "--directory" => {
                arguments.next()?;
            }
            value if !value.starts_with('-') && !value.starts_with('+') => {
                return Some((index, value));
            }
            _ => {}
        }
    }
    None
}

/// Cargo's configuration, read the way Cargo reads it.
///
/// Cargo takes an empty `build.rustc-wrapper` to mean no wrapper: a workspace
/// writes `rustc-wrapper = ""` to cancel one it inherits from a parent
/// directory or the global configuration. cargo-config2 would run the empty
/// string as the wrapper program, so evaluating a `[target.'cfg(...)']`
/// section, which asks rustc for the target's cfg, failed to execute `""`.
pub(super) fn load_cargo_config() -> Result<cargo_config2::Config> {
    Ok(without_empty_wrappers(cargo_config2::Config::load()?))
}

#[derive(Default)]
pub(super) struct ConfiguredWrappers {
    pub rustc: Option<String>,
    pub workspace: Option<String>,
}

/// Resolve only the two wrappers that the build session must preserve.
///
/// `None` means Cargo has a wrapper override this resolver cannot safely
/// model; the caller should launch Cargo with its original arguments and
/// environment. Cargo's `--config` overrides are intentionally inspected only
/// for these keys. Other command-line configuration remains in the cached
/// path.
pub(super) fn configured_wrappers(
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> Option<ConfiguredWrappers> {
    let environment = effective_environment(environment);
    if !wrapper_environment_is_supported(&environment) {
        return None;
    }
    let rustc_masked = environment.contains_key(OsStr::new("RUSTC_WRAPPER"));
    let workspace_masked = environment.contains_key(OsStr::new("RUSTC_WORKSPACE_WRAPPER"));
    let rustc_needed = !rustc_masked;
    let workspace_needed = !workspace_masked;

    if cli_config_changes_wrappers(arguments, cwd, rustc_needed, workspace_needed) {
        return None;
    }

    let cargo_home = cargo_home(cwd, &environment);
    let root = config_root(cwd, arguments, cargo_home.as_deref());
    if (rustc_needed || workspace_needed)
        && !config_hierarchy_is_supported(&root, cargo_home.clone())
    {
        return None;
    }

    if !rustc_needed && !workspace_needed {
        return Some(ConfiguredWrappers::default());
    }

    let options = cargo_config2::ResolveOptions::default()
        .env(
            environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )
        .cargo_home(cargo_home);
    let config = cargo_config2::Config::load_with_options(&root, options).ok()?;
    let config = without_empty_wrappers(config);
    let rustc = match config.build.rustc_wrapper {
        Some(path) if rustc_needed => Some(wrapper_path_to_string(path)?),
        None => None,
        Some(_) => None,
    };
    let workspace = match config.build.rustc_workspace_wrapper {
        Some(path) if workspace_needed => Some(wrapper_path_to_string(path)?),
        None => None,
        Some(_) => None,
    };
    Some(ConfiguredWrappers {
        rustc: if rustc_needed { rustc } else { None },
        workspace: if workspace_needed { workspace } else { None },
    })
}

fn wrapper_path_to_string(path: PathBuf) -> Option<String> {
    path.into_os_string().into_string().ok()
}

fn wrapper_environment_is_supported(environment: &Environment) -> bool {
    ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"]
        .iter()
        .all(|key| {
            environment
                .get(OsStr::new(key))
                .is_none_or(|value| value.to_str().is_some())
        })
}

/// Build the exact environment Cargo inherits for a normal managed launch:
/// the current process environment, with the same overlay passed to
/// `Command::envs`. A recovered CLI has already restored its caller environment
/// before dispatch, so this also follows Cargo's recovery path without reading
/// a separate process snapshot or mutating `std::env`.
fn effective_environment(overlay: &BTreeMap<String, String>) -> Environment {
    let mut environment = current_environment();
    for (name, value) in overlay {
        let name = OsString::from(name);
        #[cfg(windows)]
        let name = name.to_ascii_uppercase();
        environment.insert(name, OsString::from(value));
    }
    environment
}

fn cargo_home(cwd: &Path, environment: &Environment) -> Option<PathBuf> {
    if let Some(home) = environment
        .get(OsStr::new("CARGO_HOME"))
        .filter(|home| !home.is_empty())
    {
        let home = PathBuf::from(home);
        return Some(if home.is_absolute() {
            home
        } else {
            cwd.join(home)
        });
    }

    #[cfg(windows)]
    let home = environment
        .get(OsStr::new("USERPROFILE"))
        .or_else(|| environment.get(OsStr::new("HOME")));
    #[cfg(unix)]
    let home = environment.get(OsStr::new("HOME"));
    #[cfg(not(any(windows, unix)))]
    let home = std::env::home_dir();

    home.map(PathBuf::from).map(|home| home.join(".cargo"))
}

/// Where Cargo starts configuration discovery. `cargo install` skips the
/// caller's directory: it starts at the package `--path` names, or at
/// `CARGO_HOME` without one.
fn config_root(cwd: &Path, arguments: &[String], cargo_home: Option<&Path>) -> PathBuf {
    let Some((index, "install")) = cargo_subcommand_at(arguments) else {
        return cwd.to_path_buf();
    };
    let mut options = arguments[index + 1..]
        .iter()
        .take_while(|argument| argument.as_str() != "--");
    while let Some(argument) = options.next() {
        let path = if argument == "--path" {
            options.next().map(String::as_str)
        } else {
            argument.strip_prefix("--path=")
        };
        if let Some(path) = path {
            return cwd.join(path);
        }
    }
    cargo_home.map_or_else(|| cwd.to_path_buf(), Path::to_path_buf)
}

fn config_hierarchy_is_supported(cwd: &Path, cargo_home: Option<PathBuf>) -> bool {
    cargo_config2::Walk::with_cargo_home(cwd, cargo_home)
        .all(|path| config_file_is_supported(path.as_ref(), 0))
}

/// Cargo also reads the files a configuration `include`s, which the wrapper
/// resolver does not. They are supported when they can be read and leave both
/// wrapper keys alone, so an include of unrelated settings keeps the session.
fn config_file_is_supported(path: &Path, depth: usize) -> bool {
    const MAX_INCLUDE_DEPTH: usize = 16;
    let Ok(source) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(config) = toml::from_str::<toml::Value>(&source) else {
        return false;
    };
    if depth > 0 && config_wrapper_keys(&config) != (false, false) {
        return false;
    }
    let Some(include) = config.get("include") else {
        return true;
    };
    if depth == MAX_INCLUDE_DEPTH {
        return false;
    }
    let entries = match include {
        toml::Value::Array(entries) => entries.iter().collect::<Vec<_>>(),
        entry => vec![entry],
    };
    let base = path.parent().unwrap_or(Path::new(""));
    entries.into_iter().all(|entry| {
        let (included, optional) = match entry {
            toml::Value::String(included) => (included.as_str(), false),
            toml::Value::Table(table) => match table.get("path").and_then(toml::Value::as_str) {
                Some(included) => (
                    included,
                    table
                        .get("optional")
                        .and_then(toml::Value::as_bool)
                        .unwrap_or(false),
                ),
                None => return false,
            },
            _ => return false,
        };
        let included = base.join(included);
        if optional && !included.exists() {
            return true;
        }
        config_file_is_supported(&included, depth + 1)
    })
}

fn cli_config_changes_wrappers(
    arguments: &[String],
    cwd: &Path,
    rustc_needed: bool,
    workspace_needed: bool,
) -> bool {
    let mut arguments = arguments
        .iter()
        .take_while(|argument| argument.as_str() != "--");
    while let Some(argument) = arguments.next() {
        let value = if argument == "--config" {
            let Some(value) = arguments.next() else {
                return rustc_needed || workspace_needed;
            };
            value.as_str()
        } else if let Some(value) = argument.strip_prefix("--config=") {
            value
        } else {
            continue;
        };

        let affected = if let Some((key, value)) = value.split_once('=') {
            let source = format!("{key} = {value}");
            match toml::from_str::<toml::Value>(&source) {
                // An included file can set either wrapper, as for a configuration file.
                Ok(config) if config.get("include").is_some() => {
                    return rustc_needed || workspace_needed;
                }
                Ok(config) => config_wrapper_keys(&config),
                Err(_) => (
                    rustc_needed && key_is_wrapper(key, "rustc-wrapper"),
                    workspace_needed && key_is_wrapper(key, "rustc-workspace-wrapper"),
                ),
            }
        } else {
            let path = PathBuf::from(value);
            let path = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            let Ok(source) = std::fs::read_to_string(path) else {
                return rustc_needed || workspace_needed;
            };
            let Ok(config) = toml::from_str::<toml::Value>(&source) else {
                return rustc_needed || workspace_needed;
            };
            if config.get("include").is_some() && (rustc_needed || workspace_needed) {
                return true;
            }
            config_wrapper_keys(&config)
        };
        if (rustc_needed && affected.0) || (workspace_needed && affected.1) {
            return true;
        }
    }
    false
}

fn key_is_wrapper(key: &str, wrapper: &str) -> bool {
    let key = key
        .trim()
        .trim_matches(|character| character == '\'' || character == '"');
    key == format!("build.{wrapper}")
}

fn config_wrapper_keys(config: &toml::Value) -> (bool, bool) {
    let Some(build) = config.get("build").and_then(toml::Value::as_table) else {
        return (false, false);
    };
    (
        build.contains_key("rustc-wrapper"),
        build.contains_key("rustc-workspace-wrapper"),
    )
}

pub(super) fn without_empty_wrappers(mut config: cargo_config2::Config) -> cargo_config2::Config {
    for wrapper in [
        &mut config.build.rustc_wrapper,
        &mut config.build.rustc_workspace_wrapper,
    ] {
        if wrapper
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            *wrapper = None;
        }
    }
    config
}

/// The targets a Cargo command builds for: its `--target` flags, or else the
/// configured or host target.
pub(super) fn requested_targets(
    config: &cargo_config2::Config,
    arguments: &[String],
) -> Result<Vec<cargo_config2::TargetTriple>> {
    let mut targets = Vec::new();
    let mut args = arguments.iter().take_while(|arg| arg.as_str() != "--");
    while let Some(arg) = args.next() {
        if arg == "--target" {
            if let Some(target) = args.next() {
                targets.push(target.clone());
            }
        } else if let Some(target) = arg.strip_prefix("--target=") {
            targets.push(target.to_owned());
        }
    }
    Ok(config.build_target_for_config(&targets)?)
}

/// The environment variable that overrides Cargo's runner for one target.
pub(super) fn runner_key(target: &cargo_config2::TargetTriple) -> String {
    format!(
        "CARGO_TARGET_{}_RUNNER",
        target.triple().replace(['-', '.'], "_").to_uppercase()
    )
}

pub(super) fn lease(
    directory: &Path,
    environment: &mut BTreeMap<String, String>,
) -> Result<std::fs::File> {
    let path = directory.join("owner.lock");
    let file = std::fs::File::create(&path)?;
    file.lock()?;
    environment.insert(LEASE.into(), path.to_string_lossy().into_owned());
    Ok(file)
}

/// Only values overwritten by mbx are restored. Cargo's own launch environment
/// (notably dynamic-library paths and CARGO_MANIFEST_DIR) must survive.
#[derive(Serialize, Deserialize)]
struct Restore(Vec<(OsString, Option<OsString>)>);

pub(super) fn record_overlay(environment: &mut BTreeMap<String, String>) -> Result<()> {
    let current = current_environment();
    let caller = CALLER.get().unwrap_or(&current);
    let build_path = environment
        .get("PATH")
        .map(OsString::from)
        .or_else(|| current.get(OsStr::new("PATH")).cloned());
    environment.insert(BUILD_PATH.into(), serde_json::to_string(&build_path)?);
    let mut restore = BTreeMap::new();
    // A nested explicit mbx command may replace only part of its parent's
    // overlay. Carry the other keys too, so recovery cannot strand an outer
    // runner or compiler adapter in a later application.
    if let Some(encoded) = current.get(OsStr::new(RESTORE)) {
        let previous: Restore = serde_json::from_str(&encoded.to_string_lossy())?;
        for (name, _) in previous.0 {
            restore.insert(name.clone(), caller.get(&name).cloned());
        }
    }
    for name in environment
        .keys()
        .map(OsString::from)
        .chain(["PATH", "CARGO"].into_iter().map(OsString::from))
    {
        restore.insert(name.clone(), caller.get(&name).cloned());
    }
    restore.insert(RESTORE.into(), None);
    restore.insert(FORWARD.into(), None);
    environment.insert(
        RESTORE.into(),
        serde_json::to_string(&Restore(restore.into_iter().collect()))?,
    );
    Ok(())
}

/// A forwarding Cargo shim removes its directory from PATH. Track that new
/// inherited suffix so application launch can still restore the caller's PATH
/// while keeping any runtime library paths Cargo adds ahead of it.
pub(super) fn forward_cargo_environment(command: &mut Command) -> Result<()> {
    // Consume the handoff before real Cargo can pass it to build scripts.
    command.env_remove(FORWARD);
    if std::env::var_os(BUILD_PATH).is_some() {
        command.env(
            BUILD_PATH,
            serde_json::to_string(&std::env::var_os("PATH"))?,
        );
    }
    Ok(())
}

fn restored_environment() -> Result<Environment> {
    let mut environment = current_environment();
    if let Some(encoded) = environment.remove(OsStr::new(RESTORE)) {
        let restore: Restore = serde_json::from_str(&encoded.to_string_lossy())?;
        for (name, value) in restore.0 {
            match value {
                Some(value) => {
                    environment.insert(name, value);
                }
                None => {
                    environment.remove(&name);
                }
            }
        }
    }
    environment.remove(OsStr::new(CAPTURE));
    environment.remove(OsStr::new(FORWARD));
    Ok(environment)
}

pub(super) struct Launch {
    capture: PathBuf,
    runner: Option<cargo_config2::PathAndArgs>,
    runner_key: String,
    shim: PathBuf,
}

impl Launch {
    pub(super) fn prepare(arguments: &[String], directory: &Path) -> Result<Option<Self>> {
        let args: Vec<_> = arguments
            .iter()
            .take_while(|arg| arg.as_str() != "--")
            .collect();
        if args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
        {
            return Ok(None);
        }
        if !matches!(cargo_subcommand(arguments), Some("run" | "r")) {
            return Ok(None);
        }
        let config = load_cargo_config()?;
        let targets = requested_targets(&config, arguments)?;
        eyre::ensure!(targets.len() == 1, "cargo run requires exactly one target");
        let target = &targets[0];
        let runner = config.runner(target)?;
        let runner_key = runner_key(target);
        let shim = crate::session::install_shim_named(
            &std::env::current_exe()?,
            directory,
            SHIM,
            crate::session::ShimLink::Tracking,
        )?;
        Ok(Some(Self {
            capture: directory.join("launch.json"),
            runner,
            runner_key,
            shim,
        }))
    }

    pub(super) fn environment(
        &self,
        environment: &mut BTreeMap<String, String>,
        cargo: &OsStr,
    ) -> Result<()> {
        // Cargo splits environment runner strings on whitespace. Resolve the
        // shim through PATH so a temporary directory containing spaces works.
        let path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(self.shim.parent().unwrap().to_path_buf())
                .chain(std::env::split_paths(&path)),
        )?;
        environment.insert("PATH".into(), path.to_string_lossy().into_owned());
        environment.insert(self.runner_key.clone(), SHIM.into());
        environment.insert(CAPTURE.into(), self.capture.to_string_lossy().into_owned());
        // Without a resolvable Cargo the launch still runs; a shim then just
        // starts a fresh scope, as before the handoff existed.
        if let Some(cargo) = which::which(cargo)
            .ok()
            .and_then(|path| path.canonicalize().ok())
        {
            environment.insert(
                FORWARD.into(),
                serde_json::to_string(&cargo.into_os_string())?,
            );
        }
        Ok(())
    }

    pub(super) fn was_captured(&self) -> bool {
        self.capture.is_file()
    }

    pub(super) fn run(&self) -> Result<ExitCode> {
        let captured: Captured = serde_json::from_slice(
            &std::fs::read(&self.capture)
                .wrap_err("Cargo did not hand off the application launch")?,
        )?;
        std::fs::remove_file(&self.capture)?;
        let mut args = captured.arguments.into_iter();
        let executable = args
            .next()
            .ok_or_else(|| eyre::eyre!("Cargo supplied no executable"))?;
        let mut command = if let Some(runner) = &self.runner {
            let mut command = Command::new(&runner.path);
            command.args(&runner.args).arg(executable);
            command
        } else {
            Command::new(executable)
        };
        command
            .args(args)
            .current_dir(captured.directory)
            .env_clear()
            .envs(captured.environment);
        Ok(super::cargo::exit_code(
            command
                .status()
                .wrap_err("failed to launch Cargo application")?,
        ))
    }
}

#[derive(Serialize, Deserialize)]
struct Captured {
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    directory: PathBuf,
}

fn application_environment() -> Result<Environment> {
    let mut restored = restored_environment()?;
    // Cargo supplies its own executable path to the application, independently
    // of the CARGO value mbx changed during compiler dispatch.
    if let Some(cargo) = std::env::var_os("CARGO") {
        restored.insert("CARGO".into(), cargo);
    }
    let build_path: Option<OsString> = std::env::var(BUILD_PATH)
        .ok()
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .flatten();
    if let (Some(build), Some(launch)) = (build_path, std::env::var_os("PATH")) {
        let original = restored
            .get(OsStr::new("PATH"))
            .cloned()
            .unwrap_or_default();
        restored.insert(
            "PATH".into(),
            restore_launch_path(&build, &launch, &original)?,
        );
    }
    Ok(restored)
}

fn restore_launch_path(build: &OsStr, launch: &OsStr, original: &OsStr) -> Result<OsString> {
    let build: Vec<_> = std::env::split_paths(build).collect();
    let mut launch: Vec<_> = std::env::split_paths(launch).collect();
    // On Windows Cargo adds native DLL search paths ahead of the build PATH.
    // Replace only the inherited suffix, retaining those runtime additions.
    if launch.ends_with(&build) {
        launch.truncate(launch.len() - build.len());
        launch.extend(std::env::split_paths(original));
    }
    Ok(std::env::join_paths(launch)?)
}

/// Capture Cargo's selected executable before normal mbx CLI dispatch.
pub fn dispatch() -> Option<ExitCode> {
    if std::env::args_os()
        .next()
        .is_none_or(|arg| Path::new(&arg).file_stem() != Some(OsStr::new(SHIM)))
    {
        return None;
    }
    let result = (|| -> Result<()> {
        let path = std::env::var_os(CAPTURE)
            .ok_or_else(|| eyre::eyre!("missing Cargo launch destination"))?;
        let captured = Captured {
            arguments: std::env::args_os().skip(1).collect(),
            environment: application_environment()?.into_iter().collect(),
            directory: std::env::current_dir()?,
        };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        serde_json::to_writer(options.open(path)?, &captured)?;
        Ok(())
    })();
    Some(match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mbx[error]: failed to capture Cargo launch: {error:#}");
            ExitCode::FAILURE
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_overlays_keep_keys_changed_only_by_the_outer_session() {
        const CHILD: &str = "MBX_TEST_NESTED_OVERLAY_CHILD";
        const OUTER: &str = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER";
        if std::env::var_os(CHILD).is_some() {
            remember_caller();
            let mut overlay = BTreeMap::from([("MBX_BUILD".into(), "nested-build".into())]);
            record_overlay(&mut overlay).unwrap();
            let restore: Restore = serde_json::from_str(&overlay[RESTORE]).unwrap();
            assert!(restore.0.contains(&(OUTER.into(), None)));
            return;
        }
        let restore = Restore(vec![(OUTER.into(), None), (RESTORE.into(), None)]);
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cli::launch::tests::nested_overlays_keep_keys_changed_only_by_the_outer_session",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(OUTER, "outer-launch-shim")
            .env(RESTORE, serde_json::to_string(&restore).unwrap())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_paths_with_non_unicode_bytes_cannot_enter_the_managed_environment() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(OsString::from_vec(vec![b'w', 0xff]));
        assert_eq!(wrapper_path_to_string(path), None);
        for key in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"] {
            let mut environment = Environment::new();
            environment.insert(key.into(), OsString::new());
            assert!(wrapper_environment_is_supported(&environment));
            environment.insert(key.into(), OsString::from_vec(vec![b'w', 0xff]));
            assert!(!wrapper_environment_is_supported(&environment));
        }
    }

    #[cfg(windows)]
    #[test]
    fn configured_wrappers_resolve_windows_paths_and_respect_empty_environment_masks() {
        const CHILD_MODE: &str = "MBX_TEST_CONFIGURED_WRAPPERS_CHILD";
        const EXPECTED_RUSTC: &str = "MBX_TEST_EXPECTED_RUSTC_WRAPPER";
        const EXPECTED_WORKSPACE: &str = "MBX_TEST_EXPECTED_WORKSPACE_WRAPPER";

        if let Some(mode) = std::env::var_os(CHILD_MODE) {
            let mode = mode.to_str().expect("child mode should be Unicode");
            let expected_rustc = std::env::var(EXPECTED_RUSTC).unwrap();
            let expected_workspace = std::env::var(EXPECTED_WORKSPACE).unwrap();
            let mut environment = BTreeMap::new();
            let rustc_masked = mode == "mask";
            assert!(rustc_masked || mode == "resolve");
            if rustc_masked {
                environment.insert("RUSTC_WRAPPER".into(), String::new());
            }

            let configured =
                configured_wrappers(&std::env::current_dir().unwrap(), &[], &environment)
                    .expect("private Cargo config should be supported");
            assert_eq!(
                configured.rustc.as_deref(),
                (!rustc_masked).then_some(expected_rustc.as_str())
            );
            assert_eq!(
                configured.workspace.as_deref(),
                Some(expected_workspace.as_str())
            );
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let cargo_home = temp.path().join("private cargo home");
        let workspace = temp.path().join("workspace");
        let user_profile = temp.path().join("private profile");
        let temporary_directory = temp.path().join("private temporary");
        std::fs::create_dir_all(&cargo_home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&user_profile).unwrap();
        std::fs::create_dir_all(&temporary_directory).unwrap();

        let wrappers = temp.path().join("wrapper bin");
        let rustc_wrapper = wrappers.join("rustc wrapper.exe");
        let workspace_wrapper = wrappers.join("workspace wrapper.exe");
        for path in [&rustc_wrapper, &workspace_wrapper] {
            let path = path.to_str().expect("temporary paths should be Unicode");
            assert!(Path::new(path).is_absolute());
            assert!(path.contains('\\'));
            assert!(path.contains(' '));
        }

        let config = format!(
            "[build]\nrustc-wrapper = '{}'\nrustc-workspace-wrapper = '{}'\n",
            rustc_wrapper.display(),
            workspace_wrapper.display()
        );
        std::fs::write(cargo_home.join("config.toml"), config).unwrap();

        let test_name = "cli::launch::tests::configured_wrappers_resolve_windows_paths_and_respect_empty_environment_masks";
        for mode in ["resolve", "mask"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg(test_name)
                .arg("--nocapture")
                .current_dir(&workspace)
                .env_clear()
                .env("SystemRoot", std::env::var_os("SystemRoot").unwrap())
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("TEMP", &temporary_directory)
                .env("TMP", &temporary_directory)
                .env("USERPROFILE", &user_profile)
                .env("HOME", &user_profile)
                .env("CARGO_HOME", &cargo_home)
                .env(CHILD_MODE, mode)
                .env(EXPECTED_RUSTC, &rustc_wrapper)
                .env(EXPECTED_WORKSPACE, &workspace_wrapper)
                .output()
                .expect("configured-wrapper child should start");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let transcript = format!("{stdout}{stderr}");
            assert!(
                output.status.success(),
                "configured-wrapper child {mode} failed:\n{transcript}"
            );
            assert!(
                transcript.contains("running 1 test"),
                "configured-wrapper child {mode} did not run exactly one test:\n{transcript}"
            );
            assert!(
                transcript.contains("1 passed; 0 failed"),
                "configured-wrapper child {mode} did not pass its test:\n{transcript}"
            );
            println!("configured-wrapper child {mode} output:\n{transcript}");
        }
    }

    #[test]
    fn run_values_do_not_disable_caching_for_other_commands() {
        for args in [
            vec!["+nightly", "test", "run"],
            vec!["+stable", "build", "--bin", "run"],
            vec!["+nightly", "build", "--features", "run"],
            vec!["--config", "run", "check"],
        ] {
            assert!(!needs_plain_launch(
                &args.iter().map(|s| s.to_string()).collect::<Vec<_>>()
            ));
        }
        assert_eq!(cargo_subcommand(&["--color", "always", "run"]), Some("run"));
        assert_eq!(
            cargo_subcommand(&["-Z", "unstable-options", "--directory", "run", "check"]),
            Some("check")
        );
        assert!(needs_plain_launch(&[
            "--config".into(),
            "a.toml".into(),
            "run".into()
        ]));
    }

    #[test]
    fn config_includes_keep_the_session_unless_they_change_wrappers() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let project = root.path().join("project");
        let cargo = project.join(".cargo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(cargo.join("nested")).unwrap();
        let supported = |config: &str| {
            std::fs::write(cargo.join("config.toml"), config).unwrap();
            config_hierarchy_is_supported(&project, Some(home.clone()))
        };
        std::fs::write(cargo.join("plain.toml"), "[term]\nquiet = true\n").unwrap();
        std::fs::write(
            cargo.join("nested/wrapper.toml"),
            "[build]\nrustc-wrapper = 'w'\n",
        )
        .unwrap();
        std::fs::write(
            cargo.join("chain.toml"),
            "include = 'nested/wrapper.toml'\n",
        )
        .unwrap();
        std::fs::write(cargo.join("invalid.toml"), "[build\n").unwrap();

        assert!(supported("include = 'plain.toml'\n"));
        assert!(supported("include = ['plain.toml']\n"));
        assert!(supported(
            "include = [{ path = 'missing.toml', optional = true }, 'plain.toml']\n"
        ));
        assert!(!supported("include = 'missing.toml'\n"));
        assert!(!supported("include = 'invalid.toml'\n"));
        assert!(!supported("include = 'nested/wrapper.toml'\n"));
        assert!(!supported("include = 'chain.toml'\n"));
        assert!(!supported("include = [{ optional = true }]\n"));
        std::fs::write(cargo.join("cycle.toml"), "include = 'cycle.toml'\n").unwrap();
        assert!(!supported("include = 'cycle.toml'\n"));
    }

    #[test]
    fn install_reads_configuration_where_cargo_does() {
        let cwd = Path::new("caller");
        let home = Path::new("home");
        let arguments = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            config_root(cwd, &arguments(&["build"]), Some(home)),
            cwd.to_path_buf()
        );
        assert_eq!(
            config_root(cwd, &arguments(&["install", "ripgrep"]), Some(home)),
            home.to_path_buf()
        );
        for install in [
            &["install", "--path", "tool"][..],
            &["install", "--path=tool"],
            &["+stable", "install", "--locked", "--path", "tool"],
        ] {
            assert_eq!(
                config_root(cwd, &arguments(install), Some(home)),
                cwd.join("tool"),
                "{install:?}"
            );
        }
        assert_eq!(
            config_root(
                cwd,
                &arguments(&["install", "ripgrep", "--", "--path", "tool"]),
                Some(home)
            ),
            home.to_path_buf()
        );
    }

    #[test]
    fn runtime_search_paths_survive_restoring_the_caller_path() {
        let path = |parts: &[&str]| std::env::join_paths(parts).unwrap();
        assert_eq!(
            restore_launch_path(
                &path(&["shim", "tools"]),
                &path(&["deps", "sysroot", "shim", "tools"]),
                &path(&["cargo-proxy", "tools"])
            )
            .unwrap(),
            path(&["deps", "sysroot", "cargo-proxy", "tools"])
        );
        assert_eq!(
            restore_launch_path(
                &path(&["shim", "tools"]),
                &path(&["custom"]),
                &path(&["tools"])
            )
            .unwrap(),
            path(&["custom"])
        );
    }
}
