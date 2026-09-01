use super::*;
use crate::config::SummaryStyle;

#[test]
fn ambiguous_build_script_sidecars_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let invoked = directory.path().join(format!(
        "build-script-build{}",
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::write(&invoked, "shim").unwrap();
    for hash in ["one", "two"] {
        let name = format!(
            "build_script_build-{hash}{}{}",
            std::env::consts::EXE_SUFFIX,
            BUILD_SCRIPT_REAL_SUFFIX
        );
        std::fs::write(directory.path().join(name), "real").unwrap();
    }

    assert_eq!(find_build_script_real_path(&invoked), None);
}

fn test_config(cache_dir: &Path) -> Config {
    Config {
        cache_dir: cache_dir.to_path_buf(),
        stats_report: None,
        verify: false,
        incremental: false,
        share_out_dir: false,
        build_script_execution: false,
        events: false,
        cc: false,
        remote: Default::default(),
        http: Default::default(),
        gc: Default::default(),
        scheduler: Default::default(),
        target: crate::config::TargetSettings {
            views: false,
            root: cache_dir.join("targets"),
        },
    }
}

#[tokio::test]
async fn session_environment_directs_cargo_at_the_shim() {
    let cache = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let session = CacheSession::start(session_dir.path(), &test_config(cache.path()))
        .await
        .unwrap();

    let workspace = tempfile::tempdir().unwrap();
    let mut values = BTreeMap::from([
        ("RUSTC_WRAPPER".into(), "existing".into()),
        ("RUSTDOC".into(), "custom-rustdoc".into()),
    ]);
    let run = session
        .begin(
            workspace.path(),
            &workspace.path().join("target"),
            &["build".to_string()],
            &mut values,
        )
        .await;

    assert!(run.is_some());
    assert!(values.contains_key(SOCKET_ENV));
    assert!(values.contains_key(STAGING_ENV));
    assert_eq!(values.get(BUILD_ENV).unwrap().len(), 64);
    // The shim carries an .exe suffix on Windows, so compare stems.
    let wrapper = Path::new(values.get("RUSTC_WRAPPER").unwrap());
    assert_eq!(wrapper.file_stem().unwrap(), RUSTC_SHIM_STEM);
    assert_eq!(values.get(PREVIOUS_RUSTC_WRAPPER_ENV).unwrap(), "existing");
    assert_eq!(values.get(REAL_RUSTDOC_ENV).unwrap(), "custom-rustdoc");
    assert_eq!(
        Path::new(values.get("RUSTDOC").unwrap())
            .file_stem()
            .unwrap(),
        RUSTDOC_SHIM_STEM
    );
    assert_eq!(values.get("CARGO_INCREMENTAL").unwrap(), "0");
    assert_eq!(values.get(VERIFY_ENV).unwrap(), "0");
    assert_eq!(values.get(BUILD_SCRIPT_EXECUTION_ENV).unwrap(), "0");

    session.finish().await.unwrap();
}

#[test]
fn nested_sessions_unwrap_the_outer_rustdoc_shim() {
    let values = BTreeMap::from([
        (
            "RUSTDOC".into(),
            Path::new("outer-session")
                .join(shim_file_name(RUSTDOC_SHIM_STEM))
                .to_string_lossy()
                .into_owned(),
        ),
        (REAL_RUSTDOC_ENV.into(), "custom-rustdoc".into()),
    ]);

    assert_eq!(configured_rustdoc(&values), "custom-rustdoc");
}

/// Build scripts may hand HOST_CC to CMake, which records its absolute path in
/// CMakeCache.txt and reuses it on later cargo invocations. That path must
/// therefore outlive the temporary mbx session that first configured CMake.
#[cfg(unix)]
#[tokio::test]
async fn cc_shim_path_survives_and_is_reused_across_sessions() {
    if resolve_on_path(CcLanguage::C.default_driver()).is_none() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let mut config = test_config(cache.path());
    config.cc = true;

    let first_path = {
        let session_dir = tempfile::tempdir().unwrap();
        let session = CacheSession::start(session_dir.path(), &config)
            .await
            .unwrap();
        let path = session
            .cc_shims
            .as_ref()
            .and_then(|shims| shims.cc.as_ref())
            .map(|(shim, _)| shim.clone())
            .unwrap();
        session.finish().await.unwrap();
        path
    };

    assert!(
        first_path.is_file(),
        "the compiler path cached by a build system must survive its mbx session"
    );

    let second_dir = tempfile::tempdir().unwrap();
    let second = CacheSession::start(second_dir.path(), &config)
        .await
        .unwrap();
    let second_path = second
        .cc_shims
        .as_ref()
        .and_then(|shims| shims.cc.as_ref())
        .map(|(shim, _)| shim.as_path())
        .unwrap();
    assert_eq!(
        second_path, first_path,
        "later sessions must reuse the compiler path a build system cached"
    );
    second.finish().await.unwrap();
}

/// The persistent shim directory is shared by every mbx process using a cache.
/// Concurrent sessions must not contend on the temporary name used to install
/// the same shim.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_sessions_can_install_shared_cc_shims() {
    if resolve_on_path(CcLanguage::C.default_driver()).is_none() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let mut config = test_config(cache.path());
    config.cc = true;

    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut starts = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let config = config.clone();
        let barrier = Arc::clone(&barrier);
        starts.spawn(async move {
            let session_dir = tempfile::tempdir().unwrap();
            barrier.wait().await;
            let session = CacheSession::start(session_dir.path(), &config).await?;
            let shim = session
                .cc_shims
                .as_ref()
                .and_then(|shims| shims.cc.as_ref())
                .map(|(shim, _)| shim.clone())
                .unwrap();
            session.finish().await?;
            Ok::<_, eyre::Report>(shim)
        });
    }

    let mut installed = Vec::new();
    while let Some(result) = starts.join_next().await {
        installed.push(result.unwrap().unwrap());
    }
    assert_eq!(installed.len(), 8);
    assert!(installed.iter().all(|shim| shim == &installed[0]));
    assert!(installed[0].is_file());
}

