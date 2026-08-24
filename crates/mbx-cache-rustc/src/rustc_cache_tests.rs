#[test]
fn verbose_version_is_a_query_not_an_unmodeled_flag() {
    // cargo runs `rustc -vV` to identify the compiler; reporting it as an
    // unmodeled flag sends people hunting for a gap that is not there.
    assert_eq!(
        RustcInvocation::parse(&args(&["-vV"])).unwrap_err(),
        BypassReason::CompilerQuery
    );
}

#[test]
fn bypass_kinds_are_stable_and_field_independent() {
    assert_eq!(BypassReason::CompilerQuery.kind(), "compiler-query");
    assert_eq!(BypassReason::Incremental.kind(), "incremental");
    assert_eq!(
        BypassReason::UnsupportedCrateType("bin".into()).kind(),
        "unsupported-crate-type"
    );
    // Two reasons of one kind group together despite differing fields.
    assert_eq!(
        BypassReason::UnmappedAbsolutePath(PathBuf::from("/a")).kind(),
        BypassReason::UnmappedAbsolutePath(PathBuf::from("/b")).kind()
    );
}

#[cfg(unix)]
#[test]
fn path_mappings_resolve_symlinked_roots_for_missing_outputs() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let physical = directory.path().join("physical");
    let alias = directory.path().join("alias");
    std::fs::create_dir(&physical).unwrap();
    symlink(&physical, &alias).unwrap();

    let mappings = vec![PathMapping::new(alias, "target")];
    let output = physical.join("debug/deps/not-created.wasm");
    assert_eq!(
        normalize_mapped_path(&output, directory.path(), &mappings).unwrap(),
        "${target}/debug/deps/not-created.wasm"
    );
}

use super::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn digest(value: &str) -> CacheDigest {
    CacheDigest::blake3(value.as_bytes())
}

