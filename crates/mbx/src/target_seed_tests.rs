use super::*;
use std::time::SystemTime;

const LOCKFILE: &str = r#"
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = ["serde", "local"]

[[package]]
name = "local"
version = "0.1.0"

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"

[[package]]
name = "shared-name"
version = "0.1.0"

[[package]]
name = "shared-name"
version = "2.0.0"
source = "git+https://example.com/shared?rev=abc#abc"
"#;

#[test]
fn registry_packages_leave_out_every_name_a_path_package_uses() {
    assert_eq!(
        registry_packages(LOCKFILE),
        BTreeSet::from(["serde".to_owned()])
    );
    assert!(registry_packages("not toml [").is_empty());
}

#[test]
fn profile_directories_follow_the_profile_and_targets() {
    let arguments = |words: &[&str]| {
        words
            .iter()
            .map(|word| word.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        profile_directories(&arguments(&["build"])),
        [PathBuf::from("debug")]
    );
    assert_eq!(
        profile_directories(&arguments(&["test"])),
        [PathBuf::from("debug")]
    );
    assert_eq!(
        profile_directories(&arguments(&["bench"])),
        [PathBuf::from("release")]
    );
    assert_eq!(
        profile_directories(&arguments(&["build", "--profile", "fast"])),
        [PathBuf::from("fast")]
    );
    assert_eq!(
        profile_directories(&arguments(&[
            "build",
            "--release",
            "--target",
            "aarch64-unknown-linux-gnu",
            "--target=/specs/custom.json",
        ])),
        [
            PathBuf::from("release"),
            Path::new("aarch64-unknown-linux-gnu").join("release"),
            Path::new("custom").join("release"),
        ]
    );
}

/// Write a Cargo 1.100 unit with a fingerprint, a writable dep-info file,
/// and a read-only rlib like one restored from the store.
fn unit(profile: &Path, package: &str, hash: &str) -> PathBuf {
    let unit = profile.join("build").join(package).join(hash);
    std::fs::create_dir_all(unit.join("fingerprint")).unwrap();
    std::fs::create_dir_all(unit.join("out")).unwrap();
    std::fs::write(unit.join("fingerprint/lib-x"), b"0123456789abcdef").unwrap();
    std::fs::write(unit.join("out/x.d"), b"dep-info").unwrap();
    let rlib = unit.join("out/libx.rlib");
    std::fs::write(&rlib, b"rlib").unwrap();
    let mut permissions = std::fs::metadata(&rlib).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&rlib, permissions).unwrap();
    unit
}

fn donor(directory: &Path) -> Donor {
    Donor {
        directory: directory.to_path_buf(),
        workspace_root: PathBuf::from("/checkouts/donor"),
        updated_secs: SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    }
}

fn registry() -> BTreeSet<String> {
    BTreeSet::from(["serde".to_owned()])
}

#[test]
fn copies_registry_units_and_keeps_their_times() {
    let from = tempfile::tempdir().unwrap();
    let to = tempfile::tempdir().unwrap();
    let profile = from.path().join("debug");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join(".cargo-lock"), b"").unwrap();
    let serde = unit(&profile, "serde", "0123456789abcdef");
    unit(&profile, "local", "fedcba9876543210");
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    std::fs::File::options()
        .write(true)
        .open(serde.join("out/x.d"))
        .unwrap()
        .set_modified(modified)
        .unwrap();

    let outcome = seed(
        to.path(),
        &[PathBuf::from("debug")],
        &registry(),
        &[donor(from.path())],
    );

    assert_eq!(outcome.units, 1);
    assert_eq!(outcome.donor, Some(PathBuf::from("/checkouts/donor")));
    let copied = to.path().join("debug/build/serde/0123456789abcdef");
    assert_eq!(
        std::fs::read(copied.join("fingerprint/lib-x")).unwrap(),
        b"0123456789abcdef"
    );
    assert_eq!(
        std::fs::metadata(copied.join("out/x.d"))
            .unwrap()
            .modified()
            .unwrap(),
        modified,
        "Cargo compares a unit's times with its dependencies'"
    );
    assert!(
        !to.path().join("debug/build/local").exists(),
        "a path package's unit is never copied"
    );
    assert_eq!(
        std::fs::read_dir(to.path().join("debug/build/serde"))
            .unwrap()
            .count(),
        1,
        "nothing staged is left behind"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let inode = |path: &Path| std::fs::metadata(path).unwrap().ino();
        assert_eq!(
            inode(&copied.join("out/libx.rlib")),
            inode(&serde.join("out/libx.rlib")),
            "a read-only store object is shared"
        );
        assert_ne!(
            inode(&copied.join("out/x.d")),
            inode(&serde.join("out/x.d")),
            "a writable file is copied, so a later build cannot change the donor's"
        );
    }
}

