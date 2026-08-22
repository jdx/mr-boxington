//! Store inspection and garbage collection.
//!
//! The store holds four trees: `cas/v1` for content-addressed objects,
//! `action-results/v1` for the results that reference them, `task-manifests/v1`
//! for the prediction index, and `checkouts/v1` for the checkouts that have
//! built each identity. Only the first two are collected for size; manifests
//! are small and are what makes a cold build fast, and a checkout record is
//! dropped when its checkout is gone rather than when the store is full.

use eyre::{Context, Result};
use mbx_cache_core::{
    CacheDigest, CacheDirectory, LocalCas, RemoteActionResult, RustcMetadata, is_task_identity,
    task_manifest_actions,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CAS_DIR: &str = "cas/v1";
const ACTION_RESULTS_DIR: &str = "action-results/v1";
const CHECKOUTS_DIR: &str = "checkouts/v1";
const SWEEP_STAMP: &str = "gc/v1/last-sweep";
const CHECKOUT_RECORD_VERSION: u8 = 1;

/// How long a checkout's claim outlives the last build that renewed it.
///
/// Existence alone is not enough to keep a claim alive. An identity covers one
/// exact command line against one exact lockfile, so a checkout that lives for
/// years accumulates an identity per lockfile it ever had -- and every one of
/// them would go on rooting the actions of a build nobody will run again.
/// Without this bound the rooted set only grows, until everything is rooted and
/// the ordering means nothing. A build renews the claims it uses, so anything
/// this stale belongs to a command that has moved on.
const CHECKOUT_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub objects: u64,
    pub object_bytes: u64,
    pub action_results: u64,
    pub action_result_bytes: u64,
    pub live_checkouts: u64,
    /// Claims that root nothing any more: a checkout that is gone, or one that
    /// is still there but has not renewed this claim inside the retention
    /// window. Reported as stale rather than gone because those are different
    /// things and only one of them means the directory is missing.
    pub stale_checkouts: u64,
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
    pub removed_checkout_records: u64,
    pub removed_bytes: u64,
    pub remaining_bytes: u64,
}

/// One checkout's claim on the actions a build identity recorded.
///
/// The target directory is not read back by anything yet; it is recorded
/// because it is the other half of what the shim mapped, and a record that
/// names only half of it would have to be rewritten to answer where the
/// artifacts went.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutRecord {
    version: u8,
    workspace_root: PathBuf,
    target_dir: PathBuf,
    updated_secs: u64,
}

/// Summarize what the store currently holds.
pub fn stats(store: &Path) -> Result<StoreStats> {
    let objects = walk_files(&store.join(CAS_DIR))?;
    let results = walk_files(&store.join(ACTION_RESULTS_DIR))?;
    let checkouts = scan_checkouts(store)?;
    Ok(StoreStats {
        objects: objects.len() as u64,
        object_bytes: objects.iter().map(|entry| entry.size).sum(),
        action_results: results.len() as u64,
        action_result_bytes: results.iter().map(|entry| entry.size).sum(),
        live_checkouts: checkouts.live_records,
        stale_checkouts: checkouts.stale_records.len() as u64,
    })
}

/// Record that `workspace_root` built `identity`.
///
/// Identities are shared by every checkout of one dependency graph, so this is
/// what later tells the collector whether anything still needs what a build
/// cached: one file per checkout under the identity it built. Writing a whole
/// file per checkout rather than merging a list into one keeps concurrent
/// builds out of each other's way -- there is nothing to merge, so there is no
/// lock and no lost update.
pub fn record_checkout(
    store: &Path,
    identity: &str,
    workspace_root: &Path,
    target_dir: &Path,
) -> Result<()> {
    let record = CheckoutRecord {
        version: CHECKOUT_RECORD_VERSION,
        workspace_root: workspace_root.to_path_buf(),
        target_dir: target_dir.to_path_buf(),
        updated_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or_default(),
    };
    let mut contents = serde_json::to_vec(&record)?;
    contents.push(b'\n');
    crate::util::write_atomic(
        &checkout_record_path(store, identity, workspace_root),
        &contents,
    )
}

