use super::*;
use std::path::PathBuf;

const VIEW: &str = "0123456789abcdef";

#[test]
fn a_path_inside_a_view_names_that_view() {
    let root = Path::new("/cache/targets/v1");
    let name = |path: &str| view_name(root, Path::new(path));

    assert_eq!(
        name(&format!("/cache/targets/v1/{VIEW}/debug/app")),
        Some(VIEW.into())
    );
    assert_eq!(
        name(&format!("/cache/targets/v1/{VIEW}")),
        Some(VIEW.into()),
        "a process holding the directory itself open"
    );
    assert_eq!(
        name(&format!("/cache/targets/v1/{VIEW}/debug/app (deleted)")),
        Some(VIEW.into()),
        "a binary a rebuild replaced while it ran"
    );
    assert_eq!(name(&format!("/cache/targets/v1/{VIEW}.lock")), None);
    assert_eq!(
        name(&format!(
            "/cache/targets/v1/{VIEW}{}1-x/debug/app",
            crate::target::REMOVAL_SUFFIX
        )),
        None,
        "a view already moved aside"
    );
    assert_eq!(name("/cache/targets/v1"), None);
    assert_eq!(name("/usr/bin/sleep"), None);
}

/// A view under a fresh root with one file in it, the root given the way
/// collection gives it.
fn view_with_file() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("v1");
    let view = root.join(VIEW);
    std::fs::create_dir_all(&view).unwrap();
    let file = view.join("artifact");
    std::fs::write(&file, b"output").unwrap();
    (directory, root, view)
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_this_process_has_open_marks_its_view() {
    let (_directory, root, view) = view_with_file();
    let _open = std::fs::File::open(view.join("artifact")).unwrap();

    let found = scan(&root);

    assert!(found.holds(&view));
    assert!(!found.holds(&root.join("fedcba9876543210")));
}

#[cfg(target_os = "linux")]
#[test]
fn a_link_in_the_root_path_is_followed() {
    let (directory, root, view) = view_with_file();
    let link = directory.path().join("linked");
    std::os::unix::fs::symlink(directory.path(), &link).unwrap();
    let _open = std::fs::File::open(view.join("artifact")).unwrap();

    assert!(scan(&link.join("v1")).holds(&link.join("v1").join(VIEW)));
    assert!(scan(&root).holds(&view));
}

/// The scan must never stand in collection's way: a process table it cannot
/// read, a walk that runs out of time, or a root that does not exist all
/// leave nothing in use, so collection goes on as if the scan had not run.
#[test]
fn a_scan_that_cannot_finish_finds_nothing_in_use() {
    let (directory, root, view) = view_with_file();
    let _open = std::fs::File::open(view.join("artifact")).unwrap();
    let later = Instant::now() + SCAN_LIMIT;

    let unreadable = scan_by(&directory.path().join("no-proc"), &root, later);
    let out_of_time = scan_by(Path::new(PROC_ROOT), &root, Instant::now());
    let no_root = scan_by(
        Path::new(PROC_ROOT),
        &directory.path().join("missing"),
        later,
    );

    for found in [unreadable, out_of_time, no_root] {
        assert!(!found.holds(&view), "{found:?}");
    }
}

/// `/proc/<pid>` entries this user does not own are passed over, and the
/// numbered entries the walk reads but finds unreadable are skipped without
/// ending it.
#[cfg(target_os = "linux")]
#[test]
fn unreadable_processes_are_skipped() {
    let (directory, root, view) = view_with_file();
    let proc_root = directory.path().join("proc");
    // A process whose details are gone, as for one that exited mid-walk.
    std::fs::create_dir_all(proc_root.join("1")).unwrap();
    // One that holds a file in the view.
    let open = proc_root.join("2").join("fd");
    std::fs::create_dir_all(&open).unwrap();
    std::os::unix::fs::symlink(view.join("artifact"), open.join("3")).unwrap();
    // Not a process at all.
    std::fs::create_dir_all(proc_root.join("self-ish")).unwrap();

    let found = scan_by(&proc_root, &root, Instant::now() + SCAN_LIMIT);

    assert!(found.holds(&view));
}

/// A library mapped from a view, such as a proc-macro an editor's
/// rust-analyzer loaded, marks it like an open file does. Lines without a
/// path, and paths elsewhere, do not.
#[cfg(target_os = "linux")]
#[test]
fn a_mapped_file_marks_its_view() {
    let (directory, root, view) = view_with_file();
    let other = root.join("fedcba9876543210");
    let process = directory.path().join("proc").join("7");
    std::fs::create_dir_all(&process).unwrap();
    let library = view.join("debug/deps/libmacro.so");
    std::fs::write(
        process.join("maps"),
        format!(
            "55d0c0000000-55d0c0001000 r--p 00000000 08:01 1 /usr/lib/libc.so.6\n\
             7f0000000000-7f0000001000 rw-p 00000000 00:00 0 \n\
             7f0000002000-7f0000003000 r-xp 00001000 08:01 2 {}\n\
             7fff00000000-7fff00001000 rw-p 00000000 00:00 0 [stack]",
            library.display()
        ),
    )
    .unwrap();

    let found = scan_by(
        &directory.path().join("proc"),
        &root,
        Instant::now() + SCAN_LIMIT,
    );

    assert!(found.holds(&view));
    assert!(!found.holds(&other));
}