#[tokio::test]
async fn incremental_builds_leave_cargo_incremental_alone() {
    let cache = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let mut config = test_config(cache.path());
    config.incremental = true;
    let session = CacheSession::start(session_dir.path(), &config)
        .await
        .unwrap();

    let workspace = tempfile::tempdir().unwrap();
    let mut values = BTreeMap::new();
    session
        .begin(
            workspace.path(),
            &workspace.path().join("target"),
            &["build".to_string()],
            &mut values,
        )
        .await;

    // Absent, not "1": cargo's own per-profile default is what we want, and
    // forcing the value on would turn incremental on for release too.
    assert!(!values.contains_key("CARGO_INCREMENTAL"));

    session.finish().await.unwrap();
}

#[tokio::test]
async fn verify_mode_is_passed_to_the_shim() {
    let cache = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let mut config = test_config(cache.path());
    config.verify = true;
    let session = CacheSession::start(session_dir.path(), &config)
        .await
        .unwrap();

    let workspace = tempfile::tempdir().unwrap();
    let mut values = BTreeMap::new();
    session
        .begin(
            workspace.path(),
            &workspace.path().join("target"),
            &["build".to_string()],
            &mut values,
        )
        .await;

    assert_eq!(values.get(VERIFY_ENV).unwrap(), "1");

    session.finish().await.unwrap();
}

#[test]
fn identity_follows_the_dependency_graph_not_the_path() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(first.path().join("Cargo.lock"), "version = 4\n").unwrap();
    std::fs::write(second.path().join("Cargo.lock"), "version = 4\n").unwrap();
    let command = ["build".to_string()];

    assert_eq!(
        build_identity(first.path(), &command),
        build_identity(second.path(), &command),
        "separate worktrees of one project must share a manifest"
    );

    std::fs::write(second.path().join("Cargo.lock"), "version = 3\n").unwrap();
    assert_ne!(
        build_identity(first.path(), &command),
        build_identity(second.path(), &command),
    );
    assert_eq!(
        build_identity(first.path(), &command),
        build_identity(first.path(), &["test".to_string()]),
        "Cargo commands in one dependency graph should share predictions",
    );
}

#[test]
fn identity_falls_back_to_the_directory_name() {
    let directory = tempfile::tempdir().unwrap();
    let command = ["build".to_string()];
    assert_eq!(
        build_identity(directory.path(), &command).len(),
        64,
        "a project without a lockfile still gets an identity"
    );
}

