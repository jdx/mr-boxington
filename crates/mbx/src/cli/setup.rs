use eyre::{Context, Result};
use std::ffi::OsStr;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const CARGO_SHIM_STEM: &str = "cargo";

#[derive(usage::Args)]
pub(super) struct SetupArgs {
    /// Accept the recommended activation scope without prompting.
    #[usage(long)]
    pub(super) yes: bool,
    /// Activate the Cargo shim in mise's global configuration.
    #[usage(long)]
    pub(super) global: bool,
    /// Activate the Cargo shim in the current project's mise configuration.
    #[usage(long)]
    pub(super) local: bool,
    /// Report whether plain Cargo integration is installed and current.
    #[usage(long)]
    pub(super) status: bool,
    /// Remove mbx activation from the selected scope.
    #[usage(long)]
    pub(super) uninstall: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetupAction {
    Install,
    Status,
    Uninstall,
}

impl SetupArgs {
    pub(super) fn action(&self) -> Result<SetupAction> {
        let selected = self.status as u8 + self.uninstall as u8;
        if selected > 1 {
            eyre::bail!("--status and --uninstall are mutually exclusive");
        }
        Ok(if self.status {
            SetupAction::Status
        } else if self.uninstall {
            SetupAction::Uninstall
        } else {
            SetupAction::Install
        })
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.global && self.local {
            eyre::bail!("--global and --local are mutually exclusive");
        }
        if self.yes && (self.global || self.local) {
            eyre::bail!("--yes cannot be combined with --global or --local");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MiseScope {
    File(PathBuf),
    Global,
    Local,
    None,
}

impl std::fmt::Display for MiseScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => write!(formatter, "{}", path.display()),
            Self::Global => formatter.write_str("global"),
            Self::Local => formatter.write_str("local"),
            Self::None => formatter.write_str("none"),
        }
    }
}

pub(super) fn run(args: &SetupArgs, action: SetupAction) -> Result<ExitCode> {
    args.validate()?;
    let executable = std::env::current_exe().wrap_err("failed to locate the mbx executable")?;
    let install_dir = setup_install_dir()
        .ok_or_else(|| eyre::eyre!("the platform data directory could not be located"))?;
    let scope = setup_scope(args, action)?;
    setup_at_action(&executable, &install_dir, &scope, action)
}

/// The stable directory that holds the Cargo shim installed by setup.
pub(crate) fn setup_install_dir() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(directory) = std::env::var_os("MBX_TEST_SHIM_DIR") {
        return Some(directory.into());
    }
    Some(dirs::data_local_dir()?.join("mbx").join("bin"))
}

#[cfg(test)]
pub(super) fn setup_at(executable: &Path, install_dir: &Path) -> Result<()> {
    setup_at_action(
        executable,
        install_dir,
        &MiseScope::None,
        SetupAction::Install,
    )?;
    Ok(())
}

