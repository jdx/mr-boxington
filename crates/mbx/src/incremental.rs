//! Persistent per-checkout state for learned incremental compilation.

use crate::retired::{self, RetiredTree};
use eyre::{Context, Result};
use mbx_cache_core::CacheDigest;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SESSION_START_GRACE_SECS: u64 = 60;

/// A unit's last-use stamp, `<unit>.use` beside its directory and churn
/// record. Its mtime is the unit's last use; its content is empty, or
/// `r <budget-bytes>` while the compile path's backstop rejects the unit's
/// state under that budget. Nothing ever locks it.
const USE_STAMP_EXTENSION: &str = "use";
/// A cache hit refreshes a stamp at most this often.
const USE_STAMP_THROTTLE_SECS: u64 = 60;
/// Under budget pressure, a live checkout keeps the sessions of units used
/// this recently, unless they have expired, outgrown the backstop, or the
/// disk is short.
const HOT_FLOOR_SECS: u64 = 60 * 60;

const RECORD_FILE: &str = "checkout.json";
const RECORD_VERSION: u8 = 1;
const LOCKS_DIR: &str = ".locks";
const REGISTRAR_FILE: &str = "registrar.lock";
const EPOCH_FILE: &str = "epoch";
const COPY_STATE_EPOCH: u64 = 1;

#[derive(Serialize, Deserialize)]
struct StateEpoch {
    epoch: u64,
    pending: Vec<String>,
}

fn read_epoch(root: &Path) -> Option<StateEpoch> {
    serde_json::from_slice(&std::fs::read(root.join(EPOCH_FILE)).ok()?).ok()
}

fn copy_state_ready(root: &Path) -> bool {
    read_epoch(root)
        .is_some_and(|state| state.epoch >= COPY_STATE_EPOCH && state.pending.is_empty())
}

/// Older seeds shared writable files, and corruption can remain after the
/// links disappear. Discard all old state once, waiting for active builds.
fn discard_pre_copy_state(root: &Path) -> Result<()> {
    if copy_state_ready(root) {
        return Ok(());
    }
    let registrar_path = registrar_path(root);
    std::fs::create_dir_all(registrar_path.parent().expect("registrar has a parent"))?;
    let mut registrar = fslock::LockFile::open(&registrar_path)?;
    registrar.lock()?;
    let state = read_epoch(root);
    let state = match state {
        Some(state) if state.epoch >= COPY_STATE_EPOCH => state,
        _ => {
            let mut pending = Vec::new();
            for entry in std::fs::read_dir(root)? {
                let entry = entry?;
                if entry.file_type()?.is_dir()
                    && entry.file_name() != LOCKS_DIR
                    && !retired::is_retired_name(&entry.file_name())
                {
                    pending.push(
                        entry
                            .file_name()
                            .into_string()
                            .map_err(|_| eyre::eyre!("incremental checkout name is not UTF-8"))?,
                    );
                }
            }
            pending.sort();
            let state = StateEpoch {
                epoch: COPY_STATE_EPOCH,
                pending,
            };
            // Save the original set before renaming anything. After a crash,
            // newly created checkouts must not become migration candidates.
            crate::util::write_atomic(&root.join(EPOCH_FILE), &serde_json::to_vec(&state)?)?;
            state
        }
    };
    if state.pending.is_empty() {
        return Ok(());
    }
    let mut pending = Vec::new();
    let mut retired_count = 0;
    for key in state.pending {
        let result = (|| -> Result<bool> {
            // Epoch keys name direct children, never paths outside the root.
            if Path::new(&key).components().count() != 1
                || !matches!(
                    Path::new(&key).components().next(),
                    Some(std::path::Component::Normal(_))
                )
                || key == LOCKS_DIR
                || retired::is_retired_name(key.as_ref())
            {
                eyre::bail!("invalid incremental checkout key {key:?}");
            }
            let locks = lock_dir(root, &key);
            if probe_leases(&locks, true)? {
                return Ok(false);
            }
            if let Some(tree) = RetiredTree::retire(&root.join(&key))? {
                match std::fs::remove_file(tree.path().join(RECORD_FILE)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        // Leave a failed retirement at its original name so
                        // the pending key still refers to the old state.
                        std::fs::rename(tree.path(), root.join(&key))?;
                        return Err(error.into());
                    }
                }
                retired_count += 1;
            }
            cleanup_lock_dir(&locks);
            Ok(true)
        })();
        match result {
            Ok(true) => {}
            Ok(false) => pending.push(key),
            Err(error) => {
                log::warn!("could not discard old incremental state for {key}: {error}");
                pending.push(key);
            }
        }
    }
    crate::util::write_atomic(
        &root.join(EPOCH_FILE),
        &serde_json::to_vec(&StateEpoch { pending, ..state })?,
    )?;
    drop(registrar);
    if retired_count > 0 {
        log::info!(
            "discarded incremental state from {retired_count} checkouts left by an older mbx version; it will be rebuilt because hard-linked seeds could overwrite another checkout's state"
        );
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutRecord {
    version: u8,
    workspace_root: PathBuf,
    updated_secs: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    pub directories: u64,
    pub units: u64,
    pub bytes: u64,
    pub live_checkouts: u64,
    pub stale_checkouts: u64,
    pub untracked_directories: u64,
}

#[derive(Debug, Default)]
pub(crate) struct PruneOutcome {
    pub removed_units: u64,
    pub remaining_units: u64,
    pub skipped_active_units: u64,
    pub removed_directories: u64,
    pub removed_bytes: u64,
    pub remaining_directories: u64,
    pub remaining_bytes: u64,
    pub skipped_active_directories: u64,
    pub untracked_directories: u64,
    /// Finished rustc sessions removed from units of live checkouts.
    pub removed_sessions: u64,
    /// The units of live checkouts those sessions came from, by directory,
    /// so that passes over the same units count each once.
    pub session_units: std::collections::BTreeSet<PathBuf>,
    /// Units of live checkouts kept because they were used in the last hour.
    pub kept_hot_units: u64,
}

/// A session's proof that one checkout may be using learned incremental state.
pub(crate) struct ActiveLease {
    lock: Option<fslock::LockFile>,
    path: PathBuf,
    registrar_path: PathBuf,
}

impl Drop for ActiveLease {
    fn drop(&mut self) {
        let mut registrar = fslock::LockFile::open(&self.registrar_path).ok();
        if let Some(registrar) = &mut registrar {
            let _ = registrar.lock();
        }
        drop(self.lock.take());
        let _ = std::fs::remove_file(&self.path);
        if let Some(locks) = self.path.parent() {
            cleanup_lock_dir(locks);
        }
    }
}

pub(crate) struct Checkout {
    pub directory: PathBuf,
    pub lease: ActiveLease,
}

/// Refresh one checkout's claim, mark it active, and return its private state.
pub(crate) fn touch(root: &Path, workspace_root: &Path) -> Result<Checkout> {
    discard_pre_copy_state(root)?;
    let key = checkout_key(workspace_root);
    let directory = root.join(&key);
    let lease = acquire_lease(root, &key)?;
    std::fs::create_dir_all(&directory).wrap_err_with(|| {
        format!(
            "failed to create the incremental state directory {}",
            directory.display()
        )
    })?;
    let record = CheckoutRecord {
        version: RECORD_VERSION,
        workspace_root: workspace_root.to_path_buf(),
        updated_secs: now_secs(),
    };
    crate::util::write_advisory(&directory.join(RECORD_FILE), &serde_json::to_vec(&record)?)?;
    Ok(Checkout { directory, lease })
}

/// Inspect all traceable learned incremental checkout state.
pub(crate) fn stats(root: &Path) -> Result<Stats> {
    let mut stats = Stats::default();
    let snapshot = entries(root, true)?;
    stats.bytes = snapshot.inodes.bytes;
    for entry in snapshot.entries {
        stats.directories += 1;
        stats.units += entry.units.len() as u64;
        match entry
            .workspace_root
            .as_ref()
            .map(|workspace| crate::store::checkout_is_live_on(root, workspace))
        {
            Some(true) => stats.live_checkouts += 1,
            Some(false) => stats.stale_checkouts += 1,
            None => stats.untracked_directories += 1,
        }
    }
    Ok(stats)
}

/// List workspaces with readable learned incremental state records.
pub(crate) fn workspaces(root: &Path) -> Result<Vec<PathBuf>> {
    let listing = match std::fs::read_dir(root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut workspaces = Vec::new();
    for entry in listing {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || entry.file_name() == LOCKS_DIR
            || retired::is_retired_name(&entry.file_name())
        {
            continue;
        }
        if let Some(record) = read_record(&entry.path().join(RECORD_FILE)) {
            workspaces.push(record.workspace_root);
        }
    }
    Ok(workspaces)
}

/// Collect expired units, then units in least-recently-used order.
/// Deleted checkouts remain donors until their last unit is collected.
///
/// A unit of a live checkout keeps its directory: only its finished rustc
/// sessions go, each under rustc's own lock (see [`evict_session`]). With
/// `hot_floor`, such a unit used in the last hour is kept, unless it expired
/// or its state outgrew the backstop the compiler applies, half of
/// `backstop_budget`. That is the incremental budget compilers are given,
/// not `max_bytes`, which generated sources and a combined budget can
/// narrow: a unit the compiler accepts must not count as oversized here.
pub(crate) fn collect(
    root: &Path,
    max_bytes: Option<u64>,
    max_age: Option<Duration>,
    dry_run: bool,
    hot_floor: bool,
    backstop_budget: Option<u64>,
) -> Result<PruneOutcome> {
    collect_with_unlink(
        root,
        max_bytes,
        max_age,
        dry_run,
        hot_floor,
        backstop_budget,
        unlink_retired_checkout_record,
    )
}

fn collect_with_unlink(
    root: &Path,
    max_bytes: Option<u64>,
    max_age: Option<Duration>,
    dry_run: bool,
    hot_floor: bool,
    backstop_budget: Option<u64>,
    unlink: impl FnMut(&RetiredTree, Option<&mut Inodes>) -> std::io::Result<u64>,
) -> Result<PruneOutcome> {
    collect_with_retire_and_unlink(
        root,
        max_bytes,
        max_age,
        dry_run,
        hot_floor,
        backstop_budget,
        CollectHooks {
            retire: RetiredTree::retire,
            unlink,
        },
    )
}

struct CollectHooks<R, U> {
    retire: R,
    unlink: U,
}

fn collect_with_retire_and_unlink(
    root: &Path,
    max_bytes: Option<u64>,
    max_age: Option<Duration>,
    dry_run: bool,
    hot_floor: bool,
    backstop_budget: Option<u64>,
    mut hooks: CollectHooks<
        impl FnMut(&Path) -> std::io::Result<Option<RetiredTree>>,
        impl FnMut(&RetiredTree, Option<&mut Inodes>) -> std::io::Result<u64>,
    >,
) -> Result<PruneOutcome> {
    let swept_bytes = sweep_retired(root, dry_run);
    let now = now_secs();
    // A preview's retired sweep is projected separately; real sweeps leave
    // any survivors in the snapshot so their bytes stay against the budget.
    let Snapshot {
        entries,
        mut inodes,
    } = entries(root, !dry_run)?;
    let mut remaining = entries
        .iter()
        .map(|entry| entry.units.len())
        .collect::<Vec<_>>();
    let mut outcome = PruneOutcome {
        remaining_directories: entries.len() as u64,
        remaining_units: remaining.iter().sum::<usize>() as u64,
        remaining_bytes: inodes.bytes,
        untracked_directories: entries
            .iter()
            .filter(|entry| entry.workspace_root.is_none())
            .count() as u64,
        ..PruneOutcome::default()
    };
    let mut active = HashSet::new();
    let mut removed = HashSet::new();
    let mut kept_records = HashSet::new();
    let mut trimming = HashMap::new();
    let expired =
        |unit: &Unit| max_age.is_some_and(|age| now.saturating_sub(unit.last_used) > age.as_secs());
    let mut units = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.workspace_root.is_some())
        .flat_map(|(index, entry)| entry.units.iter().map(move |unit| (index, unit)))
        .collect::<Vec<_>>();
    units.sort_by_key(|(_, unit)| (!expired(unit), unit.last_used, &unit.directory));
    let satisfied =
        |unit: &Unit, bytes: u64| !expired(unit) && max_bytes.is_none_or(|budget| bytes <= budget);
    for (index, unit) in units {
        if satisfied(unit, inodes.bytes) {
            break;
        }
        let entry = &entries[index];
        // seed() holds the donor's checkout lease while linking its sessions.
        let guard = match deletion_guard(root, &entry.key, !dry_run) {
            Ok(guard) => guard,
            Err(error) => {
                log::debug!("incremental checkout disappeared or could not be reserved: {error}");
                continue;
            }
        };
        if guard.active {
            // Lease creation must not wait for session eviction.
            drop(guard);
            active.insert(index);
            // The compiler's backstop, by the same measure: state it would
            // reject is not kept however recently hits refreshed it.
            if hot_floor
                && !expired(unit)
                && now.saturating_sub(unit.last_used) < HOT_FLOOR_SECS
                && !exceeds_backstop(&unit.directory, backstop_budget)
            {
                outcome.kept_hot_units += 1;
                continue;
            }
            if !dry_run && let Err(error) = lease_for_trimming(&mut trimming, root, &entry.key) {
                log::debug!("incremental checkout could not be leased: {error}");
                outcome.skipped_active_units += 1;
                continue;
            }
            let mut evicted = false;
            // A staging directory is a seed's, not rustc's.
            for session in unit
                .sessions
                .iter()
                .filter(|_| !is_seeding(&unit.directory))
            {
                if satisfied(unit, inodes.bytes) {
                    break;
                }
                if !session_is_settled(session, now) {
                    continue;
                }
                if dry_run {
                    inodes.release(&session.files);
                    outcome.removed_sessions += 1;
                    evicted = true;
                    continue;
                }
                // Bytes are credited only for files whose last link went,
                // inside `evict_session`. A file another owner holds, or that
                // a compile or seed linked after the snapshot, is still on
                // disk however many measured owners it has left.
                match evict_session(session, &mut inodes) {
                    Some(true) => {
                        outcome.removed_sessions += 1;
                        evicted = true;
                    }
                    // Renamed out of rustc's reach but only partly removed:
                    // the unit was still trimmed. rustc's own cleanup of
                    // unfinished sessions, or the unit's removal, takes
                    // the rest.
                    Some(false) => evicted = true,
                    None => {}
                }
            }
            if evicted {
                outcome.session_units.insert(unit.directory.clone());
            } else {
                outcome.skipped_active_units += 1;
            }
            continue;
        }
        let Some(locks) = unit_locks(&unit.directory, dry_run)? else {
            active.insert(index);
            outcome.skipped_active_units += 1;
            continue;
        };
        // The session locks proved no compiler is using the unit. Close them
        // before the rename: Windows cannot rename a directory while a file
        // inside it is open, and these lock files are inside. The registrar is
        // still held, so no mbx compile can start in the unit meanwhile.
        drop(locks);
        // Detach the unit and its stamps before another lease can recreate
        // either name. The recursive walk runs without the registrar.
        let mut retired_records = Vec::new();
        let retired_unit = if dry_run {
            None
        } else {
            let mut record_failure = None;
            for record in &unit.records {
                match (hooks.retire)(record) {
                    Ok(Some(tree)) => retired_records.push((record.clone(), tree)),
                    Ok(None) => {}
                    Err(error) => {
                        record_failure = Some((record, error));
                        break;
                    }
                }
            }
            if let Some((record, error)) = record_failure {
                log::warn!(
                    "could not retire incremental record {}: {error}",
                    record.display()
                );
                restore_unit_records(&retired_records, &error);
                kept_records.insert(index);
                continue;
            }
            match (hooks.retire)(&unit.directory) {
                Ok(tree) => tree,
                Err(error) => {
                    log::warn!(
                        "could not retire incremental unit {}: {error}",
                        unit.directory.display()
                    );
                    restore_unit_records(&retired_records, &error);
                    kept_records.insert(index);
                    continue;
                }
            }
        };
        drop(guard);
        let unit_removed = dry_run
            || retired_unit
                .as_ref()
                .is_none_or(|tree| remove_tree(tree.path(), false, &mut inodes));
        if unit_removed {
            if dry_run {
                inodes.release(&unit.files);
            }
            let records_removed = if dry_run {
                true
            } else {
                // Try every stamp even if an earlier deletion failed.
                retired_records
                    .iter()
                    .filter(|(_, record)| !remove_tree(record.path(), false, &mut inodes))
                    .count()
                    == 0
            };
            if records_removed {
                if dry_run {
                    inodes.release(&unit.record_files);
                }
            } else {
                kept_records.insert(index);
            }
            remaining[index] -= 1;
            removed.insert(unit.directory.clone());
            outcome.removed_units += 1;
        }
    }
    // A stamp whose unit is gone is left over from an interrupted removal, or
    // a hit that raced one. A lease means a build may be recreating its unit.
    for entry in entries
        .iter()
        .filter(|entry| entry.workspace_root.is_some())
    {
        if entry.orphan_stamps.is_empty() {
            continue;
        }
        // A preview probes the lease as a real run does, and counts what it
        // would remove without removing it.
        match deletion_guard(root, &entry.key, !dry_run) {
            Ok(guard) if !guard.active => {
                for (stamp, files) in &entry.orphan_stamps {
                    if remove_tree(stamp, dry_run, &mut inodes) && dry_run {
                        inodes.release(files);
                    }
                }
            }
            Ok(_) => {}
            Err(error) => log::debug!("incremental checkout could not be reserved: {error}"),
        }
    }
    // Only empty, deleted checkouts lose their bookkeeping. An abandoned
    // checkout with usable units is still a donor, regardless of its path.
    for (index, entry) in entries.iter().enumerate() {
        let Some(workspace) = &entry.workspace_root else {
            continue;
        };
        if remaining[index] != 0
            || kept_records.contains(&index)
            || crate::store::checkout_is_live_on(root, workspace)
        {
            continue;
        }
        let guard = match deletion_guard(root, &entry.key, !dry_run) {
            Ok(guard) => guard,
            Err(error) => {
                log::debug!("incremental checkout could not be reserved: {error}");
                continue;
            }
        };
        if guard.active {
            active.insert(index);
            continue;
        }
        // A compiler may have added a unit since enumeration. The registrar
        // now prevents a new lease; do not remove newly discovered state.
        if nested_entries(&entry.directory.join("blake3"))
            .iter()
            .any(|unit| {
                unit.file_type().is_ok_and(|kind| kind.is_dir())
                    && !retired::is_retired_name(&unit.file_name())
                    && (!dry_run || !removed.contains(&unit.path()))
            })
        {
            continue;
        }
        let tree = if dry_run {
            None
        } else {
            let tree = match RetiredTree::retire(&entry.directory) {
                Ok(tree) => tree,
                Err(error) => {
                    log::warn!(
                        "could not retire incremental checkout {}: {error}",
                        entry.directory.display()
                    );
                    continue;
                }
            };
            if let Some(tree) = &tree
                && let Err(error) = (hooks.unlink)(tree, Some(&mut inodes))
            {
                restore_retired_checkout(tree, &entry.directory, &error)?;
                log::warn!(
                    "could not unlink incremental checkout record {}: {error}",
                    entry.directory.display()
                );
                continue;
            }
            guard.cleanup();
            tree
        };
        drop(guard);
        let removed = if dry_run {
            inodes.release(&entry.files);
            true
        } else {
            tree.as_ref()
                .is_none_or(|tree| remove_tree(tree.path(), false, &mut inodes))
        };
        if removed {
            outcome.removed_directories += 1;
        }
    }
    outcome.removed_bytes = outcome
        .remaining_bytes
        .saturating_sub(inodes.bytes)
        .saturating_add(swept_bytes);
    outcome.remaining_bytes = inodes.bytes;
    outcome.skipped_active_directories = active.len() as u64;
    outcome.remaining_directories -= outcome.removed_directories;
    outcome.remaining_units -= outcome.removed_units;
    if let Some(max_bytes) = max_bytes
        && outcome.remaining_bytes > max_bytes
    {
        log::warn!(
            "learned incremental state still holds {} after collection, over the {} budget; active units, untracked checkouts, and checkout bookkeeping are kept",
            bytesize::ByteSize::b(outcome.remaining_bytes)
                .display()
                .iec(),
            bytesize::ByteSize::b(max_bytes).display().iec(),
        );
    }
    Ok(outcome)
}

fn restore_unit_records(records: &[(PathBuf, RetiredTree)], cause: &std::io::Error) {
    for (live, retired) in records {
        if let Err(restore_error) = std::fs::rename(retired.path(), live) {
            log::warn!(
                "could not restore incremental record {} from {} after retirement failed ({cause}): {restore_error}",
                live.display(),
                retired.path().display()
            );
        }
    }
}

/// Delete retired trees a crash or an upgrade left behind, returning the bytes
/// freed. A preview deletes nothing and returns what deleting would free.
///
/// Before the live-state snapshot, all retired trees are measured together
/// and a file is credited only when its last link goes. A file linked from
/// two retired trees counts once, and one still linked from a live checkout
/// frees nothing. Survivors are then measured alongside live state so they
/// remain visible to the budget.
fn sweep_retired(root: &Path, dry_run: bool) -> u64 {
    let trees = retired_trees(root);
    if dry_run {
        return reclaimable(&trees);
    }
    let mut inodes = Inodes::default();
    for tree in &trees {
        inodes.measure(tree, &HashSet::new());
    }
    let measured = inodes.bytes;
    for tree in &trees {
        remove_tree(tree, false, &mut inodes);
    }
    measured.saturating_sub(inodes.bytes)
}

fn retired_trees(root: &Path) -> Vec<PathBuf> {
    let mut trees = retired_children(root);
    for entry in nested_entries(root) {
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && entry.file_name() != LOCKS_DIR
            && !retired::is_retired_name(&entry.file_name())
        {
            trees.extend(retired_children(&entry.path().join("blake3")));
        }
    }
    trees
}

/// What deleting `trees` would free: each file once, and only when every link
/// it has is inside them, which is when a real removal would credit it.
fn reclaimable(trees: &[PathBuf]) -> u64 {
    let mut files: HashMap<FileKey, (u64, u64, Option<u64>)> = HashMap::new();
    for tree in trees {
        let mut pending = vec![tree.clone()];
        while let Some(path) = pending.pop() {
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                pending.extend(nested_entries(&path).into_iter().map(|entry| entry.path()));
            } else if metadata.is_file() {
                let (key, links) = file_identity(&path, &metadata);
                files.entry(key).or_insert((metadata.len(), 0, links)).1 += 1;
            }
        }
    }
    files
        .values()
        .filter(|(_, seen, links)| links.is_some_and(|links| *seen >= links))
        .map(|(bytes, ..)| bytes)
        .sum()
}

