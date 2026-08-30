use super::*;

#[test]
fn cache_removal_uses_the_workspace_identity_reported_by_cargo() {
    let directory = tempfile::tempdir().unwrap();
    let requested = directory.path().join("workspace-alias");
    std::fs::create_dir_all(&requested).unwrap();
    std::fs::write(
        requested.join("Cargo.toml"),
        "[package]\nname = \"cache-remove-root\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    std::fs::create_dir(requested.join("src")).unwrap();
    std::fs::write(requested.join("src/lib.rs"), "").unwrap();

    let reported = cache_workspace_root(std::ffi::OsStr::new("cargo"), &requested);

    let metadata = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(requested.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(metadata.status.success());
    assert_eq!(
        reported,
        parse_cargo_roots(&metadata.stdout).unwrap().workspace_root
    );
}

#[test]
fn cache_removal_preserves_the_requested_spelling_when_cargo_cannot_resolve_it() {
    let requested = PathBuf::from("/workspace/that/does/not/exist");

    assert_eq!(
        cache_workspace_root(std::ffi::OsStr::new("definitely-not-cargo"), &requested),
        requested
    );
}