fn checkout_record_path(store: &Path, identity: &str, workspace_root: &Path) -> PathBuf {
    // The path is hashed rather than escaped: it is only ever compared against
    // the same hash of the same path, and every filesystem in play disagrees
    // about which characters a name may hold.
    let key = CacheDigest::blake3(workspace_root.to_string_lossy().as_bytes()).hash;
    store
        .join(CHECKOUTS_DIR)
        .join(identity)
        .join(format!("{key}.json"))
}

/// Evict objects until the store fits within `max_bytes`.
///
/// Objects no live checkout can reach are evicted first, so deleting a worktree
/// releases what only that worktree needed. Within each class eviction is
/// least-recently-used by access time where the filesystem records it, falling
/// back to modification time. A restore verifies the blob it serves, which
/// means it reads the whole file and the access time is real -- but only where
/// the mount records one, so the ordering is an approximation either way.
/// Evicting a live object costs a recompile, never correctness.
pub fn gc(store: &Path, max_bytes: u64) -> Result<GcOutcome> {
    let mut objects = walk_files(&store.join(CAS_DIR))?;
    let results = walk_files(&store.join(ACTION_RESULTS_DIR))?;
    let mut live_bytes = objects
        .iter()
        .chain(results.iter())
        .map(|entry| entry.size)
        .sum::<u64>();

    let mut outcome = GcOutcome::default();

    // Prune before deciding what is rooted, so a checkout deleted since the
    // last sweep stops protecting its artifacts during this one.
    let checkouts = scan_checkouts(store)?;
    for path in &checkouts.stale_records {
        if remove(path)? {
            outcome.removed_checkout_records += 1;
        }
        // The identity directory is left behind empty otherwise. It may hold
        // other checkouts, in which case this fails and that is the answer.
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    if live_bytes > max_bytes {
        // Reachability costs a read per action result and per output tree, so
        // it is computed only when something is actually about to be evicted.
        // Under budget a sweep stays a directory walk and nothing more.
        let rooted = rooted_objects(store, &checkouts.live_identities)?;
        // `false` sorts first, so unrooted objects go before rooted ones and
        // each class goes oldest-first. A store with no records at all roots
        // nothing and this is exactly the LRU it was before.
        objects.sort_by_cached_key(|entry| (rooted.contains(&entry.path), entry.used));
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

/// Sweep the store if `interval` has passed since the last attempt.
///
/// The stamp is written before the sweep, not after. Two builds finishing
/// together should cost one sweep between them, and a sweep that dies partway
/// should wait its turn like any other rather than retry on every build.
pub fn sweep_if_due(store: &Path, max_bytes: u64, interval: Duration) -> Result<Option<GcOutcome>> {
    let stamp = store.join(SWEEP_STAMP);
    if let Ok(metadata) = std::fs::metadata(&stamp)
        && let Ok(modified) = metadata.modified()
        && let Ok(since) = modified.elapsed()
        && since < interval
    {
        return Ok(None);
    }
    crate::util::write_atomic(&stamp, b"")?;
    gc(store, max_bytes).map(Some)
}

/// What the checkout registry says about the identities in this store.
struct CheckoutScan {
    live_identities: BTreeSet<String>,
    live_records: u64,
    stale_records: Vec<PathBuf>,
}

fn scan_checkouts(store: &Path) -> Result<CheckoutScan> {
    let root = store.join(CHECKOUTS_DIR);
    let mut scan = CheckoutScan {
        live_identities: BTreeSet::new(),
        live_records: 0,
        stale_records: Vec::new(),
    };
    let listing = match std::fs::read_dir(&root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(scan),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("failed to read {}", root.display()));
        }
    };
    for entry in listing {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // Anything that is not an identity was not written by mbx.
        if !is_task_identity(&name) {
            continue;
        }
        for record in walk_files(&entry.path())? {
            match read_checkout_record(&record.path) {
                Some(checkout) if claim_is_live(&checkout) => {
                    scan.live_identities.insert(name.clone());
                    scan.live_records += 1;
                }
                Some(_) => scan.stale_records.push(record.path),
                // A record this build cannot read claims nothing, but it is not
                // evidence the checkout is gone either, so leave it alone.
                None => {}
            }
        }
    }
    Ok(scan)
}

fn read_checkout_record(path: &Path) -> Option<CheckoutRecord> {
    let bytes = std::fs::read(path).ok()?;
    let record = serde_json::from_slice::<CheckoutRecord>(&bytes).ok()?;
    (record.version == CHECKOUT_RECORD_VERSION).then_some(record)
}

/// Whether a recorded claim still speaks for a checkout that is using this
/// store.
fn claim_is_live(record: &CheckoutRecord) -> bool {
    checkout_is_live(&record.workspace_root) && !claim_has_expired(record)
}

fn claim_has_expired(record: &CheckoutRecord) -> bool {
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return false;
    };
    now.as_secs()
        .saturating_sub(record.updated_secs)
        .gt(&CHECKOUT_RETENTION.as_secs())
}

