//! Render tests over an in-memory terminal.
//!
//! The dashboard is drawn cell by cell, so the only honest way to assert what a
//! reader sees is to draw a frame and read the buffer back.

use super::app::App;
use crate::events::{ActionDetail, ActionOutcome, EventWriter};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::Path;

/// The drawn frame as lines of text, trailing blanks trimmed.
fn render(app: &App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| super::ui::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// Write `count` finished streams with ids that increase by a whole second.
///
/// Deliberately not through [`EventWriter`]: its ids carry the current
/// millisecond, so a loop that creates several inside one millisecond leaves
/// them differing only by their random suffix -- ordered arbitrarily, which is
/// fine for real builds and useless for asserting which row is on top.
fn seed(store: &Path, count: usize) {
    let directory = store.join("sessions/v1");
    std::fs::create_dir_all(&directory).unwrap();
    for index in 0..count {
        let id = format!(
            "{:013}-1-s{index:03}",
            1_700_000_000_000u64 + index as u64 * 1000
        );
        let lines = [
            serde_json::json!({
                "type": "session_started",
                "v": 1,
                "ts_ms": 1,
                "session": id,
                "pid": 1,
                "mbx_version": "0.0.0",
                "workspace_root": format!("/checkouts/ws{index}"),
                "command": ["build", format!("--job{index}")],
            }),
            serde_json::json!({
                "type": "action",
                "v": 1,
                "ts_ms": 2,
                "outcome": {"kind": "hit"},
                "crate_name": format!("crate{index}"),
                "duration_ns": 1,
            }),
            serde_json::json!({
                "type": "session_finished",
                "v": 1,
                "ts_ms": 3,
                "stats": {"hits": 1},
            }),
        ];
        let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(directory.join(format!("{id}.jsonl")), body).unwrap();
    }
}

#[test]
fn an_empty_store_says_where_builds_will_appear() {
    let store = tempfile::tempdir().unwrap();
    let app = App::new(store.path(), 10);

    let frame = render(&app, 100, 20).join("\n");

    assert!(frame.contains("No builds recorded yet"), "{frame}");
}

#[test]
fn a_build_is_drawn_with_its_command_crate_and_outcome() {
    let store = tempfile::tempdir().unwrap();
    let build = EventWriter::new(store.path());
    build.started(Path::new("/checkouts/fixture"), &["test".into()]);
    build.action(
        ActionOutcome::Hit,
        Some("serde".into()),
        1,
        ActionDetail::default(),
    );
    build.action(
        ActionOutcome::Bypass {
            reason: "incremental".into(),
        },
        None,
        0,
        ActionDetail::default(),
    );
    let mut app = App::new(store.path(), 10);
    app.tick(10);

    let frame = render(&app, 110, 24).join("\n");

    assert!(frame.contains("mbx test"), "the command: {frame}");
    assert!(frame.contains("fixture"), "the workspace: {frame}");
    assert!(frame.contains("live"), "the state: {frame}");
    assert!(frame.contains("serde"), "the crate a hit restored: {frame}");
    assert!(frame.contains("incremental"), "the bypass reason: {frame}");
}

#[test]
fn the_selected_build_stays_on_screen_when_the_list_is_longer_than_the_pane() {
    let store = tempfile::tempdir().unwrap();
    // More builds than the eight-row builds pane can show.
    seed(store.path(), 9);
    let mut app = App::new(store.path(), 20);
    app.tick(20);

    // Nothing chosen: the newest build is at the top and is the marked one.
    let frame = render(&app, 110, 30).join("\n");
    assert!(frame.contains("▸"), "the selection marker: {frame}");
    assert!(frame.contains("--job8"), "the newest build: {frame}");

    // Walk to the oldest build, well past the bottom of the pane.
    for _ in 0..8 {
        app.select_next();
    }
    let frame = render(&app, 110, 30);
    let joined = frame.join("\n");
    assert!(
        joined.contains("--job0"),
        "the window should have scrolled to the selected build: {joined}"
    );
    // The marker must be on a line that is actually drawn.
    let marked: Vec<&String> = frame.iter().filter(|line| line.contains("▸")).collect();
    assert_eq!(marked.len(), 1, "exactly one row is marked: {joined}");
    assert!(
        marked[0].contains("--job0"),
        "the marked row is the selected build: {joined}"
    );
    // And the count says where in the list the reader is.
    assert!(
        joined.contains("9 of 9") || joined.contains("(9 of 9)"),
        "the pane should say which of how many is selected: {joined}"
    );
}

#[test]
fn a_narrow_terminal_still_draws_without_panicking() {
    let store = tempfile::tempdir().unwrap();
    seed(store.path(), 3);
    let mut app = App::new(store.path(), 10);
    app.tick(10);

    // Small enough that every pane is squeezed; ratatui panics on out-of-bounds
    // writes, so drawing at all is the assertion.
    for (width, height) in [(20, 6), (40, 10), (200, 60)] {
        let frame = render(&app, width, height);
        assert_eq!(frame.len(), height as usize);
    }
}

#[test]
fn each_screen_draws_its_own_content() {
    let store = tempfile::tempdir().unwrap();
    seed(store.path(), 2);
    let mut app = App::new(store.path(), 10);
    app.tick(10);

    app.select_tab(super::app::Tab::Sessions);
    let sessions = render(&app, 110, 24).join("\n");
    assert!(sessions.contains("command"), "{sessions}");
    assert!(sessions.contains("finished"), "{sessions}");

    app.select_tab(super::app::Tab::Store);
    let store_tab = render(&app, 110, 24).join("\n");
    assert!(store_tab.contains("action results"), "{store_tab}");
}
