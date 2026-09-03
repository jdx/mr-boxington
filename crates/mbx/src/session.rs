//! Build-session lifecycle: the cache agent, its transport, and the rustc shim
//! that cargo invokes through `RUSTC_WRAPPER`.

use crate::config::Config;
use crate::events::{ActionDetail, ActionOutcome, EventWriter};
use crate::util::duration_ns;
use eyre::Result;
use log::{debug, warn};
use mbx_cache_cc::CcLanguage;
#[cfg(test)]
use mbx_cache_core::AGENT_PROTOCOL_VERSION;
use mbx_cache_core::{
    ActionDiagnostic, AgentEvent, AgentEventObserver, AgentRemoteCache, AgentRequest,
    AgentResponse, AgentStats, CacheAgent, CacheDigest, FileDigestCache, FileDigestScope,
    FileIdentity, NoFileDigestCache, RecordedFileDigest, canonical_json,
};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
#[cfg(unix)]
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

mod client;
mod diagnostics;
mod server;
mod shims;
mod stats;

#[cfg(test)]
use client::validate_handshake_response;
use client::{request_agent_at, request_standalone_agent};
pub(crate) use diagnostics::{note, report_shim_warning, reserve_stderr_for_compiler};
#[cfg(unix)]
pub(crate) use server::create_fifo;
pub(crate) use server::listener_unavailable;
use server::spawn_server;
pub(crate) use shims::CC_CRATE_ENV;
#[cfg(all(test, windows))]
use shims::link_path_shim;
use shims::{CcShims, install_cc_shims, install_session_shims};
pub use shims::{
    PathShims, ShimLink, install_path_shims, install_shim, install_shim_named, shim_file_name,
};
#[cfg(test)]
use shims::{
    TargetedCompiler, resolve_in_path, resolve_named_compiler, resolve_on_path, symlink_shim,
    targeted_compiler_language,
};
use stats::StatsReport;
pub(crate) use stats::display_stats;
pub(crate) use stats::session_was_active;
#[cfg(test)]
use stats::{
    cache_misses, short_summary, should_display_short_stats, should_display_stats,
    stale_manifest_note,
};

pub const RUSTC_SHIM_STEM: &str = "mbx-rustc";
pub const RUSTDOC_SHIM_STEM: &str = "mbx-rustdoc";
pub const CC_SHIM_STEM: &str = "mbx-cc";
pub const CXX_SHIM_STEM: &str = "mbx-cxx";
pub(crate) const PATH_SHIMS_ENV: &str = "MBX_CC_SHIM_COMPILERS";
pub(crate) const SOCKET_ENV: &str = "MBX_SOCKET";
pub(crate) const REAL_CC_ENV: &str = "MBX_REAL_CC";
pub(crate) const REAL_CXX_ENV: &str = "MBX_REAL_CXX";
pub(crate) const STAGING_ENV: &str = "MBX_STAGING_DIR";
pub(crate) const BUILD_ENV: &str = "MBX_BUILD";
pub(crate) const VERIFY_ENV: &str = "MBX_VERIFY";
pub(crate) const SHARE_OUT_DIR_ENV: &str = "MBX_SHARE_OUT_DIR";
pub(crate) const BUILD_SCRIPT_EXECUTION_ENV: &str = "MBX_BUILD_SCRIPT_EXECUTION";
pub(crate) const LEARNED_INCREMENTAL_ENV: &str = "MBX_LEARNED_INCREMENTAL";
pub(crate) const LEARNED_INCREMENTAL_MAX_SIZE_ENV: &str = "MBX_LEARNED_INCREMENTAL_MAX_SIZE";
pub(crate) const INCREMENTAL_ROOT_ENV: &str = "MBX_INCREMENTAL_ROOT";
pub(crate) const MANAGED_LINKER_ENV: &str = "MBX_MANAGED_LINKER";
pub const CACHE_LINKS_ENV: &str = "MBX_CACHE_LINKS";
/// Group completed builds for one later cache export, used by CI actions.
pub const CACHE_EXPORT_GROUP_ENV: &str = "MBX_CACHE_EXPORT_GROUP";
pub(crate) const WORKSPACE_ROOT_ENV: &str = "MBX_WORKSPACE_ROOT";
pub(crate) const TARGET_DIR_ENV: &str = "MBX_TARGET_DIR";
pub(crate) const BUILD_SCRIPT_REAL_SUFFIX: &str = ".mbx-real";
const PREVIOUS_RUSTC_WRAPPER_ENV: &str = "MBX_PREVIOUS_RUSTC_WRAPPER";
const PREVIOUS_RUSTC_WORKSPACE_WRAPPER_ENV: &str = "MBX_PREVIOUS_RUSTC_WORKSPACE_WRAPPER";
const REAL_RUSTDOC_ENV: &str = "MBX_REAL_RUSTDOC";
pub(crate) const BYPASS_LOG_ENV: &str = "MBX_BYPASS_LOG";
const VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg(unix)]
static SHIM_STAGING_NONCE: AtomicU64 = AtomicU64::new(0);

