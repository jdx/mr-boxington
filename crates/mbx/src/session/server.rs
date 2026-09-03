#[cfg(windows)]
use eyre::Context;
use eyre::Result;
use log::debug;
use mbx_cache_core::CacheAgent;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[cfg(unix)]
#[derive(Debug)]
struct ListenerBindError {
    path: PathBuf,
    source: std::io::Error,
}

#[cfg(unix)]
impl std::fmt::Display for ListenerBindError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to bind cache session listener at {}: {}",
            self.path.display(),
            self.source
        )
    }
}

#[cfg(unix)]
impl std::error::Error for ListenerBindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(unix)]
impl ListenerBindError {
    fn unavailable(&self) -> bool {
        matches!(
            self.source.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
        )
    }
}

/// Whether session startup failed because this environment cannot bind the
/// local listener the compiler shims need.
#[cfg(unix)]
pub(crate) fn listener_unavailable(error: &eyre::Report) -> bool {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ListenerBindError>())
        .is_some_and(ListenerBindError::unavailable)
}

#[cfg(windows)]
pub(crate) fn listener_unavailable(_error: &eyre::Report) -> bool {
    false
}

#[cfg(unix)]
pub(super) async fn spawn_server(
    session_dir: &Path,
    agent: CacheAgent,
    task: Arc<super::SessionTask>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(String, JoinHandle<Result<()>>)> {
    use std::os::unix::fs::PermissionsExt as _;

    let socket = session_dir.join("cache-agent.sock");
    let listener = tokio::net::UnixListener::bind(&socket).map_err(|source| ListenerBindError {
        path: socket.clone(),
        source,
    })?;
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
                    let task = Arc::clone(&task);
                    connections.spawn(async move {
                        initialize_task(&agent, &task).await;
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn permission_and_platform_rejections_mean_the_listener_is_unavailable() {
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::Unsupported,
        ] {
            let error = eyre::Report::from(ListenerBindError {
                path: "/tmp/cache-agent.sock".into(),
                source: std::io::Error::from(kind),
            });
            assert!(listener_unavailable(&error));
        }
    }

    #[test]
    fn an_address_collision_remains_a_startup_error() {
        let error = eyre::Report::from(ListenerBindError {
            path: "/tmp/cache-agent.sock".into(),
            source: std::io::Error::from(std::io::ErrorKind::AddrInUse),
        });
        assert!(!listener_unavailable(&error));
    }
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
pub(super) async fn spawn_server(
    _session_dir: &Path,
    agent: CacheAgent,
    task: Arc<super::SessionTask>,
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
                    let task = Arc::clone(&task);
                    connections.spawn(async move {
                        initialize_task(&agent, &task).await;
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

async fn initialize_task(agent: &CacheAgent, task: &super::SessionTask) {
    let Some(identity) = task.identity.get() else {
        return;
    };
    task.initialized
        .get_or_init(|| async {
            match agent.begin_session_task(identity).await {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("build action manifest was not loaded: {error}");
                    false
                }
            }
        })
        .await;
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
