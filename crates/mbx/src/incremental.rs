//! Persistent per-checkout state for learned incremental compilation.

use eyre::{Context, Result};
use mbx_cache_core::CacheDigest;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECORD_FILE: &str = "checkout.json";
const RECORD_VERSION: u8 = 1;
const LOCKS_DIR: &str = ".locks";
static LEASE_NONCE: AtomicU64 = AtomicU64::new(0);

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
}

impl Drop for ActiveLease {
    fn drop(&mut self) {
        let registrar = self
            .path
            .parent()
            .map(|parent| parent.join("registrar.lock"));
        let mut registrar = registrar.and_then(|path| fslock::LockFile::open(&path).ok());
        if let Some(registrar) = &mut registrar {
            let _ = registrar.lock();
        }
        drop(self.lock.take());
        let _ = std::fs::remove_file(&self.path);
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

/// Collect deleted, expired, and least-recently-used checkout state.
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
    let mut selected = HashSet::new();
    let mut remaining_bytes = entries
        .iter()
        .map(|entry| entry.bytes)
        .fold(0_u64, u64::saturating_add);
    for entry in &entries {
        let Some(updated_secs) = entry.updated_secs else {
            continue;
        };
        let expired = max_age.is_some_and(|age| now.saturating_sub(updated_secs) > age.as_secs());
        if entry.live == Some(false) || expired {
            selected.insert(entry.key.clone());
        }
    }
    if let Some(max_bytes) = max_bytes {
        let selected_bytes = entries
            .iter()
            .filter(|entry| selected.contains(&entry.key))
            .map(|entry| entry.bytes)
            .fold(0_u64, u64::saturating_add);
        let mut projected = remaining_bytes.saturating_sub(selected_bytes);
        if projected > max_bytes {
            entries.sort_by_key(|entry| entry.updated_secs.unwrap_or(u64::MAX));
            let mut candidates = entries
                .iter()
                .filter(|entry| entry.updated_secs.is_some() && !selected.contains(&entry.key))
                .collect::<Vec<_>>();
            // Keep one recently used checkout. A budget smaller than the active
            // edit loop cannot be met sustainably by deleting it after every build.
            candidates.pop();
            for entry in candidates {
                if projected <= max_bytes {
                    break;
                }
                selected.insert(entry.key.clone());
                projected = projected.saturating_sub(entry.bytes);
            }
        }
    }

    let mut outcome = PruneOutcome::default();
    for entry in entries {
        if !selected.contains(&entry.key) {
            continue;
        }
        let guard = deletion_guard(root, &entry.key)?;
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

/// Remove learned incremental state for exactly one workspace.
pub(crate) fn remove_workspace(root: &Path, workspace_root: &Path) -> Result<Option<u64>> {
    let key = checkout_key(workspace_root);
    let directory = root.join(&key);
    let Some(record) = read_record(&directory.join(RECORD_FILE)) else {
        return Ok(None);
    };
    if record.workspace_root != workspace_root {
        return Ok(None);
    }
    let guard = deletion_guard(root, &key)?;
    if guard.active {
        eyre::bail!(
            "{} is being built, so its incremental state was kept",
            workspace_root.display()
        );
    }
    let bytes = tree_bytes(&directory);
    match std::fs::remove_dir_all(&directory) {
        Ok(()) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Some(0)),
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
                .map(|record| crate::store::checkout_is_live(&record.workspace_root)),
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

fn acquire_lease(root: &Path, key: &str) -> Result<ActiveLease> {
    let locks = lock_dir(root, key);
    std::fs::create_dir_all(&locks)?;
    let mut registrar = fslock::LockFile::open(&locks.join("registrar.lock"))?;
    registrar.lock()?;
    let nonce = LEASE_NONCE.fetch_add(1, Ordering::Relaxed);
    let path = locks.join(format!("{}-{nonce}.lease", std::process::id()));
    let mut lock = fslock::LockFile::open(&path)?;
    lock.lock()?;
    drop(registrar);
    Ok(ActiveLease {
        lock: Some(lock),
        path,
    })
}

struct DeletionGuard {
    _registrar: fslock::LockFile,
    active: bool,
}

fn deletion_guard(root: &Path, key: &str) -> Result<DeletionGuard> {
    let locks = lock_dir(root, key);
    std::fs::create_dir_all(&locks)?;
    let mut registrar = fslock::LockFile::open(&locks.join("registrar.lock"))?;
    registrar.lock()?;
    let mut active = false;
    for entry in std::fs::read_dir(&locks)? {
        let entry = entry?;
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("lease") {
            continue;
        }
        let mut lease = fslock::LockFile::open(&entry.path())?;
        if lease.try_lock()? {
            drop(lease);
            let _ = std::fs::remove_file(entry.path());
        } else {
            active = true;
        }
    }
    Ok(DeletionGuard {
        _registrar: registrar,
        active,
    })
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
    fn abandoned_checkout_state_is_pruned() {
        let cache = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let checkout = parent.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let state = touch(cache.path(), &checkout).unwrap();
        let directory = state.directory.clone();
        std::fs::write(directory.join("state"), b"incremental").unwrap();
        assert!(read_record(&directory.join(RECORD_FILE)).is_some());
        drop(state);

        std::fs::remove_dir(&checkout).unwrap();
        assert!(!crate::store::checkout_is_live(&checkout));
        let outcome = collect(cache.path(), None, None, false).unwrap();

        assert_eq!(outcome.removed_directories, 1);
        assert!(outcome.removed_bytes > 0);
        assert!(!directory.exists());
    }

    #[test]
    fn active_checkout_state_is_not_pruned_or_removed() {
        let cache = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let checkout = parent.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let state = touch(cache.path(), &checkout).unwrap();
        std::fs::remove_dir(&checkout).unwrap();

        let outcome = collect(cache.path(), Some(0), Some(Duration::ZERO), false).unwrap();
        assert_eq!(outcome.removed_directories, 0);
        assert_eq!(outcome.skipped_active_directories, 1);
        assert!(remove_workspace(cache.path(), &checkout).is_err());
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

        assert!(
            remove_workspace(cache.path(), selected.path())
                .unwrap()
                .is_some()
        );
        assert!(!selected_directory.exists());
        assert!(kept_state.directory.exists());
    }
}
