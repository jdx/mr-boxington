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

/// Initialize the command logger while retaining its existing filter settings.
pub fn init() {
    env_logger::Builder::from_env(env_logger::Env::default().filter_or("MBX_LOG", "info"))
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
