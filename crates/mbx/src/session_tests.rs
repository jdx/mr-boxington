use super::*;

fn test_config(cache_dir: &Path) -> Config {
    Config {
        cache_dir: cache_dir.to_path_buf(),
        stats_report: None,
        verify: false,
        incremental: false,
        share_out_dir: false,
        events: false,
        cc: false,
        remote: Default::default(),
        http: Default::default(),
        gc: Default::default(),
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
    let mut values = BTreeMap::from([("RUSTC_WRAPPER".into(), "existing".into())]);
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
    assert_eq!(values.get("CARGO_INCREMENTAL").unwrap(), "0");
    assert_eq!(values.get(VERIFY_ENV).unwrap(), "0");

    session.finish().await.unwrap();
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
    assert_ne!(
        build_identity(first.path(), &command),
        build_identity(first.path(), &["test".to_string()]),
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
fn writes_versioned_stats_report() {
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
    });

    write_stats_report(&path, &stats).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();

    assert_eq!(report["version"], 2);
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
