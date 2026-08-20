//! Build-session lifecycle: the cache agent, its transport, and the rustc shim
//! that cargo invokes through `RUSTC_WRAPPER`.

use crate::config::Config;
use crate::util::{duration_ns, format_duration, write_atomic};
use bytesize::ByteSize;
use eyre::{Context, Result, bail};
use log::{debug, warn};
use mbx_cache_core::{
    AGENT_PROTOCOL_VERSION, AgentRemoteCache, AgentRequest, AgentResponse, AgentStats, CacheAgent,
    CacheDigest, RemoteCacheClient, RemoteCacheConfig, canonical_json,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub const RUSTC_SHIM_STEM: &str = "mbx-rustc";
const SOCKET_ENV: &str = "MBX_SOCKET";
pub(crate) const STAGING_ENV: &str = "MBX_STAGING_DIR";
pub(crate) const BUILD_ENV: &str = "MBX_BUILD";
pub(crate) const VERIFY_ENV: &str = "MBX_VERIFY";
const PREVIOUS_RUSTC_WRAPPER_ENV: &str = "MBX_PREVIOUS_RUSTC_WRAPPER";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const IDENTITY_VERSION: u8 = 1;

/// A cache session: the agent, its listener, and the shim cargo will invoke.
pub struct CacheSession {
    socket: String,
    rustc_shim: PathBuf,
    staging: PathBuf,
    verify: bool,
    agent: CacheAgent,
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
        let staging = session_dir.join("staging");
        std::fs::create_dir(&staging)?;
        let store = config.store_dir();
        let agent = if let Some(remote) = action_remote_cache(config, &store)? {
            CacheAgent::new_remote(store, VERSION, remote)
        } else {
            CacheAgent::new(store, VERSION)
        };
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (socket, server) = spawn_server(session_dir, agent.clone(), shutdown_rx).await?;
        Ok(Self {
            socket,
            rustc_shim: shim,
            staging,
            verify: config.verify,
            agent,
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
        command: &[String],
        environment: &mut BTreeMap<String, String>,
    ) -> Option<ActionRun> {
        let identity = build_identity(workspace_root, command);
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
        environment.insert(SOCKET_ENV.into(), self.socket.clone());
        environment.insert(
            STAGING_ENV.into(),
            self.staging.to_string_lossy().into_owned(),
        );
        environment.insert(BUILD_ENV.into(), protocol_build);
        if self.verify {
            environment.insert(VERIFY_ENV.into(), "1".into());
        } else {
            environment.remove(VERIFY_ENV);
        }
        if let Some(previous) = environment.insert("RUSTC_WRAPPER".into(), shim.clone())
            && previous != shim
        {
            environment.insert(PREVIOUS_RUSTC_WRAPPER_ENV.into(), previous);
        }
        // Incremental compilation is never cacheable, so it is disabled rather
        // than left to bypass every action.
        environment.insert("CARGO_INCREMENTAL".into(), "0".into());
        action_run
    }

    /// Stop the agent and collect this session's statistics.
    pub async fn finish(&self) -> Result<AgentStats> {
        if let Some(shutdown) = self.shutdown.lock().unwrap().take() {
            let _ = shutdown.send(());
        }
        let server = self.server.lock().unwrap().take();
        if let Some(server) = server {
            server.await??;
        }
        self.agent.cancel_prefetches().await;
        let mut stats = self.agent.stats();
        stats.session_duration_ns = duration_ns(self.started.elapsed());
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

/// An in-flight build's action manifest, committed when the build succeeds.
pub struct ActionRun {
    run: String,
    agent: CacheAgent,
}

impl ActionRun {
    pub async fn commit(self) -> Result<()> {
        self.agent.commit_task(&self.run).await
    }
}

#[derive(Serialize)]
struct ActionIdentity<'a> {
    version: u8,
    workspace: &'a str,
    command: &'a [String],
}

/// Identity for this build's prefetch manifest.
///
/// Manifests are namespaced by identity, so this only affects how well one
/// build can predict another's actions; action keys themselves are independent
/// of it.
pub fn build_identity(workspace_root: &Path, command: &[String]) -> String {
    let workspace = workspace_marker(workspace_root);
    let material = ActionIdentity {
        version: IDENTITY_VERSION,
        workspace: &workspace,
        command,
    };
    let bytes = canonical_json(&material).expect("build identity must serialize");
    CacheDigest::blake3(&bytes).hash
}

/// Identify a workspace by its dependency graph rather than its location, so
/// that separate worktrees of one project share a manifest.
fn workspace_marker(workspace_root: &Path) -> String {
    match std::fs::read(workspace_root.join("Cargo.lock")) {
        Ok(lock) => CacheDigest::blake3(&lock).hash,
        Err(_) => workspace_root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    }
}

fn action_remote_cache(config: &Config, store: &Path) -> Result<Option<AgentRemoteCache>> {
    let Some(base_url) = config.remote.url.clone() else {
        return Ok(None);
    };
    let namespace = config
        .remote
        .namespace
        .as_deref()
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty())
        .ok_or_else(|| eyre::eyre!("a remote cache namespace is required when a URL is set"))?
        .to_string();
    let client = RemoteCacheClient::new(RemoteCacheConfig {
        base_url: base_url.parse().wrap_err("invalid remote cache URL")?,
        namespace,
        token: config.remote.token.clone(),
        token_file: config.remote.token_file.clone(),
        oidc_audience: config.remote.oidc_audience.clone(),
        connect_timeout: config.http.timeout,
        read_timeout: config.http.timeout,
        download_timeout: config.http.download_timeout,
        retries: config.http.retries,
    })?;
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
    compiler_invocations_avoided: u64,
    verifications: u64,
    divergences: u64,
    prefetched_actions: u64,
    prefetch_runs: u64,
    downloaded_bytes: u64,
    uploaded_bytes: u64,
    stored_bytes: u64,
    restored_output_files: u64,
    restored_output_bytes: u64,
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

impl From<&AgentStats> for StatsReport {
    fn from(stats: &AgentStats) -> Self {
        Self {
            version: 1,
            session_duration_ns: stats.session_duration_ns,
            lookups: stats.lookups,
            hits: stats.hits,
            misses: cache_misses(stats),
            compiler_invocations_avoided: stats.hits,
            verifications: stats.verifications,
            divergences: stats.divergences,
            prefetched_actions: stats.prefetched_actions,
            prefetch_runs: stats.prefetch_runs,
            downloaded_bytes: stats.downloaded_bytes,
            uploaded_bytes: stats.uploaded_bytes,
            stored_bytes: stats.stored_bytes,
            restored_output_files: stats.restored_output_files,
            restored_output_bytes: stats.restored_output_bytes,
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
    if stats.lookups == 0
        && stats.stores == 0
        && stats.verifications == 0
        && stats.downloaded_bytes == 0
        && stats.uploaded_bytes == 0
    {
        return;
    }
    note(&format!(
        "cache: {} hits, {} misses, {} prefetched; {} downloaded, {} uploaded, {} stored locally",
        stats.hits,
        cache_misses(stats),
        stats.prefetched_actions,
        ByteSize::b(stats.downloaded_bytes).display().iec(),
        ByteSize::b(stats.uploaded_bytes).display().iec(),
        ByteSize::b(stats.stored_bytes).display().iec(),
    ));
    let remote_lookup_duration_ns = stats
        .remote_manifest_lookup_duration_ns
        .saturating_add(stats.remote_action_lookup_duration_ns);
    note(&format!(
        "cache timing: {} session, {} prefetch; cumulative {} remote lookup, {} blob transfer, {} CAS write, {} materialization",
        format_nanos(stats.session_duration_ns),
        format_nanos(stats.prefetch_duration_ns),
        format_nanos(remote_lookup_duration_ns),
        format_nanos(stats.remote_blob_transfer_duration_ns),
        format_nanos(stats.local_cas_write_duration_ns),
        format_nanos(stats.materialization_duration_ns),
    ));
    if stats.verifications > 0 {
        note(&format!(
            "cache qualification: {} verified, {} diverged",
            stats.verifications, stats.divergences,
        ));
    }
}

/// Write to stderr without failing the build when the pipe is closed.
fn note(message: &str) {
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

fn cache_misses(stats: &AgentStats) -> u64 {
    stats
        .lookups
        .saturating_sub(stats.hits)
        .saturating_sub(stats.verifications)
}

fn install_session_shim(session_dir: &Path) -> Result<PathBuf> {
    let executable = std::env::current_exe().wrap_err("failed to locate the running mbx binary")?;
    install_shim(&executable, session_dir)
}

/// Install a rustc shim into `directory` as a link to `executable`.
///
/// The shim must be the same binary as the agent: the handshake requires an
/// exact version match, which a hard link guarantees for free.
pub fn install_shim(executable: &Path, directory: &Path) -> Result<PathBuf> {
    let filename = if cfg!(windows) {
        format!("{RUSTC_SHIM_STEM}.exe")
    } else {
        RUSTC_SHIM_STEM.into()
    };
    let shim = directory.join(filename);
    let _ = std::fs::remove_file(&shim);
    if let Err(link_error) = std::fs::hard_link(executable, &shim) {
        std::fs::copy(executable, &shim).wrap_err_with(|| {
            format!("failed to install the rustc shim by hard link ({link_error}) or copy")
        })?;
    }
    Ok(shim)
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
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let agent = agent.clone();
                    tokio::spawn(async move {
                        if let Err(error) = agent.handle_connection(stream).await {
                            debug!("cache agent connection failed: {error}");
                        }
                    });
                }
                _ = &mut shutdown => return Ok(()),
            }
        }
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
        loop {
            let pipe = next_server
                .take()
                .expect("the next named-pipe server is always prepared");
            tokio::select! {
                connected = pipe.connect() => {
                    connected?;
                    next_server = Some(create_named_pipe(&server_endpoint, false)?);
                    let agent = agent.clone();
                    tokio::spawn(async move {
                        if let Err(error) = agent.handle_connection(pipe).await {
                            debug!("cache agent connection failed: {error}");
                        }
                    });
                }
                _ = &mut shutdown => return Ok(()),
            }
        }
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

/// Ultra-early argv0 path used by cargo's `RUSTC_WRAPPER` integration.
///
/// Cargo invokes this thousands of times per build, so it runs before any
/// runtime, logging, or configuration is set up. Cacheable invocations restore
/// from or publish through the cache; anything else is a transparent compiler
/// call.
pub fn run_rustc_shim() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let Some(rustc) = arguments.next() else {
        eprintln!("mbx: the rustc shim expected the rustc executable as its first argument");
        return ExitCode::from(1);
    };
    let arguments = arguments.collect::<Vec<_>>();
    if std::env::var_os(PREVIOUS_RUSTC_WRAPPER_ENV).is_none() {
        match crate::rustc::compile(&rustc, &arguments) {
            Ok(exit_code) => return exit_code,
            Err(_error) => {
                #[cfg(debug_assertions)]
                eprintln!("mbx: rustc cache bypassed: {_error:#}");
            }
        }
    }

    run_transparent_rustc(rustc, arguments)
}

fn run_transparent_rustc(rustc: OsString, arguments: Vec<OsString>) -> ExitCode {
    let mut command = if let Some(wrapper) = std::env::var_os(PREVIOUS_RUSTC_WRAPPER_ENV) {
        let mut command = Command::new(wrapper);
        command.arg(&rustc);
        command
    } else {
        Command::new(&rustc)
    };
    command.args(arguments);
    command.env_remove(PREVIOUS_RUSTC_WRAPPER_ENV);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let error = command.exec();
        eprintln!("mbx: the rustc shim failed to execute rustc: {error}");
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
                // SAFETY: This process is only a transparent compiler wrapper.
                // ExitProcess is required to preserve Windows exception codes,
                // which cannot be represented by stable Rust's ExitCode API.
                unsafe { windows_sys::Win32::System::Threading::ExitProcess(exit_code) }
            }
            Err(error) => {
                eprintln!("mbx: the rustc shim failed to execute rustc: {error}");
                ExitCode::from(1)
            }
        }
    }
}

