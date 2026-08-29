//! Build-session lifecycle: the cache agent, its transport, and the rustc shim
//! that cargo invokes through `RUSTC_WRAPPER`.

use crate::config::Config;
use crate::events::{ActionDetail, ActionOutcome, EventWriter};
use crate::util::{duration_ns, format_duration, write_atomic};
use bytesize::ByteSize;
use eyre::{Context, Result, bail};
use log::{debug, warn};
use mbx_cache_cc::CcLanguage;
use mbx_cache_core::{
    AGENT_PROTOCOL_VERSION, AgentEvent, AgentEventObserver, AgentRemoteCache, AgentRequest,
    AgentResponse, AgentStats, CacheAgent, CacheDigest, FileDigestCache, FileDigestScope,
    FileIdentity, NoFileDigestCache, RecordedFileDigest, canonical_json,
};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub const RUSTC_SHIM_STEM: &str = "mbx-rustc";
pub const CC_SHIM_STEM: &str = "mbx-cc";
pub const CXX_SHIM_STEM: &str = "mbx-cxx";
pub(crate) const PATH_SHIMS_ENV: &str = "MBX_CC_SHIM_COMPILERS";
const SOCKET_ENV: &str = "MBX_SOCKET";
pub(crate) const REAL_CC_ENV: &str = "MBX_REAL_CC";
pub(crate) const REAL_CXX_ENV: &str = "MBX_REAL_CXX";
pub(crate) const STAGING_ENV: &str = "MBX_STAGING_DIR";
pub(crate) const BUILD_ENV: &str = "MBX_BUILD";
pub(crate) const VERIFY_ENV: &str = "MBX_VERIFY";
pub(crate) const SHARE_OUT_DIR_ENV: &str = "MBX_SHARE_OUT_DIR";
pub(crate) const LEARNED_INCREMENTAL_ENV: &str = "MBX_LEARNED_INCREMENTAL";
pub const CACHE_LINKS_ENV: &str = "MBX_CACHE_LINKS";
pub(crate) const WORKSPACE_ROOT_ENV: &str = "MBX_WORKSPACE_ROOT";
pub(crate) const TARGET_DIR_ENV: &str = "MBX_TARGET_DIR";
const PREVIOUS_RUSTC_WRAPPER_ENV: &str = "MBX_PREVIOUS_RUSTC_WRAPPER";
pub(crate) const BYPASS_LOG_ENV: &str = "MBX_BYPASS_LOG";
const VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg(unix)]
static SHIM_STAGING_NONCE: AtomicU64 = AtomicU64::new(0);

/// A cache session: the agent, its listener, and the shim cargo will invoke.
pub struct CacheSession {
    socket: String,
    rustc_shim: PathBuf,
    cc_shims: Option<CcShims>,
    staging: PathBuf,
    verify: bool,
    incremental: bool,
    share_out_dir: bool,
    agent: CacheAgent,
    /// The stream `mbx tui` watches, when event recording is on.
    events: Option<EventStream>,
    /// What the shims need to draw compile permits from the machine-wide pool.
    scheduler_env: Vec<(String, String)>,
    store: PathBuf,
    started: Instant,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    server: Mutex<Option<JoinHandle<Result<()>>>>,
}

impl CacheSession {
    /// Install the shim, start the agent, and begin serving the shim's requests.
    ///
    /// `session_dir` holds the shim, socket, and staging directory, and is
    /// expected to be a temporary directory owned by the caller.
    pub async fn start(session_dir: &Path, config: &Config) -> Result<Self> {
        let shim = install_session_shim(session_dir)?;
        let cc_shims = if config.cc {
            // Build systems such as CMake persist HOST_CC as an absolute
            // compiler path. Keep the C/C++ shims outside the temporary
            // session so that path remains executable on the next mbx run.
            install_cc_shims(&config.cache_dir.join("shims"))?
        } else {
            None
        };
        let staging = session_dir.join("staging");
        std::fs::create_dir(&staging)?;
        let store = config.store_dir();
        let agent = if let Some(remote) = action_remote_cache(config, &store)? {
            CacheAgent::new_remote_with_download_limit(
                store.clone(),
                VERSION,
                remote,
                config.gc.max_bytes,
            )
        } else {
            CacheAgent::new(store.clone(), VERSION)
        };
        let events = config
            .events
            .then(|| Arc::new(EventWriter::new(&store)))
            .map(EventStream);
        let agent = match &events {
            Some(events) => agent.with_observer(Arc::new(events.clone())),
            None => agent,
        };
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (socket, server) = spawn_server(session_dir, agent.clone(), shutdown_rx).await?;
        Ok(Self {
            socket,
            rustc_shim: shim,
            cc_shims,
            staging,
            verify: config.verify,
            incremental: config.incremental,
            share_out_dir: config.share_out_dir,
            agent,
            events,
            scheduler_env: crate::scheduler::session_environment(config),
            store,
            started: Instant::now(),
            shutdown: Mutex::new(Some(shutdown_tx)),
            server: Mutex::new(Some(server)),
        })
    }

    /// Begin the build's action run and return the environment cargo needs.
    ///
    /// `environment` is amended in place so a caller can pass the environment it
    /// already intends to hand to cargo. Any `RUSTC_WRAPPER` already present is
    /// preserved for the shim to chain to. The manifest identity is derived from
    /// `workspace_root` and `command` here so that callers cannot supply one the
    /// protocol would reject.
    pub async fn begin(
        &self,
        workspace_root: &Path,
        target_dir: &Path,
        command: &[String],
        environment: &mut BTreeMap<String, String>,
    ) -> Option<ActionRun> {
        let identity = build_identity(workspace_root, command);
        // Named before the first compilation, so a TUI that attaches mid-build
        // can say whose build it is watching rather than showing bare rows.
        if let Some(events) = &self.events {
            events.0.started(workspace_root, command);
        }
        // Recorded before the build rather than after it: a build that fails
        // still means this checkout is here and using the store, and the record
        // is what stops the collector treating its artifacts as abandoned.
        if let Err(error) =
            crate::store::record_checkout(&self.store, &identity, workspace_root, target_dir)
        {
            warn!("this checkout was not recorded as a cache root: {error}");
        }
        let (protocol_build, action_run) = match self.agent.begin_task(&identity).await {
            Ok(run) => (
                run.clone(),
                Some(ActionRun {
                    run,
                    agent: self.agent.clone(),
                }),
            ),
            Err(error) => {
                warn!("build action manifest was not loaded: {error}");
                (identity, None)
            }
        };
        let shim = self.rustc_shim.to_string_lossy().into_owned();
        // The shim maps these roots out of its cache keys; a dependency compiles
        // with its working directory in the registry, so it cannot find them.
        environment.insert(
            WORKSPACE_ROOT_ENV.into(),
            workspace_root.to_string_lossy().into_owned(),
        );
        environment.insert(
            TARGET_DIR_ENV.into(),
            target_dir.to_string_lossy().into_owned(),
        );
        environment.insert(SOCKET_ENV.into(), self.socket.clone());
        environment.insert(
            STAGING_ENV.into(),
            self.staging.to_string_lossy().into_owned(),
        );
        environment.insert(BUILD_ENV.into(), protocol_build);
        // Always state this explicitly: removing the key would leave the shim
        // inheriting whatever the parent environment had.
        environment.insert(
            VERIFY_ENV.into(),
            if self.verify { "1" } else { "0" }.into(),
        );
        environment.insert(
            SHARE_OUT_DIR_ENV.into(),
            if self.share_out_dir { "1" } else { "0" }.into(),
        );
        for (name, value) in &self.scheduler_env {
            environment.insert(name.clone(), value.clone());
        }
        if let Some(previous) = environment.insert("RUSTC_WRAPPER".into(), shim.clone())
            && previous != shim
        {
            // The shim defers to a wrapper that was already configured rather
            // than compiling around it, since that wrapper may do more than
            // cache. Say so, because the alternative is a silent no-op.
            warn!(
                "RUSTC_WRAPPER is already set to {previous}; deferring to it, so this build is not cached"
            );
            environment.insert(PREVIOUS_RUSTC_WRAPPER_ENV.into(), previous);
        }
        self.begin_cc(environment);
        if self.incremental {
            // Hand the decision back to cargo, which compiles local packages
            // incrementally in dev profiles and never in release. Not
            // overriding is the whole of the feature: the actions themselves
            // still bypass, they are just faster to recompile.
            environment.remove("CARGO_INCREMENTAL");
        } else {
            // Incremental compilation is never cacheable, so it is disabled
            // rather than left to bypass every action.
            environment.insert("CARGO_INCREMENTAL".into(), "0".into());
        }
        action_run
    }

    /// Point build scripts at the C and C++ shims.
    ///
    /// A build that already chose its compiler keeps it. Unlike `RUSTC_WRAPPER`,
    /// `CC` is commonly exported machine-wide for reasons that have nothing to
    /// do with this build, so standing aside is unremarkable and is logged
    /// rather than warned about; the session summary already reports how much
    /// of the build the cache covered.
    fn begin_cc(&self, environment: &mut BTreeMap<String, String>) {
        let Some(shims) = &self.cc_shims else {
            return;
        };
        // A build that named its own host compiler keeps it. This does not
        // stand in the way of the targeted shims below: those wrap a compiler
        // the build named rather than replacing one it chose, and the `cc`
        // crate reads the host and target variables in different builds.
        let host_chosen = CC_CRATE_ENV
            .iter()
            .find(|name| environment.contains_key(**name) || std::env::var_os(name).is_some());
        match host_chosen {
            Some(name) => {
                debug!("{name} is already set; host C and C++ compilations are not cached");
            }
            // `HOST_CC` rather than `CC`, because of where each sits in the
            // `cc` crate's lookup order: it reads `CC_<target>`, then
            // `HOST_CC` or `TARGET_CC` depending on whether it is
            // cross-compiling, and only then plain `CC`. Setting `CC` would
            // capture cross compiles too, and these shims wrap the *host*
            // compiler -- a `cargo build --target` would silently build target
            // objects with the host driver.
            None => shims.apply_host(environment),
        }
        shims.apply_targeted(environment);
        // Each targeted shim finds the compiler it stands in for through this
        // map, keyed by the name it is invoked under -- the same mechanism the
        // standalone shims use.
        let pins = shims.pins();
        if !pins.is_empty()
            && let Ok(encoded) = serde_json::to_string(&pins)
        {
            environment.insert(PATH_SHIMS_ENV.into(), encoded);
        }
    }

