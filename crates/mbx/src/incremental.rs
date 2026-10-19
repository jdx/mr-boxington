//! Persistent per-checkout state for learned incremental compilation.

use eyre::{Context, Result};
use mbx_cache_core::CacheDigest;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECORD_FILE: &str = "checkout.json";
const RECORD_VERSION: u8 = 1;
const LOCKS_DIR: &str = ".locks";
const REGISTRAR_FILE: &str = "registrar.lock";

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
    pub bytes: u64,
    pub live_checkouts: u64,
    pub stale_checkouts: u64,
    pub untracked_directories: u64,
}

#[derive(Debug, Default)]
pub(crate) struct PruneOutcome {
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

/// Inspect all traceable learned incremental checkout state.
pub(crate) fn stats(root: &Path) -> Result<Stats> {
    let mut stats = Stats::default();
    for entry in entries(root)? {
        stats.directories += 1;
        stats.bytes = stats.bytes.saturating_add(entry.bytes);
        match entry.live {
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
        if !entry.file_type()?.is_dir() || entry.file_name() == LOCKS_DIR {
            continue;
        }
        if let Some(record) = read_record(&entry.path().join(RECORD_FILE)) {
            workspaces.push(record.workspace_root);
        }
    }
    Ok(workspaces)
}

/// Collect expired and least-recently-used checkout state.
///
/// A deleted checkout's state is kept like any other until it expires or the
/// budget needs the room: it is what the next new checkout starts from (see
/// [`seed`]), and worktrees are usually removed as soon as their work merges.
pub(crate) fn collect(
    root: &Path,
    max_bytes: Option<u64>,
    max_age: Option<Duration>,
    dry_run: bool,
) -> Result<PruneOutcome> {
    let now = now_secs();
    let mut entries = entries(root)?;
    let initial_directories = entries.len() as u64;
    let untracked_directories = entries.iter().filter(|entry| entry.live.is_none()).count() as u64;
    let mut required = HashSet::new();
    let mut remaining_bytes = entries
        .iter()
        .map(|entry| entry.bytes)
        .fold(0_u64, u64::saturating_add);
    for entry in &entries {
        let Some(updated_secs) = entry.updated_secs else {
            continue;
        };
        let expired = max_age.is_some_and(|age| now.saturating_sub(updated_secs) > age.as_secs());
        if expired {
            required.insert(entry.key.clone());
        }
    }
    entries.sort_by_key(|entry| entry.updated_secs.unwrap_or(u64::MAX));
    let mut candidates = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.updated_secs.is_some() && !required.contains(&entry.key))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    // Keep one recently used checkout. A budget smaller than the active edit
    // loop cannot be met sustainably by deleting it after every build.
    candidates.pop();
    let mut planned = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| required.contains(&entry.key))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    planned.extend(candidates);

    let mut outcome = PruneOutcome::default();
    for index in planned {
        let entry = &entries[index];
        if !required.contains(&entry.key)
            && max_bytes.is_none_or(|max_bytes| remaining_bytes <= max_bytes)
        {
            break;
        }
        let guard = deletion_guard(root, &entry.key, !dry_run)?;
        if guard.active {
            outcome.skipped_active_directories += 1;
            continue;
        }
        if !dry_run {
            match std::fs::remove_dir_all(&entry.directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    log::warn!(
                        "could not remove incremental state {}: {error}",
                        entry.directory.display()
                    );
                    continue;
                }
            }
            guard.cleanup();
        }
        outcome.removed_directories += 1;
        outcome.removed_bytes = outcome.removed_bytes.saturating_add(entry.bytes);
        remaining_bytes = remaining_bytes.saturating_sub(entry.bytes);
        drop(guard);
    }
    outcome.remaining_directories = initial_directories.saturating_sub(outcome.removed_directories);
    outcome.remaining_bytes = remaining_bytes;
    outcome.untracked_directories = untracked_directories;
    if let Some(max_bytes) = max_bytes
        && remaining_bytes > max_bytes
    {
        log::warn!(
            "learned incremental state still holds {} after collection, over the {} budget; active, most-recently-used, and untracked directories are kept",
            bytesize::ByteSize::b(remaining_bytes).display().iec(),
            bytesize::ByteSize::b(max_bytes).display().iec(),
        );
    }
    Ok(outcome)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    Missing,
    Removed(u64),
    Active,
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
    let guard = deletion_guard(root, &key, true)?;
    if guard.active {
        return Ok(RemoveOutcome::Active);
    }
    let bytes = tree_bytes(&directory);
    match std::fs::remove_dir_all(&directory) {
        Ok(()) => {
            guard.cleanup();
            Ok(RemoveOutcome::Removed(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            guard.cleanup();
            Ok(RemoveOutcome::Removed(0))
        }
        Err(error) => Err(error.into()),
    }
}

struct Entry {
    key: String,
    directory: PathBuf,
    updated_secs: Option<u64>,
    bytes: u64,
    live: Option<bool>,
}

fn entries(root: &Path) -> Result<Vec<Entry>> {
    let listing = match std::fs::read_dir(root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut entries = Vec::new();
    for entry in listing {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.file_name() == LOCKS_DIR {
            continue;
        }
        let directory = entry.path();
        let record = read_record(&directory.join(RECORD_FILE));
        entries.push(Entry {
            key: entry.file_name().to_string_lossy().into_owned(),
            bytes: tree_bytes(&directory),
            live: record
                .as_ref()
                .map(|record| crate::store::checkout_is_live_on(root, &record.workspace_root)),
            updated_secs: record.as_ref().map(|record| record.updated_secs),
            directory,
        });
    }
    Ok(entries)
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
        return Ok(DeletionGuard {
            _registrar: None,
            locks,
            active: false,
            cleanup,
        });
    }
    std::fs::create_dir_all(registrar_path.parent().expect("registrar has a parent"))?;
    let mut registrar = fslock::LockFile::open(&registrar_path)?;
    registrar.lock()?;
    let mut active = false;
    let listing = match std::fs::read_dir(&locks) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DeletionGuard {
                _registrar: Some(registrar),
                locks,
                active,
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
        }
    }
    Ok(DeletionGuard {
        _registrar: Some(registrar),
        locks,
        active,
        cleanup,
    })
}

