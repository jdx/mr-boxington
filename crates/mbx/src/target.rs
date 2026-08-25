//! Managed target directories.
//!
//! Cargo puts build outputs in `<workspace>/target`, which means a checkout's
//! outputs live and die with the checkout, and nothing but `rm -rf` ever
//! reclaims them. A managed target directory moves them under a root mbx owns,
//! keyed by the checkout that builds there, and leaves a symlink behind so the
//! paths people type still work.
//!
//! What that buys is collection. A target directory whose checkout no longer
//! exists is unambiguous garbage -- nothing can ever ask for it again -- and it
//! is usually the largest thing on the disk by an order of magnitude. It costs
//! nothing in cache hits: the shim maps the target directory to `${target}`
//! before anything else, so an action keys the same wherever its outputs land.
//!
//! ```text
//! <root>/v1/<digest of the workspace root>/       the target directory itself
//! <root>/v1/<digest of the workspace root>.json   which checkout it belongs to
//! ```
//!
//! The record sits beside the directory rather than inside it because `cargo
//! clean` empties the directory, and a target directory nothing can trace back
//! to a checkout could never be collected.

use crate::config::Config;
use eyre::{Context, Result};
use mbx_cache_core::CacheDigest;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const VIEWS_DIR: &str = "v1";
const VIEW_RECORD_VERSION: u8 = 1;

