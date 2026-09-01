use super::cache::cache_workspace_root;
use super::cargo::{absolute, cargo_roots};
use super::exec::discover_project_root;
use crate::config::Config;
use crate::target;
use bytesize::ByteSize;
use eyre::Result;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(usage::Args)]
pub(super) struct CleanArgs {
    /// Workspace root to clean. Defaults to the current workspace.
    workspace: Option<PathBuf>,
}

pub(super) fn run(config: &Config, args: &CleanArgs) -> Result<ExitCode> {
    let working_dir = std::env::current_dir()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let workspace = match args.workspace.as_deref() {
        Some(requested) => {
            let requested = absolute(&working_dir, &requested.to_string_lossy());
            cache_workspace_root(&cargo, &requested)
        }
        None => cargo_roots(&cargo, &[], None)
            .map(|roots| roots.workspace_root)
            .unwrap_or_else(|| discover_project_root(&working_dir)),
    };
    let mut config = config.clone();
    config.apply_workspace_policy(&workspace)?;

    match target::remove_workspace(&config.target.root, &workspace)? {
        Some(bytes) => {
            if bytes > 0 {
                crate::savings::record_quietly(
                    &config.store_dir(),
                    &crate::savings::Delta {
                        freed_requested_bytes: bytes,
                        ..crate::savings::Delta::default()
                    },
                );
            }
            println!(
                "removed the managed target for {} ({})",
                workspace.display(),
                ByteSize::b(bytes).display().iec()
            );
        }
        None => println!("no managed target for {}", workspace.display()),
    }
    Ok(ExitCode::SUCCESS)
}
