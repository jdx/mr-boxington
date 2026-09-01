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

/// Write a workspace where one member depends on another.
fn write_dependent_project(directory: &Path) {
    std::fs::create_dir_all(directory.join("base/src")).unwrap();
    std::fs::create_dir_all(directory.join("above/src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[workspace]\nmembers = [\"base\", \"above\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("README.md"),
        "shared workspace documentation\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("base/Cargo.toml"),
        "[package]\nname = \"base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("base/src/lib.rs"),
        "#![doc = include_str!(\"../../README.md\")]\npub fn value() -> u32 { 0 }\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("above/Cargo.toml"),
        "[package]\nname = \"above\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbase = { path = \"../base\" }\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("above/src/lib.rs"),
        "pub fn doubled() -> u32 { base::value() * 2 }\n",
    )
    .unwrap();
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

#[cfg(unix)]
#[test]
fn transparent_rustc_replaces_the_shim_process() {
    let directory = tempfile::tempdir().unwrap();
    let shim = mbx::session::install_shim(
        Path::new(env!("CARGO_BIN_EXE_mbx")),
        directory.path(),
        mbx::session::ShimLink::Tracking,
    )
    .unwrap();
    let pid_file = directory.path().join("compiler.pid");

    // Retried because exec of the binary behind the shim can transiently fail
    // with ETXTBSY: anyone holding it open for write blocks the exec, and a
    // sibling test that forks while cargo is writing it counts, since until
    // that child reaches its own exec the inherited descriptor (cloexec or not)
    // is a writer of this file too.
    let mut child = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let attempt = Command::new(&shim)
                .arg("/bin/sh")
                .args(["-c", "printf '%s' \"$$\" > \"$1\"", "sh"])
                .arg(&pid_file)
                .spawn();
            match attempt {
                Err(error)
                    if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                other => break other.unwrap(),
            }
        }
    };
    let shim_pid = child.id();
    let status = child.wait().unwrap();

    assert!(status.success());
    assert_eq!(
        std::fs::read_to_string(pid_file).unwrap(),
        shim_pid.to_string()
    );
}

#[cfg(unix)]
#[test]
fn rustc_workspace_wrapper_is_preserved_without_becoming_the_compiler() {
    use std::os::unix::fs::PermissionsExt as _;

    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_project(project.path());

    let wrapper = project.path().join("workspace-rustc");
    let log = project.path().join("workspace-rustc.log");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\nprintf 'called\\n' >> \"$MBX_TEST_WORKSPACE_WRAPPER_LOG\"\nexec \"$@\"\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&wrapper, permissions).unwrap();

    let (stats, stderr) = build_with(
        project.path(),
        store.path(),
        &reports.path().join("stats.json"),
        &[
            ("RUSTC_WORKSPACE_WRAPPER", wrapper.to_str().unwrap()),
            ("MBX_TEST_WORKSPACE_WRAPPER_LOG", log.to_str().unwrap()),
        ],
    );

    assert!(
        std::fs::read_to_string(log).unwrap().contains("called"),
        "Cargo should still invoke the configured workspace wrapper"
    );
    assert!(
        stderr.contains("RUSTC_WORKSPACE_WRAPPER is already set"),
        "the uncached workspace compilation should be disclosed: {stderr}"
    );
    assert!(
        stats["bypasses"].get("multiple-inputs").is_none(),
        "the workspace wrapper must not be parsed as rustc: {stats}"
    );
}

/// Build `project` against `store`, returning the run's statistics.
fn build(project: &Path, store: &Path, report: &Path) -> serde_json::Value {
    build_with(project, store, report, &[]).0
}

fn document(project: &Path, store: &Path, report: &Path) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .current_dir(project)
        .args(["doc", "--offline", "--no-deps"])
        .env("MBX_CACHE_DIR", store)
        .env("MBX_STATS_REPORT", report)
        .env_remove("MBX_SOCKET")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_TARGET_DIR")
        // This suite may itself be run through mbx. The fixture needs a fresh
        // session rather than chaining the outer test build's compiler shim.
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .output()
        .expect("mbx should run");
    assert!(
        output.status.success(),
        "documentation failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap()
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
    cargo_with(project, store, report, &["build", "--offline"], settings)
}

