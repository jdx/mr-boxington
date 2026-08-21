//! Store inspection and garbage collection.
//!
//! The store holds three trees: `cas/v1` for content-addressed objects,
//! `action-results/v1` for the results that reference them, and
//! `task-manifests/v1` for the prediction index. Only the first two are
//! collected; manifests are small and are what makes a cold build fast.

use eyre::{Context, Result};
use mbx_cache_core::{LocalCas, RemoteActionResult};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const CAS_DIR: &str = "cas/v1";
const ACTION_RESULTS_DIR: &str = "action-results/v1";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub objects: u64,
    pub object_bytes: u64,
    pub action_results: u64,
    pub action_result_bytes: u64,
}

impl StoreStats {
    pub fn total_bytes(&self) -> u64 {
        self.object_bytes.saturating_add(self.action_result_bytes)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcOutcome {
    pub removed_objects: u64,
    pub removed_action_results: u64,
    pub removed_bytes: u64,
    pub remaining_bytes: u64,
}

/// Summarize what the store currently holds.
pub fn stats(store: &Path) -> Result<StoreStats> {
    let objects = walk_files(&store.join(CAS_DIR))?;
    let results = walk_files(&store.join(ACTION_RESULTS_DIR))?;
    Ok(StoreStats {
        objects: objects.len() as u64,
        object_bytes: objects.iter().map(|entry| entry.size).sum(),
        action_results: results.len() as u64,
        action_result_bytes: results.iter().map(|entry| entry.size).sum(),
    })
}

/// Evict objects until the store fits within `max_bytes`.
///
/// Eviction is least-recently-used by access time where the filesystem records
/// it, falling back to modification time. Restores hard-link or reflink out of
/// the store rather than reading through it, so this ordering is only an
/// approximation -- evicting a live object costs a recompile, never correctness.
pub fn gc(store: &Path, max_bytes: u64) -> Result<GcOutcome> {
    let mut objects = walk_files(&store.join(CAS_DIR))?;
    let results = walk_files(&store.join(ACTION_RESULTS_DIR))?;
    let mut live_bytes = objects
        .iter()
        .chain(results.iter())
        .map(|entry| entry.size)
        .sum::<u64>();

    let mut outcome = GcOutcome::default();
    if live_bytes > max_bytes {
        objects.sort_by_key(|entry| entry.used);
        for entry in &objects {
            if live_bytes <= max_bytes {
                break;
            }
            if remove(&entry.path)? {
                live_bytes = live_bytes.saturating_sub(entry.size);
                outcome.removed_objects += 1;
                outcome.removed_bytes += entry.size;
            }
        }
    }

    // An action result whose objects are gone can only produce a miss, so drop
    // it rather than leave the index pointing at nothing. This runs whether or
    // not this call evicted anything, since another process may have, and a
    // store that is over budget on results alone still needs the sweep.
    let cas = LocalCas::new(store);
    for entry in &results {
        if action_result_is_dangling(&cas, &entry.path)? && remove(&entry.path)? {
            live_bytes = live_bytes.saturating_sub(entry.size);
            outcome.removed_action_results += 1;
            outcome.removed_bytes += entry.size;
        }
    }

    outcome.remaining_bytes = live_bytes;
    Ok(outcome)
}

fn remove(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        // A concurrent build may have evicted or replaced it already.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).wrap_err_with(|| format!("failed to evict {}", path.display())),
    }
}

/// Whether an action result references an object the store no longer has.
///
/// The digests checked here are exactly the ones `LocalActionCache::store`
/// requires. If the sweep checked fewer, eviction could leave an entry that
/// cannot be republished -- `store` would reject the identical result for a
/// missing blob while the index still claimed to hold it.
///
/// Only the top-level objects are checked; a result whose output tree lost a
/// nested object still restores as a miss, which is safe.
fn action_result_is_dangling(cas: &LocalCas, path: &Path) -> Result<bool> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
        }
    };
    // An unparseable result is already useless; the cache rejects it on read.
    let Ok(result) = serde_json::from_slice::<RemoteActionResult>(&bytes) else {
        return Ok(false);
    };
    for digest in [
        Some(&result.action),
        result.metadata.as_ref(),
        result.output_root.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        // A verification failure leaves the object unusable, same as missing.
        if cas.find(digest).unwrap_or(None).is_none() {
            return Ok(true);
        }
    }
    Ok(false)
}