    /// Begin a standalone build's action run and return the environment it
    /// needs.
    ///
    /// The cargo-specific keys -- `RUSTC_WRAPPER`, `HOST_CC`,
    /// `CARGO_INCREMENTAL` -- stay untouched: the build finds its compilers
    /// through the shim directory placed first on `PATH` instead.
    pub async fn begin_exec(
        &self,
        project_root: &Path,
        command: &[String],
        shims: &PathShims,
        environment: &mut BTreeMap<String, String>,
    ) -> Option<ActionRun> {
        let identity = exec_identity(project_root, command);
        if let Some(events) = &self.events {
            events.0.started(project_root, command);
        }
        // The project root stands in for the target directory: a standalone
        // build owns its output directory, so there is nothing managed to
        // record, but the checkout itself must be known for its objects to
        // count as reachable.
        if let Err(error) =
            crate::store::record_checkout(&self.store, &identity, project_root, project_root)
        {
            warn!("this checkout was not recorded as a cache root: {error}");
        }
        let (protocol_build, action_run) = match self.agent.begin_task(&identity).await {
            Ok(run) => (
                run.clone(),
                Some(ActionRun {
                    run,
                    agent: self.agent.clone(),
                }),
            ),
            Err(error) => {
                warn!("build action manifest was not loaded: {error}");
                (identity, None)
            }
        };
        environment.insert(
            WORKSPACE_ROOT_ENV.into(),
            project_root.to_string_lossy().into_owned(),
        );
        environment.insert(SOCKET_ENV.into(), self.socket.clone());
        environment.insert(
            STAGING_ENV.into(),
            self.staging.to_string_lossy().into_owned(),
        );
        environment.insert(BUILD_ENV.into(), protocol_build);
        environment.insert(
            VERIFY_ENV.into(),
            if self.verify { "1" } else { "0" }.into(),
        );
        for (name, value) in &self.scheduler_env {
            environment.insert(name.clone(), value.clone());
        }
        if let Ok(pins) = serde_json::to_string(&shims.compilers) {
            environment.insert(PATH_SHIMS_ENV.into(), pins);
        }
        // First on PATH, so `make`'s default `cc` and an explicit `CC=gcc`
        // both resolve to a shim, which chains to the compiler pinned above.
        let path = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(shims.directory.clone()).chain(std::env::split_paths(&path));
        if let Ok(joined) = std::env::join_paths(paths) {
            environment.insert("PATH".into(), joined.to_string_lossy().into_owned());
        }
        action_run
    }

    /// Warm the recorded actions for a Cargo command without running Cargo.
    pub async fn prefetch(&self, workspace_root: &Path, command: &[String]) -> Result<()> {
        let identity = build_identity(workspace_root, command);
        self.agent.prefetch_task(&identity).await?;
        Ok(())
    }

    /// Stop the agent and collect this session's statistics.
    pub async fn finish(&self) -> Result<AgentStats> {
        if let Some(shutdown) = self.shutdown.lock().unwrap().take() {
            let _ = shutdown.send(());
        }
        let server = self.server.lock().unwrap().take();
        // Held rather than raised: the shims have already been told their objects
        // were stored, so a listener that failed is no reason to abandon the
        // uploads that promise implies.
        let served = match server {
            Some(server) => server
                .await
                .map_err(eyre::Report::from)
                .and_then(|served| served),
            None => Ok(()),
        };
        self.agent.cancel_prefetches().await;
        // Cancelling first hands the queue the transfer budget the abandoned
        // downloads were holding.
        self.agent.wait_for_uploads().await;
        served?;
        let mut stats = self.agent.stats();
        stats.session_duration_ns = duration_ns(self.started.elapsed());
        // The same totals the summary reports, so a reader of a finished stream
        // does not have to re-derive them from the rows -- and so a stream that
        // hit its row cap still ends with the whole truth.
        if let Some(events) = &self.events
            && let Ok(totals) = serde_json::to_value(StatsReport::from(&stats))
        {
            events.0.finished(totals);
        }
        Ok(stats)
    }
}

impl Drop for CacheSession {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.get_mut().unwrap().take() {
            let _ = shutdown.send(());
        }
        if let Some(server) = self.server.get_mut().unwrap().take() {
            server.abort();
        }
    }
}

/// Writes the agent's decisions to this session's event stream.
///
/// The translation lives here rather than in the agent because the agent
/// accounts for compilations and has no opinion about who is watching. What it
/// reports as a compiler invocation becomes a miss, an unconsulted compilation,
/// or a verification row, since that is the distinction a reader wants; a
/// bypass's own compile is dropped, because the bypass row already said so.
#[derive(Clone)]
struct EventStream(Arc<EventWriter>);

impl AgentEventObserver for EventStream {
    fn event(&self, event: AgentEvent) {
        match event {
            AgentEvent::ActionHit {
                crate_name,
                restore,
            } => self.0.action(
                ActionOutcome::Hit,
                crate_name,
                restore.duration_ns,
                ActionDetail {
                    avoided_compiler_ns: restore.avoided_compiler_duration_ns,
                    output_files: restore.output_files,
                    output_bytes: restore.output_bytes,
                    reflinked_output_bytes: restore.reflinked_output_bytes,
                    copied_output_bytes: restore.copied_output_bytes,
                },
            ),
            AgentEvent::Bypass { kind } => self.0.action(
                ActionOutcome::Bypass { reason: kind },
                None,
                0,
                ActionDetail::default(),
            ),
            // Nothing is emitted for the counter itself: the compiler
            // invocation that follows carries the same fact with a crate name
            // and a duration attached, and two rows would double-count it.
            AgentEvent::Unconsulted => {}
            AgentEvent::CompilerInvocation {
                outcome,
                crate_name,
                duration_ns,
            } => {
                let outcome = match outcome.as_str() {
                    "miss" => ActionOutcome::Miss,
                    "unconsulted" => ActionOutcome::Unconsulted,
                    // A verification's own row comes from the verification
                    // event, which knows whether it matched; a bypass already
                    // has one.
                    _ => return,
                };
                self.0
                    .action(outcome, crate_name, duration_ns, ActionDetail::default());
            }
            AgentEvent::Verification { matched, restore } => self.0.action(
                ActionOutcome::Verification { matched },
                None,
                restore.duration_ns,
                ActionDetail::default(),
            ),
            // A decision this build does not know how to describe is left out
            // rather than guessed at; the totals still count it.
            _ => {}
        }
    }
}

/// An in-flight build's completed action manifest.
pub struct ActionRun {
    run: String,
    agent: CacheAgent,
}

impl ActionRun {
    pub async fn commit(self) -> Result<()> {
        self.agent.commit_task(&self.run).await
    }
}

/// Identity for this build's prefetch manifest.
///
/// Manifests are namespaced by identity, so this only affects how well one
/// build can predict another's actions; action keys themselves are independent
/// of it.
pub fn build_identity(workspace_root: &Path, command: &[String]) -> String {
    mbx_cache_cargo::build_identity(workspace_root, command)
}

/// Identity material for a standalone build's prediction manifest.
///
/// Its own record rather than a reuse of the Cargo one: the two identity
/// spaces must not collide, and the Cargo crate keeps its material private
/// precisely so its scheme can evolve for Cargo's own reasons.
#[derive(Serialize)]
struct ExecIdentity<'a> {
    version: u8,
    project: &'a str,
    command: &'a [String],
}

/// Identity of a standalone build: the project and the command it runs.
///
/// Worktrees of one project must share a manifest for predictions to travel --
/// the cc adapter has no second way to build a key -- so the marker prefers
/// content and origin over location: a `Cargo.lock` digest where one exists,
/// then the git origin URL, and only then the directory name.
pub fn exec_identity(project_root: &Path, command: &[String]) -> String {
    let project = exec_marker(project_root);
    let material = ExecIdentity {
        version: 1,
        project: &project,
        command,
    };
    let bytes = canonical_json(&material).expect("exec identity must serialize");
    CacheDigest::blake3(&bytes).hash
}

fn exec_marker(project_root: &Path) -> String {
    if let Ok(lock) = std::fs::read(project_root.join("Cargo.lock")) {
        return CacheDigest::blake3(&lock).hash;
    }
    if let Some(origin) = git_origin_marker(project_root) {
        return origin;
    }
    project_root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// The origin URL names a project the same way in every worktree and clone,
/// which a checkout's directory name does not.
fn git_origin_marker(project_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        return None;
    }
    Some(format!("origin\0{url}"))
}

fn action_remote_cache(config: &Config, store: &Path) -> Result<Option<AgentRemoteCache>> {
    let Some(client) = crate::remote::remote_client(config)? else {
        return Ok(None);
    };
    // Release builds publish artifacts that must not depend on a cache, so they
    // do not read one either. The CLI already skips the whole session; this
    // keeps a library caller from reaching the remote behind its back.
    if crate::policy::release_context() {
        return Ok(None);
    }
    let Some(mode) = crate::policy::effective_remote_cache_mode(config.remote.mode) else {
        return Ok(None);
    };
    Ok(Some(AgentRemoteCache {
        client,
        mode,
        staging_dir: store.join("remote"),
    }))
}

#[derive(Serialize)]
struct StatsReport {
    version: u8,
    session_duration_ns: u64,
    lookups: u64,
    hits: u64,
    misses: u64,
    unconsulted: u64,
    compiler_invocations_avoided: u64,
    estimated_compiler_duration_avoided_ns: u64,
    compiler: BTreeMap<String, CompilerStatsReport>,
    slow_compilations: Vec<SlowCompilationReport>,
    verifications: u64,
    divergences: u64,
    prefetched_actions: u64,
    predictions_loaded: u64,
    prefetch_runs: u64,
    bypasses: BTreeMap<String, u64>,
    downloaded_bytes: u64,
    uploaded_bytes: u64,
    background_uploads: u64,
    background_upload_failures: u64,
    remote_blob_pack_uploads: u64,
    remote_blob_pack_upload_blobs: u64,
    upload_drain_duration_ns: u64,
    stored_bytes: u64,
    restored_output_files: u64,
    restored_output_bytes: u64,
    reflinked_output_files: u64,
    reflinked_output_bytes: u64,
    copied_output_files: u64,
    copied_output_bytes: u64,
    reused_output_files: u64,
    reused_output_bytes: u64,
    remote_failures: u64,
    remote_manifest_lookups: u64,
    remote_action_lookups: u64,
    remote_blob_requests: u64,
    remote_blob_pack_requests: u64,
    remote_blob_pack_blobs: u64,
    remote_manifest_lookup_duration_ns: u64,
    remote_action_lookup_duration_ns: u64,
    remote_blob_transfer_duration_ns: u64,
    local_cas_write_duration_ns: u64,
    prefetch_duration_ns: u64,
    materialization_duration_ns: u64,
}

#[derive(Serialize)]
struct CompilerStatsReport {
    invocations: u64,
    duration_ns: u64,
}

#[derive(Serialize)]
struct SlowCompilationReport {
    crate_name: String,
    duration_ns: u64,
}

