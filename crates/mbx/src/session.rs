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
    AgentResponse, AgentStats, CacheAgent,
};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub const RUSTC_SHIM_STEM: &str = "mbx-rustc";
pub const CC_SHIM_STEM: &str = "mbx-cc";
pub const CXX_SHIM_STEM: &str = "mbx-cxx";
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
            install_cc_shims(session_dir)?
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
        if let Some(name) = CC_CRATE_ENV
            .iter()
            .find(|name| environment.contains_key(**name) || std::env::var_os(name).is_some())
        {
            debug!("{name} is already set; C and C++ compilations are not cached");
            return;
        }
        // `HOST_CC` rather than `CC`, because of where each sits in the `cc`
        // crate's lookup order: it reads `CC_<target>`, then `HOST_CC` or
        // `TARGET_CC` depending on whether it is cross-compiling, and only then
        // plain `CC`. Setting `CC` would capture cross compiles too, and these
        // shims wrap the *host* compiler -- a `cargo build --target` would
        // silently build target objects with the host driver. `HOST_CC` is
        // consulted only when host and target agree, which is exactly the
        // compilation these shims can stand in for.
        shims.apply_to(environment);
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
            version: 2,
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
            "mbx[cache]: materialization: {} outputs ({}) reflinked, {} outputs ({}) copied",
            stats.reflinked_output_files,
            ByteSize::b(stats.reflinked_output_bytes).display().iec(),
            stats.copied_output_files,
            ByteSize::b(stats.copied_output_bytes).display().iec(),
        ));
    }
    if stats.verifications > 0 {
        note(&format!(
            "mbx[cache]: qualification: {} verified, {} diverged",
            stats.verifications, stats.divergences,
        ));
    }
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
}

impl CcShims {
    /// Point build scripts at whichever shims were installed.
    ///
    /// A language with no compiler on the machine contributes nothing, so a C
    /// build on an image without a C++ compiler still gets its caching.
    fn apply_to(&self, environment: &mut BTreeMap<String, String>) {
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
}

/// Variables the `cc` crate consults before falling back to the platform
/// default. A build that sets any of them has chosen its own compiler, and mbx
/// stands aside rather than redirecting it.
const CC_CRATE_ENV: &[&str] = &[
    "CC",
    "CXX",
    "HOST_CC",
    "HOST_CXX",
    "TARGET_CC",
    "TARGET_CXX",
];

/// Install the C and C++ shims, resolving the compilers they will run.
///
/// Resolution happens here rather than in the shim so the whole build agrees on
/// one compiler, and so a machine with no C compiler simply gets no shims
/// instead of a build script that fails differently than it would have.
fn install_cc_shims(session_dir: &Path) -> Result<Option<CcShims>> {
    if !cfg!(unix) {
        return Ok(None);
    }
    let executable = std::env::current_exe().wrap_err("failed to locate the running mbx binary")?;
    // Each language stands alone. An image with a C compiler and no C++ one is
    // ordinary, and it must not cost a C-only sys-crate its caching.
    let real_cc = resolve_on_path(CcLanguage::C.default_driver());
    let real_cxx = resolve_on_path(CcLanguage::Cxx.default_driver());
    if real_cc.is_none() && real_cxx.is_none() {
        debug!("no C or C++ compiler was found on PATH; build script compiles are not cached");
        return Ok(None);
    }
    let shim = |language: CcLanguage| -> Result<PathBuf> {
        install_shim_named(
            &executable,
            session_dir,
            language.shim_stem(),
            ShimLink::Tracking,
        )
    };
    let mut installed = CcShims {
        cc: None,
        cxx: None,
    };
    if let Some(real) = real_cc {
        installed.cc = Some((shim(CcLanguage::C)?, real));
    }
    if let Some(real) = real_cxx {
        installed.cxx = Some((shim(CcLanguage::Cxx)?, real));
    }
    Ok(Some(installed))
}

fn resolve_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
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
        _ => None,
    }
}

