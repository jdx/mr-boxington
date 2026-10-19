//! Persistent per-checkout state for learned incremental compilation.
//!
//! What keeps a unit's state from being collected while it is in use:
//!
//! - A shim holds the unit's use stamp (`<unit>.use`) shared from before it
//!   seeds the unit until after it has written the churn record, and stamps
//!   it only once it decides to compile with the state.
//! - rustc holds its own session lock inside the unit while it compiles.
//! - If the shim dies, its lock goes with it, and a surviving rustc is
//!   protected by its session lock alone, which collection takes and holds
//!   through renaming the unit aside.
//! - If the session dies but its shims survive, their use stamps and rustc's
//!   session locks still protect their units, both against unit eviction and
//!   against removal of the whole checkout, which does not trust the session
//!   lease alone.
//! - Before rustc locks anything, each compilation's own
//!   `<unit>/compiling-<pid>-<random>` marker covers a compiler starting up:
//!   its shim creates the marker when the compilation accepts the state,
//!   refreshes it right before rustc starts, after the machine has admitted
//!   the compilation, and removes it when the compilation finishes. A marker
//!   touched within the session-start grace keeps its unit, and its checkout,
//!   even if the shim and session have died. So does an unfinished `-working`
//!   session touched within the grace, which rustc creates before its lock
//!   file, for an older mbx compiling a stamped unit without a marker. A unit
//!   without a stamp, from an older mbx, gets the grace on anything rustc
//!   touched recently.

use eyre::{Context, Result};
use mbx_cache_core::CacheDigest;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SESSION_START_GRACE_SECS: u64 = 60;

const RECORD_FILE: &str = "checkout.json";
const RECORD_VERSION: u8 = 1;
const LOCKS_DIR: &str = ".locks";
const REGISTRAR_FILE: &str = "registrar.lock";
/// A unit's state being copied in from another checkout: `<unit>.seeding-<pid>`.
const SEEDING_MARKER: &str = ".seeding-";
/// A unit or checkout renamed aside to be deleted, as managed targets are:
/// `<name>.removing-<pid>-<random>`.
use crate::target::REMOVAL_SUFFIX;

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
    /// Units kept from the budget because they were used within the last
    /// hour.
    pub kept_hot_units: u64,
    pub removed_directories: u64,
    pub removed_bytes: u64,
    pub remaining_directories: u64,
    pub remaining_bytes: u64,
    pub skipped_active_directories: u64,
    pub untracked_directories: u64,
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

/// A unit's use stamp: an empty file beside the unit whose modification time
/// is when the unit was last used, and whose lock says who is using it.
///
/// A compiler holds it shared from before seeding until after its churn
/// record is written; collection takes it exclusively to evict the unit. It
/// is never unlinked, truncated, written to, or replaced while its checkout
/// exists, so a lock on it is a lock on that pathname, and only an accepted
/// use ever changes its modification time.
const USE_EXTENSION: &str = "use";
/// The last backstop verdict for a unit's state: modified when it was
/// measured, holding `a` (accepted) or `r` (rejected).
const BACKSTOP_EXTENSION: &str = "backstop";
const BACKSTOP_ACCEPTED: &[u8] = b"a";
const BACKSTOP_REJECTED: &[u8] = b"r";
/// A compilation with a unit's state is under way:
/// `<unit>/compiling-<pid>-<random>`, an empty file each compilation's shim
/// creates inside the unit when the compilation accepts the state, refreshes
/// right before rustc starts, and removes when it finishes. Its modification
/// time is when the compilation last started or was about to start rustc;
/// nothing else ever changes it. rustc only manages `<unit>/<crate>-<hash>/`,
/// and the marker goes with the unit's tree.
const COMPILING_PREFIX: &str = "compiling-";
/// A cache hit refreshes a unit's stamp, or measures it against the
/// backstop, at most this often.
const USE_STAMP_THROTTLE_SECS: u64 = 60;
/// How often a compiler tries for a unit a collector holds before compiling
/// without its state. Never wait on a collector.
const USE_RETRIES: u32 = 10;
const USE_RETRY_DELAY: Duration = Duration::from_millis(20);
/// The contents of a lease held by a session that knows about use stamps. An
/// older mbx leaves its lease empty.
const LEASE_VERSION: &[u8] = b"2";

/// Whether `root` is a session's checkout, rather than a standalone shim's
/// target-local fallback: decided by its name, which a session gives every
/// checkout it records.
pub(crate) fn is_checkout(root: &Path) -> bool {
    root.file_name().is_some_and(is_checkout_name)
}

pub(crate) fn use_path(unit: &Path) -> PathBuf {
    unit.with_extension(USE_EXTENSION)
}

pub(crate) fn backstop_path(unit: &Path) -> PathBuf {
    unit.with_extension(BACKSTOP_EXTENSION)
}

/// A new compilation's marker inside `unit`, unique to this compilation.
fn new_compiling_path(unit: &Path) -> PathBuf {
    unit.join(format!(
        "{COMPILING_PREFIX}{}-{}",
        std::process::id(),
        crate::util::random_string(8)
    ))
}

fn is_compiling_marker(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with(COMPILING_PREFIX)
}