impl From<&AgentStats> for StatsReport {
    fn from(stats: &AgentStats) -> Self {
        Self {
            version: 4,
            session_duration_ns: stats.session_duration_ns,
            lookups: stats.lookups,
            hits: stats.hits,
            misses: cache_misses(stats),
            unconsulted: stats.unconsulted,
            compiler_invocations_avoided: stats.hits,
            estimated_compiler_duration_avoided_ns: stats.avoided_compiler_duration_ns,
            compiler: stats
                .compiler
                .iter()
                .map(|(outcome, stats)| {
                    (
                        outcome.clone(),
                        CompilerStatsReport {
                            invocations: stats.invocations,
                            duration_ns: stats.duration_ns,
                        },
                    )
                })
                .collect(),
            slow_compilations: slow_compilations(stats)
                .into_iter()
                .map(|(crate_name, duration_ns)| SlowCompilationReport {
                    crate_name: crate_name.clone(),
                    duration_ns: *duration_ns,
                })
                .collect(),
            verifications: stats.verifications,
            divergences: stats.divergences,
            prefetched_actions: stats.prefetched_actions,
            predictions_loaded: stats.predictions_loaded,
            prefetch_runs: stats.prefetch_runs,
            bypasses: stats.bypasses.clone(),
            downloaded_bytes: stats.downloaded_bytes,
            uploaded_bytes: stats.uploaded_bytes,
            background_uploads: stats.background_uploads,
            background_upload_failures: stats.background_upload_failures,
            remote_blob_pack_uploads: stats.remote_blob_pack_uploads,
            remote_blob_pack_upload_blobs: stats.remote_blob_pack_upload_blobs,
            upload_drain_duration_ns: stats.upload_drain_duration_ns,
            stored_bytes: stats.stored_bytes,
            restored_output_files: stats.restored_output_files,
            restored_output_bytes: stats.restored_output_bytes,
            reflinked_output_files: stats.reflinked_output_files,
            reflinked_output_bytes: stats.reflinked_output_bytes,
            copied_output_files: stats.copied_output_files,
            copied_output_bytes: stats.copied_output_bytes,
            reused_output_files: stats.reused_output_files,
            reused_output_bytes: stats.reused_output_bytes,
            remote_failures: stats.remote_failures,
            remote_manifest_lookups: stats.remote_manifest_lookups,
            remote_action_lookups: stats.remote_action_lookups,
            remote_blob_requests: stats.remote_blob_requests,
            remote_blob_pack_requests: stats.remote_blob_pack_requests,
            remote_blob_pack_blobs: stats.remote_blob_pack_blobs,
            remote_manifest_lookup_duration_ns: stats.remote_manifest_lookup_duration_ns,
            remote_action_lookup_duration_ns: stats.remote_action_lookup_duration_ns,
            remote_blob_transfer_duration_ns: stats.remote_blob_transfer_duration_ns,
            local_cas_write_duration_ns: stats.local_cas_write_duration_ns,
            prefetch_duration_ns: stats.prefetch_duration_ns,
            materialization_duration_ns: stats.materialization_duration_ns,
        }
    }
}

