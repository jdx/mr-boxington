use super::*;

#[test]
fn action_diagnostics_name_key_parts_without_retaining_their_values() {
    let bytes = br#"{"adapter_version":1,"arguments":["--crate-name=example","--codegen=metadata=unit-a","--codegen=opt-level=2","--cfg=feature=\"secret-feature\""],"compiler":{"host":"host","rustc_version":"version","toolchain":"toolchain"},"environment":{"SECRET":"do-not-record"},"inputs":[{"digest":{"algorithm":"blake3","hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":7},"path":"${workspace}/src/lib.rs"}],"kind":"rustc","version":1}"#.to_vec();
    let diagnostic = action_diagnostic(
        &RustcAction {
            digest: CacheDigest::blake3(&bytes),
            bytes,
        },
        "${workspace}/src/lib.rs",
    )
    .unwrap();

    assert!(diagnostic.components.contains_key("compiler toolchain"));
    assert!(diagnostic.components.contains_key("compilation unit"));
    assert!(
        diagnostic
            .components
            .contains_key("argument --codegen opt-level")
    );
    assert!(diagnostic.components.contains_key("environment SECRET"));
    assert!(diagnostic.inputs.contains_key("${workspace}/src/lib.rs"));
    let recorded = serde_json::to_string(&diagnostic).unwrap();
    assert!(!recorded.contains("do-not-record"));
    assert!(!recorded.contains("secret-feature"));
}

#[test]
fn cargo_metadata_changes_are_diffs_within_one_compilation_unit() {
    let action = |metadata: &str| {
        let bytes = format!(
            r#"{{"adapter_version":1,"arguments":["--crate-name=example","--crate-type=lib","--codegen=metadata={metadata}"],"compiler":{{"host":"host","rustc_version":"version","toolchain":"toolchain"}},"environment":{{}},"inputs":[],"kind":"rustc","version":1}}"#
        )
        .into_bytes();
        action_diagnostic(
            &RustcAction {
                digest: CacheDigest::blake3(&bytes),
                bytes,
            },
            "${workspace}/src/lib.rs",
        )
        .unwrap()
    };

    let previous = action("old");
    let current = action("new");
    assert_eq!(
        previous.components["compilation unit"],
        current.components["compilation unit"]
    );
    assert_ne!(
        previous.components["argument --codegen metadata"],
        current.components["argument --codegen metadata"]
    );
}
use crate::materialize::{apply_file_mode, make_owner_writable};
use std::io::Write as _;

fn portable_for(values: &[&str]) -> Portable {
    Portable {
        mappings: Vec::new(),
        arguments: Vec::new(),
        names: values.iter().map(|_| "OUT_DIR".to_string()).collect(),
        values: values.iter().map(|value| (*value).to_string()).collect(),
    }
}

fn churn(sources: &str, streak: u32) -> ChurnState {
    ChurnState {
        version: CHURN_STATE_VERSION,
        sources: CacheDigest::blake3(sources.as_bytes()).key(),
        streak,
    }
}

/// The streak is what separates a crate someone is editing from one that merely
/// lost its result, and it only counts while that crate's own sources move.
#[test]
fn only_a_run_of_changed_sources_earns_incremental_state() {
    let now = CacheDigest::blake3(b"current sources");

    // Nothing recorded here yet: a first compilation in a checkout is not
    // evidence of anything, and neither is a wiped target directory.
    assert_eq!(
        learned_plan(None, &now, true, HOT_STREAK_THRESHOLD).streak,
        0
    );

    // Recorded against the sources this compilation already has, so nobody
    // edited it -- something else lost the result, and recompiling normally
    // restores it for everyone.
    let unchanged = churn("current sources", HOT_STREAK_THRESHOLD);
    let plan = learned_plan(Some(&unchanged), &now, true, HOT_STREAK_THRESHOLD);
    assert_eq!(plan.streak, 0);
    assert!(!plan.hot);

    // Changed sources climb the streak, and only its last step is hot.
    for previous in 0..HOT_STREAK_THRESHOLD - 1 {
        let recorded = churn("older sources", previous);
        let plan = learned_plan(Some(&recorded), &now, true, HOT_STREAK_THRESHOLD);
        assert_eq!(plan.streak, previous + 1);
        assert!(!plan.hot);
    }
    let recorded = churn("older sources", HOT_STREAK_THRESHOLD - 1);
    assert!(learned_plan(Some(&recorded), &now, true, HOT_STREAK_THRESHOLD).hot);

    // The streak is a state rather than a tally, so it stops at the threshold.
    let saturated = churn("older sources", HOT_STREAK_THRESHOLD);
    assert_eq!(
        learned_plan(Some(&saturated), &now, true, HOT_STREAK_THRESHOLD).streak,
        HOT_STREAK_THRESHOLD
    );

    // Disabled, the streak is still tracked so that enabling it later works.
    let plan = learned_plan(Some(&saturated), &now, false, HOT_STREAK_THRESHOLD);
    assert_eq!(plan.streak, HOT_STREAK_THRESHOLD);
    assert!(!plan.hot);
}