/// Every compilation marker `unit` has now.
#[cfg(test)]
pub(crate) fn compiling_markers_of(unit: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(unit)
        .map(|listing| {
            listing
                .flatten()
                .filter(|entry| is_compiling_marker(&entry.file_name()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// A compiler's shared hold on one unit's use stamp.
#[derive(Debug)]
pub(crate) struct UseGuard {
    file: std::fs::File,
    unit: PathBuf,
    /// This compilation's own marker, once it was stamped.
    marker: Option<(PathBuf, std::fs::File)>,
}

impl UseGuard {
    /// Record that the unit's state is being used: the compiler has decided
    /// to compile with it. Marks the compilation as under way first, so an
    /// explicit removal can tell a compiler is starting even if its shim
    /// dies; a compilation that cannot be marked must not use the state. The
    /// stamp is refreshed again when the guard drops, so a long compilation
    /// does not end with the stamp it started with.
    pub(crate) fn stamp(&mut self) -> std::io::Result<()> {
        // Written while this shim holds the stamp shared, so retirement,
        // which needs it exclusively, never sees it half made.
        let path = new_compiling_path(&self.unit);
        let marker = std::fs::File::create(&path)?;
        touch_file(&self.file)?;
        self.marker = Some((path, marker));
        Ok(())
    }

    /// Mark the compilation as starting now, right before rustc runs: it may
    /// have waited for the machine longer than the session-start grace since
    /// it was stamped. Does nothing for state that was never stamped.
    pub(crate) fn refresh(&mut self) -> std::io::Result<()> {
        let Some((path, marker)) = &mut self.marker else {
            return Ok(());
        };
        if path.exists() {
            touch_file(marker)?;
        } else {
            *marker = std::fs::File::create(&*path)?;
        }
        touch_file(&self.file)
    }
}

/// Empty a unit's backstop record, if it has one, without replacing it.
fn expire_verdict(path: &Path) -> std::io::Result<()> {
    let Some(file) = open_stamp(path)? else {
        return Ok(());
    };
    file.lock()?;
    file.set_len(0)
}

/// Set a file's modification time to now.
fn touch_file(file: &std::fs::File) -> std::io::Result<()> {
    file.set_modified(SystemTime::now())
}

impl Drop for UseGuard {
    fn drop(&mut self) {
        let Some((marker, _)) = self.marker.take() else {
            return;
        };
        match std::fs::remove_file(&marker) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log::debug!("could not mark the compilation finished: {error}"),
        }
        // The compilation changed the state, so the last backstop verdict no
        // longer describes it: empty it, under its lock, so that the next use
        // measures again and no reader of the old verdict can act on it after.
        if let Err(error) = expire_verdict(&backstop_path(&self.unit)) {
            log::debug!("could not expire the backstop verdict: {error}");
        }
        if let Err(error) = touch_file(&self.file) {
            log::debug!("could not stamp incremental state use: {error}");
        }
    }
}

/// Hold `unit`'s use stamp shared, without stamping it.
///
/// In a session's checkout, which collection and retirement may remove, the
/// stamp is only opened or created under the registrar and while the
/// checkout's record exists, so it cannot appear inside a checkout being
/// retired. A standalone shim's target-local root, which is not named like a
/// checkout, has neither.
///
/// Returns `None` while a collector holds the unit: the caller compiles
/// without state this time.
pub(crate) fn hold_unit(unit: &Path) -> std::io::Result<Option<UseGuard>> {
    for attempt in 0..USE_RETRIES {
        if attempt > 0 {
            std::thread::sleep(USE_RETRY_DELAY);
        }
        match acquire_use(unit)? {
            Acquired::Held(file) => {
                return Ok(Some(UseGuard {
                    file,
                    unit: unit.to_path_buf(),
                    marker: None,
                }));
            }
            Acquired::Retired => return Ok(None),
            Acquired::Busy => {}
        }
    }
    Ok(None)
}

/// Record a cache hit's use of `unit`, if its state is still within the
/// backstop. Throttled, and skipped while a collector holds the unit.
pub(crate) fn note_use(unit: &Path, budget: Option<u64>) {
    if std::fs::metadata(use_path(unit))
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|stamped| within(stamped, USE_STAMP_THROTTLE_SECS))
        || !unit.is_dir()
    {
        return;
    }
    let file = match acquire_use(unit) {
        Ok(Acquired::Held(file)) => file,
        Ok(Acquired::Retired | Acquired::Busy) => return,
        Err(error) => {
            log::debug!("could not record use of {}: {error}", unit.display());
            return;
        }
    };
    if backstop_verdict(unit, budget)
        && let Err(error) = touch_file(&file)
    {
        log::debug!("could not stamp incremental state use: {error}");
    }
}

enum Acquired {
    Held(std::fs::File),
    /// The checkout's record is gone: it is being, or has been, retired.
    Retired,
    /// A collector holds the unit, or Windows would not share the file.
    Busy,
}

fn acquire_use(unit: &Path) -> std::io::Result<Acquired> {
    let (Some(blake3), Some(checkout)) = (unit.parent(), unit.parent().and_then(Path::parent))
    else {
        return Ok(Acquired::Retired);
    };
    let registrar = if is_checkout(checkout) {
        let Some(registrar) = registrar_while_recorded(checkout)? else {
            return Ok(Acquired::Retired);
        };
        Some(registrar)
    } else {
        None
    };
    std::fs::create_dir_all(blake3)?;
    let (file, created) = match open_use(&use_path(unit)) {
        Ok(opened) => opened,
        #[cfg(windows)]
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Ok(Acquired::Busy);
        }
        Err(error) => return Err(error),
    };
    if created {
        // A new stamp must not read as "used now": the unit's state, if any,
        // was last used when its newest session was written.
        file.set_modified(newest_session(unit).unwrap_or(UNIX_EPOCH))?;
    }
    let locked = file.try_lock_shared();
    drop(registrar);
    match locked {
        Ok(()) => Ok(Acquired::Held(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(Acquired::Busy),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

/// Take the registrar of `checkout`'s root, if the checkout still has its
/// record: otherwise it is being, or has been, retired.
fn registrar_while_recorded(checkout: &Path) -> std::io::Result<Option<fslock::LockFile>> {
    let Some(root) = checkout.parent() else {
        return Ok(None);
    };
    let path = registrar_path(root);
    std::fs::create_dir_all(path.parent().expect("registrar has a parent"))?;
    let mut registrar = fslock::LockFile::open(&path)?;
    #[cfg(test)]
    tests::pause("registrar");
    registrar.lock()?;
    Ok(checkout.join(RECORD_FILE).exists().then_some(registrar))
}

/// Open a use stamp, creating it if it does not exist yet, and say whether
/// this call created it. Never truncates.
fn open_use(path: &Path) -> std::io::Result<(std::fs::File, bool)> {
    for _ in 0..2 {
        if let Some(file) = open_stamp(path)? {
            return Ok((file, false));
        }
        match std::fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => return Ok((file, true)),
            // Another shim created it meanwhile.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    open_stamp(path)?.map(|file| (file, false)).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} keeps disappearing", path.display()),
        )
    })
}

/// Open an existing use stamp, never creating one.
fn open_stamp(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    match std::fs::File::options().read(true).write(true).open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Open a record beside a unit, creating it empty if it is missing.
fn open_or_create(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Whether `time` is at most `secs` ago, or in the future.
fn within(time: SystemTime, secs: u64) -> bool {
    SystemTime::now()
        .duration_since(time)
        .map_or(true, |age| age.as_secs() <= secs)
}

/// Whether `unit`'s state is still within the backstop, for a compilation
/// about to use it and for a cache hit that would count as a use of it.
/// Measured at most once per throttle window, and by one shim at a time: the
/// others wait for its verdict rather than walk the state again. Without an
/// aggregate budget there is no backstop, and nothing is recorded.
pub(crate) fn backstop_verdict(unit: &Path, budget: Option<u64>) -> bool {
    use std::io::Read as _;
    if budget.is_none() {
        return true;
    }
    let Ok(mut file) = open_or_create(&backstop_path(unit)) else {
        // Nowhere to record it, as for a unit with no state yet.
        return !exceeds_backstop(unit, budget);
    };
    if file.lock().is_err() {
        return !exceeds_backstop(unit, budget);
    }
    let mut content = Vec::new();
    let fresh = file.read_to_end(&mut content).is_ok()
        && file
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|measured| within(measured, USE_STAMP_THROTTLE_SECS));
    if fresh && content == BACKSTOP_ACCEPTED {
        return true;
    }
    if fresh && content == BACKSTOP_REJECTED {
        return false;
    }
    let rejected = exceeds_backstop(unit, budget);
    let verdict = if rejected {
        BACKSTOP_REJECTED
    } else {
        BACKSTOP_ACCEPTED
    };
    if let Err(error) = write_verdict(&mut file, verdict) {
        log::debug!(
            "could not record the backstop verdict for {}: {error}",
            unit.display()
        );
    }
    !rejected
}

fn write_verdict(file: &mut std::fs::File, verdict: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek as _, Write as _};
    file.seek(std::io::SeekFrom::Start(0))?;
    file.write_all(verdict)?;
    file.set_len(verdict.len() as u64)?;
    file.set_modified(SystemTime::now())
}

/// Whether one unit's state has outgrown the backstop: half the aggregate
/// budget. Oversized state takes the shared compilation path and is left
/// intact for collection rather than rewritten on every edit.
pub(crate) fn exceeds_backstop(unit: &Path, budget: Option<u64>) -> bool {
    let Some(budget) = budget else {
        return false;
    };
    #[cfg(test)]
    tests::count_backstop_walk(unit);
    tree_bytes(unit) > budget / 2
}

/// Inspect all traceable learned incremental checkout state.
pub(crate) fn stats(root: &Path) -> Result<Stats> {
    let mut stats = Stats::default();
    let snapshot = entries(root)?;
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
        if !entry.file_type()?.is_dir() || !is_checkout_name(&entry.file_name()) {
            continue;
        }
        if let Some(record) = read_record(&entry.path().join(RECORD_FILE)) {
            workspaces.push(record.workspace_root);
        }
    }
    Ok(workspaces)
}

/// Units used within this long are kept from budget eviction, though not
/// from expiry, and not when the disk is short: a budget smaller than the
/// state an edit loop is using right now is not met by deleting it after
/// every build.
const HOT_UNIT_SECS: u64 = 3600;

/// What collection is asked to meet, and whether the last hour's units are
/// kept from the budget.
struct Policy {
    now: u64,
    max_age: Option<Duration>,
    max_bytes: Option<u64>,
    hot_floor: bool,
}

impl Policy {
    fn expired(&self, used: u64) -> bool {
        self.max_age
            .is_some_and(|age| self.now.saturating_sub(used) > age.as_secs())
    }

    fn under_budget(&self, remaining: u64) -> bool {
        self.max_bytes.is_none_or(|budget| remaining <= budget)
    }

    /// Decide a unit's fate from its last use: `None` to evict it. Checked on
    /// the scan and again under the unit's locks, by one rule, so the hour
    /// floor and the budget cannot drift apart.
    fn reevaluate(&self, used: u64, live: bool, remaining: u64) -> Option<Eviction> {
        let required = self.expired(used);
        if !required && self.hot_floor && live && self.now.saturating_sub(used) < HOT_UNIT_SECS {
            return Some(Eviction::Hot);
        }
        if !required && self.under_budget(remaining) {
            return Some(Eviction::Skipped);
        }
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Eviction {
    /// Renamed aside. Says whether its tree, its churn record, and its
    /// backstop record were deleted; a tree that was not is finished by a
    /// later collection (see [`finish_deletions`]).
    Evicted {
        tree: bool,
        record: bool,
        backstop: bool,
    },
    /// A compiler starting up, a lease, or a session lock holds it.
    Active,
    /// Used within the last hour, and the disk is not short.
    Hot,
    /// Left alone: no longer needed once read again, already gone, or
    /// unreachable.
    Skipped,
}

/// Collect expired units, then units in least-recently-used order.
/// Deleted checkouts remain donors until their last unit is collected.
///
/// Units go by their use stamp, inside active checkouts too. A unit a
/// compiler holds is never removed, and with `hot_floor` units of live
/// checkouts used within the last hour are kept from the budget; collection
/// on a disk that is running out of space goes without it.
pub(crate) fn collect(
    root: &Path,
    max_bytes: Option<u64>,
    max_age: Option<Duration>,
    dry_run: bool,
    hot_floor: bool,
) -> Result<PruneOutcome> {
    let now = now_secs();
    let Snapshot {
        entries,
        retired,
        mut inodes,
    } = entries(root)?;
    let live = entries
        .iter()
        .map(|entry| {
            entry
                .workspace_root
                .as_ref()
                .is_some_and(|workspace| crate::store::checkout_is_live_on(root, workspace))
        })
        .collect::<Vec<_>>();
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
    if !dry_run {
        // Deletions an earlier collection could not finish.
        finish_deletions(&retired, &mut inodes);
        for entry in &entries {
            finish_deletions(&entry.removing, &mut inodes);
        }
    }
    let mut active = HashSet::new();
    let mut removed = HashSet::new();
    let mut kept_records = HashSet::new();
    let policy = Policy {
        now,
        max_age,
        max_bytes,
        hot_floor,
    };
    let mut units = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.workspace_root.is_some())
        .flat_map(|(index, entry)| entry.units.iter().map(move |unit| (index, unit)))
        .collect::<Vec<_>>();
    units.sort_by_key(|(_, unit)| {
        (
            !policy.expired(unit.last_used),
            unit.last_used,
            &unit.directory,
        )
    });
    for (index, unit) in units {
        if !policy.expired(unit.last_used) && policy.under_budget(inodes.bytes) {
            break;
        }
        let entry = &entries[index];
        // Decided on the scan first, without touching anything; only a
        // candidate is read again under its locks.
        let eviction = match policy.reevaluate(unit.last_used, live[index], inodes.bytes) {
            Some(eviction) => eviction,
            None => evict_unit(
                root,
                entry,
                unit,
                &policy,
                live[index],
                inodes.bytes,
                dry_run,
            )?,
        };
        let (tree, record, backstop) = match eviction {
            Eviction::Evicted {
                tree,
                record,
                backstop,
            } => (tree, record, backstop),
            Eviction::Active => {
                active.insert(index);
                outcome.skipped_active_units += 1;
                continue;
            }
            Eviction::Hot => {
                outcome.kept_hot_units += 1;
                continue;
            }
            Eviction::Skipped => continue,
        };
        // Bytes are released only for what was actually deleted; a tree left
        // aside stays counted until [`finish_deletions`] deletes it.
        if tree {
            inodes.release(&unit.files);
        }
        if record {
            inodes.release(&unit.churn_files);
        } else {
            kept_records.insert(index);
        }
        if backstop {
            inodes.release(&unit.backstop_files);
        }
        remaining[index] -= 1;
        removed.insert(unit.directory.clone());
        outcome.removed_units += 1;
    }
    // Only empty, deleted checkouts lose their bookkeeping. An abandoned
    // checkout with usable units is still a donor, regardless of its path.
    for (index, entry) in entries.iter().enumerate() {
        if entry.workspace_root.is_none()
            || remaining[index] != 0
            || kept_records.contains(&index)
            || live[index]
        {
            continue;
        }
        if dry_run {
            let guard = match deletion_guard(root, &entry.key, false) {
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
            if nested_entries(&entry.directory.join("blake3"))
                .iter()
                .any(|unit| {
                    unit.file_type().is_ok_and(|kind| kind.is_dir())
                        && !removed.contains(&unit.path())
                })
            {
                continue;
            }
            inodes.release(&entry.files);
            outcome.removed_directories += 1;
            continue;
        }
        // A compiler may have added a unit since enumeration; retirement
        // lists the checkout again under the registrar.
        match retire_checkout(root, &entry.key, RetireMode::EmptyShell) {
            Ok(Retired::Removed(_)) => {
                inodes.release(&entry.files);
                outcome.removed_directories += 1;
            }
            Ok(Retired::Aside) => outcome.removed_directories += 1,
            Ok(Retired::Active) => {
                active.insert(index);
            }
            Ok(Retired::Untouched) => {}
            Err(error) => {
                log::debug!("incremental checkout could not be retired: {error}");
            }
        }
    }
    outcome.removed_bytes = outcome.remaining_bytes.saturating_sub(inodes.bytes);
    outcome.remaining_bytes = inodes.bytes;
    outcome.skipped_active_directories = active.len() as u64;
    outcome.remaining_directories -= outcome.removed_directories;
    outcome.remaining_units -= outcome.removed_units;
    if let Some(max_bytes) = max_bytes
        && outcome.remaining_bytes > max_bytes
    {
        log::warn!(
            "learned incremental state still holds {} after collection, over the {} budget; active units, {} units used in the last hour, untracked checkouts, and checkout bookkeeping are kept",
            bytesize::ByteSize::b(outcome.remaining_bytes)
                .display()
                .iec(),
            bytesize::ByteSize::b(max_bytes).display().iec(),
            outcome.kept_hot_units,
        );
    }
    Ok(outcome)
}

/// Evict one unit, unless something uses it or its last use, read again
/// under its locks, no longer calls for it.
///
/// A real run holds the registrar from the lease probe through renaming the
/// unit aside, so neither an older mbx's session nor a whole-checkout delete
/// and recreate can interleave, and holds the unit's use stamp exclusively and
/// every rustc session lock in it through the rename. The stamp itself is
/// never removed. The renamed tree is deleted after everything is released.
fn evict_unit(
    root: &Path,
    entry: &Entry,
    unit: &Unit,
    policy: &Policy,
    live: bool,
    remaining: u64,
    dry_run: bool,
) -> Result<Eviction> {
    if dry_run {
        return preview_eviction(root, entry, unit, policy, live, remaining);
    }
    // seed() holds the donor's checkout lease while linking its sessions.
    let Some(guard) = reserve(root, &entry.key, true) else {
        return Ok(Eviction::Skipped);
    };
    if guard.active_legacy {
        // An older mbx's shims never take use stamps.
        return Ok(Eviction::Active);
    }
    // Only an existing stamp: one is never created in another's place. Opened
    // under the registrar, so the checkout cannot be deleted and recreated
    // between here and the rename.
    let use_file = unit_use_path(&unit.directory);
    let stamp = match open_stamp(&use_file) {
        Ok(stamp) => stamp,
        Err(error) => {
            warn_walk_error(&use_file, &error);
            return Ok(Eviction::Skipped);
        }
    };
    let stamped = match &stamp {
        Some(file) => {
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => return Ok(Eviction::Active),
                Err(std::fs::TryLockError::Error(error)) => {
                    warn_walk_error(&use_file, &error);
                    return Ok(Eviction::Skipped);
                }
            }
            // Read again under the lock: a stamp is only written under a
            // shared lock, so every stamp before this one is visible.
            file.metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
        }
        // A unit without a stamp is protected by its checkout's lease.
        None if guard.active => return Ok(Eviction::Active),
        None => None,
    };
    let Some(session_locks) = session_locks(&unit.directory, false, stamp.is_some())? else {
        return Ok(Eviction::Active);
    };
    // A compilation that finished after the scan can rescue it.
    let used = last_use(&unit.directory, stamped);
    if let Some(eviction) = policy.reevaluate(used, live, remaining) {
        return Ok(eviction);
    }
    #[cfg(test)]
    tests::pause("evict");
    let aside = crate::target::removal_path(&unit.directory);
    match std::fs::rename(&unit.directory, &aside) {
        Ok(()) => {}
        // Another collector got there first, and counts it.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Eviction::Skipped);
        }
        Err(error) => {
            // Windows refuses to rename a tree with open handles inside it.
            log::debug!(
                "incremental unit {} could not be renamed aside: {error}",
                unit.directory.display()
            );
            return Ok(Eviction::Active);
        }
    }
    #[cfg(test)]
    tests::pause("renamed");
    let record = unit
        .churn_record
        .as_deref()
        .is_none_or(|record| remove_sibling(record, "churn record"));
    let backstop = unit
        .backstop
        .as_deref()
        .is_none_or(|backstop| remove_sibling(backstop, "backstop record"));
    drop(session_locks);
    drop(stamp);
    drop(guard);
    Ok(Eviction::Evicted {
        tree: remove_tree(&aside),
        record,
        backstop,
    })
}

/// What [`evict_unit`] would do, without taking any lock a compiler uses.
fn preview_eviction(
    root: &Path,
    entry: &Entry,
    unit: &Unit,
    policy: &Policy,
    live: bool,
    remaining: u64,
) -> Result<Eviction> {
    let Some(guard) = reserve(root, &entry.key, false) else {
        return Ok(Eviction::Skipped);
    };
    if guard.active_legacy || (!unit.stamped && guard.active) {
        return Ok(Eviction::Active);
    }
    drop(guard);
    if let Some(eviction) = policy.reevaluate(unit.last_used, live, remaining) {
        return Ok(eviction);
    }
    if session_locks(&unit.directory, true, unit.stamped)?.is_none() {
        return Ok(Eviction::Active);
    }
    Ok(Eviction::Evicted {
        tree: true,
        record: true,
        backstop: true,
    })
}

/// Reserve a checkout for deletion: see [`deletion_guard`]. A checkout that
/// disappeared or cannot be reserved is left alone.
fn reserve(root: &Path, key: &str, cleanup: bool) -> Option<DeletionGuard> {
    deletion_guard(root, key, cleanup)
        .inspect_err(|error| {
            log::debug!("incremental checkout disappeared or could not be reserved: {error}");
        })
        .ok()
}

/// Remove one of a unit's sibling files, saying whether it is gone.
fn remove_sibling(path: &Path, what: &str) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            log::warn!(
                "could not remove incremental {what} {}: {error}",
                path.display()
            );
            false
        }
    }
}

