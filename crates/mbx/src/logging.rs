//! Coordinate background diagnostics with the inline terminal renderer.
use std::io::{self, Write};
use std::sync::Mutex;

static CAPTURE: Mutex<Option<Vec<u8>>> = Mutex::new(None);

struct Stderr;

impl Write for Stderr {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut capture = CAPTURE.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(buffer) = capture.as_mut() {
            buffer.extend_from_slice(bytes);
            Ok(bytes.len())
        } else {
            io::stderr().write(bytes)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

/// portable-pty reports a pty it could not open or spawn into at error level
/// before returning that failure to its caller, and mbx is the caller that
/// decides what the failure means: an inline view that cannot start is not a
/// build problem, because the build continues under plain Cargo. Keep the
/// library quiet by default so its copy of the message does not reach a user
/// whose build then succeeds. `MBX_LOG` replaces this filter outright, so
/// `MBX_LOG=debug` still shows the reason the view stood down.
pub(crate) const DEFAULT_FILTER: &str = "info,portable_pty=off";

/// Initialize the command logger while retaining its existing filter settings.
pub fn init() {
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("MBX_LOG", DEFAULT_FILTER))
        .format_target(false)
        .format_timestamp(None)
        .target(env_logger::Target::Pipe(Box::new(Stderr)))
        .init();
}

pub(crate) fn note(message: &str) {
    let _ = writeln!(Stderr, "{message}");
}

/// The screen owns this guard; its destructor restores terminal mode before
/// this field is dropped, so even diagnostics arriving during shutdown survive.
pub(crate) struct Capture;

impl Capture {
    pub(crate) fn start() -> Self {
        *CAPTURE.lock().unwrap_or_else(|error| error.into_inner()) = Some(Vec::new());
        Self
    }

    pub(crate) fn drain(&self) -> Vec<u8> {
        CAPTURE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let mut capture = CAPTURE.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(bytes) = capture.take() {
            let _ = io::stderr().write_all(&bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;

    fn enabled(target: &str, level: log::Level) -> bool {
        env_logger::Builder::new()
            .parse_filters(DEFAULT_FILTER)
            .build()
            .enabled(&log::Metadata::builder().target(target).level(level).build())
    }

    #[test]
    fn a_pty_failure_mbx_recovers_from_is_not_announced_to_the_user() {
        assert!(!enabled("portable_pty", log::Level::Error));
        assert!(enabled("mbx::cli::cargo", log::Level::Info));
        assert!(!enabled("mbx::cli::cargo", log::Level::Debug));
    }

    #[test]
    fn background_notes_and_logger_writes_share_the_screen_queue() {
        let capture = Capture::start();
        std::thread::spawn(|| {
            note("background warning");
            Stderr.write_all(b"logger warning\n").unwrap();
        })
        .join()
        .unwrap();
        let output = String::from_utf8(capture.drain()).unwrap();
        assert!(output.contains("background warning"));
        assert!(output.contains("logger warning"));
    }
}
