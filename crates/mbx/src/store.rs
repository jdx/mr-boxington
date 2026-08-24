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
        if matches!(remove(path)?, Removal::Removed) {
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
        let evicted = evict_objects(&objects, &rooted, live_bytes, max_bytes, remove)?;
        live_bytes = evicted.remaining_bytes;
        outcome.removed_objects += evicted.removed_objects;
        outcome.removed_bytes += evicted.removed_bytes;
    }

    // An action result whose objects are gone can only produce a miss, so drop
    // it rather than leave the index pointing at nothing. This runs whether or
    // not this call evicted anything, since another process may have, and a
    // store that is over budget on results alone still needs the sweep.
    let cas = LocalCas::new(store);
    for entry in &results {
        if action_result_is_dangling(&cas, &entry.path)? {
            match remove(&entry.path)? {
                Removal::Removed => {
                    live_bytes = live_bytes.saturating_sub(entry.size);
                    outcome.removed_action_results += 1;
                    outcome.removed_bytes += entry.size;
                }
                Removal::Missing => live_bytes = live_bytes.saturating_sub(entry.size),
                Removal::Blocked => {}
            }
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
        // Anything that is not a directory named for an identity was not written
        // by mbx. Both halves matter: `walk_files` tolerates only a missing
        // directory, so a plain file whose name happens to look like an identity
        // would fail the whole scan -- and that scan runs inside every sweep,
        // including the automatic one after a build.
        if !is_task_identity(&name) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
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
pub(crate) fn checkout_is_live(workspace_root: &Path) -> bool {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Removal {
    Removed,
    Missing,
    Blocked,
}

fn remove(path: &Path) -> Result<Removal> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(Removal::Removed),
        // A concurrent build may have evicted or replaced it already.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Removal::Missing),
        // Windows refuses to unlink a file another process holds open, which is
        // what a concurrent build reading this blob looks like. Skipping it
        // leaves the store over budget until the next sweep; failing would
        // abandon the sweep and leave it over budget for longer.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            log::debug!("could not evict {}: {error}", path.display());
            Ok(Removal::Blocked)
        }
        Err(error) => Err(error).wrap_err_with(|| format!("failed to evict {}", path.display())),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ObjectEvictions {
    removed_objects: u64,
    removed_bytes: u64,
    remaining_bytes: u64,
}

/// Evict sorted objects without crossing the checkout-protection boundary
/// when an unrooted object cannot be removed.
fn evict_objects(
    objects: &[Entry],
    rooted: &HashSet<PathBuf>,
    mut live_bytes: u64,
    max_bytes: u64,
    mut remove_file: impl FnMut(&Path) -> Result<Removal>,
) -> Result<ObjectEvictions> {
    let mut outcome = ObjectEvictions::default();
    let mut blocked_unrooted = false;
    for entry in objects {
        if live_bytes <= max_bytes {
            break;
        }
        let is_rooted = rooted.contains(&entry.path);
        if is_rooted && blocked_unrooted {
            // A locked unrooted blob is temporarily part of the irreducible
            // store. Deleting a live checkout's artifacts in its place would
            // invert the protection ordering this collector promises.
            break;
        }
        match remove_file(&entry.path)? {
            Removal::Removed => {
                live_bytes = live_bytes.saturating_sub(entry.size);
                outcome.removed_objects += 1;
                outcome.removed_bytes += entry.size;
            }
            Removal::Missing => live_bytes = live_bytes.saturating_sub(entry.size),
            Removal::Blocked if !is_rooted => blocked_unrooted = true,
            Removal::Blocked => {}
        }
    }
    outcome.remaining_bytes = live_bytes;
    Ok(outcome)
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
#[path = "store_tests.rs"]
mod tests;