/// When a unit was last used: its stamp, or rustc's newest session if that
/// is later, falling back to the unit directory's own time.
fn last_use(unit: &Path, stamped: Option<SystemTime>) -> u64 {
    let session = newest_session(unit).or_else(|| {
        std::fs::metadata(unit)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    session
        .into_iter()
        .chain(stamped)
        .max()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |age| age.as_secs())
}

/// Delete trees an earlier collection or retirement renamed aside but could
/// not delete, releasing the files of those now gone.
fn finish_deletions(aside: &[(PathBuf, Vec<FileKey>)], inodes: &mut Inodes) {
    for (directory, files) in aside {
        if remove_tree(directory) {
            inodes.release(files);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    Missing,
    Removed(u64),
    Active,
}

/// How [`retire_checkout`] treats a checkout that still holds state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetireMode {
    /// Collection: only a checkout with no units, staging, or unfinished
    /// removals left goes.
    EmptyShell,
    /// `mbx clean` and `mbx cache remove`: the checkout's state goes too,
    /// unless something is still using it.
    Explicit,
}

#[derive(Debug, PartialEq, Eq)]
enum Retired {
    /// Not a checkout, already gone, or, for an empty shell, not empty.
    Untouched,
    Active,
    Removed(u64),
    /// Renamed aside, but not yet deleted: a later collection finishes it.
    Aside,
}

/// Remove one whole checkout, unless something still uses it.
///
/// Holds the registrar from probing leases through renaming the checkout
/// aside, so no session can register and no shim can start using a unit in
/// between. Holds every use stamp exclusively and every unit's rustc session
/// locks through the rename, so a shim, a seed, or a compiler that outlived
/// its session keeps the checkout. The renamed tree is deleted after
/// everything is released.
///
/// A compiler starting up keeps the checkout too, even one whose shim and
/// session have died: see [`session_locks`].
fn retire_checkout(root: &Path, key: &str, mode: RetireMode) -> Result<Retired> {
    if !is_checkout_name(std::ffi::OsStr::new(key)) {
        return Ok(Retired::Untouched);
    }
    let directory = root.join(key);
    let guard = deletion_guard(root, key, true)?;
    if guard.active {
        return Ok(Retired::Active);
    }
    if !directory.is_dir() {
        return Ok(Retired::Untouched);
    }
    // Never proceed on a partial listing: an entry that cannot be read could
    // be a stamp a shim holds.
    let Some(listing) = list_strictly(&directory.join("blake3")) else {
        return Ok(Retired::Active);
    };
    let contents = unit_contents(&listing);
    if mode == RetireMode::EmptyShell && !contents.is_empty() {
        return Ok(Retired::Untouched);
    }
    let mut stamps = Vec::new();
    for (path, kind) in &listing {
        if path
            .extension()
            .is_none_or(|extension| extension != USE_EXTENSION)
            || !kind.is_file()
        {
            continue;
        }
        let path = path.clone();
        let file = match open_stamp(&path) {
            Ok(Some(file)) => file,
            Ok(None) => continue,
            Err(error) => {
                warn_walk_error(&path, &error);
                return Ok(Retired::Active);
            }
        };
        match file.try_lock() {
            Ok(()) => stamps.push((path, file)),
            Err(std::fs::TryLockError::WouldBlock) => return Ok(Retired::Active),
            Err(std::fs::TryLockError::Error(error)) => {
                warn_walk_error(&path, &error);
                return Ok(Retired::Active);
            }
        }
    }
    #[cfg(test)]
    tests::pause("sessions");
    let mut held = Vec::new();
    let held_stamps = stamps
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<HashSet<_>>();
    for unit in contents.units.iter().chain(&contents.staging) {
        let stamped = held_stamps.contains(&unit_use_path(unit));
        match session_locks(unit, false, stamped)? {
            Some(locks) => held.push(locks),
            None => return Ok(Retired::Active),
        }
    }
    #[cfg(test)]
    tests::pause("retire");
    let aside = crate::target::removal_path(&directory);
    match std::fs::rename(&directory, &aside) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Retired::Untouched);
        }
        Err(error) => {
            // Windows refuses to rename a tree with open handles inside it.
            log::debug!(
                "incremental checkout {} could not be retired: {error}",
                directory.display()
            );
            return Ok(Retired::Active);
        }
    }
    guard.cleanup();
    drop(held);
    drop(stamps);
    drop(guard);
    // Measured once renamed: the tree is no checkout's any more.
    let bytes = tree_bytes(&aside);
    Ok(if remove_tree(&aside) {
        Retired::Removed(bytes)
    } else {
        Retired::Aside
    })
}

/// What a checkout's `blake3/` directory holds, read afresh and in full.
#[derive(Default)]
struct UnitContents {
    units: Vec<PathBuf>,
    staging: Vec<PathBuf>,
    removing: Vec<PathBuf>,
}

impl UnitContents {
    fn is_empty(&self) -> bool {
        self.units.is_empty() && self.staging.is_empty() && self.removing.is_empty()
    }
}

/// Every entry of `directory` and its type, or `None` if any of it could not
/// be read. A directory or entry that vanished meanwhile is simply absent.
fn list_strictly(directory: &Path) -> Option<Vec<(PathBuf, std::fs::FileType)>> {
    let listing = match std::fs::read_dir(directory) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(error) => {
            warn_walk_error(directory, &error);
            return None;
        }
    };
    let mut entries = Vec::new();
    for entry in listing {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warn_walk_error(directory, &error);
                return None;
            }
        };
        match entry.file_type() {
            Ok(kind) => entries.push((entry.path(), kind)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                warn_walk_error(&entry.path(), &error);
                return None;
            }
        }
    }
    Some(entries)
}

/// Sort a strict listing of `blake3/` into units, staging, and removals.
fn unit_contents(listing: &[(PathBuf, std::fs::FileType)]) -> UnitContents {
    let mut contents = UnitContents::default();
    for (path, kind) in listing {
        if !kind.is_dir() {
            continue;
        }
        let path = path.clone();
        if is_removing(&path) {
            contents.removing.push(path);
        } else if is_seeding(&path) {
            contents.staging.push(path);
        } else {
            contents.units.push(path);
        }
    }
    contents
}

fn is_removing(directory: &Path) -> bool {
    directory
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains(REMOVAL_SUFFIX))
}

/// The use stamp that protects a unit directory. Seed staging,
/// `<unit>.seeding-<pid>`, is protected by its destination unit's stamp,
/// which the seeding shim holds.
fn unit_use_path(directory: &Path) -> PathBuf {
    let destination = directory
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split_once(SEEDING_MARKER))
        .map(|(unit, _)| directory.with_file_name(unit));
    use_path(destination.as_deref().unwrap_or(directory))
}

fn remove_tree(directory: &Path) -> bool {
    match std::fs::remove_dir_all(directory) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
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
    files: Vec<FileKey>,
    last_used: u64,
    churn_record: Option<PathBuf>,
    churn_files: Vec<FileKey>,
    /// The last backstop verdict, which goes with the unit. Staging has none
    /// of its own.
    backstop: Option<PathBuf>,
    backstop_files: Vec<FileKey>,
    /// Whether the stamp that protects the unit exists; for staging, its
    /// destination's. A unit without one belongs to an older
    /// mbx, or has not been used since, and is protected by its checkout's
    /// lease.
    stamped: bool,
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

/// A checkout's units, including seed staging, and the units renamed aside
/// whose deletion has not finished, with their files.
fn units(directory: &Path, inodes: &mut Inodes) -> (Vec<Unit>, Vec<(PathBuf, Vec<FileKey>)>) {
    let mut units = Vec::new();
    let mut removing = Vec::new();
    for entry in nested_entries(&directory.join("blake3")) {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        if is_removing(&path) {
            let files = inodes.measure(&path, &HashSet::new());
            removing.push((path, files));
        } else if let Some(unit) = measure_unit(path, inodes) {
            units.push(unit);
        }
    }
    (units, removing)
}

fn measure_unit(directory: PathBuf, inodes: &mut Inodes) -> Option<Unit> {
    // A unit that vanished since it was listed has neither.
    let session = newest_session(&directory).or_else(|| {
        std::fs::metadata(&directory)
            .and_then(|metadata| metadata.modified())
            .ok()
    })?;
    let stamp = std::fs::metadata(unit_use_path(&directory))
        .and_then(|metadata| metadata.modified())
        .ok();
    let last_used = std::iter::once(session)
        .chain(stamp)
        .max()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |age| age.as_secs());
    // A staging name has a dot before seeding; with_extension would otherwise
    // point at the destination unit's churn record.
    let churn_record = (!is_seeding(&directory)).then(|| directory.with_extension("json"));
    let backstop = (!is_seeding(&directory)).then(|| backstop_path(&directory));
    let files = inodes.measure(&directory, &HashSet::new());
    let churn_files = churn_record
        .as_ref()
        .map_or_else(Vec::new, |record| inodes.measure(record, &HashSet::new()));
    let backstop_files = backstop.as_ref().map_or_else(Vec::new, |backstop| {
        inodes.measure(backstop, &HashSet::new())
    });
    Some(Unit {
        files,
        directory,
        last_used,
        churn_record,
        churn_files,
        backstop,
        backstop_files,
        stamped: stamp.is_some(),
    })
}

/// When rustc last wrote a session of `unit`: `<crate>-<hash>/s-...`.
fn newest_session(unit: &Path) -> Option<SystemTime> {
    std::fs::read_dir(unit)
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
}

/// Hold all existing rustc session locks through deletion; a held lock means
/// the unit is in use. Never wait for a compiler while collecting.
///
/// Returns `None` as well for a compiler starting up, decided in the same
/// walk, so nothing can appear between deciding and locking. For a unit with
/// a use stamp, that is a compilation marker or an unfinished `-working`
/// session touched within the session-start grace: a shim refreshes its
/// marker right before rustc starts, and rustc creates its `-working`
/// session before it creates and locks its lock file, which also covers an
/// older mbx compiling a stamped unit without a marker. A finished build
/// leaves neither. A unit without a stamp belongs to an older mbx, which
/// marks nothing, so anything rustc or a seed touched within the grace
/// counts instead.
fn session_locks(
    directory: &Path,
    dry_run: bool,
    stamped: bool,
) -> Result<Option<Vec<fslock::LockFile>>> {
    let recent = |modified: SystemTime| within(modified, SESSION_START_GRACE_SECS);
    // seed() builds <unit>.seeding-<pid> before its atomic rename. Protect a
    // newly started copy even before it has created its first session.
    if !stamped
        && is_seeding(directory)
        && std::fs::metadata(directory)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(recent)
    {
        return Ok(None);
    }
    let mut paths = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let listing = match std::fs::read_dir(&current) {
            Ok(listing) => listing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warn_walk_error(&current, &error);
                return Ok(None);
            }
        };
        for entry in listing {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    warn_walk_error(&current, &error);
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
            let marker =
                stamped && kind.is_file() && current == directory && is_compiling_marker(&name);
            // rustc creates its `-working` session before its lock file. An
            // older mbx compiling a stamped unit leaves no marker.
            let working = kind.is_dir() && is_unfinished_session(&name);
            let name = name.to_string_lossy();
            let session = kind.is_dir() && name.starts_with("s-");
            let lock = kind.is_file() && name.starts_with("s-") && name.ends_with(".lock");
            if marker || working || (!stamped && (session || lock)) {
                let modified = match entry.metadata().and_then(|metadata| metadata.modified()) {
                    Ok(modified) => modified,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        warn_walk_error(&entry.path(), &error);
                        return Ok(None);
                    }
                };
                // Do not win the race against a compiler starting up.
                if recent(modified) {
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

/// Remove learned incremental state for exactly one workspace.
pub(crate) fn remove_workspace(root: &Path, workspace_root: &Path) -> Result<RemoveOutcome> {
    let key = checkout_key(workspace_root);
    let directory = root.join(&key);
    let Some(record) = read_record(&directory.join(RECORD_FILE)) else {
        return Ok(RemoveOutcome::Missing);
    };
    if record.workspace_root != workspace_root {
        return Ok(RemoveOutcome::Missing);
    }
    Ok(match retire_checkout(root, &key, RetireMode::Explicit)? {
        Retired::Removed(bytes) => RemoveOutcome::Removed(bytes),
        // Explicit removal never leaves a checkout for being populated.
        Retired::Untouched => RemoveOutcome::Removed(0),
        Retired::Aside => {
            eyre::bail!(
                "incremental state for {} was set aside but could not be deleted; the next collection removes it",
                workspace_root.display()
            )
        }
        Retired::Active => RemoveOutcome::Active,
    })
}

struct Entry {
    key: String,
    directory: PathBuf,
    units: Vec<Unit>,
    /// Units renamed aside whose deletion has not finished, and their files.
    removing: Vec<(PathBuf, Vec<FileKey>)>,
    files: Vec<FileKey>,
    workspace_root: Option<PathBuf>,
}

struct Snapshot {
    entries: Vec<Entry>,
    /// Checkouts renamed aside for deletion that a retirement could not
    /// finish deleting, and their files.
    retired: Vec<(PathBuf, Vec<FileKey>)>,
    inodes: Inodes,
}

/// A checkout's directory name: the first 16 hex digits of its workspace
/// root's digest. Never a retired tree, `<key>.removing-...`.
fn is_checkout_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.len() == 16
            && name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn is_retired_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.split_once(REMOVAL_SUFFIX)
            .is_some_and(|(key, _)| is_checkout_name(std::ffi::OsStr::new(key)))
    })
}