/// Ultra-early argv0 path used by the `CC` and `CXX` build scripts inherit.
///
/// Unlike `RUSTC_WRAPPER`, there is no convention that hands the shim the real
/// compiler: the build script calls `$CC` and every argument is the
/// compilation's own. The compiler to run therefore arrives out of band, and a
/// shim that cannot find one falls back to the platform default rather than
/// failing a build it was only meant to observe.
pub fn run_cc_shim(language: CcLanguage) -> ExitCode {
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
    match crate::cc::compile(&compiler, &arguments, language) {
        Ok(exit_code) => return exit_code,
        Err(error) => {
            record_cc_bypass(&error);
            #[cfg(debug_assertions)]
            eprintln!("mbx[warning]: cc cache bypassed: {error:#}");
        }
    }
    run_transparent_cc(compiler, arguments)
}

/// The compiler a cc shim stands in for.
///
/// The session pins this when it installs the shims. A shim invoked with that
/// variable missing searches `PATH` for the platform default, skipping any
/// candidate that is this binary, so an inherited `CC` cannot make the shim
/// call itself.
fn real_compiler(language: CcLanguage) -> Result<OsString> {
    let pinned = match language {
        CcLanguage::C => REAL_CC_ENV,
        CcLanguage::Cxx => REAL_CXX_ENV,
    };
    if let Some(compiler) = std::env::var_os(pinned).filter(|value| !value.is_empty()) {
        return Ok(compiler);
    }
    let current = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(&path).ok().or(Some(path)));
    let name = language.default_driver();
    let path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if !candidate.is_file() {
            continue;
        }
        let resolved = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if current.as_deref() == Some(resolved.as_path()) {
            continue;
        }
        return Ok(candidate.into_os_string());
    }
    eyre::bail!("no {name} was found on PATH")
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
    #[cfg(windows)]
    let crate_name = crate_name_argument(&arguments);
    #[cfg(windows)]
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

        let error = command.exec();
        eprintln!("mbx[error]: the rustc shim failed to execute rustc: {error}");
        ExitCode::from(1)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;

        match command.spawn().and_then(|mut child| {
            child.wait()?;
            let mut exit_code = 1;
            // SAFETY: the child owns a valid process handle until it is
            // dropped, and `exit_code` is a valid out pointer.
            if unsafe { GetExitCodeProcess(child.as_raw_handle().cast(), &raw mut exit_code) } == 0
            {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(exit_code)
            }
        }) {
            Ok(exit_code) => {
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

#[cfg(any(windows, test))]
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

/// Whether this build may cache natively linked programs.
///
/// Restricted to platforms whose linker mbx knows how to identify: a host it
/// cannot describe would otherwise key a link as though the linker did not
/// matter.
pub fn cache_links_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
}

/// Whether the shim may cache a natively linked program. Read the same way as
/// verify mode.
///
/// The platform is checked here rather than trusted from whoever set the
/// variable: a shim installed by `mbx setup` is driven by plain cargo, with no
/// session to have applied the gate, so the value it reads is whatever the
/// developer exported.
pub(crate) fn cache_links_requested() -> bool {
    cache_links_supported()
        && std::env::var_os(CACHE_LINKS_ENV).is_some_and(|value| !value.is_empty() && value != "0")
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
    match std::env::var_os(SOCKET_ENV).filter(|socket| !socket.is_empty()) {
        Some(socket) => request_agent_at(&socket, requests),
        None => request_standalone_agent(requests),
    }
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
    let mut stream = std::os::unix::net::UnixStream::connect(Path::new(socket))
        .wrap_err("failed to connect to the cache session")?;
    sync_handshake(&mut stream)?;
    requests
        .iter()
        .map(|request| sync_request(&mut stream, request))
        .collect()
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
    serde_json::to_writer(&mut *stream, &request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut response = String::new();
    BufReader::new(&mut *stream).read_line(&mut response)?;
    validate_handshake_response(&response)
}

#[cfg(unix)]
fn sync_request(
    stream: &mut (impl std::io::Read + Write),
    request: &AgentRequest,
) -> Result<AgentResponse> {
    serde_json::to_writer(&mut *stream, request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut response = String::new();
    BufReader::new(&mut *stream).read_line(&mut response)?;
    Ok(serde_json::from_str(&response)?)
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
