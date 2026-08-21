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
        .env_remove("CI")
        // Same reason: a test asserting the default cross-checkout behaviour
        // must not read an answer out of the developer's environment.
        .env_remove("MBX_SHARE_OUT_DIR");
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
    // A cold target directory leaves nothing to derive an action key from, so
    // these compilations run without the cache ever being consulted. Reported
    // apart from misses: counting them as zero of both says the cache was asked
    // and found nothing, which is not what happened.
    assert_eq!(
        count(&cold, "misses"),
        0,
        "a cold build looks nothing up, so it cannot miss: {cold}"
    );
    assert!(
        count(&cold, "unconsulted") > 0,
        "a cold build should report the compilations it had no key for: {cold}"
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

/// How a fixture's library uses its build script's output.
#[derive(Clone, Copy)]
enum Generated {
    /// A cfg only, so the compilation never reads `OUT_DIR`.
    Cfg,
    /// Includes the generated file. `OUT_DIR` becomes an input, but only rustc
    /// records the path, so `--remap-path-prefix` can take it back out.
    Include,
    /// Keeps `OUT_DIR` in a string constant. That lands in the artifact itself,
    /// where no remapping reaches it.
    Text,
}

/// Write a fixture whose build script generates code, used as `generated` says.
fn write_generated_project(directory: &Path, generated: Generated) {
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
    let lib = match generated {
        Generated::Cfg => "#[cfg(generated)]\npub fn value() -> u32 { 7 }\n".to_string(),
        Generated::Include => {
            "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\npub fn value() -> u32 { VALUE }\n"
                .to_string()
        }
        Generated::Text => {
            "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n\
             pub const WHERE: &str = env!(\"OUT_DIR\");\n\
             pub fn value() -> u32 { VALUE }\n"
                .to_string()
        }
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
    for (generated, expect_hits) in [(Generated::Cfg, true), (Generated::Include, false)] {
        assert_eq!(
            two_checkouts_share(generated, &[]),
            expect_hits,
            "the default should not share a compilation that reads OUT_DIR"
        );
    }
}

/// With sharing on, the compilation is remapped so rustc records the
/// placeholder instead of the real `OUT_DIR`, and the include-only shape crosses
/// checkouts. The shape that keeps the value in a string still does not: the
/// remapping cannot reach into the artifact, and mbx reads the outputs rather
/// than assuming it can.
///
/// The pair is the test. Either half alone would pass for the wrong reason.
#[test]
fn sharing_out_dir_crosses_checkouts_only_where_the_artifact_allows_it() {
    let sharing = [("MBX_SHARE_OUT_DIR", "1")];
    for (generated, expect_hits) in [(Generated::Include, true), (Generated::Text, false)] {
        assert_eq!(
            two_checkouts_share(generated, &sharing),
            expect_hits,
            "sharing changed the wrong shape"
        );
    }
}

/// Build the same fixture in two checkouts, reporting whether the second one
/// reused anything from the first.
fn two_checkouts_share(generated: Generated, settings: &[(&str, &str)]) -> bool {
    let store = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_generated_project(first.path(), generated);
    write_generated_project(second.path(), generated);

    build_with(
        first.path(),
        store.path(),
        &reports.path().join("first.json"),
        settings,
    );
    let stats = build_with(
        second.path(),
        store.path(),
        &reports.path().join("second.json"),
        settings,
    );
    count(&stats, "hits") > 0
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