#[test]
fn path_shim_names_select_their_language() {
    assert_eq!(path_shim_language("cc"), Some(CcLanguage::C));
    assert_eq!(path_shim_language("gcc"), Some(CcLanguage::C));
    assert_eq!(path_shim_language("clang"), Some(CcLanguage::C));
    assert_eq!(path_shim_language("c++"), Some(CcLanguage::Cxx));
    assert_eq!(path_shim_language("g++"), Some(CcLanguage::Cxx));
    assert_eq!(path_shim_language("clang++"), Some(CcLanguage::Cxx));
    // A versioned driver was chosen deliberately and is never intercepted.
    assert_eq!(path_shim_language("gcc-13"), None);
    assert_eq!(path_shim_language("mbx"), None);
}

#[test]
fn exec_identity_is_shared_across_checkouts_with_one_lockfile() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(first.path().join("Cargo.lock"), "version = 4\n").unwrap();
    std::fs::write(second.path().join("Cargo.lock"), "version = 4\n").unwrap();
    let command = ["make".to_string()];

    assert_eq!(
        exec_identity(first.path(), &command),
        exec_identity(second.path(), &command),
        "worktrees of one project must share a manifest for predictions to travel"
    );
    assert_ne!(
        exec_identity(first.path(), &command),
        exec_identity(first.path(), &["make".to_string(), "-j8".to_string()]),
    );
}

#[test]
fn exec_identity_falls_back_to_the_directory_name() {
    let directory = tempfile::tempdir().unwrap();
    let command = ["make".to_string()];
    assert_eq!(
        exec_identity(directory.path(), &command).len(),
        64,
        "a project with no lockfile and no git origin still gets an identity"
    );
}

#[test]
/// Jujutsu's remote listing names each remote before its URL.
fn reads_the_origin_from_jujutsu_remote_output() {
    let remotes = "backup ssh://example.com/backup\norigin https://example.com/project.git\n";

    assert_eq!(
        jj_origin_url(remotes),
        Some("https://example.com/project.git")
    );
    assert_eq!(
        jj_origin_url("upstream https://example.com/project.git\n"),
        None
    );
}

#[test]
fn reads_mercurial_and_sapling_default_paths_as_origins() {
    assert_eq!(
        origin_marker_from_output(b"https://example.com/project\n"),
        Some("origin\0https://example.com/project".to_string())
    );
    assert_eq!(origin_marker_from_output(b"\n"), None);
}

#[test]
/// A nested Git checkout must not inherit an enclosing Jujutsu remote.
fn a_nested_git_checkout_does_not_query_jujutsu() {
    let directory = tempfile::tempdir().unwrap();
    let outer = directory.path();
    let inner = outer.join("vendor");
    std::fs::create_dir(&inner).unwrap();
    std::fs::create_dir(outer.join(".jj")).unwrap();
    std::fs::create_dir(inner.join(".git")).unwrap();

    assert_eq!(jj_origin_marker(&inner), None);
}

#[test]
fn native_checkouts_do_not_inherit_an_enclosing_git_origin() {
    for marker in [".hg", ".sl"] {
        let directory = tempfile::tempdir().unwrap();
        let outer = directory.path();
        let inner = outer.join("vendor");
        std::fs::create_dir(&inner).unwrap();
        std::fs::create_dir(inner.join(marker)).unwrap();
        assert!(
            Command::new("git")
                .arg("init")
                .arg("--quiet")
                .arg(outer)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(outer)
                .args(["remote", "add", "origin", "https://example.com/outer.git"])
                .status()
                .unwrap()
                .success()
        );

        assert_eq!(project_origin_marker(&inner), None, "marker: {marker}");
    }
}

#[cfg(unix)]
#[test]
fn a_shim_directory_never_supplies_the_real_compiler() {
    // The shim there stands for a *different* mbx than the one resolving --
    // an upgrade, or another checkout's build -- so no identity check against
    // the running binary can rule it out. Taking it would pin a shim as its
    // own compiler and recurse forever, so the directory is excluded by
    // location.
    let directory = tempfile::tempdir().unwrap();
    let shims = directory.path().join("shims");
    let real_dir = directory.path().join("bin");
    std::fs::create_dir(&shims).unwrap();
    std::fs::create_dir(&real_dir).unwrap();
    let other_mbx = directory.path().join("other-mbx");
    std::fs::write(&other_mbx, b"#!/bin/sh\n").unwrap();
    std::os::unix::fs::symlink(&other_mbx, shims.join("cc")).unwrap();
    let real_cc = real_dir.join("cc");
    std::fs::write(&real_cc, b"#!/bin/sh\n").unwrap();

    let running = directory.path().join("mbx");
    std::fs::write(&running, b"#!/bin/sh\n").unwrap();
    // Handed in rather than set: `PATH` is process global and these tests run
    // on a thread pool, so writing it would race whatever else reads one.
    let path = std::env::join_paths([shims.as_path(), real_dir.as_path()]).unwrap();
    let resolved = resolve_in_path(&path, "cc", &running, &shims);

    assert_eq!(
        resolved.map(|path| std::fs::canonicalize(path).unwrap()),
        Some(std::fs::canonicalize(&real_cc).unwrap()),
        "a shim must never be chosen as the compiler it stands in for"
    );
}