#[test]
fn one_changed_workspace_source_is_hot() {
    let now = CacheDigest::blake3(b"current sources");
    let recorded = churn("older sources", 0);

    let plan = learned_plan(Some(&recorded), &now, true, WORKSPACE_HOT_STREAK_THRESHOLD);

    assert_eq!(plan.streak, 1);
    assert!(plan.hot);
}

/// A marker names the artifact it was written for and is withdrawn before the
/// artifact is replaced, so only what a hot compilation wrote reads as private.
#[test]
fn private_artifacts_are_recognized_only_while_marked() {
    let root = tempfile::tempdir().unwrap();
    let deps = root.path().join("target/debug/deps");
    let outputs = RustcOutputs {
        directory: deps.clone(),
        files: vec![deps.join("libbase-1.rlib"), deps.join("libbase-1.rmeta")],
        dep_info: deps.join("base-1.d"),
    };
    let links = vec![
        root.path().join("above/src/lib.rs"),
        deps.join("libbase-1.rmeta"),
    ];
    assert!(!links_private_artifact_in(root.path(), &links));

    record_private_artifacts(root.path(), &outputs).unwrap();
    assert!(links_private_artifact_in(root.path(), &links));
    assert!(
        !links_private_artifact_in(root.path(), &[deps.join("libother-1.rlib")]),
        "a marker must not vouch for an artifact it was not written for"
    );

    forget_private_artifacts(root.path(), &outputs);
    assert!(!links_private_artifact_in(root.path(), &links));
    forget_private_artifacts(root.path(), &outputs);
}

/// The record belongs to one checkout, so a sibling worktree that cannot read
/// it simply starts over rather than inheriting somebody else's edit loop.
#[test]
fn an_unreadable_record_is_no_record() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let sources = CacheDigest::blake3(b"sources");

    write_churn_state(&path, &sources, 2).unwrap();
    assert_eq!(read_churn_state(&path).unwrap().streak, 2);

    std::fs::write(&path, br#"{"version":99,"sources":"x","streak":3}"#).unwrap();
    assert!(read_churn_state(&path).is_none());

    std::fs::write(&path, b"not json").unwrap();
    assert!(read_churn_state(&path).is_none());
}

#[test]
fn compiler_timing_survives_a_changed_action_key() {
    let invocation = CacheDigest::blake3(b"invocation");
    let timing = RustcInputPrediction {
        version: 3,
        inputs: Vec::new(),
        environment: Vec::new(),
        compiler_duration_ns: 42,
        crate_name: "demo".into(),
    };
    let prediction = ActionPrediction {
        invocation: invocation.clone(),
        action: CacheDigest::blake3(b"old action"),
        adapter: "rustc".into(),
        payload: String::from_utf8(canonical_json(&timing).unwrap()).unwrap(),
    };

    let decoded = decode_prediction_timing(&prediction, &invocation).unwrap();
    assert_eq!(decoded.crate_name, "demo");
    assert_eq!(decoded.duration_ns, 42);
}

#[test]
fn prediction_v1_does_not_supply_timing() {
    let invocation = CacheDigest::blake3(b"invocation");
    let timing = RustcInputPrediction {
        version: 1,
        inputs: Vec::new(),
        environment: Vec::new(),
        compiler_duration_ns: 42,
        crate_name: "demo".into(),
    };
    let prediction = ActionPrediction {
        invocation: invocation.clone(),
        action: CacheDigest::blake3(b"old action"),
        adapter: "rustc".into(),
        payload: String::from_utf8(canonical_json(&timing).unwrap()).unwrap(),
    };

    assert!(decode_prediction_timing(&prediction, &invocation).is_err());
}