/// Report a finished session to stderr, and to a JSON file when configured.
pub fn display_stats(stats: &AgentStats, config: &Config) {
    if let Some(path) = &config.stats_report
        && let Err(error) = write_stats_report(path, stats)
    {
        warn!(
            "the statistics report could not be written to {}: {error}",
            path.display()
        );
    }
    if !should_display_stats(stats) {
        return;
    }
    note(&format!(
        "mbx[cache]: {} hits, {} misses, {} prefetched; {} downloaded, {} uploaded, {} stored locally",
        stats.hits,
        cache_misses(stats),
        stats.prefetched_actions,
        ByteSize::b(stats.downloaded_bytes).display().iec(),
        ByteSize::b(stats.uploaded_bytes).display().iec(),
        ByteSize::b(stats.stored_bytes).display().iec(),
    ));
    if stats.remote_failures > 0 {
        // A remote cache that fails every request reports the same hits, misses
        // and bytes as one that was simply empty, and the warnings explaining
        // why scroll past hundreds of lines earlier. Without this line the
        // summary reads as "the remote had nothing for us" no matter which it
        // was, and a cache that has stopped working looks like a cold one.
        note(&format!(
            "mbx[cache]: the remote cache failed {} of its requests; this build ran without what it could not reach, and the warnings above say why",
            stats.remote_failures,
        ));
    }
    if stats.unconsulted > 0 {
        // A cold target directory hits this for everything it compiles, and
        // reporting only hits and misses there says "0 hits, 0 misses" -- which
        // reads as though the cache was asked and had nothing, rather than that
        // it was never asked. These compilations are still stored afterwards.
        note(&format!(
            "mbx[cache]: could not look up {} compilations: no usable dep-info from an earlier build and no prediction to derive an action key from",
            stats.unconsulted,
        ));
        if let Some(explanation) = stale_manifest_note(stats) {
            note(&explanation);
        }
    }
    if !stats.bypasses.is_empty() {
        let total: u64 = stats.bypasses.values().sum();
        // Most frequent first: the head of this list is where the next win is.
        let mut reasons: Vec<_> = stats.bypasses.iter().collect();
        reasons.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
        let detail = reasons
            .iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(", ");
        note(&format!(
            "mbx[cache]: bypassed {total} compilations: {detail}"
        ));
    }
    if !stats.compiler.is_empty() || stats.avoided_compiler_duration_ns > 0 {
        let spent = stats.compiler.values().fold(0_u64, |total, compiler| {
            total.saturating_add(compiler.duration_ns)
        });
        let detail = stats
            .compiler
            .iter()
            .map(|(outcome, compiler)| {
                format!(
                    "{} {} in {}",
                    compiler.invocations,
                    outcome,
                    format_nanos(compiler.duration_ns)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        note(&format!(
            "mbx[cache]: compiler time: {} estimated avoided; {} spent ({detail})",
            format_nanos(stats.avoided_compiler_duration_ns),
            format_nanos(spent),
        ));
        if let Some(compiler) = stats.compiler.get("incremental") {
            // The compiler-time line above already counts these; what it cannot
            // say is why they are absent from the store.
            note(&format!(
                "mbx[cache]: {} compilations kept their own incremental state, so they were not stored",
                compiler.invocations
            ));
        }
        let slow = slow_compilations(stats);
        if !slow.is_empty() {
            note(&format!(
                "mbx[cache]: slowest uncached crates: {}",
                slow.into_iter()
                    .map(|(name, duration)| format!("{name} {}", format_nanos(*duration)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let remote_lookup_duration_ns = stats
        .remote_manifest_lookup_duration_ns
        .saturating_add(stats.remote_action_lookup_duration_ns);
    note(&format!(
        "mbx[cache]: timing: {} session, {} prefetch; cumulative {} remote lookup, {} blob transfer, {} CAS write, {} materialization",
        format_nanos(stats.session_duration_ns),
        format_nanos(stats.prefetch_duration_ns),
        format_nanos(remote_lookup_duration_ns),
        format_nanos(stats.remote_blob_transfer_duration_ns),
        format_nanos(stats.local_cas_write_duration_ns),
        format_nanos(stats.materialization_duration_ns),
    ));
    if stats.background_uploads > 0 || stats.background_upload_failures > 0 {
        let packed = if stats.remote_blob_pack_uploads > 0 {
            format!(
                " ({} of them in {} packs)",
                stats.remote_blob_pack_upload_blobs, stats.remote_blob_pack_uploads
            )
        } else {
            String::new()
        };
        note(&format!(
            "mbx[cache]: uploads: {} published{packed}, {} not published; {} waited for after the build",
            stats.background_uploads,
            stats.background_upload_failures,
            format_nanos(stats.upload_drain_duration_ns),
        ));
    }
    if stats.restored_output_files > 0 {
        note(&format!(
            "mbx[cache]: materialization: {} outputs ({}) reflinked, {} outputs ({}) copied, {} outputs ({}) already in place",
            stats.reflinked_output_files,
            ByteSize::b(stats.reflinked_output_bytes).display().iec(),
            stats.copied_output_files,
            ByteSize::b(stats.copied_output_bytes).display().iec(),
            stats.reused_output_files,
            ByteSize::b(stats.reused_output_bytes).display().iec(),
        ));
    }
    if stats.verifications > 0 {
        note(&format!(
            "mbx[cache]: qualification: {} verified, {} diverged",
            stats.verifications, stats.divergences,
        ));
    }
}

/// Explain a session that loaded a full manifest and matched none of it.
///
/// "No prediction to derive an action key from" reads as an empty store, and
/// for a warm store that just watched its compiler change underneath it -- a
/// CI runner image updating its preinstalled toolchain is the common way --
/// that reading sends people hunting for restore failures. The distinction is
/// observable: predictions were loaded, and not one lookup was ever made.
fn stale_manifest_note(stats: &AgentStats) -> Option<String> {
    (stats.unconsulted > 0 && stats.lookups == 0 && stats.predictions_loaded > 0).then(|| {
        format!(
            "mbx[cache]: a manifest predicting {} compilations was loaded, but none matched this build; the compiler or its flags changed since they were recorded (a toolchain update does this)",
            stats.predictions_loaded,
        )
    })
}

/// Whether the cache took part in this build at all.
///
/// A run that never consulted or stored anything -- `cargo --help`, a build
/// cargo declined -- has nothing to report and should not be counted as a
/// build in the lifetime totals.
pub(crate) fn session_was_active(stats: &AgentStats) -> bool {
    should_display_stats(stats)
}

fn should_display_stats(stats: &AgentStats) -> bool {
    stats.lookups > 0
        || stats.unconsulted > 0
        || stats.stores > 0
        || stats.verifications > 0
        || stats.downloaded_bytes > 0
        || stats.uploaded_bytes > 0
        // An action result published on its own moves no payload bytes, and is
        // still the whole of what a session did.
        || stats.background_uploads > 0
        || !stats.bypasses.is_empty()
        || !stats.compiler.is_empty()
        || stats.avoided_compiler_duration_ns > 0
        // A session that reached a remote cache and got nothing but failures has
        // nothing else to report, and is the session most worth reporting.
        || stats.remote_failures > 0
}

/// Write to stderr without failing the build when the pipe is closed.
pub(crate) fn note(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Whether this process's stderr belongs to the compiler it stands in for.
///
/// Set once at cc-shim entry. Build scripts read an intercepted compiler's
/// stderr as part of its answer -- cc-rs marks a probed flag unsupported the
/// moment anything lands there -- so one printed warning changes the flags of
/// every compilation the build script produces afterwards, and with them
/// every action key, orphaning the whole build's predictions.
static STDERR_RESERVED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Declare that this process replays compiler output on stderr and must not
/// mix its own diagnostics into it.
pub(crate) fn reserve_stderr_for_compiler() {
    STDERR_RESERVED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Report a shim diagnostic without polluting a reserved stderr.
///
/// Delivered to the session's agent, which prints it from the process that
/// owns the build. When the agent cannot take it, the message falls back to
/// this process's stderr only where that stream is not the compiler's --
/// losing a diagnostic costs a little visibility, while poisoning a configure
/// probe costs the build its cache keys.
pub(crate) fn report_shim_warning(message: &str) {
    let mut message = message.replace(['\n', '\r'], "; ");
    // Stay under the agent's acceptance limit rather than losing the whole
    // diagnostic to it; the start of an error chain names the failure.
    if message.len() > 2048 {
        let end = (0..=2048).rfind(|&index| message.is_char_boundary(index));
        message.truncate(end.unwrap_or_default());
        message.push_str("...");
    }
    let delivered = matches!(
        request_agent(&[AgentRequest::RecordWarning {
            message: message.clone(),
        }])
        .map(|responses| responses.into_iter().next()),
        Ok(Some(AgentResponse::WarningRecorded))
    );
    if !delivered && !STDERR_RESERVED.load(std::sync::atomic::Ordering::Relaxed) {
        note(&format!("mbx[warning]: {message}"));
    }
}

fn write_stats_report(path: &Path, stats: &AgentStats) -> Result<()> {
    let mut report = serde_json::to_vec_pretty(&StatsReport::from(stats))?;
    report.push(b'\n');
    write_atomic(path, &report)
}

fn format_nanos(nanoseconds: u64) -> String {
    format_duration(std::time::Duration::from_nanos(nanoseconds))
}

fn slow_compilations(stats: &AgentStats) -> Vec<(&String, &u64)> {
    let mut slow = stats.slow_compilations.iter().collect::<Vec<_>>();
    slow.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
    slow.truncate(5);
    slow
}

fn cache_misses(stats: &AgentStats) -> u64 {
    stats
        .lookups
        .saturating_sub(stats.hits)
        .saturating_sub(stats.verifications)
}

/// Installed C and C++ shims, and the compilers they stand in for.
struct CcShims {
    /// Installed C shim and the compiler it stands in for, when there is one.
    cc: Option<(PathBuf, PathBuf)>,
    /// The same for C++.
    cxx: Option<(PathBuf, PathBuf)>,
    /// Target-specific compilers the build named, each behind its own shim.
    targeted: Vec<TargetedCompiler>,
}

/// A compiler a cross build asked for by name, and the shim standing in for it.
struct TargetedCompiler {
    /// The `cc` crate variable that named it, such as `CC_aarch64-linux-musl`.
    variable: String,
    /// File name of the shim installed for it, which is also its pin key.
    shim_name: String,
    /// Absolute path to the shim.
    shim: PathBuf,
    /// The compiler the build chose, which the shim execs.
    real: PathBuf,
}

impl CcShims {
    /// Point build scripts at whichever shims were installed.
    ///
    /// A language with no compiler on the machine contributes nothing, so a C
    /// build on an image without a C++ compiler still gets its caching.
    fn apply_host(&self, environment: &mut BTreeMap<String, String>) {
        for (installed, host, real) in [
            (&self.cc, "HOST_CC", REAL_CC_ENV),
            (&self.cxx, "HOST_CXX", REAL_CXX_ENV),
        ] {
            if let Some((shim, compiler)) = installed {
                environment.insert(host.into(), shim.to_string_lossy().into_owned());
                environment.insert(real.into(), compiler.to_string_lossy().into_owned());
            }
        }
    }

    /// Point each variable that named a cross compiler at its own shim.
    fn apply_targeted(&self, environment: &mut BTreeMap<String, String>) {
        for targeted in &self.targeted {
            environment.insert(
                targeted.variable.clone(),
                targeted.shim.to_string_lossy().into_owned(),
            );
        }
    }

    /// Pins for the shims that stand in for a named cross compiler.
    ///
    /// They share the map the standalone shims use, which is what lets one
    /// shim binary serve several compilers: it looks itself up by the name it
    /// was invoked under.
    fn pins(&self) -> BTreeMap<String, PathBuf> {
        self.targeted
            .iter()
            .map(|targeted| (targeted.shim_name.clone(), targeted.real.clone()))
            .collect()
    }
}

/// Whether an environment variable is how the `cc` crate names a compiler for
/// a particular target.
///
/// `TARGET_CC` and `TARGET_CXX` apply to whatever the build is cross-compiling
/// for; `CC_<target>` and `CXX_<target>` name one triple outright.
fn targeted_compiler_language(variable: &str) -> Option<CcLanguage> {
    match variable {
        "TARGET_CC" => return Some(CcLanguage::C),
        "TARGET_CXX" => return Some(CcLanguage::Cxx),
        _ => {}
    }
    // `CXX_` first: `CC_` is not a prefix of it, but reading it the other way
    // round invites the mistake.
    for (prefix, language) in [("CXX_", CcLanguage::Cxx), ("CC_", CcLanguage::C)] {
        if let Some(target) = variable.strip_prefix(prefix)
            && is_target_triple(target)
        {
            return Some(language);
        }
    }
    None
}

/// Whether a variable suffix spells a target triple rather than something else
/// that happens to start with `CC_`.
///
/// The `cc` crate hangs its own controls off that prefix -- `CC_FORCE_DISABLE`,
/// `CC_KNOWN_WRAPPER_CUSTOM`, `CC_ENABLE_DEBUG_OUTPUT` -- and autotools adds
/// `CC_FOR_BUILD`. Redirecting one of those would not miss a cache hit, it
/// would answer a question the build asked with a compiler path.
///
/// Case is what separates them: a target triple is lowercase and those knobs
/// are not. A triple also always has at least two components, which a bare word
/// does not.
fn is_target_triple(suffix: &str) -> bool {
    !suffix.is_empty()
        && suffix.contains(['-', '_'])
        && suffix.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

/// Variables the `cc` crate consults before falling back to the platform
/// default. A build that sets any of them has chosen its own compiler, and mbx
/// stands aside rather than redirecting it.
const CC_CRATE_ENV: &[&str] = &["CC", "CXX", "HOST_CC", "HOST_CXX"];

/// Install the C and C++ shims, resolving the compilers they will run.
///
/// Resolution happens here rather than in the shim so the whole build agrees on
/// one compiler, and so a machine with no C compiler simply gets no shims
/// instead of a build script that fails differently than it would have.
fn install_cc_shims(shims_dir: &Path) -> Result<Option<CcShims>> {
    if !cfg!(unix) {
        return Ok(None);
    }
    let executable = std::env::current_exe().wrap_err("failed to locate the running mbx binary")?;
    // Each language stands alone. An image with a C compiler and no C++ one is
    // ordinary, and it must not cost a C-only sys-crate its caching.
    let real_cc = resolve_on_path(CcLanguage::C.default_driver());
    let real_cxx = resolve_on_path(CcLanguage::Cxx.default_driver());
    // Wrapped first, because a cross image is entitled to ship the driver it
    // cross-compiles with and no host `cc` at all. Deciding there is nothing to
    // do before looking would leave exactly that build uncached.
    std::fs::create_dir_all(shims_dir)?;
    let targeted = wrap_targeted_compilers(&executable, shims_dir)?;
    if real_cc.is_none() && real_cxx.is_none() && targeted.is_empty() {
        debug!("no C or C++ compiler was found on PATH; build script compiles are not cached");
        return Ok(None);
    }
    let shim = |language: CcLanguage| -> Result<PathBuf> {
        let destination = shims_dir.join(shim_file_name(language.shim_stem()));
        link_path_shim(&executable, &destination)?;
        Ok(destination)
    };
    let mut installed = CcShims {
        cc: None,
        cxx: None,
        targeted,
    };
    if let Some(real) = real_cc {
        installed.cc = Some((shim(CcLanguage::C)?, real));
    }
    if let Some(real) = real_cxx {
        installed.cxx = Some((shim(CcLanguage::Cxx)?, real));
    }
    Ok(Some(installed))
}

/// Put a shim in front of every cross compiler the build named for itself.
///
/// Deriving one is not an option: which compiler a target implies lives in the
/// `cc` crate's own tables, and guessing wrong would not cost a cache hit, it
/// would build the object with the wrong compiler. So only a compiler the build
/// asked for by name is wrapped, and only when the name resolves to a single
/// executable -- a value like `ccache gcc` is a command, not a path, and is
/// left alone.
fn wrap_targeted_compilers(executable: &Path, shims_dir: &Path) -> Result<Vec<TargetedCompiler>> {
    let mut wrapped = Vec::new();
    for (variable, value) in std::env::vars() {
        let Some(language) = targeted_compiler_language(&variable) else {
            continue;
        };
        let Some(real) = resolve_named_compiler(&value, executable, shims_dir) else {
            debug!("{variable} does not name a single executable; it is left as it is");
            continue;
        };
        // Named for the variable so one shim binary can serve several
        // compilers, telling them apart by the name it was invoked under.
        let shim_name = format!(
            "{}-{}",
            language.shim_stem(),
            variable.to_ascii_lowercase().replace(['.', '/'], "_")
        );
        let shim = shims_dir.join(shim_file_name(&shim_name));
        link_path_shim(executable, &shim)?;
        wrapped.push(TargetedCompiler {
            variable,
            shim_name,
            shim,
            real,
        });
    }
    wrapped.sort_by(|left, right| left.variable.cmp(&right.variable));
    Ok(wrapped)
}

/// Resolve a compiler a build named, rejecting anything that is not one path.
fn resolve_named_compiler(value: &str, executable: &Path, shims: &Path) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() || value.split_whitespace().count() != 1 {
        return None;
    }
    let candidate = Path::new(value);
    let resolved = if candidate.is_absolute() {
        candidate.is_file().then(|| candidate.to_path_buf())
    } else {
        resolve_on_path_excluding(value, executable, shims)
    }?;
    // A build already pointed at a shim -- this session's, or one an outer
    // session left in the environment -- would otherwise be wrapped a second
    // time, and the inner shim would exec itself. Comparing the directory as
    // well as the binary catches the link that does not compare equal.
    let inside_shims = resolved
        .parent()
        .is_some_and(|parent| canonical(parent) == canonical(shims));
    (!inside_shims && !is_same_binary(&resolved, Some(executable))).then_some(resolved)
}

fn resolve_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// A directory of compiler-named shims for builds that find their compiler on
/// `PATH`, and the compilers those shims stand in for.
pub struct PathShims {
    pub directory: PathBuf,
    pub compilers: BTreeMap<String, PathBuf>,
}

/// Install a shim under every plain compiler name that resolves on `PATH`.
///
/// Resolution happens here rather than in the shim so the whole build agrees
/// on one compiler per name, and skips the running binary so a shim directory
/// already on `PATH` cannot become its own compiler. A machine with none of
/// the names simply gets no shim directory.
///
/// `directory` outlives the session on purpose. A configure step records the
/// compiler it found by absolute path -- CMake writes it into `CMakeCache.txt`,
/// autoconf into the generated makefiles -- and a session-local directory would
/// leave that build permanently naming a path that no longer exists. A stable
/// one keeps the recorded path resolvable, and keeps a later `cmake --build`
/// running through the cache rather than around it. Nothing is added to any
/// `PATH` but the one handed to a single `mbx exec` command.
pub fn install_path_shims(directory: &Path) -> Result<Option<PathShims>> {
    if !cfg!(unix) {
        return Ok(None);
    }
    let executable = std::env::current_exe().wrap_err("failed to locate the running mbx binary")?;
    std::fs::create_dir_all(directory)?;
    let mut compilers = BTreeMap::new();
    for (name, _) in PATH_SHIM_NAMES {
        let destination = directory.join(name);
        let Some(real) = resolve_on_path_excluding(name, &executable, directory) else {
            continue;
        };
        // Belt and braces for the recursion the exclusion above prevents: a
        // shim that stood in for itself would exec itself forever, so leave
        // the name uncached rather than install that.
        if canonical(&real) == canonical(&destination) {
            debug!("{name} resolved to its own shim; leaving it uncached");
            continue;
        }
        link_path_shim(&executable, &destination)?;
        compilers.insert((*name).to_string(), real);
    }
    if compilers.is_empty() {
        debug!("no C or C++ compilers were found on PATH; nothing to shim");
        return Ok(None);
    }
    Ok(Some(PathShims {
        directory: directory.to_path_buf(),
        compilers,
    }))
}

/// Point one compiler name at this binary, replacing a stale link in place.
///
/// A symlink rather than a hard link so an upgraded mbx is picked up without
/// reinstalling, and because on macOS a hard link taken while the binary is
/// replaced can be killed at exec.
///
/// The replacement goes through a temporary name and a rename because this
/// directory is shared: another build may be executing the very name being
/// replaced, and `rename` leaves it resolvable at every instant where removing
/// and recreating would not.
#[cfg(unix)]
fn link_path_shim(executable: &Path, destination: &Path) -> Result<()> {
    // Absolutized for the same reason [`symlink_shim`] does it: a symlink is
    // resolved from the directory holding it, which here is the cache's shim
    // directory and never the caller's. `current_exe` has been absolute on
    // every platform mbx runs on, but its contract does not promise one, and a
    // relative target would point inside the shim directory itself.
    let target = std::path::absolute(executable)?;
    if std::fs::read_link(destination).is_ok_and(|existing| existing == target) {
        return Ok(());
    }
    let staging = destination.with_file_name(format!(
        ".{}.{}.{}",
        destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        std::process::id(),
        SHIM_STAGING_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&staging);
    std::os::unix::fs::symlink(&target, &staging)?;
    std::fs::rename(&staging, destination)
        .wrap_err_with(|| format!("failed to install the shim {}", destination.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn link_path_shim(_executable: &Path, _destination: &Path) -> Result<()> {
    eyre::bail!("PATH shims are not supported on this platform")
}

/// Find the real compiler `name` refers to, never a shim.
///
/// The shim directory is skipped by location, not by identity. Identity alone
/// is not enough: a nested `mbx exec` sees that directory first on `PATH`, and
/// a shim there may point at a *different* mbx binary -- an upgrade, or a build
/// from another checkout -- which no inode comparison against the running one
/// can recognize. Choosing it would pin a shim as the compiler and then relink
/// it to the running binary, leaving it its own compiler and recursing until
/// the process table fills. Nothing mbx puts in that directory is ever a real
/// compiler, so the directory itself is the honest thing to exclude.
///
/// The identity check stays for the rest of `PATH`, where a copy of mbx under a
/// compiler's name can still turn up outside any directory mbx owns.
fn resolve_on_path_excluding(name: &str, this_binary: &Path, shims: &Path) -> Option<PathBuf> {
    resolve_in_path(&std::env::var_os("PATH")?, name, this_binary, shims)
}

/// The search itself, over a `PATH` the caller supplies.
///
/// Separated from the environment so a test can hand it one: `PATH` is process
/// global, and cargo runs these tests on a thread pool, so setting it here
/// would race every other test that reads one -- `which` in the linker tests
/// among them.
fn resolve_in_path(path: &OsStr, name: &str, this_binary: &Path, shims: &Path) -> Option<PathBuf> {
    let shims = canonical(shims);
    std::env::split_paths(path)
        .filter(|directory| canonical(directory) != shims)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file() && !is_same_binary(candidate, Some(this_binary)))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn install_session_shim(session_dir: &Path) -> Result<PathBuf> {
    let executable = std::env::current_exe().wrap_err("failed to locate the running mbx binary")?;
    install_shim(&executable, session_dir, ShimLink::Tracking)
}

/// How an installed shim refers to the mbx binary behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShimLink {
    /// A symlink: the shim is whatever binary that path holds when it runs.
    Tracking,
    /// A hard link, or a copy where the filesystem cannot link: the bytes that
    /// were there on the day the shim was installed.
    Pinned,
}

/// File name a shim with this stem is installed under.
pub fn shim_file_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.into()
    }
}

/// Install a rustc shim into `directory` as a link to `executable`.
///
/// The shim must be the same binary as the agent -- the handshake requires an
/// exact version match -- so it is a link to the running binary rather than an
/// independently built copy.
///
/// Which kind of link matters more than it looks. On macOS, exec of a path that
/// `link(2)` created moments ago is intermittently killed outright: SIGKILL, no
/// output, no crash report, nothing in the system log. Measured on macOS 26.6
/// (Apple silicon) at about one exec in eight hundred under heavy parallel
/// load, against a binary nothing was writing, and far more readily for a large
/// image -- the 50 MB debug build of mbx died where a 500 KB one never did,
/// which is the shape a race in per-page signature validation would have. The
/// window closes about half a second after the link appears, and the same path
/// then runs fine forever. Exec of the original path never fails, and neither
/// does exec of a symlink to it -- the kernel resolves that to a file whose
/// signature it validated long ago. Reading the new link through first,
/// fsyncing it and its directory, and taking the first exec ourselves to spend
/// the race were all tried, and none of them helped.
///
/// So [`ShimLink::Tracking`] is for the session shim, which cargo execs a few
/// milliseconds after it is installed to ask `rustc -vV` what compiler it has.
/// [`ShimLink::Pinned`] is for the plain-cargo wrapper `mbx setup` installs,
/// where nothing execs the shim for as long as it takes to type another command
/// and a hard link keeps working even if the binary it was made from is deleted.
///
/// The one thing tracking gives up: replace the mbx binary underneath a running
/// build and the session shim follows it, so cargo either execs nothing (the
/// path is gone, and the build stops with a plain error) or execs a version the
/// agent will not shake hands with, which bypasses the cache for the rest of the
/// build. Both are loud and self-inflicted, unlike the kill they replace.
pub fn install_shim(executable: &Path, directory: &Path, link: ShimLink) -> Result<PathBuf> {
    install_shim_named(executable, directory, RUSTC_SHIM_STEM, link)
}

/// Install a shim for one compiler into `directory` as a link to `executable`.
///
/// Every shim is the same binary under a different name; the name is what tells
/// it which compiler it stands in for. Everything [`install_shim`] documents
/// about which kind of link is used applies here too.
pub fn install_shim_named(
    executable: &Path,
    directory: &Path,
    stem: &str,
    link: ShimLink,
) -> Result<PathBuf> {
    let shim = directory.join(shim_file_name(stem));
    let _ = std::fs::remove_file(&shim);
    if link == ShimLink::Tracking && symlink_shim(executable, &shim) {
        return Ok(shim);
    }
    if let Err(link_error) = std::fs::hard_link(executable, &shim) {
        std::fs::copy(executable, &shim).wrap_err_with(|| {
            format!("failed to install the {stem} shim by hard link ({link_error}) or copy")
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        // Some filesystems do not retain executable permissions when the
        // cross-device fallback copies the running binary. Cargo must be able
        // to invoke the installed wrapper directly.
        let mut permissions = std::fs::metadata(&shim)?.permissions();
        permissions.set_mode(permissions.mode() | 0o100);
        std::fs::set_permissions(&shim, permissions)?;
    }
    Ok(shim)
}

/// Point `shim` at `executable` by symlink, reporting whether that worked.
///
/// A failure is not an error: the hard link the caller falls back to is what
/// every shim was before, so the only thing lost is the race described on
/// [`install_shim`].
///
/// The target is absolutized first, because the two kinds of link do not read a
/// relative one the same way: `link(2)` and `copy` resolve it from the caller's
/// working directory, while a symlink resolves it from the shim's own directory
/// -- a temporary one that shares nothing with the caller's. Resolving it here
/// gives the argument one meaning. Absolutizing rather than declining, because a
/// relative target must still get a symlink: `current_exe()` has been absolute
/// on every platform mbx runs on, but nothing in its contract promises that, and
/// a shim that quietly stopped tracking would put the kill back.
///
/// A target that is not there gets no symlink at all. A hard link and a copy
/// both refuse one, and a symlink would instead name it and leave cargo to
/// discover the dangling wrapper mid-build.
#[cfg(unix)]
fn symlink_shim(executable: &Path, shim: &Path) -> bool {
    let Ok(target) = std::path::absolute(executable) else {
        return false;
    };
    target.exists() && std::os::unix::fs::symlink(&target, shim).is_ok()
}

/// Windows has no shim symlinks: creating one needs a privilege ordinary
/// accounts lack, and the code-signature race they exist to avoid is a macOS
/// kernel behaviour with no Windows counterpart.
#[cfg(windows)]
fn symlink_shim(_executable: &Path, _shim: &Path) -> bool {
    false
}

#[cfg(unix)]
async fn spawn_server(
    session_dir: &Path,
    agent: CacheAgent,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(String, JoinHandle<Result<()>>)> {
    use std::os::unix::fs::PermissionsExt as _;

    let socket = session_dir.join("cache-agent.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let endpoint = socket.to_string_lossy().into_owned();
    let server = tokio::spawn(async move {
        let _cleanup = SocketCleanup(socket);
        let mut connections = tokio::task::JoinSet::new();
        let outcome = loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = match accepted {
                        Ok(accepted) => accepted,
                        Err(error) => break Err(eyre::Report::from(error)),
                    };
                    let agent = agent.clone();
                    connections.spawn(async move {
                        if let Err(error) = agent.handle_connection(stream).await {
                            debug!("cache agent connection failed: {error}");
                        }
                    });
                    // A build makes one connection per compilation, so finished
                    // ones are reaped as they go rather than held until shutdown.
                    while connections.try_join_next().is_some() {}
                }
                _ = &mut shutdown => break Ok(()),
            }
        };
        // A connection answers requests, and a store request queues an upload,
        // so the session has not stopped accepting work until these are done.
        while connections.join_next().await.is_some() {}
        outcome
    });
    Ok((endpoint, server))
}

#[cfg(unix)]
struct SocketCleanup(PathBuf);

#[cfg(unix)]
impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(windows)]
async fn spawn_server(
    _session_dir: &Path,
    agent: CacheAgent,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(String, JoinHandle<Result<()>>)> {
    let endpoint = format!(
        r"\\.\pipe\mbx-cache-{}-{}",
        std::process::id(),
        crate::util::random_string(12)
    );
    let first_server = create_named_pipe(&endpoint, true)?;
    let server_endpoint = endpoint.clone();
    let server = tokio::spawn(async move {
        let mut next_server = Some(first_server);
        let mut connections = tokio::task::JoinSet::new();
        let outcome = loop {
            let pipe = next_server
                .take()
                .expect("the next named-pipe server is always prepared");
            tokio::select! {
                connected = pipe.connect() => {
                    if let Err(error) = connected {
                        break Err(eyre::Report::from(error));
                    }
                    match create_named_pipe(&server_endpoint, false) {
                        Ok(prepared) => next_server = Some(prepared),
                        Err(error) => break Err(error),
                    }
                    let agent = agent.clone();
                    connections.spawn(async move {
                        if let Err(error) = agent.handle_connection(pipe).await {
                            debug!("cache agent connection failed: {error}");
                        }
                    });
                    // A build makes one connection per compilation, so finished
                    // ones are reaped as they go rather than held until shutdown.
                    while connections.try_join_next().is_some() {}
                }
                _ = &mut shutdown => break Ok(()),
            }
        };
        // A connection answers requests, and a store request queues an upload,
        // so the session has not stopped accepting work until these are done.
        while connections.join_next().await.is_some() {}
        outcome
    });
    Ok((endpoint, server))
}

#[cfg(windows)]
fn create_named_pipe(
    endpoint: &str,
    first_pipe_instance: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use std::mem::size_of;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

    let security = CurrentUserSecurityDescriptor::new()?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0,
        bInheritHandle: 0,
    };
    let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
    options.first_pipe_instance(first_pipe_instance);
    // SAFETY: `attributes` and its owned security descriptor remain valid for
    // the duration of CreateNamedPipeW, and the handle is not inheritable.
    unsafe {
        options
            .create_with_security_attributes_raw(endpoint, (&raw mut attributes).cast())
            .wrap_err("failed to create the current-user-only cache pipe")
    }
}

#[cfg(windows)]
struct CurrentUserSecurityDescriptor(windows_sys::Win32::Security::PSECURITY_DESCRIPTOR);

#[cfg(windows)]
impl CurrentUserSecurityDescriptor {
    fn new() -> Result<Self> {
        use std::mem::size_of;
        use std::ptr::null_mut;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::Security::Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::{
            GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        let mut token: HANDLE = null_mut();
        // SAFETY: `token` is a valid out pointer and the process pseudo-handle
        // is always valid in the calling process.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(std::io::Error::last_os_error())
                .wrap_err("failed to open the current process token");
        }
        let token = OwnedWindowsHandle(token);

        let mut required = 0;
        // The first call intentionally obtains the required buffer size.
        // SAFETY: a null information pointer is required for this size query.
        unsafe {
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &raw mut required);
        }
        if required == 0 {
            return Err(std::io::Error::last_os_error())
                .wrap_err("failed to size the current process user token");
        }
        let word_count = (required as usize).div_ceil(size_of::<usize>());
        let mut token_information = vec![0usize; word_count];
        // SAFETY: the aligned buffer is at least `required` bytes long and the
        // API initializes it as TOKEN_USER on success.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                token_information.as_mut_ptr().cast(),
                required,
                &raw mut required,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .wrap_err("failed to read the current process user token");
        }
        // SAFETY: GetTokenInformation successfully initialized the aligned
        // buffer as TOKEN_USER and its SID remains owned by that buffer.
        let user_sid = unsafe {
            (*(token_information.as_ptr().cast::<TOKEN_USER>()))
                .User
                .Sid
        };
        let mut sid_string = null_mut();
        // SAFETY: `user_sid` points into the live token-information buffer and
        // `sid_string` is a valid out pointer.
        if unsafe { ConvertSidToStringSidW(user_sid, &raw mut sid_string) } == 0 {
            return Err(std::io::Error::last_os_error())
                .wrap_err("failed to format the current process user SID");
        }
        let sid_string = LocalWindowsAllocation(sid_string.cast());
        // SAFETY: ConvertSidToStringSidW returned a valid NUL-terminated string
        // whose allocation remains live through `sid_string`.
        let sid = unsafe { nul_terminated_wide(sid_string.0.cast()) };

        let mut sddl: Vec<u16> = "D:P(A;;GA;;;".encode_utf16().collect();
        sddl.extend_from_slice(sid);
        sddl.extend([')' as u16, 0]);
        let mut descriptor = null_mut();
        // SAFETY: the SDDL is NUL-terminated, `descriptor` is a valid out
        // pointer, and the returned allocation is owned by LocalFree.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .wrap_err("failed to restrict the cache pipe to the current user");
        }
        drop(token);
        drop(sid_string);
        debug_assert!(!descriptor.is_null());
        Ok(Self(descriptor))
    }
}

#[cfg(windows)]
impl Drop for CurrentUserSecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: this allocation came from
        // ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0.cast());
        }
    }
}

#[cfg(windows)]
struct OwnedWindowsHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedWindowsHandle {
    fn drop(&mut self) {
        // SAFETY: this handle came from OpenProcessToken and is owned here.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
struct LocalWindowsAllocation(windows_sys::Win32::Foundation::HLOCAL);

#[cfg(windows)]
impl Drop for LocalWindowsAllocation {
    fn drop(&mut self) {
        // SAFETY: this allocation came from a Win32 API documented to use
        // LocalAlloc and has not previously been freed.
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0);
        }
    }
}

#[cfg(windows)]
unsafe fn nul_terminated_wide<'a>(value: *const u16) -> &'a [u16] {
    let mut length = 0;
    // SAFETY: the caller guarantees that `value` points to a valid
    // NUL-terminated UTF-16 string.
    while unsafe { *value.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: the scan above established the initialized string length.
    unsafe { std::slice::from_raw_parts(value, length) }
}

/// Whether this process was invoked as the rustc shim.
pub fn is_rustc_shim() -> bool {
    std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_stem)
        .is_some_and(|stem| stem == OsStr::new(RUSTC_SHIM_STEM))
}

/// Compiler names the standalone shim directory stands in for, and the
/// language each name selects.
///
/// Only the plain platform drivers: a versioned name such as `gcc-13` was
/// chosen deliberately by the build, and a compiler chosen that specifically
/// is one this table should not silently intercept.
const PATH_SHIM_NAMES: &[(&str, CcLanguage)] = &[
    ("cc", CcLanguage::C),
    ("gcc", CcLanguage::C),
    ("clang", CcLanguage::C),
    ("c++", CcLanguage::Cxx),
    ("g++", CcLanguage::Cxx),
    ("clang++", CcLanguage::Cxx),
];

/// The language a standalone shim name selects, if the name is one.
fn path_shim_language(name: &str) -> Option<CcLanguage> {
    PATH_SHIM_NAMES
        .iter()
        .find(|(shim, _)| *shim == name)
        .map(|(_, language)| *language)
}

/// Which compiler this process was invoked as a shim for, if any.
pub fn is_cc_shim() -> Option<CcLanguage> {
    let stem = std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_stem)?
        .to_str()?
        .to_string();
    match stem.as_str() {
        CC_SHIM_STEM => Some(CcLanguage::C),
        CXX_SHIM_STEM => Some(CcLanguage::Cxx),
        // `mbx-cxx-...` is tested first: `mbx-cc-` is not a prefix of it, but
        // reading it the other way round invites the mistake.
        other if other.starts_with(&format!("{CXX_SHIM_STEM}-")) => Some(CcLanguage::Cxx),
        other if other.starts_with(&format!("{CC_SHIM_STEM}-")) => Some(CcLanguage::C),
        other => path_shim_language(other),
    }
}

/// The file name this process was invoked under.
fn shim_invocation_name() -> Option<String> {
    std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_name)?
        .to_str()
        .map(ToOwned::to_owned)
}

/// Ultra-early argv0 path used by the `CC` and `CXX` build scripts inherit.
///
/// Unlike `RUSTC_WRAPPER`, there is no convention that hands the shim the real
/// compiler: the build script calls `$CC` and every argument is the
/// compilation's own. The compiler to run therefore arrives out of band, and a
/// shim that cannot find one falls back to the platform default rather than
/// failing a build it was only meant to observe.
pub fn run_cc_shim(language: CcLanguage) -> ExitCode {
    // From here on, stderr carries only what the real compiler would have
    // written: build scripts probe compilers by running them and reading that
    // stream, and a diagnostic mixed into it changes what they decide.
    reserve_stderr_for_compiler();
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let compiler = match real_compiler(language) {
        Ok(compiler) => compiler,
        Err(error) => {
            eprintln!(
                "mbx[error]: the {} shim found no compiler to run: {error:#}",
                language.shim_stem()
            );
            return ExitCode::from(1);
        }
    };
    // A shim reached outside a session has no agent to ask, and this is the
    // ordinary way a persisted compiler path is invoked: a build configured
    // under `mbx exec` and then built without it. Stand aside before probing
    // anything, so that costs one exec rather than a compiler query first.
    if session_socket().is_none() {
        return run_transparent_cc(compiler, arguments);
    }
    match crate::cc::compile(&compiler, &arguments, language) {
        Ok(exit_code) => return exit_code,
        Err(error) => {
            record_cc_bypass(&error);
            #[cfg(debug_assertions)]
            report_shim_warning(&format!("cc cache bypassed: {error:#}"));
        }
    }
    run_transparent_cc(compiler, arguments)
}

/// The compiler a cc shim stands in for.
///
/// The session pins this when it installs the shims: `mbx exec` pins every
/// name it placed on `PATH`, and a cargo session pins the pair build scripts
/// inherit. A shim invoked with no pin searches `PATH` for the name it was
/// invoked under, skipping any candidate that is this binary, so an inherited
/// `CC` or a stale shim directory cannot make the shim call itself.
fn real_compiler(language: CcLanguage) -> Result<OsString> {
    let invoked = shim_invocation_name();
    if let Some(name) = invoked.as_deref()
        && let Some(compiler) = pinned_path_shim(name)
    {
        return Ok(compiler.into_os_string());
    }
    let pinned = match language {
        CcLanguage::C => REAL_CC_ENV,
        CcLanguage::Cxx => REAL_CXX_ENV,
    };
    if let Some(compiler) = std::env::var_os(pinned).filter(|value| !value.is_empty()) {
        return Ok(compiler);
    }
    // A shim named after a real driver stands in for exactly that driver; the
    // session shim names have no driver of their own and fall back to the
    // platform default.
    let name = invoked
        .as_deref()
        .filter(|name| path_shim_language(name).is_some())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| language.default_driver().to_string());
    let current = std::env::current_exe().ok();
    let path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(&name);
        if !candidate.is_file() || is_same_binary(&candidate, current.as_deref()) {
            continue;
        }
        return Ok(candidate.into_os_string());
    }
    eyre::bail!("no {name} was found on PATH")
}

/// The compiler an exec session pinned for this shim name, if any.
fn pinned_path_shim(name: &str) -> Option<PathBuf> {
    let pins = std::env::var(PATH_SHIMS_ENV).ok()?;
    let pins: BTreeMap<String, PathBuf> = serde_json::from_str(&pins).ok()?;
    pins.get(name).cloned()
}

/// Whether `candidate` is the running mbx binary under another name.
///
/// Shims are hard links, so a path comparison alone cannot recognize one that
/// lives in a different directory -- a stale shim directory an outer session
/// left on `PATH`, say. Device and inode identify the file itself.
fn is_same_binary(candidate: &Path, current: Option<&Path>) -> bool {
    let Some(current) = current else {
        return false;
    };
    let resolved = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    let current_resolved = std::fs::canonicalize(current)
        .ok()
        .unwrap_or_else(|| current.to_path_buf());
    if resolved == current_resolved {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if let (Ok(a), Ok(b)) = (std::fs::metadata(&resolved), std::fs::metadata(current)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    false
}

fn run_transparent_cc(compiler: OsString, arguments: Vec<OsString>) -> ExitCode {
    let mut command = Command::new(&compiler);
    command.args(&arguments);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;

        let error = command.exec();
        eprintln!("mbx[error]: the cc shim failed to execute {compiler:?}: {error}");
        ExitCode::from(1)
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => ExitCode::from(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1)),
            Err(error) => {
                eprintln!("mbx[error]: the cc shim failed to execute {compiler:?}: {error}");
                ExitCode::from(1)
            }
        }
    }
}

/// Ultra-early argv0 path used by cargo's `RUSTC_WRAPPER` integration.
///
/// Cargo invokes this thousands of times per build, so it runs before any
/// runtime, logging, or configuration is set up. Cacheable invocations restore
/// from or publish through the cache; anything else is a transparent compiler
/// call.
pub fn run_rustc_shim() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let Some(rustc) = arguments.next() else {
        eprintln!("mbx[error]: the rustc shim expected the rustc executable as its first argument");
        return ExitCode::from(1);
    };
    let arguments = arguments.collect::<Vec<_>>();
    if std::env::var_os(PREVIOUS_RUSTC_WRAPPER_ENV).is_none() {
        match crate::rustc::compile(&rustc, &arguments) {
            Ok(exit_code) => return exit_code,
            Err(error) => {
                record_bypass(&error);
                #[cfg(debug_assertions)]
                eprintln!("mbx[warning]: rustc cache bypassed: {error:#}");
            }
        }
    }

    run_transparent_rustc(rustc, arguments)
}

