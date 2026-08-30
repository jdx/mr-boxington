use eyre::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(usage::Args)]
pub(super) struct SetupArgs {
    /// Report whether plain Cargo integration is installed and current.
    #[usage(long)]
    pub(super) status: bool,
    /// Refresh an existing mbx wrapper without installing a missing one.
    #[usage(long)]
    pub(super) update: bool,
    /// Remove mbx's Cargo configuration and installed wrapper.
    #[usage(long)]
    pub(super) uninstall: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetupAction {
    Install,
    Status,
    Update,
    Uninstall,
}

impl SetupArgs {
    pub(super) fn action(&self) -> Result<SetupAction> {
        let selected = self.status as u8 + self.update as u8 + self.uninstall as u8;
        if selected > 1 {
            eyre::bail!("--status, --update, and --uninstall are mutually exclusive");
        }
        Ok(if self.status {
            SetupAction::Status
        } else if self.update {
            SetupAction::Update
        } else if self.uninstall {
            SetupAction::Uninstall
        } else {
            SetupAction::Install
        })
    }
}

pub(super) fn run(action: SetupAction) -> Result<ExitCode> {
    let executable = std::env::current_exe().wrap_err("failed to locate the mbx executable")?;
    let (install_dir, config_path) = setup_paths()?;
    setup_at_action(&executable, &install_dir, &config_path, action)
}

pub(super) fn setup_paths() -> Result<(PathBuf, PathBuf)> {
    let data = dirs::data_local_dir()
        .ok_or_else(|| eyre::eyre!("the platform data directory could not be located"))?;
    let install_dir = data.join("mbx").join("bin");
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
        .ok_or_else(|| eyre::eyre!("Cargo's configuration directory could not be located"))?;
    let config_path =
        if cargo_home.join("config.toml").exists() || !cargo_home.join("config").exists() {
            cargo_home.join("config.toml")
        } else {
            cargo_home.join("config")
        };
    Ok((install_dir, config_path))
}

#[cfg(test)]
pub(super) fn setup_at(executable: &Path, install_dir: &Path, config_path: &Path) -> Result<()> {
    setup_at_action(executable, install_dir, config_path, SetupAction::Install)?;
    Ok(())
}

pub(super) fn setup_at_action(
    executable: &Path,
    install_dir: &Path,
    config_path: &Path,
    action: SetupAction,
) -> Result<ExitCode> {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut document = contents
        .parse::<toml_edit::DocumentMut>()
        .wrap_err_with(|| format!("failed to parse {}", config_path.display()))?;
    let shim_name = if cfg!(windows) {
        format!("{}.exe", crate::session::RUSTC_SHIM_STEM)
    } else {
        crate::session::RUSTC_SHIM_STEM.into()
    };
    let shim = install_dir.join(shim_name);
    let configured = document
        .get("build")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|build| build.get("rustc-wrapper"));
    let owns_configuration = configured
        .and_then(toml_edit::Item::as_str)
        .is_some_and(|configured| Path::new(configured) == shim);

    match action {
        SetupAction::Status => {
            if !owns_configuration {
                if let Some(configured) = configured {
                    println!(
                        "mbx setup is not active: build.rustc-wrapper is {}",
                        configured
                    );
                } else {
                    println!("mbx setup is not installed");
                }
                return Ok(ExitCode::FAILURE);
            }
            if !shim.is_file() {
                println!(
                    "mbx setup is configured but {} is missing; run `mbx setup --update`",
                    shim.display()
                );
                return Ok(ExitCode::FAILURE);
            }
            if same_file_contents(executable, &shim)? {
                println!("mbx setup is installed and current: {}", shim.display());
                return Ok(ExitCode::SUCCESS);
            }
            println!("mbx setup is installed but outdated; run `mbx setup --update`");
            return Ok(ExitCode::FAILURE);
        }
        SetupAction::Update if !owns_configuration => {
            if let Some(configured) = configured {
                eyre::bail!(
                    "mbx setup is not active because build.rustc-wrapper is {configured}; refusing to replace another wrapper"
                );
            }
            eyre::bail!("mbx setup is not installed; run `mbx setup` first");
        }
        SetupAction::Uninstall => {
            if owns_configuration {
                let build = document
                    .get_mut("build")
                    .and_then(toml_edit::Item::as_table_like_mut)
                    .expect("the configuration was inspected above");
                build.remove("rustc-wrapper");
                if build.is_empty() {
                    document.remove("build");
                }
                crate::util::write_atomic(config_path, document.to_string().as_bytes())?;
            }
            match std::fs::remove_file(&shim) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if owns_configuration {
                println!(
                    "removed mbx setup from {} and deleted {}",
                    config_path.display(),
                    shim.display()
                );
            } else {
                println!("mbx setup was not installed");
            }
            return Ok(ExitCode::SUCCESS);
        }
        SetupAction::Install | SetupAction::Update => {}
    }

    if let Some(configured) = configured {
        if owns_configuration {
            std::fs::create_dir_all(install_dir)?;
            crate::session::install_shim(
                executable,
                install_dir,
                crate::session::ShimLink::Pinned,
            )?;
            let verb = if action == SetupAction::Update {
                "updated"
            } else {
                "refreshed"
            };
            println!("{verb} {}; Cargo was already configured", shim.display());
            return Ok(ExitCode::SUCCESS);
        }
        println!(
            "left {} unchanged: build.rustc-wrapper is already {}",
            config_path.display(),
            configured
        );
        return Ok(ExitCode::SUCCESS);
    }

    std::fs::create_dir_all(install_dir)?;
    let shim =
        crate::session::install_shim(executable, install_dir, crate::session::ShimLink::Pinned)?;
    document["build"]["rustc-wrapper"] = toml_edit::value(shim.to_string_lossy().into_owned());
    let parent = config_path
        .parent()
        .ok_or_else(|| eyre::eyre!("Cargo configuration path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    crate::util::write_atomic(config_path, document.to_string().as_bytes())?;
    println!(
        "installed {} and configured {}",
        shim.display(),
        config_path.display()
    );
    println!("plain cargo commands now use mbx's local action cache");
    Ok(ExitCode::SUCCESS)
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
