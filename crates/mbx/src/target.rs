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
/// - the target directory is not the one cargo would have picked by default,
///   which means a flag, the environment, or a cargo configuration named it,
///   and moving it would be overriding the person who said where it goes;
/// - `<workspace>/target` is a real directory, which is somebody's build
///   outputs. Replacing it with a link would strand them, and deleting it is
///   not this function's business.
pub fn place(config: &Config, workspace_root: &Path, target_dir: &Path) -> Option<PathBuf> {
    if !config.target.views {
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
    let link = match link_view(target_dir, &managed) {
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
        if matches!(link, Link::Created) {
            // Undo this call's own link and nothing else. Left behind it would
            // redirect every later build into a directory no record names.
            if let Err(error) = remove_link(target_dir) {
                log::warn!(
                    "the unused link {} was not removed: {error}",
                    target_dir.display()
                );
            }
        }
        return None;
    }
    // Cargo would create this itself on the way to writing in it. Doing it here
    // keeps the link from dangling in the meantime, which is what someone
    // listing the workspace would see.
    if let Err(error) = std::fs::create_dir_all(&managed) {
        log::warn!(
            "the managed target directory {} was not created: {error}",
            managed.display()
        );
    }
    Some(managed)
}

/// Whether a link had to be made, or was already pointing the right way.
enum Link {
    Existing,
    Created,
}

/// Point `target_dir` at `managed` so the paths people type keep working.
///
/// A missing link is not fatal on its own -- cargo is told where to write
/// either way -- but it is refused rather than forced when something real is
/// already there, and Windows only allows a symlink to be created by a
/// privileged or developer-mode process, where the honest answer is to leave
/// the outputs where cargo would have put them.
fn link_view(target_dir: &Path, managed: &Path) -> Result<Link> {
    match std::fs::read_link(target_dir) {
        // Already pointing where it should. Re-linking would race a concurrent
        // build for no gain.
        Ok(existing) if existing == managed => return Ok(Link::Existing),
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
        Err(_) => {}
    }
    symlink_dir(managed, target_dir)
        .map(|()| Link::Created)
        .wrap_err_with(|| {
            format!(
                "could not link {} to a managed target directory",
                target_dir.display()
            )
        })
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

        let managed = place(&config, &workspace, &workspace.join("target")).unwrap();

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

        let first = place(&config, &workspace, &workspace.join("target")).unwrap();
        let second = place(&config, &workspace, &workspace.join("target")).unwrap();

        assert_eq!(first, second);
        assert_eq!(stats(&config.target.root).unwrap().views, 1);
    }

    #[test]
    fn leaves_the_target_directory_alone_unless_asked() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), false);
        let workspace = checkout(directory.path(), "project");

        assert!(place(&config, &workspace, &workspace.join("target")).is_none());
        assert!(!workspace.join("target").exists());
    }

    #[test]
    fn leaves_a_target_directory_someone_else_chose_where_it_is() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(directory.path(), true);
        let workspace = checkout(directory.path(), "project");

        // A flag, the environment, or a cargo configuration put it here, and
        // that outranks any placement of ours.
        let elsewhere = directory.path().join("chosen");
        assert!(place(&config, &workspace, &elsewhere).is_none());
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

        assert!(place(&config, &workspace, &existing).is_none());

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
        let managed = place(&config, &workspace, &workspace.join("target")).unwrap();
        std::fs::write(managed.join("artifact"), b"outputs").unwrap();

        // Somebody replaced the link with a directory of their own. The
        // placement already on disk still owns a full target directory, and
        // dropping its record would leave that directory untraceable -- which
        // means never collected.
        remove_link(&workspace.join("target")).unwrap();
        std::fs::create_dir_all(workspace.join("target")).unwrap();

        assert!(place(&config, &workspace, &workspace.join("target")).is_none());

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

        let removed = place(&config, &gone, &gone.join("target")).unwrap();
        let kept = place(&config, &staying, &staying.join("target")).unwrap();
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
        let managed = place(&config, &workspace, &workspace.join("target")).unwrap();
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
        let managed = place(&config, &workspace, &workspace.join("target")).unwrap();
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