fn cargo_with(
    project: &Path,
    store: &Path,
    report: &Path,
    arguments: &[&str],
    settings: &[(&str, &str)],
) -> (serde_json::Value, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
    command
        .current_dir(project)
        .args(arguments)
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
        .env_remove("MBX_RELEASE")
        // Same reason: a test asserting the default cross-checkout behaviour
        // must not read an answer out of the developer's environment.
        .env_remove("MBX_SHARE_OUT_DIR")
        .env_remove("MBX_BUILD_SCRIPT_EXECUTION")
        .env_remove("MBX_LEARNED_INCREMENTAL")
        // Native links are cached by default and several counts here include
        // one, so an inherited answer would decide them.
        .env_remove("MBX_CACHE_LINKS")
        // An action may group every build in the surrounding job. Individual
        // tests opt into that explicitly rather than leaking into its group.
        .env_remove(mbx::session::CACHE_EXPORT_GROUP_ENV)
        // The C shims are on by default; a test that says nothing about them
        // must not inherit a different answer from the developer's shell. The
        // compiler variables matter just as much: this suite is itself run
        // through mbx, so without this a fixture would inherit the outer
        // session's shims and every build here would stand aside.
        .env_remove("MBX_CC")
        .env_remove("CC")
        .env_remove("CXX")
        .env_remove("HOST_CC")
        .env_remove("HOST_CXX")
        .env_remove("TARGET_CC")
        .env_remove("TARGET_CXX")
        .env_remove("MBX_REAL_CC")
        .env_remove("MBX_REAL_CXX")
        .env_remove("MBX_SOCKET")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CLIPPY_CONF_DIR");
    for (name, value) in settings {
        command.env(name, value);
    }
    let output = command.output().expect("mbx should run");
    assert!(
        output.status.success(),
        "build failed ({}): {}",
        output.status,
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
        "{arguments:?} failed ({}): {}",
        output.status,
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

#[test]
fn clippy_workspace_compilations_restore_and_track_clippy_toml() {
    if !Command::new(cargo())
        .args(["clippy", "--version"])
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let appearance_store = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let changed = tempfile::tempdir().unwrap();
    let changed_flags = tempfile::tempdir().unwrap();
    let without_config = tempfile::tempdir().unwrap();
    let added_config = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    for project in [
        &first,
        &second,
        &changed,
        &changed_flags,
        &without_config,
        &added_config,
    ] {
        write_project(project.path());
    }
    for project in [&first, &second, &changed_flags] {
        std::fs::write(
            project.path().join("clippy.toml"),
            "too-many-arguments-threshold = 7\n",
        )
        .unwrap();
    }
    std::fs::write(
        changed.path().join("clippy.toml"),
        "too-many-arguments-threshold = 8\n",
    )
    .unwrap();
    std::fs::write(
        added_config.path().join("clippy.toml"),
        "too-many-arguments-threshold = 7\n",
    )
    .unwrap();

    let (cold, cold_stderr) = cargo_with(
        first.path(),
        store.path(),
        &reports.path().join("clippy-cold.json"),
        &["clippy", "--offline"],
        &[],
    );
    assert_eq!(count(&cold, "hits"), 0, "a cold clippy run cannot hit");
    assert!(
        cold["bypasses"].get("multiple-inputs").is_none(),
        "the real rustc path must not be parsed as a source: {cold}"
    );
    assert!(
        count(&cold, "stored_bytes") > 0,
        "the workspace compilation should be stored: {cold}\n{cold_stderr}"
    );

    let warm = cargo_with(
        second.path(),
        store.path(),
        &reports.path().join("clippy-warm.json"),
        &["clippy", "--offline"],
        &[],
    )
    .0;
    assert!(count(&warm, "hits") > 0, "clippy should restore: {warm}");

    let changed = cargo_with(
        changed.path(),
        store.path(),
        &reports.path().join("clippy-changed.json"),
        &["clippy", "--offline"],
        &[],
    )
    .0;
    assert!(
        count(&changed, "misses") > 0,
        "changed clippy.toml must miss: {changed}"
    );

    let changed_flags = cargo_with(
        changed_flags.path(),
        store.path(),
        &reports.path().join("clippy-changed-flags.json"),
        &["clippy", "--offline", "--", "-D", "warnings"],
        &[],
    )
    .0;
    assert!(
        count(&changed_flags, "misses") > 0,
        "changed CLIPPY_ARGS must miss: {changed_flags}"
    );

    let without_config = cargo_with(
        without_config.path(),
        appearance_store.path(),
        &reports.path().join("clippy-without-config.json"),
        &["clippy", "--offline"],
        &[],
    )
    .0;
    assert!(
        count(&without_config, "stored_bytes") > 0,
        "the no-config action should be stored: {without_config}"
    );
    let added_config = cargo_with(
        added_config.path(),
        appearance_store.path(),
        &reports.path().join("clippy-added-config.json"),
        &["clippy", "--offline"],
        &[],
    )
    .0;
    assert!(
        count(&added_config, "misses") > 0,
        "adding clippy.toml must miss: {added_config}"
    );
}

#[test]
fn rustdoc_pages_restore_and_rebuild_the_shared_index() {
    let store = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let changed = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_dependent_project(first.path());
    write_dependent_project(second.path());
    write_dependent_project(changed.path());

    let cold = document(
        first.path(),
        store.path(),
        &reports.path().join("cold-doc.json"),
    );
    assert!(
        count(&cold, "misses") >= 2,
        "both crates should render: {cold}"
    );
    let warm = document(
        second.path(),
        store.path(),
        &reports.path().join("warm-doc.json"),
    );

    assert!(count(&warm, "hits") >= 2, "rustdoc should restore: {warm}");
    assert!(second.path().join("target/doc/base/index.html").is_file());
    assert!(second.path().join("target/doc/above/index.html").is_file());
    let index = std::fs::read_to_string(second.path().join("target/doc/crates.js")).unwrap();
    assert!(
        index.contains("base") && index.contains("above"),
        "the finalized index should name both crates"
    );

    std::fs::write(
        changed.path().join("README.md"),
        "changed workspace documentation\n",
    )
    .unwrap();
    let changed_stats = document(
        changed.path(),
        store.path(),
        &reports.path().join("changed-doc.json"),
    );
    assert!(
        count(&changed_stats, "misses") >= 2,
        "a workspace-level doc input should invalidate the cached pages: {changed_stats}"
    );
    let changed_page =
        std::fs::read_to_string(changed.path().join("target/doc/base/index.html")).unwrap();
    assert!(changed_page.contains("changed workspace documentation"));
}

#[cfg(unix)]
#[test]
fn a_failed_mergeable_render_is_not_run_again_transparently() {
    use std::os::unix::fs::PermissionsExt as _;

    let project = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write_project(project.path());
    let wrapper = project.path().join("count-rustdoc");
    let count = project.path().join("rustdoc-count");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\nif [ \"$1\" = \"-Vv\" ]; then exec \"$TEST_REAL_RUSTDOC\" \"$@\"; fi\nprintf x >> \"$TEST_RUSTDOC_COUNT\"\nprintf 'one rustdoc failure\\n' >&2\nexit 7\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&wrapper, permissions).unwrap();
    let rustdoc = which::which("rustdoc").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .current_dir(project.path())
        .args(["doc", "--offline", "--no-deps"])
        .env("MBX_CACHE_DIR", store.path())
        .env("RUSTDOC", &wrapper)
        .env("TEST_REAL_RUSTDOC", rustdoc)
        .env("TEST_RUSTDOC_COUNT", &count)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(std::fs::read(&count).unwrap(), b"x");
}

/// Remove the outputs behind a target link so the next build must restore.
fn wipe_target(project: &Path) {
    let target = project.join("target");
    let outputs = std::fs::read_link(&target).unwrap_or(target);
    std::fs::remove_dir_all(outputs).unwrap();
}

fn corrupt_action_results(store: &Path) -> usize {
    let mut corrupted = 0;
    let mut pending = vec![store.join("actions/action-results")];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(entry.path());
            } else if std::fs::write(entry.path(), b"not json").is_ok() {
                corrupted += 1;
            }
        }
    }
    corrupted
}

#[test]
fn a_ci_export_group_collects_every_build_in_the_job() {
    let source_store = tempfile::tempdir().unwrap();
    let destination_store = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_named_project(first.path(), "first-fixture");
    write_named_project(second.path(), "second-fixture");
    let group = "github-run-42/test-linux";
    build_with(
        first.path(),
        source_store.path(),
        &reports.path().join("first.json"),
        &[
            (mbx::session::CACHE_EXPORT_GROUP_ENV, group),
            ("MBX_LEARNED_INCREMENTAL", "0"),
        ],
    );
    build_with(
        second.path(),
        source_store.path(),
        &reports.path().join("second.json"),
        &[
            (mbx::session::CACHE_EXPORT_GROUP_ENV, group),
            ("MBX_LEARNED_INCREMENTAL", "0"),
        ],
    );
    wipe_target(first.path());
    wipe_target(second.path());
    let warm_group = "github-run-43/test-linux";
    let first_warm = build_with(
        first.path(),
        source_store.path(),
        &reports.path().join("first-warm.json"),
        &[
            (mbx::session::CACHE_EXPORT_GROUP_ENV, warm_group),
            ("MBX_LEARNED_INCREMENTAL", "0"),
        ],
    )
    .0;
    edit_project(first.path(), 1);
    let first_changed = build_with(
        first.path(),
        source_store.path(),
        &reports.path().join("first-changed.json"),
        &[
            (mbx::session::CACHE_EXPORT_GROUP_ENV, warm_group),
            ("MBX_LEARNED_INCREMENTAL", "0"),
        ],
    )
    .0;
    let second_warm = build_with(
        second.path(),
        source_store.path(),
        &reports.path().join("second-warm.json"),
        &[
            (mbx::session::CACHE_EXPORT_GROUP_ENV, warm_group),
            ("MBX_LEARNED_INCREMENTAL", "0"),
        ],
    )
    .0;
    assert!(count(&first_warm, "hits") > 0);
    assert!(count(&first_changed, "misses") > 0);
    assert!(count(&second_warm, "hits") > 0);
    let archive = reports.path().join("job.tar");

    let export = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .current_dir(first.path())
        .args(["cache", "export", "--group", warm_group])
        .arg(&archive)
        .env("MBX_CACHE_DIR", source_store.path())
        .output()
        .unwrap();
    assert!(
        export.status.success(),
        "group export failed: {}",
        String::from_utf8_lossy(&export.stderr)
    );
    let import = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .args(["cache", "import"])
        .arg(&archive)
        .env("MBX_CACHE_DIR", destination_store.path())
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "group import failed: {}",
        String::from_utf8_lossy(&import.stderr)
    );
    let stats: serde_json::Value = serde_json::from_str(&mbx(
        destination_store.path(),
        &["cache", "stats", "--json"],
    ))
    .unwrap();
    assert!(
        stats["action_results"].as_u64().unwrap() >= 3,
        "every grouped build should seed the destination store: {stats}; export: {}; import: {}",
        String::from_utf8_lossy(&export.stdout),
        String::from_utf8_lossy(&import.stdout),
    );
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

