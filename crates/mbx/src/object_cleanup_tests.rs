use super::*;

/// Windows keeps no loose object files, and no change time to compare.
#[cfg(unix)]
mod superseded_objects {
    use super::*;
    use std::os::unix::process::ExitStatusExt as _;

    const UNIT: &str = "widget-0123456789abcdef";

    fn unit_arguments(out_dir: &Path, extra: &[&str]) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = vec![
            "--crate-name".into(),
            "widget".into(),
            "--edition=2021".into(),
            "src/main.rs".into(),
            "-C".into(),
            "extra-filename=-0123456789abcdef".into(),
            "--out-dir".into(),
            out_dir.into(),
            "-C".into(),
            "split-debuginfo=unpacked".into(),
        ];
        arguments.extend(extra.iter().map(OsString::from));
        arguments
    }

    fn write(directory: &Path, names: &[&str]) {
        for name in names {
            std::fs::write(directory.join(name), name.as_bytes()).unwrap();
        }
    }

    fn succeeded() -> std::process::ExitStatus {
        std::process::ExitStatus::from_raw(0)
    }

    /// A snapshot as if the compilation started well after the files in
    /// `directory` were written, which a test cannot backdate: a change time
    /// cannot be set.
    fn later_snapshot(directory: &Path) -> ObjectSnapshot {
        ObjectSnapshot::take(
            directory,
            &format!("{UNIT}."),
            SystemTime::now() + Duration::from_secs(10),
        )
    }

    #[test]
    fn a_successful_compilation_removes_the_objects_an_earlier_one_left() {
        let deps = tempfile::tempdir().unwrap();
        let old = [
            "widget-0123456789abcdef.widget.aaaa-cgu.0.rcgu.o",
            "widget-0123456789abcdef.widget.aaaa-cgu.1.rcgu.o",
        ];
        let others = [
            "gadget-0123456789abcdef.gadget.aaaa-cgu.0.rcgu.o",
            "widget-fedcba9876543210.widget.aaaa-cgu.0.rcgu.o",
            "widget-0123456789abcdef.d",
            "libwidget-0123456789abcdef.rlib",
            "widget-0123456789abcdef",
        ];
        write(deps.path(), &old);
        write(deps.path(), &others);
        let snapshot = later_snapshot(deps.path());
        // What the compilation itself writes is never in the snapshot.
        let new = ["widget-0123456789abcdef.widget.bbbb-cgu.0.rcgu.o"];
        write(deps.path(), &new);

        assert_eq!(snapshot.remove_superseded(&succeeded()), 2);

        for name in old {
            assert!(!deps.path().join(name).exists(), "{name} was superseded");
        }
        for name in others.iter().chain(&new) {
            assert!(deps.path().join(name).exists(), "{name} is kept");
        }
    }

    #[test]
    fn objects_written_just_before_the_compilation_are_kept() {
        let deps = tempfile::tempdir().unwrap();
        let recent = "widget-0123456789abcdef.widget.aaaa-cgu.0.rcgu.o";
        write(deps.path(), &[recent]);
        let snapshot = ObjectSnapshot::take(deps.path(), &format!("{UNIT}."), SystemTime::now());

        assert_eq!(snapshot.remove_superseded(&succeeded()), 0);
        assert!(deps.path().join(recent).exists());
    }

    #[test]
    fn objects_written_well_before_the_compilation_are_removed() {
        let deps = tempfile::tempdir().unwrap();
        let old = "widget-0123456789abcdef.widget.aaaa-cgu.0.rcgu.o";
        write(deps.path(), &[old]);
        std::thread::sleep(Duration::from_millis(2100));
        let snapshot =
            ObjectSnapshot::before_compiling(&unit_arguments(deps.path(), &[]), Path::new("/"))
                .expect("the unit is identified");

        assert_eq!(snapshot.remove_superseded(&succeeded()), 1);
        assert!(!deps.path().join(old).exists());
    }

    #[test]
    fn a_failed_compilation_removes_nothing() {
        let deps = tempfile::tempdir().unwrap();
        let old = "widget-0123456789abcdef.widget.aaaa-cgu.0.rcgu.o";
        write(deps.path(), &[old]);
        let snapshot = later_snapshot(deps.path());

        let failed = std::process::ExitStatus::from_raw(1 << 8);
        assert_eq!(snapshot.remove_superseded(&failed), 0);
        assert!(deps.path().join(old).exists());
    }

    #[test]
    fn an_object_the_compilation_replaced_or_linked_is_kept() {
        let deps = tempfile::tempdir().unwrap();
        let replaced = "widget-0123456789abcdef.widget.aaaa-cgu.0.rcgu.o";
        let relinked = "widget-0123456789abcdef.widget.aaaa-cgu.1.rcgu.o";
        let rewritten = "widget-0123456789abcdef.widget.aaaa-cgu.2.rcgu.o";
        write(deps.path(), &[replaced, relinked, rewritten]);
        let snapshot = later_snapshot(deps.path());
        std::thread::sleep(Duration::from_millis(20));
        // A new file at the same name: another inode.
        std::fs::remove_file(deps.path().join(replaced)).unwrap();
        write(deps.path(), &[replaced]);
        // A reused codegen unit linked into the incremental session: same
        // inode, later change time.
        std::fs::hard_link(
            deps.path().join(relinked),
            deps.path().join("incremental-copy.o"),
        )
        .unwrap();
        // Rewritten in place at the same length: later change time.
        std::fs::write(deps.path().join(rewritten), rewritten.to_uppercase()).unwrap();

        assert_eq!(snapshot.remove_superseded(&succeeded()), 0);
        for name in [replaced, relinked, rewritten] {
            assert!(deps.path().join(name).exists(), "{name} is kept");
        }
    }

    #[test]
    fn an_older_name_of_an_object_the_compilation_linked_anew_is_removed() {
        let deps = tempfile::tempdir().unwrap();
        let reused = "widget-0123456789abcdef.widget.aaaa-cgu.0.rcgu.o";
        let kept = "widget-0123456789abcdef.widget.aaaa-cgu.1.rcgu.o";
        write(deps.path(), &[reused, kept]);
        // Recorded just now, so the age rule alone would keep both.
        let snapshot = ObjectSnapshot::take(deps.path(), &format!("{UNIT}."), SystemTime::now());
        std::thread::sleep(Duration::from_millis(20));
        // The compilation reuses one object under a new name of this unit.
        let renamed = "widget-0123456789abcdef.widget.bbbb-cgu.0.rcgu.o";
        std::fs::hard_link(deps.path().join(reused), deps.path().join(renamed)).unwrap();
        // The other is linked from elsewhere only: its change time moves, and
        // no name the compilation created reaches it.
        std::fs::hard_link(
            deps.path().join(kept),
            deps.path().join("incremental-session.o"),
        )
        .unwrap();

        assert_eq!(snapshot.remove_superseded(&succeeded()), 1);
        assert!(!deps.path().join(reused).exists(), "an older name goes");
        assert!(deps.path().join(renamed).is_file(), "the file stays");
        assert!(deps.path().join(kept).is_file());
    }

    #[test]
    fn save_temps_keeps_every_object() {
        let deps = tempfile::tempdir().unwrap();
        for flag in [&["-C", "save-temps"][..], &["-Csave-temps=yes"]] {
            assert!(
                ObjectSnapshot::before_compiling(
                    &unit_arguments(deps.path(), flag),
                    Path::new("/")
                )
                .is_none(),
                "{flag:?}"
            );
        }
        assert!(
            ObjectSnapshot::before_compiling(
                &unit_arguments(deps.path(), &["-Csave-temps=no"]),
                Path::new("/")
            )
            .is_some()
        );
    }

    #[test]
    fn a_unit_without_its_own_hash_is_not_cleaned() {
        let deps = tempfile::tempdir().unwrap();
        let arguments: Vec<OsString> = vec![
            "--crate-name".into(),
            "widget".into(),
            "--out-dir".into(),
            deps.path().into(),
        ];
        assert!(ObjectSnapshot::before_compiling(&arguments, Path::new("/")).is_none());
        let custom = unit_arguments(deps.path(), &["-C", "extra-filename=custom"]);
        assert!(ObjectSnapshot::before_compiling(&custom, Path::new("/")).is_none());
    }

    /// From Cargo 1.100 a unit owns `build/<package>/<hash>/out` and is passed
    /// no `extra-filename`, so its objects carry only the crate name.
    #[test]
    fn a_unit_that_owns_its_output_directory_is_cleaned_by_crate_name() {
        let target = tempfile::tempdir().unwrap();
        let out = target
            .path()
            .join("debug/build/widget/0123456789abcdef/out");
        std::fs::create_dir_all(&out).unwrap();
        let old = ["widget.aaaa.1x2y3z.rcgu.o", "widget.bbbb.1x2y3z.rcgu.o"];
        let others = ["widget", "widget.d", "gadget.aaaa.1x2y3z.rcgu.o"];
        write(&out, &old);
        write(&out, &others);
        let arguments: Vec<OsString> = vec![
            "--crate-name".into(),
            "widget".into(),
            "--out-dir".into(),
            out.clone().into(),
            "-C".into(),
            "split-debuginfo=unpacked".into(),
        ];
        let (directory, prefix) = unit_objects(&arguments, Path::new("/")).unwrap();
        assert_eq!(
            (directory.as_path(), prefix.as_str()),
            (out.as_path(), "widget.")
        );
        let snapshot = ObjectSnapshot::take(
            &directory,
            &prefix,
            SystemTime::now() + Duration::from_secs(10),
        );
        let new = ["widget.aaaa.4u5v6w.rcgu.o"];
        write(&out, &new);

        assert_eq!(snapshot.remove_superseded(&succeeded()), 2);

        for name in old {
            assert!(!out.join(name).exists(), "{name} was superseded");
        }
        for name in others.iter().chain(&new) {
            assert!(out.join(name).exists(), "{name} is kept");
        }
    }

    #[test]
    fn a_hashless_unit_outside_a_directory_of_its_own_is_not_cleaned() {
        let target = tempfile::tempdir().unwrap();
        for directory in [
            "debug/build/widget/not-a-hash/out",
            "debug/build/widget/0123456789abcdef/other",
            "debug/elsewhere/widget/0123456789abcdef/out",
        ] {
            let out = target.path().join(directory);
            let arguments: Vec<OsString> = vec![
                "--crate-name".into(),
                "widget".into(),
                "--out-dir".into(),
                out.into(),
                "-C".into(),
                "split-debuginfo=unpacked".into(),
            ];
            assert!(
                unit_objects(&arguments, Path::new("/")).is_none(),
                "{directory}"
            );
        }
    }
}