/// A cache session: the agent, its listener, and the shim cargo will invoke.
pub struct CacheSession {
    socket: String,
    rustc_shim: PathBuf,
    rustdoc_shim: PathBuf,
    cc_shims: Option<CcShims>,
    staging: PathBuf,
    verify: bool,
    incremental: bool,
    share_out_dir: bool,
    build_script_execution: bool,
    agent: CacheAgent,
    /// The stream `mbx tui` watches, when event recording is on.
    events: Option<EventStream>,
    /// What the shims need to draw compile permits from the machine-wide pool.
    scheduler_env: Vec<(String, String)>,
    store: PathBuf,
    incremental_root: PathBuf,
    /// The checkout's private state directory once `begin` has claimed it,
    /// where the file-digest ledger is saved when the session finishes.
    ledger_dir: Mutex<Option<PathBuf>>,
    /// What the ledger file looked like when this session read it, so a
    /// session that was alone in the checkout can write without merging.
    ledger_stamp: Arc<Mutex<Option<crate::digest_ledger::Stamp>>>,
    started: Instant,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    server: Mutex<Option<JoinHandle<Result<()>>>>,
    task: Arc<SessionTask>,
}

/// The Cargo task is loaded only if a compiler shim connects.
pub(super) struct SessionTask {
    identity: std::sync::OnceLock<String>,
    initialized: tokio::sync::OnceCell<bool>,
    /// Whether any shim connected, which is what makes a run worth committing.
    connected: std::sync::atomic::AtomicBool,
}

impl CacheSession {
    /// Install the shim, start the agent, and begin serving the shim's requests.
    ///
    /// `session_dir` holds the shim, socket, and staging directory, and is
    /// expected to be a temporary directory owned by the caller.
    pub async fn start(session_dir: &Path, config: &Config) -> Result<Self> {
        Self::start_with_jobs(session_dir, config, None).await
    }