#[cfg(unix)]
#[test]
fn a_hard_linked_shim_is_recognized_as_the_same_binary() {
    // Both files are created here rather than linked from the running test
    // binary: a hard link needs one filesystem, and a temporary directory is
    // on a different one from the build directory often enough that CI proves
    // it. `install_shim_named` falls back to a copy in that case, which would
    // test the fallback rather than the recognition below.
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("mbx");
    std::fs::write(&binary, b"#!/bin/sh\n").unwrap();
    let shim_dir = directory.path().join("cc-path");
    std::fs::create_dir(&shim_dir).unwrap();

    // The case a path comparison cannot see: a shim directory an outer
    // session left on PATH, holding a link to the same binary under a
    // compiler's name.
    let linked = shim_dir.join("cc");
    std::fs::hard_link(&binary, &linked).unwrap();
    assert!(is_same_binary(&linked, Some(&binary)));

    // A copy is a different file, and saying so is correct: the shim resolves
    // its own name away by path first, so recognition here only has to cover
    // the links that share an inode.
    let copied = shim_dir.join("c++");
    std::fs::copy(&binary, &copied).unwrap();
    assert!(!is_same_binary(&copied, Some(&binary)));
}

#[test]
fn handshake_rejects_version_skew() {
    let response = serde_json::to_string(&AgentResponse::Hello {
        protocol: AGENT_PROTOCOL_VERSION,
        agent_version: "another-version".into(),
    })
    .unwrap();
    assert!(validate_handshake_response(&response).is_err());
}

/// Build the statistics a test needs without naming every counter.
///
/// `AgentStats` is `#[non_exhaustive]` so that the agent can keep adding
/// counters, which also means no struct literal can be written from here.
fn agent_stats(fill: impl FnOnce(&mut AgentStats)) -> AgentStats {
    let mut stats = AgentStats::default();
    fill(&mut stats);
    stats
}

#[test]
fn qualification_results_are_not_reported_as_misses() {
    let stats = agent_stats(|stats| {
        stats.lookups = 5;
        stats.hits = 2;
        stats.verifications = 2;
    });
    assert_eq!(cache_misses(&stats), 1);
}

#[test]
fn a_loaded_manifest_that_matched_nothing_is_called_out() {
    // The shape a toolchain update leaves behind: a warm store, a manifest
    // full of predictions, and not one lookup all build.
    let unmatched = agent_stats(|stats| {
        stats.unconsulted = 255;
        stats.predictions_loaded = 257;
    });
    assert!(stale_manifest_note(&unmatched).unwrap().contains("257"));

    // A genuinely cold store has nothing to explain.
    let cold = agent_stats(|stats| stats.unconsulted = 255);
    assert_eq!(stale_manifest_note(&cold), None);

    // A session whose lookups happened was matching its manifest fine; the
    // stragglers are ordinary cold units, not a stale baseline.
    let live = agent_stats(|stats| {
        stats.unconsulted = 2;
        stats.lookups = 253;
        stats.predictions_loaded = 257;
    });
    assert_eq!(stale_manifest_note(&live), None);
}

#[test]
fn compiler_only_sessions_are_reportable() {
    let stats = agent_stats(|stats| {
        stats.compiler =
            BTreeMap::from([("bypass".into(), mbx_cache_core::CompilerStats::new(1, 42))]);
    });

    assert!(should_display_stats(&stats));
}

#[test]
fn a_session_that_only_failed_its_remote_is_reportable() {
    // Nothing was looked up, stored or compiled, so every other signal this
    // gate reads is zero -- and a remote cache that answered nothing but
    // failures is the one thing the build most needs to be told about.
    let stats = agent_stats(|stats| stats.remote_failures = 3);

    assert!(should_display_stats(&stats));
}

