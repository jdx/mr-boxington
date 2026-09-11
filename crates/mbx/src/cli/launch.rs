//! Cargo owns executable selection; mbx owns the lifetime of its build context.
use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const SHIM: &str = "mbx-launch";
const CAPTURE: &str = "MBX_LAUNCH_CAPTURE";
const RESTORE: &str = "MBX_SESSION_RESTORE";
const LEASE: &str = "MBX_SESSION_LEASE";

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

pub(super) fn plain_launch(cargo: &OsStr, arguments: &[String]) -> Result<ExitCode> {
    let mut command = Command::new(cargo);
    command.args(arguments);
    if let Some(caller) = CALLER.get() {
        command.env_clear().envs(caller);
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
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let launching = arguments
        .iter()
        .find(|arg| !arg.starts_with('-') && !arg.starts_with('+'))
        .is_some_and(|arg| matches!(arg.as_str(), "run" | "r"));
    let live = std::env::var_os(LEASE)
        .and_then(|path| std::fs::File::open(path).ok())
        .is_some_and(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    if live && !launching {
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

/// Config overrides cannot be resolved by cargo-config2. Let Cargo handle
/// these launches with the caller's environment until it offers a stable
/// resolved-config API. In particular, never replace an unknown runner.
pub(super) fn needs_plain_launch(arguments: &[String]) -> bool {
    let args: Vec<_> = arguments
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .collect();
    args.iter().any(|arg| matches!(arg.as_str(), "run" | "r"))
        && args.iter().any(|arg| {
            arg.starts_with('+')
                || arg.as_str() == "--config"
                || arg.starts_with("--config=")
                || arg.as_str() == "-C"
        })
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
    environment.insert(
        RESTORE.into(),
        serde_json::to_string(&Restore(restore.into_iter().collect()))?,
    );
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
        let command = args
            .iter()
            .find(|arg| !arg.starts_with('+') && !arg.starts_with('-'));
        if !command.is_some_and(|arg| matches!(arg.as_str(), "run" | "r")) {
            return Ok(None);
        }
        let config = cargo_config2::Config::load()?;
        let mut targets = Vec::new();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--target" {
                if let Some(target) = args.next() {
                    targets.push(target.clone());
                }
            } else if let Some(target) = arg.strip_prefix("--target=") {
                targets.push(target.to_owned());
            }
        }
        let targets = config.build_target_for_config(&targets)?;
        eyre::ensure!(targets.len() == 1, "cargo run requires exactly one target");
        let target = &targets[0];
        let runner = config.runner(target)?;
        let runner_key = format!(
            "CARGO_TARGET_{}_RUNNER",
            target.triple().replace('-', "_").to_uppercase()
        );
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

    pub(super) fn environment(&self, environment: &mut BTreeMap<String, String>) -> Result<()> {
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

/// Capture Cargo's selected executable before normal mbx CLI dispatch.
pub fn dispatch() -> Option<ExitCode> {
    if !std::env::args_os()
        .next()
        .is_some_and(|arg| Path::new(&arg).file_stem() == Some(OsStr::new(SHIM)))
    {
        return None;
    }
    let result = (|| -> Result<()> {
        let path = std::env::var_os(CAPTURE)
            .ok_or_else(|| eyre::eyre!("missing Cargo launch destination"))?;
        let captured = Captured {
            arguments: std::env::args_os().skip(1).collect(),
            environment: restored_environment()?.into_iter().collect(),
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