fn entries(root: &Path) -> Result<Snapshot> {
    let mut snapshot = Snapshot {
        entries: Vec::new(),
        retired: Vec::new(),
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
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || entry.file_name() == LOCKS_DIR {
            continue;
        }
        let directory = entry.path();
        let name = entry.file_name();
        if is_retired_name(&name) {
            let files = snapshot.inodes.measure(&directory, &HashSet::new());
            snapshot.retired.push((directory, files));
            continue;
        }
        // Anything else that is not a checkout is counted, but never treated
        // as one: it is neither collected nor seeded from.
        let record = is_checkout_name(&name)
            .then(|| read_record(&directory.join(RECORD_FILE)))
            .flatten();
        let (units, removing) = units(&directory, &mut snapshot.inodes);
        let mut excluded = units
            .iter()
            .map(|unit| unit.directory.clone())
            .collect::<HashSet<_>>();
        excluded.extend(units.iter().filter_map(|unit| unit.churn_record.clone()));
        excluded.extend(units.iter().filter_map(|unit| unit.backstop.clone()));
        excluded.extend(removing.iter().map(|(directory, _)| directory.clone()));
        let files = snapshot.inodes.measure(&directory, &excluded);
        snapshot.entries.push(Entry {
            key: entry.file_name().to_string_lossy().into_owned(),
            workspace_root: record.map(|record| record.workspace_root),
            files,
            units,
            removing,
            directory,
        });
    }
    Ok(snapshot)
}

fn is_seeding(directory: &Path) -> bool {
    directory
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains(SEEDING_MARKER))
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
    #[cfg(test)]
    tests::pause("registrar");
    registrar.lock()?;
    std::fs::create_dir_all(&locks)?;
    // PIDs repeat across namespaces. Never wait on another session's lease
    // while holding the registrar its cleanup needs.
    let path = locks.join(format!(
        "{}-{}.lease",
        std::process::id(),
        crate::util::random_string(12)
    ));
    {
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?
            .write_all(LEASE_VERSION)?;
    }
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

struct DeletionGuard {
    _registrar: Option<fslock::LockFile>,
    locks: PathBuf,
    /// Some session holds a lease on the checkout.
    active: bool,
    /// One of those sessions is an older mbx, whose shims never take use
    /// stamps: its lease protects every unit of the checkout.
    active_legacy: bool,
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
            active_legacy: false,
            cleanup,
        });
    }
    // These are mbx's own lease files, not rustc session locks: the rule
    // against preview lock acquisition does not apply here. The registrar
    // makes probing safe against a lease being created but not yet locked.
    std::fs::create_dir_all(registrar_path.parent().expect("registrar has a parent"))?;
    let mut registrar = fslock::LockFile::open(&registrar_path)?;
    #[cfg(test)]
    tests::pause("registrar");
    registrar.lock()?;
    let mut active = false;
    let mut active_legacy = false;
    let listing = match std::fs::read_dir(&locks) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DeletionGuard {
                _registrar: cleanup.then_some(registrar),
                locks,
                active,
                active_legacy,
                cleanup,
            });
        }
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
            if lease_is_legacy(std::fs::metadata(entry.path())) {
                active_legacy = true;
            }
        }
    }
    Ok(DeletionGuard {
        _registrar: cleanup.then_some(registrar),
        locks,
        active,
        active_legacy,
        cleanup,
    })
}

/// Whether a held lease belongs to an older mbx, whose shims take no use
/// stamps: its lease file is empty. Read by length rather than contents,
/// since Windows refuses to read a locked range; a lease that cannot be
/// inspected is assumed to be one, so that it protects every unit.
fn lease_is_legacy(metadata: std::io::Result<std::fs::Metadata>) -> bool {
    metadata.map_or(true, |metadata| metadata.len() == 0)
}

fn cleanup_lock_dir(locks: &Path) {
    // Older development builds placed the registrar in each checkout's lock
    // directory. It is safe to remove while the global registrar is held.
    let _ = std::fs::remove_file(locks.join(REGISTRAR_FILE));
    let _ = std::fs::remove_dir(locks);
}

/// Copy the state another checkout used most recently for one unit into
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
/// Donors are tried in the order their unit was last used, by its use stamp
/// or newest session; donating does not count as a use. The donor holds a
/// lease, and its unit's use stamp shared, while it is read, so collection
/// can neither evict the unit nor remove the checkout halfway, and each
/// session it copies is locked the way rustc locks it, so the donor's own
/// compiler cannot remove that either. A session the donor's
/// compiler is still writing, or may be removing, is left out. The copy is
/// made beside its final name and renamed into place, so an interrupted copy
/// leaves no partial state; a donor that cannot be copied gives way to the
/// next.
///
/// Files are hard-linked. rustc writes a session once, into a directory of
/// its own, and links what it reuses into the next one rather than changing
/// it, so sharing them is as safe as rustc's own reuse. A clone would be just
/// as correct, but it is a new file: the donor's pages in the file cache do
/// not carry over, and the first compilation rereads every object and bitcode
/// file in the state. Lock files, which rustc locks rather than reads, are
/// copied, and so is any file that cannot be linked.
pub(crate) fn seed(checkout: &Path, unit: &str) -> Result<Option<PathBuf>> {
    let (Some(root), Some(own)) = (checkout.parent(), checkout.file_name()) else {
        return Ok(None);
    };
    let mut donors = Vec::new();
    for entry in std::fs::read_dir(root)?.flatten() {
        let name = entry.file_name();
        if name == own || !is_checkout_name(&name) || !entry.path().join(unit).is_dir() {
            continue;
        }
        if let Some(record) = read_record(&entry.path().join(RECORD_FILE)) {
            let source = entry.path().join(unit);
            let stamped = std::fs::metadata(use_path(&source))
                .and_then(|metadata| metadata.modified())
                .ok();
            let used = last_use(&source, stamped);
            donors.push(((used, record.updated_secs), record.workspace_root, name));
        }
    }
    donors.sort_by_key(|donor| std::cmp::Reverse(donor.0));
    let destination = checkout.join(unit);
    let mut failure = None;
    for (_, workspace_root, name) in donors {
        let Some(key) = name.to_str() else {
            continue;
        };
        let _lease = acquire_lease(root, key)?;
        let source = root.join(key).join(unit);
        // Never create a stamp in another checkout. Without one, the unit
        // predates stamps and the lease alone protects it.
        let _stamp = match std::fs::File::open(use_path(&source)) {
            Ok(stamp) => match stamp.try_lock_shared() {
                Ok(()) => Some(stamp),
                // A collector is evicting the donor's unit.
                Err(std::fs::TryLockError::WouldBlock) => continue,
                Err(std::fs::TryLockError::Error(error)) => {
                    warn_walk_error(&use_path(&source), &error);
                    continue;
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                warn_walk_error(&use_path(&source), &error);
                continue;
            }
        };
        if !holds_finished_session(&source) {
            continue;
        }
        #[cfg(test)]
        tests::pause("seed");
        let mut staging = destination.clone().into_os_string();
        staging.push(format!("{SEEDING_MARKER}{}", std::process::id()));
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
        } else if !is_compiling_marker(&entry.file_name()) {
            // A donor's compilation marker is its own, never the recipient's.
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

/// Hard-link a state file, or copy it: a lock file, which rustc locks rather
/// than reads, or one that cannot be linked.
fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let lock = from
        .extension()
        .is_some_and(|extension| extension == "lock");
    if lock || std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to)?;
    }
    Ok(())
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
// multiple session paths in that owner hard-link the same immutable file.
#[derive(Clone, PartialEq, Eq, Hash)]
enum FileKey {
    Object(u64, u64),
    #[cfg(not(unix))]
    Path(PathBuf),
}

#[cfg(unix)]
fn file_key(_path: &Path, metadata: &std::fs::Metadata) -> FileKey {
    use std::os::unix::fs::MetadataExt as _;
    FileKey::Object(metadata.dev(), metadata.ino())
}

#[cfg(windows)]
fn file_key(path: &Path, _metadata: &std::fs::Metadata) -> FileKey {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    if let Ok(file) = std::fs::File::open(path) {
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // The handle stays open and the output points to a correctly sized buffer.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } != 0 {
            let info = unsafe { info.assume_init() };
            return FileKey::Object(
                u64::from(info.dwVolumeSerialNumber),
                (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            );
        }
    }
    FileKey::Path(path.to_path_buf())
}

#[cfg(not(any(unix, windows)))]
fn file_key(path: &Path, _metadata: &std::fs::Metadata) -> FileKey {
    FileKey::Path(path.to_path_buf())
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
            *references -= 1;
            if *references == 0 {
                self.bytes = self.bytes.saturating_sub(*bytes);
            }
        }
    }
}