/// Whether the checkout a record names is still on disk.
///
/// Absence is believed only when the checkout is definitely absent *and* its
/// parent directory is definitely still there. Deleting a worktree, or a whole
/// project directory, leaves the parent behind; a volume being ejected or a
/// network mount going away takes several levels with it, and that is the case
/// worth being careful about -- a temporarily absent mount must not un-root a
/// checkout that is really there.
///
/// So both questions are asked the same way: only a definite answer counts, and
/// an error at either step means live. Being wrong in that direction only
/// delays collection; the other way round throws away a warm cache someone is
/// still using.
fn checkout_is_live(workspace_root: &Path) -> bool {
    if !matches!(workspace_root.try_exists(), Ok(false)) {
        return true;
    }
    match workspace_root.parent() {
        Some(parent) => !matches!(parent.try_exists(), Ok(true)),
        // A root directory has no parent to corroborate anything with.
        None => true,
    }
}

/// CAS paths reachable from the builds that live checkouts still depend on.
///
/// Recursing into output trees is the point: the descriptor blobs are tiny and
/// the leaf artifacts are where the bytes are, so rooting that stopped at the
/// top level would protect nothing worth protecting. Every read here is
/// tolerant -- a blob that has already been evicted, or that no longer parses,
/// simply roots less, and rooting less can only mean collecting sooner.
fn rooted_objects(store: &Path, identities: &BTreeSet<String>) -> Result<HashSet<PathBuf>> {
    let cas = LocalCas::new(store);
    let mut rooted = HashSet::new();
    let mut visited = BTreeSet::new();
    for identity in identities {
        // One manifest this build cannot read is not worth abandoning a sweep
        // over. It roots nothing, which can only mean collecting sooner.
        let actions = match task_manifest_actions(store, identity) {
            Ok(actions) => actions,
            Err(error) => {
                log::debug!("could not read the manifest for {identity}: {error}");
                continue;
            }
        };
        for action in actions {
            let Some(result) = read_action_result(store, &action) else {
                continue;
            };
            root_digest(&cas, &mut rooted, &result.action);
            if let Some(metadata) = &result.metadata {
                root_digest(&cas, &mut rooted, metadata);
                // The metadata blob is a descriptor too: it names the captured
                // stdout and stderr, and a restore reads both. Stopping at the
                // descriptor would leave the diagnostics of every action
                // unrooted -- including the one empty blob that every silent
                // compilation shares, which is enough on its own to turn a
                // rooted hit back into a miss.
                if let Some(rustc) = read_rustc_metadata(&cas, metadata) {
                    root_digest(&cas, &mut rooted, &rustc.stdout);
                    root_digest(&cas, &mut rooted, &rustc.stderr);
                }
            }
            if let Some(output_root) = &result.output_root {
                root_digest(&cas, &mut rooted, output_root);
                root_tree(&cas, output_root, &mut rooted, &mut visited);
            }
        }
    }
    Ok(rooted)
}