/// `--remap-path-prefix` covers the paths rustc writes itself, so most
/// artifacts come out clean. A crate that keeps the value as a string does
/// not, and that is the case the outputs are read to catch.
#[test]
fn an_output_carrying_a_normalized_value_is_not_portable() {
    let root = tempfile::tempdir().unwrap();
    let out_dir = "/checkout/target/debug/build/widget-abc/out";
    let clean = root.path().join("clean.rlib");
    std::fs::write(
        &clean,
        b"rustc output naming ${target}/debug/build/widget-abc/out",
    )
    .unwrap();
    let carries = root.path().join("carries.rlib");
    std::fs::write(&carries, format!("compiled in {out_dir} at some offset")).unwrap();

    let portable = portable_for(&[out_dir]);
    assert!(portable.contents_are_clean(&std::fs::read(&clean).unwrap()));
    assert!(!portable.contents_are_clean(&std::fs::read(&carries).unwrap()));
    // One dirty output is enough: the artifact is published as a set.
    assert!(
        ![clean, carries]
            .iter()
            .all(|output| portable.contents_are_clean(&std::fs::read(output).unwrap()))
    );
}

/// Nothing was made portable, so there is no portable key to publish under
/// and no claim to check.
#[test]
fn nothing_portable_is_never_clean() {
    assert!(!portable_for(&[]).contents_are_clean(b"an artifact"));
}

#[test]
fn a_value_is_found_at_any_offset_and_in_either_spelling() {
    assert!(carries(b"/a/b", "/a/b"));
    assert!(carries(b"...../a/b.....", "/a/b"));
    assert!(carries(b"/a/a/b", "/a/b"));
    assert!(!carries(b"/a/", "/a/b"));
    assert!(!carries(b"", "/a/b"));
    // A Windows value may have been written with forward slashes.
    assert!(carries(b"c:/a/b", "c:\\a\\b"));
    assert!(!carries(b"c:/a/c", "c:\\a\\b"));
}

fn staged_outputs(root: &Path, entries: Vec<(&[u8], PathBuf)>) -> StagedOutputs {
    let directory = tempfile::tempdir_in(root).unwrap();
    let files = entries
        .into_iter()
        .enumerate()
        .map(|(index, (contents, destination))| {
            let path = directory.path().join(format!("output-{index}"));
            std::fs::write(&path, contents).unwrap();
            (
                tempfile::TempPath::try_from_path(path).unwrap(),
                destination,
            )
        })
        .collect();
    StagedOutputs { directory, files }
}

fn test_outputs(root: &Path) -> RustcOutputs {
    let directory = root.join("out");
    RustcOutputs {
        files: vec![directory.join("libdemo.rlib")],
        dep_info: directory.join("demo.d"),
        directory,
    }
}

fn test_file(name: &str) -> CacheFileNode {
    CacheFileNode {
        digest: CacheDigest::blake3(b"artifact"),
        executable: false,
        mode: if cfg!(unix) { 0o644 } else { 0 },
        name: name.into(),
    }
}

fn test_directory(files: Vec<CacheFileNode>) -> CacheDirectory {
    CacheDirectory {
        directories: Vec::new(),
        files,
        symlinks: Vec::new(),
        version: 1,
    }
}

fn test_output_directory(file: CacheFileNode) -> CacheDirectory {
    test_directory(vec![file, test_file("demo.d")])
}

#[test]
fn parses_verbose_rustc_identity() {
    let verbose = "rustc 1.97.0 (abc 2026-08-01)\n\
                       binary: rustc\n\
                       commit-hash: abc\n\
                       commit-date: 2026-08-01\n\
                       host: x86_64-unknown-linux-gnu\n\
                       release: 1.97.0\n\
                       LLVM version: 22.0.0\n";
    assert_eq!(identity_field(verbose, "release").unwrap(), "1.97.0");
    assert_eq!(
        identity_field(verbose, "host").unwrap(),
        "x86_64-unknown-linux-gnu"
    );
}

#[test]
fn mappings_do_not_duplicate_home_placeholders() {
    let directory = tempfile::tempdir().unwrap();
    let mappings = path_mappings(directory.path(), None, None);
    let placeholders = mappings
        .iter()
        .map(|mapping| &mapping.placeholder)
        .collect::<BTreeSet<_>>();
    assert_eq!(placeholders.len(), mappings.len());
}

#[test]
fn standalone_workspace_mapping_wins_beneath_home() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let workspace = home.join("src/project");
    let mappings = path_mappings_with_env(&workspace, None, None, |name| match name {
        "HOME" => Some(home.as_os_str().to_owned()),
        _ => None,
    });

    assert!(
        mappings
            .iter()
            .any(|mapping| { mapping.placeholder == "workspace" && mapping.root == workspace })
    );
    assert!(
        mappings
            .iter()
            .any(|mapping| mapping.placeholder == "home" && mapping.root == home)
    );
}