#[test]
fn short_summary_omits_routine_compiler_probe_bypasses() {
    let routine = agent_stats(|stats| {
        stats.bypasses = BTreeMap::from([
            ("compiler-query".into(), 2),
            ("standard-input".into(), 1),
            ("cc-compiler-query".into(), 3),
            ("cc-standard-input".into(), 4),
        ]);
    });
    assert!(!should_display_short_stats(&routine));

    let mixed = agent_stats(|stats| {
        stats.lookups = 4;
        stats.hits = 3;
        stats.bypasses = BTreeMap::from([
            ("compiler-query".into(), 2),
            ("standard-input".into(), 1),
            ("cc-compiler-query".into(), 3),
            ("cc-standard-input".into(), 4),
            ("native-library".into(), 5),
        ]);
    });
    let summary = short_summary(&mixed);
    assert!(
        summary.contains("3 hits, 1 misses, 5 bypassed"),
        "{summary}"
    );
    assert!(!summary.contains("compiler-query"), "{summary}");
    assert!(!summary.contains("standard-input"), "{summary}");
}

#[test]
fn an_off_summary_still_writes_the_versioned_stats_report() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested").join("stats.json");
    let stats = agent_stats(|stats| {
        stats.session_duration_ns = 42;
        stats.lookups = 5;
        stats.hits = 2;
        stats.verifications = 1;
        stats.prefetched_actions = 3;
        stats.downloaded_bytes = 1024;
        stats.restored_output_files = 7;
        stats.restored_output_bytes = 2048;
        stats.reflinked_output_files = 5;
        stats.reflinked_output_bytes = 1536;
        stats.copied_output_files = 2;
        stats.copied_output_bytes = 512;
        stats.avoided_compiler_duration_ns = 2_000;
        stats.compiler =
            BTreeMap::from([("miss".into(), mbx_cache_core::CompilerStats::new(3, 4_000))]);
        stats.slow_compilations = BTreeMap::from([("slow_crate".into(), 3_000)]);
        stats.remote_blob_requests = 4;
        stats.remote_blob_pack_requests = 2;
        stats.remote_blob_pack_blobs = 100;
        stats.materialization_duration_ns = 9;
        stats.predictions_loaded = 11;
    });

    let mut config = Config::for_test(directory.path());
    config.stats_report = Some(path.clone());
    display_stats(&stats, &config, SummaryStyle::Off);
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();

    // Bumped whenever the report grows a field, so a reader can tell from the
    // version alone which ones it may expect.
    assert_eq!(report["version"], 4);
    assert_eq!(report["predictions_loaded"], 11);
    assert_eq!(report["session_duration_ns"], 42);
    assert_eq!(report["hits"], 2);
    assert_eq!(report["misses"], 2);
    assert_eq!(report["compiler_invocations_avoided"], 2);
    assert_eq!(report["estimated_compiler_duration_avoided_ns"], 2_000);
    assert_eq!(report["compiler"]["miss"]["invocations"], 3);
    assert_eq!(report["compiler"]["miss"]["duration_ns"], 4_000);
    assert_eq!(report["slow_compilations"][0]["crate_name"], "slow_crate");
    assert_eq!(report["slow_compilations"][0]["duration_ns"], 3_000);
    assert_eq!(report["prefetched_actions"], 3);
    assert_eq!(report["downloaded_bytes"], 1024);
    assert_eq!(report["restored_output_files"], 7);
    assert_eq!(report["restored_output_bytes"], 2048);
    assert_eq!(report["reflinked_output_files"], 5);
    assert_eq!(report["reflinked_output_bytes"], 1536);
    assert_eq!(report["copied_output_files"], 2);
    assert_eq!(report["copied_output_bytes"], 512);
    assert_eq!(report["remote_blob_requests"], 4);
    assert_eq!(report["remote_blob_pack_requests"], 2);
    assert_eq!(report["remote_blob_pack_blobs"], 100);
    assert_eq!(report["materialization_duration_ns"], 9);
}

#[test]
fn finds_crate_names_in_transparent_invocations() {
    assert_eq!(
        crate_name_argument(&["--crate-name".into(), "fixture".into()]),
        Some("fixture".into())
    );
    assert_eq!(
        crate_name_argument(&["--crate-name=attached".into()]),
        Some("attached".into())
    );
    assert_eq!(crate_name_argument(&["--version".into()]), None);
}