#[test]
fn only_a_compilation_that_can_leave_objects_is_snapshotted() {
    let deps = tempfile::tempdir().unwrap();
    let snapshot = |extra: &[&str]| {
        let mut arguments: Vec<OsString> = vec![
            "--crate-name".into(),
            "widget".into(),
            "-C".into(),
            "extra-filename=-0123456789abcdef".into(),
            "--out-dir".into(),
            deps.path().into(),
        ];
        arguments.extend(extra.iter().map(OsString::from));
        ObjectSnapshot::before_compiling(&arguments, Path::new("/")).is_some()
    };
    let unpacked = "-Csplit-debuginfo=unpacked";

    assert!(snapshot(&[unpacked]));
    assert!(snapshot(&["--emit=dep-info,link", unpacked]));
    assert!(snapshot(&["--emit", "obj=widget.o", unpacked]));
    assert!(
        !snapshot(&["--emit=dep-info,metadata", unpacked]),
        "a check writes no objects"
    );
    assert!(!snapshot(&["-C", "split-debuginfo=packed"]));
    assert!(!snapshot(&["-Csplit-debuginfo=off"]));
    assert_eq!(
        snapshot(&[]),
        cfg!(target_os = "macos"),
        "unpacked is the default only for Apple targets"
    );
    assert!(!snapshot(&["--target", "x86_64-unknown-linux-gnu"]));
}
