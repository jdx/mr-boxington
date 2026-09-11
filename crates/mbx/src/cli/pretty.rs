//! Inline Cargo progress, inspired by romancitodev's cargo-pretty.
//!
//! Keep Cargo's diagnostics intact and use only its human status messages for
//! the transient line. No unit-graph probe, nightly flags, or guessed totals.
use std::io::{self, IsTerminal, Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use ratatui::crossterm::terminal;

pub(super) fn enabled(arguments: &[String]) -> bool {
    eligible(arguments)
        && io::stderr().is_terminal()
        && io::stdout().is_terminal()
        && !crate::policy::is_ci()
        && std::env::var("TERM").as_deref() != Ok("dumb")
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("CARGO_TERM_COLOR").as_deref() != Ok("never")
        && std::env::var("CARGO_TERM_PROGRESS_WHEN").as_deref() != Ok("never")
        && supports_ansi()
}

fn supports_ansi() -> bool {
    #[cfg(windows)]
    return ratatui::crossterm::ansi_support::supports_ansi();
    #[cfg(not(windows))]
    true
}

fn eligible(arguments: &[String]) -> bool {
    // Run and test must inherit their terminal handles, including stderr.
    // Unknown/global options are passed through rather than reinterpreted.
    matches!(
        arguments.first().map(String::as_str),
        Some("build" | "b" | "check" | "c" | "clippy")
    ) && !arguments
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| {
            matches!(
                arg.as_str(),
                "--quiet"
                    | "--verbose"
                    | "--help"
                    | "--version"
                    | "--message-format"
                    | "--color"
                    | "--config"
            ) || arg.starts_with("--message-format=")
                || arg.starts_with("--color=")
                || arg.starts_with("--config=")
                || (arg.starts_with('-')
                    && !arg.starts_with("--")
                    && arg[1..].contains(['q', 'v', 'h', 'V']))
        })
}

pub(super) fn run(command: &mut Command) -> io::Result<ExitStatus> {
    run_with_output(command, &mut io::stderr())
}

fn run_with_output(command: &mut Command, output: &mut impl Write) -> io::Result<ExitStatus> {
    command
        .stderr(Stdio::piped())
        .env("CARGO_TERM_PROGRESS_WHEN", "never");
    let mut child = command.spawn()?;
    let mut stderr = child.stderr.take().expect("piped Cargo stderr");
    let (send, receive) = mpsc::sync_channel(16);
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut bytes = [0; 4096];
            loop {
                match stderr.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(size) => {
                        if send.send(Ok(bytes[..size].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = send.send(Err(error));
                        break;
                    }
                }
            }
        });
        let mut display = Display::default();
        let mut read_error = None;
        loop {
            match receive.recv_timeout(Duration::from_millis(80)) {
                Ok(Ok(bytes)) => display.feed(&bytes, output),
                Ok(Err(error)) => {
                    read_error = Some(error);
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    // Partial diagnostics must not wait for a newline or exit.
                    display.flush_partial(output);
                    let width = terminal::size()
                        .ok()
                        .map(|(width, _)| width)
                        .filter(|width| *width > 0)
                        .unwrap_or(80);
                    display.draw(output, width);
                }
            }
        }
        display.finish(output);
        // Always reap Cargo, including when its stderr could not be read.
        let status = child.wait()?;
        read_error.map_or(Ok(status), Err)
    })
}

struct Display {
    pending: Vec<u8>,
    passthrough_line: bool,
    action: Option<String>,
    started: Instant,
    frame: usize,
    drawn: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            passthrough_line: false,
            action: None,
            started: Instant::now(),
            frame: 0,
            drawn: false,
        }
    }
}

impl Display {
    fn clear(&mut self, output: &mut impl Write) {
        if self.drawn {
            let _ = output.write_all(b"\r\x1b[2K");
            self.drawn = false;
        }
    }

    fn feed(&mut self, bytes: &[u8], output: &mut impl Write) {
        for chunk in bytes.split_inclusive(|byte| *byte == b'\n') {
            self.pending.extend_from_slice(chunk);
            if chunk.ends_with(b"\n") {
                let action = if self.passthrough_line {
                    None
                } else {
                    cargo_action(&self.pending)
                };
                self.clear(output);
                if let Some(action) = action {
                    self.action = Some(action);
                } else {
                    self.action = None;
                    let _ = output.write_all(&self.pending);
                }
                self.pending.clear();
                self.passthrough_line = false;
            } else if self.pending.len() >= 8192 {
                self.flush_partial(output);
            }
        }
        let _ = output.flush();
    }

    fn flush_partial(&mut self, output: &mut impl Write) {
        if !self.pending.is_empty() {
            self.clear(output);
            self.action = None;
            let _ = output.write_all(&self.pending);
            self.pending.clear();
            self.passthrough_line = true;
            let _ = output.flush();
        }
    }