#[test]
fn recognizes_the_bypassed_invocations_that_run_a_linker() {
    // Native links bypass the cache today, so this is the only thing that
    // tells the scheduler one of them is about to run.
    for arguments in [
        vec!["--crate-type", "bin"],
        vec!["--crate-type=cdylib"],
        vec!["--crate-type", "lib,dylib"],
        vec!["--crate-type=proc-macro"],
        vec!["--crate-type=staticlib"],
        // A test harness links a program whatever its crate type says.
        vec!["--test", "--crate-type=lib"],
        // The emit that actually links, spelled both ways cargo spells it.
        vec!["--crate-type=bin", "--emit=dep-info,link"],
        vec!["--test", "--emit", "link=/tmp/out"],
    ] {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        assert!(links_natively(&arguments), "{arguments:?} links");
    }

    for arguments in [
        vec!["--crate-type", "lib"],
        vec!["--crate-type=rlib"],
        vec!["--crate-type", "lib,rlib"],
        vec!["--emit=metadata"],
        // The flag's own name is not its value: a crate called "bin" is not
        // a program.
        vec!["--crate-name", "bin"],
        // What `cargo check` and `clippy --all-targets` run: the same binary
        // and test targets, compiled to metadata, with no linker anywhere.
        vec!["--crate-type=bin", "--emit=metadata"],
        vec!["--test", "--emit", "dep-info,metadata"],
        vec!["--crate-type=cdylib", "--emit=dep-info,metadata"],
    ] {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        assert!(!links_natively(&arguments), "{arguments:?} does not link");
    }
}

/// The session shim must be a symlink, not a hard link.
///
/// Cargo execs it within milliseconds of its creation, and a hard link that new
/// is not reliably runnable on macOS -- see [`install_shim`]. Asserted on the
/// kind of link rather than by racing the kernel, which no test can do
/// dependably.
#[cfg(unix)]
#[test]
fn the_session_shim_tracks_the_binary_it_was_installed_from() {
    let directory = tempfile::tempdir().unwrap();
    let executable = std::env::current_exe().unwrap();
    let shim = install_shim(&executable, directory.path(), ShimLink::Tracking).unwrap();

    let metadata = std::fs::symlink_metadata(&shim).unwrap();
    assert!(
        metadata.file_type().is_symlink(),
        "the session shim should be a symlink, found {metadata:?}"
    );
    assert_eq!(std::fs::read_link(&shim).unwrap(), executable);
}