pub(crate) fn request_agent(requests: &[AgentRequest]) -> Result<Vec<AgentResponse>> {
    let socket =
        std::env::var_os(SOCKET_ENV).ok_or_else(|| eyre::eyre!("{SOCKET_ENV} is not set"))?;
    request_agent_at(&socket, requests)
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
mod tests {
    use super::*;

    fn test_config(cache_dir: &Path) -> Config {
        Config {
            cache_dir: cache_dir.to_path_buf(),
            stats_report: None,
            verify: false,
            remote: Default::default(),
            http: Default::default(),
        }
    }

    #[tokio::test]
    async fn session_environment_directs_cargo_at_the_shim() {
        let cache = tempfile::tempdir().unwrap();
        let session_dir = tempfile::tempdir().unwrap();
        let session = CacheSession::start(session_dir.path(), &test_config(cache.path()))
            .await
            .unwrap();

        let workspace = tempfile::tempdir().unwrap();
        let mut values = BTreeMap::from([("RUSTC_WRAPPER".into(), "existing".into())]);
        let run = session
            .begin(workspace.path(), &["build".to_string()], &mut values)
            .await;

        assert!(run.is_some());
        assert!(values.contains_key(SOCKET_ENV));
        assert!(values.contains_key(STAGING_ENV));
        assert_eq!(values.get(BUILD_ENV).unwrap().len(), 64);
        // The shim carries an .exe suffix on Windows, so compare stems.
        let wrapper = Path::new(values.get("RUSTC_WRAPPER").unwrap());
        assert_eq!(wrapper.file_stem().unwrap(), RUSTC_SHIM_STEM);
        assert_eq!(values.get(PREVIOUS_RUSTC_WRAPPER_ENV).unwrap(), "existing");
        assert_eq!(values.get("CARGO_INCREMENTAL").unwrap(), "0");
        assert!(!values.contains_key(VERIFY_ENV));

        session.finish().await.unwrap();
    }

    #[tokio::test]
    async fn verify_mode_is_passed_to_the_shim() {
        let cache = tempfile::tempdir().unwrap();
        let session_dir = tempfile::tempdir().unwrap();
        let mut config = test_config(cache.path());
        config.verify = true;
        let session = CacheSession::start(session_dir.path(), &config)
            .await
            .unwrap();

        let workspace = tempfile::tempdir().unwrap();
        let mut values = BTreeMap::new();
        session
            .begin(workspace.path(), &["build".to_string()], &mut values)
            .await;

        assert_eq!(values.get(VERIFY_ENV).unwrap(), "1");

        session.finish().await.unwrap();
    }

    #[test]
    fn identity_follows_the_dependency_graph_not_the_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("Cargo.lock"), "version = 4\n").unwrap();
        std::fs::write(second.path().join("Cargo.lock"), "version = 4\n").unwrap();
        let command = ["build".to_string()];

        assert_eq!(
            build_identity(first.path(), &command),
            build_identity(second.path(), &command),
            "separate worktrees of one project must share a manifest"
        );

        std::fs::write(second.path().join("Cargo.lock"), "version = 3\n").unwrap();
        assert_ne!(
            build_identity(first.path(), &command),
            build_identity(second.path(), &command),
        );
        assert_ne!(
            build_identity(first.path(), &command),
            build_identity(first.path(), &["test".to_string()]),
        );
    }

    #[test]
    fn identity_falls_back_to_the_directory_name() {
        let directory = tempfile::tempdir().unwrap();
        let command = ["build".to_string()];
        assert_eq!(
            build_identity(directory.path(), &command).len(),
            64,
            "a project without a lockfile still gets an identity"
        );
    }

    #[test]
    fn handshake_rejects_version_skew() {
        let response = serde_json::to_string(&AgentResponse::Hello {
            protocol: AGENT_PROTOCOL_VERSION,
            agent_version: "another-version".into(),
        })
        .unwrap();
        assert!(validate_handshake_response(&response).is_err());
    }

    #[test]
    fn qualification_results_are_not_reported_as_misses() {
        let stats = AgentStats {
            lookups: 5,
            hits: 2,
            verifications: 2,
            ..AgentStats::default()
        };
        assert_eq!(cache_misses(&stats), 1);
    }

    #[test]
    fn writes_versioned_stats_report() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("stats.json");
        let stats = AgentStats {
            session_duration_ns: 42,
            lookups: 5,
            hits: 2,
            verifications: 1,
            prefetched_actions: 3,
            downloaded_bytes: 1024,
            restored_output_files: 7,
            restored_output_bytes: 2048,
            remote_blob_requests: 4,
            remote_blob_pack_requests: 2,
            remote_blob_pack_blobs: 100,
            materialization_duration_ns: 9,
            ..AgentStats::default()
        };

        write_stats_report(&path, &stats).unwrap();
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();

        assert_eq!(report["version"], 1);
        assert_eq!(report["session_duration_ns"], 42);
        assert_eq!(report["hits"], 2);
        assert_eq!(report["misses"], 2);
        assert_eq!(report["compiler_invocations_avoided"], 2);
        assert_eq!(report["prefetched_actions"], 3);
        assert_eq!(report["downloaded_bytes"], 1024);
        assert_eq!(report["restored_output_files"], 7);
        assert_eq!(report["restored_output_bytes"], 2048);
        assert_eq!(report["remote_blob_requests"], 4);
        assert_eq!(report["remote_blob_pack_requests"], 2);
        assert_eq!(report["remote_blob_pack_blobs"], 100);
        assert_eq!(report["materialization_duration_ns"], 9);
    }
}
