use crate::uploads::{ConnectionUploads, UploadQueue, UploadSink};
use crate::{
    ActionPrediction, BlobPackLimits, CacheDigest, CacheDirectory, LocalActionCache, LocalCas,
    MAX_STAGED_BLOB_PACK_BYTES, MAX_STAGED_BLOB_PACK_ITEMS, ManifestPutOutcome, RemoteActionResult,
    RemoteCacheClient, RemoteCacheMode, RustcMetadata, TaskActionManifest, blob_pack_chunk,
    canonical_json,
};
use eyre::{Context, Result, bail};
use futures_util::{FutureExt, StreamExt, future::BoxFuture, stream};
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

const MAX_EXECUTABLE_IDENTITIES: usize = 64;
const MAX_EXECUTABLE_IDENTITY_SIZE: usize = 64 * 1024;
const MAX_EXECUTABLE_IDENTITY_BYTES: usize = 256 * 1024;
const TASK_ACTION_MANIFEST_VERSION: u8 = 1;
const MAX_TASK_ACTION_PREDICTIONS: usize = 16 * 1024;
/// Longest shim diagnostic the agent accepts, in bytes.
const MAX_WARNING_BYTES: usize = 4 * 1024;
/// Most distinct shim diagnostics one session surfaces before going quiet.
///
/// A failure mode that repeats across a build tends to repeat with the same
/// message, which deduplication already collapses; the cap only guards
/// against a message that embeds something unique per compilation.
const MAX_WARNINGS: usize = 128;
const MAX_REMOTE_TRANSFERS: usize = 64;
const MAX_PREFETCH_TRANSFERS: usize = 48;
const MAX_PREFETCH_ACTION_BATCH: usize = 256;
/// Batched action lookups issued at once, well inside the transfer budget: each
/// asks about hundreds of actions, so a handful covers a large workspace.
const MAX_PREFETCH_BATCH_LOOKUPS: usize = 4;
const PREFETCH_ACTION_BATCH_DELAY: Duration = Duration::from_millis(5);
const MAX_PREFETCH_DIRECTORY_OBJECTS: usize = 100_000;
const MAX_PREFETCH_OBJECTS_PER_WAVE: usize = 100_000;
const DEFAULT_MAX_REMOTE_DOWNLOAD_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Remote action-cache access owned by one task session.
pub struct AgentRemoteCache {
    /// Remote protocol client used by the agent.
    pub client: RemoteCacheClient,
    /// Permitted remote read/write operations.
    pub mode: RemoteCacheMode,
    /// Directory used for verified downloads before CAS ingestion.
    pub staging_dir: PathBuf,
}

/// Wire protocol version used between an in-process cache agent and its shims.
pub const AGENT_PROTOCOL_VERSION: u8 = 4;
/// Largest single protocol request the agent will read.
///
/// Requests are small JSON objects; the largest legitimate ones carry an output
/// tree or a batch of digests, which stay far below this.
const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

/// A request accepted by the task-scoped cache agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentRequest {
    /// Negotiate protocol and application versions.
    Hello {
        /// Agent protocol version understood by the caller.
        protocol: u8,
        /// Human-readable mbx client version.
        client_version: String,
    },
    /// Begin a prediction-manifest run for a stable Cargo or task identity.
    BeginTask {
        /// Stable 64-character lowercase hexadecimal task identity.
        task: String,
    },
    /// Commit predictions collected by an earlier [`Self::BeginTask`].
    CommitTask {
        /// Opaque run identifier returned by the agent.
        run: String,
    },
    /// Resolve a blob to a session-verified local CAS path.
    FindBlob {
        /// Blob to resolve.
        digest: CacheDigest,
    },
    /// Resolve blobs to session-verified local CAS paths.
    FindBlobs {
        /// Blobs to resolve, preserving request order in the response.
        digests: Vec<CacheDigest>,
    },
    /// Import a file into the local content-addressed store.
    StoreBlob {
        /// Digest the source must match.
        digest: CacheDigest,
        /// File to verify and import.
        source: PathBuf,
    },
    /// Look up an action-result record.
    FindActionResult {
        /// Action digest to resolve.
        action: CacheDigest,
    },
    /// Account for a successfully restored cache hit.
    RecordActionHit {
        /// Action that supplied the outputs.
        action: CacheDigest,
        /// Restoration work performed by the adapter.
        restore: RestoreStats,
        /// Compiler crate name, when the invocation supplied one.
        #[serde(default)]
        crate_name: Option<String>,
    },
    /// A compilation the adapter declined to cache, grouped by reason.
    RecordBypass {
        /// Stable, low-cardinality bypass-reason name.
        kind: String,
    },
    /// A compilation the adapter could not look up, having no key to look up
    /// with. Distinct from a bypass: these are cached once compiled.
    RecordUnconsulted,
    /// Account for one real compiler invocation performed by the adapter.
    RecordCompilerInvocation {
        /// Stable outcome category such as `miss`, `unconsulted`, `bypass`, or
        /// `incremental` for a compilation the adapter deliberately ran with
        /// incremental state instead of publishing.
        outcome: String,
        /// Compiler crate name, when the invocation supplied one.
        crate_name: Option<String>,
        /// Wall time spent running the compiler.
        duration_ns: u64,
    },
    /// Account for a cache hit that was rebuilt for correctness verification.
    RecordActionVerification {
        /// Whether rebuilt and cached outputs matched.
        matched: bool,
        /// Restoration work performed before rebuilding.
        restore: RestoreStats,
    },
    /// Store an action-result record locally and enqueue remote publication.
    StoreActionResult {
        /// Action-result record to store.
        result: RemoteActionResult,
    },
    /// Find an earlier input prediction for a task and invocation.
    FindActionPrediction {
        /// Stable task identity.
        task: String,
        /// Digest of the compiler invocation without discovered inputs.
        invocation: CacheDigest,
    },
    /// Record an input prediction after a successful compilation.
    RecordActionPrediction {
        /// Stable task identity.
        task: String,
        /// Adapter-owned prediction record.
        prediction: ActionPrediction,
    },
    /// Find cached identity output for an executable and environment.
    FindExecutableIdentity {
        /// Executable whose identity command would run.
        executable: PathBuf,
        /// Environment variables affecting identity output.
        environment: BTreeMap<String, Option<String>>,
    },
    /// Cache identity output for an executable and environment.
    StoreExecutableIdentity {
        /// Executable whose identity command ran.
        executable: PathBuf,
        /// Environment variables affecting identity output.
        environment: BTreeMap<String, Option<String>>,
        /// Captured identity-command standard output.
        stdout: Vec<u8>,
    },
    /// Surface a shim diagnostic through the session that owns the build.
    ///
    /// A shim must not print diagnostics itself: its stderr belongs to the
    /// compiler it stands in for, and build scripts read that stream as part
    /// of the compiler's answer -- cc-rs treats any stderr output from a flag
    /// probe as "unsupported", which changes the flags of every compilation
    /// that follows and, with them, every action key the build produces.
    ///
    /// Appended rather than grouped with the other `Record*` requests it
    /// belongs beside: these variants carry no `repr`, so inserting one moves
    /// the discriminant of every variant after it, and a break nobody asked
    /// for is worth less than the grouping. New variants go here.
    RecordWarning {
        /// Human-readable single-line diagnostic.
        message: String,
    },
}

/// Local output restoration work performed by one action-cache adapter hit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreStats {
    /// Cumulative time spent materializing and validating output files.
    pub duration_ns: u64,
    /// Compiler wall time recorded when this action was originally produced.
    /// Zero means no timing hint was available.
    pub avoided_compiler_duration_ns: u64,
    /// Number of compiler output files restored.
    pub output_files: u64,
    /// Declared size of compiler output files restored.
    pub output_bytes: u64,
    /// Number of restored output files that share data blocks with the CAS.
    pub reflinked_output_files: u64,
    /// Declared size of restored outputs that share data blocks with the CAS.
    pub reflinked_output_bytes: u64,
    /// Number of restored output files that required a byte-for-byte copy.
    pub copied_output_files: u64,
    /// Declared size of restored outputs that required a byte-for-byte copy.
    pub copied_output_bytes: u64,
}

/// One accounted cache decision, as it happens.
///
/// The agent already folds every one of these into [`AgentStats`]; an observer
/// sees the same decisions individually, before that summing loses the crate
/// they belong to. Delivered synchronously from the request handler, so an
/// observer that blocks slows the build it is watching.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AgentEvent {
    /// An action's outputs were restored from cache.
    ActionHit {
        /// Compiler crate name, when the invocation supplied one.
        crate_name: Option<String>,
        /// Restoration work performed by the adapter.
        restore: RestoreStats,
    },
    /// A compilation the adapter declined to cache.
    Bypass {
        /// Stable, low-cardinality bypass-reason name.
        kind: String,
    },
    /// A compilation no lookup was possible for.
    Unconsulted,
    /// A real compiler invocation ran.
    CompilerInvocation {
        /// Stable outcome category such as `miss`, `unconsulted`, or `bypass`.
        outcome: String,
        /// Compiler crate name, when the invocation supplied one.
        crate_name: Option<String>,
        /// Wall time spent running the compiler.
        duration_ns: u64,
    },
    /// A hit was rebuilt to verify it.
    Verification {
        /// Whether rebuilt and cached outputs matched.
        matched: bool,
        /// Restoration work performed before rebuilding.
        restore: RestoreStats,
    },
    /// A shim reported a diagnostic for the session to surface.
    Warning {
        /// Human-readable single-line diagnostic.
        message: String,
    },
}

/// A sink for [`AgentEvent`]s observed during one session.
pub trait AgentEventObserver: Send + Sync {
    /// Handle one event. Must not panic, and should not block.
    fn event(&self, event: AgentEvent);
}

/// A response returned by the task-scoped cache agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentResponse {
    /// Successful protocol negotiation.
    Hello {
        /// Agent protocol version.
        protocol: u8,
        /// Human-readable agent version.
        agent_version: String,
    },
    /// A task prediction run was begun.
    TaskBegun {
        /// Opaque run identifier to pass to compiler shims and commit later.
        run: String,
    },
    /// A task prediction run was committed.
    TaskCommitted,
    /// A local CAS path already verified against the requested digest.
    Blob {
        /// Verified local path, or `None` on a cache miss.
        path: Option<PathBuf>,
    },
    /// Local CAS paths already verified against the requested digests.
    Blobs {
        /// Verified local paths or misses, in request order.
        paths: Vec<Option<PathBuf>>,
    },
    /// A blob was stored locally.
    Stored {
        /// Path of the stored object in the local CAS.
        path: PathBuf,
    },
    /// Result of an action lookup.
    ActionResult {
        /// Validated action result, or `None` on a cache miss.
        result: Option<RemoteActionResult>,
    },
    /// Hit statistics were updated.
    ActionHitRecorded,
    /// Verification statistics were updated.
    ActionVerificationRecorded,
    /// Bypass statistics were updated.
    BypassRecorded,
    /// Unconsulted-compilation statistics were updated.
    UnconsultedRecorded,
    /// Compiler invocation accounting was recorded.
    CompilerInvocationRecorded,
    /// An action result was stored.
    ActionStored {
        /// Path of the stored local action-result record.
        path: PathBuf,
    },
    /// Result of an input-prediction lookup.
    ActionPrediction {
        /// Matching prediction, or `None` when none is known.
        prediction: Option<ActionPrediction>,
    },
    /// An input prediction was recorded.
    ActionPredictionRecorded,
    /// Result of an executable-identity lookup.
    ExecutableIdentity {
        /// Captured output, or `None` when no identity is cached.
        stdout: Option<Vec<u8>>,
    },
    /// The request failed without terminating the agent connection.
    Error {
        /// Human-readable failure description.
        message: String,
    },
    /// A shim diagnostic was accepted for the session to surface.
    ///
    /// Sits past `Error` for the reason [`AgentRequest::RecordWarning`] sits
    /// last: anywhere earlier renumbers the variants below it. New variants go
    /// here.
    WarningRecorded,
}