#[test]
fn a_profile_this_checkout_has_built_is_left_alone() {
    let from = tempfile::tempdir().unwrap();
    let to = tempfile::tempdir().unwrap();
    let profile = from.path().join("debug");
    std::fs::create_dir_all(&profile).unwrap();
    unit(&profile, "serde", "0123456789abcdef");
    std::fs::create_dir_all(to.path().join("debug/build")).unwrap();

    let outcome = seed(
        to.path(),
        &[PathBuf::from("debug")],
        &registry(),
        &[donor(from.path())],
    );

    assert_eq!(outcome, SeedOutcome::default());
    assert!(!to.path().join("debug/build/serde").exists());
}

#[test]
fn a_donor_without_cargo_1_100_units_is_skipped_for_the_next() {
    let old = tempfile::tempdir().unwrap();
    let new = tempfile::tempdir().unwrap();
    let to = tempfile::tempdir().unwrap();
    // A pre-1.100 profile: units are spread across deps/ and .fingerprint/.
    std::fs::create_dir_all(old.path().join("debug/build/serde-0123456789abcdef")).unwrap();
    std::fs::create_dir_all(old.path().join("debug/.fingerprint/serde-0123456789abcdef")).unwrap();
    let profile = new.path().join("debug");
    unit(&profile, "serde", "0123456789abcdef");

    let outcome = seed(
        to.path(),
        &[PathBuf::from("debug")],
        &registry(),
        &[donor(old.path()), donor(new.path())],
    );

    assert_eq!(outcome.units, 1);
    assert!(
        to.path()
            .join("debug/build/serde/0123456789abcdef")
            .is_dir()
    );
}

#[test]
fn a_donor_profile_a_build_is_using_is_skipped() {
    let from = tempfile::tempdir().unwrap();
    let to = tempfile::tempdir().unwrap();
    let profile = from.path().join("debug");
    unit(&profile, "serde", "0123456789abcdef");
    let mut building = fslock::LockFile::open(&profile.join(".cargo-lock")).unwrap();
    assert!(building.try_lock().unwrap());

    let outcome = seed(
        to.path(),
        &[PathBuf::from("debug")],
        &registry(),
        &[donor(from.path())],
    );

    assert_eq!(outcome, SeedOutcome::default());
    assert!(!to.path().join("debug/build").exists());
}

#[test]
fn the_pinned_build_script_binary_comes_along() {
    let from = tempfile::tempdir().unwrap();
    let to = tempfile::tempdir().unwrap();
    let profile = from.path().join("debug");
    unit(&profile, "serde", "0123456789abcdef");
    let pinned = profile.join(".mbx-build-script-shims/identity/mbx");
    std::fs::create_dir_all(pinned.parent().unwrap()).unwrap();
    std::fs::write(&pinned, b"mbx").unwrap();

    seed(
        to.path(),
        &[PathBuf::from("debug")],
        &registry(),
        &[donor(from.path())],
    );

    assert_eq!(
        std::fs::read(to.path().join("debug/.mbx-build-script-shims/identity/mbx")).unwrap(),
        b"mbx"
    );
}

#[test]
fn only_units_the_donors_last_build_read_are_copied() {
    let from = tempfile::tempdir().unwrap();
    let to = tempfile::tempdir().unwrap();
    if !crate::target_units::access_times_tracked(from.path()) {
        return;
    }
    let profile = from.path().join("debug");
    unit(&profile, "serde", "0123456789abcdef");
    let stale = unit(&profile, "serde", "fedcba9876543210");
    let old = SystemTime::now() - Duration::from_secs(10 * 24 * 60 * 60);
    for entry in std::fs::read_dir(stale.join("fingerprint"))
        .unwrap()
        .flatten()
    {
        std::fs::File::options()
            .write(true)
            .open(entry.path())
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(old)
                    .set_modified(old),
            )
            .unwrap();
    }
    std::fs::File::open(stale.join("fingerprint"))
        .unwrap()
        .set_modified(old)
        .unwrap();

    let outcome = seed(
        to.path(),
        &[PathBuf::from("debug")],
        &registry(),
        &[donor(from.path())],
    );

    assert_eq!(outcome.units, 1);
    assert!(
        to.path()
            .join("debug/build/serde/0123456789abcdef")
            .is_dir()
    );
    assert!(
        !to.path()
            .join("debug/build/serde/fedcba9876543210")
            .exists()
    );
}