/// Edit the fixture so it compiles to something new.
fn edit_project(project: &Path, revision: u32) {
    std::fs::write(
        project.join("src/lib.rs"),
        format!(
            "pub fn double(value: u32) -> u32 {{\n    value * 2 + {revision} - {revision}\n}}\n"
        ),
    )
    .unwrap();
}

/// Incremental state mbx is keeping for churning units, as opposed to the
/// `incremental/` directory cargo drives itself.
///
/// Directories only: the churn records that decide when to create one live
/// beside them as files, and every compilation writes one of those whether or
/// not it ever goes hot.
fn learned_sessions(cache: &Path) -> usize {
    std::fs::read_dir(cache.join("incremental"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|checkout| std::fs::read_dir(checkout.path()).into_iter().flatten())
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count()
}

fn compiled_incrementally(stats: &serde_json::Value) -> u64 {
    stats["compiler"]["incremental"]["invocations"]
        .as_u64()
        .unwrap_or(0)
}

/// A workspace crate somebody is editing misses on every build no matter what
/// the cache does. On its first edit, it gets its own incremental
/// state -- which never reaches the store, because it describes one checkout's
/// edit history rather than its source.
#[test]
fn a_workspace_crate_is_incremental_on_its_first_edit() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    let cold = build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    assert_eq!(
        compiled_incrementally(&cold),
        0,
        "a cold build should populate the shared cache: {cold}"
    );

    edit_project(project.path(), 1);
    let stats = build(
        project.path(),
        store.path(),
        &reports.path().join("edit-1.json"),
    );

    assert!(
        compiled_incrementally(&stats) > 0,
        "the edited crate should have compiled incrementally by now: {stats}"
    );
    assert!(
        learned_sessions(store.path()) > 0,
        "it should have left incremental state behind: {stats}"
    );
    assert_eq!(
        stats["stored_bytes"].as_u64(),
        Some(0),
        "an incremental artifact must never be published: {stats}"
    );

    let cleaned = Command::new(env!("CARGO_BIN_EXE_mbx"))
        .current_dir(project.path())
        .arg("clean")
        .env("MBX_CACHE_DIR", store.path())
        .env("MBX_GC_AUTO", "0")
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("mbx clean should run");
    assert!(
        cleaned.status.success(),
        "clean failed: {}",
        String::from_utf8_lossy(&cleaned.stderr)
    );

    edit_project(project.path(), 2);
    let after_clean = build(
        project.path(),
        store.path(),
        &reports.path().join("edit-after-clean.json"),
    );
    assert!(
        compiled_incrementally(&after_clean) > 0,
        "cargo clean should preserve learned incremental state: {after_clean}"
    );
}

/// The same evidence that turns it on turns it off: once the content stops
/// moving, the unit compiles normally again and rejoins the shared cache.
#[test]
fn a_settled_crate_publishes_again() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    for revision in 1..=4 {
        edit_project(project.path(), revision);
        build(
            project.path(),
            store.path(),
            &reports.path().join(format!("edit-{revision}.json")),
        );
    }

    // Same content as the last build, so the key it recorded is the key this
    // compilation has: the churn is over.
    wipe_target(project.path());
    let settled = build(
        project.path(),
        store.path(),
        &reports.path().join("settled.json"),
    );
    assert_eq!(
        compiled_incrementally(&settled),
        0,
        "unchanged content should compile normally: {settled}"
    );
    assert!(
        settled["stored_bytes"].as_u64().unwrap_or(0) > 0,
        "and it should be published: {settled}"
    );

    wipe_target(project.path());
    let warm = build(
        project.path(),
        store.path(),
        &reports.path().join("warm.json"),
    );
    assert!(
        warm["hits"].as_u64().unwrap_or(0) > 0,
        "so a later build can restore it: {warm}"
    );
}

/// A compilation that failed left nothing behind to compare against, so the
/// retry that follows -- with nothing edited in between -- must not read as a
/// crate that had settled and drop it back to compiling from scratch.
#[test]
fn a_failed_build_does_not_cost_the_streak() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    for revision in 1..=3 {
        edit_project(project.path(), revision);
        build(
            project.path(),
            store.path(),
            &reports.path().join(format!("edit-{revision}.json")),
        );
    }

    // Break it, then retry the same broken source twice over.
    std::fs::write(project.path().join("src/lib.rs"), "fn broken( {\n").unwrap();
    for attempt in 0..2 {
        let failed = Command::new(env!("CARGO_BIN_EXE_mbx"))
            .current_dir(project.path())
            .args(["build", "--offline"])
            .env("MBX_CACHE_DIR", store.path())
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("MBX_INCREMENTAL")
            .env_remove("CARGO_INCREMENTAL")
            .env_remove("CI")
            .output()
            .expect("mbx should run");
        assert!(!failed.status.success(), "attempt {attempt} should fail");
    }

    // One more real edit is all it should take to go hot.
    edit_project(project.path(), 4);
    let stats = build(
        project.path(),
        store.path(),
        &reports.path().join("recovered.json"),
    );

    assert!(
        compiled_incrementally(&stats) > 0,
        "the failures should not have reset what the edits established: {stats}"
    );
}

/// A fresh runner has no incremental state to reuse, so the trade is all cost.
#[test]
fn churn_earns_nothing_in_ci() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    let mut stats = build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    for revision in 1..=4 {
        edit_project(project.path(), revision);
        stats = build_with(
            project.path(),
            store.path(),
            &reports.path().join(format!("edit-{revision}.json")),
            &[("CI", "true")],
        )
        .0;
    }

    assert_eq!(
        compiled_incrementally(&stats),
        0,
        "CI should have compiled every edit normally: {stats}"
    );
}