/// Aggregate cache activity for one task session.
///
/// The agent produces these; nothing outside this crate has cause to build one.
/// Saying so keeps a new counter from being a breaking change, which is what
/// this type exists to accumulate -- reach for [`AgentStats::default`] and
/// assign the fields a test needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AgentStats {
    /// End-to-end lifetime of the task-scoped cache session.
    pub session_duration_ns: u64,
    /// Number of action-result lookups.
    pub lookups: u64,
    /// Compilations no action-result lookup was possible for, because no usable
    /// action key was available.
    ///
    /// Counted separately from a miss, which is a lookup that found nothing.
    /// Both compile, but only a miss says a lookup happened.
    pub unconsulted: u64,
    /// Number of lookups that found a valid local action result.
    pub hits: u64,
    /// Number of newly stored content-addressed objects.
    pub stores: u64,
    /// Total size of newly stored objects.
    pub stored_bytes: u64,
    /// Number of cache hits compiled again for qualification.
    pub verifications: u64,
    /// Number of qualification builds that diverged from the cached result.
    pub divergences: u64,
    /// CAS payload bytes downloaded from the remote cache.
    pub downloaded_bytes: u64,
    /// CAS payload bytes uploaded to the remote cache.
    pub uploaded_bytes: u64,
    /// Objects published to the remote cache after the build asked for them.
    ///
    /// A store request returns once the object is in the local CAS, so the
    /// upload it implies happens off the build's critical path. This counts the
    /// blobs and action results that publication actually completed for.
    pub background_uploads: u64,
    /// Queued uploads that did not publish, having been reported and recovered
    /// from.
    pub background_upload_failures: u64,
    /// Framed requests that published several blobs at once.
    pub remote_blob_pack_uploads: u64,
    /// Blobs published through those framed requests.
    pub remote_blob_pack_upload_blobs: u64,
    /// Time the session spent waiting for queued uploads once the build ended.
    pub upload_drain_duration_ns: u64,
    /// Complete actions staged before an adapter requested them.
    pub prefetched_actions: u64,
    /// Predictions carried by the task manifest this session loaded.
    ///
    /// Zero means no earlier build left a manifest behind -- a genuinely cold
    /// start. Together with `lookups`, this is what tells a session that loaded
    /// hundreds of predictions and matched none of them apart from one that had
    /// nothing to match against; the first means the invocations changed (a
    /// compiler update does this to every one of them at once), the second that
    /// the store was empty.
    pub predictions_loaded: u64,
    /// Compilations that were not cacheable, counted by reason.
    pub bypasses: BTreeMap<String, u64>,
    /// Estimated compiler time avoided by restored action hits.
    pub avoided_compiler_duration_ns: u64,
    /// Real compiler work performed in this session, grouped by outcome.
    pub compiler: BTreeMap<String, CompilerStats>,
    /// Cumulative real compiler time by crate name.
    pub slow_compilations: BTreeMap<String, u64>,
    /// Remote cache operations that failed and were degraded to a local result.
    ///
    /// A remote cache that cannot be reached, or that answers in a way this
    /// client refuses, costs hit rate rather than correctness, so every one of
    /// these is recovered from rather than raised. Counting them is what keeps a
    /// remote that is failing every request from reading as one that merely had
    /// nothing to offer.
    pub remote_failures: u64,
    /// Number of task manifest requests made to the remote cache.
    pub remote_manifest_lookups: u64,
    /// Cumulative time spent requesting remote task manifests.
    pub remote_manifest_lookup_duration_ns: u64,
    /// Number of action-result requests made to the remote cache.
    pub remote_action_lookups: u64,
    /// Cumulative time spent requesting remote action results.
    pub remote_action_lookup_duration_ns: u64,
    /// Number of blob requests made to the remote cache.
    pub remote_blob_requests: u64,
    /// Number of packed blob requests made to the remote cache.
    pub remote_blob_pack_requests: u64,
    /// Number of verified blobs received through packed responses.
    pub remote_blob_pack_blobs: u64,
    /// Cumulative time spent downloading and verifying remote blobs.
    pub remote_blob_transfer_duration_ns: u64,
    /// Cumulative time spent ingesting downloaded blobs into the local CAS.
    pub local_cas_write_duration_ns: u64,
    /// Number of speculative prefetch runs started for task manifests.
    pub prefetch_runs: u64,
    /// Cumulative wall time of speculative task-manifest prefetch runs.
    pub prefetch_duration_ns: u64,
    /// Cumulative time spent staging or materializing and validating cached outputs.
    pub materialization_duration_ns: u64,
    /// Number of compiler output files restored from action hits.
    pub restored_output_files: u64,
    /// Declared size of compiler output files restored from action hits.
    pub restored_output_bytes: u64,
    /// Number of restored output files materialized with filesystem reflinks.
    pub reflinked_output_files: u64,
    /// Declared size of outputs materialized with filesystem reflinks.
    pub reflinked_output_bytes: u64,
    /// Number of restored output files materialized by copying their bytes.
    pub copied_output_files: u64,
    /// Declared size of outputs materialized by copying their bytes.
    pub copied_output_bytes: u64,
}

/// Count and cumulative wall time for one compiler-invocation outcome.
///
/// Non-exhaustive for the same reason as [`AgentStats`], which holds these:
/// leaving the outer bag open does not help if describing an outcome in more
/// detail still breaks the type inside it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CompilerStats {
    /// Number of compiler invocations observed.
    pub invocations: u64,
    /// Cumulative wall time spent in those invocations.
    pub duration_ns: u64,
}

impl CompilerStats {
    /// The counts observed for one outcome.
    pub fn new(invocations: u64, duration_ns: u64) -> Self {
        Self {
            invocations,
            duration_ns,
        }
    }
}

#[derive(Default)]
struct AtomicAgentStats {
    lookups: AtomicU64,
    unconsulted: AtomicU64,
    hits: AtomicU64,
    stores: AtomicU64,
    stored_bytes: AtomicU64,
    verifications: AtomicU64,
    divergences: AtomicU64,
    downloaded_bytes: AtomicU64,
    uploaded_bytes: AtomicU64,
    background_uploads: AtomicU64,
    background_upload_failures: AtomicU64,
    remote_blob_pack_uploads: AtomicU64,
    remote_blob_pack_upload_blobs: AtomicU64,
    upload_drain_duration_ns: AtomicU64,
    prefetched_actions: AtomicU64,
    predictions_loaded: AtomicU64,
    remote_failures: AtomicU64,
    remote_manifest_lookups: AtomicU64,
    remote_manifest_lookup_duration_ns: AtomicU64,
    remote_action_lookups: AtomicU64,
    remote_action_lookup_duration_ns: AtomicU64,
    remote_blob_requests: AtomicU64,
    remote_blob_pack_requests: AtomicU64,
    remote_blob_pack_blobs: AtomicU64,
    remote_blob_transfer_duration_ns: AtomicU64,
    local_cas_write_duration_ns: AtomicU64,
    prefetch_runs: AtomicU64,
    prefetch_duration_ns: AtomicU64,
    materialization_duration_ns: AtomicU64,
    bypasses: Mutex<BTreeMap<String, u64>>,
    avoided_compiler_duration_ns: AtomicU64,
    compiler: Mutex<BTreeMap<String, CompilerStats>>,
    slow_compilations: Mutex<BTreeMap<String, u64>>,
    restored_output_files: AtomicU64,
    restored_output_bytes: AtomicU64,
    reflinked_output_files: AtomicU64,
    reflinked_output_bytes: AtomicU64,
    copied_output_files: AtomicU64,
    copied_output_bytes: AtomicU64,
}

struct AtomicDurationTimer<'a> {
    started: Instant,
    target: &'a AtomicU64,
}

impl<'a> AtomicDurationTimer<'a> {
    fn start(target: &'a AtomicU64) -> Self {
        Self {
            started: Instant::now(),
            target,
        }
    }
}

impl Drop for AtomicDurationTimer<'_> {
    fn drop(&mut self) {
        atomic_saturating_add(self.target, duration_ns(self.started));
    }
}

fn duration_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX)
}

fn validate_crate_name(crate_name: Option<&str>) -> Result<()> {
    if let Some(crate_name) = crate_name
        && (crate_name.len() > 256 || crate_name.contains(['\0', '\n', '\r']))
    {
        bail!("invalid compiler crate name");
    }
    Ok(())
}

fn atomic_saturating_add(target: &AtomicU64, value: u64) {
    let _ = target.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(value))
    });
}

fn queue_prefetch_digest(
    verified: &BTreeMap<CacheDigest, PathBuf>,
    pending: &mut BTreeMap<CacheDigest, ()>,
    digest: CacheDigest,
) {
    if verified.contains_key(&digest) || pending.contains_key(&digest) {
        return;
    }
    pending.insert(digest, ());
}

fn queue_prefetch_directory(
    seen: &BTreeMap<CacheDigest, ()>,
    pending: &mut BTreeMap<CacheDigest, ()>,
    digest: CacheDigest,
    limit: usize,
) -> bool {
    if seen.contains_key(&digest) || pending.contains_key(&digest) {
        return true;
    }
    if seen.len().saturating_add(pending.len()) >= limit {
        return false;
    }
    pending.insert(digest, ());
    true
}

/// Shared state for an agent hosted by the process that owns a build session.
///
/// Transport listeners deliberately live in the embedder so the session
/// lifecycle owns them. This type only contains ecosystem-independent CAS and
/// protocol logic.
#[derive(Clone)]
pub struct CacheAgent {
    cas: LocalCas,
    actions: LocalActionCache,
    verified_blobs: Arc<Mutex<BTreeMap<CacheDigest, VerifiedBlob>>>,
    version: Arc<str>,
    write_locks: Arc<Mutex<BTreeMap<CacheDigest, Weak<tokio::sync::Mutex<()>>>>>,
    action_locks: Arc<Mutex<BTreeMap<CacheDigest, Weak<tokio::sync::Mutex<()>>>>>,
    stats: Arc<AtomicAgentStats>,
    observer: Option<Arc<dyn AgentEventObserver>>,
    executable_identities: Arc<Mutex<BTreeMap<ExecutableIdentityKey, Vec<u8>>>>,
    manifest_dir: Arc<PathBuf>,
    task_actions: Arc<Mutex<BTreeMap<String, TaskActionState>>>,
    next_task_run: Arc<AtomicU64>,
    manifest_write_lock: Arc<Mutex<()>>,
    remote: Option<Arc<RemoteCacheClient>>,
    remote_mode: RemoteCacheMode,
    remote_staging_dir: Arc<PathBuf>,
    remote_download_limit: u64,
    remote_download_bytes: Arc<AtomicU64>,
    pending_remote_actions: Arc<Mutex<BTreeMap<CacheDigest, RemoteActionResult>>>,
    remote_transfers: Arc<tokio::sync::Semaphore>,
    prefetch_transfers: Arc<tokio::sync::Semaphore>,
    prefetch_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// Distinct shim diagnostics already surfaced, so a warning that fires
    /// once per compilation is printed once per session.
    warnings: Arc<Mutex<BTreeSet<String>>>,
    /// Deferred remote publication, present only when the session may write.
    uploads: Option<UploadQueue>,
}

/// Records background upload activity against a session's statistics.
struct AgentUploadSink {
    stats: Arc<AtomicAgentStats>,
}

