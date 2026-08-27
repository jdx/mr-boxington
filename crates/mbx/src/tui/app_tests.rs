use super::*;
use crate::events::{ActionDetail, EventWriter};

fn writer(store: &Path) -> EventWriter {
    let writer = EventWriter::new(store);
    writer.started(Path::new("/checkouts/fixture"), &["build".into()]);
    writer
}

#[test]
fn a_session_takes_its_name_and_counts_from_its_stream() {
    let store = tempfile::tempdir().unwrap();
    let build = writer(store.path());
    build.action(
        ActionOutcome::Hit,
        Some("serde".into()),
        10,
        ActionDetail {
            avoided_compiler_ns: 5_000,
            output_bytes: 128,
            ..ActionDetail::default()
        },
    );
    build.action(
        ActionOutcome::Miss,
        Some("mbx".into()),
        20,
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

    let session = app.selected_session().expect("the build should be listed");
    assert_eq!(session.title(), "mbx build");
    assert_eq!(session.workspace_name(), Some("fixture"));
    assert_eq!(session.count("hit"), 1);
    assert_eq!(session.count("miss"), 1);
    assert_eq!(session.count("incremental"), 1);
    assert_eq!(session.avoided_compiler_ns, 5_000);
    assert_eq!(session.restored_bytes, 128);
    assert_eq!(session.rows.len(), 3);
    assert_eq!(session.bypasses(), vec![("incremental", 1)]);
    assert_eq!(session.state, SessionState::Live);
}

#[test]
fn the_hit_rate_counts_only_attempted_lookups() {
    let store = tempfile::tempdir().unwrap();
    let build = writer(store.path());
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    build.action(ActionOutcome::Miss, None, 1, ActionDetail::default());
    // Neither of these was ever looked up, so neither can move a hit rate.
    build.action(ActionOutcome::Unconsulted, None, 1, ActionDetail::default());
    build.action(
        ActionOutcome::Bypass {
            reason: "native-library".into(),
        },
        None,
        0,
        ActionDetail::default(),
    );

    let mut app = App::new(store.path(), 10);
    app.tick(10);

    let session = app.selected_session().unwrap();
    assert_eq!(session.hit_rate(), Some(75.0));
}

#[test]
fn a_build_with_no_lookups_reports_no_hit_rate() {
    let store = tempfile::tempdir().unwrap();
    let build = writer(store.path());
    build.action(ActionOutcome::Unconsulted, None, 1, ActionDetail::default());

    let mut app = App::new(store.path(), 10);
    app.tick(10);

    // Zero of zero is not zero percent: a cold build looked nothing up, and
    // "0%" would read as a cache that failed rather than one never asked.
    assert_eq!(app.selected_session().unwrap().hit_rate(), None);
}

#[test]
fn a_finished_session_carries_the_totals_its_stream_ended_with() {
    let store = tempfile::tempdir().unwrap();
    let build = writer(store.path());
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    build.finished(serde_json::json!({ "hits": 41, "misses": 1 }));

    let mut app = App::new(store.path(), 10);
    app.tick(10);

    let session = app.selected_session().unwrap();
    assert_eq!(session.state, SessionState::Finished);
    let totals = session.totals.as_ref().expect("totals should be recorded");
    assert_eq!(crate::events::stat(totals, "hits"), 41);
}

#[test]
fn a_running_build_sorts_above_a_finished_one() {
    let store = tempfile::tempdir().unwrap();
    let done = writer(store.path());
    done.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    done.finished(serde_json::json!({ "hits": 1 }));
    drop(done);
    let running = writer(store.path());
    running.action(ActionOutcome::Miss, None, 1, ActionDetail::default());

    let mut app = App::new(store.path(), 10);
    app.tick(10);

    let states: Vec<SessionState> = app.sessions().map(|session| session.state).collect();
    assert_eq!(
        states,
        vec![SessionState::Live, SessionState::Finished],
        "the build somebody is watching belongs at the top"
    );
}

#[test]
fn a_build_that_starts_later_is_picked_up_without_a_restart() {
    let store = tempfile::tempdir().unwrap();
    let mut app = App::new(store.path(), 10);
    app.tick(10);
    assert!(app.is_empty());

    let build = writer(store.path());
    build.action(
        ActionOutcome::Hit,
        Some("serde".into()),
        1,
        ActionDetail::default(),
    );
    app.tick(10);

    assert_eq!(app.sessions().count(), 1);
    assert_eq!(app.selected_session().unwrap().count("hit"), 1);
}

#[test]
fn pausing_stops_reading_and_resuming_catches_up() {
    let store = tempfile::tempdir().unwrap();
    let build = writer(store.path());
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    let mut app = App::new(store.path(), 10);
    app.tick(10);
    assert_eq!(app.selected_session().unwrap().count("hit"), 1);

    app.toggle_pause();
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    app.tick(10);
    assert_eq!(
        app.selected_session().unwrap().count("hit"),
        1,
        "a paused dashboard should not move"
    );

    app.toggle_pause();
    app.tick(10);
    assert_eq!(
        app.selected_session().unwrap().count("hit"),
        2,
        "resuming should pick up what was appended while paused"
    );
}

#[test]
fn a_capped_stream_says_so() {
    let store = tempfile::tempdir().unwrap();
    let build = crate::events::EventWriter::with_cap_for_test(store.path(), 1);
    build.started(Path::new("/checkouts/fixture"), &["build".into()]);
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());

    let mut app = App::new(store.path(), 10);
    app.tick(10);

    assert!(app.selected_session().unwrap().truncated);
}