#[test]
fn standalone_workspace_mapping_uses_the_outer_workspace_for_members() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let member = workspace.join("crates/widget");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(workspace.join("Cargo.lock"), "").unwrap();
    std::fs::write(member.join("Cargo.toml"), "[package]\nname = \"widget\"\n").unwrap();

    let mappings = path_mappings_with_env(&member, None, None, |_| None);

    assert!(
        mappings
            .iter()
            .any(|mapping| { mapping.placeholder == "workspace" && mapping.root == workspace })
    );
}

#[test]
fn standalone_registry_mapping_uses_the_default_cargo_home() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let dependency = home.join(".cargo/registry/src/index/widget-1.0.0");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::write(
        dependency.join("Cargo.toml"),
        "[package]\nname = \"widget\"\n",
    )
    .unwrap();

    let mappings = path_mappings_with_env(&dependency, None, None, |name| match name {
        "HOME" => Some(home.as_os_str().to_owned()),
        _ => None,
    });

    assert!(mappings.iter().any(|mapping| {
        mapping.placeholder == "cargo_home" && mapping.root == home.join(".cargo")
    }));
    assert!(mappings.iter().any(|mapping| {
        mapping.placeholder == "cargo_registry" && mapping.root == home.join(".cargo/registry")
    }));
    assert!(
        !mappings
            .iter()
            .any(|mapping| mapping.placeholder == "workspace")
    );
}

#[cfg(unix)]
#[test]
fn cargo_registry_mapping_follows_a_child_symlink() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let cargo_home = directory.path().join("cargo-home");
    let physical_registry = directory.path().join("host-cargo-registry");
    std::fs::create_dir_all(&cargo_home).unwrap();
    std::fs::create_dir_all(&physical_registry).unwrap();
    symlink(&physical_registry, cargo_home.join("registry")).unwrap();
    let source = physical_registry.join("src/index/widget-1.0.0/src/lib.rs");

    let mappings = PathMapping::ordered(&path_mappings_with_env(
        directory.path(),
        None,
        None,
        |name| match name {
            "CARGO_HOME" => Some(cargo_home.as_os_str().to_owned()),
            _ => None,
        },
    ));

    assert_eq!(
        normalize_mapped_path(&source, directory.path(), &mappings).unwrap(),
        "${cargo_registry}/src/index/widget-1.0.0/src/lib.rs"
    );
}

#[test]
fn standalone_target_mapping_covers_the_profile_tree() {
    assert_eq!(
        standalone_target_root(Path::new("/tmp/target/debug/deps"), None),
        Path::new("/tmp/target")
    );
    assert_eq!(
        standalone_target_root(
            Path::new("/tmp/target/x86_64-unknown-linux-gnu/release/deps"),
            Some("x86_64-unknown-linux-gnu"),
        ),
        Path::new("/tmp/target")
    );
    assert_eq!(
        standalone_target_root(
            Path::new("/tmp/target/custom/release/deps"),
            Some("/tmp/targets/custom.json"),
        ),
        Path::new("/tmp/target")
    );
}

#[test]
fn validates_exact_rustc_output_set() {
    let root = tempfile::tempdir().unwrap();
    let outputs = test_outputs(root.path());
    let files =
        validated_outputs(test_output_directory(test_file("libdemo.rlib")), &outputs).unwrap();

    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|(_, path)| path == &outputs.files[0]));
    assert!(files.iter().any(|(_, path)| path == &outputs.dep_info));
}

#[test]
fn rejects_cached_output_path_traversal() {
    let root = tempfile::tempdir().unwrap();
    let outputs = test_outputs(root.path());
    assert!(
        validated_outputs(
            test_output_directory(test_file("../libdemo.rlib")),
            &outputs,
        )
        .is_err()
    );
}

#[test]
fn rejects_executable_rustc_outputs() {
    let root = tempfile::tempdir().unwrap();
    let outputs = test_outputs(root.path());
    let mut file = test_file("libdemo.rlib");
    file.executable = true;
    assert!(validated_outputs(test_output_directory(file), &outputs).is_err());
}

#[test]
fn accepts_wasm_executable_rustc_outputs() {
    let root = tempfile::tempdir().unwrap();
    let mut outputs = test_outputs(root.path());
    outputs.files = vec![outputs.directory.join("demo.wasm")];
    let mut file = test_file("demo.wasm");
    file.executable = true;

    assert!(validated_outputs(test_output_directory(file), &outputs).is_ok());
}