fn read_action_result(store: &Path, action: &CacheDigest) -> Option<RemoteActionResult> {
    let path = mbx_cache_core::LocalActionCache::new(store)
        .path_for(action)
        .ok()?;
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn root_tree(
    cas: &LocalCas,
    output_root: &CacheDigest,
    rooted: &mut HashSet<PathBuf>,
    visited: &mut BTreeSet<CacheDigest>,
) {
    let mut pending = vec![output_root.clone()];
    while let Some(digest) = pending.pop() {
        if !visited.insert(digest.clone()) {
            continue;
        }
        let Some(directory) = read_directory(cas, &digest) else {
            continue;
        };
        for file in &directory.files {
            root_digest(cas, rooted, &file.digest);
        }
        for child in &directory.directories {
            root_digest(cas, rooted, &child.digest);
            pending.push(child.digest.clone());
        }
    }
}

fn read_rustc_metadata(cas: &LocalCas, digest: &CacheDigest) -> Option<RustcMetadata> {
    let bytes = std::fs::read(cas.path_for(digest).ok()?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn read_directory(cas: &LocalCas, digest: &CacheDigest) -> Option<CacheDirectory> {
    let bytes = std::fs::read(cas.path_for(digest).ok()?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn root_digest(cas: &LocalCas, rooted: &mut HashSet<PathBuf>, digest: &CacheDigest) {
    if let Ok(path) = cas.path_for(digest) {
        rooted.insert(path);
    }
}

fn remove(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        // A concurrent build may have evicted or replaced it already.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        // Windows refuses to unlink a file another process holds open, which is
        // what a concurrent build reading this blob looks like. Skipping it
        // leaves the store over budget until the next sweep; failing would
        // abandon the sweep and leave it over budget for longer.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            log::debug!("could not evict {}: {error}", path.display());
            Ok(false)
        }
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
/// Presence is all that is checked, not content. Verifying would re-hash a
/// large fraction of the store on every sweep, which a chore could afford and
/// an automatic sweep cannot; it would also refresh the access time of every
/// descriptor blob while leaving the artifacts alone, biasing the very
/// ordering this module depends on. A blob that is present but corrupt now
/// keeps its result and turns into a miss on restore, and both CAS write paths
/// republish over a corrupt blob rather than trusting it.
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
        match cas.path_for(digest) {
            Ok(path) => {
                if !path.exists() {
                    return Ok(true);
                }
            }
            // A digest the CAS cannot even address is not one it can hold.
            Err(_) => return Ok(true),
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
    use mbx_cache_core::{ActionPrediction, CacheFileNode, LocalActionCache};

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

    /// Publish an action result whose output tree holds `outputs`.
    fn store_result(store: &Path, name: &str, outputs: &[CacheDigest]) -> CacheDigest {
        let directory = CacheDirectory {
            directories: Vec::new(),
            files: outputs
                .iter()
                .enumerate()
                .map(|(index, digest)| CacheFileNode {
                    digest: digest.clone(),
                    executable: false,
                    mode: 0o644,
                    name: format!("output-{index}"),
                })
                .collect(),
            symlinks: Vec::new(),
            version: 1,
        };
        let encoded = serde_json::to_vec(&directory).unwrap();
        let output_root = store_object(store, &encoded);
        let action = store_object(store, name.as_bytes());
        LocalActionCache::new(store)
            .store(&RemoteActionResult {
                version: 1,
                action: action.clone(),
                metadata: None,
                output_root: Some(output_root),
            })
            .unwrap();
        action
    }

    /// Record a checkout of `identity` and the manifest that roots `actions`.
    ///
    /// The agent only accepts predictions over its socket, so the manifest is
    /// written here instead. The predictions come from the public type rather
    /// than hand-rolled JSON, leaving only the two wrapper fields to drift --
    /// and `mbx_cache_core`'s own tests write a manifest through the agent and
    /// read it back with the same accessor this uses, so drift shows up there.
    fn record_build(store: &Path, identity: &str, workspace_root: &Path, actions: &[CacheDigest]) {
        record_checkout(
            store,
            identity,
            workspace_root,
            &workspace_root.join("target"),
        )
        .unwrap();
        let predictions = actions
            .iter()
            .enumerate()
            .map(|(index, action)| ActionPrediction {
                invocation: CacheDigest::blake3(format!("{identity}-{index}").as_bytes()),
                action: action.clone(),
                adapter: "rustc".into(),
                payload: "{}".into(),
            })
            .collect::<Vec<_>>();
        let manifest = serde_json::json!({
            "version": 1,
            "task": identity,
            "predictions": predictions,
        });
        let path = store
            .join("task-manifests")
            .join("v1")
            .join(format!("{identity}.json"));
        crate::util::write_atomic(&path, &serde_json::to_vec(&manifest).unwrap()).unwrap();
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
    fn keeps_an_action_result_whose_blob_is_present_but_corrupt() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path();
        let action = store_object(store, b"action key");
        LocalActionCache::new(store)
            .store(&RemoteActionResult {
                version: 1,
                action: action.clone(),
                metadata: None,
                output_root: None,
            })
            .unwrap();
        let path = LocalCas::new(store).path_for(&action).unwrap();
        std::fs::write(&path, b"corrupted!").unwrap();

        let outcome = gc(store, u64::MAX).unwrap();

        // The sweep does not read content, so this result survives and costs a
        // miss on restore. Verifying instead would re-hash the whole store.
        assert_eq!(outcome.removed_action_results, 0);
        assert!(path.exists());
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

    #[test]
    fn evicts_objects_no_live_checkout_needs_before_older_rooted_ones() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let live = directory.path().join("live");
        let deleted = directory.path().join("deleted");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(&deleted).unwrap();

        let kept = store_object(&store, b"0123456789");
        let dropped = store_object(&store, b"abcdefghij");
        record_build(
            &store,
            &"a".repeat(64),
            &live,
            &[store_result(
                &store,
                "live action",
                std::slice::from_ref(&kept),
            )],
        );
        record_build(
            &store,
            &"b".repeat(64),
            &deleted,
            &[store_result(
                &store,
                "deleted action",
                std::slice::from_ref(&dropped),
            )],
        );
        // The rooted object is the older one, so plain LRU would take it first.
        age(&store, &kept, Duration::from_secs(60 * 60));
        age(&store, &dropped, Duration::from_secs(1));
        std::fs::remove_dir_all(&deleted).unwrap();

        gc(&store, stats(&store).unwrap().total_bytes() - 10).unwrap();

        let cas = LocalCas::new(&store);
        assert!(
            cas.find(&kept).unwrap().is_some(),
            "a live checkout still needs this object"
        );
        assert!(
            cas.find(&dropped).unwrap().is_none(),
            "nothing that still exists needs this object"
        );
    }

    #[test]
    fn keeps_rooting_when_a_sibling_worktree_survives() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();

        // Worktrees of one dependency graph share an identity by design, so the
        // survivor's claim has to keep the whole identity rooted.
        let identity = "c".repeat(64);
        let shared = store_object(&store, b"shared artifact");
        let action = store_result(&store, "shared action", std::slice::from_ref(&shared));
        record_build(&store, &identity, &first, std::slice::from_ref(&action));
        record_build(&store, &identity, &second, &[action]);
        let spare = store_object(&store, b"unrooted spare");
        std::fs::remove_dir_all(&second).unwrap();

        let outcome = gc(&store, stats(&store).unwrap().total_bytes() - 1).unwrap();

        assert_eq!(outcome.removed_checkout_records, 1);
        let cas = LocalCas::new(&store);
        assert!(cas.find(&shared).unwrap().is_some());
        assert!(cas.find(&spare).unwrap().is_none());
    }

    #[test]
    fn roots_objects_nested_in_an_output_tree() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let checkout = directory.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();

        // The leaf artifact, not the descriptor, is where a build's bytes are.
        let artifact = store_object(&store, b"the compiled artifact");
        let action = store_result(&store, "an action", std::slice::from_ref(&artifact));
        record_build(&store, &"d".repeat(64), &checkout, &[action]);
        let spare = store_object(&store, b"unrooted spare");

        gc(&store, stats(&store).unwrap().total_bytes() - 1).unwrap();

        let cas = LocalCas::new(&store);
        assert!(cas.find(&artifact).unwrap().is_some());
        assert!(cas.find(&spare).unwrap().is_none());
    }

    #[test]
    fn treats_a_store_with_no_checkout_records_as_plain_lru() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path();
        // A store written before checkouts were recorded roots nothing, so it
        // has to keep collecting exactly as it did.
        let old = store_object(store, b"0123456789");
        let recent = store_object(store, b"abcdefghij");
        age(store, &old, Duration::from_secs(60 * 60));
        age(store, &recent, Duration::from_secs(1));

        gc(store, 10).unwrap();

        let cas = LocalCas::new(store);
        assert!(cas.find(&old).unwrap().is_none());
        assert!(cas.find(&recent).unwrap().is_some());
    }

    #[test]
    fn drops_checkout_records_whose_worktree_is_gone() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let gone = directory.path().join("gone");
        std::fs::create_dir_all(&gone).unwrap();
        record_checkout(&store, &"e".repeat(64), &gone, &gone.join("target")).unwrap();

        assert_eq!(stats(&store).unwrap().live_checkouts, 1);
        std::fs::remove_dir_all(&gone).unwrap();
        let after = stats(&store).unwrap();
        assert_eq!(after.stale_checkouts, 1);
        assert_eq!(after.live_checkouts, 0);

        let outcome = gc(&store, u64::MAX).unwrap();

        assert_eq!(outcome.removed_checkout_records, 1);
        assert_eq!(stats(&store).unwrap(), StoreStats::default());
    }

    #[test]
    fn keeps_a_checkout_recorded_while_its_worktree_exists() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let live = directory.path().join("live");
        std::fs::create_dir_all(&live).unwrap();
        record_checkout(&store, &"f".repeat(64), &live, &live.join("target")).unwrap();

        let outcome = gc(&store, u64::MAX).unwrap();

        assert_eq!(outcome.removed_checkout_records, 0);
        assert_eq!(stats(&store).unwrap().live_checkouts, 1);
    }

    #[test]
    fn forgets_a_claim_no_build_has_renewed() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let checkout = directory.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();

        // The checkout is still there, but this identity names a command line
        // against a lockfile that has since moved on, so nothing renews it.
        let identity = "2".repeat(64);
        let orphan = store_object(&store, b"what that command built");
        record_build(
            &store,
            &identity,
            &checkout,
            &[store_result(
                &store,
                "stale action",
                std::slice::from_ref(&orphan),
            )],
        );
        let stale = CheckoutRecord {
            version: CHECKOUT_RECORD_VERSION,
            workspace_root: checkout.clone(),
            target_dir: checkout.join("target"),
            updated_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                - CHECKOUT_RETENTION.as_secs()
                - 1,
        };
        crate::util::write_atomic(
            &checkout_record_path(&store, &identity, &checkout),
            &serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();

        assert_eq!(stats(&store).unwrap().stale_checkouts, 1);
        gc(&store, stats(&store).unwrap().total_bytes() - 1).unwrap();

        assert!(
            LocalCas::new(&store).find(&orphan).unwrap().is_none(),
            "an expired claim roots nothing"
        );
    }

    #[test]
    fn keeps_a_checkout_whose_absence_it_cannot_corroborate() {
        let directory = tempfile::tempdir().unwrap();

        // A checkout that is gone from a parent that is still there is the one
        // case worth believing: that is what deleting a worktree looks like.
        let parent = directory.path().join("worktrees");
        std::fs::create_dir_all(&parent).unwrap();
        assert!(!checkout_is_live(&parent.join("removed")));

        // A checkout whose parent went with it looks like an ejected volume or a
        // mount that is temporarily away, and un-rooting those would throw away
        // a cache somebody is still using.
        assert!(checkout_is_live(
            &directory.path().join("gone/deeper/still")
        ));

        // And anything still on disk is live whatever its parent says.
        assert!(checkout_is_live(directory.path()));
    }

    #[test]
    fn sweeps_only_once_within_the_interval() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path();
        store_object(store, b"kept");

        assert!(
            sweep_if_due(store, u64::MAX, Duration::from_secs(3600))
                .unwrap()
                .is_some()
        );
        assert!(
            sweep_if_due(store, u64::MAX, Duration::from_secs(3600))
                .unwrap()
                .is_none(),
            "the interval has not passed"
        );
        assert!(
            sweep_if_due(store, u64::MAX, Duration::ZERO)
                .unwrap()
                .is_some(),
            "a zero interval always sweeps"
        );
    }

    #[test]
    fn does_not_count_its_own_bookkeeping_against_the_budget() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let checkout = directory.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        record_checkout(&store, &"1".repeat(64), &checkout, &checkout.join("target")).unwrap();
        sweep_if_due(&store, u64::MAX, Duration::ZERO).unwrap();

        // Checkout records and the sweep stamp live outside the collected
        // trees; counting them would make the budget mean something else.
        assert_eq!(stats(&store).unwrap().total_bytes(), 0);
    }
}