/// A crate's action key hashes the artifacts it links against, so rebuilding a
/// dependency changes it without anybody having touched the crate. Watching the
/// key instead of the sources would send the whole cone above an edited crate
/// hot, and stop all of it publishing -- the sharing loss this feature exists
/// to avoid.
#[test]
fn editing_one_crate_leaves_its_dependents_publishing() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_dependent_project(project.path());

    let mut stats = build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    for revision in 1..=5 {
        std::fs::write(
            project.path().join("base/src/lib.rs"),
            format!("pub fn value() -> u32 {{ {revision} }}\n"),
        )
        .unwrap();
        stats = build(
            project.path(),
            store.path(),
            &reports.path().join(format!("edit-{revision}.json")),
        );
    }

    assert_eq!(
        compiled_incrementally(&stats),
        1,
        "only the edited crate should be compiling incrementally: {stats}"
    );
    assert!(
        stats["stored_bytes"].as_u64().unwrap_or(0) > 0,
        "the crate above it never changed, so it should still publish: {stats}"
    );
}

#[test]
fn learned_incremental_can_be_turned_off() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    let mut stats = build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    for revision in 1..=4 {
        edit_project(project.path(), revision);
        stats = build_with(
            project.path(),
            store.path(),
            &reports.path().join(format!("edit-{revision}.json")),
            &[("MBX_LEARNED_INCREMENTAL", "0")],
        )
        .0;
    }

    assert_eq!(
        compiled_incrementally(&stats),
        0,
        "the setting should have kept every edit normal: {stats}"
    );
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
fn restores_predictions_across_equivalent_cargo_commands() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());

    build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    wipe_target(project.path());

    let warm = cargo_with(
        project.path(),
        store.path(),
        &reports.path().join("warm.json"),
        &["build", "--offline", "--workspace"],
        &[],
    )
    .0;

    assert!(
        count(&warm, "hits") > 0,
        "adding a Cargo selector should not discard learned predictions: {warm}"
    );
    assert_eq!(
        count(&warm, "unconsulted"),
        0,
        "equivalent compilations should all have predictions: {warm}"
    );
}

#[test]
fn failed_prediction_lookups_are_timed_as_misses() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(project.path());
    build(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
    );
    assert!(corrupt_action_results(store.path()) > 0);
    wipe_target(project.path());

    let rebuilt = build(
        project.path(),
        store.path(),
        &reports.path().join("rebuilt.json"),
    );

    assert!(count(&rebuilt, "lookups") > 0, "{rebuilt}");
    assert!(
        rebuilt["compiler"]["miss"]["invocations"]
            .as_u64()
            .is_some_and(|invocations| invocations > 0),
        "{rebuilt}"
    );
    assert_eq!(count(&rebuilt, "unconsulted"), 0, "{rebuilt}");
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

    edit_project(second.path(), 1);
    let edited = build(
        second.path(),
        store.path(),
        &reports.path().join("second-edit.json"),
    );
    assert!(
        compiled_incrementally(&edited) > 0,
        "a warm checkout should recognize its first edit immediately: {edited}"
    );
}

#[test]
fn a_release_marker_does_not_disable_the_cache() {
    let store = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_project(first.path());
    write_project(second.path());

    build_with(
        first.path(),
        store.path(),
        &reports.path().join("first.json"),
        &[("MBX_RELEASE", "1")],
    );
    let (warm, _) = build_with(
        second.path(),
        store.path(),
        &reports.path().join("second.json"),
        &[("MBX_RELEASE", "1")],
    );

    assert!(
        count(&warm, "hits") > 0,
        "a release-marked build should still reuse cached output: {warm}"
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

/// Write a build-script fixture that leaves an observable execution count and
/// a nested output tree. The count is deliberately outside `OUT_DIR`, so a
/// restore cannot counterfeit an execution.
fn write_execution_cached_project(directory: &Path, declares_inputs: bool) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"execution-cache-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(directory.join("input.txt"), "first\n").unwrap();
    let declaration = if declares_inputs {
        "println!(\"cargo:rerun-if-changed=input.txt\");\n    println!(\"cargo:rerun-if-env-changed=EXECUTION_CACHE_MODE\");"
    } else {
        "println!(\"cargo:rustc-cfg=generated\");"
    };
    std::fs::write(
        directory.join("build.rs"),
        format!(
            "use std::{{env, fs, path::PathBuf}};\n\
             fn main() {{\n\
                 let count = PathBuf::from(\"runs\");\n\
                 let runs = fs::read_to_string(&count).ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0) + 1;\n\
                 fs::write(count, runs.to_string()).unwrap();\n\
                 let input = fs::read_to_string(\"input.txt\").unwrap();\n\
                 let out = PathBuf::from(env::var_os(\"OUT_DIR\").unwrap());\n\
                 fs::create_dir_all(out.join(\"nested\")).unwrap();\n\
                 fs::write(out.join(\"generated.rs\"), format!(\"pub const VALUE: &str = {{:?}};\\n\", input)).unwrap();\n\
                 fs::write(out.join(\"nested/header.h\"), input).unwrap();\n\
                 println!(\"cargo:rustc-env=MANIFEST_COPY={{}}\", env::var(\"CARGO_MANIFEST_DIR\").unwrap());\n\
                 {declaration}\n\
             }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        directory.join("src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n\
         pub const MANIFEST_COPY: &str = env!(\"MANIFEST_COPY\");\n",
    )
    .unwrap();
    let status = Command::new(cargo())
        .current_dir(directory)
        .args(["generate-lockfile", "--offline"])
        .status()
        .unwrap();
    assert!(status.success());
}

/// Write a fixture that relies on Cargo's implicit package-wide build-script
/// input. Its execution log lives outside the package so observing a run does
/// not itself invalidate that input.
fn write_default_input_project(directory: &Path, execution_log: &Path) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"default-input-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(directory.join("input.txt"), "first\n").unwrap();
    let build_script =
        "use std::{env, fs, path::PathBuf};\n\
         fn main() {\n\
             let log = PathBuf::from($EXECUTION_LOG);\n\
             let mut runs = fs::read_to_string(&log).unwrap_or_default();\n\
             runs.push_str(\"run\\n\");\n\
             fs::write(log, runs).unwrap();\n\
             let input = fs::read_to_string(\"input.txt\").unwrap();\n\
             let out = PathBuf::from(env::var_os(\"OUT_DIR\").unwrap());\n\
             fs::write(out.join(\"generated.rs\"), format!(\"pub const VALUE: &str = {:?};\\n\", input)).unwrap();\n\
         }\n"
            .replace(
                "$EXECUTION_LOG",
                &format!("{:?}", execution_log.to_str().unwrap()),
            );
    std::fs::write(directory.join("build.rs"), build_script).unwrap();
    std::fs::write(
        directory.join("src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n",
    )
    .unwrap();
    let status = Command::new(cargo())
        .current_dir(directory)
        .args(["generate-lockfile", "--offline"])
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn build_script_execution_and_out_dir_restore_across_checkouts() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_execution_cached_project(first.path(), true);
    write_execution_cached_project(second.path(), true);

    let no_link_cache = [("MBX_CACHE_LINKS", "0")];
    build_with(
        first.path(),
        store.path(),
        &reports.path().join("first.json"),
        &no_link_cache,
    );
    let (warm, stderr) = build_with(
        second.path(),
        store.path(),
        &reports.path().join("second.json"),
        &no_link_cache,
    );

    assert_eq!(
        std::fs::read_to_string(first.path().join("runs")).unwrap(),
        "1"
    );
    assert!(
        !second.path().join("runs").exists(),
        "the second checkout ran its build script instead of restoring it: {warm}\n{stderr}"
    );
    let header = second
        .path()
        .join("target/debug/build")
        .read_dir()
        .unwrap()
        .find_map(|entry| {
            let path = entry.ok()?.path().join("out/nested/header.h");
            path.is_file().then_some(path)
        })
        .expect("nested OUT_DIR output should be restored");
    assert_eq!(std::fs::read_to_string(header).unwrap(), "first\n");
    let replayed = second
        .path()
        .join("target/debug/build")
        .read_dir()
        .unwrap()
        .find_map(|entry| {
            let path = entry.ok()?.path().join("output");
            path.is_file()
                .then(|| std::fs::read_to_string(path).ok())
                .flatten()
        })
        .expect("Cargo should retain the replayed build-script directives");
    assert!(
        replayed.contains(second.path().to_string_lossy().as_ref()),
        "replayed directives should name the restoring checkout: {replayed}"
    );
    assert!(
        !replayed.contains(first.path().to_string_lossy().as_ref()),
        "replayed directives retained the publishing checkout: {replayed}"
    );
    assert!(count(&warm, "hits") >= 1, "build script should hit: {warm}");
}

#[test]
fn changed_declared_input_executes_build_script_again() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_execution_cached_project(project.path(), true);
    build(
        project.path(),
        store.path(),
        &reports.path().join("first.json"),
    );
    std::fs::write(project.path().join("input.txt"), "second\n").unwrap();
    build(
        project.path(),
        store.path(),
        &reports.path().join("second.json"),
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("runs")).unwrap(),
        "2"
    );
}

#[test]
fn changed_declared_environment_executes_build_script_again() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_execution_cached_project(project.path(), true);
    build_with(
        project.path(),
        store.path(),
        &reports.path().join("first.json"),
        &[("EXECUTION_CACHE_MODE", "first")],
    );
    build_with(
        project.path(),
        store.path(),
        &reports.path().join("second.json"),
        &[("EXECUTION_CACHE_MODE", "second")],
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("runs")).unwrap(),
        "2"
    );
}