fn run_transparent_rustc(rustc: OsString, arguments: Vec<OsString>) -> ExitCode {
    let crate_name = crate_name_argument(&arguments);
    // A bypassed compilation is still a real compiler process the machine has
    // to pay for. Probe invocations pass through unscheduled: cargo runs them
    // to learn about the compiler before it plans anything, so making one wait
    // for a permit would stall a build's startup behind its siblings' permits.
    // Most probes carry no --crate-name; the target-info queries carry the
    // placeholder name `___` alongside `--print`, and compile nothing.
    let is_query = arguments.iter().any(|argument| {
        argument == "-"
            || argument
                .to_str()
                .is_some_and(|argument| argument.starts_with("--print"))
    });
    let demand = crate_name
        .as_deref()
        .filter(|_| !is_query)
        .map(|name| crate::scheduler::Demand::new(name, links_natively(&arguments)));
    let permit = demand
        .as_ref()
        .and_then(|demand| crate::scheduler::pool().and_then(|pool| pool.admit(demand)));
    // Started after admission, matching the cached paths: waiting for the
    // machine is not time this compilation cost, and reporting it as compiler
    // time would make the jobs that wait longest -- bypassed links -- look
    // like the slowest crates in the build.
    let started = Instant::now();
    let mut command = if let Some(wrapper) = std::env::var_os(PREVIOUS_RUSTC_WRAPPER_ENV) {
        let mut command = Command::new(wrapper);
        command.arg(&rustc);
        command
    } else {
        Command::new(&rustc)
    };
    command.args(&arguments);
    command.env_remove(PREVIOUS_RUSTC_WRAPPER_ENV);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;

        if permit.is_none() {
            let error = command.exec();
            eprintln!("mbx[error]: the rustc shim failed to execute rustc: {error}");
            return ExitCode::from(1);
        }
        // A held permit must be released when the compiler finishes, and its
        // lease lock is close-on-exec, so this process has to outlive the
        // compiler rather than become it.
        match command.status() {
            Ok(status) => {
                drop(permit);
                if let Some(demand) = &demand {
                    crate::scheduler::record_compiler_memory(demand, &status);
                }
                record_compiler_invocation(
                    "bypass",
                    crate_name.as_deref(),
                    duration_ns(started.elapsed()),
                );
                crate::materialize::exit_code(status)
            }
            Err(error) => {
                eprintln!("mbx[error]: the rustc shim failed to execute rustc: {error}");
                ExitCode::from(1)
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;

        match command.spawn().and_then(|mut child| {
            let status = child.wait()?;
            let mut exit_code = 1;
            // SAFETY: the child owns a valid process handle until it is
            // dropped, and `exit_code` is a valid out pointer.
            if unsafe { GetExitCodeProcess(child.as_raw_handle().cast(), &raw mut exit_code) } == 0
            {
                Err(std::io::Error::last_os_error())
            } else {
                Ok((exit_code, status))
            }
        }) {
            Ok((exit_code, status)) => {
                drop(permit);
                if let Some(demand) = &demand {
                    crate::scheduler::record_compiler_memory(demand, &status);
                }
                record_compiler_invocation(
                    "bypass",
                    crate_name.as_deref(),
                    duration_ns(started.elapsed()),
                );
                // SAFETY: This process is only a transparent compiler wrapper.
                // ExitProcess is required to preserve Windows exception codes,
                // which cannot be represented by stable Rust's ExitCode API.
                unsafe { windows_sys::Win32::System::Threading::ExitProcess(exit_code) }
            }
            Err(error) => {
                eprintln!("mbx[error]: the rustc shim failed to execute rustc: {error}");
                ExitCode::from(1)
            }
        }
    }
}

/// Whether a bypassed invocation will run a linker.
///
/// Read off the raw arguments because there is no parsed invocation here:
/// this is the path a compilation takes when the cache could not model it,
/// and a link mbx cannot describe exactly -- an unidentifiable linker, a
/// native library, a flag that would embed this checkout -- still lands here.
/// They are also the compilations that run a machine out of memory, so the
/// scheduler has to recognize one without the parser's help.
///
/// A test harness links a program whatever its crate type says, which is why
/// `--test` counts on its own -- but only once the emit says a linker runs at
/// all. `cargo check` and `clippy --all-targets` compile those very same
/// binary and test targets with `--emit=metadata`, and charging a check the
/// link weight would make the cheapest stage the heaviest thing queued.
fn links_natively(arguments: &[OsString]) -> bool {
    let mut produces_program = false;
    // No `--emit` at all is rustc's default, which is `link`. Cargo always
    // passes one; something driving rustc by hand may not.
    let mut emits_link = true;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        if argument == "--test" {
            produces_program = true;
        }
        let kinds = if argument == "--crate-type" {
            arguments.next().and_then(|kinds| kinds.to_str())
        } else {
            argument.strip_prefix("--crate-type=")
        };
        // rustc accepts them comma-separated, and any one of them links.
        if kinds.is_some_and(|kinds| {
            kinds.split(',').any(|kind| {
                matches!(
                    kind,
                    "bin" | "cdylib" | "dylib" | "proc-macro" | "staticlib"
                )
            })
        }) {
            produces_program = true;
        }
        let emits = if argument == "--emit" {
            arguments.next().and_then(|emits| emits.to_str())
        } else {
            argument.strip_prefix("--emit=")
        };
        if let Some(emits) = emits {
            // Each kind may carry a path of its own, as `link=/some/where`.
            emits_link = emits
                .split(',')
                .any(|emit| emit.split('=').next() == Some("link"));
        }
    }
    produces_program && emits_link
}

