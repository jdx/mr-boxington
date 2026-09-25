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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const VIEWS_DIR: &str = "v1";
/// Marks a view moved aside for removal; never part of a view's own name,
/// which is a bare hex digest.
const REMOVAL_SUFFIX: &str = ".removing-";
const VIEW_RECORD_VERSION: u8 = 1;
/// How recently a view's record must have been refreshed for collection to
/// treat a build as having just claimed it. Records carry whole seconds, so a
/// refresh in the same second as the selection is invisible to a comparison;
/// a record this fresh is a build starting, whatever the selection said.
const RECENTLY_CLAIMED: u64 = 2;

/// The clock records and collection share: whole seconds since the epoch.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

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
    /// Selected for removal, then found in use by a build that started after
    /// the selection was made.
    pub kept_active_views: u64,
    /// Units no build had used for the age limit, removed from target
    /// directories that were kept.
    pub removed_units: u64,
    /// Logical bytes of those units, not counted in `removed_bytes`.
    pub removed_unit_bytes: u64,
    pub remaining_bytes: u64,
    pub remaining_views: u64,
}

impl CollectionOutcome {
    /// Logical bytes removed, whole directories and units together.
    pub(crate) fn freed_bytes(&self) -> u64 {
        self.removed_bytes.saturating_add(self.removed_unit_bytes)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub managed: Option<PathBuf>,
    /// Present only when the old directory was actually removed.
    pub removed_bytes: Option<u64>,
}

/// What adopting an existing target directory produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AdoptionOutcome {
    /// The managed directory now holding the outputs; `None` when placement
    /// declined and the directory was put back where it was.
    pub managed: Option<PathBuf>,
    /// Logical bytes the adopted outputs occupy.
    pub adopted_bytes: u64,
}

/// An adoption that moved the outputs but could neither link them nor put them
/// back. Every other adoption failure leaves the outputs where they were.
#[derive(Debug)]
pub struct StrandedAdoption {
    /// Where the outputs now are.
    pub retained: PathBuf,
}

impl std::fmt::Display for StrandedAdoption {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the old target directory was retained at {}",
            self.retained.display()
        )
    }
}