    fn draw(&mut self, output: &mut impl Write, width: u16) {
        let Some(action) = &self.action else {
            return;
        };
        // ASCII output gives a predictable cell width even on narrow terminals.
        let spinner = ['|', '/', '-', '\\'][self.frame % 4];
        let text = format!(
            "{spinner} [{:.1}s] {action}",
            self.started.elapsed().as_secs_f32()
        );
        let text: String = text
            .chars()
            .map(|c| {
                if c.is_ascii() && !c.is_control() {
                    c
                } else {
                    '?'
                }
            })
            .take(usize::from(width.saturating_sub(1)))
            .collect();
        self.clear(output);
        let _ = write!(output, "\r\x1b[36m{text}\x1b[0m");
        let _ = output.flush();
        self.frame += 1;
        self.drawn = true;
    }

    fn finish(&mut self, output: &mut impl Write) {
        self.clear(output);
        self.flush_partial(output);
        let _ = output.flush();
    }
}

fn cargo_action(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    // Only strip SGR color sequences. Other control sequences are not status
    // messages and must retain their original bytes on the passthrough path.
    let mut plain = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next()? != '[' {
                return None;
            }
            loop {
                match chars.next()? {
                    'm' => break,
                    '0'..='9' | ';' => {}
                    _ => return None,
                }
            }
        } else {
            plain.push(c);
        }
    }
    // Cargo right-aligns status verbs to twelve columns. Requiring this
    // prefix avoids swallowing similarly named build-script warnings.
    ["   Compiling ", "    Checking ", "    Building "]
        .iter()
        .find_map(|prefix| {
            plain.strip_prefix(prefix).map(|name| {
                let name = name.trim_end();
                let name = name.split_once(" (").map_or(name, |(package, _)| package);
                format!("{} {name}", prefix.trim())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn drains_child_output_and_preserves_failure_status() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf '   Compiling demo v1.0\\n' >&2; printf 'warning: keep me\\n' >&2; exit 23",
        ]);
        let mut output = Vec::new();
        let status = run_with_output(&mut command, &mut output).unwrap();
        assert_eq!(status.code(), Some(23));
        assert_eq!(output, b"warning: keep me\n");
    }

    #[test]
    fn narrow_terminal_does_not_wrap_the_status_line() {
        let mut display = Display::default();
        let mut output = Vec::new();
        display.feed(b"   Compiling demo v1.0\n", &mut output);
        display.draw(&mut output, 2);
        assert_eq!(output, b"\r\x1b[36m|\x1b[0m");
        display.finish(&mut output);
        assert!(output.ends_with(b"\r\x1b[2K"));
    }

    #[test]
    fn preserves_diagnostics_and_non_utf8_bytes() {
        let mut display = Display::default();
        let mut output = Vec::new();
        display.feed(b"   Compiling demo v1.0\n", &mut output);
        display.draw(&mut output, 40);
        display.feed(b"warning: hello\n\xff\n    Finished dev\n", &mut output);
        display.finish(&mut output);
        assert!(output.ends_with(b"warning: hello\n\xff\n    Finished dev\n"));
        assert!(display.action.is_none());
    }

    #[test]
    fn partial_lines_are_prompt_and_never_misclassified() {
        let mut display = Display::default();
        let mut output = Vec::new();
        display.feed(b"warning: ", &mut output);
        display.flush_partial(&mut output);
        assert_eq!(output, b"warning: ");
        display.feed(b"   Compiling something\n", &mut output);
        assert_eq!(output, b"warning:    Compiling something\n");
        display.feed(&vec![b'x'; 20000], &mut output);
        display.finish(&mut output);
        assert_eq!(
            output.len(),
            b"warning:    Compiling something\n".len() + 20000
        );
    }

    #[test]
    fn recognizes_only_cargo_status_lines() {
        assert_eq!(
            cargo_action(b"   Compiling demo v1.0 (/a/long/workspace/path)\n").as_deref(),
            Some("Compiling demo v1.0")
        );
        assert_eq!(
            cargo_action(b"\x1b[1m\x1b[32m   Compiling\x1b[0m demo v1.0\n").as_deref(),
            Some("Compiling demo v1.0")
        );
        assert_eq!(cargo_action(b"   Compiling demo\x1b[2K\n"), None);
        assert_eq!(cargo_action(b"warning: Compiling demo\n"), None);
        assert_eq!(cargo_action(b"Compiling demo\n"), None);
    }

    #[test]
    fn respects_machine_output_and_passthrough_commands() {
        for args in [
            vec!["test"],
            vec!["run"],
            vec!["build", "--message-format=json"],
            vec!["build", "-vv"],
            vec!["check", "-q"],
            vec!["build", "--config", "term.quiet=true"],
            vec!["build", "--color=never"],
        ] {
            assert!(!eligible(
                &args.iter().map(|arg| (*arg).into()).collect::<Vec<_>>()
            ));
        }
        assert!(eligible(&["build".into(), "--release".into()]));
        assert!(eligible(&[
            "clippy".into(),
            "--".into(),
            "-Dwarnings".into()
        ]));
    }
}