fn crate_name_argument(arguments: &[OsString]) -> Option<String> {
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--crate-name" {
            return arguments
                .next()
                .and_then(|name| name.to_str())
                .map(str::to_string);
        }
        if let Some(argument) = argument.to_str()
            && let Some(name) = argument.strip_prefix("--crate-name=")
        {
            return Some(name.to_string());
        }
    }
    None
}

/// Whether the shim should verify cached results against a real compilation.
///
/// An empty value or `0` is off, matching how the configuration reads it, so
/// that an explicit disable cannot be mistaken for an enable.
pub(crate) fn verify_requested() -> bool {
    std::env::var_os(VERIFY_ENV).is_some_and(|value| !value.is_empty() && value != "0")
}

/// The session file-digest ledger, answered by the cache agent.
///
/// Both directions are best-effort: a lookup that cannot reach the agent
/// reports misses and the caller hashes as it always did, and a record that
/// fails is dropped -- the ledger is a shortcut, never a dependency.
struct AgentFileDigestCache;

impl FileDigestCache for AgentFileDigestCache {
    fn find(&self, scope: FileDigestScope, files: &[FileIdentity]) -> Vec<Option<CacheDigest>> {
        if files.is_empty() {
            return Vec::new();
        }
        let response = request_agent(&[AgentRequest::FindFileDigests {
            scope,
            files: files.to_vec(),
        }]);
        match response.map(|responses| responses.into_iter().next()) {
            Ok(Some(AgentResponse::FileDigests { digests })) if digests.len() == files.len() => {
                digests
            }
            _ => vec![None; files.len()],
        }
    }