#[test]
fn build_script_without_declared_inputs_uses_the_package_tree() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let log = tempfile::NamedTempFile::new().unwrap();
    write_default_input_project(first.path(), log.path());
    write_default_input_project(second.path(), log.path());
    build(
        first.path(),
        store.path(),
        &reports.path().join("first.json"),
    );
    let (warm, _) = build_with(
        second.path(),
        store.path(),
        &reports.path().join("second.json"),
        &[],
    );
    assert_eq!(std::fs::read_to_string(log.path()).unwrap(), "run\n");
    assert!(count(&warm, "hits") >= 1, "build script should hit: {warm}");
}

#[test]
fn a_changed_implicit_package_input_executes_the_build_script_again() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let log = tempfile::NamedTempFile::new().unwrap();
    write_default_input_project(project.path(), log.path());
    build(
        project.path(),
        store.path(),
        &reports.path().join("first.json"),
    );
    std::fs::write(project.path().join("input.txt"), "second\n").unwrap();
    build(
        project.path(),
        store.path(),
        &reports.path().join("second.json"),
    );
    assert_eq!(std::fs::read_to_string(log.path()).unwrap(), "run\nrun\n");
}

#[test]
fn build_script_execution_cache_can_be_turned_off() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_execution_cached_project(first.path(), true);
    write_execution_cached_project(second.path(), true);
    build(
        first.path(),
        store.path(),
        &reports.path().join("enabled.json"),
    );
    let disabled = [("MBX_BUILD_SCRIPT_EXECUTION", "0")];
    build_with(
        second.path(),
        store.path(),
        &reports.path().join("disabled.json"),
        &disabled,
    );
    assert_eq!(
        std::fs::read_to_string(second.path().join("runs")).unwrap(),
        "1",
        "the opt-out should execute the build script instead of restoring it"
    );
}

#[test]
fn installed_build_script_wrapper_is_transparent_outside_an_mbx_session() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_execution_cached_project(project.path(), true);
    build(
        project.path(),
        store.path(),
        &reports.path().join("first.json"),
    );
    std::fs::write(project.path().join("input.txt"), "second\n").unwrap();

    let status = Command::new(cargo())
        .current_dir(project.path())
        .args(["build", "--offline"])
        .env_remove("MBX_SOCKET")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        std::fs::read_to_string(project.path().join("runs")).unwrap(),
        "2"
    );
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

/// Turning sharing off preserves the old checkout-specific behavior for a
/// compilation that consumes `OUT_DIR`, without affecting one that only uses a
/// build-script cfg.
#[test]
fn out_dir_sharing_can_be_turned_off() {
    let disabled = [("MBX_SHARE_OUT_DIR", "0")];
    for (generated, expect_hits) in [(Generated::Cfg, true), (Generated::Include, false)] {
        assert_eq!(
            two_checkouts_share(generated, &disabled),
            expect_hits,
            "the opt-out changed the wrong shape"
        );
    }
}

/// By default, the compilation is remapped so rustc records the
/// placeholder instead of the real `OUT_DIR`, and the include-only shape crosses
/// checkouts. The shape that keeps the value in a string still does not: the
/// remapping cannot reach into the artifact, and mbx reads the outputs rather
/// than assuming it can.
///
/// The pair is the test. Either half alone would pass for the wrong reason.
#[test]
fn out_dir_crosses_checkouts_only_where_the_artifact_allows_it() {
    for (generated, expect_hits) in [(Generated::Include, true), (Generated::Text, false)] {
        assert_eq!(
            two_checkouts_share(generated, &[]),
            expect_hits,
            "the default shared the wrong shape"
        );
    }
}