/// Which checkout a managed target directory belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewRecord {
    version: u8,
    workspace_root: PathBuf,
    updated_secs: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ViewStats {
    pub views: u64,
    pub bytes: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneOutcome {
    pub removed_views: u64,
    pub removed_bytes: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CollectionOutcome {
    pub removed_views: u64,
    pub removed_bytes: u64,
    pub removed_stale_views: u64,
    pub removed_live_views: u64,
    pub remaining_bytes: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub managed: Option<PathBuf>,
    /// Present only when the old directory was actually removed.
    pub removed_bytes: Option<u64>,
}

/// Whether an interactive caller may offer to remove this target directory.
///
/// Match placement's eligibility rules exactly, then require a real directory.
/// The latter uses symlink metadata so a link to a directory is never offered
/// for recursive deletion.
pub fn can_remove_existing(
    config: &Config,
    workspace_root: &Path,
    target_dir: &Path,
    requested: bool,
) -> bool {
    config.target.views
        && !requested
        && target_dir == workspace_root.join("target")
        && std::fs::symlink_metadata(target_dir).is_ok_and(|metadata| metadata.is_dir())
}

/// Replace a confirmed existing target directory without risking its outputs.
///
/// The old directory is first renamed into a temporary sibling on the same
/// filesystem. It is removed only after the managed link and record both
/// succeed; otherwise it is restored to its original path.
pub fn migrate_existing(
    config: &Config,
    workspace_root: &Path,
    target_dir: &Path,
    requested: bool,
) -> Result<MigrationOutcome> {
    migrate_existing_with(config, workspace_root, target_dir, requested, || {
        place(config, workspace_root, target_dir, requested)
    })
}

fn migrate_existing_with(
    config: &Config,
    workspace_root: &Path,
    target_dir: &Path,
    requested: bool,
    place_target: impl FnOnce() -> Option<PathBuf>,
) -> Result<MigrationOutcome> {
    if !std::fs::symlink_metadata(target_dir).is_ok_and(|metadata| metadata.is_dir()) {
        eyre::bail!(
            "{} is no longer a real target directory, so it was not migrated",
            target_dir.display()
        );
    }
    if !can_remove_existing(config, workspace_root, target_dir, requested) {
        eyre::bail!("{} is not eligible for migration", target_dir.display());
    }

    let backup_root = tempfile::Builder::new()
        .prefix(".mbx-target-backup-")
        .tempdir_in(workspace_root)
        .wrap_err("could not create a temporary target backup")?;
    let backup = backup_root.path().join("target");
    std::fs::rename(target_dir, &backup)
        .wrap_err_with(|| format!("could not temporarily move {}", target_dir.display()))?;
    let old_bytes = tree_bytes(&backup);

    if let Some(managed) = place_target() {
        if let Err(error) = std::fs::remove_dir_all(&backup) {
            let retained = backup_root.keep().join("target");
            log::warn!(
                "the old target directory was retained at {}: {error}",
                retained.display()
            );
            return Ok(MigrationOutcome {
                managed: Some(managed),
                removed_bytes: None,
            });
        }
        return Ok(MigrationOutcome {
            managed: Some(managed),
            removed_bytes: Some(old_bytes),
        });
    }

    let managed = view_dir(&config.target.root, workspace_root);
    let restore = (|| -> Result<()> {
        match std::fs::symlink_metadata(target_dir) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    && std::fs::read_link(target_dir).is_ok_and(|link| link == managed) =>
            {
                remove_link(target_dir).wrap_err("could not remove the failed managed link")?;
            }
            Ok(_) => eyre::bail!(
                "{} was occupied while restoring the old target directory",
                target_dir.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).wrap_err("could not inspect the failed migration"),
        }
        std::fs::rename(&backup, target_dir)
            .wrap_err_with(|| format!("could not restore {}", target_dir.display()))
    })();
    if let Err(error) = restore {
        let retained = backup_root.keep().join("target");
        return Err(error).wrap_err_with(|| {
            format!(
                "the old target directory was retained at {}",
                retained.display()
            )
        });
    }
    Ok(MigrationOutcome::default())
}

/// Where this checkout's outputs should be written, if mbx is placing them.
///
/// `None` leaves cargo's own answer alone, and every reason for that is a
/// reason not to be clever:
///
/// - the feature was explicitly turned off;
/// - a flag or the environment named the target directory, so moving it would
///   be overriding the person who said where it goes. This is asked separately
///   from where the directory actually is, because `--target-dir target` names
///   the default location and still means the caller chose it -- and cargo
///   prefers that flag over the `CARGO_TARGET_DIR` placement would set, so
///   relocating anyway leaves cargo writing one place while the shim maps
///   another and the build quietly stops using the cache at all;
/// - the target directory is not the one cargo would have picked by default,
///   which means a cargo configuration named it;
/// - `<workspace>/target` is a real directory, which is somebody's build
///   outputs. Replacing it with a link would strand them, and deleting it is
///   not this function's business.
pub fn place(
    config: &Config,
    workspace_root: &Path,
    target_dir: &Path,
    requested: bool,
) -> Option<PathBuf> {
    if !config.target.views {
        return None;
    }
    if requested {
        log::debug!(
            "leaving the target directory at {} where it was asked for",
            target_dir.display()
        );
        return None;
    }
    if target_dir != workspace_root.join("target") {
        log::debug!(
            "leaving the target directory at {} where it was asked for",
            target_dir.display()
        );
        return None;
    }
    let managed = view_dir(&config.target.root, workspace_root);
    if !managed.is_absolute() {
        // This path becomes the shim's `${target}` mapping, and the shim runs
        // with cargo's working directory rather than this one, so a relative
        // one would map nothing and bypass the cache for the whole build.
        log::warn!(
            "the managed target directory {} is not absolute, so the target directory was left alone",
            managed.display()
        );
        return None;
    }
    // The link is what decides whether placement happens at all, so it goes
    // first and nothing is written until it is in place. A refusal has to leave
    // no trace: an unused directory and record would be counted and reported
    // for a checkout nothing manages, and worse, a refusal must not disturb a
    // record an earlier placement wrote -- the directory that one names may be
    // full of outputs, and the record is the only thing that can trace it.
    let link = match link_view(target_dir, &managed, workspace_root) {
        Ok(link) => link,
        Err(error) => {
            // Placement is best-effort: existing build outputs, a custom
            // link, or a platform that cannot create the link are all normal
            // reasons to let cargo keep its own target directory. This was a
            // warning while managed targets were opt-in, but would become
            // noise on every existing checkout now that they are the default.
            log::debug!("{error}");
            return None;
        }
    };
    let record_path = view_record_path(&config.target.root, workspace_root);
    let previous_record = match read_record_state(&record_path) {
        Ok(record) => record,
        Err(error) => {
            log::warn!("the existing target record could not be preserved: {error}");
            if let Err(error) = rollback_link(target_dir, &link) {
                log::warn!(
                    "the unused link {} was not rolled back: {error}",
                    target_dir.display()
                );
            }
            return None;
        }
    };
    // Recorded before the directory exists, so a directory can never exist that
    // `prune` has no way to see.
    if let Err(error) = record_view(&config.target.root, workspace_root) {
        log::warn!("the managed target directory was not recorded: {error}");
        // Undo this call's own link and nothing else. Left behind it would
        // redirect every later build into a directory no record names. A link
        // replaced during recovery goes back to its previous managed view.
        if let Err(error) = rollback_link(target_dir, &link) {
            log::warn!(
                "the unused link {} was not rolled back: {error}",
                target_dir.display()
            );
        }
        return None;
    }
    // Cargo would create this itself on the way to writing in it. Doing it here
    // keeps the link from dangling in the meantime, which is what someone
    // listing the workspace would see.
    if let Err(error) = prepare_view(&managed, &link) {
        log::warn!(
            "the managed target directory {} was not prepared: {error}",
            managed.display()
        );
        // A newly created link is the migration path: nothing used it before
        // this attempt, so both the link and record can be rolled back and an
        // existing target backup can be restored. Replacing an older managed
        // view may already have moved its directory, so keep the established
        // link and record in that recovery case.
        if matches!(&link, Link::Created) {
            match rollback_link(target_dir, &link) {
                Ok(()) => {
                    if let Err(error) = restore_record_state(&record_path, previous_record) {
                        log::warn!("the target record was not rolled back: {error}");
                    }
                }
                Err(error) => log::warn!(
                    "the unused link {} was not rolled back: {error}",
                    target_dir.display()
                ),
            }
            return None;
        }
    }
    Some(managed)
}

/// Whether a link had to be made, or was already pointing the right way.
enum Link {
    Existing,
    Created,
    Replaced(PathBuf),
}

/// Point `target_dir` at `managed` so the paths people type keep working.
///
/// A missing link is not fatal on its own -- cargo is told where to write
/// either way -- but it is refused rather than forced when something real is
/// already there, and Windows only allows a symlink to be created by a
/// privileged or developer-mode process, where the honest answer is to leave
/// the outputs where cargo would have put them.
fn link_view(target_dir: &Path, managed: &Path, workspace_root: &Path) -> Result<Link> {
    let replaced = match std::fs::read_link(target_dir) {
        // Already pointing where it should. Re-linking would race a concurrent
        // build for no gain.
        Ok(existing) if existing == managed => return Ok(Link::Existing),
        Ok(existing) if replaceable_managed_link(&existing, managed, workspace_root) => {
            // This is one of our links, but no longer the view this checkout
            // should use. That happens after a checkout moves, its old view is
            // pruned, or the configured target root changes.
            remove_link(target_dir).wrap_err_with(|| {
                format!(
                    "could not replace the outdated managed target link {}",
                    target_dir.display()
                )
            })?;
            Some(existing)
        }
        Ok(existing) => {
            eyre::bail!(
                "{} already links to {}, so it was left alone",
                target_dir.display(),
                existing.display()
            );
        }
        Err(_) if target_dir.exists() => {
            eyre::bail!(
                "{} is a real directory holding build outputs, so it was left alone; remove it to use a managed target directory",
                target_dir.display()
            );
        }
        Err(_) => None,
    };
    if let Err(error) = symlink_dir(managed, target_dir) {
        if let Some(previous) = replaced
            && let Err(restore_error) = symlink_dir(&previous, target_dir)
        {
            eyre::bail!(
                "could not link {} to a managed target directory: {error}; its previous managed link could not be restored: {restore_error}",
                target_dir.display()
            );
        }
        return Err(error).wrap_err_with(|| {
            format!(
                "could not link {} to a managed target directory",
                target_dir.display()
            )
        });
    }
    Ok(replaced.map_or(Link::Created, Link::Replaced))
}

/// Put a link back the way it was before this placement attempt.
fn rollback_link(target_dir: &Path, link: &Link) -> Result<()> {
    match link {
        Link::Existing => Ok(()),
        Link::Created => remove_link(target_dir).map_err(Into::into),
        Link::Replaced(previous) => {
            remove_link(target_dir)?;
            symlink_dir(previous, target_dir).wrap_err("could not restore the previous link")
        }
    }
}

/// Create a new view, carrying forward and retiring an outdated one.
fn prepare_view(managed: &Path, link: &Link) -> Result<()> {
    let Link::Replaced(previous) = link else {
        return std::fs::create_dir_all(managed).map_err(Into::into);
    };
    match std::fs::rename(previous, managed) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(managed)?;
        }
        Err(_) => {
            // A root can cross filesystems, where rename cannot carry the old
            // view forward. Establish the new view before retiring the old
            // one; a rebuild is preferable to an orphan nothing will scan.
            std::fs::create_dir_all(managed)?;
            std::fs::remove_dir_all(previous).wrap_err_with(|| {
                format!(
                    "could not retire the old target view {}",
                    previous.display()
                )
            })?;
        }
    }
    let record = previous.with_extension("json");
    if let Err(error) = std::fs::remove_file(&record)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error).wrap_err_with(|| {
            format!(
                "could not retire the old target record {}",
                record.display()
            )
        });
    }
    Ok(())
}