    fn record(&self, scope: FileDigestScope, entries: Vec<RecordedFileDigest>) {
        if entries.is_empty() {
            return;
        }
        let _ = request_agent(&[AgentRequest::RecordFileDigests { scope, entries }]);
    }
}

/// The file-digest ledger this shim may consult, or none under verification.
///
/// Verification exists to qualify the whole cached path end to end, so it
/// rehashes every input the way a first encounter would rather than trusting
/// what this same session recorded.
pub(crate) fn file_digest_cache() -> &'static dyn FileDigestCache {
    if verify_requested() {
        &NoFileDigestCache
    } else {
        &AgentFileDigestCache
    }
}

/// Record file digests in the session ledger, best-effort.
pub(crate) fn record_file_digests(scope: FileDigestScope, entries: Vec<RecordedFileDigest>) {
    if !verify_requested() {
        AgentFileDigestCache.record(scope, entries);
    }
}

/// Whether this build may cache natively linked programs.
///
/// Restricted to platforms whose linker mbx knows how to identify: a host it
/// cannot describe would otherwise key a link as though the linker did not
/// matter.
pub fn cache_links_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
}

/// Whether the shim may cache a natively linked program.
///
/// Unset means on, which is the one place this differs from verify mode: a
/// shim installed by `mbx setup` is driven by plain cargo, with no session to
/// have written the variable, and reading absence as "off" there would leave
/// the persistent wrapper on a default nobody chose. A session always states
/// the answer explicitly, so `MBX_CACHE_LINKS=0` still turns it off for both.
///
/// The platform is checked here rather than trusted from whoever set the
/// variable, for the same reason: the standalone shim has no session to have
/// applied the gate.
pub(crate) fn cache_links_requested() -> bool {
    cache_links_supported()
        && std::env::var_os(CACHE_LINKS_ENV).is_none_or(|value| !value.is_empty() && value != "0")
}

/// Whether the shim may make a compilation independent of its `OUT_DIR` so two
/// checkouts can share it. Read the same way as verify mode.
pub(crate) fn share_out_dir_requested() -> bool {
    std::env::var_os(SHARE_OUT_DIR_ENV).is_some_and(|value| !value.is_empty() && value != "0")
}

/// Whether the shim may compile a churning crate with its own incremental
/// state instead of publishing it. Read the same way as verify mode.
pub(crate) fn learned_incremental_requested() -> bool {
    std::env::var_os(LEARNED_INCREMENTAL_ENV).is_some_and(|value| !value.is_empty() && value != "0")
}