fn tree_bytes(root: &Path) -> u64 {
    let mut inodes = Inodes::default();
    inodes.measure(root, &HashSet::new());
    inodes.bytes
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Collection as `mbx gc` runs it, keeping the last hour's units.
    fn collect(
        root: &Path,
        max_bytes: Option<u64>,
        max_age: Option<Duration>,
        dry_run: bool,
    ) -> Result<PruneOutcome> {
        super::collect(root, max_bytes, max_age, dry_run, true)
    }

    pub(crate) fn within_throttled(time: SystemTime) -> bool {
        within(time, USE_STAMP_THROTTLE_SECS)
    }
    use std::sync::Mutex;

    type Pauses = HashMap<&'static str, Box<dyn FnOnce()>>;

    thread_local! {
        static PAUSES: std::cell::RefCell<Pauses> = std::cell::RefCell::new(HashMap::new());
    }

    /// Run the hook set for `point` on this thread, once.
    pub(super) fn pause(point: &'static str) {
        let hook = PAUSES.with(|pauses| pauses.borrow_mut().remove(point));
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Run `hook` the next time this thread reaches `point`.
    fn on_pause(point: &'static str, hook: impl FnOnce() + 'static) {
        PAUSES.with(|pauses| pauses.borrow_mut().insert(point, Box::new(hook)));
    }

    static BACKSTOP_WALKS: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

    pub(super) fn count_backstop_walk(unit: &Path) {
        let mut walks = BACKSTOP_WALKS.lock().unwrap();
        *walks
            .get_or_insert_default()
            .entry(unit.to_path_buf())
            .or_default() += 1;
    }

    fn backstop_walks(unit: &Path) -> usize {
        BACKSTOP_WALKS
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|walks| walks.get(unit).copied())
            .unwrap_or_default()
    }

    pub(crate) fn modified(path: &Path) -> SystemTime {
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    pub(crate) fn set_modified(path: &Path, time: SystemTime) {
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(time)).unwrap();
    }

    fn ago(secs: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(secs)
    }

    /// Where a contender started by [`contend_at`] reports its result.
    type Contender<T> = std::rc::Rc<std::cell::RefCell<Option<std::sync::mpsc::Receiver<T>>>>;

    /// When this thread next reaches `point`, holding the registrar, start
    /// `contender` on another thread, wait until it is about to take the
    /// registrar, and check that it cannot finish while this thread holds it.
    fn contend_at<T: Send + 'static>(
        point: &'static str,
        contender: impl FnOnce() -> T + Send + 'static,
    ) -> Contender<T> {
        let slot: Contender<T> = std::rc::Rc::default();
        let filled = slot.clone();
        on_pause(point, move || {
            let (waiting, registrar) = std::sync::mpsc::channel();
            let (sent, result) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                on_pause("registrar", move || {
                    let _ = waiting.send(());
                });
                let _ = sent.send(contender());
            });
            registrar
                .recv_timeout(Duration::from_secs(5))
                .expect("the contender reaches the registrar");
            assert!(
                matches!(
                    result.recv_timeout(Duration::from_millis(100)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ),
                "the contender must wait for the registrar"
            );
            *filled.borrow_mut() = Some(result);
        });
        slot
    }

    /// The result of a contender started by [`contend_at`].
    fn contended<T>(contender: &Contender<T>) -> T {
        contender
            .borrow_mut()
            .take()
            .expect("the pause point was reached")
            .recv_timeout(Duration::from_secs(5))
            .expect("the contender finished")
    }

    /// Wait until nothing holds a lock on `path` any more.
    ///
    /// Dropping a lock does not release it at once while another test's
    /// thread is starting a process: the forked child shares the descriptor
    /// until it execs, and a flock belongs to the shared open file.
    pub(crate) fn wait_until_unlocked(path: &Path) {
        let file = std::fs::File::open(path).unwrap();
        assert!(
            lock_when_released(&file),
            "{} is still locked",
            path.display()
        );
    }

    /// Lock `file` exclusively once whatever held it lets go, waiting up to
    /// 2 s; see [`wait_until_unlocked`].
    pub(crate) fn lock_when_released(file: &std::fs::File) -> bool {
        (0..200).any(|_| {
            let locked = file.try_lock().is_ok();
            if !locked {
                std::thread::sleep(Duration::from_millis(10));
            }
            locked
        })
    }

    /// Makes a directory writable again when dropped, after a test took its
    /// permissions away.
    #[cfg(unix)]
    pub(crate) struct Restore(pub(crate) PathBuf);

    #[cfg(unix)]
    impl Drop for Restore {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    /// Hold `unit`'s use stamp exclusively, as a collector evicting it does.
    fn hold_exclusively(unit: &Path) -> std::fs::File {
        let (file, _) = open_use(&use_path(unit)).unwrap();
        file.try_lock().unwrap();
        file
    }

    #[test]
    fn a_new_use_stamp_starts_at_the_units_last_use() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let used = now_secs() - 7200;
        let directory = unit(&state.directory, "unit", used);
        let guard = hold_unit(&directory).unwrap().unwrap();
        assert_eq!(
            modified(&use_path(&directory)),
            UNIX_EPOCH + Duration::from_secs(used)
        );
        drop(guard);
        assert_eq!(
            modified(&use_path(&directory)),
            UNIX_EPOCH + Duration::from_secs(used),
            "an unstamped guard leaves the stamp alone"
        );

        let fresh = state.directory.join("blake3/fresh");
        drop(hold_unit(&fresh).unwrap().unwrap());
        assert_eq!(modified(&use_path(&fresh)), UNIX_EPOCH);
        assert!(!fresh.exists(), "holding a unit does not create its state");
    }

    #[test]
    fn stamping_marks_the_use_and_the_guard_restamps_on_drop() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit(&state.directory, "unit", 0);
        let stamp = use_path(&directory);
        let mut guard = hold_unit(&directory).unwrap().unwrap();
        guard.stamp().unwrap();
        assert!(within_throttled(modified(&stamp)));
        assert!(
            !backstop_path(&directory).exists(),
            "the verdict is the backstop check's to record"
        );
        std::fs::write(backstop_path(&directory), BACKSTOP_ACCEPTED).unwrap();
        set_modified(&stamp, ago(7200));
        drop(guard);
        assert!(within_throttled(modified(&stamp)));
        assert_eq!(std::fs::metadata(&stamp).unwrap().len(), 0);
        assert!(
            std::fs::read(backstop_path(&directory)).unwrap().is_empty(),
            "the compilation changed the state its verdict described"
        );
    }

    #[test]
    fn a_standalone_root_holds_units_without_a_checkout_record() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("blake3/unit");
        assert!(hold_unit(&directory).unwrap().is_some());
        assert!(use_path(&directory).is_file());

        let managed = tempfile::tempdir().unwrap();
        let checkout = managed.path().join("0123456789abcdef");
        let retired = checkout.join("blake3/unit");
        assert!(hold_unit(&retired).unwrap().is_none());
        assert!(
            !checkout.exists(),
            "nothing is created in a checkout without a record"
        );
    }

    #[test]
    fn a_unit_being_collected_is_not_held() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit(&state.directory, "unit", 0);
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (locked, wait) = std::sync::mpsc::channel();
        let collector = {
            let directory = directory.clone();
            std::thread::spawn(move || {
                let file = hold_exclusively(&directory);
                locked.send(()).unwrap();
                let _ = released.recv();
                drop(file);
            })
        };
        wait.recv().unwrap();
        assert!(hold_unit(&directory).unwrap().is_none());
        drop(release);
        collector.join().unwrap();
        wait_until_unlocked(&use_path(&directory));
        assert!(hold_unit(&directory).unwrap().is_some());
    }

    #[test]
    fn cache_hits_are_throttled_need_state_and_skip_a_unit_being_collected() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let missing = state.directory.join("blake3/missing");
        note_use(&missing, None);
        assert!(!use_path(&missing).exists());

        let directory = unit(&state.directory, "unit", 0);
        let stamp = use_path(&directory);
        note_use(&directory, None);
        assert!(within_throttled(modified(&stamp)));
        assert!(
            !backstop_path(&directory).exists(),
            "no budget, no backstop verdict"
        );

        let recent = ago(30);
        set_modified(&stamp, recent);
        note_use(&directory, None);
        assert_eq!(modified(&stamp), recent, "throttled");

        let old = ago(7200);
        set_modified(&stamp, old);
        let collector = hold_exclusively(&directory);
        note_use(&directory, None);
        assert_eq!(modified(&stamp), old);
        drop(collector);
        wait_until_unlocked(&use_path(&directory));
        note_use(&directory, None);
        assert!(within_throttled(modified(&stamp)));
    }

    #[test]
    fn rejected_cache_hits_never_refresh_the_stamp() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit(&state.directory, "unit", 0);
        drop(hold_unit(&directory).unwrap());
        let stamp = use_path(&directory);
        let backstop = backstop_path(&directory);
        for window in 0..3 {
            note_use(&directory, Some(32));
            assert_eq!(modified(&stamp), UNIX_EPOCH, "window {window}");
            assert_eq!(std::fs::read(&backstop).unwrap(), BACKSTOP_REJECTED);
            set_modified(&backstop, ago(USE_STAMP_THROTTLE_SECS + 1));
        }
        assert_eq!(backstop_walks(&directory), 3);
        drop(state.lease);
        wait_until_unlocked(&stamp);
        let outcome = collect(cache.path(), None, Some(Duration::from_secs(60)), false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert!(!directory.exists());
    }

    #[test]
    fn simultaneous_rejected_hits_measure_once() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let directory = unit(&state.directory, "unit", 0);
        drop(hold_unit(&directory).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let hits = (0..8)
            .map(|_| {
                let directory = directory.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    note_use(&directory, Some(32));
                })
            })
            .collect::<Vec<_>>();
        for hit in hits {
            hit.join().unwrap();
        }
        assert_eq!(backstop_walks(&directory), 1);
        assert_eq!(modified(&use_path(&directory)), UNIX_EPOCH);
    }

    #[test]
    fn a_missing_or_interrupted_verdict_is_measured() {
        for interrupted in [false, true] {
            let cache = tempfile::tempdir().unwrap();
            let workspace = tempfile::tempdir().unwrap();
            let state = touch(cache.path(), workspace.path()).unwrap();
            let directory = unit(&state.directory, "unit", 0);
            drop(hold_unit(&directory).unwrap());
            if interrupted {
                std::fs::write(backstop_path(&directory), b"").unwrap();
            }
            note_use(&directory, Some(32));
            assert_eq!(backstop_walks(&directory), 1);
            assert_eq!(modified(&use_path(&directory)), UNIX_EPOCH);
            assert_eq!(
                std::fs::read(backstop_path(&directory)).unwrap(),
                BACKSTOP_REJECTED
            );
        }
    }

    #[test]
    fn leases_carry_their_version_and_empty_ones_read_as_legacy() {
        let cache = tempfile::tempdir().unwrap();
        let key = "0123456789abcdef";
        let lease = acquire_lease(cache.path(), key).unwrap();
        // Only the length is portable while it is locked: Windows refuses to
        // read a locked range, even through another handle in the same
        // process, and unlocking truncates the file.
        assert_eq!(
            std::fs::metadata(&lease.path).unwrap().len(),
            LEASE_VERSION.len() as u64
        );
        #[cfg(unix)]
        assert_eq!(std::fs::read(&lease.path).unwrap(), LEASE_VERSION);
        let guard = deletion_guard(cache.path(), key, false).unwrap();
        assert!(guard.active);
        assert!(!guard.active_legacy);
        drop(guard);

        let legacy = lock_dir(cache.path(), key).join("1-legacy.lease");
        std::fs::write(&legacy, b"").unwrap();
        let mut held = fslock::LockFile::open(&legacy).unwrap();
        assert!(held.try_lock().unwrap());
        let guard = deletion_guard(cache.path(), key, false).unwrap();
        assert!(guard.active_legacy);
        drop(guard);
        drop(held);
        drop(lease);
    }

    #[test]
    fn retired_trees_are_counted_until_finished_and_never_treated_as_checkouts() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let live = checkout_with_sessions(cache.path(), "live", 10, unit, &["s-1-a-finished"]);
        let mut name = live.file_name().unwrap().to_os_string();
        name.push(".removing-1-abc");
        let retired = cache.path().join(name);
        std::fs::create_dir_all(retired.join(unit).join("widget-0abc/s-2-b-finished")).unwrap();
        std::fs::write(retired.join("blake3/payload"), vec![0; 100]).unwrap();
        std::fs::write(
            retired.join(RECORD_FILE),
            std::fs::read(live.join(RECORD_FILE)).unwrap(),
        )
        .unwrap();
        std::fs::remove_dir_all(live.join(unit)).unwrap();

        let stats = stats(cache.path()).unwrap();
        assert_eq!(stats.directories, 1);
        assert!(stats.bytes >= 100);
        assert_eq!(workspaces(cache.path()).unwrap().len(), 1);
        let own = checkout_with_sessions(cache.path(), "own", 20, unit, &[]);
        assert_eq!(seed(&own, unit).unwrap(), None, "never a donor");

        let preview = collect(cache.path(), None, None, true).unwrap();
        assert!(preview.remaining_bytes >= 100);
        assert!(retired.exists());
        let outcome = collect(cache.path(), None, None, false).unwrap();
        assert!(outcome.removed_bytes >= 100);
        assert!(!retired.exists());
    }

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
        let kept = collect(cache.path(), None, None, false).unwrap();
        assert_eq!(
            kept.removed_directories, 0,
            "a new checkout may seed from it"
        );
        assert!(directory.exists());

        // Expiry is measured in whole seconds.
        std::thread::sleep(Duration::from_millis(1100));
        let outcome = collect(cache.path(), None, Some(Duration::ZERO), false).unwrap();
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
        let outcome = collect(cache.path(), None, None, false).unwrap();

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
        let outcome = collect(cache.path(), None, Some(Duration::ZERO), true).unwrap();

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

        let outcome = collect(cache.path(), Some(0), Some(Duration::ZERO), false).unwrap();
        assert_eq!(outcome.removed_directories, 0);
        assert_eq!(outcome.skipped_active_directories, 1);
        assert_eq!(
            remove_workspace(cache.path(), &checkout).unwrap(),
            RemoveOutcome::Active
        );
        assert!(state.directory.exists());
    }

    /// A unit of `checkout` whose one session holds 32 bytes and was last
    /// written at `used`.
    pub(crate) fn unit(checkout: &Path, name: &str, used: u64) -> PathBuf {
        let unit = checkout.join("blake3").join(name);
        let session = unit.join("crate-id/s-session");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(session.join("dep-graph.bin"), vec![0; 32]).unwrap();
        set_modified(&session, UNIX_EPOCH + Duration::from_secs(used));
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
        let outcome = collect(cache.path(), Some(before.bytes - 64), None, false).unwrap();
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
        let preview = collect(cache.path(), Some(0), None, true).unwrap();
        assert_eq!(preview.removed_bytes, 32 + 5);
        assert!(record.exists());
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
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
        assert!(session_locks(&vanished, false, false).unwrap().is_some());
        let surviving = unit(&state.directory, "surviving", 0);
        std::fs::remove_dir_all(surviving.join("crate-id/s-session")).unwrap();
        // Without sessions, the unit's own time is its last use.
        set_modified(&surviving, UNIX_EPOCH);
        assert!(session_locks(&surviving, false, false).unwrap().is_some());
        drop(state.lease);
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
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
        let outcome = collect(cache.path(), None, Some(Duration::from_secs(60)), false).unwrap();
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
        let outcome = collect(cache.path(), None, Some(Duration::from_secs(60)), false).unwrap();
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
        let preview = collect(cache.path(), Some(0), None, true).unwrap();
        assert_eq!(preview.removed_units, 2);
        assert_eq!(preview.skipped_active_units, 0);
        assert_eq!(preview.skipped_active_directories, 0);
        assert!(inactive.exists());
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
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
                let outcome = super::collect(cache.path(), Some(0), None, dry_run, false).unwrap();
                assert_eq!(outcome.skipped_active_units, 1);
                assert_eq!(outcome.removed_units, 0);
                assert!(directory.exists());
            }
        }
    }

    #[test]
    fn active_checkout_lease_protects_all_units() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        unit(&state.directory, "first", 0);
        unit(&state.directory, "second", 1);
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
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
        let kept = collect(cache.path(), None, None, false).unwrap();
        assert_eq!(kept.removed_units, 0);
        assert_eq!(kept.removed_directories, 0);
        assert!(state.directory.exists());
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
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
        let within_budget = collect(cache.path(), Some(before.bytes + 16), None, false).unwrap();
        assert_eq!(within_budget.removed_units, 0);
        assert_eq!(within_budget.remaining_bytes, before.bytes);

        let one_removed =
            collect(cache.path(), None, Some(Duration::from_secs(60)), false).unwrap();
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
        let last_removed = collect(cache.path(), Some(0), None, false).unwrap();
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
        let removed = collect(cache.path(), Some(0), None, false).unwrap();
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
        let outcome = collect(cache.path(), Some(before.bytes - 32), None, false).unwrap();
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
            let outcome = collect(cache.path(), Some(0), None, dry_run).unwrap();
            assert_eq!(outcome.removed_units, 0);
            assert_eq!(outcome.skipped_active_units, 1);
            assert_eq!(outcome.skipped_active_directories, 1);
            assert!(donor.exists());
        }
        drop(seeding);
        assert_eq!(
            collect(cache.path(), Some(0), None, false)
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
        // Without the hour floor, which would keep it anyway.
        for dry_run in [true, false] {
            let outcome = super::collect(cache.path(), Some(0), None, dry_run, false).unwrap();
            assert_eq!(outcome.removed_units, 0);
            assert_eq!(outcome.skipped_active_units, 1);
            assert!(staging.exists());
        }
        filetime::set_file_mtime(&staging, filetime::FileTime::from_unix_time(0, 0)).unwrap();
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
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
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.untracked_directories, 1);
        assert_eq!(outcome.remaining_bytes, 64);
        assert_eq!(outcome.remaining_units, 1);
        assert!(untracked_unit.exists());
        assert!(directory.exists());
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

    /// The checkout directory of the workspace `/work/<label>`.
    fn checkout_dir(cache: &Path, label: &str) -> PathBuf {
        cache.join(checkout_key(&PathBuf::from(format!("/work/{label}"))))
    }

    /// The checkout of `/work/<label>`, last used at `updated_secs`, holding
    /// the given rustc session directories for one unit.
    fn checkout_with_sessions(
        cache: &Path,
        label: &str,
        updated_secs: u64,
        unit: &str,
        sessions: &[&str],
    ) -> PathBuf {
        let directory = checkout_dir(cache, label);
        for session in sessions {
            let session = directory.join(unit).join("widget-0abc").join(session);
            std::fs::create_dir_all(&session).unwrap();
            std::fs::write(session.join("dep-graph.bin"), label).unwrap();
            let lock = session_lock_name(session.file_name().unwrap()).unwrap();
            std::fs::write(session.with_file_name(lock), b"").unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        let record = CheckoutRecord {
            version: RECORD_VERSION,
            workspace_root: PathBuf::from(format!("/work/{label}")),
            updated_secs,
        };
        std::fs::write(
            directory.join(RECORD_FILE),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        directory
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
            let donor = checkout_dir(cache.path(), "newer")
                .join(unit)
                .join("widget-0abc");
            let inode = |path: &Path| std::fs::metadata(path).unwrap().ino();
            assert_eq!(
                inode(&copied.join("s-2-b-finished/dep-graph.bin")),
                inode(&donor.join("s-2-b-finished/dep-graph.bin")),
                "state files are shared, not copied"
            );
            assert_ne!(
                inode(&copied.join("s-2-b.lock")),
                inode(&donor.join("s-2-b.lock")),
                "lock files are not shared"
            );
        }
        assert_eq!(
            std::fs::read_dir(own.join("blake3")).unwrap().count(),
            1,
            "no staging directory is left behind"
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
        let donor = checkout_dir(cache.path(), "donor");
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

    /// Give `unit` a use stamp last used at `used`.
    pub(crate) fn stamp(unit: &Path, used: u64) -> PathBuf {
        let path = use_path(unit);
        drop(open_use(&path).unwrap());
        set_modified(&path, UNIX_EPOCH + Duration::from_secs(used));
        path
    }

    /// A live checkout without a session lease, as a session leaves it.
    pub(crate) fn idle_checkout(cache: &Path, workspace: &Path) -> PathBuf {
        let state = touch(cache, workspace).unwrap();
        drop(state.lease);
        state.directory
    }

    fn policy(max_bytes: Option<u64>, hot_floor: bool) -> Policy {
        Policy {
            now: now_secs(),
            max_age: None,
            max_bytes,
            hot_floor,
        }
    }

    fn only_entry(cache: &Path) -> Entry {
        let mut snapshot = entries(cache).unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        snapshot.entries.pop().unwrap()
    }

    fn removing_dirs(checkout: &Path) -> Vec<PathBuf> {
        nested_entries(&checkout.join("blake3"))
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| is_removing(path))
            .collect()
    }

    #[test]
    fn a_stamp_beats_older_sessions_and_falls_back_to_them() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 1000);
        let measured = |cache: &Path| only_entry(cache).units[0].last_used;
        assert_eq!(measured(cache.path()), 1000, "no stamp yet");
        stamp(&directory, 5000);
        assert_eq!(measured(cache.path()), 5000);
        stamp(&directory, 500);
        assert_eq!(measured(cache.path()), 1000, "a later session wins");
    }

    #[test]
    fn a_held_stamp_keeps_its_unit_while_a_preview_lists_it() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let compiler = hold_unit(&directory).unwrap().unwrap();
        let preview = collect(cache.path(), Some(0), None, true).unwrap();
        assert_eq!(preview.removed_units, 1);
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.skipped_active_units, 1);
        assert_eq!(outcome.skipped_active_directories, 1);
        assert!(directory.exists());
        drop(compiler);
        wait_until_unlocked(&use_path(&directory));
        assert_eq!(
            collect(cache.path(), Some(0), None, false)
                .unwrap()
                .removed_units,
            1
        );
    }

    #[test]
    fn a_sessions_lease_protects_only_units_without_a_stamp() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = touch(cache.path(), workspace.path()).unwrap();
        let stamped = unit(&state.directory, "stamped", 0);
        stamp(&stamped, 0);
        let legacy = unit(&state.directory, "legacy", 0);
        for dry_run in [true, false] {
            let outcome = collect(cache.path(), Some(0), None, dry_run).unwrap();
            assert_eq!(outcome.removed_units, 1, "dry run {dry_run}");
            assert_eq!(outcome.skipped_active_units, 1);
        }
        assert!(!stamped.exists());
        assert!(legacy.exists());
        assert!(use_path(&stamped).exists(), "the stamp is permanent");
    }

    #[test]
    fn an_older_mbx_session_protects_every_unit() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let key = checkout.file_name().unwrap().to_str().unwrap().to_owned();
        let lease = lock_dir(cache.path(), &key).join("1-old.lease");
        std::fs::create_dir_all(lease.parent().unwrap()).unwrap();
        std::fs::write(&lease, b"").unwrap();
        let mut held = fslock::LockFile::open(&lease).unwrap();
        assert!(held.try_lock().unwrap());
        for dry_run in [true, false] {
            let outcome = collect(cache.path(), Some(0), None, dry_run).unwrap();
            assert_eq!(outcome.removed_units, 0);
            assert_eq!(outcome.skipped_active_units, 1);
        }
        assert!(directory.exists());
    }

    #[test]
    fn eviction_keeps_the_stamp_and_removes_the_unit_and_its_records() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        let stamp = stamp(&directory, 0);
        std::fs::write(directory.with_extension("json"), b"churn").unwrap();
        std::fs::write(backstop_path(&directory), BACKSTOP_ACCEPTED).unwrap();
        let before = stats(cache.path()).unwrap();
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.removed_bytes, 32 + 5 + 1);
        assert_eq!(outcome.remaining_bytes, before.bytes - (32 + 5 + 1));
        assert!(!directory.exists());
        assert!(!directory.with_extension("json").exists());
        assert!(!backstop_path(&directory).exists());
        assert!(stamp.exists());
        assert!(removing_dirs(&checkout).is_empty());
    }

    #[test]
    fn a_compiler_waiting_on_an_evicting_unit_finds_it_gone_under_the_same_stamp() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        let stamp = stamp(&directory, 0);
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt as _;
            std::fs::metadata(&stamp).unwrap().ino()
        };
        let compiler_directory = directory.clone();
        let compiler = contend_at("evict", move || {
            let held = hold_unit(&compiler_directory).unwrap();
            (held.is_some(), compiler_directory.exists())
        });
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        let (held, existed) = contended(&compiler);
        assert!(
            held,
            "the compiler gets the unit once the collector is done"
        );
        assert!(!existed, "and finds no state left to compile against");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(std::fs::metadata(&stamp).unwrap().ino(), inode);
        }
    }

    #[test]
    fn racing_collectors_remove_a_unit_once() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let collectors = (0..2)
            .map(|_| {
                let root = cache.path().to_path_buf();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    collect(&root, Some(0), None, false).unwrap().removed_units
                })
            })
            .collect::<Vec<_>>();
        let removed = collectors
            .into_iter()
            .map(|collector| collector.join().unwrap())
            .sum::<u64>();
        assert_eq!(removed, 1);
        assert!(!directory.exists());
    }

    #[test]
    fn a_stale_scan_never_touches_a_unit_recreated_since() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let stale = only_entry(cache.path());
        assert_eq!(
            collect(cache.path(), Some(0), None, false)
                .unwrap()
                .removed_units,
            1
        );
        // The collector's own lock on the stamp may linger for a moment.
        wait_until_unlocked(&use_path(&directory));
        let unit = &stale.units[0];
        assert_eq!(
            evict_unit(
                cache.path(),
                &stale,
                unit,
                &policy(Some(0), false),
                true,
                u64::MAX,
                false
            )
            .unwrap(),
            Eviction::Skipped,
            "the loser of a race does not count the unit again"
        );
        // A compiler recreates the unit.
        let mut compiler = hold_unit(&directory).unwrap().unwrap();
        std::fs::create_dir_all(directory.join("crate-id/s-new")).unwrap();
        compiler.stamp().unwrap();
        drop(compiler);
        wait_until_unlocked(&use_path(&directory));
        assert_eq!(
            evict_unit(
                cache.path(),
                &stale,
                unit,
                &policy(Some(0), true),
                true,
                u64::MAX,
                false
            )
            .unwrap(),
            Eviction::Hot,
            "judged by its new use, not the stale scan"
        );
        assert!(directory.join("crate-id/s-new").exists());
    }

    #[test]
    fn a_use_after_the_scan_keeps_the_unit() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let stamped = unit(&checkout, "stamped", 0);
        stamp(&stamped, 0);
        let legacy = unit(&checkout, "legacy", 0);
        let scanned = only_entry(cache.path());
        stamp(&stamped, now_secs() - 120);
        filetime::set_file_mtime(
            legacy.join("crate-id/s-session"),
            filetime::FileTime::from_unix_time(now_secs() as i64 - 120, 0),
        )
        .unwrap();
        for unit in &scanned.units {
            assert_eq!(
                evict_unit(
                    cache.path(),
                    &scanned,
                    unit,
                    &policy(Some(0), true),
                    true,
                    u64::MAX,
                    false
                )
                .unwrap(),
                Eviction::Hot,
                "{}",
                unit.directory.display()
            );
        }
        assert!(stamped.exists());
        assert!(legacy.exists());
    }

    #[test]
    fn a_compiler_whose_shim_died_keeps_its_unit_by_its_session_lock() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let lock = directory.join("crate-id/s-session.lock");
        std::fs::write(&lock, b"").unwrap();
        set_modified(&lock, UNIX_EPOCH);
        let mut rustc = fslock::LockFile::open(&lock).unwrap();
        assert!(rustc.try_lock().unwrap());
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.skipped_active_units, 1);
        assert!(directory.exists());
    }

    #[test]
    fn an_older_session_starting_mid_eviction_finds_the_unit_gone_not_half_removed() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let key = checkout.file_name().unwrap().to_str().unwrap().to_owned();
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let (root, session_directory) = (cache.path().to_path_buf(), directory.clone());
        let session = contend_at("evict", move || {
            let lease = acquire_lease(&root, &key).unwrap();
            let existed = session_directory.exists();
            drop(lease);
            existed
        });
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert!(
            !contended(&session),
            "the session registered only after the unit was renamed away"
        );
    }

    #[test]
    fn a_checkout_cannot_be_removed_and_recreated_under_an_eviction() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let (root, workspace_root) = (cache.path().to_path_buf(), workspace.path().to_path_buf());
        let remover = contend_at("evict", move || {
            let removed = remove_workspace(&root, &workspace_root).unwrap();
            let recreated = touch(&root, &workspace_root).unwrap();
            (removed, recreated.directory.clone())
        });
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        let (removed, recreated) = contended(&remover);
        assert!(matches!(removed, RemoveOutcome::Removed(_)));
        assert_eq!(recreated, checkout);
        assert!(!directory.exists());
        assert!(removing_dirs(&checkout).is_empty());
    }

    #[test]
    fn staging_is_protected_by_its_destinations_stamp_and_an_older_mbx_lease() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let key = checkout.file_name().unwrap().to_str().unwrap().to_owned();
        let destination = checkout.join("blake3/unit");
        let staging = checkout.join("blake3/unit.seeding-123");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("partial"), b"partial").unwrap();
        let churn = destination.with_extension("json");
        std::fs::write(&churn, b"destination churn").unwrap();
        set_modified(&staging, UNIX_EPOCH);
        assert_eq!(unit_use_path(&staging), use_path(&destination));

        // A seeding compiler holds the destination's stamp.
        let seeding = hold_unit(&destination).unwrap().unwrap();
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.skipped_active_units, 1);
        assert!(staging.exists());
        drop(seeding);

        // An older mbx's seed takes no stamp; its session's lease protects it.
        let lease = lock_dir(cache.path(), &key).join("1-old.lease");
        std::fs::create_dir_all(lease.parent().unwrap()).unwrap();
        std::fs::write(&lease, b"").unwrap();
        let mut held = fslock::LockFile::open(&lease).unwrap();
        assert!(held.try_lock().unwrap());
        assert_eq!(
            collect(cache.path(), Some(0), None, false)
                .unwrap()
                .removed_units,
            0
        );
        assert!(staging.exists());
        drop(held);
        wait_until_unlocked(&lease);

        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert!(!staging.exists());
        assert!(churn.exists(), "the destination's churn record stays");
        assert!(use_path(&destination).exists());
    }

    #[test]
    fn the_hour_floor_keeps_recent_units_from_the_budget_unless_the_disk_is_short() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let legacy = unit(&checkout, "legacy", now_secs() - 300);
        let stamped = unit(&checkout, "stamped", 0);
        stamp(&stamped, now_secs() - 600);
        let preview = collect(cache.path(), Some(0), None, true).unwrap();
        assert_eq!(preview.removed_units, 0);
        assert_eq!(preview.kept_hot_units, 2);
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.kept_hot_units, 2);
        assert!(legacy.exists() && stamped.exists());
        // The last collection's lock on the stamp may linger for a moment.
        wait_until_unlocked(&use_path(&stamped));

        // Expiry is not the budget.
        let expired = collect(cache.path(), None, Some(Duration::from_secs(400)), false).unwrap();
        assert_eq!(expired.removed_units, 1);
        assert!(!stamped.exists());

        let outcome = super::collect(cache.path(), Some(0), None, false, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.kept_hot_units, 0);
        assert!(!legacy.exists());
    }

    #[test]
    fn a_removed_checkouts_units_go_before_a_live_checkouts_hot_units() {
        let cache = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        let gone = tempfile::tempdir().unwrap();
        let live_checkout = idle_checkout(cache.path(), live.path());
        let hot = unit(&live_checkout, "hot", now_secs() - 1200);
        let gone_checkout = idle_checkout(cache.path(), gone.path());
        let donor = unit(&gone_checkout, "donor", now_secs() - 600);
        gone.close().unwrap();
        let before = stats(cache.path()).unwrap();
        let outcome = collect(cache.path(), Some(before.bytes - 32), None, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.kept_hot_units, 1);
        assert!(hot.exists());
        assert!(!donor.exists());
        assert_eq!(outcome.removed_directories, 1, "then its bookkeeping");
    }

    #[cfg(unix)]
    #[test]
    fn a_unit_that_could_not_be_deleted_is_counted_until_it_is() {
        use std::os::unix::fs::PermissionsExt as _;
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        std::fs::write(directory.with_extension("json"), b"churn").unwrap();
        let session = directory.join("crate-id/s-session");
        std::fs::set_permissions(&session, std::fs::Permissions::from_mode(0o555)).unwrap();
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.removed_bytes, 5, "only the churn record went");
        let aside = removing_dirs(&checkout);
        assert_eq!(aside.len(), 1);
        assert_eq!(stats(cache.path()).unwrap().units, 0);
        assert!(stats(cache.path()).unwrap().bytes >= 32);
        let entry = only_entry(cache.path());
        assert!(entry.units.is_empty());
        assert_eq!(entry.removing.len(), 1);
        std::fs::set_permissions(
            aside[0].join("crate-id/s-session"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let outcome = collect(cache.path(), None, None, false).unwrap();
        assert_eq!(outcome.removed_bytes, 32);
        assert!(removing_dirs(&checkout).is_empty());
    }

    #[test]
    fn a_churn_record_that_cannot_be_removed_is_not_counted() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let record = directory.with_extension("json");
        std::fs::write(&record, b"churn").unwrap();
        let blocked = record.clone();
        on_pause("renamed", move || {
            // Something a file removal refuses.
            std::fs::remove_file(&blocked).unwrap();
            std::fs::create_dir_all(blocked.join("blocked")).unwrap();
        });
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.removed_units, 1);
        assert_eq!(outcome.removed_bytes, 32, "only the tree");
        assert!(!directory.exists());
        std::fs::remove_dir_all(&record).unwrap();
    }

    #[test]
    fn units_list_removals_apart_and_ignore_a_stamp_without_a_unit() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        unit(&checkout, "unit", 0);
        stamp(&checkout.join("blake3/gone"), 0);
        std::fs::create_dir_all(checkout.join("blake3/old.removing-1-abc/crate-id")).unwrap();
        std::fs::create_dir_all(checkout.join("blake3/new.seeding-1")).unwrap();
        let entry = only_entry(cache.path());
        let mut names = entry
            .units
            .iter()
            .map(|unit| unit.directory.file_name().unwrap().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["new.seeding-1", "unit"]);
        assert_eq!(entry.removing.len(), 1);
    }

    /// A gone checkout's empty shell, kept only by what something holds.
    fn retire_both_ways(cache: &Path, workspace: &Path, checkout: &Path, expect_kept: bool) {
        assert_eq!(
            remove_workspace(cache, workspace).unwrap() == RemoveOutcome::Active,
            expect_kept,
            "explicit removal"
        );
        assert_eq!(checkout.exists(), expect_kept);
    }

    #[test]
    fn whatever_outlived_its_session_keeps_its_checkout() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);

        // A shim still compiling.
        let shim = hold_unit(&directory).unwrap().unwrap();
        retire_both_ways(cache.path(), workspace.path(), &checkout, true);
        drop(shim);

        // A seed still copying, into a unit with no state yet.
        let destination = checkout.join("blake3/seeded");
        let seeder = hold_unit(&destination).unwrap().unwrap();
        std::fs::create_dir_all(checkout.join("blake3/seeded.seeding-1")).unwrap();
        retire_both_ways(cache.path(), workspace.path(), &checkout, true);
        drop(seeder);
        std::fs::remove_dir_all(checkout.join("blake3/seeded.seeding-1")).unwrap();

        // A compiler whose shim died.
        let lock = directory.join("crate-id/s-session.lock");
        std::fs::write(&lock, b"").unwrap();
        let mut rustc = fslock::LockFile::open(&lock).unwrap();
        assert!(rustc.try_lock().unwrap());
        retire_both_ways(cache.path(), workspace.path(), &checkout, true);
        drop(rustc);
        wait_until_unlocked(&lock);
        wait_until_unlocked(&use_path(&directory));
        wait_until_unlocked(&use_path(&destination));

        retire_both_ways(cache.path(), workspace.path(), &checkout, false);
    }

    #[test]
    fn a_gone_checkouts_shell_stays_while_a_shim_holds_a_stamp_in_it() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let shim = hold_unit(&checkout.join("blake3/unit")).unwrap().unwrap();
        workspace.close().unwrap();
        let outcome = collect(cache.path(), None, None, false).unwrap();
        assert_eq!(outcome.removed_directories, 0);
        assert_eq!(outcome.skipped_active_directories, 1);
        assert!(checkout.exists());
        drop(shim);
        wait_until_unlocked(&use_path(&checkout.join("blake3/unit")));
        let outcome = collect(cache.path(), None, None, false).unwrap();
        assert_eq!(outcome.removed_directories, 1);
        assert!(!checkout.exists());
    }

    #[test]
    fn only_an_explicit_removal_takes_state_or_abandoned_staging() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let key = checkout.file_name().unwrap().to_str().unwrap().to_owned();
        unit(&checkout, "unit", 0);
        assert_eq!(
            retire_checkout(cache.path(), &key, RetireMode::EmptyShell).unwrap(),
            Retired::Untouched
        );
        std::fs::remove_dir_all(checkout.join("blake3/unit")).unwrap();
        std::fs::create_dir_all(checkout.join("blake3/unit.seeding-1")).unwrap();
        assert_eq!(
            retire_checkout(cache.path(), &key, RetireMode::EmptyShell).unwrap(),
            Retired::Untouched
        );
        // Old enough that nothing reads as a compiler starting up.
        set_modified(&checkout.join("blake3/unit.seeding-1"), UNIX_EPOCH);
        unit(&checkout, "unit", 0);
        assert!(matches!(
            retire_checkout(cache.path(), &key, RetireMode::Explicit).unwrap(),
            Retired::Removed(bytes) if bytes > 0
        ));
        assert!(!checkout.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_retired_checkout_that_could_not_be_deleted_is_counted_until_it_is() {
        use std::os::unix::fs::PermissionsExt as _;
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        let session = directory.join("crate-id/s-session");
        std::fs::set_permissions(&session, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(remove_workspace(cache.path(), workspace.path()).is_err());
        assert!(!checkout.exists());
        let retired = std::fs::read_dir(cache.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| is_retired_name(path.file_name().unwrap()))
            .unwrap();
        let stats = stats(cache.path()).unwrap();
        assert_eq!(stats.directories, 0);
        assert!(stats.bytes >= 32);
        std::fs::set_permissions(
            retired.join("blake3/unit/crate-id/s-session"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let outcome = collect(cache.path(), None, None, false).unwrap();
        assert!(outcome.removed_bytes >= 32);
        assert_eq!(outcome.remaining_bytes, 0);
        assert!(!retired.exists());
    }

    #[test]
    fn a_compiler_starting_during_retirement_creates_nothing() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let key = checkout.file_name().unwrap().to_str().unwrap().to_owned();
        let fresh = checkout.join("blake3/fresh");
        let compiler = contend_at("retire", move || hold_unit(&fresh).unwrap().is_none());
        assert!(matches!(
            retire_checkout(cache.path(), &key, RetireMode::EmptyShell).unwrap(),
            Retired::Removed(_)
        ));
        assert!(contended(&compiler));
        assert!(!checkout.exists());
    }

    /// Make every session of `checkout`'s `unit` look last written at `used`.
    fn age_sessions(checkout: &Path, unit: &str, used: u64) {
        for session in nested_entries(&checkout.join(unit).join("widget-0abc")) {
            set_modified(&session.path(), UNIX_EPOCH + Duration::from_secs(used));
        }
    }

    #[test]
    fn seeding_prefers_the_donor_whose_unit_was_used_most_recently() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        let recorded = checkout_with_sessions(cache.path(), "recorded", 30, unit, &["s-1-a-done"]);
        let used = checkout_with_sessions(cache.path(), "used", 10, unit, &["s-2-b-done"]);
        age_sessions(&recorded, unit, 1000);
        age_sessions(&used, unit, 1000);
        stamp(&used.join(unit), 5000);

        assert_eq!(seed(&own, unit).unwrap(), Some(PathBuf::from("/work/used")));
        assert_eq!(
            modified(&use_path(&used.join(unit))),
            UNIX_EPOCH + Duration::from_secs(5000),
            "donating is not a use"
        );
    }

    #[test]
    fn seeding_skips_a_donor_being_evicted() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        let evicted = checkout_with_sessions(cache.path(), "evicted", 30, unit, &["s-1-a-done"]);
        checkout_with_sessions(cache.path(), "other", 10, unit, &["s-2-b-done"]);
        stamp(&evicted.join(unit), now_secs());
        let (locked, wait) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let directory = evicted.join(unit);
        let collector = std::thread::spawn(move || {
            let file = hold_exclusively(&directory);
            locked.send(()).unwrap();
            let _ = released.recv();
            drop(file);
        });
        wait.recv().unwrap();
        assert_eq!(
            seed(&own, unit).unwrap(),
            Some(PathBuf::from("/work/other"))
        );
        drop(release);
        collector.join().unwrap();
    }

    #[test]
    fn a_donor_without_a_stamp_is_protected_by_its_lease_alone() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        let donor = checkout_with_sessions(cache.path(), "donor", 10, unit, &["s-1-a-done"]);
        assert_eq!(
            seed(&own, unit).unwrap(),
            Some(PathBuf::from("/work/donor"))
        );
        assert!(
            !use_path(&donor.join(unit)).exists(),
            "no stamp is created in another checkout"
        );
    }

    #[test]
    fn a_donor_being_copied_is_kept_by_collection_and_explicit_removal() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        let donor = checkout_with_sessions(cache.path(), "donor", 10, unit, &["s-1-a-done"]);
        age_sessions(&donor, unit, 0);
        set_modified(&donor.join(unit).join("widget-0abc/s-1-a.lock"), UNIX_EPOCH);
        stamp(&donor.join(unit), 0);
        let root = cache.path().to_path_buf();
        let (sent, received) = std::sync::mpsc::channel();
        on_pause("seed", move || {
            let outcome = collect(&root, Some(0), None, false).unwrap();
            let removed = remove_workspace(&root, Path::new("/work/donor")).unwrap();
            sent.send((outcome.removed_units, removed)).unwrap();
        });
        assert_eq!(
            seed(&own, unit).unwrap(),
            Some(PathBuf::from("/work/donor"))
        );
        let (removed_units, removed) = received.recv().unwrap();
        assert_eq!(removed_units, 0);
        assert_eq!(removed, RemoveOutcome::Active);
        assert!(donor.join(unit).join("widget-0abc/s-1-a-done").is_dir());
        assert!(own.join(unit).join("widget-0abc/s-1-a-done").is_dir());
    }

    /// A unit with a fresh, unlocked lock file and the given session
    /// directory, in a checkout with no lease and no stamp held.
    fn starting_compiler(cache: &Path, workspace: &Path, session: &str) -> PathBuf {
        let checkout = idle_checkout(cache, workspace);
        let krate = checkout.join("blake3/unit/crate-id");
        std::fs::create_dir_all(krate.join(session)).unwrap();
        std::fs::write(krate.join("s-1-a.lock"), b"").unwrap();
        checkout
    }

    #[test]
    fn explicit_removal_waits_for_a_compiler_starting_up() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = starting_compiler(cache.path(), workspace.path(), "s-1-a-working");
        assert_eq!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Active
        );
        assert!(checkout.join("blake3/unit").exists());
    }

    #[test]
    fn explicit_removal_does_not_wait_after_a_finished_build() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = starting_compiler(cache.path(), workspace.path(), "s-1-a-done");
        // Its shim stamped the unit and has finished: no marker is left.
        stamp(&checkout.join("blake3/unit"), now_secs());
        assert!(matches!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Removed(_)
        ));
        assert!(!checkout.exists());
    }

    #[cfg(unix)]
    #[test]
    fn retirement_refuses_a_checkout_it_cannot_list_in_full() {
        use std::os::unix::fs::PermissionsExt as _;
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        let (locked, wait) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let shim_directory = directory.clone();
        let shim = std::thread::spawn(move || {
            let held = hold_unit(&shim_directory).unwrap().unwrap();
            locked.send(()).unwrap();
            let _ = released.recv();
            drop(held);
        });
        wait.recv().unwrap();
        let blake3 = checkout.join("blake3");
        std::fs::set_permissions(&blake3, std::fs::Permissions::from_mode(0o000)).unwrap();
        let restore = Restore(blake3);
        let key = checkout.file_name().unwrap().to_str().unwrap().to_owned();
        for mode in [RetireMode::Explicit, RetireMode::EmptyShell] {
            assert_eq!(
                retire_checkout(cache.path(), &key, mode).unwrap(),
                Retired::Active
            );
        }
        drop(restore);
        assert!(directory.exists());
        assert!(checkout.join(RECORD_FILE).exists());
        drop(release);
        shim.join().unwrap();
    }

    #[test]
    fn explicit_removal_sees_a_compiler_that_starts_after_the_stamps_are_locked() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        let krate = directory.join("crate-id");
        on_pause("sessions", move || {
            std::fs::create_dir_all(krate.join("s-1-a-working")).unwrap();
            std::fs::write(krate.join("s-1-a.lock"), b"").unwrap();
        });
        assert_eq!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Active
        );
        assert!(directory.join("crate-id/s-1-a-working").is_dir());
        assert!(checkout.join(RECORD_FILE).exists());
        assert!(
            std::fs::read_dir(cache.path())
                .unwrap()
                .flatten()
                .all(|entry| !is_retired_name(&entry.file_name())),
            "nothing was renamed"
        );
    }

    #[test]
    fn a_lease_that_cannot_be_inspected_counts_as_an_older_mbxs() {
        assert!(lease_is_legacy(Err(std::io::Error::other("unreadable"))));
        let directory = tempfile::tempdir().unwrap();
        let empty = directory.path().join("empty.lease");
        std::fs::write(&empty, b"").unwrap();
        assert!(lease_is_legacy(std::fs::metadata(&empty)));
        let versioned = directory.path().join("versioned.lease");
        std::fs::write(&versioned, LEASE_VERSION).unwrap();
        assert!(!lease_is_legacy(std::fs::metadata(&versioned)));
    }

    #[test]
    fn a_stamped_compilation_is_marked_until_it_finishes() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        drop(hold_unit(&directory).unwrap().unwrap());
        assert!(
            compiling_markers_of(&directory).is_empty(),
            "an unstamped guard marks nothing"
        );
        let mut compiler = hold_unit(&directory).unwrap().unwrap();
        compiler.stamp().unwrap();
        assert_eq!(compiling_markers_of(&directory).len(), 1);
        drop(compiler);
        assert!(compiling_markers_of(&directory).is_empty());
    }

    #[test]
    fn each_compilation_has_its_own_marker() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = checkout.join("blake3/unit");
        std::fs::create_dir_all(&directory).unwrap();
        let mut first = hold_unit(&directory).unwrap().unwrap();
        first.stamp().unwrap();
        let mut second = hold_unit(&directory).unwrap().unwrap();
        second.stamp().unwrap();
        assert_eq!(compiling_markers_of(&directory).len(), 2);
        drop(first);
        let left = compiling_markers_of(&directory);
        assert_eq!(left, [second.marker.as_ref().unwrap().0.clone()]);
        // The second compilation's shim and session die before rustc locks
        // anything: only its marker is left. Leaking its stamp's descriptor
        // would keep the lock, so release it and leave the marker behind.
        second.marker.take();
        drop(second);
        wait_until_unlocked(&use_path(&directory));
        assert_eq!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Active
        );
        for marker in left {
            std::fs::remove_file(marker).unwrap();
        }
        assert!(matches!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Removed(_)
        ));
    }

    #[test]
    fn refreshing_marks_a_compilation_as_starting_again() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = checkout.join("blake3/unit");
        std::fs::create_dir_all(&directory).unwrap();
        let mut compiler = hold_unit(&directory).unwrap().unwrap();
        compiler.stamp().unwrap();
        let marker = compiler.marker.as_ref().unwrap().0.clone();
        set_modified(&marker, ago(SESSION_START_GRACE_SECS + 60));
        let starting = || session_locks(&directory, true, true).unwrap().is_none();
        assert!(!starting());
        compiler.refresh().unwrap();
        assert!(within_throttled(modified(&marker)));
        assert!(starting());
        // A shim that died after the refresh leaves its marker fresh.
        compiler.marker.take();
        drop(compiler);
        wait_until_unlocked(&use_path(&directory));
        assert_eq!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Active
        );
        assert!(marker.exists());
    }

    #[test]
    fn a_cache_hit_measuring_the_backstop_does_not_stop_the_marker() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        // A hit is measuring right now.
        let measuring = std::fs::File::create(backstop_path(&directory)).unwrap();
        measuring.try_lock().unwrap();
        let mut compiler = hold_unit(&directory).unwrap().unwrap();
        compiler.stamp().unwrap();
        assert_eq!(compiling_markers_of(&directory).len(), 1);
        drop(measuring);
    }

    /// A unit with no sessions whose compilation was marked as started
    /// `started` seconds ago and never finished.
    fn interrupted_compilation(cache: &Path, workspace: &Path, started: u64) -> PathBuf {
        let checkout = idle_checkout(cache, workspace);
        let directory = checkout.join("blake3/unit");
        std::fs::create_dir_all(&directory).unwrap();
        stamp(&directory, now_secs() - started);
        let marker = directory.join("compiling-1-abcdefgh");
        std::fs::write(&marker, b"").unwrap();
        set_modified(&marker, ago(started));
        checkout
    }

    #[test]
    fn explicit_removal_keeps_a_compilation_that_has_not_finished() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = interrupted_compilation(cache.path(), workspace.path(), 0);
        assert_eq!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Active
        );
        assert!(checkout.join("blake3/unit").is_dir());
        assert!(
            std::fs::read_dir(cache.path())
                .unwrap()
                .flatten()
                .all(|entry| !is_retired_name(&entry.file_name())),
            "nothing was renamed"
        );
    }

    #[test]
    fn a_stale_compilation_marker_does_not_keep_a_checkout() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = interrupted_compilation(
            cache.path(),
            workspace.path(),
            SESSION_START_GRACE_SECS + 60,
        );
        let directory = checkout.join("blake3/unit");
        // Hits keep refreshing the stamp, but not the marker.
        for _ in 0..3 {
            set_modified(&use_path(&directory), ago(USE_STAMP_THROTTLE_SECS + 1));
            note_use(&directory, None);
            assert!(within_throttled(modified(&use_path(&directory))));
        }
        wait_until_unlocked(&use_path(&directory));
        assert!(matches!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Removed(_)
        ));
        assert!(!checkout.exists());
    }

    #[test]
    fn explicit_removal_sees_a_marker_made_after_the_checkout_was_listed() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = checkout.join("blake3/unit");
        std::fs::create_dir_all(&directory).unwrap();
        stamp(&directory, now_secs());
        // The shim marked its compilation after the listing, then died.
        let marker = directory.join("compiling-1-x");
        let made = marker.clone();
        on_pause("sessions", move || std::fs::write(&made, b"").unwrap());
        assert_eq!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Active
        );
        assert!(marker.exists());
        assert!(checkout.join(RECORD_FILE).exists());
    }

    #[test]
    fn a_donors_compilation_marker_is_never_seeded() {
        let cache = tempfile::tempdir().unwrap();
        let unit = "blake3/0123456789";
        let own = checkout_with_sessions(cache.path(), "own", 40, unit, &[]);
        let donor = checkout_with_sessions(cache.path(), "donor", 10, unit, &["s-1-a-done"]);
        std::fs::write(donor.join(unit).join("compiling-1-x"), b"").unwrap();
        assert_eq!(
            seed(&own, unit).unwrap(),
            Some(PathBuf::from("/work/donor"))
        );
        assert!(compiling_markers_of(&own.join(unit)).is_empty());
    }

    #[test]
    fn a_rejected_compilation_records_its_verdict_for_the_baseline_after_it() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        assert!(!backstop_verdict(&directory, Some(32)));
        assert_eq!(
            std::fs::read(backstop_path(&directory)).unwrap(),
            BACKSTOP_REJECTED
        );
        note_use(&directory, Some(32));
        assert_eq!(backstop_walks(&directory), 1, "the verdict is reused");
        assert_eq!(modified(&use_path(&directory)), UNIX_EPOCH);
    }

    /// A stamped unit with a fresh, unlocked lock file and the given session
    /// directory, as an older mbx's compiler starting up leaves it.
    fn stamped_with_session(cache: &Path, workspace: &Path, session: &str) -> PathBuf {
        let checkout = idle_checkout(cache, workspace);
        let directory = unit(&checkout, "unit", 0);
        stamp(&directory, 0);
        let krate = directory.join("crate-id");
        std::fs::create_dir_all(krate.join(session)).unwrap();
        std::fs::write(krate.join("s-1-a.lock"), b"").unwrap();
        directory
    }

    #[test]
    fn an_older_mbx_compiling_a_stamped_unit_is_seen_starting_up() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let directory = stamped_with_session(cache.path(), workspace.path(), "s-1-a-working");
        let outcome = super::collect(cache.path(), Some(0), None, false, false).unwrap();
        assert_eq!(outcome.removed_units, 0);
        assert_eq!(outcome.skipped_active_units, 1);
        assert_eq!(
            remove_workspace(cache.path(), workspace.path()).unwrap(),
            RemoveOutcome::Active
        );
        assert!(directory.exists());
    }

    #[test]
    fn a_stamped_unit_with_only_finished_sessions_is_not_starting_up() {
        for explicit in [false, true] {
            let cache = tempfile::tempdir().unwrap();
            let workspace = tempfile::tempdir().unwrap();
            let directory = stamped_with_session(cache.path(), workspace.path(), "s-1-a-done");
            set_modified(&directory.join("crate-id/s-1-a-done"), UNIX_EPOCH);
            if explicit {
                assert!(matches!(
                    remove_workspace(cache.path(), workspace.path()).unwrap(),
                    RemoveOutcome::Removed(_)
                ));
            } else {
                let outcome = super::collect(cache.path(), Some(0), None, false, false).unwrap();
                assert_eq!(outcome.removed_units, 1);
            }
            assert!(!directory.exists());
        }
    }

    #[test]
    fn a_finished_compilation_expires_the_verdict_under_its_lock() {
        let cache = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let checkout = idle_checkout(cache.path(), workspace.path());
        let directory = unit(&checkout, "unit", 0);
        let backstop = backstop_path(&directory);
        std::fs::write(&backstop, BACKSTOP_ACCEPTED).unwrap();
        let mut compiler = hold_unit(&directory).unwrap().unwrap();
        compiler.stamp().unwrap();
        // A cache hit is reading the verdict.
        let reader = open_stamp(&backstop).unwrap().unwrap();
        reader.lock().unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let dropper = std::thread::spawn(move || {
            drop(compiler);
            done.send(()).unwrap();
        });
        assert!(
            matches!(
                finished.recv_timeout(Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "the verdict is expired under its lock"
        );
        reader.unlock().unwrap();
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        dropper.join().unwrap();
        use std::io::Read as _;
        let mut content = Vec::new();
        (&reader).read_to_end(&mut content).unwrap();
        assert!(content.is_empty(), "the same file, emptied");
        assert!(backstop.exists());
    }
}