struct Entry {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

fn walk_files(root: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let listing = match std::fs::read_dir(&directory) {
            Ok(listing) => listing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .wrap_err_with(|| format!("failed to read {}", directory.display()));
            }
        };
        for entry in listing {
            let entry = entry?;
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                // A concurrent build may be publishing into the store.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                let used = metadata
                    .accessed()
                    .or_else(|_| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                entries.push(Entry {
                    path: entry.path(),
                    size: metadata.len(),
                    used,
                });
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mbx_cache_core::{CacheDigest, LocalActionCache};
    use std::time::Duration;

    fn store_object(store: &Path, contents: &[u8]) -> CacheDigest {
        let digest = CacheDigest::blake3(contents);
        LocalCas::new(store).store_bytes(&digest, contents).unwrap();
        digest
    }

    /// Backdate an object so eviction order is deterministic.
    fn age(store: &Path, digest: &CacheDigest, age: Duration) {
        let path = LocalCas::new(store).path_for(digest).unwrap();
        let when = SystemTime::now() - age;
        let time = filetime::FileTime::from_system_time(when);
        filetime::set_file_times(path, time, time).unwrap();
    }

    #[test]
    fn reports_an_empty_store() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(stats(directory.path()).unwrap(), StoreStats::default());
    }

    #[test]
    fn counts_objects_and_results() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path();
        store_object(store, b"first");
        store_object(store, b"second object");

        let stats = stats(store).unwrap();

        assert_eq!(stats.objects, 2);
        assert_eq!(stats.object_bytes, 5 + 13);
        assert_eq!(stats.total_bytes(), 18);
    }

    #[test]
    fn keeps_the_store_under_its_budget_evicting_oldest_first() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path();
        let old = store_object(store, b"0123456789");
        let recent = store_object(store, b"abcdefghij");
        age(store, &old, Duration::from_secs(60 * 60));
        age(store, &recent, Duration::from_secs(1));

        let outcome = gc(store, 10).unwrap();

        assert_eq!(outcome.removed_objects, 1);
        assert_eq!(outcome.removed_bytes, 10);
        assert_eq!(outcome.remaining_bytes, 10);
        let cas = LocalCas::new(store);
        assert!(cas.find(&old).unwrap().is_none());
        assert!(cas.find(&recent).unwrap().is_some());
    }

    #[test]
    fn leaves_a_store_within_budget_alone() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path();
        store_object(store, b"kept");

        let outcome = gc(store, 1024).unwrap();

        assert_eq!(
            outcome,
            GcOutcome {
                remaining_bytes: 4,
                ..GcOutcome::default()
            }
        );
        assert_eq!(stats(store).unwrap().objects, 1);
    }

    #[test]
    fn drops_an_action_result_whose_descriptor_blob_is_gone() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path();
        let action = store_object(store, b"action key");
        let output_root = store_object(store, b"output root blob");
        let result = RemoteActionResult {
            version: 1,
            action: action.clone(),
            metadata: None,
            output_root: Some(output_root.clone()),
        };
        let cache = LocalActionCache::new(store);
        cache.store(&result).unwrap();

        // Evict only the descriptor blob, leaving the output root in place.
        std::fs::remove_file(LocalCas::new(store).find(&action).unwrap().unwrap()).unwrap();

        let outcome = gc(store, u64::MAX).unwrap();

        // Left behind, this entry would report a hit that `store` could never
        // republish, since publication requires the descriptor blob.
        assert_eq!(outcome.removed_action_results, 1);
        assert!(cache.find(&action).unwrap().is_none());
    }

    #[test]
    fn drops_action_results_left_without_their_objects() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path();
        let action = store_object(store, b"action key");
        let metadata = store_object(store, b"metadata blob");
        let result = RemoteActionResult {
            version: 1,
            action: action.clone(),
            metadata: Some(metadata),
            output_root: None,
        };
        LocalActionCache::new(store).store(&result).unwrap();

        // A budget of zero evicts every object, orphaning the result.
        let outcome = gc(store, 0).unwrap();

        assert!(outcome.removed_objects >= 1);
        assert_eq!(outcome.removed_action_results, 1);
        assert!(
            LocalActionCache::new(store)
                .find(&action)
                .unwrap()
                .is_none()
        );
    }
}
