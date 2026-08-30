use super::cargo::{absolute, cargo_roots};
use super::exec::discover_project_root;
use crate::config::Config;
use crate::{store, target};
use bytesize::ByteSize;
use eyre::Result;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(usage::Args)]
pub(super) struct CacheArgs {
    #[usage(subcommand)]
    pub(super) command: CacheCommands,
}

#[derive(usage::Subcommands)]
pub(super) enum CacheCommands {
    /// Print the store directory.
    Dir(JsonArgs),
    /// Summarize what the store holds.
    Stats(JsonArgs),
    /// Show cache use attributed to recorded workspaces.
    Projects,
    /// List the largest objects and action-result records.
    Largest(LargestArgs),
    /// Verify local objects and action results.
    Verify,
    /// Export the cache closure of this checkout's last build.
    Export(ExportArgs),
    /// Import a cache export into the local store.
    Import(ImportArgs),
    /// Remove one workspace's managed target and cache claims.
    Remove(RemoveCacheArgs),
}

#[derive(usage::Args)]
pub(super) struct JsonArgs {
    /// Print a stable machine-readable report.
    #[usage(long)]
    json: bool,
}

#[derive(usage::Args)]
pub(super) struct LargestArgs {
    /// Maximum entries to print.
    #[usage(long, default = "20")]
    limit: usize,
}

#[derive(usage::Args)]
pub(super) struct ExportArgs {
    /// Export every build that set MBX_CACHE_EXPORT_GROUP to this CI group.
    #[usage(long, value_name = "GROUP")]
    group: Option<String>,
    /// Tar archive to write.
    archive: PathBuf,
}

#[derive(usage::Args)]
pub(super) struct ImportArgs {
    /// Tar archive to import.
    archive: PathBuf,
}

#[derive(usage::Args)]
pub(super) struct RemoveCacheArgs {
    /// Workspace root to forget.
    workspace: PathBuf,
}

pub(super) fn run(config: &Config, command: CacheCommands) -> Result<ExitCode> {
    match command {
        CacheCommands::Dir(args) => {
            if args.json {
                print_json(&CacheDirReport {
                    version: 1,
                    store: config.store_dir().display().to_string(),
                })?;
            } else {
                println!("{}", config.store_dir().display());
            }
            Ok(ExitCode::SUCCESS)
        }
        CacheCommands::Stats(args) => cache_stats(config, args.json).map(|()| ExitCode::SUCCESS),
        CacheCommands::Projects => cache_projects(config).map(|()| ExitCode::SUCCESS),
        CacheCommands::Largest(args) => {
            cache_largest(config, args.limit).map(|()| ExitCode::SUCCESS)
        }
        CacheCommands::Verify => cache_verify(config),
        CacheCommands::Export(args) => {
            cache_export(config, &args.archive, args.group.as_deref()).map(|()| ExitCode::SUCCESS)
        }
        CacheCommands::Import(args) => {
            cache_import(config, &args.archive).map(|()| ExitCode::SUCCESS)
        }
        CacheCommands::Remove(args) => {
            cache_remove(config, &args.workspace).map(|()| ExitCode::SUCCESS)
        }
    }
}

pub(super) fn cache_export(config: &Config, archive: &Path, group: Option<&str>) -> Result<()> {
    let working_dir = std::env::current_dir()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let workspace = cargo_roots(&cargo, &[], None)
        .map(|roots| roots.workspace_root)
        .unwrap_or_else(|| discover_project_root(&working_dir));
    let outcome = match group {
        Some(group) => store::export_group(&config.store_dir(), group, archive)?,
        None => store::export_checkout(&config.store_dir(), &workspace, archive)?,
    };
    let subject = group.map_or_else(
        || workspace.display().to_string(),
        |group| format!("export group {group:?}"),
    );
    println!(
        "exported {} actions and {} objects ({}) for {subject}",
        outcome.actions,
        outcome.objects,
        ByteSize::b(outcome.bytes).display().iec(),
    );
    Ok(())
}

pub(super) fn cache_import(config: &Config, archive: &Path) -> Result<()> {
    let outcome = store::import_archive(&config.store_dir(), archive)?;
    println!(
        "imported {} actions and {} objects from {} ({})",
        outcome.actions,
        outcome.objects,
        archive.display(),
        ByteSize::b(outcome.bytes).display().iec()
    );
    Ok(())
}

pub(super) fn cache_stats(config: &Config, json: bool) -> Result<()> {
    let store = config.store_dir();
    let stats = store::stats(&store)?;
    if json {
        let views = target::stats(&config.target.root)?;
        return print_json(&CacheStatsReport {
            version: 1,
            store: store.display().to_string(),
            objects: stats.objects,
            object_bytes: stats.object_bytes,
            action_results: stats.action_results,
            action_result_bytes: stats.action_result_bytes,
            total_bytes: stats.total_bytes(),
            live_checkouts: stats.live_checkouts,
            stale_checkouts: stats.stale_checkouts,
            target_directories: views.views,
            target_bytes: views.bytes,
        });
    }
    println!("store: {}", store.display());
    println!(
        "objects: {} ({})",
        stats.objects,
        ByteSize::b(stats.object_bytes).display().iec()
    );
    println!(
        "action results: {} ({})",
        stats.action_results,
        ByteSize::b(stats.action_result_bytes).display().iec()
    );
    println!(
        "total: {}",
        ByteSize::b(stats.total_bytes()).display().iec()
    );
    println!(
        "checkouts: {} live, {} stale",
        stats.live_checkouts, stats.stale_checkouts
    );
    let views = target::stats(&config.target.root)?;
    println!(
        "target directories: {} ({})",
        views.views,
        ByteSize::b(views.bytes).display().iec()
    );
    Ok(())
}

