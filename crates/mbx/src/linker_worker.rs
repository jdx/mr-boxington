//! Lifetime management for a selected linker's optional mbx worker.

use crate::managed_linker::Selection;
use eyre::{Context, Result, bail};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(2);
const START_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) struct Worker {
    child: Child,
    socket: String,
}

impl Worker {
    pub(crate) async fn start(session_dir: &Path, selection: &Selection) -> Result<Option<Self>> {
        if !selection.starts_worker {
            return Ok(None);
        }
        if !cfg!(unix) {
            bail!("this managed linker worker requires Unix domain sockets");
        }
        let socket_path = session_dir.join("linker.sock");
        let mut worker = Self {
            child: Command::new(&selection.executable)
                .arg("--mbx-worker")
                .arg(&socket_path)
                .arg(std::process::id().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .wrap_err_with(|| {
                    format!(
                        "failed to start linker worker `{}`",
                        selection.executable.display()
                    )
                })?,
            socket: socket_path.to_string_lossy().into_owned(),
        };
        let started = Instant::now();
        loop {
            if socket_path.exists() {
                return Ok(Some(worker));
            }
            if let Some(status) = worker.child.try_wait()? {
                bail!("linker worker exited before accepting links: {status}");
            }
            if started.elapsed() >= START_TIMEOUT {
                bail!(
                    "linker worker did not create `{}` within {} seconds",
                    socket_path.display(),
                    START_TIMEOUT.as_secs()
                );
            }
            tokio::time::sleep(START_POLL_INTERVAL).await;
        }
    }

    pub(crate) fn socket(&self) -> &str {
        &self.socket
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