fn retired_children(parent: &Path) -> Vec<PathBuf> {
    nested_entries(parent)
        .into_iter()
        .filter(|entry| retired::is_retired_name(&entry.file_name()))
        .map(|entry| entry.path())
        .collect()
}

/// Remove a tree or a single file. A file whose every link went is forgotten
/// in the ledger whichever owners still reference it: the data is gone. Other
/// files are released by their owners as before.
///
/// The removal is `std::fs::remove_dir_all`, which never follows a link, even
/// one swapped in for a directory while it runs; symlinks inside are removed
/// themselves and free nothing measured.
fn remove_tree(directory: &Path, dry_run: bool, inodes: &mut Inodes) -> bool {
    if dry_run {
        return true;
    }
    let removal = crate::disk_usage::remove_tree_reporting(
        directory,
        |path, metadata| metadata.is_file().then(|| file_identity(path, metadata)),
        |(key, _)| key.clone(),
    );
    // Link counts are from before the removal, so a file linked twice inside
    // the tree went only when both links did.
    let mut removed = HashMap::<FileKey, (u64, Option<u64>)>::new();
    for (key, links) in removal.files {
        removed.entry(key).or_insert((0, links)).0 += 1;
    }
    for (key, (paths, links)) in removed {
        if links.is_some_and(|links| paths >= links) {
            inodes.forget(&key);
        }
    }
    match removal.result {
        Ok(()) => true,
        Err(error) => {
            log::warn!(
                "could not remove incremental state {}: {error}",
                directory.display()
            );
            false
        }
    }
}

struct Unit {
    directory: PathBuf,
    /// Every owner's references: the finished sessions' and the rest's.
    files: Vec<FileKey>,
    last_used: u64,
    /// Finished sessions, oldest first, each its own owner in the ledger.
    sessions: Vec<Session>,
    /// The churn record and last-use stamp beside the unit, which go with it.
    records: Vec<PathBuf>,
    record_files: Vec<FileKey>,
}

fn nested_entries(directory: &Path) -> Vec<std::fs::DirEntry> {
    match std::fs::read_dir(directory) {
        Ok(listing) => listing
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    warn_walk_error(directory, &error);
                    None
                }
            })
            .collect(),
        Err(error) => {
            warn_walk_error(directory, &error);
            Vec::new()
        }
    }
}

fn warn_walk_error(path: &Path, error: &std::io::Error) {
    if error.kind() != std::io::ErrorKind::NotFound {
        log::warn!(
            "could not inspect incremental state {}: {error}",
            path.display()
        );
    }
}

/// A checkout's units, and the last-use stamps whose unit is gone.
fn units(directory: &Path, inodes: &mut Inodes) -> (Vec<Unit>, Vec<(PathBuf, Vec<FileKey>)>) {
    let mut units = Vec::new();
    let mut orphans = Vec::new();
    for entry in nested_entries(&directory.join("blake3")) {
        if retired::is_retired_name(&entry.file_name()) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            units.extend(measure_unit(path, inodes));
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == USE_STAMP_EXTENSION)
            && !path.with_extension("").exists()
        {
            let files = inodes.measure(&path, &HashSet::new());
            orphans.push((path, files));
        }
    }
    (units, orphans)
}

/// When a unit was last used: the newest of its last-use stamp, which a cache
/// hit refreshes, and its newest rustc session, or the directory's own mtime
/// when it has no session yet.
fn last_used(directory: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(directory).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    let stamp = (!is_seeding(directory))
        .then(|| std::fs::metadata(use_stamp_path(directory)).ok())
        .flatten()
        .and_then(|stamp| stamp.modified().ok());
    let used = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| std::fs::read_dir(entry.path()).ok())
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("s-"))
        .filter_map(|entry| {
            entry
                .metadata()
                .ok()
                .filter(|metadata| metadata.is_dir())?
                .modified()
                .ok()
        })
        .max()
        .or_else(|| metadata.modified().ok())
        .max(stamp)
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |age| age.as_secs());
    Some(used)
}

/// A finished rustc session, `<unit>/<crate>/s-<time>-<random>-<hash>`.
struct Session {
    directory: PathBuf,
    lock: PathBuf,
    files: Vec<FileKey>,
}

/// A unit's finished sessions and their lock files, oldest first. Sessions
/// are only ever directly inside a crate's directory.
fn finished_sessions(unit: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut sessions = Vec::new();
    for krate in nested_entries(unit) {
        if !krate.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        for session in nested_entries(&krate.path()) {
            let name = session.file_name();
            let Some(lock) = session_lock_name(&name) else {
                continue;
            };
            if !name.to_string_lossy().starts_with("s-") || is_unfinished_session(&name) {
                continue;
            }
            let Some(modified) = session
                .metadata()
                .ok()
                .filter(|metadata| metadata.is_dir())
                .and_then(|metadata| metadata.modified().ok())
            else {
                continue;
            };
            sessions.push((modified, session.path(), krate.path().join(lock)));
        }
    }
    sessions.sort();
    sessions
        .into_iter()
        .map(|(_, session, lock)| (session, lock))
        .collect()
}

fn measure_unit(directory: PathBuf, inodes: &mut Inodes) -> Option<Unit> {
    let last_used = last_used(&directory)?;
    // A staging name has a dot before seeding; with_extension would otherwise
    // point at the destination unit's churn record and stamp.
    let records = if is_seeding(&directory) {
        Vec::new()
    } else {
        vec![directory.with_extension("json"), use_stamp_path(&directory)]
    };
    // Each finished session is an owner of its own, so removing one frees
    // only what no other session, working session, or unit still holds.
    let sessions = finished_sessions(&directory)
        .into_iter()
        .map(|(session, lock)| Session {
            files: inodes.measure(&session, &HashSet::new()),
            directory: session,
            lock,
        })
        .collect::<Vec<_>>();
    let excluded = sessions
        .iter()
        .map(|session| session.directory.clone())
        .collect::<HashSet<_>>();
    let mut files = inodes.measure(&directory, &excluded);
    for session in &sessions {
        files.extend(session.files.iter().cloned());
    }
    let mut record_files = Vec::new();
    for record in &records {
        record_files.extend(inodes.measure(record, &HashSet::new()));
    }
    Some(Unit {
        files,
        sessions,
        directory,
        last_used,
        records,
        record_files,
    })
}

/// Whether a unit's state exceeds the backstop: more than half the budget.
/// The compile path then stops using it, and collection stops keeping it for
/// recent use.
///
/// This runs on every learned compile, so it sums sizes first, one directory
/// listing per directory. The sum counts a linked file once per link and so
/// never undercounts; only a sum over the limit is measured again, link-aware,
/// which on Windows opens every file.
pub(crate) fn exceeds_backstop(unit: &Path, budget: Option<u64>) -> bool {
    budget.is_some_and(|budget| {
        let limit = budget / 2;
        listed_bytes(unit) > limit && tree_bytes(unit) > limit
    })
}

