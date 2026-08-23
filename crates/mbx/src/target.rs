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
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Where this checkout's outputs should be written, if mbx is placing them.
///
/// `None` leaves cargo's own answer alone, and every reason for that is a
/// reason not to be clever:
///
/// - the feature is off, which is the default;
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
            log::warn!("{error}");
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
    let mut outcome = PruneOutcome::default();
    for (record_path, directory) in views(root)? {
        let Some(record) = read_view_record(&record_path) else {
            // A record this build cannot read names no checkout, and a target
            // directory nobody can trace is not one to delete on a guess.
            continue;
        };
        if crate::store::checkout_is_live(&record.workspace_root) {
            continue;
        }
        let bytes = tree_bytes(&directory);
        match std::fs::remove_dir_all(&directory) {
            Ok(()) => {
                outcome.removed_views += 1;
                outcome.removed_bytes += bytes;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                outcome.removed_views += 1;
            }
            Err(error) => {
                log::warn!(
                    "could not remove the target directory {}: {error}",
                    directory.display()
                );
                continue;
            }
        }
        // Last, so a failed removal above leaves the record to try again with.
        if let Err(error) = std::fs::remove_file(&record_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "could not remove the target record {}: {error}",
                record_path.display()
            );
        }
    }
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
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total += metadata.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TargetSettings;

    fn test_config(root: &Path, views: bool) -> Config {
        Config {
            cache_dir: root.join("cache"),
            stats_report: None,
            verify: false,
            incremental: false,
            share_out_dir: false,
            remote: Default::default(),
            http: Default::default(),
            gc: Default::default(),
            target: TargetSettings {
                views,
                root: root.join("targets"),
            },
        }
    }

    /// A checkout with the default target directory, ready to be managed.
    fn checkout(root: &Path, name: &str) -> PathBuf {
        let workspace = root.join(name);
        std::fs::create_dir_all(&workspace).unwrap();
        workspace
    }

    #[test]
    fn places_the_default_target_directory_under_the_managed_root() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");

        let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();

        assert!(managed.is_absolute(), "the shim maps only absolute roots");
        assert!(managed.starts_with(views_root(&config.target.root)));
        assert!(managed.is_dir());
        assert_eq!(
            std::fs::read_link(workspace.join("target")).unwrap(),
            managed,
            "the workspace should still have a target directory to reach"
        );
        assert_eq!(stats(&config.target.root).unwrap().views, 1);
    }

    #[test]
    fn placing_a_target_directory_twice_reaches_the_same_one() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");

        let first = place(&config, &workspace, &workspace.join("target"), false).unwrap();
        let second = place(&config, &workspace, &workspace.join("target"), false).unwrap();

        assert_eq!(first, second);
        assert_eq!(stats(&config.target.root).unwrap().views, 1);
    }

    #[test]
    fn replaces_an_outdated_managed_target_link() {
        let directory = tempfile::tempdir().unwrap();
        let first = test_config(&directory.path().join("first"), true);
        let second = test_config(&directory.path().join("second"), true);
        let workspace = checkout(directory.path(), "project");
        let old = place(&first, &workspace, &workspace.join("target"), false).unwrap();
        std::fs::write(old.join("artifact"), b"outputs").unwrap();

        let new = place(&second, &workspace, &workspace.join("target"), false).unwrap();

        assert_ne!(old, new);
        assert_eq!(std::fs::read_link(workspace.join("target")).unwrap(), new);
        assert!(new.join("artifact").is_file(), "the old view should move");
        assert!(!old.exists(), "the old root must not retain an orphan");
        assert_eq!(stats(&first.target.root).unwrap(), ViewStats::default());
        assert_eq!(stats(&second.target.root).unwrap().views, 1);
    }

    #[test]
    fn leaves_somebody_elses_dangling_link_alone() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");
        let target = workspace.join("target");
        let elsewhere = directory.path().join("missing");
        symlink_dir(&elsewhere, &target).unwrap();

        assert!(place(&config, &workspace, &target, false).is_none());
        assert_eq!(std::fs::read_link(target).unwrap(), elsewhere);
    }

    #[test]
    fn leaves_another_live_checkouts_view_alone() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let original = checkout(directory.path(), "original");
        let copied = checkout(directory.path(), "copied");
        let managed = place(&config, &original, &original.join("target"), false).unwrap();
        std::fs::write(managed.join("artifact"), b"outputs").unwrap();
        symlink_dir(&managed, &copied.join("target")).unwrap();

        assert!(place(&config, &copied, &copied.join("target"), false).is_none());

        assert_eq!(std::fs::read_link(copied.join("target")).unwrap(), managed);
        assert!(managed.join("artifact").exists());
        assert_eq!(stats(&config.target.root).unwrap().views, 1);
    }

    #[test]
    fn leaves_the_target_directory_alone_unless_asked() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), false);
        let workspace = checkout(directory.path(), "project");

        assert!(place(&config, &workspace, &workspace.join("target"), false).is_none());
        assert!(!workspace.join("target").exists());
    }

    #[test]
    fn leaves_a_requested_target_directory_alone_even_at_the_default_place() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");

        // `--target-dir target` names the default location and still means the
        // caller chose it. Cargo prefers that flag over the CARGO_TARGET_DIR
        // placement would set, so relocating would leave cargo writing one
        // place while the shim mapped another -- measured as a build that
        // looked nothing up and stored almost nothing.
        assert!(place(&config, &workspace, &workspace.join("target"), true).is_none());

        assert!(!workspace.join("target").exists());
        assert_eq!(stats(&config.target.root).unwrap(), ViewStats::default());
    }

    #[test]
    fn leaves_a_target_directory_someone_else_chose_where_it_is() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");

        // A flag, the environment, or a cargo configuration put it here, and
        // that outranks any placement of ours.
        let elsewhere = directory.path().join("chosen");
        assert!(place(&config, &workspace, &elsewhere, false).is_none());
        assert_eq!(stats(&config.target.root).unwrap(), ViewStats::default());
    }

    #[test]
    fn refuses_to_displace_real_build_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");
        let existing = workspace.join("target");
        std::fs::create_dir_all(existing.join("debug")).unwrap();
        std::fs::write(existing.join("debug/libfixture.rlib"), b"outputs").unwrap();

        assert!(place(&config, &workspace, &existing, false).is_none());

        assert!(
            existing.join("debug/libfixture.rlib").exists(),
            "somebody's build outputs are not ours to move or delete"
        );
        assert_eq!(
            stats(&config.target.root).unwrap(),
            ViewStats::default(),
            "a refusal that leaves a directory and a record behind would report \
             a managed target directory for a checkout nothing manages"
        );
    }

    #[test]
    fn a_refusal_leaves_an_earlier_placement_alone() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");
        let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();
        std::fs::write(managed.join("artifact"), b"outputs").unwrap();

        // Somebody replaced the link with a directory of their own. The
        // placement already on disk still owns a full target directory, and
        // dropping its record would leave that directory untraceable -- which
        // means never collected.
        remove_link(&workspace.join("target")).unwrap();
        std::fs::create_dir_all(workspace.join("target")).unwrap();

        assert!(place(&config, &workspace, &workspace.join("target"), false).is_none());

        assert_eq!(stats(&config.target.root).unwrap().views, 1);
        assert!(managed.join("artifact").exists());
        std::fs::remove_dir_all(&workspace).unwrap();
        assert_eq!(
            prune(&config.target.root).unwrap().removed_views,
            1,
            "the earlier placement must still be collectable"
        );
    }

    #[test]
    fn frees_the_target_directory_of_a_checkout_that_is_gone() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let gone = checkout(directory.path(), "gone");
        let staying = checkout(directory.path(), "staying");

        let removed = place(&config, &gone, &gone.join("target"), false).unwrap();
        let kept = place(&config, &staying, &staying.join("target"), false).unwrap();
        std::fs::write(removed.join("artifact"), vec![0_u8; 512]).unwrap();
        std::fs::write(kept.join("artifact"), vec![0_u8; 256]).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();

        let outcome = prune(&config.target.root).unwrap();

        assert_eq!(
            outcome,
            PruneOutcome {
                removed_views: 1,
                removed_bytes: 512,
            }
        );
        assert!(!removed.exists());
        assert!(kept.join("artifact").exists());
        assert_eq!(stats(&config.target.root).unwrap().views, 1);
    }

    #[test]
    fn keeps_the_target_directory_of_an_idle_checkout() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");
        let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();
        std::fs::write(managed.join("artifact"), b"outputs").unwrap();

        let outcome = prune(&config.target.root).unwrap();

        assert_eq!(outcome, PruneOutcome::default());
        assert!(managed.join("artifact").exists());
    }

    #[test]
    fn leaves_a_target_directory_it_cannot_trace() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");
        let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();
        std::fs::remove_dir_all(&workspace).unwrap();
        // `cargo clean` cannot reach the record, but a corrupt one still must
        // not turn into a licence to delete a directory full of outputs.
        std::fs::write(view_record_path(&config.target.root, &workspace), b"{").unwrap();

        assert_eq!(prune(&config.target.root).unwrap(), PruneOutcome::default());
        assert!(managed.exists());
    }

    #[test]
    fn counts_nothing_before_anything_is_placed() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            stats(&directory.path().join("targets")).unwrap(),
            ViewStats::default()
        );
        assert_eq!(
            prune(&directory.path().join("targets")).unwrap(),
            PruneOutcome::default()
        );
    }
}