/// A native program has no extension to recognize it by, so the contract has to
/// be what the invocation declared rather than what the name looks like.
#[test]
fn accepts_native_executable_rustc_outputs() {
    let root = tempfile::tempdir().unwrap();
    let mut outputs = test_outputs(root.path());
    outputs.files = vec![outputs.directory.join("demo-abc123")];
    let mut file = test_file("demo-abc123");
    file.executable = true;

    assert!(validated_outputs(test_output_directory(file), &outputs).is_ok());

    // A library artifact under the same roof is still not a program.
    outputs.files = vec![outputs.directory.join("libdemo.rlib")];
    let mut library = test_file("libdemo.rlib");
    library.executable = true;
    assert!(validated_outputs(test_output_directory(library), &outputs).is_err());
}

#[cfg(unix)]
#[test]
fn restores_declared_executable_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::write(&source, b"wasm").unwrap();
    let node = CacheFileNode {
        digest: CacheDigest::blake3(b"wasm"),
        executable: true,
        mode: 0o644,
        name: "fixture.wasm".into(),
    };

    let (staged, _) = stage_verified_cached_output(root.path(), 0, &source, &node).unwrap();
    assert_eq!(
        std::fs::metadata(staged).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn rejects_group_or_world_writable_rustc_outputs() {
    let root = tempfile::tempdir().unwrap();
    let outputs = test_outputs(root.path());
    let mut file = test_file("libdemo.rlib");
    file.mode = 0o666;
    assert!(validated_outputs(test_output_directory(file), &outputs).is_err());
}

#[cfg(unix)]
#[test]
fn publication_masks_unsafe_rustc_output_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o666))
        .unwrap();
    assert_eq!(file_mode(&file.as_file().metadata().unwrap()), 0o644);
}

#[test]
fn rolls_back_outputs_after_a_partial_persist() {
    let root = tempfile::tempdir().unwrap();
    let first_destination = root.path().join("first.rlib");
    let blocked_destination = root.path().join("blocked.rmeta");
    std::fs::create_dir(&blocked_destination).unwrap();
    let staged = staged_outputs(
        root.path(),
        vec![
            (b"first", first_destination.clone()),
            (b"second", blocked_destination.clone()),
        ],
    );

    assert!(persist_outputs(staged).is_err());
    assert!(!first_destination.exists());
    assert!(blocked_destination.is_dir());
}

#[test]
fn qualification_does_not_publish_cached_outputs() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("cached.rlib");
    let staged = staged_outputs(root.path(), vec![(b"cached", destination.clone())]);

    finalize_restored_outputs(staged, false).unwrap();

    assert!(!destination.exists());
}

#[test]
fn materialized_outputs_are_independent_from_the_cas() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("cas-blob");
    std::fs::write(&source, b"artifact").unwrap();
    let staging = tempfile::tempdir_in(root.path()).unwrap();
    let node = test_file("artifact.rlib");

    let (output, _) = stage_verified_cached_output(staging.path(), 0, &source, &node).unwrap();
    std::fs::write(&output, b"modified").unwrap();

    assert_eq!(std::fs::read(source).unwrap(), b"artifact");
    assert_eq!(std::fs::read(output).unwrap(), b"modified");
}

#[test]
fn rejects_cached_outputs_with_the_wrong_size() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("cas-blob");
    std::fs::write(&source, b"short").unwrap();
    let staging = tempfile::tempdir_in(root.path()).unwrap();
    let node = test_file("artifact.rlib");

    assert!(stage_verified_cached_output(staging.path(), 0, &source, &node).is_err());
}

#[test]
fn materializes_read_only_cached_outputs() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("cas-blob");
    std::fs::write(&source, b"artifact").unwrap();
    let mut permissions = std::fs::metadata(&source).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&source, permissions).unwrap();
    let staging = tempfile::tempdir_in(root.path()).unwrap();
    let node = test_file("artifact.rlib");

    let (output, _) = stage_verified_cached_output(staging.path(), 0, &source, &node).unwrap();

    assert_eq!(std::fs::read(output).unwrap(), b"artifact");
    assert!(std::fs::metadata(&source).unwrap().permissions().readonly());
    make_owner_writable(&source).unwrap();
}

#[test]
fn rejects_same_size_corrupt_cached_metadata() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("cas-blob");
    std::fs::write(&source, b"corrupt!").unwrap();
    let digest = CacheDigest::blake3(b"artifact");

    assert!(read_verified_blob(&source, &digest, "test blob").is_err());
}