/// Build the same fixture in two checkouts, reporting whether the second one
/// reused anything from the first.
/// Whether the *crate* shares, which is what every caller here is asking.
///
/// Native links are left out rather than counted: a build script's own binary
/// reads no `OUT_DIR` -- the value is handed to it when it runs, long after it
/// compiles -- so it shares between these checkouts whatever the crate does,
/// and counting it would answer a question nobody asked.
fn two_checkouts_share(generated: Generated, settings: &[(&str, &str)]) -> bool {
    let store = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    write_generated_project(first.path(), generated);
    write_generated_project(second.path(), generated);

    let settings: Vec<(&str, &str)> = [
        ("MBX_CACHE_LINKS", "0"),
        ("MBX_BUILD_SCRIPT_EXECUTION", "0"),
    ]
    .into_iter()
    .chain(settings.iter().copied())
    .collect();
    build_with(
        first.path(),
        store.path(),
        &reports.path().join("first.json"),
        &settings,
    );
    let (stats, _) = build_with(
        second.path(),
        store.path(),
        &reports.path().join("second.json"),
        &settings,
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
        stderr.contains("mbx[gc]: evicted"),
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

    assert!(
        !stderr.contains("mbx[gc]:"),
        "no sweep should run: {stderr}"
    );
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

/// The per-compilation stream `mbx tui` reads.
mod session_events {
    use super::*;
    use std::path::PathBuf;

    /// Where session streams live, beside the rest of the store's bookkeeping.
    fn sessions_dir(store: &Path) -> PathBuf {
        store.join("actions/sessions/v1")
    }

    /// The events of the one session `store` recorded, in order.
    fn stream(store: &Path) -> Vec<serde_json::Value> {
        let directory = sessions_dir(store);
        let mut streams: Vec<PathBuf> = std::fs::read_dir(&directory)
            .expect("a session directory should exist")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .collect();
        assert_eq!(
            streams.len(),
            1,
            "one build should record exactly one stream, found {streams:?}"
        );
        let contents = std::fs::read_to_string(streams.pop().unwrap()).unwrap();
        contents
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line should be JSON"))
            .collect()
    }

    fn outcomes(events: &[serde_json::Value], kind: &str) -> usize {
        events
            .iter()
            .filter(|event| event["type"] == "action" && event["outcome"]["kind"] == kind)
            .count()
    }

    #[test]
    fn a_build_records_its_compilations_between_a_start_and_its_totals() {
        let store = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_project(project.path());

        let cold = build(
            project.path(),
            store.path(),
            &reports.path().join("cold.json"),
        );

        let events = stream(store.path());
        assert_eq!(events.first().unwrap()["type"], "session_started");
        assert_eq!(
            events.first().unwrap()["command"],
            serde_json::json!(["build", "--offline"])
        );
        let last = events.last().unwrap();
        assert_eq!(last["type"], "session_finished");
        // The stream's own totals are the summary's totals, so a reader of a
        // finished session never has to re-derive them from the rows.
        assert_eq!(last["stats"]["unconsulted"], cold["unconsulted"]);
        assert_eq!(last["stats"]["hits"], cold["hits"]);
        // A cold build compiles without a key to look up, and every one of
        // those compilations should appear as a row.
        assert_eq!(
            outcomes(&events, "unconsulted") as u64,
            count(&cold, "unconsulted"),
            "every unconsulted compilation should have a row: {events:?}"
        );
    }

    #[test]
    fn a_warm_build_records_a_row_per_hit_naming_its_crate() {
        let store = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_project(project.path());

        build(
            project.path(),
            store.path(),
            &reports.path().join("cold.json"),
        );
        // Load-bearing, not cleanup: without the wipe cargo finds the cold
        // build's outputs and the warm build has nothing to restore.
        wipe_target(project.path());
        let warm = build(
            project.path(),
            store.path(),
            &reports.path().join("warm.json"),
        );

        // Two builds, two streams; this reads the newest.
        let directory = sessions_dir(store.path());
        let mut streams: Vec<PathBuf> = std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .collect();
        streams.sort();
        let contents = std::fs::read_to_string(streams.last().unwrap()).unwrap();
        let events: Vec<serde_json::Value> = contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        let hits = count(&warm, "hits");
        assert!(hits > 0, "the warm build should hit: {warm}");
        assert_eq!(
            outcomes(&events, "hit") as u64,
            hits,
            "every hit should have a row: {events:?}"
        );
        // The crate name is what the protocol bump was for: a row that cannot
        // say which crate it restored is not worth showing.
        assert!(
            events.iter().any(|event| {
                event["outcome"]["kind"] == "hit" && event["crate_name"] == "fixture"
            }),
            "a hit row should name the crate it restored: {events:?}"
        );
    }

    #[test]
    fn recording_can_be_turned_off() {
        let store = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_project(project.path());

        build_with(
            project.path(),
            store.path(),
            &reports.path().join("cold.json"),
            &[("MBX_EVENTS", "0")],
        );

        assert!(
            !sessions_dir(store.path()).exists(),
            "a build that records nothing should leave no session directory"
        );
    }
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
    fn cargo_reports_workspace_paths_for_managed_artifacts() {
        let store = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write_project(project.path());

        let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
            .current_dir(project.path())
            .args(["build", "--offline", "--message-format=json"])
            .env("MBX_CACHE_DIR", store.path())
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .expect("mbx should run");
        assert!(
            output.status.success(),
            "build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let artifact = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|message| message["reason"] == "compiler-artifact")
            .expect("Cargo should report the built artifact");
        let target = project.path().join("target");
        for filename in artifact["filenames"].as_array().unwrap() {
            let filename = Path::new(filename.as_str().unwrap());
            assert!(
                filename.starts_with(&target),
                "debugger-facing artifact path escaped the workspace: {}",
                filename.display()
            );
        }
    }

    #[test]
    fn mbx_clean_removes_the_managed_view_and_link() {
        let store = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let reports = tempfile::tempdir().unwrap();
        write_project(project.path());
        build(
            project.path(),
            store.path(),
            &reports.path().join("cold.json"),
        );
        let managed = managed(project.path());

        let output = Command::new(env!("CARGO_BIN_EXE_mbx"))
            .current_dir(project.path().join("src"))
            .arg("clean")
            .env("MBX_CACHE_DIR", store.path())
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .expect("mbx clean should run");

        assert!(
            output.status.success(),
            "clean failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!managed.exists());
        assert!(std::fs::symlink_metadata(project.path().join("target")).is_err());
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
        assert!(
            std::fs::read_dir(store.path().join("incremental"))
                .unwrap()
                .next()
                .is_some(),
            "the build should record learned incremental state"
        );
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
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("freed 1 target directories"),
            "successful target collection should still be reported"
        );
        assert!(
            stdout.contains("freed 1 learned incremental directories"),
            "successful incremental collection should still be reported: {stdout}"
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

/// Build `project` into an explicit target directory, so two builds of the
/// same checkout differ only in where their outputs land.
fn build_into_target(
    project: &Path,
    store: &Path,
    target: &Path,
    settings: &[(&str, &str)],
) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
    command
        .current_dir(project)
        .args(["build", "--offline"])
        .env("MBX_CACHE_DIR", store)
        .env("CARGO_TARGET_DIR", target)
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("MBX_INCREMENTAL")
        .env_remove("CARGO_INCREMENTAL")
        .env_remove("MBX_LEARNED_INCREMENTAL")
        .env_remove("CI")
        .env_remove("MBX_SHARE_OUT_DIR")
        .env_remove("MBX_SOCKET")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER");
    for (name, value) in settings {
        command.env(name, value);
    }
    let output = command.output().expect("mbx should run");
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Read the dep-info rustc wrote for the fixture's library.
fn dep_info_contents(target: &Path) -> String {
    let deps = target.join("debug/deps");
    let entry = std::fs::read_dir(&deps)
        .expect("deps directory should exist")
        .filter_map(Result::ok)
        .find(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("fixture-") && name.ends_with(".d")
        })
        .expect("the library's dep-info should exist");
    std::fs::read_to_string(entry.path()).expect("dep-info should be readable")
}

/// A restored compilation must describe the checkout it was restored into.
///
/// Dep-info rules are keyed by absolute output paths, so a result stored
/// verbatim hands the next target directory rules naming the one that
/// published them. Cargo reads that file, and `MBX_VERIFY=1` compares it, so
/// the stale spelling is both a wrong artifact and a permanent divergence.
#[test]
fn a_restored_dep_info_names_the_target_directory_it_was_restored_into() {
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_project(project.path());

    build_into_target(project.path(), store.path(), first.path(), &[]);
    build_into_target(project.path(), store.path(), second.path(), &[]);

    let restored = dep_info_contents(second.path());
    let foreign = first.path().to_string_lossy().into_owned();
    assert!(
        !restored.contains(&foreign),
        "restored dep-info still names the publishing target directory:\n{restored}"
    );
    assert!(
        restored.contains(&*second.path().to_string_lossy()),
        "restored dep-info should name this target directory:\n{restored}"
    );
}

/// The same compilation restored into a different target directory is what a
/// fresh compilation there would have produced.
///
/// Unix only, and the reason is worth stating. On Windows the compiled
/// artifact itself is not byte-identical between two target directories: with
/// everything else held constant -- one checkout, one set of sources,
/// incremental off -- the rlib still differs, because the debug information
/// records where the compilation wrote its objects. Nothing this fix does can
/// change that, and rewriting inside a compiled artifact would be corruption
/// rather than translation, so verification there reports a difference that is
/// real.
#[cfg(unix)]
#[test]
fn verification_is_clean_across_target_directories() {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_project(project.path());

    build_into_target(project.path(), store.path(), first.path(), &[]);
    let report = reports.path().join("verify.json");
    let stderr = build_into_target(
        project.path(),
        store.path(),
        second.path(),
        &[
            ("MBX_VERIFY", "1"),
            ("MBX_STATS_REPORT", report.to_str().unwrap()),
        ],
    );
    let reported = stderr
        .lines()
        .filter(|line| line.contains("diverged"))
        .collect::<Vec<_>>()
        .join("\n");

    let stats: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("a report should be written"))
            .expect("the report should be JSON");
    assert!(
        count(&stats, "verifications") > 0,
        "the run should have verified something: {stats}"
    );
    assert_eq!(
        count(&stats, "divergences"),
        0,
        "a restore into another target directory diverged: {reported}"
    );
}
/// Write a fixture whose build script compiles C through `$CC`.
///
/// Deliberately hand-rolled rather than using the `cc` crate: this suite
/// resolves offline and takes no dependencies, and what is under test is the
/// shim the build script inherits, not the crate that would call it.
#[cfg(unix)]
fn write_c_project(directory: &Path) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::create_dir_all(directory.join("include")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("include/hello.h"),
        "int hello_value(void);\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("src/hello.c"),
        "#include \"hello.h\"\nint hello_value(void) { return 7; }\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("build.rs"),
        r#"use std::{env, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    // The `cc` crate reads HOST_CC before CC when host and target agree, and
    // that is the variable mbx sets; mirroring its precedence is what makes
    // this fixture exercise the same path a real build script takes.
    let compiler = env::var("HOST_CC")
        .or_else(|_| env::var("CC"))
        .unwrap_or_else(|_| "cc".into());
    let status = Command::new(&compiler)
        .arg("-O2")
        .arg("-Iinclude")
        .arg("-c")
        .arg("-o")
        .arg(out.join("hello.o"))
        .arg("src/hello.c")
        .status()
        .expect("the C compiler should run");
    assert!(status.success(), "the fixture's C should compile");
    // The tests disable build-script execution caching so the second checkout
    // reaches the compiler shim whose behavior is under test.
    println!("cargo:rustc-cfg=c_compiled");
}
"#,
    )
    .unwrap();
    std::fs::write(
        directory.join("src/lib.rs"),
        "pub fn value() -> u32 { 7 }\n",
    )
    .unwrap();
    let status = Command::new(cargo())
        .current_dir(directory)
        .args(["generate-lockfile", "--offline"])
        .status()
        .expect("cargo should run");
    assert!(status.success(), "the fixture should resolve offline");
}

/// Whether a C compiler is available to compile the fixture at all.
#[cfg(unix)]
fn has_c_compiler() -> bool {
    // Deliberately a real compilation rather than `cc -v`, which is what the
    // adapter's own identity probe runs. Sharing that signal would mean a
    // regression in the probe skipped these tests instead of failing them --
    // the feature would go dead and the suite would stay green.
    let Ok(directory) = tempfile::tempdir() else {
        return false;
    };
    let source = directory.path().join("probe.c");
    if std::fs::write(&source, "int probe(void) { return 0; }\n").is_err() {
        return false;
    }
    Command::new("cc")
        .arg("-c")
        .arg("-o")
        .arg(directory.path().join("probe.o"))
        .arg(&source)
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Build the fixture in two fresh checkouts sharing one store, and report how
/// much the second one reused.
///
/// The hit count is the measure: a C compilation that crosses checkouts is one
/// more cached action than the same build without it. Compiler statistics are
/// keyed by outcome rather than by what was compiled, so they cannot tell the
/// C compile apart from the Rust one.
#[cfg(unix)]
fn warm_checkout_hits(settings: &[(&str, &str)]) -> u64 {
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_c_project(first.path());
    write_c_project(second.path());

    let settings: Vec<(&str, &str)> = [("MBX_BUILD_SCRIPT_EXECUTION", "0")]
        .into_iter()
        .chain(settings.iter().copied())
        .collect();
    build_with(
        first.path(),
        store.path(),
        &reports.path().join("first.json"),
        &settings,
    );
    let (warm, _) = build_with(
        second.path(),
        store.path(),
        &reports.path().join("second.json"),
        &settings,
    );
    count(&warm, "hits")
}

/// A build script's C compilation is cached, and a second checkout restores it
/// rather than compiling again.
#[cfg(unix)]
#[test]
fn a_build_script_c_compilation_crosses_checkouts() {
    if !has_c_compiler() {
        return;
    }
    let cached = warm_checkout_hits(&[]);
    let uncached = warm_checkout_hits(&[("MBX_CC", "0")]);
    assert_eq!(
        cached,
        uncached + 1,
        "the C compilation should be exactly one more restored action"
    );
}

/// Write a fixture whose build script generates a header into `OUT_DIR` and
/// compiles several objects into that same directory.
///
/// This is the shape that a manifest counting every filename gets wrong: the
/// objects land beside the generated header, so what the key recorded depended
/// on how many sibling compilations had finished.
#[cfg(unix)]
fn write_generated_header_project(directory: &Path) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(
            directory.join(format!("src/{name}.c")),
            format!("#include \"config.h\"\nint {name}(void) {{ return CONFIG_V; }}\n"),
        )
        .unwrap();
    }
    std::fs::write(
        directory.join("build.rs"),
        r##"use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out.join("config.h"), "#define CONFIG_V 7
").unwrap();
    let compiler = env::var("HOST_CC")
        .or_else(|_| env::var("CC"))
        .unwrap_or_else(|_| "cc".into());
    for name in ["a", "b", "c"] {
        let status = Command::new(&compiler)
            .arg(format!("-I{}", out.display()))
            .arg("-c")
            .arg("-o")
            .arg(out.join(format!("{name}.o")))
            .arg(format!("src/{name}.c"))
            .status()
            .expect("the C compiler should run");
        assert!(status.success());
    }
}
"##,
    )
    .unwrap();
    std::fs::write(
        directory.join("src/lib.rs"),
        "pub fn value() -> u32 { 7 }\n",
    )
    .unwrap();
    let status = Command::new(cargo())
        .current_dir(directory)
        .args(["generate-lockfile", "--offline"])
        .status()
        .expect("cargo should run");
    assert!(status.success(), "the fixture should resolve offline");
}