fn absolute(segments: &[&str]) -> PathBuf {
    let mut path = if cfg!(windows) {
        PathBuf::from(r"C:\")
    } else {
        PathBuf::from("/")
    };
    path.extend(segments);
    path
}

fn workspace() -> PathBuf {
    absolute(&["work", "project"])
}

fn sysroot() -> PathBuf {
    absolute(&["toolchains", "1.97.1"])
}

fn context(inputs: &[(&str, &str)]) -> ActionContext {
    ActionContext {
        compiler: CompilerIdentity {
            toolchain: "core:rust@1.97.1".into(),
            rustc_version: "1.97.1 (8bab26f4f 2026-07-14)".into(),
            host: "x86_64-unknown-linux-gnu".into(),
        },
        working_dir: workspace(),
        path_mappings: vec![
            PathMapping::new(workspace().join("target"), "target"),
            PathMapping::new(workspace(), "workspace"),
            PathMapping::new(absolute(&["home", "user", ".cargo"]), "cargo_home"),
            PathMapping::new(sysroot(), "sysroot"),
        ],
        environment: BTreeMap::from([("CARGO_PKG_VERSION".into(), Some("1.0.0".into()))]),
        portable_environment: BTreeSet::new(),
        inputs: inputs
            .iter()
            .map(|(path, contents)| ActionInput {
                path: (*path).into(),
                digest: digest(contents),
            })
            .collect(),
    }
}

fn common_invocation() -> RustcInvocation {
    let output = workspace().join("target/debug/deps");
    RustcInvocation::parse(&[
        "--crate-name".into(),
        "widget".into(),
        "--edition=2024".into(),
        "src/lib.rs".into(),
        "--crate-type".into(),
        "lib".into(),
        "--emit=dep-info,metadata,link".into(),
        "-Cembed-bitcode=no".into(),
        "-C".into(),
        "metadata=abc123".into(),
        "--out-dir".into(),
        output.clone().into_os_string(),
        format!("-Ldependency={}", output.display()).into(),
        "--extern".into(),
        format!("serde={}", output.join("libserde.rlib").display()).into(),
        format!("--sysroot={}", sysroot().display()).into(),
        "--cap-lints".into(),
        "allow".into(),
    ])
    .unwrap()
}

#[test]
fn parses_a_cargo_library_invocation() {
    let invocation = common_invocation();
    assert_eq!(invocation.source(), Path::new("src/lib.rs"));
    let action = invocation
        .action(context(&[
            ("src/lib.rs", "source"),
            ("target/debug/deps/libserde.rlib", "serde"),
        ]))
        .unwrap();
    let json = String::from_utf8(action.bytes).unwrap();
    assert!(json.contains(r#""kind":"rustc""#));
    assert!(json.contains(r#""--out-dir=${target}/debug/deps""#));
    assert!(json.contains(r#""--extern=serde=${target}/debug/deps/libserde.rlib""#));
    assert_eq!(action.digest.algorithm, "blake3");
}

#[test]
fn resolves_cargo_library_outputs() {
    let working_dir = absolute(&["workspace"]);
    let invocation = RustcInvocation::parse(&args(&[
        "--crate-name=widget",
        "--crate-type=lib",
        "--emit=dep-info,metadata,link",
        "--out-dir=target/debug/deps",
        "-Cextra-filename=-abc123",
        "src/lib.rs",
    ]))
    .unwrap();
    assert_eq!(
        invocation.outputs(&working_dir).unwrap(),
        RustcOutputs {
            directory: working_dir.join("target/debug/deps"),
            files: vec![
                working_dir.join("target/debug/deps/libwidget-abc123.rlib"),
                working_dir.join("target/debug/deps/libwidget-abc123.rmeta"),
            ],
            dep_info: working_dir.join("target/debug/deps/widget-abc123.d"),
        }
    );
}

#[test]
fn resolves_a_compiler_linked_wasm_binary() {
    let working_dir = absolute(&["workspace"]);
    let invocation = RustcInvocation::parse(&args(&[
        "--crate-name=widget",
        "--crate-type=bin",
        "--emit=dep-info,link",
        "--out-dir=target/wasm32-unknown-unknown/debug/deps",
        "--target=wasm32-unknown-unknown",
        "-Cextra-filename=-abc123",
        "src/main.rs",
    ]))
    .unwrap();
    let executable =
        working_dir.join("target/wasm32-unknown-unknown/debug/deps/widget-abc123.wasm");

    assert_eq!(
        invocation.outputs(&working_dir).unwrap(),
        RustcOutputs {
            directory: working_dir.join("target/wasm32-unknown-unknown/debug/deps"),
            files: vec![executable],
            dep_info: working_dir.join("target/wasm32-unknown-unknown/debug/deps/widget-abc123.d"),
        }
    );
}

#[test]
fn accepts_wasm_tests_but_not_native_tests() {
    let wasm = args(&[
        "--test",
        "--emit=dep-info,link",
        "--target=wasm32-unknown-unknown",
        "src/lib.rs",
    ]);
    assert!(RustcInvocation::parse(&wasm).is_ok());

    let implicit_binary = args(&[
        "--emit=dep-info,link",
        "--target=wasm32-unknown-unknown",
        "src/main.rs",
    ]);
    assert_eq!(
        RustcInvocation::parse(&implicit_binary),
        Err(BypassReason::UnsupportedCrateType("bin".into()))
    );

    let native = args(&["--test", "--emit=dep-info,link", "src/lib.rs"]);
    assert_eq!(
        RustcInvocation::parse(&native),
        Err(BypassReason::UnsupportedCrateType("test".into()))
    );
}

#[test]
fn custom_wasm_linker_modes_still_bypass() {
    let arguments = args(&[
        "--crate-type=bin",
        "--emit=dep-info,link",
        "--target=wasm32-unknown-unknown",
        "-Clink-self-contained=no",
        "src/main.rs",
    ]);
    assert_eq!(
        RustcInvocation::parse(&arguments),
        Err(BypassReason::UnknownCodegenOption(
            "link-self-contained".into()
        ))
    );

    let arguments = args(&[
        "--crate-type=bin",
        "--emit=dep-info,link",
        "--target=wasm32-unknown-unknown",
        "-Clinker=/tmp/custom-linker",
        "src/main.rs",
    ]);
    assert_eq!(
        RustcInvocation::parse(&arguments),
        Err(BypassReason::UnknownCodegenOption("linker".into()))
    );
}

#[test]
fn infers_a_valid_crate_name_from_a_hyphenated_source() {
    let working_dir = absolute(&["workspace"]);
    let invocation = RustcInvocation::parse(&args(&[
        "--crate-type=lib",
        "--emit=dep-info,metadata",
        "my-library.rs",
    ]))
    .unwrap();

    assert_eq!(
        invocation.outputs(&working_dir).unwrap().dep_info,
        working_dir.join("my_library.d")
    );
}

#[test]
fn refuses_an_output_file_with_implicit_emit_paths() {
    let working_dir = absolute(&["workspace"]);
    let invocation = RustcInvocation::parse(&args(&[
        "--crate-name=widget",
        "--crate-type=lib",
        "--emit=dep-info,metadata,link",
        "-o",
        "target/custom.rlib",
        "src/lib.rs",
    ]))
    .unwrap();

    // rustc applies -o to every emit that has no path of its own, so the
    // artifact names cannot be derived from the crate name.
    assert_eq!(
        invocation.outputs(&working_dir).unwrap_err(),
        BypassReason::ImplicitEmitWithOutputFile(working_dir.join("target/custom.rlib"))
    );
}

#[test]
fn resolves_an_output_file_when_every_emit_names_its_path() {
    let working_dir = absolute(&["workspace"]);
    let invocation = RustcInvocation::parse(&args(&[
        "--crate-name=widget",
        "--crate-type=lib",
        "--emit=dep-info=target/widget.d,metadata=target/widget.rmeta,link=target/widget.rlib",
        "-o",
        "target/custom.rlib",
        "src/lib.rs",
    ]))
    .unwrap();

    assert_eq!(
        invocation.outputs(&working_dir).unwrap(),
        RustcOutputs {
            directory: working_dir.join("target"),
            files: vec![
                working_dir.join("target/widget.rlib"),
                working_dir.join("target/widget.rmeta"),
            ],
            dep_info: working_dir.join("target/widget.d"),
        }
    );
}

#[test]
fn dep_info_must_share_the_artifact_output_directory() {
    let working_dir = absolute(&["workspace"]);
    let invocation = RustcInvocation::parse(&args(&[
        "--crate-name=widget",
        "--crate-type=lib",
        "--emit=dep-info=target/dep-info/widget.d,metadata,link",
        "--out-dir=target/debug/deps",
        "src/lib.rs",
    ]))
    .unwrap();

    assert_eq!(
        invocation.outputs(&working_dir),
        Err(BypassReason::SplitOutputDirectories)
    );
}

#[test]
fn equivalent_worktrees_produce_the_same_action_key() {
    let first_context = context(&[
        ("src/lib.rs", "source"),
        ("target/debug/deps/libserde.rlib", "serde"),
    ]);
    let first = common_invocation().action(first_context).unwrap();
    let other = absolute(&["other", "checkout"]);
    let output = other.join("target/debug/deps");
    let invocation = RustcInvocation::parse(&[
        "--crate-name=widget".into(),
        "--edition=2024".into(),
        "src/lib.rs".into(),
        "--crate-type=lib".into(),
        "--emit=dep-info,metadata,link".into(),
        "-Cembed-bitcode=no".into(),
        "-Cmetadata=abc123".into(),
        format!("--out-dir={}", output.display()).into(),
        format!("-Ldependency={}", output.display()).into(),
        format!("--extern=serde={}", output.join("libserde.rlib").display()).into(),
        format!("--sysroot={}", sysroot().display()).into(),
        "--cap-lints=allow".into(),
    ])
    .unwrap();
    let mut second_context = context(&[]);
    second_context.working_dir = other.clone();
    second_context.path_mappings[0].root = other.join("target");
    second_context.path_mappings[1].root = other.clone();
    second_context.inputs = vec![
        ActionInput {
            path: "src/lib.rs".into(),
            digest: digest("source"),
        },
        ActionInput {
            path: "target/debug/deps/libserde.rlib".into(),
            digest: digest("serde"),
        },
    ];
    let second = invocation.action(second_context).unwrap();
    assert_eq!(first.digest, second.digest);
}

#[test]
fn predicts_inputs_without_reusing_stale_contents() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().canonicalize().unwrap();
    std::fs::create_dir(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/lib.rs"), "pub fn value() -> u8 { 1 }").unwrap();
    let invocation = RustcInvocation::parse(&args(&[
        "--crate-name=widget",
        "--crate-type=lib",
        "--emit=dep-info,metadata,link",
        "--out-dir=target/debug/deps",
        "src/lib.rs",
    ]))
    .unwrap();
    let compiler = CompilerIdentity {
        toolchain: "stable".into(),
        rustc_version: "rustc test".into(),
        host: "test-host".into(),
    };
    let context = ActionContext {
        compiler,
        working_dir: workspace.clone(),
        path_mappings: vec![PathMapping::new(&workspace, "workspace")],
        environment: BTreeMap::new(),
        portable_environment: BTreeSet::new(),
        inputs: Vec::new(),
    };
    let dep_info = RustcDepInfo {
        files: vec!["src/lib.rs".into()],
        environment: BTreeMap::new(),
    };
    let discovered = invocation.discover_inputs(&dep_info, &workspace).unwrap();
    let mut initial_context = context.clone();
    discovered.clone().apply_to(&mut initial_context).unwrap();
    let initial = invocation.action(initial_context).unwrap();
    let prediction = invocation.prediction(&context, &discovered).unwrap();
    assert_eq!(prediction.inputs, ["${workspace}/src/lib.rs"]);

    let predicted = prediction
        .discover(&workspace, &context.path_mappings)
        .unwrap();
    let mut predicted_context = context.clone();
    predicted.apply_to(&mut predicted_context).unwrap();
    assert_eq!(invocation.action(predicted_context).unwrap(), initial);

    std::fs::write(workspace.join("src/lib.rs"), "pub fn value() -> u8 { 2 }").unwrap();
    let changed = prediction
        .discover(&workspace, &context.path_mappings)
        .unwrap();
    let mut changed_context = context;
    changed.apply_to(&mut changed_context).unwrap();
    assert_ne!(
        invocation.action(changed_context).unwrap().digest,
        initial.digest
    );
}

#[test]
fn predicted_mapping_root_round_trips() {
    let workspace = workspace();
    assert_eq!(
        denormalize_path("${workspace}", &[PathMapping::new(&workspace, "workspace")]).unwrap(),
        workspace
    );
}

#[test]
fn absolute_environment_values_remain_literal_action_inputs() {
    let invocation = common_invocation();
    let mut first_context = context(&[
        ("src/lib.rs", "source"),
        ("target/debug/deps/libserde.rlib", "serde"),
    ]);
    let first_out_dir = workspace().join("target/debug/build/widget/out");
    first_context
        .environment
        .insert("OUT_DIR".into(), Some(first_out_dir.display().to_string()));
    let first = invocation.action(first_context).unwrap();

    let mut second_context = context(&[
        ("src/lib.rs", "source"),
        ("target/debug/deps/libserde.rlib", "serde"),
    ]);
    second_context.environment.insert(
        "OUT_DIR".into(),
        Some(absolute(&["other", "out"]).display().to_string()),
    );
    let second = invocation.action(second_context).unwrap();

    // The descriptor is canonical JSON, so the value appears the way JSON writes it --
    // on Windows that means escaped separators. Quote it with the same serializer instead
    // of hand-rolling the escaping; that also pins the surrounding quotes, so this only
    // matches a whole JSON string rather than any substring.
    let descriptor = String::from_utf8(first.bytes).unwrap();
    let expected =
        String::from_utf8(canonical_json(&first_out_dir.display().to_string()).unwrap()).unwrap();
    assert!(descriptor.contains(&expected), "{descriptor}");
    assert_ne!(first.digest, second.digest);
}

/// The counterpart to the test above. Naming `OUT_DIR` portable normalizes
/// its value like any other path, which is what lets two checkouts agree on
/// a compilation that reads it. The caller earns the claim by remapping the
/// value inside the compilation and reading the outputs; the key only
/// records that it was made.
#[test]
fn portable_environment_values_normalize_across_checkouts() {
    let other = absolute(&["other", "checkout"]);
    let output = other.join("target/debug/deps");
    let relocated = RustcInvocation::parse(&[
        "--crate-name=widget".into(),
        "--edition=2024".into(),
        "src/lib.rs".into(),
        "--crate-type=lib".into(),
        "--emit=dep-info,metadata,link".into(),
        "-Cembed-bitcode=no".into(),
        "-Cmetadata=abc123".into(),
        format!("--out-dir={}", output.display()).into(),
        format!("-Ldependency={}", output.display()).into(),
        format!("--extern=serde={}", output.join("libserde.rlib").display()).into(),
        format!("--sysroot={}", sysroot().display()).into(),
        "--cap-lints=allow".into(),
    ])
    .unwrap();

    let here = |portable: bool| {
        let mut context = context(&[
            ("src/lib.rs", "source"),
            ("target/debug/deps/libserde.rlib", "serde"),
        ]);
        context.environment.insert(
            "OUT_DIR".into(),
            Some(
                workspace()
                    .join("target/debug/build/widget/out")
                    .display()
                    .to_string(),
            ),
        );
        if portable {
            context.portable_environment.insert("OUT_DIR".into());
        }
        common_invocation().action(context).unwrap().digest
    };
    let there = |portable: bool| {
        let mut context = context(&[]);
        context.working_dir = other.clone();
        context.path_mappings[0].root = other.join("target");
        context.path_mappings[1].root = other.clone();
        context.inputs = vec![
            ActionInput {
                path: "src/lib.rs".into(),
                digest: digest("source"),
            },
            ActionInput {
                path: "target/debug/deps/libserde.rlib".into(),
                digest: digest("serde"),
            },
        ];
        context.environment.insert(
            "OUT_DIR".into(),
            Some(
                other
                    .join("target/debug/build/widget/out")
                    .display()
                    .to_string(),
            ),
        );
        if portable {
            context.portable_environment.insert("OUT_DIR".into());
        }
        relocated.action(context).unwrap().digest
    };

    assert_ne!(here(false), there(false));
    assert_eq!(here(true), there(true));
    // A different key, not a relabelled one: an artifact compiled without
    // the remapping must never be restored under the portable key.
    assert_ne!(here(false), here(true));
}

#[test]
fn content_and_environment_change_the_action_key() {
    let invocation = common_invocation();
    let first = invocation
        .action(context(&[
            ("src/lib.rs", "source"),
            ("target/debug/deps/libserde.rlib", "serde"),
        ]))
        .unwrap();
    let changed_source = invocation
        .action(context(&[
            ("src/lib.rs", "changed"),
            ("target/debug/deps/libserde.rlib", "serde"),
        ]))
        .unwrap();
    let mut changed_environment = context(&[
        ("src/lib.rs", "source"),
        ("target/debug/deps/libserde.rlib", "serde"),
    ]);
    changed_environment
        .environment
        .insert("CARGO_PKG_VERSION".into(), Some("2.0.0".into()));
    let changed_environment = invocation.action(changed_environment).unwrap();
    assert_ne!(first.digest, changed_source.digest);
    assert_ne!(first.digest, changed_environment.digest);
}

#[test]
fn unknown_and_incremental_options_bypass() {
    for (arguments, expected) in [
        (
            vec!["--future-flag", "src/lib.rs"],
            BypassReason::UnknownFlag("--future-flag".into()),
        ),
        (
            vec!["-Cfuture-option=yes", "src/lib.rs"],
            BypassReason::UnknownCodegenOption("future-option".into()),
        ),
        (
            vec!["-Cincremental=target/incremental", "src/lib.rs"],
            BypassReason::Incremental,
        ),
    ] {
        assert_eq!(RustcInvocation::parse(&args(&arguments)), Err(expected));
    }
}

#[test]
fn linked_and_unmodeled_outputs_bypass() {
    for (arguments, expected) in [
        (
            vec!["--crate-type=bin", "--emit=link", "src/main.rs"],
            BypassReason::UnsupportedCrateType("bin".into()),
        ),
        (
            vec!["--crate-type=lib", "--emit=obj", "src/lib.rs"],
            BypassReason::UnsupportedEmit("obj".into()),
        ),
        (
            vec!["--crate-type=lib", "--emit=dep-info", "src/lib.rs"],
            BypassReason::NoCacheableOutput,
        ),
    ] {
        assert_eq!(RustcInvocation::parse(&args(&arguments)), Err(expected));
    }
}

#[test]
fn action_requires_every_direct_input() {
    let error = common_invocation()
        .action(context(&[("src/lib.rs", "source")]))
        .unwrap_err();
    assert_eq!(
        error,
        BypassReason::MissingRequiredInput("${target}/debug/deps/libserde.rlib".into())
    );
}

#[test]
fn action_rejects_unmapped_absolute_paths() {
    let unmapped = absolute(&["tmp", "rustc-output"]);
    let invocation = RustcInvocation::parse(&[
        "--crate-type=lib".into(),
        "--emit=link".into(),
        "src/lib.rs".into(),
        format!("--out-dir={}", unmapped.display()).into(),
    ])
    .unwrap();
    let error = invocation
        .action(context(&[("src/lib.rs", "source")]))
        .unwrap_err();
    assert_eq!(error, BypassReason::UnmappedAbsolutePath(unmapped));
}

#[test]
fn custom_targets_are_required_inputs() {
    let invocation = RustcInvocation::parse(&args(&[
        "--crate-type=lib",
        "--emit=metadata",
        "--target=targets/custom.json",
        "src/lib.rs",
    ]))
    .unwrap();
    assert_eq!(invocation.target(), Some("targets/custom.json"));
    let error = invocation
        .action(context(&[("src/lib.rs", "source")]))
        .unwrap_err();
    assert_eq!(
        error,
        BypassReason::MissingRequiredInput("${workspace}/targets/custom.json".into())
    );
}

#[test]
fn remap_destinations_are_stable_virtual_paths() {
    let invocation = RustcInvocation::parse(&[
        "--crate-type=lib".into(),
        "--emit=metadata".into(),
        format!("--remap-path-prefix={}=/src", workspace().display()).into(),
        "src/lib.rs".into(),
    ])
    .unwrap();
    let action = invocation
        .action(context(&[("src/lib.rs", "source")]))
        .unwrap();
    assert!(
        String::from_utf8(action.bytes)
            .unwrap()
            .contains(r#"--remap-path-prefix=${workspace}=/src"#)
    );
}