/// Whether a link target is a view mbx previously placed.
///
/// A view still carrying its record proves ownership directly. A pruned view
/// has lost that record and directory, so its digest-shaped path under the
/// configured views root is the remaining evidence. Other symlinks are never
/// replaced, including dangling ones.
fn replaceable_managed_link(existing: &Path, managed: &Path, workspace_root: &Path) -> bool {
    let Some(name) = existing.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !mbx_cache_core::is_task_identity(name) {
        return false;
    }
    if let Some(record) = read_view_record(&existing.with_extension("json"))
        && view_key(&record.workspace_root) == name
    {
        return record.workspace_root == workspace_root
            || !crate::store::checkout_is_live(&record.workspace_root);
    }
    // A collector removes the record and directory together. With neither
    // left, a digest-shaped dangling link under this root is safe to recover;
    // a directory with a missing or corrupt record is not evidence of ours to
    // move, and uncertainty must leave it alone.
    existing.parent() == managed.parent() && matches!(existing.try_exists(), Ok(false))
}

/// Unlink a directory symlink without following it.
///
/// Windows unlinks one with `remove_dir` -- it removes the link, never what the
/// link points at -- while `remove_file` refuses. Unix is the other way round.
fn remove_link(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::fs::remove_dir(path)
    }
    #[cfg(not(windows))]
    {
        std::fs::remove_file(path)
    }
}

