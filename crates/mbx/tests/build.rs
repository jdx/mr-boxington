//! End-to-end coverage for cached cargo commands.
//!
//! Each test drives the real binary over a throwaway project with no
//! dependencies, so nothing here needs the network.

use std::path::Path;
use std::process::Command;

fn write_project(directory: &Path) {
    write_named_project(directory, "fixture");
}

/// Write the fixture under `name`.
///
/// The name reaches the lockfile, and the lockfile is what the build identity
/// is keyed on, so two differently named fixtures are two different identities
/// -- which is what it takes to tell one checkout's artifacts from another's.
fn write_named_project(directory: &Path, name: &str) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
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

/// Write a workspace whose dependency builds before a member that fails.
fn write_partially_failing_project(directory: &Path) {
    std::fs::create_dir_all(directory.join("good/src")).unwrap();
    std::fs::create_dir_all(directory.join("bad/src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nmembers = [\"good\", \"bad\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("good/Cargo.toml"),
        "[package]\nname = \"good\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(directory.join("good/src/lib.rs"), "pub fn good() {}\n").unwrap();
    std::fs::write(
        directory.join("bad/Cargo.toml"),
        "[package]\nname = \"bad\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ngood = { path = \"../good\" }\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("bad/src/lib.rs"),
        "use good as _;\ncompile_error!(\"expected failure\");\n",
    )
    .unwrap();
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
    build_with(project, store, report, &[]).0
}

/// Build as `build` does, with `settings` added to the environment.
///
/// Returns what the build said on stderr alongside its statistics, because some
/// of what mbx reports -- a sweep, for one -- is only ever said there.
fn build_with(
    project: &Path,
    store: &Path,
    report: &Path,
    settings: &[(&str, &str)],
) -> (serde_json::Value, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
    command
        .current_dir(project)
        .args(["build", "--offline"])
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
    let stats = std::fs::read(report).expect("a statistics report should be written");
    (
        serde_json::from_slice(&stats).expect("the report should be JSON"),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Run `mbx` against `store` and return its stdout.
fn mbx(store: &Path, arguments: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .args(arguments)
        .env("MBX_CACHE_DIR", store)
        .output()
        .expect("mbx should run");
    assert!(
        output.status.success(),
        "{arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The bytes `mbx gc` weighs against its budget.
fn store_bytes(store: &Path) -> u64 {
    tree_bytes(&store.join("actions/cas")) + tree_bytes(&store.join("actions/action-results"))
}

/// Total size of every file under `directory`.
fn tree_bytes(directory: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(listing) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in listing.flatten() {
            let metadata = entry.metadata().expect("the entry should be readable");
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total += metadata.len();
            }
        }
    }
    total
}

fn count(stats: &serde_json::Value, field: &str) -> u64 {
    stats[field]
        .as_u64()
        .unwrap_or_else(|| panic!("{field} should be a number"))
}

/// Remove the outputs behind a target link so the next build must restore.
fn wipe_target(project: &Path) {
    let target = project.join("target");
    let outputs = std::fs::read_link(&target).unwrap_or(target);
    std::fs::remove_dir_all(outputs).unwrap();
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

    let (stats, _) = build_with(
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

#[test]
fn workspace_policy_reaches_cargo() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());
    std::fs::write(project.path().join(".mbx.toml"), "incremental = true\n").unwrap();

    let stats = build(
        project.path(),
        store.path(),
        &reports.path().join("workspace-policy.json"),
    );

    assert!(
        incremental_sessions(project.path()) > 0,
        "the checked-in workspace policy should reach cargo: {stats}"
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
    assert!(
        cold["compiler"]["unconsulted"]["duration_ns"]
            .as_u64()
            .is_some_and(|duration| duration > 0),
        "the cold build should report time spent compiling: {cold}"
    );
    assert!(
        cold["slow_compilations"]
            .as_array()
            .is_some_and(|crates| !crates.is_empty()),
        "the cold build should identify its slowest uncached crates: {cold}"
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
    wipe_target(project.path());

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
    assert!(
        count(&warm, "estimated_compiler_duration_avoided_ns") > 0,
        "a warm build should report the compiler time recorded by its cold build: {warm}"
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
    let (stats, _) = build_with(
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

#[test]
fn forwards_non_build_cargo_subcommands() {
    let root = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .current_dir(root.path())
        .args(["new", "--vcs", "none", "new-project"])
        .env("MBX_CACHE_DIR", store.path())
        .env("MBX_GC_AUTO", "0")
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("mbx new should run");
    assert!(
        output.status.success(),
        "mbx new failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.path().join("new-project/Cargo.toml").is_file());

    let initialized = root.path().join("initialized-project");
    std::fs::create_dir(&initialized).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .current_dir(&initialized)
        .args(["init", "--vcs", "none"])
        .env("MBX_CACHE_DIR", store.path())
        .env("MBX_GC_AUTO", "0")
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("mbx init should run");
    assert!(
        output.status.success(),
        "mbx init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(initialized.join("Cargo.toml").is_file());
}

#[test]
fn a_build_records_the_checkout_it_ran_in() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );

    // The record is what later tells the collector this checkout exists, so a
    // build that leaves none has silently opted out of being protected.
    let records = store.path().join("actions/checkouts/v1");
    assert!(
        tree_bytes(&records) > 0,
        "the build should record its checkout under {}",
        records.display()
    );
}

#[test]
fn a_failed_build_records_the_compilations_it_completed() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_partially_failing_project(project.path());

    let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .current_dir(project.path())
        .args(["build", "--workspace", "--offline"])
        .env("MBX_CACHE_DIR", store.path())
        .env("MBX_GC_AUTO", "0")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_INCREMENTAL")
        .output()
        .expect("mbx should run");

    assert!(!output.status.success(), "the bad member should fail");
    assert!(
        tree_bytes(&store.path().join("actions/task-manifests/v1")) > 0,
        "the successful dependency should still be recorded as reachable"
    );
}

#[test]
fn a_build_sweeps_the_store_to_its_budget() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    let (_, stderr) = build_with(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
        // A one-byte budget swept every build: nothing this build stored can
        // stay, so the sweep is unambiguous.
        &[("MBX_GC_MAX_SIZE", "1"), ("MBX_GC_INTERVAL", "0")],
    );

    assert!(
        stderr.contains("gc: evicted"),
        "the sweep should say what it evicted: {stderr}"
    );
    let stats = mbx(store.path(), &["cache", "stats"]);
    assert!(
        stats.contains("objects: 0"),
        "the store should be swept empty: {stats}"
    );
}

#[test]
fn automatic_sweeps_can_be_turned_off() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    let (_, stderr) = build_with(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
        &[
            ("MBX_GC_MAX_SIZE", "1"),
            ("MBX_GC_INTERVAL", "0"),
            ("MBX_GC_AUTO", "0"),
        ],
    );

    assert!(!stderr.contains("gc:"), "no sweep should run: {stderr}");
    let stats = mbx(store.path(), &["cache", "stats"]);
    assert!(
        !stats.contains("objects: 0"),
        "the store should be left alone: {stats}"
    );
}

/// Two checkouts, one deleted, a budget that fits only one of them.
///
/// Plain recency would keep the deleted checkout's newer artifacts and evict
/// the surviving one's, so the survivor coming back warm is the whole claim.
#[test]
fn deleting_a_checkout_releases_what_only_it_used() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let surviving = tempfile::tempdir().unwrap();
    let deleted = tempfile::tempdir().unwrap();
    write_named_project(surviving.path(), "surviving");
    write_named_project(deleted.path(), "deleted");

    build(
        surviving.path(),
        store.path(),
        &reports.path().join("surviving.json"),
    );
    // Budget the store as it stood with one checkout in it, plus slack. The
    // slack matters: an action result is only dropped once its objects are
    // gone, which happens after eviction has already decided how deep to go, so
    // a budget set to the exact byte forces eviction one object further than
    // the arithmetic suggests. At a real budget that rounding is noise; at this
    // scale it is the whole margin.
    let budget = store_bytes(store.path()) + 4096;
    build(
        deleted.path(),
        store.path(),
        &reports.path().join("deleted.json"),
    );
    std::fs::remove_dir_all(deleted.path()).unwrap();

    mbx(store.path(), &["gc", "--max-size", &budget.to_string()]);

    // Load-bearing, not cleanup: the wipe is what forces the next build to go
    // to the store, which is the only way to see what survived the sweep.
    wipe_target(surviving.path());
    let warm = build(
        surviving.path(),
        store.path(),
        &reports.path().join("warm.json"),
    );

    assert!(
        count(&warm, "hits") > 0,
        "the surviving checkout should still be warm: {warm}"
    );
}

/// Managed target directories rest on a symlink standing in for `target`, and
/// Windows only lets a privileged or developer-mode process create one, so mbx
/// leaves the target directory where cargo put it there. These cover the
/// platforms where the feature is available.
#[cfg(unix)]
mod target_views {
    use super::*;

    fn managed(project: &Path) -> std::path::PathBuf {
        std::fs::read_link(project.join("target")).expect("target should be a link")
    }

    #[test]
    fn a_managed_target_directory_keeps_the_workspace_clean() {
        let store = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_project(project.path());

        build(
            project.path(),
            store.path(),
            &reports.path().join("cold.json"),
        );

        let directory = managed(project.path());
        assert!(
            directory.starts_with(store.path().join("targets")),
            "outputs should land under the managed root, not the workspace: {}",
            directory.display()
        );
        // The link is the point: a relocation that breaks the paths people type
        // would not be worth the disk it reclaims.
        assert!(
            project.path().join("target/debug/libfixture.rlib").exists(),
            "the workspace should still reach its own build outputs"
        );
    }

    #[test]
    fn a_managed_target_directory_still_hits_the_cache() {
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
        let (warm, _) = build_with(
            second.path(),
            store.path(),
            &reports.path().join("second.json"),
            &[("MBX_TARGET_VIEWS", "1")],
        );

        // The shim maps the target directory out of its keys before anything
        // else, so moving it must not cost a single hit.
        assert!(
            count(&warm, "hits") > 0,
            "a relocated target directory should still reuse the first build: {warm}"
        );
    }

    #[test]
    fn deleting_a_checkout_frees_its_managed_target_directory() {
        let store = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_project(project.path());

        build_with(
            project.path(),
            store.path(),
            &reports.path().join("cold.json"),
            &[("MBX_TARGET_VIEWS", "1")],
        );
        let directory = managed(project.path());
        assert!(tree_bytes(&directory) > 0);

        // Outputs used to live inside the checkout and die with it. Now that
        // they outlive it, collecting them is mbx's job.
        std::fs::remove_dir_all(project.path()).unwrap();
        let output = mbx(store.path(), &["gc", "--max-size", "20GiB"]);

        assert!(
            !directory.exists(),
            "the target directory of a checkout that is gone should be freed"
        );
        assert!(
            output.contains("freed 1 target directories"),
            "gc should say what it freed: {output}"
        );
    }

    #[test]
    fn a_store_sweep_failure_still_frees_managed_target_directories() {
        let store = tempfile::tempdir().unwrap();
        let gone = tempfile::tempdir().unwrap();
        let current = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_named_project(gone.path(), "gone");
        write_named_project(current.path(), "current");

        build_with(
            gone.path(),
            store.path(),
            &reports.path().join("gone.json"),
            &[("MBX_TARGET_VIEWS", "1")],
        );
        let directory = managed(gone.path());
        std::fs::remove_dir_all(gone.path()).unwrap();

        // Make the due store sweep fail after it writes its throttle stamp.
        // Target collection is independent and must not be skipped with it.
        let cas = store.path().join("actions/cas/v1");
        std::fs::remove_dir_all(&cas).unwrap();
        std::fs::write(&cas, b"not a directory").unwrap();
        let (_, stderr) = build_with(
            current.path(),
            store.path(),
            &reports.path().join("current.json"),
            &[("MBX_TARGET_VIEWS", "1"), ("MBX_GC_INTERVAL", "0")],
        );

        assert!(stderr.contains("the store was not swept"), "{stderr}");
        assert!(
            !directory.exists(),
            "target collection should still run when store collection fails"
        );
    }

    #[test]
    fn explicit_gc_still_frees_targets_when_store_collection_fails() {
        let store = tempfile::tempdir().unwrap();
        let gone = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_project(gone.path());

        build_with(
            gone.path(),
            store.path(),
            &reports.path().join("gone.json"),
            &[("MBX_TARGET_VIEWS", "1")],
        );
        let directory = managed(gone.path());
        std::fs::remove_dir_all(gone.path()).unwrap();
        let cas = store.path().join("actions/cas/v1");
        std::fs::remove_dir_all(&cas).unwrap();
        std::fs::write(&cas, b"not a directory").unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
            .args(["gc", "--max-size", "20GiB"])
            .env("MBX_CACHE_DIR", store.path())
            .output()
            .expect("mbx should run");

        assert!(
            !output.status.success(),
            "the broken store should be reported"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("freed 1 target directories"),
            "successful target collection should still be reported"
        );
        assert!(
            !directory.exists(),
            "explicit gc should collect targets independently of the store"
        );
    }

    #[test]
    fn a_noninteractive_build_never_removes_a_real_target_directory() {
        let store = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_project(project.path());
        // Establish real outputs without a prompt. The next command captures
        // its stdio, so it is non-interactive and must preserve them too.
        build_with(
            project.path(),
            store.path(),
            &reports.path().join("cold.json"),
            &[("MBX_TARGET_VIEWS", "0")],
        );
        assert!(project.path().join("target").is_dir());

        build(
            project.path(),
            store.path(),
            &reports.path().join("warm.json"),
        );

        assert!(
            std::fs::read_link(project.path().join("target")).is_err(),
            "existing build outputs are not ours to move"
        );
        assert!(project.path().join("target/debug/libfixture.rlib").exists());
    }
}