/// Remove the managed target view owned by exactly one workspace.
pub fn remove_workspace(root: &Path, workspace_root: &Path) -> Result<Option<u64>> {
    let record_path = view_record_path(root, workspace_root);
    let Some(record) = read_view_record(&record_path) else {
        // Collection removes the record and directory but cannot remove a
        // link inside a live checkout. Let an explicit clean finish that job
        // once the destination is gone, without deleting an unrecorded view
        // that may still contain outputs.
        let directory = view_dir(root, workspace_root);
        let link = workspace_root.join("target");
        if !directory.exists()
            && std::fs::read_link(&link).is_ok_and(|destination| destination == directory)
        {
            remove_link(&link)?;
            return Ok(Some(0));
        }
        return Ok(None);
    };
    if record.workspace_root != workspace_root {
        return Ok(None);
    }
    let directory = record_path.with_extension("");
    let bytes = tree_bytes(&directory);
    match std::fs::remove_dir_all(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    std::fs::remove_file(&record_path).or_else(|error| {
        (error.kind() == std::io::ErrorKind::NotFound)
            .then_some(())
            .ok_or(error)
    })?;
    let link = workspace_root.join("target");
    if std::fs::read_link(&link).is_ok_and(|destination| destination == directory) {
        remove_link(&link)?;
    }
    Ok(Some(bytes))
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
    if let Err(error) = restore_backup(target_dir, &managed, &backup) {
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

/// Whether an existing target directory can be renamed into the managed root.
///
/// A rename cannot cross filesystems, and copying a target directory is not
/// worth the disk and time it would take, so a checkout on a different volume
/// from the managed root is offered removal instead of adoption. The nearest
/// existing ancestor of the view stands in for a root not created yet.
pub fn can_move_existing(config: &Config, workspace_root: &Path, target_dir: &Path) -> bool {
    let managed = view_dir(&config.target.root, workspace_root);
    managed
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .is_some_and(|existing| crate::store::same_filesystem_paths(target_dir, existing))
}

/// Move a confirmed existing target directory under the managed root, keeping
/// its outputs.
///
/// Cargo's own build locks prove no build is writing into the directory, and
/// holding them until the move keeps one from starting there meanwhile. The
/// directory is then renamed straight into the view and the link and record
/// are placed exactly as for a checkout that had no target directory, so at
/// no point does an empty view exist for a concurrent build to fill: a build
/// that starts after the move finds the outputs already there and the link
/// it would have made itself. Cargo keeps addressing the outputs through the
/// `target` link, so nothing recompiles. If placement declines, the directory
/// goes back where it was.
pub fn adopt_existing(
    config: &Config,
    workspace_root: &Path,
    target_dir: &Path,
    requested: bool,
) -> Result<AdoptionOutcome> {
    adopt_existing_with(config, workspace_root, target_dir, requested, || {
        place(config, workspace_root, target_dir, requested)
    })
}

fn adopt_existing_with(
    config: &Config,
    workspace_root: &Path,
    target_dir: &Path,
    requested: bool,
    place_target: impl FnOnce() -> Option<PathBuf>,
) -> Result<AdoptionOutcome> {
    if !std::fs::symlink_metadata(target_dir).is_ok_and(|metadata| metadata.is_dir()) {
        eyre::bail!(
            "{} is no longer a real target directory, so it was not adopted",
            target_dir.display()
        );
    }
    if !can_remove_existing(config, workspace_root, target_dir, requested) {
        eyre::bail!("{} is not eligible for adoption", target_dir.display());
    }
    if !can_move_existing(config, workspace_root, target_dir) {
        eyre::bail!(
            "{} is not on the same filesystem as the managed target root {}, so it was not adopted",
            target_dir.display(),
            config.target.root.display()
        );
    }
    // A build writing here would keep writing at this path after the rename,
    // with nothing at it any more.
    let Some(build_locks) = cargo_locks(target_dir)? else {
        eyre::bail!(
            "Cargo is using {}, so it was not adopted",
            target_dir.display()
        );
    };
    let adopted_bytes = tree_bytes(target_dir);
    let managed = view_dir(&config.target.root, workspace_root);
    if let Some(parent) = managed.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("could not create {}", parent.display()))?;
    }
    // Recorded before the outputs move, as placement records before it
    // creates a directory: a view must never exist that collection cannot
    // trace to its checkout.
    let record_path = view_record_path(&config.target.root, workspace_root);
    let previous_record = read_record_state(&record_path)?;
    record_view(&config.target.root, workspace_root)?;
    // Held no further: an open handle inside the directory makes Windows
    // refuse to rename it.
    drop(build_locks);
    if let Err(error) = move_into_view(target_dir, &managed) {
        if let Err(restore_error) = restore_record_state(&record_path, previous_record) {
            log::warn!("the target record was not rolled back: {restore_error}");
        }
        return Err(error);
    }
    if let Some(placed) = place_target() {
        return Ok(AdoptionOutcome {
            managed: Some(placed),
            adopted_bytes,
        });
    }
    // Placement declined and undid its own link, so the original path is free
    // again unless something else took it.
    let restore = (|| -> Result<()> {
        match std::fs::symlink_metadata(target_dir) {
            Ok(_) => eyre::bail!(
                "{} was occupied while restoring the old target directory",
                target_dir.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).wrap_err("could not inspect the failed adoption"),
        }
        std::fs::rename(&managed, target_dir)
            .wrap_err_with(|| format!("could not restore {}", target_dir.display()))?;
        restore_record_state(&record_path, previous_record)
    })();
    match restore {
        Ok(()) => Ok(AdoptionOutcome::default()),
        Err(error) => Err(error.wrap_err(StrandedAdoption { retained: managed })),
    }
}

/// Rename a target directory into its view.
///
/// A POSIX rename replaces an empty directory in one step. Windows refuses
/// that, so an empty view is removed first; a view that is not empty holds
/// outputs this call was never asked to replace, and the failure is reported
/// before anything has moved.
fn move_into_view(target_dir: &Path, managed: &Path) -> Result<()> {
    let refused = match std::fs::rename(target_dir, managed) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if std::fs::symlink_metadata(managed).is_err() {
        // Nothing stood in the way, so the refusal is the rename's own.
        return Err(refused)
            .wrap_err_with(|| format!("could not move the outputs into {}", managed.display()));
    }
    std::fs::remove_dir(managed).wrap_err_with(|| {
        format!(
            "could not replace the managed target directory {}",
            managed.display()
        )
    })?;
    std::fs::rename(target_dir, managed)
        .wrap_err_with(|| format!("could not move the outputs into {}", managed.display()))
}

/// Put a backed-up target directory back at its original path, removing the
/// link a failed placement may have left there.
fn restore_backup(target_dir: &Path, managed: &Path, backup: &Path) -> Result<()> {
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
    std::fs::rename(backup, target_dir)
        .wrap_err_with(|| format!("could not restore {}", target_dir.display()))
}

/// Whether placement could use the managed root, without changing any paths.
/// Locks or a later filesystem error can still make placement decline.
pub(crate) fn placement_candidate(
    config: &Config,
    workspace_root: &Path,
    target_dir: &Path,
    requested: bool,
) -> bool {
    if !config.target.views || requested || target_dir != workspace_root.join("target") {
        return false;
    }
    let managed = view_dir(&config.target.root, workspace_root);
    match std::fs::read_link(target_dir) {
        Ok(existing) => {
            existing == managed || replaceable_managed_link(&existing, &managed, workspace_root)
        }
        Err(_) => !target_dir.exists(),
    }
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
    // Cargo keeps its build lock in the old view. Hold those locks across the
    // link swap, or leave the view alone if a build is still using it. Merely
    // changing the link can strand Cargo's resolved diagnostic-output paths.
    let build_locks = match lock_replaced_view(target_dir, &managed, workspace_root) {
        Ok(locks) => locks,
        Err(error) => {
            log::debug!("leaving the managed target directory in place: {error:#}");
            return None;
        }
    };
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
    // The locks proved no build was using the old view and held that answer
    // across the link swap, which is the step that could strand a build's
    // resolved paths. They cannot be held any further: an open handle inside
    // the old view makes Windows refuse to rename or remove it, and the
    // relocation below would fail with the link already pointing at a view
    // that has none of the outputs.
    drop(build_locks);
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
    if let Err(error) = ignore_managed_link(workspace_root, target_dir) {
        // Git integration is a courtesy, not a condition of placing build
        // outputs. A checkout with unusual metadata must still be buildable.
        log::debug!("the managed target link was not excluded from Git: {error:#}");
    }
    Some(managed)
}

/// Keep the managed link out of `git status` without changing tracked files.
///
/// Git gives directory-only patterns such as `target/` different semantics
/// for a symlink, so the pattern that ignored Cargo's directory stops matching
/// after placement. The repository-local exclude file is the right place for
/// mbx's implementation detail: it applies to this checkout while leaving the
/// project's `.gitignore` untouched.
fn ignore_managed_link(workspace_root: &Path, target_dir: &Path) -> Result<()> {
    let ignored = Command::new("git")
        .current_dir(workspace_root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .args(["check-ignore", "--quiet", "--no-index", "--"])
        .arg(target_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if ignored.is_ok_and(|status| status.success()) {
        return Ok(());
    }

    let paths = Command::new("git")
        .current_dir(workspace_root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .args([
            "rev-parse",
            "--show-prefix",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ])
        .output()
        .wrap_err("could not locate the Git repository")?;
    if !paths.status.success() {
        eyre::bail!("the workspace is not in a Git repository");
    }
    let paths = String::from_utf8(paths.stdout).wrap_err("Git returned non-UTF-8 paths")?;
    let mut paths = paths
        .split_terminator('\n')
        .map(|path| path.trim_end_matches('\r'));
    let prefix = PathBuf::from(
        paths
            .next()
            .ok_or_else(|| eyre::eyre!("Git did not report the workspace prefix"))?,
    );
    let exclude = PathBuf::from(
        paths
            .next()
            .ok_or_else(|| eyre::eyre!("Git did not report its exclude file"))?,
    );
    // Git may resolve a symlinked spelling of the worktree (notably `/var` to
    // `/private/var` on macOS). Its own prefix is stable across that boundary;
    // combine it with the part we know relative to Cargo's workspace instead
    // of trying to strip two differently spelled absolute roots.
    let relative = prefix.join(
        target_dir
            .strip_prefix(workspace_root)
            .wrap_err("the target link is outside the Cargo workspace")?,
    );
    let pattern = format!("/{}", gitignore_path(&relative)?);

    let existing = std::fs::read(&exclude).unwrap_or_default();
    if existing
        .split(|byte| *byte == b'\n')
        .any(|line| line.strip_suffix(b"\r").unwrap_or(line) == pattern.as_bytes())
    {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)?;
    let mut addition = Vec::with_capacity(pattern.len() + 2);
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        addition.push(b'\n');
    }
    writeln!(addition, "{pattern}")?;
    file.write_all(&addition)?;
    Ok(())
}

fn gitignore_path(path: &Path) -> Result<String> {
    let path = path.to_string_lossy().replace('\\', "/");
    if path.contains(['\n', '\r']) {
        eyre::bail!("the target path contains a newline");
    }
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        if matches!(character, '\\' | '*' | '?' | '[' | ']' | '#' | '!') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Ok(escaped)
}

/// Whether a link had to be made, or was already pointing the right way.
enum Link {
    Existing,
    Created,
    Replaced(PathBuf),
}

/// Cargo's lock is in `<profile>/.cargo-lock`, or
/// `<target-triple>/<profile>/.cargo-lock` for cross-compilation. Do not follow
/// symlinks into arbitrary directories while inspecting the managed view.
fn lock_replaced_view(
    target_dir: &Path,
    managed: &Path,
    workspace_root: &Path,
) -> Result<Vec<fslock::LockFile>> {
    let Ok(existing) = std::fs::read_link(target_dir) else {
        return Ok(Vec::new());
    };
    if existing == managed
        || !replaceable_managed_link(&existing, managed, workspace_root)
        || !existing.exists()
    {
        return Ok(Vec::new());
    }
    match cargo_locks(&existing)? {
        Some(locks) => Ok(locks),
        None => eyre::bail!("Cargo is using {}", existing.display()),
    }
}

/// Take every Cargo lock in a target directory, or report that one is held.
///
/// `None` means a build is running there. The locks come back held for the
/// caller that needs them held, such as a migration that replaces the view;
/// collection only asks and lets them go.
///
/// Cargo's lock is `<profile>/.cargo-lock`, or
/// `<target-triple>/<profile>/.cargo-lock` when cross-compiling, and an
/// editor's target directory nested one level down repeats that shape, so the
/// walk goes three levels deep. A directory holding a lock is a profile, and
/// the output directories Cargo fills a profile with hold thousands of entries
/// and never a lock, so those are not entered. The same names above a lock
/// are entered: a custom profile may be called `deps`.
fn cargo_locks(directory: &Path) -> Result<Option<Vec<fslock::LockFile>>> {
    let mut pending = vec![(directory.to_path_buf(), 0)];
    let mut locks = Vec::new();
    while let Some((directory, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            // A directory that vanished under the walk holds no lock.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut is_profile = false;
        let mut children = Vec::new();
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_file() && entry.file_name() == ".cargo-lock" {
                let mut lock = fslock::LockFile::open(&entry.path())?;
                if !lock.try_lock()? {
                    return Ok(None);
                }
                locks.push(lock);
                is_profile = true;
            } else if kind.is_dir() && depth < 3 {
                children.push((entry.file_name(), entry.path()));
            }
        }
        for (name, path) in children {
            if is_profile && is_profile_output_dir(&name) {
                continue;
            }
            pending.push((path, depth + 1));
        }
    }
    Ok(Some(locks))
}

/// Whether a directory name is one Cargo creates inside a profile directory
/// for outputs, beside the lock rather than above one.
fn is_profile_output_dir(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some("deps" | "build" | "incremental" | "examples" | ".fingerprint" | "doc")
    )
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
            || !crate::store::checkout_is_live_on(
                managed.parent().unwrap_or(managed),
                &record.workspace_root,
            );
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

/// Mark an existing managed view as used when this build wrote into it.
///
/// Placement records a view every time it happens, but a build can write into a
/// managed directory without placing it: turning `target.views` off, or naming a
/// target directory explicitly, leaves the symlink an earlier build created and
/// cargo keeps writing through it. The record would then stop being refreshed
/// while the directory stayed in daily use, and age-based collection would
/// eventually delete outputs somebody was still building against.
///
/// Only an existing record is refreshed. A directory that is not the managed
/// view for this workspace gets nothing, so a checkout that has genuinely gone
/// back to its own `target/` still expires on schedule.
pub fn touch_managed(config: &Config, workspace_root: &Path, target_dir: &Path) {
    let record_path = view_record_path(&config.target.root, workspace_root);
    if !record_path.exists() {
        return;
    }
    let managed = view_dir(&config.target.root, workspace_root);
    // Resolved on both sides: `target_dir` is typically the symlink, and the
    // managed root itself may sit behind one.
    let (Ok(written), Ok(expected)) = (
        std::fs::canonicalize(target_dir),
        std::fs::canonicalize(&managed),
    ) else {
        return;
    };
    if written != expected {
        return;
    }
    if let Err(error) = record_view(&config.target.root, workspace_root) {
        log::debug!("the managed target record was not refreshed: {error}");
    }
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
    collect_with(root, max_bytes, max_age, dry_run, now_secs(), || {}, || {})
}

/// [`collect`] with the sweep's clock, and with hooks where a build can
/// arrive: between selecting views and removing them, and between removing a
/// directory and its record. Tests stand in for that build, and pass a fixed
/// `now` so the ages they set up do not shift while the sweep runs.
fn collect_with(
    root: &Path,
    max_bytes: Option<u64>,
    max_age: Option<Duration>,
    dry_run: bool,
    now: u64,
    before_removal: impl FnOnce(),
    mut after_removal: impl FnMut(),
) -> Result<CollectionOutcome> {
    let mut outcome = CollectionOutcome::default();
    if !dry_run {
        remove_abandoned_removals(root);
    }
    let mut entries = Vec::new();
    let mut total_views = 0_u64;
    let mut uncollectable_bytes = 0_u64;
    for (record_path, directory) in views(root)? {
        total_views += 1;
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
            crate::store::checkout_is_live_on(root, &record.workspace_root),
        ));
    }
    let expired =
        |updated: u64| max_age.is_some_and(|age| now.saturating_sub(updated) > age.as_secs());
    // Before the budget is weighed, so a checkout's unused units go ahead of
    // a whole directory somebody may still be using.
    if let Some(max_age) = max_age
        && entries.iter().any(|entry| entry.4 && !expired(entry.2))
        && crate::target_units::access_times_tracked(&views_root(root))
    {
        for (_, directory, updated, bytes, live) in &mut entries {
            if !*live || expired(*updated) {
                continue;
            }
            // Held while units go, so a build that starts meanwhile waits
            // rather than reading a fingerprint whose outputs are leaving.
            let _locks = if dry_run {
                None
            } else {
                match cargo_locks(directory) {
                    Ok(Some(locks)) => Some(locks),
                    Ok(None) => continue,
                    Err(error) => {
                        log::warn!(
                            "could not tell whether {} is in use: {error}",
                            directory.display()
                        );
                        continue;
                    }
                }
            };
            let units = crate::target_units::prune(
                directory,
                max_age,
                UNIX_EPOCH + Duration::from_secs(now),
                dry_run,
            );
            *bytes = bytes.saturating_sub(units.removed_bytes);
            outcome.removed_units += units.removed_units;
            outcome.removed_unit_bytes += units.removed_bytes;
        }
    }
    let mut remaining = entries
        .iter()
        .map(|entry| entry.3)
        .fold(uncollectable_bytes, u64::saturating_add);
    let mut selected = HashSet::new();
    for (record_path, _, updated, bytes, live) in &entries {
        if !live || expired(*updated) {
            selected.insert(record_path.clone());
            remaining = remaining.saturating_sub(*bytes);
        }
    }
    if let Some(max_bytes) = max_bytes
        && remaining > max_bytes
    {
        entries.sort_by_key(|entry| entry.2);
        // Only the views the passes above left alone. An abandoned or expired
        // one is going regardless, so letting it hold the protected place below
        // would spend that protection on a directory already being deleted.
        let mut candidates: Vec<&(PathBuf, PathBuf, u64, u64, bool)> = entries
            .iter()
            .filter(|entry| !selected.contains(&entry.0))
            .collect();
        // Spare the most recently used of them: that is the checkout somebody
        // is almost certainly working in, very likely the one whose build just
        // called this. Deleting it cannot hold the total down anyway, because
        // the next build recreates it, so a budget smaller than one working
        // target directory would otherwise delete those outputs after every
        // build forever.
        candidates.pop();
        for entry in candidates {
            if remaining <= max_bytes {
                break;
            }
            if selected.insert(entry.0.clone()) {
                remaining = remaining.saturating_sub(entry.3);
            }
        }
        if remaining > max_bytes {
            // Say so rather than delete the last directory standing: the
            // budget cannot be met, and the user is the only one who can
            // decide whether to raise it or keep fewer checkouts.
            log::warn!(
                "managed target directories still hold {} after collection, over the {} budget; the most recently used one is kept",
                bytesize::ByteSize::b(remaining).display().iec(),
                bytesize::ByteSize::b(max_bytes).display().iec(),
            );
        }
    }

    before_removal();
    for (record_path, directory, updated, bytes, live) in entries {
        if !selected.contains(&record_path) {
            continue;
        }
        // The selection above is a snapshot, and collection runs in a process
        // of its own after the build that scheduled it: a build can begin in
        // one of the selected checkouts while the earlier ones are still being
        // removed. Its placement refreshes the record, and Cargo holds its
        // lock for as long as it compiles, so either is grounds to leave the
        // directory standing until the next sweep looks again. Every selected
        // view gets the checks, not only the live ones: a checkout deleted and
        // cloned again at the same path has the same view, and the clone can
        // start building it before its predecessor's directory is gone.
        if !dry_run {
            let claimed_since = read_view_record(&record_path)
                .is_some_and(|record| recently_claimed(root, &record, updated, now));
            let in_use = match cargo_locks(&directory) {
                Ok(locks) => locks.is_none(),
                Err(error) => {
                    log::warn!(
                        "could not tell whether {} is in use: {error}",
                        directory.display()
                    );
                    true
                }
            };
            if claimed_since || in_use {
                outcome.kept_active_views += 1;
                remaining = remaining.saturating_add(bytes);
                continue;
            }
        }
        // Moved aside in one step before its files go. Holding Cargo's lock
        // would not keep a build out: unlinking the lock file frees it, and
        // Cargo simply creates another. A build that starts after the rename
        // finds no directory and makes a fresh one, the same as it would after
        // the removal finished; a tree that is half gone is never at the path
        // a build can reach. Windows refuses the rename while anything inside
        // is open, which is the same answer.
        let removal = if dry_run {
            Ok(())
        } else {
            let aside = removal_path(&directory);
            std::fs::rename(&directory, &aside).and_then(|()| std::fs::remove_dir_all(&aside))
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
        if !dry_run {
            after_removal();
        }
        // Last, so a failed removal above leaves the record to try again with.
        // And only if it still describes the directory just removed: a build
        // that placed the checkout while the removal ran has written a new
        // record and made a new directory, and taking that record would leave
        // the directory invisible to every later collection.
        let superseded = !dry_run
            && (read_view_record(&record_path)
                .is_some_and(|record| recently_claimed(root, &record, updated, now))
                || directory.exists());
        if !dry_run
            && !superseded
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
    outcome.remaining_views = total_views.saturating_sub(outcome.removed_views);
    Ok(outcome)
}

/// Whether a record read back during collection shows a build claiming the
/// view since it was selected on `updated`.
///
/// `now` is the sweep's own clock, the one the selection compared ages
/// against, so how fresh a record counts as does not depend on how long the
/// removals before this one took.
///
/// The grace for a same-second refresh applies only while the checkout
/// exists: a record written moments ago for a checkout that is already gone
/// is a test fixture or a deleted clone, not a build about to start.
fn recently_claimed(root: &Path, record: &ViewRecord, updated: u64, now: u64) -> bool {
    if record.updated_secs > updated {
        return true;
    }
    now.saturating_sub(record.updated_secs) <= RECENTLY_CLAIMED
        && crate::store::checkout_is_live_on(root, &record.workspace_root)
}

/// Where a view goes while its files are removed.
fn removal_path(directory: &Path) -> PathBuf {
    let name = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    directory.with_file_name(format!("{name}{REMOVAL_SUFFIX}{}", std::process::id()))
}

/// Finish removing views a collector that died left moved aside.
///
/// They have no record, so nothing else lists or protects them; a collector
/// still deleting one holds no lock either, but the removal is idempotent and
/// two of them racing only means the files go sooner.
fn remove_abandoned_removals(root: &Path) {
    let Ok(listing) = std::fs::read_dir(views_root(root)) else {
        return;
    };
    for entry in listing.flatten() {
        if entry.file_name().to_string_lossy().contains(REMOVAL_SUFFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
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
        updated_secs: now_secs(),
    };
    let mut contents = serde_json::to_vec(&record)?;
    contents.push(b'\n');
    crate::util::write_advisory(&view_record_path(root, workspace_root), &contents)
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
        return crate::util::write_advisory(path, &contents);
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

pub(crate) fn view_dir(root: &Path, workspace_root: &Path) -> PathBuf {
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

pub(crate) fn tree_bytes(directory: &Path) -> u64 {
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