#[cfg(unix)]
fn symlink_dir(source: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, link)
}

#[cfg(windows)]
fn symlink_dir(source: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(source, link)
}

/// Summarize the managed target directories under `root`.
pub fn stats(root: &Path) -> Result<ViewStats> {
    let mut stats = ViewStats::default();
    for (_, directory) in views(root)? {
        stats.views += 1;
        stats.bytes += tree_bytes(&directory);
    }
    Ok(stats)
}

/// Remove the target directories of checkouts that no longer exist.
///
/// Only the checkout being gone counts. A checkout that is merely idle keeps
/// its outputs: collecting those would cost a rebuild of work the cache can
/// usually serve, but it would also delete something the person who owns that
/// directory can still see, which is a different kind of surprise.
pub fn prune(root: &Path) -> Result<PruneOutcome> {
    let outcome = collect(root, None, None, false)?;
    Ok(PruneOutcome {
        removed_views: outcome.removed_views,
        removed_bytes: outcome.removed_bytes,
    })
}

/// Collect abandoned, expired, and over-budget managed target directories.
///
/// Limits are opt-in. Abandoned checkouts are always collected; live views
/// are considered oldest-first only when an age or size policy requests it.
pub(crate) fn collect(
    root: &Path,
    max_bytes: Option<u64>,
    max_age: Option<Duration>,
    dry_run: bool,
) -> Result<CollectionOutcome> {
    let mut outcome = CollectionOutcome::default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let mut entries = Vec::new();
    let mut uncollectable_bytes = 0_u64;
    for (record_path, directory) in views(root)? {
        let Some(record) = read_view_record(&record_path) else {
            // A record this build cannot read names no checkout, and a target
            // directory nobody can trace is not one to delete on a guess.
            uncollectable_bytes = uncollectable_bytes.saturating_add(tree_bytes(&directory));
            continue;
        };
        let bytes = tree_bytes(&directory);
        entries.push((
            record_path,
            directory,
            record.updated_secs,
            bytes,
            crate::store::checkout_is_live(&record.workspace_root),
        ));
    }
    let mut remaining = entries
        .iter()
        .map(|entry| entry.3)
        .fold(uncollectable_bytes, u64::saturating_add);
    let mut selected = HashSet::new();
    for (record_path, _, updated, bytes, live) in &entries {
        let expired = max_age.is_some_and(|age| now.saturating_sub(*updated) > age.as_secs());
        if !live || expired {
            selected.insert(record_path.clone());
            remaining = remaining.saturating_sub(*bytes);
        }
    }
    if let Some(max_bytes) = max_bytes
        && remaining > max_bytes
    {
        entries.sort_by_key(|entry| entry.2);
        for (record_path, _, _, bytes, _) in &entries {
            if remaining <= max_bytes {
                break;
            }
            if selected.insert(record_path.clone()) {
                remaining = remaining.saturating_sub(*bytes);
            }
        }
    }

    for (record_path, directory, _, bytes, live) in entries {
        if !selected.contains(&record_path) {
            continue;
        }
        let removal = if dry_run {
            Ok(())
        } else {
            std::fs::remove_dir_all(&directory)
        };
        match removal {
            Ok(()) => {
                outcome.removed_views += 1;
                outcome.removed_bytes += bytes;
                if live {
                    outcome.removed_live_views += 1;
                } else {
                    outcome.removed_stale_views += 1;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                outcome.removed_views += 1;
                if live {
                    outcome.removed_live_views += 1;
                } else {
                    outcome.removed_stale_views += 1;
                }
            }
            Err(error) => {
                remaining = remaining.saturating_add(bytes);
                log::warn!(
                    "could not remove the target directory {}: {error}",
                    directory.display()
                );
                continue;
            }
        }
        // Last, so a failed removal above leaves the record to try again with.
        if !dry_run
            && let Err(error) = std::fs::remove_file(&record_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "could not remove the target record {}: {error}",
                record_path.display()
            );
        }
    }
    outcome.remaining_bytes = remaining;
    Ok(outcome)
}

