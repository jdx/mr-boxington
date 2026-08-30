use super::{FileDigestScope, FileIdentity, RecordedFileDigest};
use crate::{ActionPrediction, CacheDigest, RemoteActionResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Wire protocol version used between an in-process cache agent and its shims.
pub const AGENT_PROTOCOL_VERSION: u8 = 5;
/// Largest single protocol request the agent will read.
///
/// Requests are small JSON objects; the largest legitimate ones carry an output
/// tree or a batch of digests, which stay far below this.
pub(super) const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

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
    /// Find session-recorded digests for files with these identities.
    FindFileDigests {
        /// What the recorded digests may stand in for.
        scope: FileDigestScope,
        /// File identities to resolve, preserving request order in the
        /// response.
        files: Vec<FileIdentity>,
    },
    /// Record digests of files a shim hashed or wrote this session.
    RecordFileDigests {
        /// What the recorded digests may stand in for.
        scope: FileDigestScope,
        /// Hashed files and the identities their digests describe.
        entries: Vec<RecordedFileDigest>,
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
    /// Number of output files already in place with the cached contents, kept
    /// rather than rewritten.
    pub reused_output_files: u64,
    /// Declared size of outputs kept in place rather than rewritten.
    pub reused_output_bytes: u64,
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
    /// Digests recorded earlier for the requested file identities.
    FileDigests {
        /// Recorded digests or misses, in request order.
        digests: Vec<Option<CacheDigest>>,
    },
    /// File digests were recorded.
    FileDigestsRecorded,
}