/// The sizes of every file under `root`, each link counted.
fn listed_bytes(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(listing) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in listing.flatten() {
            // Not followed: a symlink is neither descended into nor counted.
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

fn use_stamp_path(unit: &Path) -> PathBuf {
    unit.with_extension(USE_STAMP_EXTENSION)
}

fn rejection(budget: u64) -> Vec<u8> {
    format!("r {budget}").into_bytes()
}

/// Set a stamp's content and mtime, creating it if needed.
fn write_stamp(stamp: &Path, content: &[u8], modified: SystemTime) {
    let written = std::fs::File::create(stamp).and_then(|mut file| {
        std::io::Write::write_all(&mut file, content)?;
        file.set_modified(modified)
    });
    if let Err(error) = written {
        log::debug!(
            "could not update the incremental use stamp {}: {error}",
            stamp.display()
        );
    }
}

/// Record a cache hit as a use of `unit`, a unit directory of this checkout,
/// so collection does not mistake state restored on every build for idle
/// state. Only a unit with state here gets a stamp. At most one write a
/// minute, and none while the compile path's backstop rejects the unit's
/// state under the current `budget`: that state is waiting to be collected.
pub(crate) fn stamp_use(unit: &Path, budget: Option<u64>) {
    if !unit.is_dir() {
        return;
    }
    let stamp = use_stamp_path(unit);
    if let Ok(metadata) = std::fs::metadata(&stamp) {
        let recent = metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age.as_secs() < USE_STAMP_THROTTLE_SECS);
        if recent
            || budget.is_some_and(|budget| {
                metadata.len() > 0
                    && std::fs::read(&stamp).is_ok_and(|held| held == rejection(budget))
            })
        {
            return;
        }
    }
    write_stamp(&stamp, b"", SystemTime::now());
}

/// The compile path's backstop rejected `unit`'s state under `budget`. Hits
/// stop refreshing it until a compile accepts it or the budget changes, so
/// it ages like unused state. Its last use stays as it was.
pub(crate) fn mark_rejected(unit: &Path, budget: u64) {
    let stamp = use_stamp_path(unit);
    let content = rejection(budget);
    if std::fs::read(&stamp).is_ok_and(|held| held == content) {
        return;
    }
    let modified = std::fs::metadata(&stamp)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    write_stamp(&stamp, &content, modified);
}

/// A compile accepted `unit`'s state: clear any earlier rejection.
pub(crate) fn clear_rejection(unit: &Path) {
    let stamp = use_stamp_path(unit);
    if std::fs::metadata(&stamp).is_ok_and(|metadata| metadata.len() > 0) {
        write_stamp(&stamp, b"", SystemTime::now());
    }
}

/// Hold all existing rustc session locks through deletion; a held lock means
/// the unit is in use. Never wait for a compiler while collecting.
fn unit_locks(directory: &Path, dry_run: bool) -> Result<Option<Vec<fslock::LockFile>>> {
    let now = now_secs();
    // seed() builds <unit>.seeding-<pid> before its atomic rename. Protect a
    // newly started copy even before it has created its first session.
    if is_seeding(directory)
        && std::fs::metadata(directory)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|mtime| {
                now.saturating_sub(
                    mtime
                        .duration_since(UNIX_EPOCH)
                        .map_or(now, |time| time.as_secs()),
                ) <= SESSION_START_GRACE_SECS
            })
    {
        return Ok(None);
    }
    let mut paths = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let listing = match std::fs::read_dir(&directory) {
            Ok(listing) => listing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warn_walk_error(&directory, &error);
                return Ok(None);
            }
        };
        for entry in listing {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    warn_walk_error(&directory, &error);
                    return Ok(None);
                }
            };
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    warn_walk_error(&entry.path(), &error);
                    return Ok(None);
                }
            };
            if kind.is_dir() {
                pending.push(entry.path());
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let session = kind.is_dir() && name.starts_with("s-");
            let lock = kind.is_file() && name.starts_with("s-") && name.ends_with(".lock");
            if session || lock {
                let modified = match entry.metadata().and_then(|metadata| metadata.modified()) {
                    Ok(modified) => modified,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        warn_walk_error(&entry.path(), &error);
                        return Ok(None);
                    }
                }
                .duration_since(UNIX_EPOCH)
                .map_or(now, |time| time.as_secs());
                // rustc creates its lock file before flocking it. Do not win
                // that race against a starting compiler; a unit used within
                // the last minute is not an eviction candidate anyway.
                if now.saturating_sub(modified) <= SESSION_START_GRACE_SECS {
                    return Ok(None);
                }
            }
            if lock {
                paths.push(entry.path());
            }
        }
    }
    let mut locks = Vec::new();
    // A preview must never take a compiler's lock, even momentarily.
    if !dry_run {
        for path in paths {
            let mut lock = match fslock::LockFile::open(&path) {
                Ok(lock) => lock,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    warn_walk_error(&path, &error);
                    return Ok(None);
                }
            };
            match lock.try_lock() {
                Ok(true) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                result => {
                    if let Err(error) = result {
                        warn_walk_error(&path, &error);
                    }
                    return Ok(None);
                }
            }
            locks.push(lock);
        }
    }
    Ok(Some(locks))
}

/// Whether a finished session and its lock file are past the grace a
/// starting compiler gets between creating its lock file and locking it.
/// Takes no lock: a preview decides with this alone.
fn session_is_settled(session: &Session, now: u64) -> bool {
    [&session.directory, &session.lock].into_iter().all(|path| {
        std::fs::symlink_metadata(path)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                let modified = modified
                    .duration_since(UNIX_EPOCH)
                    .map_or(now, |time| time.as_secs());
                now.saturating_sub(modified) > SESSION_START_GRACE_SECS
            })
    })
}

/// Remove one finished session of a live checkout the way rustc's own
/// collector does: only under an exclusive lock on its lock file, taken
/// without waiting. A compiler starting from the session, or a seed copying
/// it, holds that lock shared, and the session is kept.
///
/// Under the lock, the session is first renamed to the unfinished name rustc
/// gives a session it is writing, so if removal stops halfway no compiler or
/// seed ever starts from what is left; rustc collects such a directory once
/// its lock is free. The lock file goes once the session has.
///
/// Returns `None` when the session was kept, and otherwise whether it is
/// gone entirely. Only files whose last link went are credited.
fn evict_session(session: &Session, inodes: &mut Inodes) -> Option<bool> {
    // fslock creates a missing file. A missing lock file means rustc is
    // removing the session, or already has.
    if !std::fs::symlink_metadata(&session.lock).is_ok_and(|metadata| metadata.is_file()) {
        return None;
    }
    let mut lock = fslock::LockFile::open(&session.lock).ok()?;
    if !lock.try_lock().ok()? {
        return None;
    }
    if !std::fs::symlink_metadata(&session.directory).is_ok_and(|metadata| metadata.is_dir()) {
        return None;
    }
    let mut unfinished = session.lock.with_extension("").into_os_string();
    unfinished.push("-working");
    let unfinished = PathBuf::from(unfinished);
    let mut renamed = std::fs::rename(&session.directory, &unfinished);
    // An earlier eviction that stopped halfway may have left the name taken.
    // The lock held here guards that directory too.
    if renamed.is_err()
        && std::fs::symlink_metadata(&unfinished).is_ok_and(|metadata| metadata.is_dir())
        && remove_tree(&unfinished, false, inodes)
    {
        renamed = std::fs::rename(&session.directory, &unfinished);
    }
    if let Err(error) = renamed {
        warn_walk_error(&session.directory, &error);
        return None;
    }
    let removed = remove_tree(&unfinished, false, inodes);
    if removed {
        // Removed while still held, as rustc does; rustc also removes a
        // stray lock file whose session is gone.
        let _ = std::fs::remove_file(&session.lock);
    }
    drop(lock);
    Some(removed)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    Missing,
    Removed(u64),
    Active,
}

/// Remove learned incremental state for exactly one workspace.
pub(crate) fn remove_workspace(root: &Path, workspace_root: &Path) -> Result<RemoveOutcome> {
    remove_workspace_with(root, workspace_root, || {})
}

fn remove_workspace_with(
    root: &Path,
    workspace_root: &Path,
    on_retired: impl FnOnce(),
) -> Result<RemoveOutcome> {
    remove_workspace_with_unlink(root, workspace_root, on_retired, |tree| {
        unlink_retired_checkout_record(tree, None)
    })
}

fn remove_workspace_with_unlink(
    root: &Path,
    workspace_root: &Path,
    on_retired: impl FnOnce(),
    unlink: impl FnOnce(&RetiredTree) -> std::io::Result<u64>,
) -> Result<RemoveOutcome> {
    let key = checkout_key(workspace_root);
    let directory = root.join(&key);
    let Some(record) = read_record(&directory.join(RECORD_FILE)) else {
        return Ok(RemoveOutcome::Missing);
    };
    if record.workspace_root != workspace_root {
        return Ok(RemoveOutcome::Missing);
    }
    let guard = deletion_guard(root, &key, true)?;
    if guard.active {
        return Ok(RemoveOutcome::Active);
    }
    let tree = RetiredTree::retire(&directory)?;
    let record_bytes = match tree.as_ref().map(unlink).transpose() {
        Ok(bytes) => bytes.unwrap_or(0),
        Err(error) => {
            restore_retired_checkout(tree.as_ref().unwrap(), &directory, &error)?;
            return Err(error.into());
        }
    };
    guard.cleanup();
    drop(guard);
    on_retired();
    let bytes = tree
        .as_ref()
        .map_or(0, RetiredTree::bytes)
        .saturating_add(record_bytes);
    if let Some(tree) = tree {
        tree.remove()?;
    }
    Ok(RemoveOutcome::Removed(bytes))
}

/// Restore under the registrar before a failed unlink can expose a donor.
fn restore_retired_checkout(
    tree: &RetiredTree,
    live: &Path,
    unlink_error: &std::io::Error,
) -> Result<()> {
    std::fs::rename(tree.path(), live).wrap_err_with(|| {
        format!(
            "failed to restore incremental checkout {} from {} after record unlink failed: {unlink_error}",
            live.display(),
            tree.path().display()
        )
    })
}

/// Remove the record under the registrar so older binaries cannot discover
/// the retired checkout as a seed donor, even though they scan its name.
fn unlink_retired_checkout_record(
    tree: &RetiredTree,
    inodes: Option<&mut Inodes>,
) -> std::io::Result<u64> {
    let path = tree.path().join(RECORD_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let (key, links) = file_identity(&path, &metadata);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }
    if let Some(inodes) = inodes
        && links == Some(1)
    {
        inodes.forget(&key);
    }
    Ok(metadata.len())
}

struct Entry {
    key: String,
    directory: PathBuf,
    units: Vec<Unit>,
    orphan_stamps: Vec<(PathBuf, Vec<FileKey>)>,
    files: Vec<FileKey>,
    workspace_root: Option<PathBuf>,
}

struct Snapshot {
    entries: Vec<Entry>,
    inodes: Inodes,
}

fn entries(root: &Path, include_retired: bool) -> Result<Snapshot> {
    let mut snapshot = Snapshot {
        entries: Vec::new(),
        inodes: Inodes::default(),
    };
    let listing = match std::fs::read_dir(root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(snapshot),
        Err(error) => return Err(error.into()),
    };
    for entry in listing {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn_walk_error(root, &error);
                continue;
            }
        };
        if !entry.file_type().is_ok_and(|kind| kind.is_dir())
            || entry.file_name() == LOCKS_DIR
            || retired::is_retired_name(&entry.file_name())
        {
            continue;
        }
        let directory = entry.path();
        let record = read_record(&directory.join(RECORD_FILE));
        let (units, orphan_stamps) = units(&directory, &mut snapshot.inodes);
        let mut excluded = units
            .iter()
            .map(|unit| unit.directory.clone())
            .collect::<HashSet<_>>();
        excluded.extend(units.iter().flat_map(|unit| unit.records.iter().cloned()));
        excluded.extend(orphan_stamps.iter().map(|(stamp, _)| stamp.clone()));
        excluded.extend(
            nested_entries(&directory.join("blake3"))
                .into_iter()
                .filter(|entry| retired::is_retired_name(&entry.file_name()))
                .map(|entry| entry.path()),
        );
        let files = snapshot.inodes.measure(&directory, &excluded);
        snapshot.entries.push(Entry {
            key: entry.file_name().to_string_lossy().into_owned(),
            workspace_root: record.map(|record| record.workspace_root),
            files,
            units,
            orphan_stamps,
            directory,
        });
    }
    if include_retired {
        // Retired state occupies space without being a usable checkout or
        // unit. Use the live ledger to count cross-boundary hard links once.
        for tree in retired_trees(root) {
            snapshot.inodes.measure(&tree, &HashSet::new());
        }
    }
    Ok(snapshot)
}

fn is_seeding(directory: &Path) -> bool {
    directory
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains(".seeding-"))
}

fn checkout_key(workspace_root: &Path) -> String {
    CacheDigest::blake3(workspace_root.as_os_str().as_encoded_bytes()).hash[..16].to_string()
}

fn lock_dir(root: &Path, key: &str) -> PathBuf {
    root.join(LOCKS_DIR).join(key)
}

fn registrar_path(root: &Path) -> PathBuf {
    root.join(LOCKS_DIR).join(REGISTRAR_FILE)
}

fn acquire_lease(root: &Path, key: &str) -> Result<ActiveLease> {
    let locks = lock_dir(root, key);
    let registrar_path = registrar_path(root);
    std::fs::create_dir_all(registrar_path.parent().expect("registrar has a parent"))?;
    let mut registrar = fslock::LockFile::open(&registrar_path)?;
    registrar.lock()?;
    std::fs::create_dir_all(&locks)?;
    // PIDs repeat across namespaces. Never wait on another session's lease
    // while holding the registrar its cleanup needs.
    let path = locks.join(format!(
        "{}-{}.lease",
        std::process::id(),
        crate::util::random_string(12)
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut lock = fslock::LockFile::open(&path)?;
    if !lock.try_lock()? {
        eyre::bail!("the incremental lease {} is already held", path.display());
    }
    drop(registrar);
    Ok(ActiveLease {
        lock: Some(lock),
        path,
        registrar_path,
    })
}

/// Lease a live checkout the collector trims, once, for the rest of the sweep.
///
/// The build's lease protects the checkout only while the build runs. Ours
/// keeps `mbx clean` or `mbx cache remove` from removing the checkout, and a
/// new build from reseeding it under the same session names, between or
/// during the evictions of its units. Leases are per process, so builds are
/// not held up; later units of the checkout count as live for this sweep.
fn lease_for_trimming(
    leases: &mut HashMap<String, ActiveLease>,
    root: &Path,
    key: &str,
) -> Result<()> {
    if !leases.contains_key(key) {
        leases.insert(key.to_string(), acquire_lease(root, key)?);
    }
    Ok(())
}

struct DeletionGuard {
    _registrar: Option<fslock::LockFile>,
    locks: PathBuf,
    active: bool,
    cleanup: bool,
}

impl DeletionGuard {
    fn cleanup(&self) {
        if self.cleanup {
            cleanup_lock_dir(&self.locks);
        }
    }
}

fn deletion_guard(root: &Path, key: &str, cleanup: bool) -> Result<DeletionGuard> {
    let locks = lock_dir(root, key);
    let registrar_path = registrar_path(root);
    if !cleanup && !registrar_path.exists() {
        // Lease registration always creates the registrar first.
        return Ok(DeletionGuard {
            _registrar: None,
            locks,
            active: false,
            cleanup,
        });
    }
    // These are mbx's own lease files, not rustc session locks: the rule
    // against preview lock acquisition does not apply here. The registrar
    // makes probing safe against a lease being created but not yet locked.
    std::fs::create_dir_all(registrar_path.parent().expect("registrar has a parent"))?;
    let mut registrar = fslock::LockFile::open(&registrar_path)?;
    registrar.lock()?;
    let active = probe_leases(&locks, cleanup)?;
    Ok(DeletionGuard {
        _registrar: cleanup.then_some(registrar),
        locks,
        active,
        cleanup,
    })
}

/// The caller holds the registrar so no lease can be registered mid-probe.
fn probe_leases(locks: &Path, cleanup: bool) -> Result<bool> {
    let mut active = false;
    let listing = match std::fs::read_dir(locks) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    for entry in listing {
        let entry = entry?;
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("lease") {
            continue;
        }
        let mut lease = fslock::LockFile::open(&entry.path())?;
        if lease.try_lock()? {
            drop(lease);
            if cleanup {
                let _ = std::fs::remove_file(entry.path());
            }
        } else {
            active = true;
        }
    }
    Ok(active)
}

fn cleanup_lock_dir(locks: &Path) {
    // Older development builds placed the registrar in each checkout's lock
    // directory. It is safe to remove while the global registrar is held.
    let _ = std::fs::remove_file(locks.join(REGISTRAR_FILE));
    let _ = std::fs::remove_dir(locks);
}

/// Copy the other checkout's state for one unit used most recently into
/// `checkout`, under the same unit name. Returns that checkout's workspace
/// root, or `None` when no other checkout has state for the unit.
///
/// Only worth doing for a compilation whose state does not depend on where
/// the checkout lives (`-Zremap-cwd-prefix`): rustc ignores state recorded
/// under another working directory. The unit's name is the digest of its
/// path-normalized invocation, so it is the same in every checkout of the
/// project, and rustc validates whatever it inherits against the sources it
/// compiles, as it does after any edit.
///
/// The donor holds a lease while it is read, so collection cannot remove it
/// halfway, and each session it copies is locked the way rustc locks it, so
/// the donor's own compiler cannot remove that either. A session the donor's
/// compiler is still writing, or may be removing, is left out. The copy is
/// made beside its final name and renamed into place, so an interrupted copy
/// leaves no partial state; a donor that cannot be copied gives way to the
/// next.
///
/// Files are independent copies, using a filesystem clone where supported.
/// rustc can rewrite inherited files in place (including ThinLTO bitcode),
/// so hard links would let a later compilation change the donor's session.
pub(crate) fn seed(checkout: &Path, unit: &str) -> Result<Option<PathBuf>> {
    let (Some(root), Some(own)) = (checkout.parent(), checkout.file_name()) else {
        return Ok(None);
    };
    if !copy_state_ready(root) {
        return Ok(None);
    }
    let mut donors = Vec::new();
    for entry in std::fs::read_dir(root)?.flatten() {
        let name = entry.file_name();
        if name == own
            || name == LOCKS_DIR
            || retired::is_retired_name(&name)
            || !entry.path().join(unit).is_dir()
        {
            continue;
        }
        if let Some(record) = read_record(&entry.path().join(RECORD_FILE)) {
            let used = last_used(&entry.path().join(unit)).unwrap_or_default();
            donors.push((used, record.updated_secs, name, record.workspace_root));
        }
    }
    // The donor whose state for this unit was used last is likeliest to be
    // close to this checkout's sources. Ties, as with coarse timestamps, go
    // to the checkout built last, then by name, so the choice is stable.
    donors.sort_by(|a, b| (b.0, b.1, &a.2).cmp(&(a.0, a.1, &b.2)));
    let destination = checkout.join(unit);
    let mut failure = None;
    for (_, _, name, workspace_root) in donors {
        let Some(key) = name.to_str() else {
            continue;
        };
        let _lease = acquire_lease(root, key)?;
        let source = root.join(key).join(unit);
        if !holds_finished_session(&source) {
            continue;
        }
        let mut staging = destination.clone().into_os_string();
        staging.push(format!(".seeding-{}", std::process::id()));
        let staging = PathBuf::from(staging);
        let mut held = Vec::new();
        let copied = destination
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| copy_state(&source, &staging, &mut held));
        // Every session the donor's compiler could have been removing was
        // skipped, so there may be nothing left to start from.
        let copied = copied.and_then(|()| {
            if holds_finished_session(&staging) {
                std::fs::rename(&staging, &destination).map(|()| true)
            } else {
                Ok(false)
            }
        });
        drop(held);
        match copied {
            Ok(true) => return Ok(Some(workspace_root)),
            Ok(false) => {
                let _ = std::fs::remove_dir_all(&staging);
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                failure = Some(eyre::Report::from(error).wrap_err(format!(
                    "failed to copy incremental state from {}",
                    workspace_root.display()
                )));
            }
        }
    }
    failure.map_or(Ok(None), Err)
}

/// rustc names a session it is still writing `s-<time>-<random>-working`,
/// and renames it once the compilation finishes.
fn is_unfinished_session(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with("s-") && name.ends_with("-working"))
}