/// The `mbx setup` wrapper keeps the bytes it was installed from.
///
/// A symlink there would break the moment the binary it names is deleted, and
/// nothing execs it soon enough for the race to matter.
#[cfg(unix)]
#[test]
fn the_installed_wrapper_pins_the_bytes_it_was_made_from() {
    let directory = tempfile::tempdir().unwrap();
    let executable = std::env::current_exe().unwrap();
    let shim = install_shim(&executable, directory.path(), ShimLink::Pinned).unwrap();

    assert!(
        !std::fs::symlink_metadata(&shim)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // Length rather than contents: the point is that the shim is a file of its
    // own and not a name that can dangle, and reading the binary twice to prove
    // it costs a hundred megabytes.
    assert_eq!(
        std::fs::metadata(&shim).unwrap().len(),
        std::fs::metadata(&executable).unwrap().len()
    );
}

/// No shim is ever installed as a link to nothing.
///
/// A symlink can name a target that does not exist, so the tracking path has to
/// decline one; the hard link it falls back to then fails where the mistake was
/// made rather than when cargo execs the wrapper.
#[cfg(unix)]
#[test]
fn a_shim_is_never_installed_as_a_link_to_nothing() {
    let directory = tempfile::tempdir().unwrap();
    for missing in [directory.path().join("gone"), PathBuf::from("relative/mbx")] {
        assert!(
            install_shim(&missing, directory.path(), ShimLink::Tracking).is_err(),
            "{} should not install a shim",
            missing.display()
        );
    }
}

/// A relative target is resolved before it is linked, not after.
///
/// A symlink resolves its target from the shim's own directory, and the session
/// shim's directory is a temporary one that shares nothing with the caller's, so
/// the relative target a hard link would have read from the working directory
/// has to be resolved against that directory here. `Cargo.toml` stands in for
/// the binary: under test is how the path is read, not what it points at.
#[cfg(unix)]
#[test]
fn a_relative_target_is_resolved_before_it_is_linked() {
    let directory = tempfile::tempdir().unwrap();
    let shim = directory.path().join(RUSTC_SHIM_STEM);
    assert!(symlink_shim(Path::new("Cargo.toml"), &shim));

    let target = std::fs::read_link(&shim).unwrap();
    assert!(
        target.is_absolute(),
        "{} should be absolute",
        target.display()
    );
    // Resolves through the link, which it could not if the target had been left
    // relative: the shim's own directory holds no `Cargo.toml`.
    assert_eq!(
        std::fs::canonicalize(&shim).unwrap(),
        std::fs::canonicalize("Cargo.toml").unwrap()
    );
}

/// An image with a C compiler and no C++ one is ordinary, and it must not cost
/// a C-only sys-crate its caching.
#[test]
fn a_missing_cpp_compiler_still_leaves_c_compilations_cached() {
    let shims = CcShims {
        cc: Some((
            PathBuf::from("/session/mbx-cc"),
            PathBuf::from("/usr/bin/cc"),
        )),
        cxx: None,
        targeted: Vec::new(),
    };
    let mut environment = BTreeMap::new();
    shims.apply_host(&mut environment);

    assert_eq!(
        environment.get("HOST_CC").map(String::as_str),
        Some("/session/mbx-cc")
    );
    assert_eq!(
        environment.get("MBX_REAL_CC").map(String::as_str),
        Some("/usr/bin/cc")
    );
    // Nothing is claimed for the language that has no compiler, so the `cc`
    // crate keeps whatever it would have chosen for C++.
    assert!(!environment.contains_key("HOST_CXX"));
    assert!(!environment.contains_key("MBX_REAL_CXX"));
}

/// Both present is the ordinary case, and both get pointed at their shim.
#[test]
fn both_compilers_present_are_both_redirected() {
    let shims = CcShims {
        cc: Some((
            PathBuf::from("/session/mbx-cc"),
            PathBuf::from("/usr/bin/cc"),
        )),
        cxx: Some((
            PathBuf::from("/session/mbx-cxx"),
            PathBuf::from("/usr/bin/c++"),
        )),
        targeted: Vec::new(),
    };
    let mut environment = BTreeMap::new();
    shims.apply_host(&mut environment);
    for name in ["HOST_CC", "HOST_CXX", "MBX_REAL_CC", "MBX_REAL_CXX"] {
        assert!(environment.contains_key(name), "{name} should be set");
    }
}

/// The `cc` crate names a compiler for a target in four ways, and only those
/// four should be wrapped -- `CCACHE_DIR` and friends merely start with the
/// same letters.
#[test]
fn only_the_cc_crates_target_variables_name_a_cross_compiler() {
    use CcLanguage::{C, Cxx};
    for (variable, expected) in [
        ("TARGET_CC", Some(C)),
        ("TARGET_CXX", Some(Cxx)),
        ("CC_aarch64-unknown-linux-musl", Some(C)),
        ("CC_aarch64_unknown_linux_musl", Some(C)),
        ("CXX_aarch64-unknown-linux-musl", Some(Cxx)),
        ("CC", None),
        ("CXX", None),
        ("HOST_CC", None),
        ("CC_", None),
        ("CCACHE_DIR", None),
        ("CXXFLAGS", None),
        // The `cc` crate hangs its own controls off the same prefix, and
        // autotools adds one of its own. Redirecting any of them would answer
        // a question the build asked with a compiler path.
        ("CC_FORCE_DISABLE", None),
        ("CC_KNOWN_WRAPPER_CUSTOM", None),
        ("CC_ENABLE_DEBUG_OUTPUT", None),
        ("CC_FOR_BUILD", None),
        ("CXX_FOR_BUILD", None),
        // A bare word is not a triple either.
        ("CC_gcc", None),
    ] {
        assert_eq!(
            targeted_compiler_language(variable).map(|l| format!("{l:?}")),
            expected.map(|l| format!("{l:?}")),
            "{variable}"
        );
    }
}

/// A build already pointed at a shim -- an outer session's, or this one's --
/// must not have a second shim put in front of it, or the inner one execs
/// itself.
#[test]
fn a_compiler_that_is_already_a_shim_is_not_wrapped_again() {
    let executable = std::env::current_exe().expect("current exe");
    let shims = tempfile::tempdir().expect("tempdir");
    let planted = shims.path().join("aarch64-linux-musl-gcc");
    std::fs::write(&planted, "#!/bin/sh\nexit 0\n").expect("write shim");

    assert_eq!(
        resolve_named_compiler(&planted.display().to_string(), &executable, shims.path()),
        None,
        "a compiler inside the shim directory is a shim, not a compiler"
    );
}

/// A cross image is entitled to ship the driver it cross-compiles with and no
/// host `cc` at all, and that build is exactly the one this wrapping exists
/// for.
#[test]
fn a_cross_only_image_still_gets_its_named_compiler_wrapped() {
    let shims = CcShims {
        cc: None,
        cxx: None,
        targeted: vec![TargetedCompiler {
            variable: "CC_aarch64-unknown-linux-musl".into(),
            shim_name: "mbx-cc-cc_aarch64-unknown-linux-musl".into(),
            shim: PathBuf::from("/session/mbx-cc-cc_aarch64-unknown-linux-musl"),
            real: PathBuf::from("/usr/bin/aarch64-linux-musl-gcc"),
        }],
    };
    let mut environment = BTreeMap::new();
    shims.apply_host(&mut environment);
    shims.apply_targeted(&mut environment);

    // Nothing is claimed for a host compiler that is not there...
    assert!(!environment.contains_key("HOST_CC"));
    // ...and the cross one is still wrapped.
    assert_eq!(
        environment
            .get("CC_aarch64-unknown-linux-musl")
            .map(String::as_str),
        Some("/session/mbx-cc-cc_aarch64-unknown-linux-musl")
    );
    assert!(!shims.pins().is_empty());
}

/// A value that is a command rather than a path is left alone: wrapping it
/// would mean running the first word and dropping the rest.
#[test]
fn a_compiler_named_as_a_command_is_not_wrapped() {
    let executable = std::env::current_exe().expect("current exe");
    let shims = tempfile::tempdir().expect("tempdir");
    for value in ["ccache gcc", "", "   ", "cc -m32"] {
        assert_eq!(
            resolve_named_compiler(value, &executable, shims.path()),
            None,
            "{value:?} is not a single executable"
        );
    }
}

/// A build that named its own cross compiler still gets it wrapped, even
/// though it named its host compiler too and mbx stood aside for that one.
#[test]
fn naming_a_host_compiler_does_not_cost_the_cross_one_its_shim() {
    let shims = CcShims {
        cc: Some((
            PathBuf::from("/session/mbx-cc"),
            PathBuf::from("/usr/bin/cc"),
        )),
        cxx: None,
        targeted: vec![TargetedCompiler {
            variable: "CC_aarch64-unknown-linux-musl".into(),
            shim_name: "mbx-cc-cc_aarch64-unknown-linux-musl".into(),
            shim: PathBuf::from("/session/mbx-cc-cc_aarch64-unknown-linux-musl"),
            real: PathBuf::from("/usr/bin/aarch64-linux-musl-gcc"),
        }],
    };
    let mut environment = BTreeMap::new();
    shims.apply_targeted(&mut environment);

    assert_eq!(
        environment
            .get("CC_aarch64-unknown-linux-musl")
            .map(String::as_str),
        Some("/session/mbx-cc-cc_aarch64-unknown-linux-musl")
    );
    // Standing aside for the host pair must not have been applied here.
    assert!(!environment.contains_key("HOST_CC"));
    // The shim finds its compiler by the name it is invoked under.
    assert_eq!(
        shims.pins().get("mbx-cc-cc_aarch64-unknown-linux-musl"),
        Some(&PathBuf::from("/usr/bin/aarch64-linux-musl-gcc"))
    );
}

/// A targeted shim has to be recognised as one, or it would exec nothing.
#[test]
fn targeted_shim_names_dispatch_to_their_language() {
    for (stem, expected) in [
        ("mbx-cc-cc_aarch64-unknown-linux-musl", "C"),
        ("mbx-cxx-cxx_aarch64-unknown-linux-musl", "Cxx"),
        ("mbx-cc-target_cc", "C"),
        ("mbx-cxx-target_cxx", "Cxx"),
    ] {
        let language = if stem.starts_with("mbx-cxx-") {
            CcLanguage::Cxx
        } else {
            CcLanguage::C
        };
        assert_eq!(format!("{language:?}"), expected, "{stem}");
    }
}
