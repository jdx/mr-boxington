use super::*;

fn write_tree(root: &Path, files: &[(&str, &[u8])]) {
    for (name, contents) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

#[test]
fn identical_trees_at_different_paths_share_one_stable_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("out-dirs");
    let first = directory.path().join("checkout-a/target/build/x/out");
    let second = directory.path().join("checkout-b/target/build/x/out");
    let files: &[(&str, &[u8])] = &[
        ("generated.rs", b"pub const VALUE: u32 = 7;\n"),
        ("nested/header.h", b"#define X 1\n"),
    ];
    write_tree(&first, files);
    write_tree(&second, files);

    let a = stabilize(&first, &root).unwrap().unwrap();
    let b = stabilize(&second, &root).unwrap().unwrap();

    assert_eq!(a, b, "the same bytes name the same directory");
    assert!(a.starts_with(&root));
    assert_eq!(
        std::fs::read(a.join("generated.rs")).unwrap(),
        b"pub const VALUE: u32 = 7;\n"
    );
    assert_eq!(
        std::fs::read(a.join("nested/header.h")).unwrap(),
        b"#define X 1\n"
    );
    assert!(
        std::fs::metadata(a.join("generated.rs"))
            .unwrap()
            .permissions()
            .readonly(),
        "the shared copy is read-only"
    );
    assert!(
        std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)),
        "no staging directory is left behind"
    );
}

#[test]
fn different_bytes_name_different_directories() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("out-dirs");
    let first = directory.path().join("a/out");
    let second = directory.path().join("b/out");
    write_tree(
        &first,
        &[("generated.rs", b"pub const WHERE: &str = \"/a\";\n")],
    );
    write_tree(
        &second,
        &[("generated.rs", b"pub const WHERE: &str = \"/b\";\n")],
    );

    let a = stabilize(&first, &root).unwrap().unwrap();
    let b = stabilize(&second, &root).unwrap().unwrap();

    assert_ne!(a, b);
}

#[test]
fn a_renamed_file_names_a_different_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("out-dirs");
    let first = directory.path().join("a/out");
    let second = directory.path().join("b/out");
    write_tree(&first, &[("one.rs", b"x")]);
    write_tree(&second, &[("two.rs", b"x")]);

    assert_ne!(
        stabilize(&first, &root).unwrap(),
        stabilize(&second, &root).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn the_executable_bit_is_part_of_the_name_and_survives_the_copy() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("out-dirs");
    let plain = directory.path().join("a/out");
    let executable = directory.path().join("b/out");
    write_tree(&plain, &[("tool", b"#!/bin/sh\n")]);
    write_tree(&executable, &[("tool", b"#!/bin/sh\n")]);
    std::fs::set_permissions(
        executable.join("tool"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let a = stabilize(&plain, &root).unwrap().unwrap();
    let b = stabilize(&executable, &root).unwrap().unwrap();

    assert_ne!(a, b);
    assert_eq!(
        std::fs::metadata(b.join("tool"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o555
    );
    assert_eq!(
        std::fs::metadata(a.join("tool"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );
}

#[cfg(unix)]
#[test]
fn a_tree_with_a_symlink_is_left_alone() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("out-dirs");
    let out = directory.path().join("a/out");
    write_tree(&out, &[("generated.rs", b"x")]);
    std::os::unix::fs::symlink("generated.rs", out.join("link.rs")).unwrap();

    assert_eq!(stabilize(&out, &root).unwrap(), None);
    assert!(!root.exists(), "nothing was copied");
}

#[test]
fn the_scan_finds_a_mention_and_skips_what_is_not_source() {
    let directory = tempfile::tempdir().unwrap();
    let src = directory.path().join("src");
    write_tree(&src, &[("lib.rs", b"pub fn value() -> u32 { 1 }\n")]);
    assert!(!sources_mention_out_dir(&src));

    write_tree(
        &src,
        &[(
            "deep/generated.rs",
            b"include!(concat!(env!(\"OUT_DIR\"), \"/x.rs\"));\n",
        )],
    );
    assert!(sources_mention_out_dir(&src));

    // A mention under a target directory or in a non-Rust file is not the
    // crate reading it.
    let other = directory.path().join("other");
    write_tree(
        &other,
        &[
            ("target/debug/build/out/generated.rs", b"OUT_DIR"),
            ("notes.md", b"OUT_DIR"),
            (".hidden/lib.rs", b"OUT_DIR"),
            ("lib.rs", b"nothing"),
        ],
    );
    assert!(!sources_mention_out_dir(&other));
}

#[test]
fn unused_trees_are_collected_by_age_and_used_ones_kept() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("out-dirs");
    let old = directory.path().join("a/out");
    let recent = directory.path().join("b/out");
    write_tree(&old, &[("generated.rs", b"old")]);
    write_tree(&recent, &[("generated.rs", b"recent")]);
    let old = stabilize(&old, &root).unwrap().unwrap();
    let recent = stabilize(&recent, &root).unwrap().unwrap();
    let old_marker = marker_path(&root, &old.file_name().unwrap().to_string_lossy());
    let long_ago = std::time::SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(&old_marker)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(long_ago))
        .unwrap();
    // A staging directory a dead process left is finished off too.
    let staging = root.join(format!("{STAGING_PREFIX}abc-1"));
    std::fs::create_dir(&staging).unwrap();
    std::fs::File::options()
        .read(true)
        .open(&staging)
        .ok()
        .map(|file| file.set_times(std::fs::FileTimes::new().set_modified(long_ago)));

    let dry = collect(&root, Some(Duration::from_secs(24 * 60 * 60)), true).unwrap();
    assert_eq!(dry.removed_directories, 1);
    assert!(old.exists(), "a dry run removes nothing");

    let outcome = collect(&root, Some(Duration::from_secs(24 * 60 * 60)), false).unwrap();

    assert_eq!(outcome.removed_directories, 1);
    assert_eq!(outcome.removed_bytes, 3);
    assert_eq!(outcome.remaining_directories, 1);
    assert!(!old.exists());
    assert!(!old_marker.exists());
    assert!(recent.exists());
    assert_eq!(stats(&root).remaining_directories, 1);
}

#[test]
fn a_use_refreshes_the_marker_at_most_hourly() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("out-dirs");
    let out = directory.path().join("a/out");
    write_tree(&out, &[("generated.rs", b"x")]);
    let stable = stabilize(&out, &root).unwrap().unwrap();
    let marker = marker_path(&root, &stable.file_name().unwrap().to_string_lossy());
    let earlier = std::time::SystemTime::now() - Duration::from_secs(10 * 60);
    std::fs::File::options()
        .write(true)
        .open(&marker)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(earlier))
        .unwrap();

    stabilize(&out, &root).unwrap();
    let within_the_hour = std::fs::metadata(&marker).unwrap().modified().unwrap();
    assert!(
        within_the_hour <= earlier + Duration::from_secs(1),
        "ten minutes old is fresh enough"
    );

    let long_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(&marker)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(long_ago))
        .unwrap();
    stabilize(&out, &root).unwrap();
    assert!(
        std::fs::metadata(&marker).unwrap().modified().unwrap()
            > long_ago + Duration::from_secs(60),
        "two hours old is restamped"
    );
}
