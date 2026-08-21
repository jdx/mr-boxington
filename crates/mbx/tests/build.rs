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
    build_with(project, store, report, &[])
}

/// Build as `build` does, with `settings` added to the environment.
fn build_with(
    project: &Path,
    store: &Path,
    report: &Path,
    settings: &[(&str, &str)],
) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
    command
        .current_dir(project)
        .args(["build", "build", "--offline"])
        .env("MBX_CACHE_DIR", store)
        .env("MBX_STATS_REPORT", report)
        // Cargo's own environment for this test would otherwise redirect the
        // fixture's output into this crate's target directory.
        .env_remove("CARGO_TARGET_DIR")
        // All three decide whether cargo compiles incrementally, so a test that
        // says nothing about it must not inherit an answer from the machine it
        // runs on. CARGO_INCREMENTAL is the one that bites: an enabled build
        // defers to it, and `Swatinem/rust-cache` sets it to 0 for the whole
        // job, so leaving it would make this suite pass locally and fail in CI.
        .env_remove("MBX_INCREMENTAL")
        .env_remove("CARGO_INCREMENTAL")
        .env_remove("CI");
    for (name, value) in settings {
        command.env(name, value);
    }
    let output = command.output().expect("mbx should run");
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
fn incremental_is_opt_in_and_reaches_cargo() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    build(
        project.path(),
        store.path(),
        &reports.path().join("default.json"),
    );
    assert_eq!(
        incremental_sessions(project.path()),
        0,
        "the default build should still force CARGO_INCREMENTAL=0"
    );

    let stats = build_with(
        project.path(),
        store.path(),
        &reports.path().join("incremental.json"),
        &[("MBX_INCREMENTAL", "1")],
    );
    assert!(
        incremental_sessions(project.path()) > 0,
        "cargo should have compiled the member incrementally: {stats}"
    );
}

/// How many incremental sessions rustc left behind. Cargo creates the directory
/// either way, so its contents are the only evidence that anything used it.
fn incremental_sessions(project: &Path) -> usize {
    match std::fs::read_dir(project.join("target/debug/incremental")) {
        Ok(entries) => entries.count(),
        Err(_) => 0,
    }
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

    // Load-bearing, not cleanup: the wipe is what forces the warm build to
    // restore from the store. Left in place, cargo would find the cold build's
    // outputs and the test would pass without exercising the cache at all.
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
    // A hit that never lands on disk is still a broken restore, so check the
    // artifact itself rather than trusting the counter.
    assert!(
        project.path().join("target/debug/libfixture.rlib").exists(),
        "the restored artifact should be materialized: {warm}"
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

/// Write a fixture whose build script generates code, optionally exposing
/// `OUT_DIR` to the compilation.
fn write_generated_project(directory: &Path, consume_out_dir: bool) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n         [lints.rust]\nunexpected_cfgs = { level = \"allow\" }\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("build.rs"),
        "use std::{env, fs, path::PathBuf};\n         fn main() {\n         \u{20}   let out = PathBuf::from(env::var(\"OUT_DIR\").unwrap());\n         \u{20}   fs::write(out.join(\"generated.rs\"), \"pub const VALUE: u32 = 7;\\n\").unwrap();\n         \u{20}   println!(\"cargo:rustc-cfg=generated\");\n         }\n",
    )
    .unwrap();
    let lib = if consume_out_dir {
        "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\npub fn value() -> u32 { VALUE }\n"
    } else {
        "#[cfg(generated)]\npub fn value() -> u32 { 7 }\n"
    };
    std::fs::write(directory.join("src/lib.rs"), lib).unwrap();
    let status = Command::new(cargo())
        .current_dir(directory)
        .args(["generate-lockfile", "--offline"])
        .status()
        .expect("cargo should run");
    assert!(status.success());
}

/// A build script alone does not stop two checkouts sharing a cache. Consuming
/// `OUT_DIR` does, because its value is an absolute path the compilation reads
/// and could bake into the artifact.
#[test]
fn out_dir_decides_whether_two_checkouts_share() {
    for (consume_out_dir, expect_hits) in [(false, true), (true, false)] {
        let store = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_generated_project(first.path(), consume_out_dir);
        write_generated_project(second.path(), consume_out_dir);

        build(
            first.path(),
            store.path(),
            &reports.path().join("first.json"),
        );
        let stats = build(
            second.path(),
            store.path(),
            &reports.path().join("second.json"),
        );

        assert_eq!(
            count(&stats, "hits") > 0,
            expect_hits,
            "consume_out_dir={consume_out_dir} gave {stats}"
        );
    }
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