#[test]
#[ignore = "local materialization benchmark"]
fn benchmark_cached_output_materialization() {
    let size_mib = std::env::var("MBX_BENCH_MIB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(128);
    let iterations = std::env::var("MBX_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4);
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("cas-blob");
    let mut source_file = std::fs::File::create(&source).unwrap();
    let chunk = vec![0x5a; 1024 * 1024];
    for _ in 0..size_mib {
        source_file.write_all(&chunk).unwrap();
    }
    source_file.sync_all().unwrap();
    drop(source_file);
    let digest = CacheDigest::blake3_file(&source).unwrap();
    let node = CacheFileNode {
        digest: digest.clone(),
        executable: false,
        mode: if cfg!(unix) { 0o644 } else { 0 },
        name: "artifact.rlib".into(),
    };

    let staging = tempfile::tempdir_in(root.path()).unwrap();
    let started = std::time::Instant::now();
    for _ in 0..iterations {
        let temporary = staging.path().join("legacy-output");
        reflink_copy::reflink_or_copy(&source, &temporary).unwrap();
        let temporary = tempfile::TempPath::try_from_path(temporary).unwrap();
        make_owner_writable(&temporary).unwrap();
        assert!(digest.matches_file(&temporary).unwrap());
        apply_file_mode(&temporary, node.mode, node.executable).unwrap();
    }
    let legacy = started.elapsed();

    let staging = tempfile::tempdir_in(root.path()).unwrap();
    let started = std::time::Instant::now();
    let mut method = None;
    for _ in 0..iterations {
        let (_, observed) =
            stage_verified_cached_output(staging.path(), 0, &source, &node).unwrap();
        method = Some(observed);
    }
    let materialized = started.elapsed();

    println!(
        "materialized {iterations} x {size_mib} MiB with {method:?}: legacy_reverify={legacy:.2?}, verified_cas={materialized:.2?}, speedup={:.2}x",
        legacy.as_secs_f64() / materialized.as_secs_f64()
    );
}

/// The dep-info writes a path literally; stderr is JSON, where a Windows
/// separator arrives doubled. Both spellings have to round-trip, or every
/// artifact notification on Windows keeps the publishing checkout's path.
#[test]
fn both_spellings_of_a_root_round_trip_through_a_placeholder() {
    let mappings = vec![PathMapping::new(
        if cfg!(windows) {
            r"D:\work\target"
        } else {
            "/work/target"
        },
        "target",
    )];
    let root = mappings[0].root.to_str().unwrap().to_string();
    let escaped = root.replace('\\', r"\\");

    // A dep-info rule and a JSON artifact notification, as rustc writes them.
    let original = format!("{root}/deps/lib.rlib: src/lib.rs\n{{\"artifact\":\"{escaped}\"}}\n");
    let normalized = normalize_output_text(original.as_bytes(), &mappings);

    assert!(
        !String::from_utf8_lossy(&normalized).contains(&root),
        "the literal root survived normalization: {}",
        String::from_utf8_lossy(&normalized)
    );
    if escaped != root {
        assert!(
            !String::from_utf8_lossy(&normalized).contains(&escaped),
            "the escaped root survived normalization: {}",
            String::from_utf8_lossy(&normalized)
        );
    }
    assert_eq!(
        denormalize_output_text(&normalized, &mappings),
        original.as_bytes(),
        "a normalized output did not come back as it went in"
    );
}

/// A restore happens on a machine whose roots differ from the one that
/// published, which is the whole point of the placeholder.
#[test]
fn a_placeholder_is_rewritten_into_the_restoring_checkouts_root() {
    let published = vec![PathMapping::new(
        if cfg!(windows) {
            r"D:\one\target"
        } else {
            "/one/target"
        },
        "target",
    )];
    let restoring = vec![PathMapping::new(
        if cfg!(windows) {
            r"D:\two\target"
        } else {
            "/two/target"
        },
        "target",
    )];
    let published_root = published[0].root.to_str().unwrap();
    let restoring_root = restoring[0].root.to_str().unwrap();

    let original = format!("{published_root}/deps/lib.rlib: src/lib.rs\n");
    let stored = normalize_output_text(original.as_bytes(), &published);
    let restored = denormalize_output_text(&stored, &restoring);

    let restored = String::from_utf8_lossy(&restored).into_owned();
    assert!(
        restored.contains(restoring_root),
        "restore should name the restoring root: {restored}"
    );
    assert!(
        !restored.contains(published_root),
        "restore still names the publishing root: {restored}"
    );
}

/// rustc writes a path with the platform separator in some places and forward
/// slashes in others, which is why `carries` searches both, and stderr is JSON
/// where a Windows separator arrives doubled. A spelling missed here is a path
/// from the publishing checkout left in place.
///
/// The root is spelled the Windows way whatever this platform is, because a
/// unix root has only one spelling and the test would prove nothing there.
#[test]
fn every_spelling_of_a_root_is_normalized() {
    let mappings = vec![PathMapping::new(r"D:\work\target", "target")];
    let root = r"D:\work\target";
    let spellings = [
        root.to_string(),
        root.replace('\\', r"\\"),
        root.replace('\\', "/"),
    ];
    assert_eq!(
        spellings
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "the fixture should exercise three distinct spellings"
    );

    for spelling in &spellings {
        let original = format!("{spelling}/deps/lib.rlib: src/lib.rs\n");
        let normalized = normalize_output_text(original.as_bytes(), &mappings);
        assert!(
            !String::from_utf8_lossy(&normalized).contains(spelling.as_str()),
            "{spelling} survived normalization: {}",
            String::from_utf8_lossy(&normalized)
        );
        assert_eq!(
            denormalize_output_text(&normalized, &mappings),
            original.as_bytes(),
            "{spelling} did not come back as it went in"
        );
    }
}

/// A root is a directory, not a text prefix. `/work/target` has nothing to do
/// with `/work/target-backup`, and rewriting the second would hand a restore a
/// directory that never existed.
#[test]
fn a_sibling_sharing_a_prefix_is_left_alone() {
    let mappings = vec![PathMapping::new(
        if cfg!(windows) {
            r"D:\work\target"
        } else {
            "/work/target"
        },
        "target",
    )];
    let root = mappings[0].root.to_str().unwrap().to_string();
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let sibling = format!("{root}-backup{separator}keep.rlib");
    let inside = format!("{root}{separator}deps{separator}lib.rlib");

    let original = format!("{sibling}\n{inside}\n");
    let normalized = normalize_output_text(original.as_bytes(), &mappings);
    let normalized = String::from_utf8_lossy(&normalized).into_owned();

    assert!(
        normalized.contains(&sibling),
        "the sibling directory was rewritten: {normalized}"
    );
    assert!(
        !normalized.contains(&inside),
        "the root itself was not rewritten: {normalized}"
    );
}

/// A root arrives however its environment variable was written, and a
/// trailing separator must not stop the rewrite: the byte after the match
/// would then be a child's first letter rather than the separator before it.
#[test]
fn a_root_written_with_a_trailing_separator_still_rewrites() {
    let plain = vec![PathMapping::new("/work/target", "target")];
    let trailing = vec![PathMapping::new("/work/target/", "target")];
    let original = "/work/target/deps/lib.rlib: src/lib.rs\n";

    let from_trailing = normalize_output_text(original.as_bytes(), &trailing);
    assert!(
        !String::from_utf8_lossy(&from_trailing).contains("/work/target/deps"),
        "a trailing separator left the path unrewritten: {}",
        String::from_utf8_lossy(&from_trailing)
    );
    assert_eq!(
        from_trailing,
        normalize_output_text(original.as_bytes(), &plain),
        "how the root was written should not change what is stored"
    );
    assert_eq!(
        denormalize_output_text(&from_trailing, &trailing),
        original.as_bytes()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn the_shim_appends_an_oso_prefix_for_cached_links() {
    let arguments = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
    let base = arguments(&[
        "--crate-name=app",
        "--crate-type=bin",
        "--emit=dep-info,link",
        "--out-dir",
        "/work/target/debug/deps",
        "src/main.rs",
    ]);
    let extended = with_oso_prefix(&base, true);
    assert_eq!(
        extended.last().unwrap(),
        &OsString::from("-Clink-arg=-Wl,-oso_prefix,/work/target/debug/deps/"),
    );
    // Off when links are not cached, when the caller chose a prefix, and when
    // there is no output directory to cover.
    assert_eq!(with_oso_prefix(&base, false).len(), base.len());
    let mut chosen = base.clone();
    chosen.push("-Clink-arg=-Wl,-oso_prefix,/elsewhere/".into());
    assert_eq!(with_oso_prefix(&chosen, true).len(), chosen.len());
    let query = arguments(&["--print=cfg"]);
    assert_eq!(with_oso_prefix(&query, true).len(), query.len());
    // An explicit `--target` is never a host link, so nothing is appended:
    // handing a wasm link this flag would bypass it as unmodeled.
    let wasm = arguments(&[
        "--crate-type=bin",
        "--emit=dep-info,link",
        "--target=wasm32-unknown-unknown",
        "--out-dir",
        "/work/target/wasm32-unknown-unknown/debug/deps",
        "src/main.rs",
    ]);
    assert_eq!(with_oso_prefix(&wasm, true).len(), wasm.len());
    let split_target = arguments(&[
        "--target",
        "wasm32-unknown-unknown",
        "--out-dir=/work/target/debug/deps",
        "src/main.rs",
    ]);
    assert_eq!(
        with_oso_prefix(&split_target, true).len(),
        split_target.len()
    );
    let relative = arguments(&["--out-dir", "target/debug/deps", "src/main.rs"]);
    assert_eq!(with_oso_prefix(&relative, true).len(), relative.len());
    let split = arguments(&["--out-dir=/work/target/debug/deps", "src/main.rs"]);
    assert_eq!(
        with_oso_prefix(&split, true).last().unwrap(),
        &OsString::from("-Clink-arg=-Wl,-oso_prefix,/work/target/debug/deps/"),
    );
}

fn in_place_node(bytes: &[u8], executable: bool) -> CacheFileNode {
    CacheFileNode {
        digest: CacheDigest::blake3(bytes),
        executable,
        mode: if executable { 0o755 } else { 0o644 },
        name: "libdep.rlib".into(),
    }
}

#[test]
fn an_output_holding_the_cached_bytes_is_already_in_place() {
    use mbx_cache_core::NoFileDigestCache;
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("libdep.rlib");
    let bytes = b"cached artifact bytes";
    std::fs::write(&destination, bytes).unwrap();

    assert!(output_already_in_place(
        &in_place_node(bytes, false),
        &destination,
        &NoFileDigestCache,
    ));
    assert!(
        !output_already_in_place(
            &in_place_node(b"different artifact bytes!", false),
            &destination,
            &NoFileDigestCache,
        ),
        "content that hashes differently must be rewritten"
    );
    assert!(
        !output_already_in_place(
            &in_place_node(b"cached artifact byte", false),
            &destination,
            &NoFileDigestCache,
        ),
        "a length mismatch must refuse before reading"
    );
    assert!(
        !output_already_in_place(
            &in_place_node(bytes, false),
            &directory.path().join("absent.rlib"),
            &NoFileDigestCache,
        ),
        "a missing output must be materialized"
    );
    #[cfg(unix)]
    assert!(
        !output_already_in_place(
            &in_place_node(bytes, true),
            &destination,
            &NoFileDigestCache
        ),
        "an executable node must not keep a non-executable file"
    );
}

#[test]
fn a_ledger_answer_decides_in_place_without_reading() {
    use mbx_cache_core::{FileDigestCache, FileDigestScope, FileIdentity, RecordedFileDigest};

    /// Vouches one digest for every identity it is asked about.
    struct FixedLedger(CacheDigest);
    impl FileDigestCache for FixedLedger {
        fn find(
            &self,
            _scope: FileDigestScope,
            files: &[FileIdentity],
        ) -> Vec<Option<CacheDigest>> {
            vec![Some(self.0.clone()); files.len()]
        }
        fn record(&self, _scope: FileDigestScope, _entries: Vec<RecordedFileDigest>) {}
    }

    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("libdep.rlib");
    let bytes = b"cached artifact bytes";
    std::fs::write(&destination, bytes).unwrap();
    let node = in_place_node(bytes, false);

    assert!(output_already_in_place(
        &node,
        &destination,
        &FixedLedger(node.digest.clone()),
    ));
    // A ledger that names other bytes of the same length refuses the keep
    // without falling back to a read that would say otherwise: the recorded
    // identity is the fresher claim about what is on disk.
    let mut other = CacheDigest::blake3(b"other bytes entirely here");
    other.size = node.digest.size;
    assert!(!output_already_in_place(
        &node,
        &destination,
        &FixedLedger(other),
    ));
}

/// The budget is a backstop, so it removes state only once the state has
/// actually passed it. A budget lower than what one compilation leaves behind
/// would discard the state before every compile, and the edit loop would be a
/// full recompilation labelled incremental.
#[test]
fn incremental_state_is_discarded_only_past_its_budget() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("unit");
    let session = directory.join("s-session");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("dep-graph.bin"), vec![0_u8; 4096]).unwrap();

    // Inside the budget, and without one, the state stays where it is.
    assert_eq!(
        prepare_incremental_directory(&directory, Some(1 << 20)).unwrap(),
        None
    );
    assert!(session.join("dep-graph.bin").is_file());
    assert_eq!(
        prepare_incremental_directory(&directory, None).unwrap(),
        None
    );
    assert!(session.join("dep-graph.bin").is_file());

    // Past it, the state goes and the caller is told how much went, with the
    // directory left ready for the compilation that follows.
    assert_eq!(
        prepare_incremental_directory(&directory, Some(1024)).unwrap(),
        Some(4096)
    );
    assert!(directory.is_dir());
    assert!(!session.exists());

    // A directory that does not exist yet is simply created.
    let fresh = root.path().join("fresh");
    assert_eq!(
        prepare_incremental_directory(&fresh, Some(1)).unwrap(),
        None
    );
    assert!(fresh.is_dir());
}