#[derive(serde::Serialize)]
pub(super) struct CacheDirReport {
    version: u8,
    store: String,
}

#[derive(serde::Serialize)]
pub(super) struct CacheStatsReport {
    version: u8,
    store: String,
    objects: u64,
    object_bytes: u64,
    action_results: u64,
    action_result_bytes: u64,
    total_bytes: u64,
    live_checkouts: u64,
    stale_checkouts: u64,
    target_directories: u64,
    target_bytes: u64,
}

#[derive(serde::Serialize)]
pub(super) struct GcReport {
    pub(super) version: u8,
    pub(super) max_bytes: u64,
    pub(super) dry_run: bool,
    pub(super) action_store: GcActionStoreReport,
    pub(super) targets: GcTargetReport,
}

#[derive(serde::Serialize)]
pub(super) struct GcActionStoreReport {
    pub(super) removed_objects: u64,
    pub(super) removed_action_results: u64,
    pub(super) removed_checkout_records: u64,
    pub(super) removed_session_streams: u64,
    pub(super) removed_bytes: u64,
    pub(super) remaining_bytes: u64,
}

#[derive(serde::Serialize)]
pub(super) struct GcTargetReport {
    pub(super) removed_directories: u64,
    pub(super) removed_bytes: u64,
}

pub(super) fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub(super) fn cache_projects(config: &Config) -> Result<()> {
    let projects = store::projects(&config.store_dir())?;
    if projects.is_empty() {
        println!("no recorded workspaces");
        return Ok(());
    }
    for project in projects {
        let state = if project.live { "live" } else { "stale" };
        println!(
            "{}\t{} action cache\t{} managed targets\t{} identities\t{state}",
            project.workspace_root.display(),
            ByteSize::b(project.action_bytes).display().iec(),
            ByteSize::b(project.target_bytes).display().iec(),
            project.identities,
        );
    }
    Ok(())
}

pub(super) fn cache_largest(config: &Config, limit: usize) -> Result<()> {
    for entry in store::largest(&config.store_dir(), limit)? {
        let path = entry
            .path
            .strip_prefix(config.store_dir())
            .unwrap_or(&entry.path);
        println!(
            "{}\t{}\t{}",
            ByteSize::b(entry.bytes).display().iec(),
            entry.kind,
            path.display()
        );
    }
    Ok(())
}

pub(super) fn cache_verify(config: &Config) -> Result<ExitCode> {
    let outcome = store::verify(&config.store_dir())?;
    println!(
        "verified {} objects and {} action results",
        outcome.checked_objects, outcome.checked_action_results
    );
    for path in &outcome.problems {
        println!("invalid: {}", path.display());
    }
    Ok(if outcome.problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub(super) fn cache_remove(config: &Config, workspace: &Path) -> Result<()> {
    let working_dir = std::env::current_dir()?;
    let requested = absolute(&working_dir, &workspace.to_string_lossy());
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let workspace = cache_workspace_root(&cargo, &requested);
    let target_bytes = target::remove_workspace(&config.target.root, &workspace)?;
    let removed = store::remove_project(&config.store_dir(), &workspace)?;
    println!(
        "removed {} checkout records for {}",
        removed.removed_checkout_records,
        workspace.display()
    );
    if let Some(bytes) = target_bytes {
        crate::savings::record_quietly(
            &config.store_dir(),
            &crate::savings::Delta {
                freed_requested_bytes: bytes,
                ..crate::savings::Delta::default()
            },
        );
        println!(
            "freed managed target directory ({})",
            ByteSize::b(bytes).display().iec()
        );
    }
    println!("shared cache objects remain available to other workspaces and normal GC");
    Ok(())
}

/// Resolve the workspace exactly as Cargo does when recording cache ownership.
///
/// Filesystem canonicalization is not interchangeable with Cargo's reported
/// root: on Windows it can introduce a `\\?\` prefix, and symlink spellings can
/// differ too. Both checkout records and managed-target keys use the metadata
/// spelling, so removal must obtain that same identity or it can silently miss
/// the workspace it was asked to forget.
pub(super) fn cache_workspace_root(cargo: &std::ffi::OsStr, requested: &Path) -> PathBuf {
    let arguments = vec![
        "--manifest-path".to_string(),
        requested.join("Cargo.toml").to_string_lossy().into_owned(),
    ];
    cargo_roots(cargo, &arguments, None)
        .map(|roots| roots.workspace_root)
        .unwrap_or_else(|| requested.to_path_buf())
}