    /// Start a session whose Cargo jobserver limits compiler concurrency.
    pub(crate) async fn start_with_jobs(
        session_dir: &Path,
        config: &Config,
        cargo_jobs: Option<u64>,
    ) -> Result<Self> {
        let (shim, rustdoc_shim) =
            install_session_shims(session_dir, &config.cache_dir.join("shims"))?;
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
        let task = Arc::new(SessionTask {
            identity: std::sync::OnceLock::new(),
            initialized: tokio::sync::OnceCell::new(),
            connected: std::sync::atomic::AtomicBool::new(false),
        });
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
            .map(EventStream::new);
        let agent = match &events {
            Some(events) => agent.with_observer(Arc::new(events.clone())),
            None => agent,
        };
        // A shim that needs the build's predictions is the one that waits for
        // the manifest; the probes cargo runs first only report bypasses.
        let agent = agent.with_task_loader({
            let task = Arc::clone(&task);
            Arc::new(move |agent: &CacheAgent, identity: &str| {
                let task = Arc::clone(&task);
                let identity = identity.to_string();
                Box::pin(async move {
                    if task.identity.get().is_some_and(|known| *known == identity) {
                        server::initialize_task(agent, &task).await;
                    }
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>
            })
        });
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (socket, server) =
            spawn_server(session_dir, agent.clone(), Arc::clone(&task), shutdown_rx).await?;
        Ok(Self {
            socket,
            rustc_shim: shim,
            rustdoc_shim,
            cc_shims,
            staging,
            verify: config.verify,
            incremental: config.incremental,
            share_out_dir: config.share_out_dir,
            build_script_execution: config.build_script_execution,
            agent,
            events,
            scheduler_env: crate::scheduler::session_environment_with_jobs(config, cargo_jobs),
            store,
            incremental_root: config.cache_dir.join("incremental"),
            ledger_dir: Mutex::new(None),
            ledger_stamp: Arc::new(Mutex::new(None)),
            started: Instant::now(),
            shutdown: Mutex::new(Some(shutdown_tx)),
            server: Mutex::new(Some(server)),
            task,
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
            events.writer.started(workspace_root, command);
        }
        // Recorded before the build rather than after it: a build that fails
        // still means this checkout is here and using the store, and the record
        // is what stops the collector treating its artifacts as abandoned.
        if let Err(error) =
            crate::store::record_checkout(&self.store, &identity, workspace_root, target_dir)
        {
            warn!("this checkout was not recorded as a cache root: {error}");
        }
        let _ = self.task.identity.set(identity.clone());
        // Read now, in the background, rather than when the first shim
        // connects: cargo spends its own startup planning the build, and that
        // is time the manifest can be parsed in instead of stalling the first
        // compilation on it.
        {
            let agent = self.agent.clone();
            let task = Arc::clone(&self.task);
            tokio::spawn(async move {
                server::initialize_task(&agent, &task).await;
            });
        }
        let action_run = Some(ActionRun {
            run: identity.clone(),
            receipt: build_receipt_run(&identity),
            identity: identity.clone(),
            workspace_root: workspace_root.to_path_buf(),
            export_group: std::env::var(CACHE_EXPORT_GROUP_ENV).ok(),
            store: self.store.clone(),
            agent: self.agent.clone(),
            initialized: Some(Arc::clone(&self.task)),
        });
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
        // Always replace an inherited value. If recording this checkout fails,
        // the shim must use its target-local fallback rather than another
        // checkout's persistent state.
        environment.insert(
            INCREMENTAL_ROOT_ENV.into(),
            target_dir
                .join("mbx-incremental")
                .to_string_lossy()
                .into_owned(),
        );
        match crate::incremental::touch(&self.incremental_root, workspace_root) {
            Ok(root) => {
                environment.insert(
                    INCREMENTAL_ROOT_ENV.into(),
                    root.to_string_lossy().into_owned(),
                );
                // Seeded under the ledger's own lock on a blocking thread, so
                // cargo's startup overlaps the read, and a shim that asks
                // before it is done waits for the answer instead of hashing
                // every unchanged dependency again.
                let agent = self.agent.clone();
                let ledger_root = root.clone();
                let stamp = Arc::clone(&self.ledger_stamp);
                tokio::task::spawn_blocking(move || {
                    let seeded = agent.seed_file_digests_with(|| {
                        *stamp.lock().unwrap() = crate::digest_ledger::stamp(&ledger_root);
                        crate::digest_ledger::load(&ledger_root)
                    });
                    debug!("file-digest ledger seeded with {seeded} entries");
                });
                *self.ledger_dir.lock().unwrap() = Some(root);
            }
            Err(error) => warn!("incremental state was not recorded: {error:#}"),
        }
        environment.insert(SOCKET_ENV.into(), self.socket.clone());
        environment.insert(
            STAGING_ENV.into(),
            self.staging.to_string_lossy().into_owned(),
        );
        environment.insert(BUILD_ENV.into(), identity);
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
        environment.insert(
            BUILD_SCRIPT_EXECUTION_ENV.into(),
            if self.build_script_execution {
                "1"
            } else {
                "0"
            }
            .into(),
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
        if let Some(previous) = environment.get("RUSTC_WORKSPACE_WRAPPER") {
            // Cargo nests this inside RUSTC_WRAPPER, so the shim receives the
            // workspace wrapper where it ordinarily receives rustc. Remember
            // that path to recognize the nested invocation. Clippy has a
            // modeled identity and configuration inputs; other workspace
            // wrappers remain transparent because mbx cannot key their work.
            if executable_stem(OsStr::new(previous)) != Some("clippy-driver") {
                warn!(
                    "RUSTC_WORKSPACE_WRAPPER is already set to {previous}; deferring to it for workspace crates, so those compilations are not cached"
                );
            }
            environment.insert(
                PREVIOUS_RUSTC_WORKSPACE_WRAPPER_ENV.into(),
                previous.clone(),
            );
        }
        let rustdoc = configured_rustdoc(environment);
        environment.insert(REAL_RUSTDOC_ENV.into(), rustdoc);
        environment.insert(
            "RUSTDOC".into(),
            self.rustdoc_shim.to_string_lossy().into_owned(),
        );
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
                let value = environment
                    .get(*name)
                    .cloned()
                    .or_else(|| std::env::var(name).ok())
                    .unwrap_or_else(|| "<non-UTF-8 value>".into());
                append_cacheability_observation(
                    environment,
                    "cc-compiler-override",
                    &format!(
                        "{name} is already set to `{value}`, so host C and C++ compilations do not pass through mbx; unset it for this build to make them visible to the cache"
                    ),
                );
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
            events.writer.started(project_root, command);
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
        let (protocol_build, action_run) =
            match self.agent.begin_task_on_prediction(&identity).await {
                Ok(run) => (
                    run.clone(),
                    Some(ActionRun {
                        receipt: run.clone(),
                        run,
                        identity: identity.clone(),
                        workspace_root: project_root.to_path_buf(),
                        export_group: std::env::var(CACHE_EXPORT_GROUP_ENV).ok(),
                        store: self.store.clone(),
                        agent: self.agent.clone(),
                        initialized: None,
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
        // Saved after the shims are done and before the summary, so the next
        // session in this checkout starts from everything this one hashed.
        // Best-effort like the ledger itself.
        if let Some(ledger_dir) = self.ledger_dir.lock().unwrap().take() {
            let stamp = self.ledger_stamp.lock().unwrap().take();
            match crate::digest_ledger::save(&ledger_dir, self.agent.file_digests(), stamp) {
                Ok(count) => debug!("file-digest ledger saved with {count} entries"),
                Err(error) => warn!("the file-digest ledger was not saved: {error:#}"),
            }
        }
        let mut stats = self.agent.stats();
        stats.session_duration_ns = duration_ns(self.started.elapsed());
        // The same totals the summary reports, so a reader of a finished stream
        // does not have to re-derive them from the rows -- and so a stream that
        // hit its row cap still ends with the whole truth.
        if let Some(events) = &self.events
            && let Ok(totals) = serde_json::to_value(StatsReport::from(&stats))
        {
            events.writer.finished(totals);
        }
        Ok(stats)
    }
}

/// Select the rustdoc behind any session shim already present in the caller.
///
/// Integration tests (and nested `mbx` commands in general) can begin a cache
/// session from inside another one. Chaining the outer rustdoc shim would make
/// it name itself as the real rustdoc and recurse; unwrap it just as the rustc
/// path preserves and explicitly models an existing wrapper.
fn configured_rustdoc(environment: &BTreeMap<String, String>) -> String {
    let configured = environment
        .get("RUSTDOC")
        .cloned()
        .or_else(|| std::env::var("RUSTDOC").ok())
        .unwrap_or_else(|| "rustdoc".into());
    if Path::new(&configured).file_stem() == Some(OsStr::new(RUSTDOC_SHIM_STEM)) {
        environment
            .get(REAL_RUSTDOC_ENV)
            .cloned()
            .or_else(|| std::env::var(REAL_RUSTDOC_ENV).ok())
            .unwrap_or_else(|| "rustdoc".into())
    } else {
        configured
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
struct EventStream {
    writer: Arc<EventWriter>,
    diagnostics: Arc<Mutex<VecDeque<PendingDiagnostic>>>,
}

struct PendingDiagnostic {
    outcome: String,
    crate_name: Option<String>,
    diagnostic: ActionDiagnostic,
}

impl EventStream {
    fn new(writer: Arc<EventWriter>) -> Self {
        Self {
            writer,
            diagnostics: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn take_diagnostic(&self, outcome: &str, crate_name: Option<&str>) -> Option<ActionDiagnostic> {
        let mut diagnostics = self.diagnostics.lock().unwrap();
        let position = diagnostics.iter().position(|pending| {
            pending.outcome == outcome && pending.crate_name.as_deref() == crate_name
        })?;
        diagnostics
            .remove(position)
            .map(|pending| pending.diagnostic)
    }
}

impl AgentEventObserver for EventStream {
    fn event(&self, event: AgentEvent) {
        match event {
            AgentEvent::ActionHit {
                crate_name,
                restore,
            } => {
                let diagnostic = self.take_diagnostic("hit", crate_name.as_deref());
                self.writer.action_with_diagnostic(
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
                    diagnostic,
                );
            }
            AgentEvent::Bypass { kind } => self.writer.action(
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
                let recorded_outcome = match outcome.as_str() {
                    "miss" => ActionOutcome::Miss,
                    "unconsulted" => ActionOutcome::Unconsulted,
                    // A verification's own row comes from the verification
                    // event, which knows whether it matched; a bypass already
                    // has one.
                    _ => return,
                };
                let diagnostic = self.take_diagnostic(&outcome, crate_name.as_deref());
                self.writer.action_with_diagnostic(
                    recorded_outcome,
                    crate_name,
                    duration_ns,
                    ActionDetail::default(),
                    diagnostic,
                );
            }
            AgentEvent::Verification { matched, restore } => self.writer.action(
                ActionOutcome::Verification { matched },
                None,
                restore.duration_ns,
                ActionDetail::default(),
            ),
            AgentEvent::ActionDiagnostic {
                outcome,
                crate_name,
                diagnostic,
            } => self
                .diagnostics
                .lock()
                .unwrap()
                .push_back(PendingDiagnostic {
                    outcome,
                    crate_name,
                    diagnostic,
                }),
            // A decision this build does not know how to describe is left out
            // rather than guessed at; the totals still count it.
            _ => {}
        }
    }
}

/// An in-flight build's completed action manifest.
pub struct ActionRun {
    run: String,
    receipt: String,
    identity: String,
    workspace_root: PathBuf,
    export_group: Option<String>,
    store: PathBuf,
    agent: CacheAgent,
    initialized: Option<Arc<SessionTask>>,
}

impl ActionRun {
    pub async fn commit(self) -> Result<()> {
        if self.initialized.as_ref().is_some_and(|task| {
            !task.connected.load(std::sync::atomic::Ordering::Relaxed)
                || task.initialized.get() != Some(&true)
        }) {
            return Ok(());
        }
        let predictions = self.agent.commit_task_actions(&self.run).await?;
        // A build that predicted nothing new has nothing to export that the
        // receipt before it did not already name, so that receipt stands. A
        // grouped export still gets its receipt: the group is the record of
        // which runs took part.
        if predictions.is_empty() && self.export_group.is_none() {
            return Ok(());
        }
        crate::store::record_build_receipt(
            &self.store,
            &self.receipt,
            &self.identity,
            &self.workspace_root,
            self.export_group.as_deref(),
            predictions,
        )
    }
}

/// A unique receipt name for one invocation of a stable Cargo task.
fn build_receipt_run(identity: &str) -> String {
    CacheDigest::blake3(
        format!(
            "{identity}\0{}\0{}",
            std::process::id(),
            crate::util::random_string(12)
        )
        .as_bytes(),
    )
    .hash
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
    os: &'static str,
    arch: &'static str,
}

/// Identity of a standalone build: the project and the command it runs.
///
/// Worktrees of one project must share a manifest for predictions to travel --
/// the cc adapter has no second way to build a key -- so the marker prefers
/// content and origin over location: a `Cargo.lock` digest where one exists,
/// then the Git or Jujutsu origin URL, and only then the directory name.
pub fn exec_identity(project_root: &Path, command: &[String]) -> String {
    let project = exec_marker(project_root);
    let material = ExecIdentity {
        version: 2,
        project: &project,
        command,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    };
    let bytes = canonical_json(&material).expect("exec identity must serialize");
    CacheDigest::blake3(&bytes).hash
}

fn exec_marker(project_root: &Path) -> String {
    if let Ok(lock) = std::fs::read(project_root.join("Cargo.lock")) {
        return CacheDigest::blake3(&lock).hash;
    }
    if let Some(origin) = project_origin_marker(project_root) {
        return origin;
    }
    project_root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// The origin URL names a project the same way in every worktree and clone,
/// which a checkout's directory name does not. Try Git first for colocated
/// repositories so they keep exactly the identity they had before.
fn project_origin_marker(project_root: &Path) -> Option<String> {
    if !project_root.join(".git").exists() {
        if project_root.join(".jj").exists() {
            return jj_origin_marker(project_root);
        }
        if project_root.join(".sl").exists() {
            return sapling_origin_marker(project_root);
        }
        if project_root.join(".hg").exists() {
            return mercurial_origin_marker(project_root);
        }
    }
    git_origin_marker(project_root)
        .or_else(|| jj_origin_marker(project_root))
        .or_else(|| sapling_origin_marker(project_root))
        .or_else(|| mercurial_origin_marker(project_root))
}

/// Read Git's origin without imposing a repository layout on explicit roots.
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
    origin_marker_from_output(&output.stdout)
}

/// Read Jujutsu's origin only from a root that is itself a Jujutsu checkout.
fn jj_origin_marker(project_root: &Path) -> Option<String> {
    // Without this gate `jj -R` may discover an unrelated enclosing checkout.
    if !project_root.join(".jj").exists() {
        return None;
    }
    let output = Command::new("jj")
        .arg("-R")
        .arg(project_root)
        .arg("--ignore-working-copy")
        .args(["git", "remote", "list"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let remotes = String::from_utf8_lossy(&output.stdout);
    let url = jj_origin_url(&remotes)?;
    Some(format!("origin\0{url}"))
}

/// Extract the fetch URL for the conventionally named origin remote.
fn jj_origin_url(remotes: &str) -> Option<&str> {
    remotes.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        if fields.next()? == "origin" {
            fields.next()
        } else {
            None
        }
    })
}

/// Read the default source from a native Sapling checkout.
fn sapling_origin_marker(project_root: &Path) -> Option<String> {
    default_path_origin_marker("sl", project_root, &["config", "paths.default"], ".sl")
}

/// Read the default source from a Mercurial checkout.
fn mercurial_origin_marker(project_root: &Path) -> Option<String> {
    default_path_origin_marker("hg", project_root, &["paths", "default"], ".hg")
}

fn default_path_origin_marker(
    program: &str,
    project_root: &Path,
    arguments: &[&str],
    marker: &str,
) -> Option<String> {
    // Without this gate the command may discover an unrelated enclosing
    // checkout, just as Jujutsu would.
    if !project_root.join(marker).exists() {
        return None;
    }
    let output = Command::new(program)
        .arg("-R")
        .arg(project_root)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    origin_marker_from_output(&output.stdout)
}

fn origin_marker_from_output(output: &[u8]) -> Option<String> {
    let url = String::from_utf8_lossy(output).trim().to_string();
    (!url.is_empty()).then(|| format!("origin\0{url}"))
}

fn action_remote_cache(config: &Config, store: &Path) -> Result<Option<AgentRemoteCache>> {
    let Some(client) = crate::remote::remote_client(config)? else {
        return Ok(None);
    };
    let Some(mode) = crate::policy::effective_remote_cache_mode(config.remote.mode) else {
        return Ok(None);
    };
    Ok(Some(AgentRemoteCache {
        client,
        mode,
        staging_dir: store.join("remote"),
    }))
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

/// Whether this process was invoked as the rustdoc shim.
pub fn is_rustdoc_shim() -> bool {
    std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_stem)
        .is_some_and(|stem| stem == OsStr::new(RUSTDOC_SHIM_STEM))
}

/// Ultra-early argv0 path used by Cargo's `RUSTDOC` integration.
pub fn run_rustdoc_shim() -> ExitCode {
    let rustdoc = std::env::var_os(REAL_RUSTDOC_ENV).unwrap_or_else(|| "rustdoc".into());
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    match crate::rustdoc::document(&rustdoc, &arguments) {
        Ok(code) => code,
        Err(error) => {
            let _ = request_agent(&[AgentRequest::RecordBypass {
                kind: "rustdoc".into(),
            }]);
            #[cfg(debug_assertions)]
            eprintln!("mbx[warning]: rustdoc cache bypassed: {error:#}");
            run_transparent_rustdoc(rustdoc, arguments)
        }
    }
}

fn run_transparent_rustdoc(rustdoc: OsString, arguments: Vec<OsString>) -> ExitCode {
    let status = Command::new(&rustdoc).args(arguments).status();
    match status {
        Ok(status) => ExitCode::from(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1)),
        Err(error) => {
            eprintln!("mbx[error]: the rustdoc shim failed to execute rustdoc: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Whether this process replaced a Cargo build-script executable.
pub fn is_build_script_shim() -> bool {
    let Some(invoked) = std::env::args_os().next().map(PathBuf::from) else {
        return false;
    };
    invoked
        .file_stem()
        .and_then(OsStr::to_str)
        .is_some_and(|stem| stem == "build-script-build")
        && find_build_script_real_path(&invoked).is_some()
}

pub(crate) fn build_script_real_path(executable: &Path) -> PathBuf {
    let mut name = executable.as_os_str().to_os_string();
    name.push(BUILD_SCRIPT_REAL_SUFFIX);
    PathBuf::from(name)
}

pub(crate) fn build_script_execution_requested() -> bool {
    std::env::var_os(BUILD_SCRIPT_EXECUTION_ENV)
        .is_some_and(|value| !value.is_empty() && value != "0")
}

/// Locate the preserved binary. Cargo runs an un-hashed hard link named
/// `build-script-build`, while rustc produced and mbx wrapped the hashed
/// `build_script_build-<unit>` sibling.
pub(crate) fn find_build_script_real_path(executable: &Path) -> Option<PathBuf> {
    let direct = build_script_real_path(executable);
    if direct.is_file() {
        return Some(direct);
    }
    let parent = executable.parent()?;
    let mut matches = std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| {
                        name.starts_with("build_script_build-")
                            && name.ends_with(BUILD_SCRIPT_REAL_SUFFIX)
                    })
        });
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

/// Run Cargo's build script through the execution cache.
pub fn run_build_script_shim() -> ExitCode {
    // The wrapper lives in Cargo's target directory, so it can outlive the mbx
    // session that installed it. A later plain `cargo` invocation must remain
    // a transparent build-script call.
    if session_socket().is_none() || !build_script_execution_requested() {
        return crate::build_script::run_real();
    }
    match crate::build_script::run() {
        Ok(code) => code,
        Err(error) => {
            report_shim_warning(&format!("build-script cache bypassed: {error:#}"));
            crate::build_script::run_real()
        }
    }
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
    ("cl.exe", CcLanguage::Cxx),
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
        "cl" if cfg!(windows) => Some(CcLanguage::Cxx),
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
    let mut arguments = arguments.collect::<Vec<_>>();
    if let Some(linker) = std::env::var_os(MANAGED_LINKER_ENV).filter(|path| !path.is_empty()) {
        use_managed_linker(&mut arguments, &linker);
    }
    let is_workspace_wrapper = std::env::var_os(PREVIOUS_RUSTC_WORKSPACE_WRAPPER_ENV)
        .is_some_and(|wrapper| wrapper == rustc);
    let (wrapper_argument, compiler_arguments) = workspace_wrapper_arguments(&rustc, &arguments);
    let cacheable_workspace_wrapper = is_workspace_wrapper && wrapper_argument.is_some();
    if std::env::var_os(PREVIOUS_RUSTC_WRAPPER_ENV).is_none()
        && (!is_workspace_wrapper || cacheable_workspace_wrapper)
    {
        // Cargo composes RUSTC_WRAPPER outside RUSTC_WORKSPACE_WRAPPER. Clippy
        // occupies the latter, so its wrapper protocol arrives here as
        // `clippy-driver <real-rustc> <rustc arguments>`. The real rustc path
        // must still be passed to the driver when it executes, but it is not a
        // source input and must not be handed to the rustc argument parser.
        match crate::rustc::compile(&rustc, compiler_arguments, wrapper_argument) {
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

/// Route native links through the selected executable while leaving metadata
/// queries and `cargo check` byte-for-byte unchanged.
fn use_managed_linker(arguments: &mut Vec<OsString>, linker: &OsStr) {
    if links_natively(arguments) {
        arguments.push("-Clinker=clang".into());
        arguments.push(format!("-Clink-arg=-fuse-ld={}", Path::new(linker).display()).into());
    }
}

fn workspace_wrapper_arguments<'a>(
    compiler: &OsStr,
    arguments: &'a [OsString],
) -> (Option<&'a OsStr>, &'a [OsString]) {
    let wrapper_argument = arguments.first().filter(|argument| {
        executable_stem(compiler) == Some("clippy-driver")
            && executable_stem(argument) == Some("rustc")
    });
    match wrapper_argument {
        Some(argument) => (Some(argument.as_os_str()), &arguments[1..]),
        None => (None, arguments),
    }
}

fn executable_stem(executable: &OsStr) -> Option<&str> {
    Path::new(executable).file_stem()?.to_str()
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
    command.env_remove(PREVIOUS_RUSTC_WORKSPACE_WRAPPER_ENV);

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

pub(crate) fn crate_name_argument(arguments: &[OsString]) -> Option<String> {
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
    cfg!(any(target_os = "linux", target_os = "macos", windows))
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

/// How much incremental state one crate may keep before the shim discards it;
/// `None` is no limit. The session writes the resolved setting, but a shim
/// running without one may inherit the user's own spelling, so both a byte
/// count and a size with a unit are accepted. Anything unreadable falls back
/// to the declared default rather than to no limit or to zero.
pub(crate) fn learned_incremental_max_size() -> Option<u64> {
    let default = Some(crate::config::DEFAULT_LEARNED_INCREMENTAL_MAX_SIZE);
    match std::env::var(LEARNED_INCREMENTAL_MAX_SIZE_ENV) {
        Ok(value) if !value.trim().is_empty() => {
            crate::config::parse_optional_byte_size(&value).unwrap_or(default)
        }
        _ => default,
    }
}

/// Tell the session that this compilation was not cacheable.
///
/// Bypasses never reach the agent otherwise, so without this they are invisible
/// outside a debug build. Reported by reason kind rather than message, since
/// several reasons carry a path or a flag.
fn record_bypass(error: &eyre::Report) {
    let reason = error.downcast_ref::<mbx_cache_rustc::BypassReason>();
    let kind = reason.map_or("other", mbx_cache_rustc::BypassReason::kind);
    append_bypass_log(
        kind,
        error,
        reason.and_then(mbx_cache_rustc::BypassReason::remediation),
    );
    // A shim running outside a session has nowhere to report, which is fine.
    let _ = request_agent(&[AgentRequest::RecordBypass { kind: kind.into() }]);
}

/// Tell the session that a C or C++ compilation was not cacheable.
///
/// Kinds are prefixed so that a reason the two adapters share by name, such as
/// an unmodeled flag, does not merge into one statistic covering both.
fn record_cc_bypass(error: &eyre::Report) {
    let reason = error.downcast_ref::<mbx_cache_cc::CcBypassReason>();
    let kind = reason.map_or_else(
        || "cc-other".to_string(),
        |reason| format!("cc-{}", reason.kind()),
    );
    append_bypass_log(
        &kind,
        error,
        reason.and_then(mbx_cache_cc::CcBypassReason::remediation),
    );
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
    record_compiler_invocation_with_diagnostic(outcome, crate_name, duration_ns, None);
}

pub(crate) fn record_compiler_invocation_with_diagnostic(
    outcome: &str,
    crate_name: Option<&str>,
    duration_ns: u64,
    diagnostic: Option<mbx_cache_core::ActionDiagnostic>,
) {
    let mut requests = Vec::new();
    if let Some(diagnostic) = diagnostic
        && let Some(request) = action_diagnostic_request(outcome, crate_name, diagnostic)
    {
        requests.push(request);
    }
    requests.push(AgentRequest::RecordCompilerInvocation {
        outcome: outcome.into(),
        crate_name: crate_name.map(str::to_string),
        duration_ns,
    });
    let _ = request_agent(&requests);
}

const ACTION_DIAGNOSTIC_PREFIX: &str = "@mbx-action-diagnostic\t";

#[derive(Serialize)]
struct ActionDiagnosticEnvelope<'a> {
    outcome: &'a str,
    crate_name: Option<&'a str>,
    diagnostic: ActionDiagnostic,
}

pub(crate) fn action_diagnostic_request(
    outcome: &str,
    crate_name: Option<&str>,
    diagnostic: ActionDiagnostic,
) -> Option<AgentRequest> {
    // Keep diagnostics on the v6 wire without adding fields to the public,
    // exhaustively matchable AgentRequest variants. The agent recognizes this
    // reserved warning envelope and does not surface it as a user warning.
    let payload = serde_json::to_string(&ActionDiagnosticEnvelope {
        outcome,
        crate_name,
        diagnostic,
    })
    .ok()?;
    Some(AgentRequest::RecordWarning {
        message: format!("{ACTION_DIAGNOSTIC_PREFIX}{payload}"),
    })
}

/// Append the full reason to `MBX_BYPASS_LOG`, when one is configured.
///
/// The aggregate counts say which kinds dominate; this says exactly which flag
/// or path caused each one. It exists because stderr cannot be relied on:
/// cargo swallows the output of its own probe invocations, so some bypasses are
/// invisible there.
fn append_bypass_log(kind: &str, error: &eyre::Report, remediation: Option<&str>) {
    let Some(path) = std::env::var_os(BYPASS_LOG_ENV).filter(|path| !path.is_empty()) else {
        return;
    };
    // O_APPEND places each write at the end of the file, so one write per
    // record is what keeps parallel shims from splicing their lines together.
    // Records are a single short line for that reason: a write the kernel had
    // to break up could still interleave, and nothing here can prevent it.
    let suffix = remediation.map_or_else(String::new, |text| format!("\t{text}"));
    let line = format!("{kind}\t{error:#}{suffix}\n");
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

/// Record a cacheability problem that prevents compiler invocations from ever
/// reaching a shim. It is separate from bypasses because there is no observed
/// compilation to count.
fn append_cacheability_observation(
    environment: &BTreeMap<String, String>,
    kind: &str,
    detail: &str,
) {
    let Some(path) = environment
        .get(BYPASS_LOG_ENV)
        .filter(|path| !path.is_empty())
    else {
        return;
    };
    let line = format!("@observation\t{kind}\t{detail}\n");
    if let Err(problem) = append_line(OsStr::new(path), &line) {
        debug!("cacheability observation was not recorded: {problem}");
    }
}

fn append_line(path: &OsStr, line: &str) -> std::io::Result<()> {
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

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
