//! Cargo-pretty's inline view, with session-local mbx cache information.
//!
//! Cargo remains the only orchestrator: the original command runs once in a
//! terminal, including runners and doctests. Only its presentation is adapted.
mod model;
#[cfg(test)]
mod tests;
mod upstream;
mod view;

use eyre::{Context, Result};
use mbx_cache_core::AgentStats;
use model::{Model, strip_ansi};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use ratatui::crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    execute, terminal,
};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io::{self, IsTerminal, Read, Write},
    process::ExitCode,
    sync::mpsc,
    time::{Duration, Instant},
};

pub(super) fn enabled(arguments: &[String]) -> bool {
    eligible(arguments)
        && io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && io::stderr().is_terminal()
        && !crate::policy::is_ci()
        && std::env::var("TERM").as_deref() != Ok("dumb")
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("CARGO_TERM_COLOR").as_deref() != Ok("never")
        && std::env::var("CARGO_TERM_PROGRESS_WHEN").as_deref() != Ok("never")
        && terminal::size().is_ok_and(|(cols, rows)| cols >= 50 && rows >= 16)
}

fn cargo_verb(arguments: &[String]) -> Option<&str> {
    arguments
        .get(usize::from(
            arguments.first().is_some_and(|arg| arg.starts_with('+')),
        ))
        .map(String::as_str)
}