fn cleanup_lock_dir(locks: &Path) {
    // Older development builds placed the registrar in each checkout's lock
    // directory. It is safe to remove while the global registrar is held.
    let _ = std::fs::remove_file(locks.join(REGISTRAR_FILE));
    let _ = std::fs::remove_dir(locks);
}

/// Copy the most recently used other checkout's state for one unit into
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
        if name == own || name == LOCKS_DIR || !entry.path().join(unit).is_dir() {
            continue;
        }
        if let Some(record) = read_record(&entry.path().join(RECORD_FILE)) {
            donors.push((record.updated_secs, record.workspace_root, name));
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

fn tree_bytes(root: &Path) -> u64 {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(listing) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in listing.flatten() {
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
        drop(own);
        assert_eq!(
            std::fs::read(&occupied).unwrap(),
            b"another namespace's lease"
        );
        assert!(deletion_guard(cache.path(), key, false).unwrap().active);
        drop(other);
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
        std::fs::write(directory.join("state"), b"incremental").unwrap();
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

    #[test]
    fn aggregate_budget_keeps_the_most_recent_checkout() {
        let cache = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_state = touch(cache.path(), first.path()).unwrap();
        std::fs::write(first_state.directory.join("state"), vec![0; 32]).unwrap();
        drop(first_state.lease);
        std::thread::sleep(Duration::from_secs(1));
        let second_state = touch(cache.path(), second.path()).unwrap();
        std::fs::write(second_state.directory.join("state"), vec![0; 32]).unwrap();
        drop(second_state.lease);

        let outcome = collect(cache.path(), Some(1), None, false).unwrap();
        assert_eq!(outcome.removed_directories, 1);
        assert!(!first_state.directory.exists());
        assert!(second_state.directory.exists());
    }

    #[test]
    fn an_active_old_checkout_does_not_hide_an_inactive_candidate() {
        let cache = tempfile::tempdir().unwrap();
        let active = tempfile::tempdir().unwrap();
        let inactive = tempfile::tempdir().unwrap();
        let newest = tempfile::tempdir().unwrap();
        let active_state = touch(cache.path(), active.path()).unwrap();
        std::fs::write(active_state.directory.join("state"), vec![0; 32]).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        let inactive_state = touch(cache.path(), inactive.path()).unwrap();
        std::fs::write(inactive_state.directory.join("state"), vec![0; 32]).unwrap();
        drop(inactive_state.lease);
        std::thread::sleep(Duration::from_secs(1));
        let newest_state = touch(cache.path(), newest.path()).unwrap();
        std::fs::write(newest_state.directory.join("state"), vec![0; 32]).unwrap();
        drop(newest_state.lease);

        let budget =
            tree_bytes(&active_state.directory).saturating_add(tree_bytes(&newest_state.directory));
        let outcome = collect(cache.path(), Some(budget), None, false).unwrap();

        assert_eq!(outcome.removed_directories, 1);
        assert_eq!(outcome.skipped_active_directories, 1);
        assert!(active_state.directory.exists());
        assert!(!inactive_state.directory.exists());
        assert!(newest_state.directory.exists());
        assert!(outcome.remaining_bytes <= budget);
    }

    #[test]
    fn unreadable_state_is_counted_but_not_deleted() {
        let cache = tempfile::tempdir().unwrap();
        let directory = cache.path().join("untracked");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("state"), vec![0; 32]).unwrap();

        let stats = stats(cache.path()).unwrap();
        assert_eq!(stats.directories, 1);
        assert_eq!(stats.untracked_directories, 1);
        assert_eq!(stats.bytes, 32);
        let outcome = collect(cache.path(), Some(0), None, false).unwrap();
        assert_eq!(outcome.untracked_directories, 1);
        assert_eq!(outcome.remaining_bytes, 32);
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

    /// A checkout last used at `updated_secs`, holding the given rustc session
    /// directories for one unit.
    fn checkout_with_sessions(
        cache: &Path,
        key: &str,
        updated_secs: u64,
        unit: &str,
        sessions: &[&str],
    ) -> PathBuf {
        let directory = cache.join(key);
        for session in sessions {
            let session = directory.join(unit).join("widget-0abc").join(session);
            std::fs::create_dir_all(&session).unwrap();
            std::fs::write(session.join("dep-graph.bin"), key).unwrap();
            let lock = session_lock_name(session.file_name().unwrap()).unwrap();
            std::fs::write(session.with_file_name(lock), b"").unwrap();
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
