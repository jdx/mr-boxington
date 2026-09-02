//! Lifetime management for the jdxld worker owned by an mbx build session.

use eyre::{Context, Result, bail};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(2);
const START_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A jdxld worker and the connection details inherited by compiler shims.
pub(crate) struct Worker {
    child: Child,
    executable: String,
    socket: String,
}

impl Worker {
    /// Start the configured worker, or leave jdxld disabled when no path was set.
    pub(crate) async fn start(
        session_dir: &Path,
        configured: Option<&Path>,
    ) -> Result<Option<Self>> {
        let Some(configured) = configured else {
            return Ok(None);
        };
        if !cfg!(target_os = "linux") {
            bail!("jdxld session linking is supported only on Linux");
        }

        let executable = which::which(configured)
            .wrap_err_with(|| format!("failed to find jdxld `{}`", configured.display()))?;
        // Clang accepts an absolute `--ld-path`, unlike GCC's `-fuse-ld` on
        // common distributions. Check it before Cargo starts doing work.
        which::which("clang")
            .wrap_err("failed to find the linker driver `clang` required by jdxld")?;
        let socket_path = session_dir.join("jdxld.sock");
        let socket = socket_path.to_string_lossy().into_owned();
        let executable_string = executable.to_string_lossy().into_owned();
        let child = Command::new(&executable)
            .arg("--mbx-worker")
            .arg(&socket_path)
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .wrap_err_with(|| format!("failed to start jdxld `{}`", executable.display()))?;
        // Own the child before the first await so cancellation runs Drop and
        // cannot strand a worker whose socket has not appeared yet.
        let mut worker = Self {
            child,
            executable: executable_string,
            socket,
        };

        let started = Instant::now();
        loop {
            if socket_path.exists() {
                return Ok(Some(worker));
            }
            if let Some(status) = worker
                .child
                .try_wait()
                .wrap_err("failed to inspect the jdxld worker")?
            {
                bail!("jdxld worker exited before accepting links: {status}");
            }
            if started.elapsed() >= START_TIMEOUT {
                bail!(
                    "jdxld worker did not create `{}` within {} seconds",
                    socket_path.display(),
                    START_TIMEOUT.as_secs()
                );
            }
            tokio::time::sleep(START_POLL_INTERVAL).await;
        }
    }

    pub(crate) fn executable(&self) -> &str {
        &self.executable
    }

    pub(crate) fn socket(&self) -> &str {
        &self.socket
    }

    /// Stop the worker once Cargo and all compiler shims have exited.
    pub(crate) fn finish(mut self) -> Result<()> {
        self.stop().wrap_err("failed to stop the jdxld worker")
    }

    fn stop(&mut self) -> std::io::Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        let _ = self.child.wait()?;
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