fn eligible(arguments: &[String]) -> bool {
    matches!(
        cargo_verb(arguments),
        Some("build" | "b" | "check" | "c" | "clippy" | "run" | "r" | "test" | "t")
    ) && !arguments
        .iter()
        .take_while(|a| a.as_str() != "--")
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

fn cargo_arguments(arguments: &[String]) -> Vec<String> {
    let mut result = arguments.to_vec();
    let index = result
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(result.len());
    result.insert(
        index,
        "--message-format=json,json-diagnostic-rendered-ansi".into(),
    );
    result
}

pub(super) fn run(
    cargo: &OsStr,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    stats: impl Fn() -> AgentStats,
) -> Result<ExitCode> {
    let (cols, rows) = terminal::size()?;
    let pair = NativePtySystem::default()
        .openpty(pty_size(cols, rows))
        .map_err(|e| eyre::eyre!(e))?;
    let mut command = CommandBuilder::new(cargo);
    command.args(cargo_arguments(arguments));
    for (key, value) in environment {
        command.env(key, value);
    }
    command.cwd(std::env::current_dir()?);
    // Cargo supplies the build-unit denominator itself; no unstable unit-graph
    // probe, altered compiler settings or metadata guess is involved.
    command.env("CARGO_TERM_PROGRESS_WHEN", "always");
    command.env("CARGO_TERM_PROGRESS_WIDTH", cols.to_string());
    let mut reader = pair.master.try_clone_reader().map_err(|e| eyre::eyre!(e))?;
    let mut input = pair.master.take_writer().map_err(|e| eyre::eyre!(e))?;
    let mut screen = Screen::new()?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| eyre::eyre!(e))?;
    drop(pair.slave);
    let (send, receive) = mpsc::sync_channel(32);
    std::thread::spawn(move || {
        let mut bytes = [0; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    if send.send(Ok(bytes[..n].to_vec())).is_err() {
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
    let mut model = Model::new(arguments);
    let is_run = matches!(cargo_verb(arguments), Some("run" | "r"));
    let is_test = matches!(cargo_verb(arguments), Some("test" | "t"));
    let mut decoder = Decoder::default();
    let mut proxy = false;
    let mut last_frame = Instant::now() - Duration::from_secs(1);
    let result = (|| -> Result<portable_pty::ExitStatus> {
        loop {
            match receive.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(bytes)) => {
                    model.mix = stats().into();
                    decoder.feed(&bytes, &mut model, &mut screen, is_run, is_test, &mut proxy)?
                }
                Ok(Err(error)) => return Err(error.into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Do not wait on a descendant that outlives Cargo while
                    // retaining a copy of its output handle.
                    if child.try_wait().map_err(|e| eyre::eyre!(e))?.is_some() {
                        break;
                    }
                    decoder.flush_partial(&mut screen, model.build_finished)?;
                }
            }
            forward_input(&mut input)?;
            if let Ok((cols, rows)) = terminal::size() {
                let size = pty_size(cols, rows);
                if pair.master.get_size().map_err(|e| eyre::eyre!(e))? != size {
                    pair.master.resize(size).map_err(|e| eyre::eyre!(e))?;
                    screen.clear()?;
                }
            }
            if !proxy
                && decoder.pending.is_empty()
                && !decoder.partial
                && last_frame.elapsed() >= Duration::from_millis(80)
            {
                model.mix = stats().into();
                screen.draw(view::render(&model, None, screen.width(), screen.height()))?;
                last_frame = Instant::now();
            }
        }
        decoder.finish(&mut screen)?;
        child.wait().map_err(|e| eyre::eyre!(e))
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let status = result.wrap_err("running Cargo with the pretty display")?;
    model.mix = stats().into();
    model.finished = Some((status.success(), model.started.elapsed()));
    if !proxy {
        screen.draw(view::render(&model, None, screen.width(), screen.height()))?;
        // Full diagnostic text remains in scrollback even if the user dismisses
        // the optional browser, and the child's failure status stays authoritative.
        screen.commit();
        for error in &model.errors {
            screen.diagnostic(error)?;
            screen.write(b"\r\n")?;
        }
        let count = model.warnings.len() + model.failures.len();
        if count > 0 {
            let mut browser = view::Browser::default();
            loop {
                screen.draw(view::render(
                    &model,
                    Some(&mut browser),
                    screen.width(),
                    screen.height(),
                ))?;
                if let Event::Key(key) = event::read()?
                    && key.kind != event::KeyEventKind::Release
                {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break;
                        }
                        KeyCode::Up if browser.inspecting => {
                            browser.scroll = browser.scroll.saturating_sub(1)
                        }
                        KeyCode::Down if browser.inspecting => {
                            browser.scroll = browser.scroll.saturating_add(1)
                        }
                        KeyCode::Up => {
                            browser.selected = browser.selected.saturating_sub(1);
                            browser.scroll = 0;
                        }
                        KeyCode::Down => {
                            browser.selected = (browser.selected + 1).min(count - 1);
                            browser.scroll = 0;
                        }
                        KeyCode::Enter => {
                            browser.inspecting = !browser.inspecting;
                            browser.scroll = 0;
                        }
                        KeyCode::PageUp => browser.scroll = browser.scroll.saturating_sub(10),
                        KeyCode::PageDown => browser.scroll = browser.scroll.saturating_add(10),
                        _ => {}
                    }
                }
            }
            screen.clear()?;
            for warning in &model.warnings {
                screen.diagnostic(&warning.rendered)?;
                screen.write(b"\r\n")?;
            }
        }
        screen.commit();
    }
    Ok(ExitCode::from(status_code(&status)))
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn status_code(status: &portable_pty::ExitStatus) -> u8 {
    #[cfg(unix)]
    if let Some(name) = status.signal() {
        // portable-pty retains the signal's platform name rather than number.
        for signal in 1..=127 {
            let description = unsafe { libc::strsignal(signal) };
            if !description.is_null()
                && unsafe { std::ffi::CStr::from_ptr(description) }.to_string_lossy() == name
            {
                return 128 + signal as u8;
            }
        }
    }
    status.exit_code() as u8
}

// Unix input remains byte-for-byte terminal input, including function keys,
// paste sequences, control characters and application-specific escape codes.
#[cfg(unix)]
fn forward_input(input: &mut impl Write) -> io::Result<()> {
    let mut poll = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    if unsafe { libc::poll(&mut poll, 1, 0) } > 0 && poll.revents & libc::POLLIN != 0 {
        let mut bytes = [0u8; 8192];
        let size =
            unsafe { libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len()) };
        if size > 0 {
            input.write_all(&bytes[..size as usize])?;
            input.flush()?;
        } else if size < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn forward_input(input: &mut impl Write) -> io::Result<()> {
    while event::poll(Duration::ZERO)? {
        match event::read()? {
            Event::Key(key) if key.kind != event::KeyEventKind::Release => {
                input.write_all(&key_bytes(key))?
            }
            Event::Paste(text) => input.write_all(text.as_bytes())?,
            _ => {}
        }
    }
    input.flush()
}

#[cfg(windows)]
fn key_bytes(key: event::KeyEvent) -> Vec<u8> {
    let mut bytes = match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii() => {
            vec![(c as u8) & 0x1f]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![27],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::F(n @ 1..=4) => vec![27, b'O', b'P' + n - 1],
        KeyCode::F(n @ 5..=12) => format!(
            "\x1b[{}~",
            [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]
        )
        .into_bytes(),
        _ => Vec::new(),
    };
    if key.modifiers.contains(KeyModifiers::ALT) {
        bytes.insert(0, 27);
    }
    bytes
}

#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
    partial: bool,
    swallow_lf: bool,
}

impl Decoder {
    fn feed(
        &mut self,
        bytes: &[u8],
        model: &mut Model,
        screen: &mut Screen,
        is_run: bool,
        is_test: bool,
        proxy: &mut bool,
    ) -> io::Result<()> {
        if *proxy {
            return screen.write(bytes);
        }
        for chunk in bytes.split_inclusive(|byte| matches!(byte, b'\n' | b'\r')) {
            if *proxy {
                screen.write(chunk)?;
                continue;
            }
            if self.swallow_lf && chunk == b"\n" {
                self.swallow_lf = false;
                continue;
            }
            self.swallow_lf = false;
            self.pending.extend_from_slice(chunk);
            if chunk.ends_with(b"\n") || chunk.ends_with(b"\r") {
                let clean = std::str::from_utf8(&self.pending).ok().map(strip_ansi);
                let line = clean
                    .as_deref()
                    .unwrap_or("")
                    .trim_end_matches(['\n', '\r']);
                let handled = !self.partial
                    && ((!model.build_finished && (model.cargo(line) || model.status(line)))
                        || (is_test && model.test_line(line)));
                self.swallow_lf = handled && chunk.ends_with(b"\r");
                if !handled {
                    screen.write(&self.pending)?;
                }
                self.pending.clear();
                self.partial = false;
                if is_run && model.build_finished && model.build_ok == Some(true) {
                    model.finished = Some((true, model.started.elapsed()));
                    screen.draw(view::summary(model))?;
                    screen.commit();
                    for warning in &model.warnings {
                        screen.diagnostic(&warning.rendered)?;
                        screen.write(b"\r\n")?;
                    }
                    *proxy = true;
                    execute!(io::stderr(), cursor::Show)?;
                }
            } else if self.pending.len() >= 1024 * 1024 {
                self.partial = true;
                self.flush_partial(screen, true)?;
            }
        }
        Ok(())
    }

    fn flush_partial(&mut self, screen: &mut Screen, native_output: bool) -> io::Result<()> {
        // Compiler JSON may be emitted in many writes; retain a bounded record.
        // Ordinary partial output (prompts, custom harnesses) is forwarded now.
        if !self.pending.is_empty()
            && (native_output || self.partial || !self.pending.starts_with(b"{"))
        {
            screen.write(&self.pending)?;
            self.pending.clear();
            self.partial = true;
        }
        Ok(())
    }

    fn finish(&mut self, screen: &mut Screen) -> io::Result<()> {
        if !self.pending.is_empty() {
            screen.write(&self.pending)?;
            self.pending.clear();
        }
        Ok(())
    }
}

struct Screen {
    drawn: u16,
}
impl Screen {
    fn new() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut screen = Self { drawn: 0 };
        if let Err(error) = execute!(io::stderr(), cursor::Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }
        screen.clear()?;
        Ok(screen)
    }
    fn width(&self) -> u16 {
        terminal::size()
            .map_or(80, |s| s.0.max(2))
            .saturating_sub(1)
    }
    fn height(&self) -> u16 {
        terminal::size()
            .map_or(24, |s| s.1.max(3))
            .saturating_sub(2)
            .min(27)
    }
    fn clear(&mut self) -> io::Result<()> {
        if self.drawn > 0 {
            write!(io::stderr(), "\r\x1b[{}A\x1b[J", self.drawn)?;
            self.drawn = 0;
        }
        Ok(())
    }
    fn draw(&mut self, block: norimel::Block) -> io::Result<()> {
        self.clear()?;
        let text = block.to_string().replace('\n', "\r\n");
        write!(io::stderr(), "{text}\r\n")?;
        self.drawn = block.size().1;
        io::stderr().flush()
    }
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.clear()?;
        io::stderr().write_all(bytes)?;
        io::stderr().flush()
    }
    // Cargo JSON contains LF-delimited diagnostic text, unlike bytes read
    // from the child PTY. Raw mode requires explicit carriage returns here.
    fn diagnostic(&mut self, text: &str) -> io::Result<()> {
        self.write(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes())
    }
    fn commit(&mut self) {
        self.drawn = 0;
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.clear();
        let _ = execute!(io::stderr(), cursor::Show);
        let _ = terminal::disable_raw_mode();
    }
}