pub(super) fn setup_at_action(
    executable: &Path,
    install_dir: &Path,
    scope: &MiseScope,
    action: SetupAction,
) -> Result<ExitCode> {
    let shim = install_dir.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    let was_installed = shim.is_file();
    let configured_path = portable_home_path(install_dir);

    match action {
        SetupAction::Status => {
            if !shim.is_file() {
                println!("mbx setup is not installed: {} is missing", shim.display());
                return Ok(ExitCode::FAILURE);
            }
            if !cargo_shim_is_current(executable, &shim)? {
                println!("mbx setup is outdated; run `mbx setup`");
                return Ok(ExitCode::FAILURE);
            }
            if matches!(scope, MiseScope::None) || mise_path_is_configured(scope, &configured_path)?
            {
                println!("mbx setup is installed and current: {}", shim.display());
                return Ok(ExitCode::SUCCESS);
            }
            println!("mbx setup is installed but is not active in the selected mise config");
            return Ok(ExitCode::FAILURE);
        }
        SetupAction::Uninstall => {
            let activation_removed = !matches!(scope, MiseScope::None)
                && update_mise_path(scope, "--remove", &configured_path)?;
            if activation_removed {
                println!(
                    "removed mbx Cargo activation; {} was left in place for other scopes",
                    shim.display()
                );
            } else {
                println!("{} was left in place for other scopes", shim.display());
            }
            return Ok(ExitCode::SUCCESS);
        }
        SetupAction::Install => {
            std::fs::create_dir_all(install_dir)?;
            crate::session::install_shim_named(
                executable,
                install_dir,
                CARGO_SHIM_STEM,
                crate::session::ShimLink::Tracking,
            )?;
            write_cargo_shim_target(install_dir, executable)?;
        }
    }

    let activated = if !matches!(scope, MiseScope::None) {
        update_mise_path(scope, "--append", &configured_path)?
    } else {
        false
    };
    if activated {
        println!("plain cargo commands now run through mbx in this mise scope");
    } else if was_installed {
        println!("refreshed the Cargo shim at {}", shim.display());
    } else {
        println!("installed the Cargo shim at {}", shim.display());
        if matches!(scope, MiseScope::None) {
            print_manual_activation(install_dir);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn setup_scope(args: &SetupArgs, action: SetupAction) -> Result<MiseScope> {
    if let Some(path) = std::env::var_os("MISE_CONFIG_FILE")
        && (args.yes || (!args.global && !args.local && action != SetupAction::Install))
    {
        return Ok(MiseScope::File(path.into()));
    }
    if args.global {
        return Ok(MiseScope::Global);
    }
    if args.local {
        return Ok(MiseScope::Local);
    }
    if args.yes {
        return Ok(recommended_mise_scope().unwrap_or(MiseScope::None));
    }
    if action != SetupAction::Install {
        return Ok(recommended_mise_scope().unwrap_or(MiseScope::None));
    }
    if !mise_is_activated() {
        return Ok(MiseScope::None);
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Ok(MiseScope::None);
    }
    let recommended = recommended_mise_scope().unwrap_or(MiseScope::Global);
    use demand::{DemandOption, Select};
    let mut select = Select::new("Where should plain cargo commands use mbx?");
    match &recommended {
        MiseScope::Global => {
            select = select.option(
                DemandOption::new(MiseScope::Global)
                    .label("Everywhere mise is active")
                    .selected(true),
            );
            select =
                select.option(DemandOption::new(MiseScope::Local).label("Only in this project"));
        }
        MiseScope::File(_) => {
            select = select.option(
                DemandOption::new(recommended.clone())
                    .label("The project mise config that applies here")
                    .selected(true),
            );
            select = select
                .option(DemandOption::new(MiseScope::Global).label("Everywhere mise is active"));
        }
        MiseScope::Local | MiseScope::None => unreachable!("recommended scope is concrete"),
    }
    select = select
        .option(DemandOption::new(MiseScope::None).label("Create the shim without activating it"));
    match select.run() {
        Ok(scope) => Ok(scope),
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => Ok(MiseScope::None),
        Err(error) => Err(error.into()),
    }
}

fn mise_is_activated() -> bool {
    command_exists("mise") && std::env::var_os("MISE_SHELL").is_some()
}

fn recommended_mise_scope() -> Option<MiseScope> {
    if !mise_is_activated() {
        return None;
    }
    if let Some(path) = mbx_mise_config() {
        let global = mise_scope_config_path(&MiseScope::Global).ok();
        return Some(if global.as_deref() == Some(path.as_path()) {
            MiseScope::Global
        } else {
            MiseScope::File(path)
        });
    }
    nearest_project_config()
        .map(MiseScope::File)
        .or(Some(MiseScope::Global))
}

fn mbx_mise_config() -> Option<PathBuf> {
    let output = Command::new("mise")
        .args(["config", "ls", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    mbx_mise_config_from_json(&output.stdout)
}

pub(super) fn mbx_mise_config_from_json(json: &[u8]) -> Option<PathBuf> {
    let value = serde_json::from_slice::<serde_json::Value>(json).ok()?;
    value.as_array()?.iter().find_map(|config| {
        let tools = config.get("tools")?.as_array()?;
        let defines_mbx = tools
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|tool| {
                tool == "mr-boxington"
                    || tool
                        .rsplit([':', '/'])
                        .next()
                        .is_some_and(|name| name == "mr-boxington")
            });
        defines_mbx.then(|| config.get("path")?.as_str().map(PathBuf::from))?
    })
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| {
            directory
                .join(if cfg!(windows) {
                    format!("{name}.exe")
                } else {
                    name.into()
                })
                .is_file()
        })
    })
}

fn portable_home_path(path: &Path) -> String {
    dirs::home_dir()
        .and_then(|home| path.strip_prefix(home).ok())
        .map(|relative| Path::new("~").join(relative).display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn mise_scope_arguments(scope: &MiseScope, command: &mut Command) {
    match scope {
        MiseScope::File(path) => {
            command.args(["--file".as_ref(), path.as_os_str()]);
        }
        MiseScope::Global => {
            command.arg("--global");
        }
        MiseScope::Local | MiseScope::None => {}
    }
}

fn update_mise_path(scope: &MiseScope, operation: &str, path: &str) -> Result<bool> {
    if !mise_supports_collection_updates() {
        let config = mise_scope_config_path(scope)?;
        eprintln!(
            "mbx[setup]: this mise cannot update env._.path without replacing existing entries"
        );
        let action = match operation {
            "--append" => "add",
            "--remove" => "remove",
            _ => eyre::bail!("unsupported mise path update: {operation}"),
        };
        eprintln!(
            "mbx[setup]: {action} {path:?} {} env._.path in {}",
            if operation == "--append" {
                "to"
            } else {
                "from"
            },
            config.display()
        );
        return Ok(false);
    }
    let mut command = Command::new("mise");
    command.args(["config", "set", operation]);
    mise_scope_arguments(scope, &mut command);
    command.args(["env._.path", path]);
    let status = command
        .status()
        .wrap_err("failed to run `mise config set`")?;
    if !status.success() {
        eyre::bail!("mise could not update env._.path");
    }
    Ok(true)
}

fn mise_supports_collection_updates() -> bool {
    Command::new("mise")
        .args(["config", "set", "--help"])
        .output()
        .is_ok_and(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("--append")
        })
}

fn mise_scope_config_path(scope: &MiseScope) -> Result<PathBuf> {
    match scope {
        MiseScope::File(path) => Ok(path.clone()),
        MiseScope::Global => {
            if let Some(path) = std::env::var_os("MISE_GLOBAL_CONFIG_FILE") {
                return Ok(path.into());
            }
            if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
                return Ok(PathBuf::from(path).join("mise/config.toml"));
            }
            let home = dirs::home_dir()
                .ok_or_else(|| eyre::eyre!("the home directory could not be located"))?;
            Ok(home.join(".config/mise/config.toml"))
        }
        MiseScope::Local => {
            let cwd = std::env::current_dir()?;
            if let Some(config) = nearest_project_config() {
                return Ok(config);
            }
            let filename = std::env::var_os("MISE_DEFAULT_CONFIG_FILENAME")
                .unwrap_or_else(|| "mise.toml".into());
            Ok(cwd.join(filename))
        }
        MiseScope::None => eyre::bail!("mise activation has no selected config"),
    }
}

fn nearest_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let filename =
        std::env::var_os("MISE_DEFAULT_CONFIG_FILENAME").unwrap_or_else(|| "mise.toml".into());
    for directory in cwd.ancestors() {
        let candidate = directory.join(&filename);
        if candidate.is_file() {
            return Some(candidate);
        }
        for alternate in ["mise.toml", ".mise.toml"] {
            let candidate = directory.join(alternate);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn mise_path_is_configured(scope: &MiseScope, path: &str) -> Result<bool> {
    if !mise_supports_collection_updates() {
        return mise_path_file_is_configured(&mise_scope_config_path(scope)?, path);
    }
    let mut command = Command::new("mise");
    command.args(["config", "get"]);
    mise_scope_arguments(scope, &mut command);
    command.arg("env._.path");
    let output = command
        .output()
        .wrap_err("failed to run `mise config get`")?;
    Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).contains(path))
}

fn mise_path_file_is_configured(config: &Path, path: &str) -> Result<bool> {
    let contents = match std::fs::read_to_string(config) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let document = contents
        .parse::<toml_edit::DocumentMut>()
        .wrap_err_with(|| format!("failed to parse {}", config.display()))?;
    let Some(value) = document
        .get("env")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|env| env.get("_"))
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|paths| paths.get("path"))
    else {
        return Ok(false);
    };
    Ok(value.as_str() == Some(path)
        || value
            .as_array()
            .is_some_and(|array| array.iter().any(|entry| entry.as_str() == Some(path))))
}