/// A header generated into `OUT_DIR` is cached like any other, even though the
/// build writes its objects into that same directory.
#[cfg(unix)]
#[test]
fn objects_landing_beside_a_generated_header_still_cross_checkouts() {
    if !has_c_compiler() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_generated_header_project(first.path());
    write_generated_header_project(second.path());

    build_with(
        first.path(),
        store.path(),
        &reports.path().join("cold.json"),
        &[("MBX_BUILD_SCRIPT_EXECUTION", "0")],
    );
    let (warm, _) = build_with(
        second.path(),
        store.path(),
        &reports.path().join("warm.json"),
        &[("MBX_BUILD_SCRIPT_EXECUTION", "0")],
    );
    assert_eq!(
        count(&warm, "hits"),
        5,
        "all three C compilations, the Rust one, and the build script should be restored: {warm}"
    );
}

/// A cached C warning names the checkout it is replayed in, not the one that
/// published it.
///
/// A compiler diagnostic names the file it is about, and a generated source
/// lives at an absolute path that differs per checkout, so replaying the stored
/// bytes verbatim would point the reader at somebody else's tree.
#[cfg(unix)]
#[test]
fn a_restored_c_diagnostic_names_the_checkout_it_is_replayed_in() {
    if !has_c_compiler() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_warning_project(first.path());
    write_warning_project(second.path());

    build_with(
        first.path(),
        store.path(),
        &reports.path().join("cold.json"),
        &[],
    );
    let (warm, stderr) = build_with(
        second.path(),
        store.path(),
        &reports.path().join("warm.json"),
        &[],
    );
    assert!(
        count(&warm, "hits") > 0,
        "the second checkout should have restored the compilation: {warm}"
    );
    let diagnostic = stderr
        .lines()
        .find(|line| line.contains("CCWARN>>>"))
        .unwrap_or_default();
    assert!(
        !diagnostic.contains(&first.path().display().to_string()),
        "a replayed warning must not name the checkout that published it: {diagnostic}"
    );
}