/// Every managed target directory under `root`, as record and directory.
fn views(root: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let directory = views_root(root);
    let listing = match std::fs::read_dir(&directory) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("failed to read {}", directory.display()));
        }
    };
    let mut views = Vec::new();
    for entry in listing {
        let entry = entry?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            views.push((path.clone(), path.with_extension("")));
        }
    }
    Ok(views)
}

fn record_view(root: &Path, workspace_root: &Path) -> Result<()> {
    let record = ViewRecord {
        version: VIEW_RECORD_VERSION,
        workspace_root: workspace_root.to_path_buf(),
        updated_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or_default(),
    };
    let mut contents = serde_json::to_vec(&record)?;
    contents.push(b'\n');
    crate::util::write_atomic(&view_record_path(root, workspace_root), &contents)
}

fn read_record_state(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).wrap_err_with(|| format!("could not read {}", path.display())),
    }
}

fn restore_record_state(path: &Path, previous: Option<Vec<u8>>) -> Result<()> {
    if let Some(contents) = previous {
        return crate::util::write_atomic(path, &contents);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).wrap_err_with(|| format!("could not remove {}", path.display())),
    }
}

fn read_view_record(path: &Path) -> Option<ViewRecord> {
    let bytes = std::fs::read(path).ok()?;
    let record = serde_json::from_slice::<ViewRecord>(&bytes).ok()?;
    (record.version == VIEW_RECORD_VERSION).then_some(record)
}

/// The directory holding every managed target directory.
///
/// Absolutized lexically, because these paths are handed to cargo and to the
/// shim, which run with a working directory of their own. Lexically and not by
/// canonicalizing: the directory may not exist yet, and resolving symlinks here
/// would break the very mapping it exists to serve.
fn views_root(root: &Path) -> PathBuf {
    let views = root.join(VIEWS_DIR);
    std::path::absolute(&views).unwrap_or(views)
}

fn view_dir(root: &Path, workspace_root: &Path) -> PathBuf {
    views_root(root).join(view_key(workspace_root))
}

fn view_record_path(root: &Path, workspace_root: &Path) -> PathBuf {
    views_root(root).join(format!("{}.json", view_key(workspace_root)))
}

/// Name a checkout's directory after a digest of its path.
///
/// Keyed by path and nothing else, so one checkout has one target directory
/// whatever it builds -- unlike a cache identity, which covers one command line
/// against one lockfile and would give the same checkout a fresh directory
/// every time either changed.
fn view_key(workspace_root: &Path) -> String {
    CacheDigest::blake3(workspace_root.to_string_lossy().as_bytes()).hash
}

fn tree_bytes(directory: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(listing) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in listing.flatten() {
            // Symlinks are not followed: a link's target is either inside this
            // tree and already counted, or outside it and not ours to count.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file()
                && let Ok(metadata) = entry.metadata()
            {
                total += metadata.len();
            }
        }
    }
    total
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