fn print_manual_activation(path: &Path) {
    let path = path.display().to_string();
    if cfg!(windows) {
        println!("prepend the shim for this PowerShell session:");
        println!("  $env:Path = \"{path};$env:Path\"");
    } else {
        let shell_path = std::env::var_os("SHELL");
        let shell = shell_path
            .as_deref()
            .and_then(|shell| Path::new(shell).file_stem())
            .and_then(OsStr::to_str);
        println!("prepend the Cargo shim to PATH in your shell:");
        match shell {
            Some("fish") => println!("  set -gx PATH {} $PATH", fish_quote(&path)),
            Some("nu") | Some("nushell") => {
                println!("  $env.PATH = ($env.PATH | prepend '{path}')")
            }
            _ => println!("  export PATH=\"{path}:$PATH\""),
        }
    }
    println!("mbx does not edit shell startup files");
}

fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub(super) fn same_file_contents(left: &Path, right: &Path) -> Result<bool> {
    let left_metadata = std::fs::metadata(left)?;
    let right_metadata = std::fs::metadata(right)?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }
    Ok(mbx_cache_core::CacheDigest::blake3_file(left)?
        == mbx_cache_core::CacheDigest::blake3_file(right)?)
}

#[cfg(windows)]
pub(crate) fn cargo_shim_target(install_dir: &Path) -> Option<PathBuf> {
    if let Some(target) = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|directory| !same_path(directory, install_dir))
            .map(|directory| directory.join("mbx.exe"))
            .find(|candidate| candidate.is_file())
    }) {
        return Some(target);
    }
    use std::os::windows::ffi::OsStringExt as _;

    let bytes = std::fs::read(install_dir.join(super::CARGO_SHIM_TARGET_FILE)).ok()?;
    let mut chunks = bytes.chunks_exact(2);
    let target = std::ffi::OsString::from_wide(
        &chunks
            .by_ref()
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .collect::<Vec<_>>(),
    );
    if !chunks.remainder().is_empty() {
        return None;
    }
    let target = PathBuf::from(target);
    (target.is_absolute() && target.is_file()).then_some(target)
}

fn write_cargo_shim_target(install_dir: &Path, executable: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;

        let target = executable
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        std::fs::write(install_dir.join(super::CARGO_SHIM_TARGET_FILE), target)?;
    }
    #[cfg(not(windows))]
    let _ = (install_dir, executable);
    Ok(())
}

pub(crate) fn cargo_shim_is_current(executable: &Path, shim: &Path) -> Result<bool> {
    #[cfg(windows)]
    if let Some(target) = shim.parent().and_then(cargo_shim_target) {
        return Ok(same_path(&target, executable));
    }
    same_file_contents(executable, shim)
}

#[cfg(windows)]
fn same_path(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
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