/// A fixture whose build script compiles a generated source that warns, so the
/// diagnostic carries an absolute path.
#[cfg(unix)]
fn write_warning_project(directory: &Path) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("build.rs"),
        r##"use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let source = out.join("gen.c");
    fs::write(&source, "int value(void) { int unused = 1; return 7; }
").unwrap();
    let compiler = env::var("HOST_CC")
        .or_else(|_| env::var("CC"))
        .unwrap_or_else(|_| "cc".into());
    let output = Command::new(&compiler)
        .arg("-Wall")
        .arg("-c")
        .arg("-o")
        .arg(out.join("gen.o"))
        .arg(&source)
        .output()
        .expect("the C compiler should run");
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        println!("cargo:warning=CCWARN>>>{line}");
    }
    assert!(output.status.success());
}
"##,
    )
    .unwrap();
    std::fs::write(
        directory.join("src/lib.rs"),
        "pub fn value() -> u32 { 7 }\n",
    )
    .unwrap();
    let status = Command::new(cargo())
        .current_dir(directory)
        .args(["generate-lockfile", "--offline"])
        .status()
        .expect("cargo should run");
    assert!(status.success(), "the fixture should resolve offline");
}

/// A prediction that no longer describes the tree must not strand the
/// compilation.
///
/// This adapter has no second way to build a key, so a stale prediction that
/// aborted the cache path would fail identically on every later build. The
/// recovery is what the test pins: compile, republish, and hit next time.
#[cfg(unix)]
#[test]
fn a_stale_prediction_recovers_instead_of_stranding_the_compilation() {
    if !has_c_compiler() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_c_project(project.path());

    // Record a prediction that names the header.
    build_with(
        project.path(),
        store.path(),
        &reports.path().join("cold.json"),
        &[],
    );

    // Delete the header and stop including it. The command line is untouched,
    // so the invocation still resolves to the same prediction -- one that now
    // names a file that is gone.
    std::fs::remove_file(project.path().join("include/hello.h")).unwrap();
    std::fs::write(
        project.path().join("src/hello.c"),
        "int hello_value(void) { return 7; }\n",
    )
    .unwrap();

    let (recovered, _) = build_with(
        project.path(),
        store.path(),
        &reports.path().join("recover.json"),
        &[],
    );
    assert_eq!(
        recovered["bypasses"].get("cc-input-read"),
        None,
        "a stale prediction is not a bypass: {recovered}"
    );

    // A second checkout of the same modified sources shares the store but none
    // of the outputs, so what it restores is what the recovery republished.
    let second = tempfile::tempdir().unwrap();
    write_c_project(second.path());
    std::fs::remove_file(second.path().join("include/hello.h")).unwrap();
    std::fs::write(
        second.path().join("src/hello.c"),
        "int hello_value(void) { return 7; }\n",
    )
    .unwrap();
    let (warm, _) = build_with(
        second.path(),
        store.path(),
        &reports.path().join("warm.json"),
        &[],
    );
    assert_eq!(
        count(&warm, "hits"),
        3,
        "the Rust, the C, and the build script's own compilation should be restored: {warm}"
    );
}

/// A build that chose its own compiler keeps it.
///
/// `CC` is commonly exported machine-wide, so the shim standing aside is the
/// difference between redirecting a build the user configured and leaving it
/// alone.
#[cfg(unix)]
#[test]
fn an_existing_cc_setting_is_left_alone() {
    if !has_c_compiler() {
        return;
    }
    let chosen = which::which("cc").expect("cc should resolve");
    let preset = warm_checkout_hits(&[("HOST_CC", chosen.to_str().unwrap())]);
    let uncached = warm_checkout_hits(&[("MBX_CC", "0")]);
    assert_eq!(
        preset, uncached,
        "mbx should not have intercepted a compiler the build chose"
    );
}

/// The object a cache hit restores is the object the compiler produced.
#[cfg(unix)]
#[test]
fn a_restored_object_is_byte_identical_to_a_compiled_one() {
    if !has_c_compiler() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_c_project(first.path());
    write_c_project(second.path());

    build_with(
        first.path(),
        store.path(),
        &reports.path().join("first.json"),
        &[],
    );
    build_with(
        second.path(),
        store.path(),
        &reports.path().join("second.json"),
        &[],
    );

    let compiled = find_object(first.path()).expect("the first checkout should have an object");
    let restored = find_object(second.path()).expect("the second checkout should have an object");
    assert_eq!(
        std::fs::read(&compiled).unwrap(),
        std::fs::read(&restored).unwrap(),
        "a restored object must match the one that was compiled"
    );
}

/// Find the fixture's object file beneath a checkout's target directory.
#[cfg(unix)]
fn find_object(project: &Path) -> Option<std::path::PathBuf> {
    let target = project.join("target");
    let root = std::fs::read_link(&target).unwrap_or(target);
    let mut pending = vec![root];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "hello.o") {
                return Some(path);
            }
        }
    }
    None
}