impl UploadSink for AgentUploadSink {
    fn record_blob_uploaded(&self, bytes: u64) {
        self.stats
            .background_uploads
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .uploaded_bytes
            .fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_action_uploaded(&self) {
        self.stats
            .background_uploads
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_blob_pack_uploaded(&self, blobs: u64) {
        self.stats
            .remote_blob_pack_uploads
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .remote_blob_pack_upload_blobs
            .fetch_add(blobs, Ordering::Relaxed);
    }

    fn record_upload_failure(&self) {
        self.stats
            .background_upload_failures
            .fetch_add(1, Ordering::Relaxed);
        self.stats.remote_failures.fetch_add(1, Ordering::Relaxed);
    }
}

/// A CAS blob whose contents this session hashed, and the file identity that
/// says the hash still describes what is on disk.
///
/// Length alone would miss an overwrite that keeps the size; the modification
/// time is what makes a rewrite visible without reading the bytes again.
#[derive(Debug, Clone)]
struct VerifiedBlob {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

impl VerifiedBlob {
    /// Describe a file that was just verified, or nothing when the filesystem
    /// will not report a modification time to compare against later.
    fn describe(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok()?,
        })
    }

    /// Whether the file still has the identity it had when it was verified.
    fn is_unchanged(&self) -> bool {
        let Ok(metadata) = std::fs::metadata(&self.path) else {
            return false;
        };
        metadata.len() == self.len && metadata.modified().is_ok_and(|now| now == self.modified)
    }
}

#[derive(Debug, Clone, Default)]
struct TaskActionState {
    manifest: String,
    baseline_loaded: bool,
    predictions: BTreeMap<CacheDigest, ActionPrediction>,
    pending_predictions: BTreeMap<CacheDigest, ActionPrediction>,
    remote_etag: Option<String>,
}

struct PrefetchedAction {
    adapter: String,
    result: RemoteActionResult,
}

struct RemoteDownloadReservation {
    counter: Arc<AtomicU64>,
    reserved: u64,
    committed: bool,
}

impl RemoteDownloadReservation {
    fn bytes(&self) -> u64 {
        self.reserved
    }

    fn commit(mut self, bytes: u64) {
        debug_assert!(bytes <= self.reserved);
        self.counter
            .fetch_sub(self.reserved.saturating_sub(bytes), Ordering::AcqRel);
        self.committed = true;
    }
}

impl Drop for RemoteDownloadReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.counter.fetch_sub(self.reserved, Ordering::AcqRel);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ExecutableIdentityKey {
    executable: PathBuf,
    environment: BTreeMap<String, Option<String>>,
}

impl CacheAgent {
    /// Create an agent backed by the cache rooted at `cache_dir`.
    pub fn new(cache_dir: impl Into<PathBuf>, version: impl Into<Arc<str>>) -> Self {
        Self::build(cache_dir.into(), version.into(), None, 0)
    }

    /// Create an agent with local-first access to a remote action cache.
    pub fn new_remote(
        cache_dir: impl Into<PathBuf>,
        version: impl Into<Arc<str>>,
        remote: AgentRemoteCache,
    ) -> Self {
        Self::build(
            cache_dir.into(),
            version.into(),
            Some(remote),
            DEFAULT_MAX_REMOTE_DOWNLOAD_BYTES,
        )
    }

    /// Create a remote agent with a cumulative download budget for the session.
    pub fn new_remote_with_download_limit(
        cache_dir: impl Into<PathBuf>,
        version: impl Into<Arc<str>>,
        remote: AgentRemoteCache,
        max_remote_download_bytes: u64,
    ) -> Self {
        Self::build(
            cache_dir.into(),
            version.into(),
            Some(remote),
            max_remote_download_bytes,
        )
    }

    fn build(
        cache_dir: PathBuf,
        version: Arc<str>,
        remote: Option<AgentRemoteCache>,
        remote_download_limit: u64,
    ) -> Self {
        let remote_mode = remote
            .as_ref()
            .map_or(RemoteCacheMode::ReadOnly, |remote| remote.mode);
        let remote_staging_dir = remote.as_ref().map_or_else(
            || cache_dir.join("remote"),
            |remote| remote.staging_dir.clone(),
        );
        let remote = remote.map(|remote| Arc::new(remote.client));
        let stats = Arc::new(AtomicAgentStats::default());
        let remote_transfers = Arc::new(tokio::sync::Semaphore::new(MAX_REMOTE_TRANSFERS));
        // A session that cannot publish never queues an upload, so it does not
        // need somewhere to queue one.
        let uploads = remote
            .clone()
            .filter(|_| remote_mode.writes())
            .map(|client| {
                UploadQueue::new(
                    client,
                    Arc::new(AgentUploadSink {
                        stats: stats.clone(),
                    }),
                    remote_transfers.clone(),
                )
            });
        Self {
            cas: LocalCas::new(cache_dir.clone()),
            actions: LocalActionCache::new(cache_dir.clone()),
            verified_blobs: Arc::new(Mutex::new(BTreeMap::new())),
            version,
            write_locks: Arc::new(Mutex::new(BTreeMap::new())),
            action_locks: Arc::new(Mutex::new(BTreeMap::new())),
            stats,
            observer: None,
            executable_identities: Arc::new(Mutex::new(BTreeMap::new())),
            manifest_dir: Arc::new(task_manifest_dir(&cache_dir)),
            task_actions: Arc::new(Mutex::new(BTreeMap::new())),
            next_task_run: Arc::new(AtomicU64::new(0)),
            manifest_write_lock: Arc::new(Mutex::new(())),
            remote,
            remote_mode,
            remote_staging_dir: Arc::new(remote_staging_dir),
            remote_download_limit,
            remote_download_bytes: Arc::new(AtomicU64::new(0)),
            pending_remote_actions: Arc::new(Mutex::new(BTreeMap::new())),
            remote_transfers,
            prefetch_transfers: Arc::new(tokio::sync::Semaphore::new(MAX_PREFETCH_TRANSFERS)),
            prefetch_tasks: Arc::new(Mutex::new(Vec::new())),
            warnings: Arc::new(Mutex::new(BTreeSet::new())),
            uploads,
        }
    }

    /// Report each accounted cache decision to `observer` as it happens.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn AgentEventObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    fn emit(&self, event: impl FnOnce() -> AgentEvent) {
        if let Some(observer) = &self.observer {
            observer.event(event());
        }
    }