#[test]
fn tabs_cycle_and_can_be_jumped_to() {
    let store = tempfile::tempdir().unwrap();
    let mut app = App::new(store.path(), 10);
    assert_eq!(app.tab, Tab::Live);

    app.next_tab();
    assert_eq!(app.tab, Tab::Sessions);
    app.next_tab();
    assert_eq!(app.tab, Tab::Store);
    app.next_tab();
    assert_eq!(app.tab, Tab::Live);

    app.select_tab(Tab::Store);
    assert_eq!(app.tab, Tab::Store);
}

#[test]
fn selection_stays_inside_the_list() {
    let store = tempfile::tempdir().unwrap();
    let build = writer(store.path());
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    let mut app = App::new(store.path(), 10);
    app.tick(10);

    // One session: moving either way keeps the selection on it rather than
    // running off the end.
    app.select_next();
    assert_eq!(app.selected(), 0);
    app.select_previous();
    assert_eq!(app.selected(), 0);
    assert!(app.selected_session().is_some());
}

#[test]
fn the_watched_build_stays_watched_when_another_one_starts() {
    let store = tempfile::tempdir().unwrap();
    let first = writer(store.path());
    first.action(
        ActionOutcome::Hit,
        Some("first".into()),
        1,
        ActionDetail::default(),
    );
    // Finished, so a build that starts later sorts above it.
    first.finished(serde_json::json!({ "hits": 1 }));
    drop(first);

    let mut app = App::new(store.path(), 10);
    app.tick(10);
    app.select_next();
    let watched = app.selected_session().unwrap().id.clone();

    // Another build starts and takes the top of the list.
    let second = writer(store.path());
    second.action(
        ActionOutcome::Miss,
        Some("second".into()),
        1,
        ActionDetail::default(),
    );
    app.tick(10);

    assert_eq!(
        app.selected_session().unwrap().id,
        watched,
        "a build starting elsewhere must not steal the selection"
    );
}

#[test]
fn a_collected_stream_is_dropped_rather_than_followed() {
    let store = tempfile::tempdir().unwrap();
    let build = writer(store.path());
    build.action(ActionOutcome::Hit, None, 1, ActionDetail::default());
    build.finished(serde_json::json!({ "hits": 1 }));
    let paths = crate::events::session_paths(store.path(), build.id());
    drop(build);

    let mut app = App::new(store.path(), 10);
    app.tick(10);
    assert_eq!(app.sessions().count(), 1);

    // Collection removes the stream while the dashboard is watching it.
    std::fs::remove_file(&paths.events).unwrap();
    std::fs::remove_file(&paths.lock).unwrap();
    app.tick(10);

    assert!(app.is_empty(), "a stream that is gone should be dropped");
    // The probe must not put the lock back, or collection and the dashboard
    // would undo each other on every tick.
    assert!(!paths.lock.exists(), "probing must not recreate the lock");
}

#[test]
fn the_build_window_scrolls_to_keep_the_selection_visible() {
    // Shorter than the pane: never scrolled.
    assert_eq!(window_start(3, 0, 5), 0);
    assert_eq!(window_start(3, 2, 5), 0);

    // Longer than the pane: the top holds until the selection reaches the
    // bottom row, then follows it.
    assert_eq!(window_start(10, 0, 4), 0);
    assert_eq!(window_start(10, 3, 4), 0);
    assert_eq!(window_start(10, 4, 4), 1);
    assert_eq!(window_start(10, 9, 4), 6);

    // The last window is full rather than running past the end.
    assert_eq!(window_start(10, 9, 4) + 4, 10);

    // Degenerate panes do not panic.
    assert_eq!(window_start(10, 5, 0), 0);
    assert_eq!(window_start(0, 0, 4), 0);
}
