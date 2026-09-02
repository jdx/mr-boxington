//! Persistent per-checkout state for learned incremental compilation.

use eyre::{Context, Result};
use mbx_cache_core::CacheDigest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECORD_FILE: &str = "checkout.json";
const RECORD_VERSION: u8 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutRecord {
    version: u8,
    workspace_root: PathBuf,
    updated_secs: u64,
}

#[derive(Debug, Default)]
pub(crate) struct PruneOutcome {
    pub removed_directories: u64,
    pub removed_bytes: u64,
}

/// Refresh one checkout's claim and return its private state directory.
pub(crate) fn touch(root: &Path, workspace_root: &Path) -> Result<PathBuf> {
    let checkout = CacheDigest::blake3(workspace_root.as_os_str().as_encoded_bytes());
    let directory = root.join(&checkout.hash[..16]);
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
    crate::util::write_atomic(&directory.join(RECORD_FILE), &serde_json::to_vec(&record)?)?;
    Ok(directory)
}

/// Remove state for deleted or expired checkouts.
pub(crate) fn prune(root: &Path, max_age: Option<Duration>, dry_run: bool) -> Result<PruneOutcome> {
    let listing = match std::fs::read_dir(root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PruneOutcome::default());
        }
        Err(error) => return Err(error.into()),
    };
    let mut outcome = PruneOutcome::default();
    let now = now_secs();
    for entry in listing {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let directory = entry.path();
        let Some(record) = read_record(&directory.join(RECORD_FILE)) else {
            // An unreadable record is not enough evidence to delete data.
            continue;
        };
        let expired =
            max_age.is_some_and(|age| now.saturating_sub(record.updated_secs) > age.as_secs());
        if crate::store::checkout_is_live(&record.workspace_root) && !expired {
            continue;
        }
        let bytes = tree_bytes(&directory);
        if !dry_run {
            match std::fs::remove_dir_all(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    log::warn!(
                        "could not remove incremental state {}: {error}",
                        directory.display()
                    );
                    continue;
                }
            }
        }
        outcome.removed_directories += 1;
        outcome.removed_bytes = outcome.removed_bytes.saturating_add(bytes);
    }
    Ok(outcome)
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
        std::fs::write(state.join("state"), b"incremental").unwrap();
        assert!(read_record(&state.join(RECORD_FILE)).is_some());

        std::fs::remove_dir(&checkout).unwrap();
        assert!(!crate::store::checkout_is_live(&checkout));
        let outcome = prune(cache.path(), None, false).unwrap();

        assert_eq!(outcome.removed_directories, 1);
        assert!(outcome.removed_bytes > 0);
        assert!(!state.exists());
    }
}
