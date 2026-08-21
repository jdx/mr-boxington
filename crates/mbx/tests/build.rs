//! End-to-end coverage for `mbx build`.
//!
//! Each test drives the real binary over a throwaway project with no
//! dependencies, so nothing here needs the network.

use std::path::Path;
use std::process::Command;

fn write_project(directory: &Path) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("src/lib.rs"),
        "pub fn double(value: u32) -> u32 {\n    value * 2\n}\n",
    )
    .unwrap();
    // Manifest identity follows the lockfile, and cargo would otherwise write
    // it during the first build -- changing the identity between two runs that
    // are supposed to share a manifest.
    let status = Command::new(cargo())
        .current_dir(directory)
        .args(["generate-lockfile", "--offline"])
        .status()
        .expect("cargo should run");
    assert!(status.success(), "the fixture should resolve offline");
}

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Build `project` against `store`, returning the run's statistics.
fn build(project: &Path, store: &Path, report: &Path) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .current_dir(project)
        .args(["build", "build", "--offline"])
        .env("MBX_CACHE_DIR", store)
        .env("MBX_STATS_REPORT", report)
        // Cargo's own environment for this test would otherwise redirect the
        // fixture's output into this crate's target directory.
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("mbx should run");
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = std::fs::read(report).expect("a statistics report should be written");
    serde_json::from_slice(&report).expect("the report should be JSON")
}

fn count(stats: &serde_json::Value, field: &str) -> u64 {
    stats[field]
        .as_u64()
        .unwrap_or_else(|| panic!("{field} should be a number"))
}

#[test]
fn restores_a_wiped_target_directory_from_the_store() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    let cold = build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    assert_eq!(count(&cold, "hits"), 0, "a cold build cannot hit");
    assert!(
        count(&cold, "stored_bytes") > 0,
        "a cold build should publish its outputs: {cold}"
    );

    std::fs::remove_dir_all(project.path().join("target")).unwrap();

    let warm = build(
        project.path(),
        store.path(),
        &reports.path().join("warm.json"),
    );
    assert!(
        count(&warm, "hits") > 0,
        "the rebuilt target directory should be restored: {warm}"
    );
    assert_eq!(
        count(&warm, "compiler_invocations_avoided"),
        count(&warm, "hits")
    );
}

#[test]
fn a_second_checkout_starts_warm() {
    let store = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(first.path());
    write_project(second.path());

    build(
        first.path(),
        store.path(),
        &reports.path().join("first.json"),
    );
    let warm = build(
        second.path(),
        store.path(),
        &reports.path().join("second.json"),
    );

    assert!(
        count(&warm, "hits") > 0,
        "a checkout at another path should reuse the first build: {warm}"
    );
}

#[test]
fn an_empty_store_reports_nothing() {
    let store = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .args(["cache", "stats"])
        .env("MBX_CACHE_DIR", store.path())
        .output()
        .expect("mbx should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("objects: 0"), "unexpected output: {stdout}");
}