/// Whether a unit's directory holds a session rustc finished:
/// `<crate>-<hash>/s-...`.
fn holds_finished_session(unit: &Path) -> bool {
    let Ok(crates) = std::fs::read_dir(unit) else {
        return false;
    };
    crates.flatten().any(|krate| {
        std::fs::read_dir(krate.path()).is_ok_and(|sessions| {
            sessions.flatten().any(|session| {
                let name = session.file_name();
                session.path().is_dir()
                    && name.to_str().is_some_and(|name| name.starts_with("s-"))
                    && !is_unfinished_session(&name)
            })
        })
    })
}

/// The name of the lock file rustc keeps beside a session: the session's name
/// up to its third `-`, `s-<time>-<random>.lock`, the same before and after
/// rustc finishes the session.
fn session_lock_name(session: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let name = session.to_str()?;
    let (end, _) = name.match_indices('-').nth(2)?;
    Some(format!("{}.lock", &name[..end]).into())
}

/// Hold a shared lock on a finished session's lock file, as a reader.
///
/// rustc removes a superseded session only after it takes that lock
/// exclusively, so a session locked here cannot disappear while it is being
/// copied. A session whose lock is taken, or that has no lock file, may be
/// going away and is left out. rustc locks through the same primitive as
/// `File::try_lock_shared`.
fn lock_session(lock: &Path) -> Option<std::fs::File> {
    let lock = std::fs::File::open(lock).ok()?;
    lock.try_lock_shared().ok()?;
    Some(lock)
}