    fn reserve_remote_download(&self, bytes: u64) -> Result<RemoteDownloadReservation> {
        let mut current = self.remote_download_bytes.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(bytes)
                .ok_or_else(|| eyre::eyre!("remote cache download budget overflowed"))?;
            if next > self.remote_download_limit {
                bail!(
                    "remote cache download budget exceeded: {} bytes requested with {} of {} bytes already used",
                    bytes,
                    current,
                    self.remote_download_limit
                );
            }
            match self.remote_download_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(RemoteDownloadReservation {
                        counter: self.remote_download_bytes.clone(),
                        reserved: bytes,
                        committed: false,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn reserve_remote_download_up_to(&self, requested: u64) -> Result<RemoteDownloadReservation> {
        let mut current = self.remote_download_bytes.load(Ordering::Acquire);
        loop {
            let reserved = requested.min(self.remote_download_limit.saturating_sub(current));
            let next = current + reserved;
            match self.remote_download_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(RemoteDownloadReservation {
                        counter: self.remote_download_bytes.clone(),
                        reserved,
                        committed: false,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    /// Load the last committed action manifest for a task into this session.
    pub async fn begin_task(&self, task: &str) -> Result<String> {
        self.begin_task_with_remote_errors(task, false).await
    }

    /// Load a task and finish its prefetch, surfacing remote lookup failures.
    pub async fn prefetch_task(&self, task: &str) -> Result<String> {
        let run = self.begin_task_with_remote_errors(task, true).await?;
        self.wait_for_prefetches().await;
        Ok(run)
    }

    async fn begin_task_with_remote_errors(&self, task: &str, strict: bool) -> Result<String> {
        validate_task_identity(task)?;
        let (remote_manifest, mut remote_etag) = if self.remote_mode.reads() {
            match self.get_remote_task_manifest(task).await {
                Ok(Some((manifest, etag))) => (Some(manifest), Some(etag)),
                Ok(None) => (None, None),
                Err(error) => {
                    if strict {
                        return Err(error).wrap_err_with(|| {
                            format!("remote task action manifest lookup failed for {task}")
                        });
                    }
                    self.note_remote_failure();
                    warn!("remote task action manifest lookup failed for {task}: {error}");
                    (None, None)
                }
            }
        } else {
            (None, None)
        };
        let manifest = {
            let _write_guard = self.manifest_write_lock.lock().unwrap();
            let _file_guard = self.lock_task_manifest(task)?;
            let local_manifest = self.load_task_manifest(task)?;
            let manifest = match (remote_manifest, local_manifest) {
                (Some(remote), Some(local)) => {
                    let (manifest, merged) = merge_remote_task_manifest(task, remote, local);
                    if !merged {
                        remote_etag = None;
                    }
                    Some(manifest)
                }
                (Some(remote), None) => Some(remote),
                (None, local) => local,
            };
            if let Some(manifest) = &manifest {
                self.persist_task_manifest(manifest)?;
            }
            manifest
        };
        let state = if let Some(manifest) = manifest {
            TaskActionState {
                manifest: task.to_string(),
                baseline_loaded: true,
                predictions: manifest
                    .predictions
                    .into_iter()
                    .map(|prediction| (prediction.invocation.clone(), prediction))
                    .collect(),
                pending_predictions: BTreeMap::new(),
                remote_etag,
            }
        } else {
            TaskActionState {
                manifest: task.to_string(),
                baseline_loaded: true,
                remote_etag,
                ..TaskActionState::default()
            }
        };
        let sequence = self.next_task_run.fetch_add(1, Ordering::Relaxed);
        let run =
            CacheDigest::blake3(format!("{task}\0{}\0{sequence}", std::process::id()).as_bytes())
                .hash;
        // The largest baseline rather than a sum: beginning the same task again
        // in one session reloads the same manifest, and counting it twice would
        // overstate what there was to match.
        self.stats.predictions_loaded.fetch_max(
            state.predictions.len().try_into().unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        let predictions = state.predictions.values().cloned().collect();
        self.task_actions.lock().unwrap().insert(run.clone(), state);
        self.spawn_prefetch_predictions(predictions);
        Ok(run)
    }

    /// Cancel speculative downloads before the owning session exits.
    pub async fn cancel_prefetches(&self) {
        let tasks = std::mem::take(&mut *self.prefetch_tasks.lock().unwrap());
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            if let Err(error) = task.await
                && !error.is_cancelled()
            {
                warn!("remote action prefetch task failed: {error}");
            }
        }
    }

    /// Publish everything a build queued, before the session stops.
    ///
    /// Store requests return once an object is durable locally, so at this point
    /// the remote cache may still be behind the local one. Uploads run on the
    /// session's runtime and are abandoned if it goes away, so a session that
    /// wants them published has to wait here.
    pub async fn wait_for_uploads(&self) {
        let Some(uploads) = &self.uploads else {
            return;
        };
        let _timer = AtomicDurationTimer::start(&self.stats.upload_drain_duration_ns);
        uploads.drain().await;
    }

    async fn wait_for_prefetches(&self) {
        let tasks = std::mem::take(&mut *self.prefetch_tasks.lock().unwrap());
        for task in tasks {
            if let Err(error) = task.await {
                warn!("remote action prefetch task failed: {error}");
            }
        }
    }

    /// Atomically publish the completed actions collected by a task run.
    pub async fn commit_task(&self, run: &str) -> Result<()> {
        validate_task_identity(run)?;
        let state = self
            .task_actions
            .lock()
            .unwrap()
            .get(run)
            .cloned()
            .ok_or_else(|| eyre::eyre!("task action manifest baseline was not loaded"))?;
        if !state.baseline_loaded {
            bail!("task action manifest baseline was not loaded");
        }
        let task = state.manifest;
        validate_task_identity(&task)?;
        let (manifest, introduced) = {
            let _write_guard = self.manifest_write_lock.lock().unwrap();
            let _file_guard = self.lock_task_manifest(&task)?;
            let mut predictions = self
                .load_task_manifest(&task)?
                .map(|manifest| {
                    manifest
                        .predictions
                        .into_iter()
                        .map(|prediction| (prediction.invocation.clone(), prediction))
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default();
            // Which predictions this run adds, as opposed to inherits. Only a
            // new one may be withheld for a failed upload: retracting an
            // inherited one would un-advertise a result that is plausibly still
            // on the server from whichever session put it there.
            let introduced: BTreeSet<CacheDigest> = state
                .pending_predictions
                .keys()
                .filter(|invocation| !predictions.contains_key(*invocation))
                .cloned()
                .collect();
            // Only publish predictions recorded by this run. `predictions`
            // also contains the baseline loaded by `begin_task`; extending
            // with that snapshot would overwrite newer entries committed by
            // another agent process after this run began.
            predictions.extend(state.pending_predictions);
            let manifest = TaskActionManifest {
                version: TASK_ACTION_MANIFEST_VERSION,
                task: task.clone(),
                predictions: predictions.into_values().collect(),
            };
            validate_task_manifest(&manifest, &task)?;
            self.persist_task_manifest(&manifest)?;
            (manifest, introduced)
        };
        self.task_actions.lock().unwrap().remove(run);
        if self.remote_mode.writes() {
            // A manifest advertises the actions it predicts, so it must not
            // reach the remote cache before the results a reader would then go
            // looking for -- nor name a result that never got there at all.
            let mut manifest = manifest;
            if let Some(uploads) = &self.uploads {
                let actions: Vec<CacheDigest> = manifest
                    .predictions
                    .iter()
                    .map(|prediction| prediction.action.clone())
                    .collect();
                let unpublished = uploads.wait_for_actions(&actions).await;
                let withheld = manifest
                    .predictions
                    .iter()
                    .filter(|prediction| {
                        introduced.contains(&prediction.invocation)
                            && unpublished.contains(&prediction.action)
                    })
                    .count();
                if withheld > 0 {
                    // The local manifest keeps them: this checkout can still use
                    // what it built, and a later session can publish it.
                    warn!(
                        "{withheld} of {} predicted actions were not published, so the remote task action manifest omits them",
                        manifest.predictions.len()
                    );
                    manifest.predictions.retain(|prediction| {
                        !(introduced.contains(&prediction.invocation)
                            && unpublished.contains(&prediction.action))
                    });
                }
            }
            match self
                .put_remote_task_manifest(&task, manifest, state.remote_etag)
                .await
            {
                Ok(remote_manifest) => {
                    let _write_guard = self.manifest_write_lock.lock().unwrap();
                    let reconciliation = (|| {
                        let _file_guard = self.lock_task_manifest(&task)?;
                        let manifest = match self.load_task_manifest(&task)? {
                            Some(local) => {
                                merge_remote_task_manifest(&task, remote_manifest, local).0
                            }
                            None => remote_manifest,
                        };
                        self.persist_task_manifest(&manifest)
                    })();
                    if let Err(error) = reconciliation {
                        warn!(
                            "remote task action manifest reconciliation failed for {task}: {error}"
                        );
                    }
                }
                Err(error) => {
                    self.note_remote_failure();
                    warn!("remote task action manifest upload failed for {task}: {error}");
                }
            }
        }
        Ok(())
    }

    fn task_manifest_path(&self, task: &str) -> PathBuf {
        self.manifest_dir.join(format!("{task}.json"))
    }

    fn task_manifest_lock_path(&self, task: &str) -> PathBuf {
        self.manifest_dir.join("locks").join(format!("{task}.lock"))
    }

    fn lock_task_manifest(&self, task: &str) -> Result<fslock::LockFile> {
        let path = self.task_manifest_lock_path(task);
        fs::create_dir_all(path.parent().expect("task manifest lock has a parent"))?;
        let mut lock = fslock::LockFile::open(&path)?;
        lock.lock()?;
        Ok(lock)
    }

    fn load_task_manifest(&self, task: &str) -> Result<Option<TaskActionManifest>> {
        match fs::read(self.task_manifest_path(task)) {
            Ok(contents) => Ok(Some(self.parse_task_manifest(task, &contents, false)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn parse_task_manifest(
        &self,
        task: &str,
        contents: &[u8],
        require_canonical: bool,
    ) -> Result<TaskActionManifest> {
        let manifest: TaskActionManifest = serde_json::from_slice(contents)?;
        validate_task_manifest(&manifest, task)?;
        if require_canonical && canonical_json(&manifest)? != contents {
            bail!("task action manifest is not canonical JSON");
        }
        Ok(manifest)
    }

    fn task_manifest_selector(task: &str) -> Result<(Vec<u8>, CacheDigest)> {
        TaskActionManifest::selector(task)
    }

    fn persist_task_manifest(&self, manifest: &TaskActionManifest) -> Result<()> {
        let bytes = canonical_json(manifest)?;
        fs::create_dir_all(self.manifest_dir.as_path())?;
        let mut temporary = tempfile::NamedTempFile::new_in(self.manifest_dir.as_path())?;
        std::io::Write::write_all(temporary.as_file_mut(), &bytes)?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(self.task_manifest_path(&manifest.task))
            .map_err(|error| error.error)?;
        Ok(())
    }

    async fn get_remote_task_manifest(
        &self,
        task: &str,
    ) -> Result<Option<(TaskActionManifest, String)>> {
        let Some(remote) = &self.remote else {
            return Ok(None);
        };
        let (_, selector) = Self::task_manifest_selector(task)?;
        let _permit = self.remote_transfers.acquire().await?;
        self.stats
            .remote_manifest_lookups
            .fetch_add(1, Ordering::Relaxed);
        let _timer = AtomicDurationTimer::start(&self.stats.remote_manifest_lookup_duration_ns);
        let Some(remote_manifest) = remote.get_action_manifest(&selector).await? else {
            return Ok(None);
        };
        let manifest = self.parse_task_manifest(task, &remote_manifest.bytes, true)?;
        Ok(Some((manifest, remote_manifest.etag)))
    }

    async fn put_remote_task_manifest(
        &self,
        task: &str,
        mut manifest: TaskActionManifest,
        mut expected_etag: Option<String>,
    ) -> Result<TaskActionManifest> {
        let Some(remote) = &self.remote else {
            return Ok(manifest);
        };
        let (_, selector) = Self::task_manifest_selector(task)?;
        for _ in 0..4 {
            let bytes = canonical_json(&manifest)?;
            let outcome = {
                let _permit = self.remote_transfers.acquire().await?;
                remote
                    .put_action_manifest(&selector, &bytes, expected_etag.as_deref())
                    .await?
            };
            match outcome {
                ManifestPutOutcome::Stored => return Ok(manifest),
                ManifestPutOutcome::PreconditionFailed => {
                    let Some((remote_manifest, etag)) = self.get_remote_task_manifest(task).await?
                    else {
                        expected_etag = None;
                        continue;
                    };
                    manifest = merge_task_manifests(task, Some(remote_manifest), manifest)?;
                    expected_etag = Some(etag);
                }
            }
        }
        bail!("remote task action manifest changed too frequently")
    }

    /// Return a snapshot of this session's cache activity.
    pub fn stats(&self) -> AgentStats {
        AgentStats {
            session_duration_ns: 0,
            lookups: self.stats.lookups.load(Ordering::Relaxed),
            unconsulted: self.stats.unconsulted.load(Ordering::Relaxed),
            hits: self.stats.hits.load(Ordering::Relaxed),
            stores: self.stats.stores.load(Ordering::Relaxed),
            stored_bytes: self.stats.stored_bytes.load(Ordering::Relaxed),
            verifications: self.stats.verifications.load(Ordering::Relaxed),
            divergences: self.stats.divergences.load(Ordering::Relaxed),
            downloaded_bytes: self.stats.downloaded_bytes.load(Ordering::Relaxed),
            uploaded_bytes: self.stats.uploaded_bytes.load(Ordering::Relaxed),
            background_uploads: self.stats.background_uploads.load(Ordering::Relaxed),
            background_upload_failures: self
                .stats
                .background_upload_failures
                .load(Ordering::Relaxed),
            remote_blob_pack_uploads: self.stats.remote_blob_pack_uploads.load(Ordering::Relaxed),
            remote_blob_pack_upload_blobs: self
                .stats
                .remote_blob_pack_upload_blobs
                .load(Ordering::Relaxed),
            upload_drain_duration_ns: self.stats.upload_drain_duration_ns.load(Ordering::Relaxed),
            prefetched_actions: self.stats.prefetched_actions.load(Ordering::Relaxed),
            predictions_loaded: self.stats.predictions_loaded.load(Ordering::Relaxed),
            bypasses: self.stats.bypasses.lock().unwrap().clone(),
            avoided_compiler_duration_ns: self
                .stats
                .avoided_compiler_duration_ns
                .load(Ordering::Relaxed),
            compiler: self.stats.compiler.lock().unwrap().clone(),
            slow_compilations: self.stats.slow_compilations.lock().unwrap().clone(),
            remote_failures: self.stats.remote_failures.load(Ordering::Relaxed),
            remote_manifest_lookups: self.stats.remote_manifest_lookups.load(Ordering::Relaxed),
            remote_manifest_lookup_duration_ns: self
                .stats
                .remote_manifest_lookup_duration_ns
                .load(Ordering::Relaxed),
            remote_action_lookups: self.stats.remote_action_lookups.load(Ordering::Relaxed),
            remote_action_lookup_duration_ns: self
                .stats
                .remote_action_lookup_duration_ns
                .load(Ordering::Relaxed),
            remote_blob_requests: self.stats.remote_blob_requests.load(Ordering::Relaxed),
            remote_blob_pack_requests: self.stats.remote_blob_pack_requests.load(Ordering::Relaxed),
            remote_blob_pack_blobs: self.stats.remote_blob_pack_blobs.load(Ordering::Relaxed),
            remote_blob_transfer_duration_ns: self
                .stats
                .remote_blob_transfer_duration_ns
                .load(Ordering::Relaxed),
            local_cas_write_duration_ns: self
                .stats
                .local_cas_write_duration_ns
                .load(Ordering::Relaxed),
            prefetch_runs: self.stats.prefetch_runs.load(Ordering::Relaxed),
            prefetch_duration_ns: self.stats.prefetch_duration_ns.load(Ordering::Relaxed),
            materialization_duration_ns: self
                .stats
                .materialization_duration_ns
                .load(Ordering::Relaxed),
            restored_output_files: self.stats.restored_output_files.load(Ordering::Relaxed),
            restored_output_bytes: self.stats.restored_output_bytes.load(Ordering::Relaxed),
            reflinked_output_files: self.stats.reflinked_output_files.load(Ordering::Relaxed),
            reflinked_output_bytes: self.stats.reflinked_output_bytes.load(Ordering::Relaxed),
            copied_output_files: self.stats.copied_output_files.load(Ordering::Relaxed),
            copied_output_bytes: self.stats.copied_output_bytes.load(Ordering::Relaxed),
        }
    }

    /// Handle requests without a transport connection.
    ///
    /// Persistent wrappers use this entry point when Cargo invokes mbx outside
    /// an orchestrated session. It intentionally has the same response
    /// semantics as [`Self::handle_connection`], while leaving framing and the
    /// version handshake to callers that actually cross a process boundary.
    pub async fn handle_requests(
        &self,
        requests: impl IntoIterator<Item = AgentRequest>,
    ) -> Vec<AgentResponse> {
        let mut connection = ConnectionUploads::default();
        let mut responses = Vec::new();
        for request in requests {
            responses.push(self.respond_on(request, &mut connection).await);
        }
        responses
    }

    fn write_lock(&self, digest: &CacheDigest) -> Arc<tokio::sync::Mutex<()>> {
        Self::digest_lock(&self.write_locks, digest)
    }

    fn action_lock(&self, digest: &CacheDigest) -> Arc<tokio::sync::Mutex<()>> {
        Self::digest_lock(&self.action_locks, digest)
    }

    fn digest_lock(
        locks: &Mutex<BTreeMap<CacheDigest, Weak<tokio::sync::Mutex<()>>>>,
        digest: &CacheDigest,
    ) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = locks.lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(digest).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(digest.clone(), Arc::downgrade(&lock));
        lock
    }

    fn spawn_prefetch_predictions(&self, predictions: Vec<ActionPrediction>) {
        if predictions.is_empty() || !self.remote_mode.reads() || self.remote.is_none() {
            return;
        }
        let agent = self.clone();
        let task = tokio::spawn(async move {
            agent.prefetch_predictions(predictions.iter()).await;
        });
        self.prefetch_tasks.lock().unwrap().push(task);
    }

    async fn prefetch_predictions<'a>(
        &self,
        predictions: impl Iterator<Item = &'a ActionPrediction>,
    ) {
        if !self.remote_mode.reads() || self.remote.is_none() {
            return;
        }
        self.stats.prefetch_runs.fetch_add(1, Ordering::Relaxed);
        let _timer = AtomicDurationTimer::start(&self.stats.prefetch_duration_ns);
        let mut actions = BTreeMap::new();
        for prediction in predictions {
            actions
                .entry(prediction.action.clone())
                .or_insert_with(|| prediction.adapter.clone());
        }
        // One request per predicted action is the bulk of a prefetch's latency on
        // a large workspace, so ask for them together where the server allows it.
        match self.prefetch_action_batches(&actions).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                self.note_remote_failure();
                warn!("remote action batch lookup failed: {error}");
            }
        }
        let mut actions = actions.into_iter();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..MAX_PREFETCH_TRANSFERS {
            let Some((action, adapter)) = actions.next() else {
                break;
            };
            let agent = self.clone();
            tasks.spawn(async move { agent.resolve_prefetch_action(action, adapter).await });
        }
        let mut resolved = Vec::new();
        while !tasks.is_empty() {
            let result = if resolved.is_empty() {
                tasks.join_next().await
            } else {
                match tokio::time::timeout(PREFETCH_ACTION_BATCH_DELAY, tasks.join_next()).await {
                    Ok(result) => result,
                    Err(_) => {
                        self.prefetch_resolved_actions(std::mem::take(&mut resolved))
                            .await;
                        continue;
                    }
                }
            };
            let Some(result) = result else {
                break;
            };
            match result {
                Ok(Ok(Some(action))) => resolved.push(action),
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    self.note_remote_failure();
                    warn!("remote action prefetch failed: {error}");
                }
                Err(error) => {
                    self.note_remote_failure();
                    warn!("remote action prefetch task failed: {error}");
                }
            }
            if let Some((action, adapter)) = actions.next() {
                let agent = self.clone();
                tasks.spawn(async move { agent.resolve_prefetch_action(action, adapter).await });
            }
            if resolved.len() == MAX_PREFETCH_ACTION_BATCH {
                self.prefetch_resolved_actions(std::mem::take(&mut resolved))
                    .await;
            }
        }
        if !resolved.is_empty() {
            self.prefetch_resolved_actions(resolved).await;
        }
    }

    /// Resolve predicted actions in batched lookups, staging what comes back.
    ///
    /// Returns whether the batch extension answered. A server without it, or one
    /// that stops answering part way through, leaves the per-action path to
    /// resolve whatever is left -- the results already staged here are memoized,
    /// so nothing is looked up twice.
    async fn prefetch_action_batches(
        &self,
        actions: &BTreeMap<CacheDigest, String>,
    ) -> Result<bool> {
        let Some(remote) = self.remote.as_deref() else {
            return Ok(false);
        };
        let Some(limit) = remote.action_batch_limit().await? else {
            return Ok(false);
        };
        let wanted: Vec<CacheDigest> = actions
            .keys()
            .filter(|action| !self.action_is_staged(action))
            .cloned()
            .collect();
        if wanted.is_empty() {
            return Ok(true);
        }
        let mut chunks: Vec<Vec<CacheDigest>> = Vec::new();
        for chunk in wanted.chunks(limit) {
            chunks.push(chunk.to_vec());
        }
        let mut lookups = stream::iter(chunks)
            .map(|chunk| async move {
                let _prefetch_permit = self.prefetch_transfers.acquire().await?;
                let _permit = self.remote_transfers.acquire().await?;
                self.stats
                    .remote_action_lookups
                    .fetch_add(1, Ordering::Relaxed);
                let _timer =
                    AtomicDurationTimer::start(&self.stats.remote_action_lookup_duration_ns);
                remote.get_action_results(&chunk).await
            })
            .buffer_unordered(MAX_PREFETCH_BATCH_LOOKUPS);
        let mut answered = true;
        while let Some(lookup) = lookups.next().await {
            let results = match lookup {
                Ok(Some(results)) => results,
                // Advertised but unavailable. What is left falls back.
                Ok(None) => {
                    answered = false;
                    continue;
                }
                Err(error) => {
                    self.note_remote_failure();
                    warn!("remote action batch lookup failed: {error}");
                    answered = false;
                    continue;
                }
            };
            let mut resolved = Vec::with_capacity(results.len());
            for result in results {
                let Some(adapter) = actions.get(&result.action).cloned() else {
                    continue;
                };
                let lock = self.action_lock(&result.action);
                let _guard = lock.lock().await;
                if self.actions.find(&result.action)?.is_some() {
                    continue;
                }
                self.pending_remote_actions
                    .lock()
                    .unwrap()
                    .insert(result.action.clone(), result.clone());
                resolved.push(PrefetchedAction { adapter, result });
            }
            while !resolved.is_empty() {
                let wave = resolved
                    .drain(..resolved.len().min(MAX_PREFETCH_ACTION_BATCH))
                    .collect();
                self.prefetch_resolved_actions(wave).await;
            }
        }
        Ok(answered)
    }

    /// Whether an action's result is already local or already looked up.
    fn action_is_staged(&self, action: &CacheDigest) -> bool {
        if self
            .pending_remote_actions
            .lock()
            .unwrap()
            .contains_key(action)
        {
            return true;
        }
        self.actions.find(action).is_ok_and(|found| found.is_some())
    }

    #[cfg(test)]
    async fn prefetch_action(&self, action: CacheDigest, adapter: String) -> Result<()> {
        if let Some(action) = self.resolve_prefetch_action(action, adapter).await? {
            self.prefetch_resolved_actions(vec![action]).await;
        }
        Ok(())
    }

    async fn resolve_prefetch_action(
        &self,
        action: CacheDigest,
        adapter: String,
    ) -> Result<Option<PrefetchedAction>> {
        let remote = self
            .remote
            .as_ref()
            .ok_or_else(|| eyre::eyre!("remote cache is not configured"))?;
        let result = {
            let lock = self.action_lock(&action);
            let _guard = lock.lock().await;
            if self.actions.find(&action)?.is_some() {
                return Ok(None);
            }
            if let Some(result) = self
                .pending_remote_actions
                .lock()
                .unwrap()
                .get(&action)
                .cloned()
            {
                result
            } else {
                let _prefetch_permit = self.prefetch_transfers.acquire().await?;
                let result = {
                    let _permit = self.remote_transfers.acquire().await?;
                    self.get_remote_action_result(remote, &action).await?
                };
                let Some(result) = result else {
                    return Ok(None);
                };
                self.pending_remote_actions
                    .lock()
                    .unwrap()
                    .insert(action.clone(), result.clone());
                result
            }
        };
        Ok(Some(PrefetchedAction { adapter, result }))
    }

    fn prefetch_resolved_actions(&self, actions: Vec<PrefetchedAction>) -> BoxFuture<'_, ()> {
        self.prefetch_resolved_actions_inner(actions).boxed()
    }

    async fn prefetch_resolved_actions_inner(&self, actions: Vec<PrefetchedAction>) {
        let Some(remote) = self.remote.as_deref() else {
            return;
        };
        if actions.is_empty() {
            return;
        }

        let mut top_level = BTreeMap::new();
        for action in &actions {
            for digest in [
                Some(&action.result.action),
                action.result.metadata.as_ref(),
                action.result.output_root.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                top_level.insert(digest.clone(), ());
            }
        }
        let mut verified = self
            .fetch_remote_blobs(
                remote,
                top_level.into_keys().collect(),
                Some(&self.prefetch_transfers),
            )
            .await;

        let mut next = BTreeMap::new();
        let mut pending_directories = BTreeMap::new();
        let mut parsed_directories = BTreeMap::new();
        let mut rustc_metadata = BTreeMap::new();
        for action in &actions {
            if action.adapter == "rustc"
                && let Some(metadata_digest) = &action.result.metadata
            {
                match verified
                    .get(metadata_digest)
                    .ok_or_else(|| eyre::eyre!("remote rustc action metadata is missing"))
                    .and_then(|path| Self::parse_rustc_metadata(path))
                {
                    Ok(metadata) => {
                        queue_prefetch_digest(&verified, &mut next, metadata.stdout.clone());
                        queue_prefetch_digest(&verified, &mut next, metadata.stderr.clone());
                        rustc_metadata.insert(metadata_digest.clone(), metadata);
                    }
                    Err(error) => warn!(
                        "remote rustc action metadata prefetch failed for {}: {error}",
                        action.result.action.hash
                    ),
                }
            }
            if let Some(output_root) = &action.result.output_root {
                pending_directories.insert(output_root.clone(), ());
            }
        }

        let mut seen_directories = BTreeMap::new();
        loop {
            let mut following = BTreeMap::new();
            let mut directory_limit_exceeded = false;
            for digest in pending_directories.into_keys() {
                following.remove(&digest);
                if seen_directories.insert(digest.clone(), ()).is_some() {
                    continue;
                }
                if seen_directories.len() > MAX_PREFETCH_DIRECTORY_OBJECTS {
                    warn!("remote action output tree is too large to prefetch");
                    following.clear();
                    break;
                }
                match verified
                    .get(&digest)
                    .ok_or_else(|| eyre::eyre!("remote action output directory is missing"))
                    .and_then(|path| Self::parse_cache_directory(path))
                {
                    Ok(directory) => {
                        for file in &directory.files {
                            queue_prefetch_digest(&verified, &mut next, file.digest.clone());
                            if next.len() >= MAX_PREFETCH_OBJECTS_PER_WAVE {
                                self.flush_prefetch_digest_batch(remote, &mut verified, &mut next)
                                    .await;
                            }
                        }
                        for child in &directory.directories {
                            if !queue_prefetch_directory(
                                &seen_directories,
                                &mut following,
                                child.digest.clone(),
                                MAX_PREFETCH_DIRECTORY_OBJECTS,
                            ) {
                                warn!("remote action output tree is too large to prefetch");
                                directory_limit_exceeded = true;
                                break;
                            }
                            queue_prefetch_digest(&verified, &mut next, child.digest.clone());
                            if next.len() >= MAX_PREFETCH_OBJECTS_PER_WAVE {
                                self.flush_prefetch_digest_batch(remote, &mut verified, &mut next)
                                    .await;
                            }
                        }
                        parsed_directories.insert(digest, directory);
                    }
                    Err(error) => warn!(
                        "remote action output directory prefetch failed for {}: {error}",
                        digest.hash
                    ),
                }
                if directory_limit_exceeded {
                    following.clear();
                    break;
                }
            }
            self.flush_prefetch_digest_batch(remote, &mut verified, &mut next)
                .await;
            if following.is_empty() {
                break;
            }
            pending_directories = following;
        }

        for action in actions {
            match Self::validate_prefetched_action(
                &action,
                &verified,
                &rustc_metadata,
                &parsed_directories,
            ) {
                Ok(()) => {
                    if let Err(error) = self.actions.store(&action.result) {
                        warn!(
                            "remote action prefetch could not publish {}: {error}",
                            action.result.action.hash
                        );
                        continue;
                    }
                    self.pending_remote_actions
                        .lock()
                        .unwrap()
                        .remove(&action.result.action);
                    self.stats
                        .prefetched_actions
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => warn!(
                    "remote action prefetch was incomplete for {}: {error}",
                    action.result.action.hash
                ),
            }
        }
    }

    async fn flush_prefetch_digest_batch(
        &self,
        remote: &RemoteCacheClient,
        verified: &mut BTreeMap<CacheDigest, PathBuf>,
        pending: &mut BTreeMap<CacheDigest, ()>,
    ) {
        if pending.is_empty() {
            return;
        }
        let digests = std::mem::take(pending).into_keys().collect();
        verified.extend(
            self.fetch_remote_blobs(remote, digests, Some(&self.prefetch_transfers))
                .await,
        );
    }

    async fn fetch_remote_blobs(
        &self,
        remote: &RemoteCacheClient,
        digests: Vec<CacheDigest>,
        prefetch_limit: Option<&tokio::sync::Semaphore>,
    ) -> BTreeMap<CacheDigest, PathBuf> {
        let mut verified = BTreeMap::new();
        let mut missing = BTreeMap::new();
        for digest in digests {
            match self.find_verified_blob(&digest) {
                Ok(Some(path)) => {
                    verified.insert(digest, path);
                }
                Ok(None) => {
                    missing.insert(digest, ());
                }
                Err(error) => warn!(
                    "local cache blob lookup failed for {}: {error}",
                    digest.hash
                ),
            }
        }
        if missing.is_empty() {
            return verified;
        }

        let mut pack_candidates = missing.clone();
        while !pack_candidates.is_empty() {
            let candidates = match blob_pack_chunk(
                &pack_candidates.keys().cloned().collect::<Vec<_>>(),
                BlobPackLimits {
                    max_items: MAX_STAGED_BLOB_PACK_ITEMS,
                    max_bytes: MAX_STAGED_BLOB_PACK_BYTES,
                },
            ) {
                Ok(candidates) if !candidates.is_empty() => candidates,
                Ok(_) => break,
                Err(error) => {
                    warn!("remote cache blob pack skipped: {error}");
                    break;
                }
            };
            // A pack and an individual fetch share these per-digest locks. Hold
            // them through ingestion so overlapping prefetch and foreground
            // requests cannot download or charge the same object twice.
            let mut pack_guards = BTreeMap::new();
            for digest in candidates {
                let guard = self.write_lock(&digest).lock_owned().await;
                match self.find_verified_blob(&digest) {
                    Ok(Some(path)) => {
                        pack_candidates.remove(&digest);
                        missing.remove(&digest);
                        verified.insert(digest, path);
                    }
                    Ok(None) => {
                        pack_guards.insert(digest, guard);
                    }
                    Err(error) => {
                        warn!(
                            "local cache blob lookup failed for {}: {error}",
                            digest.hash
                        );
                        pack_guards.insert(digest, guard);
                    }
                }
            }
            let requested = pack_guards.keys().cloned().collect::<Vec<_>>();
            if requested.is_empty() {
                continue;
            }
            let requested_bytes = requested
                .iter()
                .fold(0_u64, |total, digest| total.saturating_add(digest.size));
            let pack_reservation = match self
                .reserve_remote_download_up_to(requested_bytes.min(MAX_STAGED_BLOB_PACK_BYTES))
            {
                Ok(reservation) if reservation.bytes() > 0 => reservation,
                Ok(_) => break,
                Err(error) => {
                    warn!("remote cache blob pack skipped: {error}");
                    break;
                }
            };
            let (pack, transfer_duration_ns) = {
                let _prefetch_permit = match prefetch_limit {
                    Some(limit) => match limit.acquire().await {
                        Ok(permit) => Some(permit),
                        Err(error) => {
                            warn!(
                                "remote cache blob pack could not acquire prefetch limit: {error}"
                            );
                            break;
                        }
                    },
                    None => None,
                };
                let _transfer_permit = match self.remote_transfers.acquire().await {
                    Ok(permit) => permit,
                    Err(error) => {
                        warn!("remote cache blob pack could not acquire transfer limit: {error}");
                        break;
                    }
                };
                let transfer_started = Instant::now();
                let pack = remote
                    .get_blob_pack_with_limit(
                        &requested,
                        self.remote_staging_dir.as_path(),
                        pack_reservation.bytes(),
                    )
                    .await;
                (pack, duration_ns(transfer_started))
            };
            let pack = match pack {
                Ok(Some(pack)) => pack,
                Ok(None) => break,
                Err(error) => {
                    atomic_saturating_add(
                        &self.stats.remote_blob_transfer_duration_ns,
                        transfer_duration_ns,
                    );
                    warn!(
                        "remote cache blob pack failed; falling back to individual blobs: {error}"
                    );
                    break;
                }
            };
            pack_reservation.commit(pack.payload_bytes);
            atomic_saturating_add(
                &self.stats.remote_blob_transfer_duration_ns,
                transfer_duration_ns,
            );
            atomic_saturating_add(&self.stats.remote_blob_pack_requests, pack.requests);
            atomic_saturating_add(&self.stats.remote_blob_pack_blobs, pack.blob_count);
            atomic_saturating_add(&self.stats.downloaded_bytes, pack.payload_bytes);
            if pack.requested.is_empty() {
                // The server's negotiated cap can be smaller than the local
                // staging cap used to select this locked slice. Fall back for
                // this slice, but keep packing later candidates that may fit.
                for digest in &requested {
                    pack_candidates.remove(digest);
                }
                continue;
            }
            for digest in &pack.requested {
                pack_candidates.remove(digest);
            }
            let mut ingests = stream::iter(pack.blobs.into_iter().map(|(digest, source)| {
                let digest_for_result = digest.clone();
                let guard = pack_guards
                    .remove(&digest)
                    .expect("requested packed blob has a write lock");
                async move {
                    (
                        digest_for_result,
                        self.ingest_packed_blob(digest, source, guard).await,
                    )
                }
            }))
            .buffer_unordered(MAX_PREFETCH_TRANSFERS);
            while let Some((digest, result)) = ingests.next().await {
                match result {
                    Ok(path) => {
                        missing.remove(&digest);
                        verified.insert(digest, path);
                    }
                    Err(error) => warn!(
                        "remote cache packed blob ingest failed for {}: {error}",
                        digest.hash
                    ),
                }
            }
        }

        let mut transfers = stream::iter(missing.into_keys().map(|digest| {
            let digest_for_result = digest.clone();
            async move {
                (
                    digest_for_result,
                    self.fetch_remote_blob_with_limit(remote, &digest, prefetch_limit)
                        .await,
                )
            }
        }))
        .buffer_unordered(MAX_PREFETCH_TRANSFERS);
        while let Some((digest, result)) = transfers.next().await {
            match result {
                Ok(path) => {
                    verified.insert(digest, path);
                }
                Err(error) => warn!(
                    "remote cache blob prefetch failed for {}: {error}",
                    digest.hash
                ),
            }
        }
        verified
    }

    async fn ingest_packed_blob(
        &self,
        digest: CacheDigest,
        source: PathBuf,
        _guard: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<PathBuf> {
        let digest_size = digest.size;
        let agent = self.clone();
        let (path, stored, cas_duration_ns) = tokio::task::spawn_blocking(move || {
            if let Some(path) = agent.find_verified_blob(&digest)? {
                return Ok::<_, eyre::Report>((path, false, 0));
            }
            let cas_started = Instant::now();
            let path = agent.cas.store_verified_file(&digest, &source)?;
            let cas_duration_ns = duration_ns(cas_started);
            agent.remember_verified_blob(&digest, &path);
            Ok((path, true, cas_duration_ns))
        })
        .await??;
        atomic_saturating_add(&self.stats.local_cas_write_duration_ns, cas_duration_ns);
        if stored {
            self.stats.stores.fetch_add(1, Ordering::Relaxed);
            atomic_saturating_add(&self.stats.stored_bytes, digest_size);
        }
        Ok(path)
    }

    fn parse_rustc_metadata(path: &Path) -> Result<RustcMetadata> {
        let bytes = fs::read(path)?;
        let metadata: RustcMetadata = serde_json::from_slice(&bytes)?;
        if metadata.version != 1 || metadata.kind != "rustc" || canonical_json(&metadata)? != bytes
        {
            bail!("remote rustc action metadata is invalid");
        }
        Ok(metadata)
    }

    fn parse_cache_directory(path: &Path) -> Result<CacheDirectory> {
        let bytes = fs::read(path)?;
        let directory: CacheDirectory = serde_json::from_slice(&bytes)?;
        if directory.version != 1 || canonical_json(&directory)? != bytes {
            bail!("remote action output directory is invalid");
        }
        Ok(directory)
    }

    #[cfg(test)]
    fn load_cache_directory(&self, digest: &CacheDigest) -> Result<CacheDirectory> {
        let path = self
            .find_verified_blob(digest)?
            .ok_or_else(|| eyre::eyre!("remote action output directory is missing"))?;
        Self::parse_cache_directory(&path)
    }

    fn validate_prefetched_action(
        action: &PrefetchedAction,
        verified: &BTreeMap<CacheDigest, PathBuf>,
        rustc_metadata: &BTreeMap<CacheDigest, RustcMetadata>,
        directories: &BTreeMap<CacheDigest, CacheDirectory>,
    ) -> Result<()> {
        if !verified.contains_key(&action.result.action) {
            bail!("remote action descriptor is missing");
        }
        if let Some(metadata) = &action.result.metadata {
            if action.adapter == "rustc" {
                let metadata = rustc_metadata
                    .get(metadata)
                    .ok_or_else(|| eyre::eyre!("remote rustc action metadata is missing"))?;
                for digest in [&metadata.stdout, &metadata.stderr] {
                    if !verified.contains_key(digest) {
                        bail!("remote rustc action diagnostic blob is missing");
                    }
                }
            } else if !verified.contains_key(metadata) {
                bail!("remote action metadata is missing");
            }
        }
        let mut pending = action
            .result
            .output_root
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut seen = BTreeMap::new();
        while let Some(digest) = pending.pop() {
            if seen.insert(digest.clone(), ()).is_some() {
                continue;
            }
            if seen.len() > MAX_PREFETCH_DIRECTORY_OBJECTS {
                bail!("remote action output tree is too large");
            }
            let directory = directories
                .get(&digest)
                .ok_or_else(|| eyre::eyre!("remote action output directory is missing"))?;
            for file in &directory.files {
                if !verified.contains_key(&file.digest) {
                    bail!("remote action output file is missing");
                }
            }
            pending.extend(
                directory
                    .directories
                    .iter()
                    .map(|directory| directory.digest.clone()),
            );
        }
        Ok(())
    }

    #[cfg(test)]
    async fn prefetch_output_tree(
        &self,
        remote: &RemoteCacheClient,
        output_root: &CacheDigest,
    ) -> Result<()> {
        let mut pending = vec![output_root.clone()];
        let mut seen = BTreeMap::new();
        while let Some(digest) = pending.pop() {
            if seen.insert(digest.clone(), ()).is_some() {
                continue;
            }
            if seen.len() > MAX_PREFETCH_DIRECTORY_OBJECTS {
                bail!("remote action output tree is too large");
            }
            self.fetch_remote_blob_with_limit(remote, &digest, Some(&self.prefetch_transfers))
                .await?;
            let directory = self.load_cache_directory(&digest)?;
            let mut transfers = stream::iter(directory.files.into_iter().map(|file| async move {
                self.fetch_remote_blob_with_limit(
                    remote,
                    &file.digest,
                    Some(&self.prefetch_transfers),
                )
                .await
                .map(|_| ())
            }))
            .buffer_unordered(MAX_PREFETCH_TRANSFERS);
            while let Some(result) = transfers.next().await {
                result?;
            }
            pending.extend(
                directory
                    .directories
                    .into_iter()
                    .map(|directory| directory.digest),
            );
        }
        Ok(())
    }

    async fn fetch_remote_blob(
        &self,
        remote: &RemoteCacheClient,
        digest: &CacheDigest,
    ) -> Result<PathBuf> {
        self.fetch_remote_blob_with_limit(remote, digest, None)
            .await
    }

    async fn fetch_remote_blob_with_limit(
        &self,
        remote: &RemoteCacheClient,
        digest: &CacheDigest,
        prefetch_limit: Option<&tokio::sync::Semaphore>,
    ) -> Result<PathBuf> {
        let lock = self.write_lock(digest);
        let _guard = lock.lock().await;
        if let Some(path) = self.find_verified_blob(digest)? {
            return Ok(path);
        }
        let _prefetch_permit = match prefetch_limit {
            Some(limit) => Some(limit.acquire().await?),
            None => None,
        };
        let _permit = self.remote_transfers.acquire().await?;
        let reservation = self.reserve_remote_download(digest.size)?;
        self.stats
            .remote_blob_requests
            .fetch_add(1, Ordering::Relaxed);
        let transfer_timer =
            AtomicDurationTimer::start(&self.stats.remote_blob_transfer_duration_ns);
        let temporary = remote
            .get_blob_file(digest, self.remote_staging_dir.as_path())
            .await?;
        drop(transfer_timer);
        let _cas_timer = AtomicDurationTimer::start(&self.stats.local_cas_write_duration_ns);
        let path = self.cas.store_verified_file(digest, temporary.path())?;
        reservation.commit(digest.size);
        self.remember_verified_blob(digest, &path);
        self.stats.stores.fetch_add(1, Ordering::Relaxed);
        self.stats
            .stored_bytes
            .fetch_add(digest.size, Ordering::Relaxed);
        self.stats
            .downloaded_bytes
            .fetch_add(digest.size, Ordering::Relaxed);
        Ok(path)
    }

    /// Answer one request outside any connection.
    ///
    /// An upload queued this way has no sibling requests to order against, so
    /// the queue falls back to treating every blob it holds as a prerequisite.
    #[cfg(test)]
    async fn respond(&self, request: AgentRequest) -> AgentResponse {
        self.respond_on(request, &mut ConnectionUploads::default())
            .await
    }

    async fn respond_on(
        &self,
        request: AgentRequest,
        connection: &mut ConnectionUploads,
    ) -> AgentResponse {
        let result = match request {
            AgentRequest::BeginTask { task } => self
                .begin_task(&task)
                .await
                .map(|run| AgentResponse::TaskBegun { run }),
            AgentRequest::CommitTask { run } => self
                .commit_task(&run)
                .await
                .map(|()| AgentResponse::TaskCommitted),
            AgentRequest::FindBlob { digest } => self.find_blob(&digest).await,
            AgentRequest::FindBlobs { digests } => self.find_blobs(digests).await,
            AgentRequest::StoreBlob { digest, source } => {
                self.store_blob(&digest, &source, connection).await
            }
            AgentRequest::FindActionResult { action } => {
                self.stats.lookups.fetch_add(1, Ordering::Relaxed);
                self.find_action_result(&action).await
            }
            AgentRequest::RecordActionHit {
                action,
                restore,
                crate_name,
            } => self.record_action_hit(&action, restore, crate_name),
            AgentRequest::RecordBypass { kind } => {
                *self
                    .stats
                    .bypasses
                    .lock()
                    .unwrap()
                    .entry(kind.clone())
                    .or_insert(0) += 1;
                self.emit(|| AgentEvent::Bypass { kind });
                Ok(AgentResponse::BypassRecorded)
            }
            AgentRequest::RecordUnconsulted => {
                self.stats.unconsulted.fetch_add(1, Ordering::Relaxed);
                self.emit(|| AgentEvent::Unconsulted);
                Ok(AgentResponse::UnconsultedRecorded)
            }
            AgentRequest::RecordWarning { message } => self.record_warning(message),
            AgentRequest::RecordCompilerInvocation {
                outcome,
                crate_name,
                duration_ns,
            } => self.record_compiler_invocation(&outcome, crate_name.as_deref(), duration_ns),
            AgentRequest::RecordActionVerification { matched, restore } => {
                self.record_materialization(restore);
                self.stats.verifications.fetch_add(1, Ordering::Relaxed);
                if !matched {
                    self.stats.divergences.fetch_add(1, Ordering::Relaxed);
                }
                self.emit(|| AgentEvent::Verification { matched, restore });
                Ok(AgentResponse::ActionVerificationRecorded)
            }
            AgentRequest::StoreActionResult { result } => {
                self.store_action_result(&result, connection).await
            }
            AgentRequest::FindActionPrediction { task, invocation } => {
                self.find_action_prediction(&task, &invocation)
            }
            AgentRequest::RecordActionPrediction { task, prediction } => {
                self.record_action_prediction(&task, prediction)
            }
            AgentRequest::FindExecutableIdentity {
                executable,
                environment,
            } => self.find_executable_identity(executable, environment),
            AgentRequest::StoreExecutableIdentity {
                executable,
                environment,
                stdout,
            } => self.store_executable_identity(executable, environment, stdout),
            AgentRequest::Hello { .. } => {
                Err(eyre::eyre!("hello is only valid as the first request"))
            }
        };
        result.unwrap_or_else(|error| AgentResponse::Error {
            message: error.to_string(),
        })
    }

    async fn find_blob(&self, digest: &CacheDigest) -> Result<AgentResponse> {
        if let Some(path) = self.find_verified_blob(digest)? {
            return Ok(AgentResponse::Blob { path: Some(path) });
        }
        if !self.remote_mode.reads() {
            return Ok(AgentResponse::Blob { path: None });
        }
        let Some(remote) = &self.remote else {
            return Ok(AgentResponse::Blob { path: None });
        };
        match self.fetch_remote_blob(remote, digest).await {
            Ok(path) => Ok(AgentResponse::Blob { path: Some(path) }),
            Err(error) => {
                warn!(
                    "remote cache blob lookup failed for {}: {error}",
                    digest.hash
                );
                Ok(AgentResponse::Blob { path: None })
            }
        }
    }

    async fn find_blobs(&self, digests: Vec<CacheDigest>) -> Result<AgentResponse> {
        let mut paths = BTreeMap::new();
        let mut missing = Vec::new();
        for digest in &digests {
            match self.find_verified_blob(digest)? {
                Some(path) => {
                    paths.insert(digest.clone(), path);
                }
                None => {
                    missing.push(digest.clone());
                }
            }
        }

        if !missing.is_empty()
            && self.remote_mode.reads()
            && let Some(remote) = &self.remote
        {
            paths.extend(self.fetch_remote_blobs(remote, missing, None).await);
        }

        Ok(AgentResponse::Blobs {
            paths: digests
                .into_iter()
                .map(|digest| paths.get(&digest).cloned())
                .collect(),
        })
    }

    async fn store_blob(
        &self,
        digest: &CacheDigest,
        source: &Path,
        connection: &mut ConnectionUploads,
    ) -> Result<AgentResponse> {
        let path = {
            let lock = self.write_lock(digest);
            let _guard = lock.lock().await;
            if let Some(path) = self.find_verified_blob(digest)? {
                path
            } else {
                let path = self.cas.store_file(digest, source)?;
                self.remember_verified_blob(digest, &path);
                self.stats.stores.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .stored_bytes
                    .fetch_add(digest.size, Ordering::Relaxed);
                path
            }
        };
        // The object is durable locally, so the build has what it needs and the
        // remote publication can happen after this request returns.
        if let Some(uploads) = &self.uploads {
            uploads.queue_blob(digest, path.clone(), connection);
        }
        Ok(AgentResponse::Stored { path })
    }

    fn find_verified_blob(&self, digest: &CacheDigest) -> Result<Option<PathBuf>> {
        let remembered = self.verified_blobs.lock().unwrap().get(digest).cloned();
        if let Some(remembered) = remembered {
            // The contents behind this digest were hashed in full when the
            // session first reached for them. Hashing again on every later
            // lookup would re-read each dependency's rlib once per crate that
            // links it, so what is rechecked here is that the file has not
            // been written since: an overwrite moves the modification time,
            // and truncation or eviction changes the length or removes it.
            // Only a replacement that reproduces both -- which no writer in
            // the store does, since blobs are published by rename under a
            // content-derived name -- would go unnoticed until `mbx cache
            // verify` reads the store back in full.
            if remembered.is_unchanged() {
                return Ok(Some(remembered.path));
            }
            self.verified_blobs.lock().unwrap().remove(digest);
        }
        let path = self.cas.find(digest)?;
        if let Some(path) = &path {
            self.remember_verified_blob(digest, path);
        }
        Ok(path)
    }

    fn remember_verified_blob(&self, digest: &CacheDigest, path: &Path) {
        let Some(verified) = VerifiedBlob::describe(path) else {
            // Without an identity to compare against there is nothing to
            // shortcut safely, so the digest stays unremembered and every
            // lookup re-reads it.
            return;
        };
        self.verified_blobs
            .lock()
            .unwrap()
            .insert(digest.clone(), verified);
    }

    async fn find_action_result(&self, action: &CacheDigest) -> Result<AgentResponse> {
        if let Some(result) = self.actions.find(action)? {
            return Ok(AgentResponse::ActionResult {
                result: Some(result),
            });
        }
        if !self.remote_mode.reads() {
            return Ok(AgentResponse::ActionResult { result: None });
        }
        let Some(remote) = &self.remote else {
            return Ok(AgentResponse::ActionResult { result: None });
        };
        let lock = self.action_lock(action);
        let _guard = lock.lock().await;
        if let Some(result) = self.actions.find(action)? {
            return Ok(AgentResponse::ActionResult {
                result: Some(result),
            });
        }
        if let Some(result) = self
            .pending_remote_actions
            .lock()
            .unwrap()
            .get(action)
            .cloned()
        {
            return Ok(AgentResponse::ActionResult {
                result: Some(result),
            });
        }
        let _permit = self.remote_transfers.acquire().await?;
        match self.get_remote_action_result(remote, action).await {
            Ok(Some(result)) => {
                self.pending_remote_actions
                    .lock()
                    .unwrap()
                    .insert(action.clone(), result.clone());
                Ok(AgentResponse::ActionResult {
                    result: Some(result),
                })
            }
            Ok(None) => Ok(AgentResponse::ActionResult { result: None }),
            Err(error) => {
                self.note_remote_failure();
                warn!(
                    "remote cache action lookup failed for {}: {error}",
                    action.hash
                );
                Ok(AgentResponse::ActionResult { result: None })
            }
        }
    }

    async fn store_action_result(
        &self,
        result: &RemoteActionResult,
        connection: &ConnectionUploads,
    ) -> Result<AgentResponse> {
        let path = self.actions.store(result)?;
        if let Some(uploads) = &self.uploads {
            uploads.queue_action_result(result, connection);
        }
        Ok(AgentResponse::ActionStored { path })
    }

    /// Record that a remote operation failed and the build carried on without it.
    fn note_remote_failure(&self) {
        self.stats.remote_failures.fetch_add(1, Ordering::Relaxed);
    }

    async fn get_remote_action_result(
        &self,
        remote: &RemoteCacheClient,
        action: &CacheDigest,
    ) -> Result<Option<RemoteActionResult>> {
        self.stats
            .remote_action_lookups
            .fetch_add(1, Ordering::Relaxed);
        let _timer = AtomicDurationTimer::start(&self.stats.remote_action_lookup_duration_ns);
        remote.get_action_result(action).await
    }

    fn record_action_hit(
        &self,
        action: &CacheDigest,
        restore: RestoreStats,
        crate_name: Option<String>,
    ) -> Result<AgentResponse> {
        validate_crate_name(crate_name.as_deref())?;
        if self.actions.find(action)?.is_none() {
            let pending = self.pending_remote_actions.lock().unwrap().remove(action);
            if let Some(result) = pending {
                self.actions.store(&result)?;
            } else {
                bail!("cannot record a hit for a missing action result");
            }
        }
        self.record_restore(restore);
        self.stats.hits.fetch_add(1, Ordering::Relaxed);
        self.emit(|| AgentEvent::ActionHit {
            crate_name,
            restore,
        });
        Ok(AgentResponse::ActionHitRecorded)
    }

    fn record_restore(&self, restore: RestoreStats) {
        self.record_materialization(restore);
        atomic_saturating_add(
            &self.stats.avoided_compiler_duration_ns,
            restore.avoided_compiler_duration_ns,
        );
        atomic_saturating_add(&self.stats.restored_output_files, restore.output_files);
        atomic_saturating_add(&self.stats.restored_output_bytes, restore.output_bytes);
        atomic_saturating_add(
            &self.stats.reflinked_output_files,
            restore.reflinked_output_files,
        );
        atomic_saturating_add(
            &self.stats.reflinked_output_bytes,
            restore.reflinked_output_bytes,
        );
        atomic_saturating_add(&self.stats.copied_output_files, restore.copied_output_files);
        atomic_saturating_add(&self.stats.copied_output_bytes, restore.copied_output_bytes);
    }

    fn record_compiler_invocation(
        &self,
        outcome: &str,
        crate_name: Option<&str>,
        duration_ns: u64,
    ) -> Result<AgentResponse> {
        if !matches!(
            outcome,
            "miss" | "unconsulted" | "bypass" | "verification" | "incremental"
        ) {
            bail!("invalid compiler invocation outcome");
        }
        validate_crate_name(crate_name)?;
        let mut compiler = self.stats.compiler.lock().unwrap();
        let stats = compiler.entry(outcome.to_string()).or_default();
        stats.invocations = stats.invocations.saturating_add(1);
        stats.duration_ns = stats.duration_ns.saturating_add(duration_ns);
        drop(compiler);
        if outcome != "verification"
            && let Some(crate_name) = crate_name.filter(|name| !name.is_empty())
        {
            let mut slow = self.stats.slow_compilations.lock().unwrap();
            let duration = slow.entry(crate_name.to_string()).or_default();
            *duration = duration.saturating_add(duration_ns);
        }
        self.emit(|| AgentEvent::CompilerInvocation {
            outcome: outcome.to_string(),
            crate_name: crate_name.map(str::to_string),
            duration_ns,
        });
        Ok(AgentResponse::CompilerInvocationRecorded)
    }

    fn record_materialization(&self, restore: RestoreStats) {
        atomic_saturating_add(&self.stats.materialization_duration_ns, restore.duration_ns);
    }

    /// Surface a shim diagnostic on this process's stderr.
    ///
    /// The agent lives in the process that owns the session, so printing here
    /// reaches the terminal running the build rather than the stderr of the
    /// compilation the shim stands in for -- which build scripts read as part
    /// of the compiler's answer. Deduplicated because one cause tends to fire
    /// once per compilation, and capped so a message unique per compilation
    /// cannot scroll the build away.
    fn record_warning(&self, message: String) -> Result<AgentResponse> {
        if message.is_empty()
            || message.len() > MAX_WARNING_BYTES
            || message.contains(['\n', '\r', '\0'])
        {
            bail!("invalid shim warning");
        }
        let mut warnings = self.warnings.lock().unwrap();
        if !warnings.contains(&message) && warnings.len() < MAX_WARNINGS {
            eprintln!("mbx[warning]: {message}");
            self.emit(|| AgentEvent::Warning {
                message: message.clone(),
            });
            warnings.insert(message);
        }
        Ok(AgentResponse::WarningRecorded)
    }

    fn find_action_prediction(
        &self,
        task: &str,
        invocation: &CacheDigest,
    ) -> Result<AgentResponse> {
        validate_task_identity(task)?;
        invocation.validate()?;
        let prediction = self
            .task_actions
            .lock()
            .unwrap()
            .get(task)
            .and_then(|state| state.predictions.get(invocation))
            .cloned();
        Ok(AgentResponse::ActionPrediction { prediction })
    }

    fn record_action_prediction(
        &self,
        task: &str,
        prediction: ActionPrediction,
    ) -> Result<AgentResponse> {
        validate_task_identity(task)?;
        validate_action_prediction(&prediction)?;
        let mut tasks = self.task_actions.lock().unwrap();
        let state = tasks.entry(task.to_string()).or_default();
        if !state.predictions.contains_key(&prediction.invocation)
            && state.predictions.len() >= MAX_TASK_ACTION_PREDICTIONS
        {
            bail!("task action manifest contains too many predictions");
        }
        state
            .predictions
            .insert(prediction.invocation.clone(), prediction.clone());
        state
            .pending_predictions
            .insert(prediction.invocation.clone(), prediction);
        Ok(AgentResponse::ActionPredictionRecorded)
    }

    fn executable_identity_key(
        &self,
        executable: PathBuf,
        environment: BTreeMap<String, Option<String>>,
    ) -> Result<ExecutableIdentityKey> {
        // Restricted to the variables that actually select what an identity
        // probe reports: the toolchain rustup resolves, and the SDK a linker
        // driver builds against. Anything else would let one key stand for two
        // different compilers.
        if !environment.keys().all(|name| {
            matches!(
                name.as_str(),
                "RUSTUP_HOME" | "RUSTUP_TOOLCHAIN" | "SDKROOT" | "MACOSX_DEPLOYMENT_TARGET"
            )
        }) {
            bail!("executable identity contains an unsupported environment variable");
        }
        Ok(ExecutableIdentityKey {
            executable,
            environment,
        })
    }

    fn find_executable_identity(
        &self,
        executable: PathBuf,
        environment: BTreeMap<String, Option<String>>,
    ) -> Result<AgentResponse> {
        let key = self.executable_identity_key(executable, environment)?;
        let stdout = self
            .executable_identities
            .lock()
            .unwrap()
            .get(&key)
            .cloned();
        Ok(AgentResponse::ExecutableIdentity { stdout })
    }

    fn store_executable_identity(
        &self,
        executable: PathBuf,
        environment: BTreeMap<String, Option<String>>,
        stdout: Vec<u8>,
    ) -> Result<AgentResponse> {
        if stdout.len() > MAX_EXECUTABLE_IDENTITY_SIZE {
            bail!("executable identity exceeds {MAX_EXECUTABLE_IDENTITY_SIZE} bytes");
        }
        let key = self.executable_identity_key(executable, environment)?;
        let mut identities = self.executable_identities.lock().unwrap();
        let is_new = !identities.contains_key(&key);
        let previous_size = identities.get(&key).map_or(0, Vec::len);
        if is_new && identities.len() >= MAX_EXECUTABLE_IDENTITIES {
            bail!("executable identity cache contains too many entries");
        }
        let retained_bytes = identities.values().map(Vec::len).sum::<usize>();
        if retained_bytes - previous_size + stdout.len() > MAX_EXECUTABLE_IDENTITY_BYTES {
            bail!("executable identity cache contains too many bytes");
        }
        identities.insert(key, stdout.clone());
        Ok(AgentResponse::ExecutableIdentity {
            stdout: Some(stdout),
        })
    }

    /// Serve newline-delimited protocol requests on an authenticated session stream.
    pub async fn handle_connection<S>(&self, stream: S) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        let hello = read_request(&mut reader)
            .await?
            .ok_or_else(|| eyre::eyre!("connection closed before the agent handshake"))?;
        let request: AgentRequest = serde_json::from_str(&hello)?;
        match request {
            AgentRequest::Hello {
                protocol,
                client_version,
            } if protocol == AGENT_PROTOCOL_VERSION && client_version == self.version.as_ref() => {}
            AgentRequest::Hello { protocol, .. } if protocol != AGENT_PROTOCOL_VERSION => {
                send_response(
                    &mut writer,
                    &AgentResponse::Error {
                        message: format!(
                            "unsupported agent protocol {protocol}; expected {AGENT_PROTOCOL_VERSION}"
                        ),
                    },
                )
                .await?;
                return Ok(());
            }
            AgentRequest::Hello { client_version, .. } => {
                send_response(
                    &mut writer,
                    &AgentResponse::Error {
                        message: format!(
                            "cache client {client_version} does not match agent {}",
                            self.version
                        ),
                    },
                )
                .await?;
                return Ok(());
            }
            _ => bail!("the first agent request must be hello"),
        }
        send_response(
            &mut writer,
            &AgentResponse::Hello {
                protocol: AGENT_PROTOCOL_VERSION,
                agent_version: self.version.to_string(),
            },
        )
        .await?;

        // Tickets accumulate for the life of the connection because a shim
        // publishes a compilation's blobs and the action result naming them over
        // one connection, in that order.
        let mut connection = ConnectionUploads::default();
        while let Some(line) = read_request(&mut reader).await? {
            let response = match serde_json::from_str(&line) {
                Ok(request) => self.respond_on(request, &mut connection).await,
                Err(error) => AgentResponse::Error {
                    message: format!("invalid agent request: {error}"),
                },
            };
            send_response(&mut writer, &response).await?;
        }
        Ok(())
    }
}

/// Read one newline-delimited request, refusing one that grows past the cap.
///
/// Any process running as this user can open the session socket, so a request
/// that never terminates its line must not be able to grow the agent's memory
/// without bound.
async fn read_request<R>(reader: &mut R) -> Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            break;
        }
        let (consumed, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index, true),
            None => (available.len(), false),
        };
        if line.len() + consumed > MAX_REQUEST_BYTES {
            bail!("agent request exceeded {MAX_REQUEST_BYTES} bytes");
        }
        line.extend_from_slice(&available[..consumed]);
        // The newline itself is consumed but never kept.
        reader.consume(consumed + usize::from(complete));
        if complete {
            return Ok(Some(String::from_utf8(line)?));
        }
    }
    if line.is_empty() {
        Ok(None)
    } else {
        Ok(Some(String::from_utf8(line)?))
    }
}

/// Whether `task` is a well-formed task action identity.
///
/// Identities name files and directories in the store, so anything that reads
/// the store back has to be able to tell an identity from whatever else a user
/// left lying there.
pub fn is_task_identity(task: &str) -> bool {
    task.len() == 64
        && task
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_task_identity(task: &str) -> Result<()> {
    if !is_task_identity(task) {
        bail!("invalid task action identity");
    }
    Ok(())
}

/// Where a store keeps its task prediction manifests.
fn task_manifest_dir(store: &Path) -> PathBuf {
    store.join("task-manifests").join("v1")
}

/// The action digests a task's prediction manifest recorded.
///
/// Read straight off disk rather than through an agent, because a collector
/// needs the action set of tasks no session is running. A manifest that is
/// missing or no longer parseable yields no actions rather than an error: this
/// is a prediction index, so the worst a thin answer costs is a cold prefetch,
/// or an object collected earlier than it deserved.
pub fn task_manifest_actions(store: &Path, task: &str) -> Result<Vec<CacheDigest>> {
    validate_task_identity(task)?;
    let path = task_manifest_dir(store).join(format!("{task}.json"));
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
        }
    };
    let Ok(manifest) = serde_json::from_slice::<TaskActionManifest>(&bytes) else {
        return Ok(Vec::new());
    };
    if validate_task_manifest(&manifest, task).is_err() {
        return Ok(Vec::new());
    }
    Ok(manifest
        .predictions
        .into_iter()
        .map(|prediction| prediction.action)
        .collect())
}

fn validate_action_prediction(prediction: &ActionPrediction) -> Result<()> {
    match prediction.invalid_reason() {
        Some(reason) => bail!("invalid action prediction: {reason}"),
        None => Ok(()),
    }
}

fn validate_task_manifest(manifest: &TaskActionManifest, task: &str) -> Result<()> {
    if manifest.task == task && manifest.validate() {
        Ok(())
    } else {
        bail!("invalid task action manifest")
    }
}

fn merge_task_manifests(
    task: &str,
    base: Option<TaskActionManifest>,
    update: TaskActionManifest,
) -> Result<TaskActionManifest> {
    validate_task_manifest(&update, task)?;
    let mut predictions = BTreeMap::new();
    if let Some(base) = base {
        validate_task_manifest(&base, task)?;
        predictions.extend(
            base.predictions
                .into_iter()
                .map(|prediction| (prediction.invocation.clone(), prediction)),
        );
    }
    predictions.extend(
        update
            .predictions
            .into_iter()
            .map(|prediction| (prediction.invocation.clone(), prediction)),
    );
    let manifest = TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.to_owned(),
        predictions: predictions.into_values().collect(),
    };
    validate_task_manifest(&manifest, task)?;
    Ok(manifest)
}

fn merge_remote_task_manifest(
    task: &str,
    remote: TaskActionManifest,
    local: TaskActionManifest,
) -> (TaskActionManifest, bool) {
    match merge_task_manifests(task, Some(remote), local.clone()) {
        Ok(manifest) => (manifest, true),
        Err(error) => {
            warn!("remote task action manifest merge failed for {task}: {error}");
            (local, false)
        }
    }
}

async fn send_response(
    writer: &mut (impl AsyncWrite + Unpin),
    response: &AgentResponse,
) -> Result<()> {
    let mut encoded = serde_json::to_vec(response)?;
    encoded.push(b'\n');
    writer.write_all(&encoded).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