/// Tell the session that this compilation was not cacheable.
///
/// Bypasses never reach the agent otherwise, so without this they are invisible
/// outside a debug build. Reported by reason kind rather than message, since
/// several reasons carry a path or a flag.
fn record_bypass(error: &eyre::Report) {
    let kind = error
        .downcast_ref::<mbx_cache_rustc::BypassReason>()
        .map_or("other", mbx_cache_rustc::BypassReason::kind);
    append_bypass_log(kind, error);
    // A shim running outside a session has nowhere to report, which is fine.
    let _ = request_agent(&[AgentRequest::RecordBypass { kind: kind.into() }]);
}

/// Tell the session that a C or C++ compilation was not cacheable.
///
/// Kinds are prefixed so that a reason the two adapters share by name, such as
/// an unmodeled flag, does not merge into one statistic covering both.
fn record_cc_bypass(error: &eyre::Report) {
    let kind = error
        .downcast_ref::<mbx_cache_cc::CcBypassReason>()
        .map_or_else(
            || "cc-other".to_string(),
            |reason| format!("cc-{}", reason.kind()),
        );
    append_bypass_log(&kind, error);
    // A shim running outside a session has nowhere to report, which is fine.
    let _ = request_agent(&[AgentRequest::RecordBypass { kind }]);
}

/// Record a compilation the cache had no key to look up with.
pub(crate) fn record_unconsulted() {
    // A shim running outside a session has nowhere to report, which is fine.
    let _ = request_agent(&[AgentRequest::RecordUnconsulted]);
}

pub(crate) fn record_compiler_invocation(
    outcome: &str,
    crate_name: Option<&str>,
    duration_ns: u64,
) {
    let _ = request_agent(&[AgentRequest::RecordCompilerInvocation {
        outcome: outcome.into(),
        crate_name: crate_name.map(str::to_string),
        duration_ns,
    }]);
}

/// Append the full reason to `MBX_BYPASS_LOG`, when one is configured.
///
/// The aggregate counts say which kinds dominate; this says exactly which flag
/// or path caused each one. It exists because stderr cannot be relied on:
/// cargo swallows the output of its own probe invocations, so some bypasses are
/// invisible there.
fn append_bypass_log(kind: &str, error: &eyre::Report) {
    let Some(path) = std::env::var_os(BYPASS_LOG_ENV).filter(|path| !path.is_empty()) else {
        return;
    };
    // O_APPEND places each write at the end of the file, so one write per
    // record is what keeps parallel shims from splicing their lines together.
    // Records are a single short line for that reason: a write the kernel had
    // to break up could still interleave, and nothing here can prevent it.
    let line = format!("{kind}\t{error:#}\n");
    if let Err(problem) = append_line(&path, &line) {
        // Say so once. This runs per compilation and a destination that cannot
        // be written now will fail for every later record too, so warning each
        // time would bury the build in identical lines. Without this the
        // requested log is simply absent, with nothing explaining why.
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            eprintln!(
                "mbx[warning]: {BYPASS_LOG_ENV} was not written ({}): {problem}",
                Path::new(&path).display()
            );
        });
    }
}

fn append_line(path: &OsString, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

pub(crate) fn request_agent(requests: &[AgentRequest]) -> Result<Vec<AgentResponse>> {
    match session_socket() {
        Some(socket) => request_agent_at(&socket, requests),
        None => request_standalone_agent(requests),
    }
}

/// The session socket a shim should ask, if a session named one.
///
/// An empty value means the same as an absent one: there is no session to
/// reach. Everything that asks whether this process has a session has to
/// agree on that, because the alternatives are not equivalent for a cc shim
/// -- a standalone agent runs in the shim itself and writes to the stderr the
/// compiler owns, which is what [`report_shim_warning`] exists to avoid.
fn session_socket() -> Option<OsString> {
    std::env::var_os(SOCKET_ENV).filter(|socket| !socket.is_empty())
}

/// Serve a persistent Cargo wrapper directly from the local store.
///
/// There is deliberately no remote access here: the short-lived process has
/// no trusted session policy or opportunity to batch transfers. Prediction
/// manifests are still persisted after every successful record so the many
/// wrapper processes in one Cargo build, and later builds, share what they
/// learn without a daemon.
fn request_standalone_agent(requests: &[AgentRequest]) -> Result<Vec<AgentResponse>> {
    thread_local! {
        static STANDALONE_AGENT: RefCell<Option<StandaloneAgent>> = const { RefCell::new(None) };
    }

    STANDALONE_AGENT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let config = Config::load()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            *slot = Some(StandaloneAgent {
                agent: CacheAgent::new(config.store_dir(), VERSION),
                runtime,
                runs: BTreeMap::new(),
            });
        }
        let standalone = slot.as_mut().expect("standalone agent was initialized");
        let StandaloneAgent {
            agent,
            runtime,
            runs,
        } = standalone;
        runtime.block_on(async {
            let mut responses = Vec::with_capacity(requests.len());
            for request in requests.iter().cloned() {
                let (request, run_to_commit) = match request {
                    AgentRequest::FindActionPrediction { task, invocation } => {
                        let run = match runs.get(&task) {
                            Some(run) => run.clone(),
                            None => {
                                let run = agent.begin_task(&task).await?;
                                runs.insert(task.clone(), run.clone());
                                run
                            }
                        };
                        (
                            AgentRequest::FindActionPrediction {
                                task: run,
                                invocation,
                            },
                            None,
                        )
                    }
                    AgentRequest::RecordActionPrediction { task, prediction } => {
                        let run = match runs.remove(&task) {
                            Some(run) => run,
                            None => agent.begin_task(&task).await?,
                        };
                        (
                            AgentRequest::RecordActionPrediction {
                                task: run.clone(),
                                prediction,
                            },
                            Some(run),
                        )
                    }
                    request => (request, None),
                };
                let response = agent
                    .handle_requests(std::iter::once(request))
                    .await
                    .into_iter()
                    .next()
                    .expect("one request returns one response");
                let succeeded = !matches!(response, AgentResponse::Error { .. });
                responses.push(response);
                if succeeded && let Some(run) = run_to_commit {
                    agent.commit_task(&run).await?;
                }
            }
            Ok(responses)
        })
    })
}

struct StandaloneAgent {
    agent: CacheAgent,
    runtime: tokio::runtime::Runtime,
    runs: BTreeMap<String, String>,
}

#[cfg(unix)]
fn request_agent_at(socket: &OsString, requests: &[AgentRequest]) -> Result<Vec<AgentResponse>> {
    // One compilation asks the agent several separate questions, and paying a
    // connect plus a handshake round trip for each adds up across a warm
    // build. The protocol is strict request-response, so a connection held for
    // the life of this shim process serves them all.
    thread_local! {
        static CONNECTION: RefCell<Option<(OsString, std::os::unix::net::UnixStream)>> =
            const { RefCell::new(None) };
    }
    CONNECTION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_none_or(|(known, _)| known != socket) {
            let mut stream = std::os::unix::net::UnixStream::connect(Path::new(socket))
                .wrap_err("failed to connect to the cache session")?;
            sync_handshake(&mut stream)?;
            *slot = Some((socket.clone(), stream));
        }
        let (_, stream) = slot.as_mut().expect("the connection was just established");
        let responses = requests
            .iter()
            .map(|request| sync_request(stream, request))
            .collect::<Result<Vec<_>>>();
        if responses.is_err() {
            // A failed exchange leaves the stream in an unknown protocol
            // state, so the next request starts over on a fresh connection.
            *slot = None;
        }
        responses
    })
}

#[cfg(windows)]
fn request_agent_at(socket: &OsString, requests: &[AgentRequest]) -> Result<Vec<AgentResponse>> {
    let endpoint = socket.to_string_lossy().into_owned();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            let mut stream = connect_pipe(&endpoint).await?;
            let request = AgentRequest::Hello {
                protocol: AGENT_PROTOCOL_VERSION,
                client_version: VERSION.into(),
            };
            let mut encoded = serde_json::to_vec(&request)?;
            encoded.push(b'\n');
            stream.write_all(&encoded).await?;
            stream.flush().await?;
            let mut response = String::new();
            tokio::io::BufReader::new(&mut stream)
                .read_line(&mut response)
                .await?;
            validate_handshake_response(&response)?;
            let mut responses = Vec::with_capacity(requests.len());
            for request in requests {
                let mut encoded = serde_json::to_vec(request)?;
                encoded.push(b'\n');
                stream.write_all(&encoded).await?;
                stream.flush().await?;
                let mut response = String::new();
                tokio::io::BufReader::new(&mut stream)
                    .read_line(&mut response)
                    .await?;
                responses.push(serde_json::from_str(&response)?);
            }
            Ok(responses)
        })
}

/// Open the agent's pipe, waiting out the instances that are already busy.
///
/// The agent keeps one spare instance and creates the next only after accepting,
/// so the rustc processes cargo runs in parallel routinely arrive to a busy
/// pipe. Without this they would fall back to uncached compiles.
#[cfg(windows)]
async fn connect_pipe(endpoint: &str) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use std::time::Instant;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

    const RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(5);

    let deadline = Instant::now() + RETRY_DEADLINE;
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint) {
            Ok(client) => return Ok(client),
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline => {}
            Err(error) => {
                return Err(error).wrap_err("failed to connect to the cache session");
            }
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

#[cfg(unix)]
fn sync_handshake(stream: &mut (impl std::io::Read + Write)) -> Result<()> {
    let request = AgentRequest::Hello {
        protocol: AGENT_PROTOCOL_VERSION,
        client_version: VERSION.into(),
    };
    write_request_line(stream, &request)?;
    let mut response = String::new();
    BufReader::new(&mut *stream).read_line(&mut response)?;
    validate_handshake_response(&response)
}

#[cfg(unix)]
fn sync_request(
    stream: &mut (impl std::io::Read + Write),
    request: &AgentRequest,
) -> Result<AgentResponse> {
    write_request_line(stream, request)?;
    let mut response = String::new();
    BufReader::new(&mut *stream).read_line(&mut response)?;
    Ok(serde_json::from_str(&response)?)
}

/// Send one request as a single write.
///
/// Serializing straight onto the socket emits a syscall per JSON fragment,
/// and a prediction payload has thousands of them, each waking the agent's
/// reader. The encoded line is assembled first so the kernel sees it once.
#[cfg(unix)]
fn write_request_line(stream: &mut impl Write, request: &AgentRequest) -> Result<()> {
    let mut encoded = serde_json::to_vec(request)?;
    encoded.push(b'\n');
    stream.write_all(&encoded)?;
    stream.flush()?;
    Ok(())
}

fn validate_handshake_response(response: &str) -> Result<()> {
    match serde_json::from_str(response)? {
        AgentResponse::Hello {
            protocol,
            agent_version,
        } if protocol == AGENT_PROTOCOL_VERSION && agent_version == VERSION => Ok(()),
        AgentResponse::Error { message } => bail!(message),
        _ => bail!("the cache agent returned an incompatible handshake"),
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