/// Copy one unit's state: `<unit>/<crate>-<hash>/<session>`. Sessions are
/// only ever directly inside a crate's directory, which is the only level
/// where they are treated as such: a crate may be named `s`.
fn copy_state(
    unit: &Path,
    destination: &Path,
    held: &mut Vec<std::fs::File>,
) -> std::io::Result<()> {
    std::fs::create_dir(destination)?;
    for entry in std::fs::read_dir(unit)? {
        let entry = entry?;
        let to = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_crate_state(&entry.path(), &to, held)?;
        } else {
            copy_file(&entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Copy a crate's finished sessions, holding each one's lock, and the lock
/// files of the sessions copied.
fn copy_crate_state(
    krate: &Path,
    destination: &Path,
    held: &mut Vec<std::fs::File>,
) -> std::io::Result<()> {
    std::fs::create_dir(destination)?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(krate)? {
        let entry = entry?;
        entries.push((entry.file_name(), entry.path(), entry.file_type()?));
    }
    // Lock every finished session first, so the lock files of the sessions
    // left out are left out too.
    let mut skipped = HashSet::new();
    for (name, path, kind) in &entries {
        if !kind.is_dir() {
            continue;
        }
        let lock_name = session_lock_name(name);
        let lock = lock_name
            .as_ref()
            .filter(|_| !is_unfinished_session(name))
            .and_then(|lock_name| lock_session(&path.with_file_name(lock_name)));
        match lock {
            Some(lock) => held.push(lock),
            None => {
                skipped.extend(lock_name);
                skipped.insert(name.clone());
            }
        }
    }
    for (name, from, kind) in &entries {
        if skipped.contains(name) {
            continue;
        }
        let to = destination.join(name);
        if kind.is_dir() {
            copy_tree(from, &to)?;
        } else if kind.is_file() {
            copy_file(from, &to)?;
        }
    }
    Ok(())
}

/// Copy a finished session's files as they are.
fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let to = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if kind.is_file() {
            copy_file(&entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Copy a state file without sharing its inode with the donor. The enclosing
/// staging directory keeps partial copies out of rustc's session search.
fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let modified = std::fs::metadata(from)?.modified()?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    reflink_copy::reflink_or_copy(from, to)?;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    std::fs::copy(from, to)?;
    // rustc selects sessions by the timestamp in their name, but keep the
    // original mtime too, independent of the platform's copy implementation.
    crate::materialize::set_modified(to, modified)
        .map_err(|error| std::io::Error::other(error.to_string()))
}

fn read_record(path: &Path) -> Option<CheckoutRecord> {
    let bytes = std::fs::read(path).ok()?;
    let record = serde_json::from_slice::<CheckoutRecord>(&bytes).ok()?;
    (record.version == RECORD_VERSION).then_some(record)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

// A sweep owns one reference per unit (and per bookkeeping tree), even if
// multiple session paths in that owner hard-link the same file.
#[derive(Clone, PartialEq, Eq, Hash)]
enum FileKey {
    Object(u64, u64),
    #[cfg(not(unix))]
    Path(PathBuf),
}

fn file_key(path: &Path, metadata: &std::fs::Metadata) -> FileKey {
    file_identity(path, metadata).0
}

/// A file's key and, where the platform tells, its number of links.
#[cfg(unix)]
fn file_identity(_path: &Path, metadata: &std::fs::Metadata) -> (FileKey, Option<u64>) {
    use std::os::unix::fs::MetadataExt as _;
    (
        FileKey::Object(metadata.dev(), metadata.ino()),
        Some(metadata.nlink()),
    )
}

#[cfg(windows)]
fn file_identity(path: &Path, _metadata: &std::fs::Metadata) -> (FileKey, Option<u64>) {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    if let Ok(file) = std::fs::File::open(path) {
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // The handle stays open and the output points to a correctly sized buffer.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } != 0 {
            let info = unsafe { info.assume_init() };
            return (
                FileKey::Object(
                    u64::from(info.dwVolumeSerialNumber),
                    (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
                ),
                Some(u64::from(info.nNumberOfLinks)),
            );
        }
    }
    // An unknown count credits nothing.
    (FileKey::Path(path.to_path_buf()), None)
}

#[cfg(not(any(unix, windows)))]
fn file_identity(path: &Path, _metadata: &std::fs::Metadata) -> (FileKey, Option<u64>) {
    (FileKey::Path(path.to_path_buf()), None)
}

#[derive(Default)]
struct Inodes {
    files: HashMap<FileKey, (u64, usize)>,
    bytes: u64,
}

impl Inodes {
    /// Walk each subtree once, deduplicating both inside an owner and across
    /// owners. Bookkeeping participates in the same whole-sweep accounting.
    fn measure(&mut self, root: &Path, excluded: &HashSet<PathBuf>) -> Vec<FileKey> {
        let mut files = HashSet::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            if excluded.contains(&path) {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                pending.extend(nested_entries(&path).into_iter().map(|entry| entry.path()));
            } else if metadata.is_file() {
                let key = file_key(&path, &metadata);
                self.files.entry(key.clone()).or_insert((metadata.len(), 0));
                files.insert(key);
            }
        }
        for key in &files {
            let (bytes, references) = self.files.get_mut(key).expect("measured inode");
            if *references == 0 {
                self.bytes = self.bytes.saturating_add(*bytes);
            }
            *references += 1;
        }
        files.into_iter().collect()
    }

    fn release(&mut self, files: &[FileKey]) {
        for key in files {
            let (bytes, references) = self.files.get_mut(key).expect("measured inode");
            // Already forgotten: its last link went with an evicted session.
            if *references == 0 {
                continue;
            }
            *references -= 1;
            if *references == 0 {
                self.bytes = self.bytes.saturating_sub(*bytes);
            }
        }
    }

    /// A file's last link was removed: whatever owners remain, the data is
    /// gone. A file this sweep never measured frees nothing it counted.
    fn forget(&mut self, key: &FileKey) {
        if let Some((bytes, references)) = self.files.get_mut(key)
            && *references > 0
        {
            *references = 0;
            self.bytes = self.bytes.saturating_sub(*bytes);
        }
    }
}

/// The size of learned state, each distinct file once however many links it
/// has: what the compiler's backstop and the collector both weigh.
pub(crate) fn tree_bytes(root: &Path) -> u64 {
    let mut inodes = Inodes::default();
    inodes.measure(root, &HashSet::new());
    inodes.bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_from_another_pid_namespace_does_not_block_a_session() {
        // Isolate the old process-local counter, so the held name is exactly
        // the one the pre-fix implementation would try first.
        const CHILD_ENV: &str = "MBX_TEST_INCREMENTAL_LEASE_COLLISION";
        if std::env::var_os(CHILD_ENV).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "incremental::tests::a_lease_from_another_pid_namespace_does_not_block_a_session",
                    "--exact",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .contains("test result: ok. 1 passed"),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            return;
        }

        let cache = tempfile::tempdir().unwrap();
        let key = "checkout";
        let locks = lock_dir(cache.path(), key);
        std::fs::create_dir_all(&locks).unwrap();
        let occupied = locks.join(format!("{}-0.lease", std::process::id()));
        std::fs::write(&occupied, b"another namespace's lease").unwrap();
        let mut other = fslock::LockFile::open(&occupied).unwrap();
        assert!(other.try_lock().unwrap());

        let root = cache.path().to_path_buf();
        let (sent, received) = std::sync::mpsc::channel();
        let acquiring = std::thread::spawn(move || {
            let _ = sent.send(acquire_lease(&root, key));
        });
        let own = received
            .recv_timeout(Duration::from_secs(5))
            .expect("lease acquisition must not wait for another namespace")
            .unwrap();
        acquiring.join().unwrap();
        assert_ne!(own.path, occupied);
        assert!(deletion_guard(cache.path(), key, false).unwrap().active);
        let own_path = own.path.clone();
        drop(own);
        assert!(!own_path.exists(), "normal lease drop removes its file");
        assert_eq!(
            std::fs::read(&occupied).unwrap(),
            b"another namespace's lease"
        );
        assert!(deletion_guard(cache.path(), key, false).unwrap().active);
        drop(other);
        let stale_contents = std::fs::read(&occupied).unwrap();
        assert!(!deletion_guard(cache.path(), key, false).unwrap().active);
        assert_eq!(
            std::fs::read(&occupied).unwrap(),
            stale_contents,
            "a preview must retain the stale lease file"
        );
        assert!(!deletion_guard(cache.path(), key, true).unwrap().active);
        assert!(!occupied.exists());
    }

    #[test]
    fn abandoned_checkout_state_is_kept_until_it_expires() {
        let cache = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let checkout_parent = parent.path().join("manager");
        std::fs::create_dir(&checkout_parent).unwrap();
        let checkout = checkout_parent.join("checkout");
        let key = checkout_key(&checkout);
        std::fs::create_dir(&checkout).unwrap();
        let state = touch(cache.path(), &checkout).unwrap();
        let directory = state.directory.clone();
        unit(&directory, "unit", 0);
        assert!(read_record(&directory.join(RECORD_FILE)).is_some());
        drop(state);

        std::fs::remove_dir(&checkout).unwrap();
        std::fs::remove_dir(&checkout_parent).unwrap();
        assert!(!crate::store::checkout_is_live_on(cache.path(), &checkout));
        let kept = collect(cache.path(), None, None, false, true, None).unwrap();
        assert_eq!(
            kept.removed_directories, 0,
            "a new checkout may seed from it"
        );
        assert!(directory.exists());

        // Expiry is measured in whole seconds.
        std::thread::sleep(Duration::from_millis(1100));
        let outcome = collect(cache.path(), None, Some(Duration::ZERO), false, true, None).unwrap();
        assert_eq!(outcome.removed_directories, 1);
        assert!(outcome.removed_bytes > 0);
        assert!(!directory.exists());
        assert!(!lock_dir(cache.path(), &key).exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn checkout_on_another_filesystem_is_kept() {
        let Some(other_fs) = Path::new("/dev/shm")
            .exists()
            .then_some(Path::new("/dev/shm"))
        else {
            return;
        };
        let cache = tempfile::tempdir().unwrap();
        if crate::store::same_filesystem_paths(cache.path(), other_fs) {
            return;
        }
        // An unwritable mount is not a test failure, as in
        // real_cross_device_probe_warns.
        let Ok(parent) = tempfile::Builder::new()
            .prefix("mbx-incremental-")
            .tempdir_in(other_fs)
        else {
            return;
        };
        let checkout_parent = parent.path().join("manager");
        std::fs::create_dir(&checkout_parent).unwrap();
        let checkout = checkout_parent.join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let state = touch(cache.path(), &checkout).unwrap();
        let directory = state.directory.clone();
        drop(state);

        std::fs::remove_dir(&checkout).unwrap();
        std::fs::remove_dir(&checkout_parent).unwrap();
        let outcome = collect(cache.path(), None, None, false, true, None).unwrap();

        assert_eq!(outcome.removed_directories, 0);
        assert!(directory.exists());
    }

    #[test]
    fn dry_run_does_not_create_checkout_lock_bookkeeping() {
        let cache = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let checkout = parent.path().join("checkout");
        let key = checkout_key(&checkout);
        std::fs::create_dir(&checkout).unwrap();
        let state = touch(cache.path(), &checkout).unwrap();
        drop(state);
        assert!(!lock_dir(cache.path(), &key).exists());

        std::fs::remove_dir(&checkout).unwrap();
        // Expiry is measured in whole seconds.
        std::thread::sleep(Duration::from_millis(1100));
        let outcome = collect(cache.path(), None, Some(Duration::ZERO), true, true, None).unwrap();

        assert_eq!(outcome.removed_directories, 1);
        assert!(!lock_dir(cache.path(), &key).exists());
    }

    #[test]
    fn active_checkout_state_is_not_pruned_or_removed() {
        let cache = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let checkout = parent.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let state = touch(cache.path(), &checkout).unwrap();
        std::fs::remove_dir(&checkout).unwrap();
        // Expiry is measured in whole seconds.
        std::thread::sleep(Duration::from_millis(1100));

        let outcome = collect(
            cache.path(),
            Some(0),
            Some(Duration::ZERO),
            false,
            true,
            Some(0),
        )
        .unwrap();
        assert_eq!(outcome.removed_directories, 0);
        assert_eq!(outcome.skipped_active_directories, 1);
        assert_eq!(
            remove_workspace(cache.path(), &checkout).unwrap(),
            RemoveOutcome::Active
        );
        assert!(state.directory.exists());
    }

    fn unit(checkout: &Path, name: &str, used: u64) -> PathBuf {
        let unit = checkout.join("blake3").join(name);
        let session = unit.join("crate-id/s-session");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(session.join("dep-graph.bin"), vec![0; 32]).unwrap();
        filetime::set_file_mtime(&session, filetime::FileTime::from_unix_time(used as i64, 0))
            .unwrap();
        unit
    }

    #[test]
    fn unit_lru_ignores_checkout_recency_and_keeps_empty_live_checkouts() {
        let cache = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_state = touch(cache.path(), first.path()).unwrap();
        let recent = unit(&first_state.directory, "recent", now_secs());
        drop(first_state.lease);
        let second_state = touch(cache.path(), second.path()).unwrap();
        let old = unit(&second_state.directory, "old", 1);
        let older = unit(&second_state.directory, "older", 0);
        drop(second_state.lease);
        let before = stats(cache.path()).unwrap();
        let outcome = collect(
            cache.path(),
            Some(before.bytes - 64),
            None,
            false,
            true,
            Some(before.bytes - 64),
        )
        .unwrap();
        assert_eq!(outcome.removed_units, 2);
        assert_eq!(outcome.remaining_units, 1);
        assert_eq!(outcome.removed_directories, 0);
        assert_eq!(outcome.remaining_bytes, before.bytes - 64);
        assert!(recent.exists());
        assert!(!old.exists());
        assert!(!older.exists());
        assert!(second_state.directory.join(RECORD_FILE).exists());
    }

    #[test]
    fn unit_eviction_removes_its_churn_record_and_counts_each_subtree_once() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit(&state.directory, "unit", 0);
        let record = directory.with_extension("json");
        std::fs::write(&record, b"churn").unwrap();
        std::fs::create_dir(state.directory.join("private")).unwrap();
        std::fs::write(state.directory.join("private/marker"), b"private").unwrap();
        std::fs::write(state.directory.join("blake3/orphan.json"), b"orphan").unwrap();
        drop(state.lease);
        let before = stats(cache.path()).unwrap();
        assert_eq!(before.bytes, tree_bytes(&state.directory));
        let preview = collect(cache.path(), Some(0), None, true, true, Some(0)).unwrap();
        assert_eq!(preview.removed_bytes, 32 + 5);
        assert!(record.exists());
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_bytes, 32 + 5);
        assert_eq!(outcome.remaining_bytes, tree_bytes(&state.directory));
        assert_eq!(
            before.bytes - outcome.remaining_bytes,
            outcome.removed_bytes
        );
        assert!(!record.exists());
        assert!(!directory.exists());
        assert!(state.directory.join("private/marker").exists());
        assert!(state.directory.join("blake3/orphan.json").exists());
    }

    #[test]
    fn failed_record_retirement_keeps_unit_and_all_records_live() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit(&state.directory, "unit", 0);
        let churn_record = directory.with_extension("json");
        let stamp_record = use_stamp_path(&directory);
        std::fs::write(&churn_record, b"churn").unwrap();
        std::fs::write(&stamp_record, b"stamp").unwrap();
        drop(state.lease);

        let mut attempts = 0;
        let outcome = collect_with_retire_and_unlink(
            cache.path(),
            Some(0),
            None,
            false,
            false,
            None,
            CollectHooks {
                retire: |path: &Path| {
                    attempts += 1;
                    if attempts == 2 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "injected record retirement failure",
                        ));
                    }
                    RetiredTree::retire(path)
                },
                unlink: unlink_retired_checkout_record,
            },
        )
        .unwrap();

        assert_eq!(attempts, 2);
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.remaining_units, 1);
        assert!(directory.is_dir());
        assert!(churn_record.is_file());
        assert!(stamp_record.is_file());
        assert!(retired_trees(cache.path()).is_empty());
    }

    #[test]
    fn vanished_units_and_sessions_do_not_abort_collection() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let vanished = unit(&state.directory, "vanished", 0);
        let listed = std::fs::read_dir(state.directory.join("blake3"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        std::fs::remove_dir_all(&vanished).unwrap();
        assert!(measure_unit(listed.path(), &mut Inodes::default()).is_none());
        assert!(unit_locks(&vanished, false).unwrap().is_some());
        let surviving = unit(&state.directory, "surviving", 0);
        std::fs::remove_dir_all(surviving.join("crate-id/s-session")).unwrap();
        assert!(unit_locks(&surviving, false).unwrap().is_some());
        drop(state.lease);
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert!(!surviving.exists());
    }

    #[test]
    fn unit_age_ignores_a_checkout_touched_today() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let old = unit(&state.directory, "old", 0);
        let recent = unit(&state.directory, "recent", now_secs());
        drop(state.lease);
        let outcome = collect(
            cache.path(),
            None,
            Some(Duration::from_secs(60)),
            false,
            true,
            None,
        )
        .unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(state.directory.exists());
    }

    #[test]
    fn newest_session_wins_and_units_without_sessions_use_their_own_mtime() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let old = unit(&state.directory, "old", 0);
        let recent_session = old.join("crate-id/s-new");
        std::fs::create_dir_all(&recent_session).unwrap();
        let fallback = state.directory.join("blake3/fallback");
        std::fs::create_dir_all(&fallback).unwrap();
        std::fs::write(fallback.join("state"), b"no session yet").unwrap();
        drop(state.lease);
        let outcome = collect(
            cache.path(),
            None,
            Some(Duration::from_secs(60)),
            false,
            true,
            None,
        )
        .unwrap();
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.remaining_units, 2);
        assert!(fallback.exists());
        assert!(old.exists());
    }

    #[test]
    fn held_session_lock_protects_only_its_unit_even_when_over_budget() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let active = unit(&state.directory, "active", 0);
        let inactive = unit(&state.directory, "inactive", 1);
        let mut lock = fslock::LockFile::open(&active.join("crate-id/s-session.lock")).unwrap();
        lock.lock().unwrap();
        filetime::set_file_mtime(
            active.join("crate-id/s-session.lock"),
            filetime::FileTime::from_unix_time(0, 0),
        )
        .unwrap();
        drop(state.lease);
        let preview = collect(cache.path(), Some(0), None, true, true, Some(0)).unwrap();
        assert_eq!(preview.removed_units, 2);
        assert_eq!(preview.skipped_active_units, 0);
        assert_eq!(preview.skipped_active_directories, 0);
        assert!(inactive.exists());
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.skipped_active_units, 1);
        assert_eq!(outcome.skipped_active_directories, 1);
        assert_eq!(outcome.remaining_units, 1);
        assert!(outcome.remaining_bytes > 0);
        assert!(active.exists());
        assert!(!inactive.exists());
    }

    #[test]
    fn fresh_sessions_and_unflocked_locks_receive_a_grace_period() {
        for fresh_lock in [false, true] {
            let cache = tempfile::tempdir().unwrap();
            let workspace = tempfile::tempdir().unwrap();
            let state = touch(cache.path(), workspace.path()).unwrap();
            let directory = unit(
                &state.directory,
                "unit",
                if fresh_lock { 0 } else { now_secs() },
            );
            let lock = directory.join("crate-id/s-session.lock");
            std::fs::write(&lock, b"").unwrap();
            if !fresh_lock {
                filetime::set_file_mtime(&lock, filetime::FileTime::from_unix_time(0, 0)).unwrap();
            }
            drop(state.lease);
            for dry_run in [true, false] {
                let outcome = collect(cache.path(), Some(0), None, dry_run, true, Some(0)).unwrap();
                assert_eq!(outcome.skipped_active_units, 1);
                assert_eq!(outcome.removed_units, 0);
                assert!(directory.exists());
            }
        }
    }

    fn mtime(path: &Path) -> i64 {
        filetime::FileTime::from_last_modification_time(&std::fs::metadata(path).unwrap())
            .unix_seconds()
    }

    fn set_mtime(path: &Path, secs: u64) {
        filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(secs as i64, 0)).unwrap();
    }

    #[test]
    fn a_hit_stamps_a_unit_with_state_at_most_once_a_minute() {
        let checkout = tempfile::tempdir().unwrap();
        let missing = checkout.path().join("blake3/missing");
        stamp_use(&missing, None);
        assert!(!use_stamp_path(&missing).exists(), "no state, no stamp");

        let directory = unit(checkout.path(), "unit", 0);
        let stamp = use_stamp_path(&directory);
        stamp_use(&directory, None);
        assert!(mtime(&stamp) >= now_secs() as i64 - 5);
        let recent = now_secs() - 30;
        set_mtime(&stamp, recent);
        stamp_use(&directory, None);
        assert_eq!(mtime(&stamp), recent as i64, "throttled");
        set_mtime(&stamp, 1000);
        stamp_use(&directory, None);
        assert!(mtime(&stamp) >= now_secs() as i64 - 5);
    }

    #[test]
    fn a_rejected_unit_is_refreshed_by_hits_only_under_another_budget() {
        let checkout = tempfile::tempdir().unwrap();
        let directory = unit(checkout.path(), "unit", 0);
        let stamp = use_stamp_path(&directory);
        mark_rejected(&directory, 100);
        assert_eq!(std::fs::read(&stamp).unwrap(), b"r 100");
        assert_eq!(mtime(&stamp), 0, "a new rejection is not a use");
        set_mtime(&stamp, 1000);
        mark_rejected(&directory, 200);
        assert_eq!(std::fs::read(&stamp).unwrap(), b"r 200");
        assert_eq!(mtime(&stamp), 1000, "rejecting keeps the last use");

        stamp_use(&directory, Some(200));
        assert_eq!(mtime(&stamp), 1000);
        stamp_use(&directory, Some(300));
        assert!(mtime(&stamp) >= now_secs() as i64 - 5);
        assert_eq!(std::fs::read(&stamp).unwrap(), b"");

        mark_rejected(&directory, 300);
        set_mtime(&stamp, 1000);
        clear_rejection(&directory);
        assert_eq!(std::fs::read(&stamp).unwrap(), b"");
        assert!(mtime(&stamp) >= now_secs() as i64 - 5, "a compile is a use");
    }

    #[test]
    fn last_use_is_the_newest_of_stamp_session_and_fallback() {
        let checkout = tempfile::tempdir().unwrap();
        let directory = unit(checkout.path(), "unit", 100);
        assert_eq!(last_used(&directory), Some(100));
        std::fs::write(use_stamp_path(&directory), b"").unwrap();
        set_mtime(&use_stamp_path(&directory), 50);
        assert_eq!(last_used(&directory), Some(100), "an older stamp");
        set_mtime(&use_stamp_path(&directory), 200);
        assert_eq!(last_used(&directory), Some(200), "a newer stamp");

        let fallback = checkout.path().join("blake3/fallback");
        std::fs::create_dir_all(&fallback).unwrap();
        set_mtime(&fallback, 300);
        assert_eq!(last_used(&fallback), Some(300));
        std::fs::write(use_stamp_path(&fallback), b"").unwrap();
        set_mtime(&use_stamp_path(&fallback), 400);
        assert_eq!(last_used(&fallback), Some(400));
    }

    #[test]
    fn a_stamp_goes_with_its_unit_and_an_orphan_on_its_own() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit(&state.directory, "unit", 0);
        let stamp = use_stamp_path(&directory);
        std::fs::write(&stamp, b"r 1").unwrap();
        set_mtime(&stamp, 0);
        let orphan = state.directory.join("blake3/gone.use");
        std::fs::write(&orphan, b"r 22").unwrap();
        let before = stats(cache.path()).unwrap();
        assert_eq!(before.bytes, tree_bytes(&state.directory));

        // A live lease keeps the orphan: its build may be recreating the unit.
        collect(cache.path(), None, None, false, true, None).unwrap();
        assert!(orphan.exists());
        drop(state.lease);
        // A preview counts the orphan as the real run removes it.
        let preview = collect(cache.path(), Some(0), None, true, true, Some(0)).unwrap();
        assert_eq!(preview.removed_bytes, 32 + 3 + 4);
        assert!(stamp.exists() && orphan.exists());
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.removed_bytes, 32 + 3 + 4);
        assert_eq!(outcome.remaining_bytes, tree_bytes(&state.directory));
        assert!(!directory.exists());
        assert!(!stamp.exists());
        assert!(!orphan.exists());
    }

    /// A unit of finished rustc sessions, each 32 bytes of its own with an
    /// old lock file, last written at `used`.
    fn unit_with_sessions(checkout: &Path, name: &str, sessions: &[(&str, u64)]) -> PathBuf {
        let unit = checkout.join("blake3").join(name);
        for (session, used) in sessions {
            let session = unit.join("widget-0abc").join(session);
            std::fs::create_dir_all(&session).unwrap();
            std::fs::write(session.join("dep-graph.bin"), vec![0; 32]).unwrap();
            let lock =
                session.with_file_name(session_lock_name(session.file_name().unwrap()).unwrap());
            std::fs::write(&lock, b"").unwrap();
            set_mtime(&lock, 0);
            set_mtime(&session, *used);
        }
        unit
    }

    #[test]
    fn a_trimming_lease_is_taken_once_and_held_until_the_sweep_lets_go() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let key = state
            .directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let leases_on_disk = || {
            std::fs::read_dir(lock_dir(cache.path(), &key))
                .unwrap()
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lease"))
                .count()
        };
        let mut trimming = HashMap::new();
        // Two of the checkout's units are trimmed; the build ends between them.
        lease_for_trimming(&mut trimming, cache.path(), &key).unwrap();
        drop(state.lease);
        lease_for_trimming(&mut trimming, cache.path(), &key).unwrap();
        assert_eq!(leases_on_disk(), 1);
        assert_eq!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Active
        );

        drop(trimming);
        assert!(
            matches!(
                remove_workspace(cache.path(), workspace.path()).unwrap(),
                RemoveOutcome::Removed(_)
            ),
            "removable once the sweep let go"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_linked_after_the_snapshot_is_not_credited() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit_with_sessions(&state.directory, "unit", &[("s-1-a-old", 10)]);
        let session = directory.join("widget-0abc/s-1-a-old");
        set_mtime(&session, 0);
        // A link the snapshot never measured, as when a compile or a seed
        // links the file after collection listed the unit.
        let elsewhere = tempfile::tempdir().unwrap();
        let kept = elsewhere.path().join("still-linked.bin");
        std::fs::hard_link(session.join("dep-graph.bin"), &kept).unwrap();
        let before = stats(cache.path()).unwrap().bytes;

        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_sessions, 1);
        assert!(!session.exists());
        assert!(kept.exists(), "its bytes are still on disk");
        assert_eq!(outcome.removed_bytes, 0, "nothing was freed");
        assert_eq!(outcome.remaining_bytes, before);
        drop(state);
    }

    #[cfg(unix)]
    #[test]
    fn a_session_removed_only_in_part_still_counts_its_unit_as_trimmed() {
        use std::os::unix::fs::PermissionsExt as _;
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit_with_sessions(&state.directory, "unit", &[("s-1-a-old", 10)]);
        let session = directory.join("widget-0abc/s-1-a-old");
        // A file that cannot be removed stops the removal halfway.
        let stuck = session.join("stuck");
        std::fs::create_dir(&stuck).unwrap();
        std::fs::write(stuck.join("held"), b"held").unwrap();
        set_mtime(&session, 0);
        std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(stuck.join("probe"), b"").is_ok() {
            eprintln!("skipped: permissions are not enforced for this user");
            return;
        }

        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        let unfinished = directory.join("widget-0abc/s-1-a-working");
        std::fs::set_permissions(
            unfinished.join("stuck"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        assert!(!session.exists(), "out of rustc's reach");
        assert!(unfinished.join("stuck/held").exists());
        assert_eq!(outcome.removed_sessions, 0);
        assert_eq!(outcome.skipped_active_units, 0);
        assert_eq!(outcome.session_units.len(), 1);
        drop(state);
    }

    #[test]
    fn an_unfinished_leftover_under_the_same_name_does_not_block_eviction() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit_with_sessions(&state.directory, "unit", &[("s-1-a-old", 0)]);
        // What an eviction that stopped halfway leaves behind.
        let leftover = directory.join("widget-0abc/s-1-a-working");
        std::fs::create_dir(&leftover).unwrap();
        std::fs::write(leftover.join("part.bin"), b"part").unwrap();

        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_sessions, 1);
        assert!(!directory.join("widget-0abc/s-1-a-old").exists());
        assert!(!leftover.exists());
        drop(state);
    }

    fn lock_shared(lock: &Path) -> std::fs::File {
        let file = std::fs::File::open(lock).unwrap();
        file.try_lock_shared().unwrap();
        file
    }

    #[test]
    fn a_live_checkout_loses_finished_sessions_but_keeps_its_structure() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit_with_sessions(
            &state.directory,
            "unit",
            &[("s-1-a-old", 10), ("s-2-b-new", 20), ("s-3-c-working", 0)],
        );
        let krate = directory.join("widget-0abc");
        std::fs::write(krate.join("crate-level"), b"kept").unwrap();
        let record = directory.with_extension("json");
        std::fs::write(&record, b"churn").unwrap();
        let stamp = use_stamp_path(&directory);
        std::fs::write(&stamp, b"").unwrap();
        set_mtime(&stamp, 0);
        // A starting compiler, or a seed, copying the older session.
        let reader = lock_shared(&krate.join("s-1-a.lock"));

        let preview = collect(cache.path(), Some(0), None, true, true, Some(0)).unwrap();
        assert_eq!(preview.removed_sessions, 2, "a preview probes no lock");
        assert_eq!(preview.session_units.len(), 1);
        assert_eq!(preview.removed_bytes, 64);
        assert!(krate.join("s-2-b-new").exists());

        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_sessions, 1);
        assert_eq!(outcome.session_units.len(), 1);
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.skipped_active_directories, 1);
        assert_eq!(outcome.removed_bytes, 32);
        assert_eq!(
            outcome.remaining_bytes,
            stats(cache.path()).unwrap().bytes,
            "only what went is credited"
        );
        assert!(!krate.join("s-2-b-new").exists());
        assert!(!krate.join("s-2-b-working").exists());
        assert!(!krate.join("s-2-b.lock").exists());
        assert!(krate.join("s-1-a-old/dep-graph.bin").exists());
        assert!(krate.join("s-1-a.lock").exists());
        assert!(krate.join("s-3-c-working/dep-graph.bin").exists());
        assert!(krate.join("s-3-c.lock").exists());
        assert!(krate.join("crate-level").exists());
        assert!(record.exists() && stamp.exists());

        drop(reader);
        // A child process another test forks at this moment shares the open
        // lock until it execs, so the lock may stay held for an instant
        // after the drop. Retry briefly rather than race it.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let outcome = loop {
            let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
            if outcome.removed_sessions > 0 || std::time::Instant::now() >= deadline {
                break outcome;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(outcome.removed_sessions, 1);
        assert!(!krate.join("s-1-a-old").exists());
        assert!(!krate.join("s-1-a.lock").exists());
        assert!(krate.join("s-3-c-working").exists());
        assert!(directory.exists() && record.exists() && stamp.exists());
        drop(state);
    }

    #[test]
    fn evicting_a_session_credits_only_files_whose_last_link_went() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit_with_sessions(
            &state.directory,
            "unit",
            &[("s-1-a-old", 10), ("s-2-b-new", 20), ("s-3-c-working", 30)],
        );
        let krate = directory.join("widget-0abc");
        // rustc links what a new session reuses from the one it started from.
        for (from, to) in [
            ("s-1-a-old/dep-graph.bin", "s-2-b-new/reused.bin"),
            ("s-2-b-new/dep-graph.bin", "s-3-c-working/reused.bin"),
        ] {
            std::fs::hard_link(krate.join(from), krate.join(to)).unwrap();
        }
        for session in ["s-1-a-old", "s-2-b-new", "s-3-c-working"] {
            set_mtime(&krate.join(session), 0);
        }
        let before = stats(cache.path()).unwrap().bytes;

        // The older session's file is freed with its second link, in the
        // newer session; the newer one's own file lives on in the working one.
        let outcome = collect(
            cache.path(),
            Some(before - 1),
            None,
            false,
            true,
            Some(before - 1),
        )
        .unwrap();
        assert_eq!(outcome.removed_sessions, 2);
        assert_eq!(outcome.removed_bytes, 32);
        assert_eq!(outcome.remaining_bytes, before - 32);
        assert_eq!(stats(cache.path()).unwrap().bytes, before - 32);
        assert!(!krate.join("s-1-a-old").exists());
        assert!(!krate.join("s-2-b-new").exists());
        assert!(krate.join("s-3-c-working/reused.bin").exists());
        drop(state);
    }

    #[test]
    fn an_inode_shared_by_a_live_session_and_an_idle_unit_is_credited_once() {
        let cache = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        let idle = tempfile::tempdir().unwrap();
        let live_state = touch(cache.path(), live.path()).unwrap();
        let session_unit = unit_with_sessions(&live_state.directory, "unit", &[("s-1-a-x", 0)]);
        let idle_state = touch(cache.path(), idle.path()).unwrap();
        let idle_unit = unit(&idle_state.directory, "unit", 10);
        drop(idle_state.lease);
        // A seed linked the idle unit's file into the live one.
        let shared = session_unit.join("widget-0abc/s-1-a-x/dep-graph.bin");
        let linked = idle_unit.join("crate-id/s-session/dep-graph.bin");
        std::fs::remove_file(&linked).unwrap();
        std::fs::hard_link(&shared, &linked).unwrap();
        set_mtime(&session_unit.join("widget-0abc/s-1-a-x"), 0);
        set_mtime(&idle_unit.join("crate-id/s-session"), 10);
        let before = stats(cache.path()).unwrap().bytes;

        // The live session goes first, then the idle unit.
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_sessions, 1);
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.removed_bytes, 32, "freed with the second link");
        assert_eq!(outcome.remaining_bytes, before - 32);
        assert_eq!(stats(cache.path()).unwrap().bytes, before - 32);
        assert!(!idle_unit.exists());
        drop(live_state);
    }

    #[test]
    fn the_backstop_and_the_hot_floor_weigh_a_unit_alike() {
        let cache = tempfile::tempdir().unwrap();
        let untracked = cache.path().join("untracked");
        std::fs::create_dir(&untracked).unwrap();
        std::fs::write(untracked.join("state"), vec![0; 1000]).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory =
            unit_with_sessions(&state.directory, "unit", &[("s-1-a-x", 0), ("s-2-b-x", 0)]);
        // rustc linked the older session's file into the newer one.
        let krate = directory.join("widget-0abc");
        std::fs::remove_file(krate.join("s-2-b-x/dep-graph.bin")).unwrap();
        std::fs::hard_link(
            krate.join("s-1-a-x/dep-graph.bin"),
            krate.join("s-2-b-x/dep-graph.bin"),
        )
        .unwrap();
        for session in ["s-1-a-x", "s-2-b-x"] {
            set_mtime(&krate.join(session), 0);
        }
        let stamp = use_stamp_path(&directory);
        std::fs::write(&stamp, b"").unwrap();
        set_mtime(&stamp, now_secs() - 120);

        for (budget, oversized) in [(64, false), (63, true)] {
            assert_eq!(exceeds_backstop(&directory, Some(budget)), oversized);
            let outcome =
                collect(cache.path(), Some(budget), None, true, true, Some(budget)).unwrap();
            assert_eq!(
                outcome.kept_hot_units,
                u64::from(!oversized),
                "budget {budget}"
            );
        }
        drop(state);
    }

    /// rustc's two sessions of a unit share files: removing the unit removes
    /// every link, so the file's bytes are freed even though each path saw
    /// two links before the removal began.
    #[cfg(unix)]
    #[test]
    fn removing_a_tree_frees_a_file_whose_links_were_all_inside_it() {
        let tree = tempfile::tempdir().unwrap();
        let unit = tree.path().join("unit");
        std::fs::create_dir_all(unit.join("s-1")).unwrap();
        std::fs::create_dir_all(unit.join("s-2")).unwrap();
        std::fs::write(unit.join("s-1/shared.o"), vec![0; 64]).unwrap();
        std::fs::hard_link(unit.join("s-1/shared.o"), unit.join("s-2/shared.o")).unwrap();
        std::fs::write(unit.join("s-2/own.o"), vec![0; 8]).unwrap();
        let mut inodes = Inodes::default();
        inodes.measure(&unit, &HashSet::new());
        assert_eq!(inodes.bytes, 72);

        assert!(remove_tree(&unit, false, &mut inodes));

        assert!(!unit.exists());
        assert_eq!(inodes.bytes, 0);
    }

    #[test]
    fn removing_a_tree_removes_a_directory_symlink_without_following_it() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("kept"), b"kept").unwrap();
        let tree = tempfile::tempdir().unwrap();
        let unit = tree.path().join("unit");
        std::fs::create_dir(&unit).unwrap();
        std::fs::write(unit.join("state"), b"state").unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(outside.path(), unit.join("link"));
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(outside.path(), unit.join("link"));
        if let Err(error) = linked {
            eprintln!("skipped: cannot create a directory symlink here: {error}");
            return;
        }

        assert!(remove_tree(&unit, false, &mut Inodes::default()));
        assert!(!unit.exists());
        assert_eq!(std::fs::read(outside.path().join("kept")).unwrap(), b"kept");
    }

    #[test]
    fn the_backstop_counts_a_linked_file_once_only_when_the_sum_is_over() {
        let unit = tempfile::tempdir().unwrap();
        std::fs::write(unit.path().join("a"), vec![0; 20]).unwrap();
        std::fs::hard_link(unit.path().join("a"), unit.path().join("b")).unwrap();
        assert_eq!(listed_bytes(unit.path()), 40);
        assert_eq!(tree_bytes(unit.path()), 20);

        // Under the limit by the sum, the precise measure is never taken.
        assert!(!exceeds_backstop(unit.path(), Some(80)));
        // Over by the sum alone: one copy of the data fits.
        assert!(!exceeds_backstop(unit.path(), Some(40)));
        assert!(exceeds_backstop(unit.path(), Some(39)));
        assert!(!exceeds_backstop(unit.path(), None));
    }

    #[test]
    fn sessions_and_lock_files_inside_the_start_grace_are_kept() {
        for fresh_lock in [false, true] {
            let cache = tempfile::tempdir().unwrap();
            let workspace = tempfile::tempdir().unwrap();
            let state = touch(cache.path(), workspace.path()).unwrap();
            let used = if fresh_lock { 0 } else { now_secs() };
            let directory = unit_with_sessions(&state.directory, "unit", &[("s-1-a-x", used)]);
            let lock = directory.join("widget-0abc/s-1-a.lock");
            if fresh_lock {
                set_mtime(&lock, now_secs());
            }
            // Old by its stamp, so the floor does not decide this.
            std::fs::write(use_stamp_path(&directory), b"").unwrap();
            for dry_run in [true, false] {
                let outcome =
                    collect(cache.path(), Some(0), None, dry_run, false, Some(0)).unwrap();
                assert_eq!(outcome.removed_sessions, 0);
                assert_eq!(outcome.skipped_active_units, 1);
            }
            assert!(directory.join("widget-0abc/s-1-a-x").exists());
            drop(state);
        }
    }

    #[test]
    fn the_hot_floor_keeps_recent_units_unless_expired_oversized_or_short_of_disk() {
        let cache = tempfile::tempdir().unwrap();
        // Counted, never collected: keeps the budget under pressure.
        let untracked = cache.path().join("untracked");
        std::fs::create_dir(&untracked).unwrap();
        std::fs::write(untracked.join("state"), vec![0; 1000]).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit_with_sessions(&state.directory, "unit", &[("s-1-a-x", 0)]);
        let session = directory.join("widget-0abc/s-1-a-x");
        let stamp = use_stamp_path(&directory);
        std::fs::write(&stamp, b"").unwrap();
        set_mtime(&stamp, now_secs() - 120);

        for dry_run in [true, false] {
            let kept = collect(cache.path(), Some(100), None, dry_run, true, Some(100)).unwrap();
            assert_eq!(kept.kept_hot_units, 1);
            assert_eq!(kept.removed_sessions, 0);
        }
        assert!(session.exists());
        let expired = collect(
            cache.path(),
            Some(100),
            Some(Duration::from_secs(60)),
            true,
            true,
            Some(100),
        )
        .unwrap();
        assert_eq!(expired.kept_hot_units, 0);
        assert_eq!(expired.removed_sessions, 1);
        let oversized = collect(cache.path(), Some(63), None, true, true, Some(63)).unwrap();
        assert_eq!(oversized.kept_hot_units, 0);
        assert_eq!(oversized.removed_sessions, 1);
        // Generated sources or a combined budget can narrow collection's
        // limit below the budget compilers are given. A unit the compiler
        // accepts stays protected.
        let narrowed = collect(cache.path(), Some(63), None, true, true, Some(100)).unwrap();
        assert_eq!(narrowed.kept_hot_units, 1);
        assert_eq!(narrowed.removed_sessions, 0);
        let low_disk = collect(cache.path(), Some(100), None, false, false, Some(100)).unwrap();
        assert_eq!(low_disk.removed_sessions, 1);
        assert!(!session.exists());
        drop(state);
    }

    #[test]
    fn session_eviction_stops_once_under_budget() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let older =
            unit_with_sessions(&state.directory, "older", &[("s-1-a-x", 0), ("s-2-b-x", 1)]);
        let newer = unit_with_sessions(&state.directory, "newer", &[("s-3-c-x", 5)]);
        let before = stats(cache.path()).unwrap().bytes;
        let outcome = collect(
            cache.path(),
            Some(before - 32),
            None,
            false,
            true,
            Some(before - 32),
        )
        .unwrap();
        assert_eq!(outcome.removed_sessions, 1);
        assert_eq!(outcome.remaining_bytes, before - 32);
        assert!(!older.join("widget-0abc/s-1-a-x").exists());
        assert!(older.join("widget-0abc/s-2-b-x").exists());
        assert!(newer.join("widget-0abc/s-3-c-x").exists());
        drop(state);
    }

    #[test]
    fn active_checkout_lease_protects_all_units() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        unit(&state.directory, "first", 0);
        unit(&state.directory, "second", 1);
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.skipped_active_units, 2);
        assert_eq!(outcome.skipped_active_directories, 1);
    }

    #[test]
    fn gone_checkout_keeps_units_until_eviction_then_removes_bookkeeping() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        unit(&state.directory, "unit", 0);
        std::fs::write(state.directory.join("file-digests.json"), b"bookkeeping").unwrap();
        drop(state.lease);
        workspace.close().unwrap();
        let before = stats(cache.path()).unwrap();
        let kept = collect(cache.path(), None, None, false, true, None).unwrap();
        assert_eq!(kept.removed_units, 0);
        assert_eq!(kept.removed_directories, 0);
        assert!(state.directory.exists());
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.removed_directories, 1);
        assert_eq!(outcome.removed_bytes, before.bytes);
        assert_eq!(outcome.remaining_bytes, 0);
        assert!(!state.directory.exists());
    }

    #[test]
    fn hard_linked_sessions_share_the_budget_and_only_free_the_last_reference() {
        let cache = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_state = touch(cache.path(), first.path()).unwrap();
        let old = unit(&first_state.directory, "unit", 0);
        let second_state = touch(cache.path(), second.path()).unwrap();
        let recent = unit(&second_state.directory, "unit", now_secs());
        let shared = old.join("crate-id/s-session/dep-graph.bin");
        let linked = recent.join("crate-id/s-session/dep-graph.bin");
        std::fs::remove_file(&linked).unwrap();
        std::fs::hard_link(&shared, &linked).unwrap();
        // Repeated links inside a unit also count only once.
        std::fs::hard_link(&shared, old.join("crate-id/s-session/alias.bin")).unwrap();
        // Creating the alias refreshed the session directory; keep this
        // donor old so the test exercises eviction rather than startup grace.
        filetime::set_file_mtime(
            old.join("crate-id/s-session"),
            filetime::FileTime::from_unix_time(0, 0),
        )
        .unwrap();
        drop(first_state.lease);
        drop(second_state.lease);
        let before = stats(cache.path()).unwrap();
        assert_eq!(
            before.bytes,
            tree_bytes(&first_state.directory) + tree_bytes(&second_state.directory) - 32
        );
        let within_budget = collect(
            cache.path(),
            Some(before.bytes + 16),
            None,
            false,
            true,
            Some(before.bytes + 16),
        )
        .unwrap();
        assert_eq!(within_budget.removed_units, 0);
        assert_eq!(within_budget.remaining_bytes, before.bytes);

        let one_removed = collect(
            cache.path(),
            None,
            Some(Duration::from_secs(60)),
            false,
            true,
            None,
        )
        .unwrap();
        assert_eq!(one_removed.removed_units, 1);
        assert_eq!(
            one_removed.removed_bytes, 0,
            "the other unit still owns the inode"
        );
        assert_eq!(one_removed.remaining_bytes, before.bytes);
        assert_eq!(stats(cache.path()).unwrap().bytes, before.bytes);
        assert!(!old.exists());
        assert!(linked.exists());

        filetime::set_file_mtime(
            recent.join("crate-id/s-session"),
            filetime::FileTime::from_unix_time(0, 0),
        )
        .unwrap();
        let last_removed = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(last_removed.removed_units, 1);
        assert_eq!(last_removed.removed_bytes, 32);
    }

    #[test]
    fn bookkeeping_links_share_accounting_with_unit_files() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit(&state.directory, "unit", 0);
        let private = state.directory.join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::hard_link(
            directory.join("crate-id/s-session/dep-graph.bin"),
            private.join("linked"),
        )
        .unwrap();
        drop(state.lease);
        let before = stats(cache.path()).unwrap();
        assert_eq!(before.bytes, tree_bytes(&state.directory));
        let removed = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(removed.removed_units, 1);
        assert_eq!(removed.removed_bytes, 0);
        assert_eq!(removed.remaining_bytes, before.bytes);
        assert!(private.join("linked").exists());
    }

    #[test]
    fn deleted_checkout_units_follow_the_same_lru_order_as_live_units() {
        let cache = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        let gone = tempfile::tempdir().unwrap();
        let live_state = touch(cache.path(), live.path()).unwrap();
        let oldest = unit(&live_state.directory, "unit", 0);
        let gone_state = touch(cache.path(), gone.path()).unwrap();
        let donor = unit(&gone_state.directory, "unit", 100);
        drop(live_state.lease);
        drop(gone_state.lease);
        gone.close().unwrap();
        let before = stats(cache.path()).unwrap();
        let outcome = collect(
            cache.path(),
            Some(before.bytes - 32),
            None,
            false,
            true,
            Some(before.bytes - 32),
        )
        .unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.removed_directories, 0);
        assert!(!oldest.exists());
        assert!(donor.exists());
    }

    #[test]
    fn a_seed_donors_lease_protects_its_units_after_the_workspace_is_deleted() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let key = checkout_key(workspace.path());
        let state = touch(cache.path(), workspace.path()).unwrap();
        let donor = unit(&state.directory, "unit", 0);
        drop(state.lease);
        workspace.close().unwrap();
        let seeding = acquire_lease(cache.path(), &key).unwrap();
        for dry_run in [true, false] {
            let outcome = collect(cache.path(), Some(0), None, dry_run, true, Some(0)).unwrap();
            assert_eq!(outcome.removed_units, 0);
            assert_eq!(outcome.skipped_active_units, 1);
            assert_eq!(outcome.skipped_active_directories, 1);
            assert!(donor.exists());
        }
        drop(seeding);
        assert_eq!(
            collect(cache.path(), Some(0), None, false, true, Some(0))
                .unwrap()
                .removed_directories,
            1
        );
    }

    #[test]
    fn fresh_seeding_staging_is_protected_and_stale_staging_is_collected() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let staging = state.directory.join("blake3/unit.seeding-123");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("partial"), b"partial").unwrap();
        let churn = state.directory.join("blake3/unit.json");
        std::fs::write(&churn, b"destination churn").unwrap();
        drop(state.lease);
        for dry_run in [true, false] {
            let outcome = collect(cache.path(), Some(0), None, dry_run, true, Some(0)).unwrap();
            assert_eq!(outcome.removed_units, 0);
            assert_eq!(outcome.skipped_active_units, 1);
            assert!(staging.exists());
        }
        filetime::set_file_mtime(&staging, filetime::FileTime::from_unix_time(0, 0)).unwrap();
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.removed_bytes, 7);
        assert!(!staging.exists());
        assert!(
            churn.exists(),
            "staging must not own the destination's churn record"
        );
    }

    #[test]
    fn unreadable_state_is_counted_but_not_deleted() {
        let cache = tempfile::tempdir().unwrap();
        let directory = cache.path().join("untracked");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("state"), vec![0; 32]).unwrap();
        let untracked_unit = unit(&directory, "unit", 0);

        let stats = stats(cache.path()).unwrap();
        assert_eq!(stats.directories, 1);
        assert_eq!(stats.untracked_directories, 1);
        assert_eq!(stats.bytes, 64);
        assert_eq!(stats.units, 1);
        let outcome = collect(cache.path(), Some(0), None, false, true, Some(0)).unwrap();
        assert_eq!(outcome.untracked_directories, 1);
        assert_eq!(outcome.remaining_bytes, 64);
        assert_eq!(outcome.remaining_units, 1);
        assert!(untracked_unit.exists());
        assert!(directory.exists());
    }

    #[test]
    fn removal_releases_registrar_before_deleting_and_allows_same_workspace() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = touch(cache.path(), workspace.path()).unwrap();
        let live = checkout.directory.clone();
        std::fs::write(live.join("old-state"), b"old").unwrap();
        let expected_bytes = tree_bytes(&live);
        drop(checkout);
        // This separate open must observe a held registrar. The pre-retirement
        // implementation kept this guard through the recursive deletion.
        let guard = deletion_guard(cache.path(), &checkout_key(workspace.path()), true).unwrap();
        let mut probe = fslock::LockFile::open(&registrar_path(cache.path())).unwrap();
        assert!(!probe.try_lock().unwrap());
        drop(guard);
        let mut fresh = None;
        let mut retired = None;
        let outcome = remove_workspace_with(cache.path(), workspace.path(), || {
            assert!(
                probe.try_lock().unwrap(),
                "deletion must not hold the registrar"
            );
            probe.unlock().unwrap();
            let path = std::fs::read_dir(cache.path())
                .unwrap()
                .flatten()
                .find(|entry| retired::is_retired_name(&entry.file_name()))
                .unwrap()
                .path();
            assert!(path.join("old-state").exists());
            assert!(!path.join(RECORD_FILE).exists());
            fresh = Some(touch(cache.path(), workspace.path()).unwrap());
            assert!(live.exists());
            assert!(path.exists());
            retired = Some(path);
        })
        .unwrap();
        assert_eq!(outcome, RemoveOutcome::Removed(expected_bytes));
        assert!(!retired.unwrap().exists());
        assert!(fresh.unwrap().directory.exists());
        assert!(!live.join("old-state").exists());
    }

    #[test]
    fn record_unlink_failure_restores_live_checkout_and_preserves_locks() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = touch(cache.path(), workspace.path()).unwrap();
        let live = checkout.directory.clone();
        drop(checkout);
        let locks = lock_dir(cache.path(), &checkout_key(workspace.path()));
        std::fs::create_dir_all(&locks).unwrap();
        let error = remove_workspace_with_unlink(
            cache.path(),
            workspace.path(),
            || panic!("failed retirement must not proceed to deletion"),
            |tree| {
                assert!(!live.exists());
                assert!(tree.path().join(RECORD_FILE).is_file());
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected record unlink failure",
                ))
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("injected record unlink failure"));
        assert!(live.join(RECORD_FILE).is_file());
        assert!(locks.exists());
        assert!(retired_trees(cache.path()).is_empty());
        let mut registrar = fslock::LockFile::open(&registrar_path(cache.path())).unwrap();
        assert!(registrar.try_lock().unwrap());
    }

    #[test]
    fn collection_restores_failed_record_unlink_and_continues_to_next_checkout() {
        let cache = tempfile::tempdir().unwrap();
        for _ in 0..2 {
            let workspace = tempfile::tempdir().unwrap();
            drop(touch(cache.path(), workspace.path()).unwrap());
            std::fs::create_dir_all(lock_dir(cache.path(), &checkout_key(workspace.path())))
                .unwrap();
            // Dropping the workspace makes this empty checkout abandoned.
        }
        let mut restored = None;
        let mut attempts = 0;
        let outcome = collect_with_unlink(
            cache.path(),
            Some(0),
            None,
            false,
            false,
            None,
            |tree, inodes| {
                attempts += 1;
                if attempts == 1 {
                    let record = read_record(&tree.path().join(RECORD_FILE)).unwrap();
                    restored = Some(checkout_key(&record.workspace_root));
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected record unlink failure",
                    ))
                } else {
                    unlink_retired_checkout_record(tree, inodes)
                }
            },
        )
        .unwrap();
        assert_eq!(attempts, 2);
        let key = restored.unwrap();
        let live = cache.path().join(&key);
        assert!(live.join(RECORD_FILE).is_file());
        assert!(lock_dir(cache.path(), &key).exists());
        assert!(retired_trees(cache.path()).is_empty());
        assert_eq!(outcome.remaining_directories, 1);
        assert_eq!(outcome.removed_directories, 1);
        assert_eq!(outcome.remaining_bytes, stats(cache.path()).unwrap().bytes);
    }

    #[test]
    fn record_unlink_rollback_failure_reports_both_errors() {
        let cache = tempfile::tempdir().unwrap();
        let live = cache.path().join("checkout");
        std::fs::create_dir(&live).unwrap();
        let tree = RetiredTree::retire(&live).unwrap().unwrap();
        // A nonempty destination makes the restore fail on every platform.
        std::fs::create_dir(&live).unwrap();
        std::fs::write(live.join("occupied"), b"occupied").unwrap();
        let unlink_error = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected record unlink failure",
        );
        let error = restore_retired_checkout(&tree, &live, &unlink_error).unwrap_err();
        let message = format!("{error:?}");
        assert!(message.contains("injected record unlink failure"));
        assert!(message.contains("failed to restore incremental checkout"));
        assert!(message.contains(&tree.path().display().to_string()));
        assert!(
            error.source().is_some(),
            "retain the rename error as a cause"
        );
    }

    #[test]
    fn retired_checkouts_and_units_are_hidden_and_swept() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        let donor = checkout_with_sessions(cache.path(), "donor", 20, unit, &["s-2-b-finished"]);
        let retired = donor.with_file_name("donor.retired-123-leftover");
        std::fs::rename(&donor, &retired).unwrap();
        let nested = own.join("blake3/unused.retired-123-leftover");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("state"), b"nested").unwrap();
        let bytes = tree_bytes(&retired) + tree_bytes(&nested);
        assert_eq!(stats(cache.path()).unwrap().directories, 1);
        assert_eq!(stats(cache.path()).unwrap().units, 0);
        assert_eq!(
            workspaces(cache.path()).unwrap(),
            [PathBuf::from("/work/own")]
        );
        assert_eq!(seed(&own, unit).unwrap(), None);
        collect(cache.path(), None, None, true, false, None).unwrap();
        assert!(retired.exists());
        assert!(nested.exists());
        let outcome = collect(cache.path(), None, None, false, false, None).unwrap();
        assert!(outcome.removed_bytes >= bytes);
        assert!(!retired.exists());
        assert!(!nested.exists());
    }

    #[test]
    fn retired_bytes_share_stats_ledger_with_live_state() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let before = stats(cache.path()).unwrap().bytes;
        let retired = cache.path().join("donor.retired-123-leftover");
        std::fs::create_dir_all(&retired).unwrap();
        std::fs::write(retired.join("shared"), b"leftover").unwrap();
        std::fs::hard_link(retired.join("shared"), state.directory.join("shared")).unwrap();
        let nested = state.directory.join("blake3/unit.retired-123-leftover");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("state"), b"unit").unwrap();
        std::fs::write(cache.path().join("stamp.retired-123-leftover"), b"use").unwrap();
        let measured = stats(cache.path()).unwrap();
        assert_eq!(measured.bytes, before + 8 + 4 + 3);
        assert_eq!(measured.directories, 1);
        assert_eq!(measured.units, 0);
        let preview = collect(cache.path(), None, None, true, false, None).unwrap();
        assert_eq!(
            preview.removed_bytes, 7,
            "the live hard link keeps eight bytes"
        );
        assert_eq!(preview.remaining_bytes, before + 8);
        let outcome = collect(cache.path(), None, None, false, false, None).unwrap();
        assert_eq!(outcome.removed_bytes, preview.removed_bytes);
        assert_eq!(outcome.remaining_bytes, preview.remaining_bytes);
        assert_eq!(stats(cache.path()).unwrap().bytes, before + 8);
    }

    #[cfg(unix)]
    #[test]
    fn failed_retired_incremental_deletion_stays_counted_until_retry() {
        use std::os::unix::fs::PermissionsExt as _;
        // Root can bypass the directory permissions used to force failure.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let _state = touch(cache.path(), workspace.path()).unwrap();
        let before = stats(cache.path()).unwrap().bytes;
        let retired = cache.path().join("stamp.retired-123-leftover");
        std::fs::write(&retired, b"leftover").unwrap();
        let permissions = std::fs::metadata(cache.path()).unwrap().permissions();
        std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let outcome = collect(cache.path(), None, None, false, false, None);
        std::fs::set_permissions(cache.path(), permissions).unwrap();
        let outcome = outcome.unwrap();
        assert_eq!(outcome.removed_bytes, 0);
        assert_eq!(outcome.remaining_bytes, before + 8);
        assert_eq!(stats(cache.path()).unwrap().bytes, before + 8);
        assert!(retired.exists());
        let outcome = collect(cache.path(), None, None, false, false, None).unwrap();
        assert_eq!(outcome.removed_bytes, 8);
        assert_eq!(outcome.remaining_bytes, before);
        assert!(!retired.exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn unlinking_checkout_record_forgets_its_measured_identity() {
        let cache = tempfile::tempdir().unwrap();
        let directory = cache.path().join("checkout");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(RECORD_FILE), b"record").unwrap();
        let tree = RetiredTree::retire(&directory).unwrap().unwrap();
        let mut inodes = Inodes::default();
        inodes.measure(tree.path(), &HashSet::new());
        assert_eq!(inodes.bytes, 6);
        assert_eq!(
            unlink_retired_checkout_record(&tree, Some(&mut inodes)).unwrap(),
            6
        );
        assert_eq!(inodes.bytes, 0);
        assert!(!tree.path().join(RECORD_FILE).exists());
    }

    #[test]
    fn explicit_removal_targets_one_workspace() {
        let cache = tempfile::tempdir().unwrap();
        let selected = tempfile::tempdir().unwrap();
        let kept = tempfile::tempdir().unwrap();
        let selected_state = touch(cache.path(), selected.path()).unwrap();
        let kept_state = touch(cache.path(), kept.path()).unwrap();
        let selected_directory = selected_state.directory.clone();
        drop(selected_state);
        drop(kept_state.lease);

        assert!(matches!(
            remove_workspace(cache.path(), selected.path()).unwrap(),
            RemoveOutcome::Removed(_)
        ));
        assert!(!selected_directory.exists());
        assert!(kept_state.directory.exists());
    }

    /// A checkout recorded at `updated_secs`, holding the given rustc session
    /// directories for one unit, last written then too.
    fn checkout_with_sessions(
        cache: &Path,
        key: &str,
        updated_secs: u64,
        unit: &str,
        sessions: &[&str],
    ) -> PathBuf {
        crate::util::write_advisory(&cache.join(EPOCH_FILE), br#"{"epoch":1,"pending":[]}"#)
            .unwrap();
        let directory = cache.join(key);
        for session in sessions {
            let session = directory.join(unit).join("widget-0abc").join(session);
            std::fs::create_dir_all(&session).unwrap();
            std::fs::write(session.join("dep-graph.bin"), key).unwrap();
            let lock = session_lock_name(session.file_name().unwrap()).unwrap();
            std::fs::write(session.with_file_name(lock), b"").unwrap();
            filetime::set_file_mtime(
                &session,
                filetime::FileTime::from_unix_time(updated_secs as i64, 0),
            )
            .unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        let record = CheckoutRecord {
            version: RECORD_VERSION,
            workspace_root: PathBuf::from(format!("/work/{key}")),
            updated_secs,
        };
        std::fs::write(
            directory.join(RECORD_FILE),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        directory
    }

    fn retired_checkouts(root: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| retired::is_retired_name(&entry.file_name()))
            .map(|entry| entry.path())
            .collect()
    }

    #[test]
    fn upgrading_discards_shared_state_once_and_collection_sweeps_it() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(
            cache.path(),
            &checkout_key(workspace.path()),
            10,
            unit,
            &["s-1-a-finished"],
        );
        let donor = checkout_with_sessions(cache.path(), "donor", 20, unit, &["s-2-b-finished"]);
        let source = own
            .join(unit)
            .join("widget-0abc/s-1-a-finished/state.pre-lto.bc");
        std::fs::write(&source, b"shared bitcode").unwrap();
        std::fs::hard_link(
            &source,
            donor
                .join(unit)
                .join("widget-0abc/s-2-b-finished/state.pre-lto.bc"),
        )
        .unwrap();
        std::fs::remove_file(cache.path().join(EPOCH_FILE)).unwrap();

        let checkout = touch(cache.path(), workspace.path()).unwrap();
        assert_eq!(checkout.directory, own);
        assert!(!own.join(unit).exists());
        assert!(!donor.exists());
        assert_eq!(
            std::fs::read(cache.path().join(EPOCH_FILE)).unwrap(),
            br#"{"epoch":1,"pending":[]}"#
        );
        let retired = retired_checkouts(cache.path());
        assert_eq!(retired.len(), 2);
        assert!(retired.iter().all(|path| !path.join(RECORD_FILE).exists()));
        // The shared bitcode is one file linked from both trees: it is freed,
        // and counted, once.
        let mut measured = Inodes::default();
        for path in &retired {
            measured.measure(path, &HashSet::new());
        }
        let bytes = measured.bytes;
        assert!(bytes < retired.iter().map(|path| tree_bytes(path)).sum::<u64>());
        assert!(bytes >= b"shared bitcode".len() as u64);
        assert_eq!(stats(cache.path()).unwrap().directories, 1);
        assert_eq!(stats(cache.path()).unwrap().units, 0);
        assert_eq!(workspaces(cache.path()).unwrap(), [workspace.path()]);
        let preview = collect(cache.path(), None, None, true, false, None).unwrap();
        assert_eq!(preview.removed_bytes, bytes);
        assert!(retired.iter().all(|path| path.exists()));
        let outcome = collect(cache.path(), None, None, false, false, None).unwrap();
        assert_eq!(outcome.removed_bytes, bytes);
        assert!(retired.iter().all(|path| !path.exists()));
        std::fs::write(own.join("new-state"), b"keep").unwrap();
        drop(checkout);
        let _next = touch(cache.path(), workspace.path()).unwrap();
        assert!(own.join("new-state").exists());
        assert!(retired_checkouts(cache.path()).is_empty());
    }

    #[test]
    fn upgrading_defers_active_checkouts_and_never_discards_new_ones() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let busy = checkout_with_sessions(cache.path(), "busy", 20, unit, &["s-2-b-finished"]);
        checkout_with_sessions(cache.path(), "idle", 10, unit, &["s-1-a-finished"]);
        std::fs::remove_file(cache.path().join(EPOCH_FILE)).unwrap();
        let lease = acquire_lease(cache.path(), "busy").unwrap();
        assert!(deletion_guard(cache.path(), "busy", false).unwrap().active);
        let checkout = touch(cache.path(), workspace.path()).unwrap();
        assert_eq!(read_epoch(cache.path()).unwrap().pending, ["busy"]);
        assert!(busy.exists());
        assert!(!cache.path().join("idle").exists());
        assert_eq!(seed(&checkout.directory, unit).unwrap(), None);

        let fresh_workspace = tempfile::tempdir().unwrap();
        let fresh = touch(cache.path(), fresh_workspace.path()).unwrap();
        let session = fresh
            .directory
            .join(unit)
            .join("widget-0abc/s-3-c-finished");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(session.join("dep-graph.bin"), b"fresh state").unwrap();
        std::fs::write(session.with_file_name("s-3-c.lock"), b"").unwrap();
        drop(fresh);
        let again = touch(cache.path(), workspace.path()).unwrap();
        assert!(session.exists());
        assert_eq!(read_epoch(cache.path()).unwrap().pending, ["busy"]);
        assert_eq!(seed(&again.directory, unit).unwrap(), None);
        drop(lease);
        let _last = touch(cache.path(), workspace.path()).unwrap();
        assert!(!busy.exists());
        assert!(session.exists());
        assert!(copy_state_ready(cache.path()));
        assert_eq!(
            seed(&checkout.directory, unit).unwrap(),
            Some(fresh_workspace.path().to_path_buf())
        );
        assert_eq!(
            std::fs::read(
                checkout
                    .directory
                    .join(unit)
                    .join("widget-0abc/s-3-c-finished/dep-graph.bin")
            )
            .unwrap(),
            b"fresh state"
        );
        assert!(!lock_dir(cache.path(), "busy").exists());
    }

    #[test]
    fn fresh_roots_record_the_copy_epoch_without_retiring_anything() {
        let cache = tempfile::tempdir().unwrap();
        let root = cache.path().join("incremental");
        let workspace = tempfile::tempdir().unwrap();
        let _checkout = touch(&root, workspace.path()).unwrap();
        assert_eq!(
            std::fs::read(root.join(EPOCH_FILE)).unwrap(),
            br#"{"epoch":1,"pending":[]}"#
        );
        assert!(retired_checkouts(&root).is_empty());
        assert_eq!(stats(&root).unwrap().directories, 1);
        assert_eq!(workspaces(&root).unwrap(), [workspace.path()]);
        let outcome = collect(&root, None, None, false, false, None).unwrap();
        assert_eq!(outcome.remaining_directories, 1);
        assert_eq!(outcome.removed_bytes, 0);
    }

    #[test]
    fn seeding_waits_for_a_readable_complete_copy_epoch() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        checkout_with_sessions(cache.path(), "donor", 20, unit, &["s-2-b-finished"]);
        std::fs::remove_file(cache.path().join(EPOCH_FILE)).unwrap();
        assert_eq!(seed(&own, unit).unwrap(), None);
        for content in [
            "invalid JSON",
            r#"{"epoch":0,"pending":[]}"#,
            r#"{"epoch":1,"pending":["donor"]}"#,
        ] {
            std::fs::write(cache.path().join(EPOCH_FILE), content).unwrap();
            assert_eq!(seed(&own, unit).unwrap(), None);
        }
        std::fs::write(
            cache.path().join(EPOCH_FILE),
            br#"{"epoch":2,"pending":[]}"#,
        )
        .unwrap();
        assert_eq!(
            seed(&own, unit).unwrap(),
            Some(PathBuf::from("/work/donor"))
        );
    }

    #[test]
    fn seeding_copies_the_newest_finished_state_of_another_checkout() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        checkout_with_sessions(cache.path(), "older", 10, unit, &["s-1-a-finished"]);
        checkout_with_sessions(cache.path(), "newer", 20, unit, &["s-2-b-finished"]);
        // The most recent, but its compiler is still writing its only session.
        checkout_with_sessions(cache.path(), "busy", 30, unit, &["s-3-c-working"]);
        checkout_with_sessions(
            cache.path(),
            "other",
            50,
            "blake3/9876543210",
            &["s-4-d-done"],
        );

        let donor = seed(&own, unit).unwrap();

        assert_eq!(donor, Some(PathBuf::from("/work/newer")));
        let copied = own.join(unit).join("widget-0abc");
        assert_eq!(
            std::fs::read_to_string(copied.join("s-2-b-finished/dep-graph.bin")).unwrap(),
            "newer"
        );
        // One session, and its lock file.
        assert_eq!(std::fs::read_dir(&copied).unwrap().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let donor = cache.path().join("newer").join(unit).join("widget-0abc");
            let inode = |path: &Path| std::fs::metadata(path).unwrap().ino();
            assert_ne!(
                inode(&copied.join("s-2-b-finished/dep-graph.bin")),
                inode(&donor.join("s-2-b-finished/dep-graph.bin")),
                "state files must have independent inodes"
            );
            assert_ne!(
                inode(&copied.join("s-2-b.lock")),
                inode(&donor.join("s-2-b.lock")),
                "lock files are not shared"
            );
        }
        let original = cache
            .path()
            .join("newer")
            .join(unit)
            .join("widget-0abc/s-2-b-finished/dep-graph.bin");
        let copy = copied.join("s-2-b-finished/dep-graph.bin");
        assert_eq!(
            std::fs::metadata(&original).unwrap().modified().unwrap(),
            std::fs::metadata(&copy).unwrap().modified().unwrap()
        );
        // Model rustc truncating and rewriting an inherited session file.
        std::fs::write(&copy, b"rewritten by recipient").unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"newer");
        assert_eq!(
            std::fs::read_dir(own.join("blake3")).unwrap().count(),
            1,
            "no staging directory is left behind"
        );
    }

    #[test]
    fn seeding_prefers_the_donor_whose_unit_was_used_last() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        // Its checkout record is newer, but its state for this unit is older.
        checkout_with_sessions(cache.path(), "recorded", 30, unit, &["s-1-a-finished"]);
        let hit = checkout_with_sessions(cache.path(), "hit", 10, unit, &["s-2-b-finished"]);
        let stamp = use_stamp_path(&hit.join(unit));
        std::fs::write(&stamp, b"").unwrap();
        set_mtime(&stamp, 35);

        assert_eq!(seed(&own, unit).unwrap(), Some(PathBuf::from("/work/hit")));
    }

    #[test]
    fn seeding_breaks_a_tie_in_last_use_by_the_checkout_built_last_then_by_name() {
        let unit = "blake3/0123456789";
        let tied = |donors: &[(&str, u64)]| {
            let cache = tempfile::tempdir().unwrap();
            let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
            for (key, updated) in donors {
                let donor =
                    checkout_with_sessions(cache.path(), key, *updated, unit, &["s-1-a-finished"]);
                set_mtime(&donor.join(unit).join("widget-0abc/s-1-a-finished"), 20);
            }
            seed(&own, unit).unwrap()
        };

        assert_eq!(
            tied(&[("a", 20), ("b", 30)]),
            Some(PathBuf::from("/work/b"))
        );
        assert_eq!(
            tied(&[("b", 20), ("a", 20)]),
            Some(PathBuf::from("/work/a"))
        );
    }

    #[test]
    fn seeding_leaves_unfinished_sessions_and_unknown_units_alone() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        checkout_with_sessions(
            cache.path(),
            "donor",
            20,
            unit,
            &["s-1-a-finished", "s-2-b-working"],
        );

        assert_eq!(seed(&own, "blake3/9999999999").unwrap(), None);
        assert!(!own.join("blake3/9999999999").exists());

        seed(&own, unit).unwrap();
        let copied = own.join(unit).join("widget-0abc");
        assert!(copied.join("s-1-a-finished").is_dir());
        assert!(!copied.join("s-2-b-working").exists());
    }

    /// Hold `session`'s lock the way rustc does while it removes the session.
    fn lock_exclusively(checkout: &Path, unit: &str, session: &str) -> std::fs::File {
        let lock = checkout
            .join(unit)
            .join("widget-0abc")
            .join(session_lock_name(std::ffi::OsStr::new(session)).unwrap());
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock)
            .unwrap();
        file.try_lock().unwrap();
        file
    }

    #[test]
    fn seeding_leaves_out_a_session_the_donor_is_removing() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        let donor = checkout_with_sessions(
            cache.path(),
            "donor",
            20,
            unit,
            &["s-1-a-superseded", "s-2-b-current"],
        );
        let _removing = lock_exclusively(&donor, unit, "s-1-a-superseded");

        seed(&own, unit).unwrap();

        let copied = own.join(unit).join("widget-0abc");
        assert!(copied.join("s-2-b-current").is_dir());
        assert!(copied.join("s-2-b.lock").is_file());
        assert!(!copied.join("s-1-a-superseded").exists());
        assert!(!copied.join("s-1-a.lock").exists());
    }

    #[test]
    fn seeding_falls_back_to_another_donor_when_one_has_nothing_to_give() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        checkout_with_sessions(cache.path(), "older", 10, unit, &["s-1-a-finished"]);
        let busy = checkout_with_sessions(cache.path(), "newer", 20, unit, &["s-2-b-finished"]);
        let _removing = lock_exclusively(&busy, unit, "s-2-b-finished");

        assert_eq!(
            seed(&own, unit).unwrap(),
            Some(PathBuf::from("/work/older"))
        );
        assert!(
            own.join(unit)
                .join("widget-0abc")
                .join("s-1-a-finished")
                .is_dir()
        );
        assert_eq!(
            std::fs::read_dir(own.join("blake3")).unwrap().count(),
            1,
            "the abandoned copy is cleaned up"
        );
    }

    #[test]
    fn a_crate_named_s_is_seeded_too() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        let donor = cache.path().join("donor");
        let krate = donor.join(unit).join("s-0abc");
        std::fs::create_dir_all(krate.join("s-1-a-finished")).unwrap();
        std::fs::write(krate.join("s-1-a-finished/dep-graph.bin"), b"s").unwrap();
        std::fs::write(krate.join("s-1-a.lock"), b"").unwrap();
        let record = CheckoutRecord {
            version: RECORD_VERSION,
            workspace_root: PathBuf::from("/work/donor"),
            updated_secs: 20,
        };
        std::fs::write(
            donor.join(RECORD_FILE),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();

        assert_eq!(
            seed(&own, unit).unwrap(),
            Some(PathBuf::from("/work/donor"))
        );
        assert!(
            own.join(unit)
                .join("s-0abc/s-1-a-finished/dep-graph.bin")
                .is_file()
        );
    }

    #[test]
    fn a_checkout_never_seeds_from_itself() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &["s-1-a-finished"]);
        std::fs::remove_dir_all(own.join(unit)).unwrap();

        assert_eq!(seed(&own, unit).unwrap(), None);
    }
}
