use super::*;

/// Bypass kinds are the grouping keys in build statistics, `mbx explain`, and
/// the bypass log. Renaming one silently reclassifies historical output, so the
/// strings are pinned here and the payloads deliberately vary.
#[test]
fn bypass_kinds_are_stable_and_field_independent() {
    let cases: Vec<(CcBypassReason, &str)> = vec![
        (
            CcBypassReason::NonUtf8Argument { index: 3 },
            "non-utf8-argument",
        ),
        (CcBypassReason::ResponseFile("args".into()), "response-file"),
        (
            CcBypassReason::UnknownFlag("-fmagic".into()),
            "unknown-flag",
        ),
        (CcBypassReason::MissingValue("-o".into()), "missing-value"),
        (CcBypassReason::CompilerQuery, "compiler-query"),
        (CcBypassReason::NotACompile, "not-a-compile"),
        (
            CcBypassReason::NonObjectOutput("-E".into()),
            "non-object-output",
        ),
        (CcBypassReason::StandardInput, "standard-input"),
        (CcBypassReason::MissingInput, "missing-input"),
        (CcBypassReason::MultipleInputs, "multiple-inputs"),
        (CcBypassReason::MissingOutput, "missing-output"),
        (
            CcBypassReason::UnsupportedLanguage("a.S".into()),
            "unsupported-language",
        ),
        (
            CcBypassReason::CallerDependencyFlags("-MMD".into()),
            "caller-dependency-flags",
        ),
        (
            CcBypassReason::PrecompiledHeader("-include-pch".into()),
            "precompiled-header",
        ),
        (
            CcBypassReason::CoverageInstrumentation("--coverage".into()),
            "coverage-instrumentation",
        ),
        (
            CcBypassReason::SplitDebugOutput("-gsplit-dwarf".into()),
            "split-debug-output",
        ),
        (
            CcBypassReason::SaveTemps("-save-temps".into()),
            "save-temps",
        ),
        (
            CcBypassReason::ToolPassthrough("-Xclang".into()),
            "tool-passthrough",
        ),
        (CcBypassReason::Plugin("-fplugin=x".into()), "plugin"),
        (
            CcBypassReason::LocalCpuTarget("-march=native".into()),
            "local-cpu-target",
        ),
        (
            CcBypassReason::UnsupportedCompilerDriver("cl.exe".into()),
            "unsupported-compiler-driver",
        ),
        (
            CcBypassReason::CompilerIdentityUnavailable("probe failed".into()),
            "compiler-identity-unavailable",
        ),
        (
            CcBypassReason::UnsupportedEnvironment("CPATH".into()),
            "unsupported-environment",
        ),
        (
            CcBypassReason::RealCompilerUnpinned,
            "real-compiler-unpinned",
        ),
        (
            CcBypassReason::EmbeddedTimestampMacro("/w/a.c".into()),
            "embedded-timestamp-macro",
        ),
        (
            CcBypassReason::MalformedDepfile("no rule".into()),
            "malformed-depfile",
        ),
        (
            CcBypassReason::DepfileRead {
                path: "/w/a.d".into(),
                message: "gone".into(),
            },
            "depfile-read",
        ),
        (CcBypassReason::TooManyInputs, "too-many-inputs"),
        (
            CcBypassReason::UnmappedAbsolutePath("/opt/x".into()),
            "unmapped-absolute-path",
        ),
        (CcBypassReason::NonUtf8Path("/w/a".into()), "non-utf8-path"),
        (
            CcBypassReason::RelativeWorkingDirectory("w".into()),
            "relative-working-directory",
        ),
        (
            CcBypassReason::RelativePathMapping("w".into()),
            "relative-path-mapping",
        ),
        (
            CcBypassReason::InvalidPathPlaceholder("bad name".into()),
            "invalid-path-placeholder",
        ),
        (
            CcBypassReason::MissingRequiredInput("${workspace}/a.c".into()),
            "missing-required-input",
        ),
        (
            CcBypassReason::InvalidInputDigest("/w/a.c".into()),
            "invalid-input-digest",
        ),
        (
            CcBypassReason::ConflictingInput("${workspace}/a.c".into()),
            "conflicting-input",
        ),
        (
            CcBypassReason::InputRead {
                path: "/w/a.c".into(),
                message: "gone".into(),
            },
            "input-read",
        ),
        (
            CcBypassReason::InputChanged("/w/a.c".into()),
            "input-changed",
        ),
        (
            CcBypassReason::InputModifiedDuringCompilation("/w/a.c".into()),
            "input-modified-during-compilation",
        ),
        (
            CcBypassReason::DiscoveryWorkingDirectory,
            "discovery-working-directory",
        ),
        (
            CcBypassReason::UnsupportedPrediction,
            "unsupported-prediction",
        ),
        (
            CcBypassReason::InvalidPredictedInput("../x".into()),
            "invalid-predicted-input",
        ),
        (
            CcBypassReason::Serialization("bad json".into()),
            "serialization",
        ),
    ];
    for (reason, kind) in cases {
        assert_eq!(reason.kind(), kind, "kind changed for {reason:?}");
    }
}

fn argv(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// Build an absolute path that is absolute on this platform, since a key that
/// rejects relative roots would otherwise reject every fixture on Windows.
fn absolute(segments: &[&str]) -> PathBuf {
    let mut path = if cfg!(windows) {
        PathBuf::from(r"C:\")
    } else {
        PathBuf::from("/")
    };
    path.extend(segments);
    path
}

fn text(path: &Path) -> String {
    path.to_str().expect("fixture paths are UTF-8").to_string()
}

fn typical() -> Vec<OsString> {
    argv(&[
        "-O2",
        "-ffunction-sections",
        "-fdata-sections",
        "-fPIC",
        "-gdwarf-4",
        "-I",
        "/work/target/debug/build/zstd-sys-1234/out",
        "-Ilib/zstd",
        "-DZSTD_MULTITHREAD=1",
        "-Wall",
        "-Wextra",
        "-std=c11",
        "-o",
        "/work/target/debug/build/zstd-sys-1234/out/entropy.o",
        "-c",
        "lib/zstd/entropy.c",
    ])
}

#[test]
fn parses_a_typical_cc_crate_invocation() {
    let invocation = CcInvocation::parse(&typical()).expect("invocation should be admitted");
    assert_eq!(invocation.source(), Path::new("lib/zstd/entropy.c"));
    assert_eq!(
        invocation.output(),
        Path::new("/work/target/debug/build/zstd-sys-1234/out/entropy.o")
    );
    assert_eq!(invocation.language(), CcLanguage::C);
    assert_eq!(
        invocation.include_dirs(),
        [
            PathBuf::from("/work/target/debug/build/zstd-sys-1234/out"),
            PathBuf::from("lib/zstd"),
        ]
    );
    assert_eq!(
        invocation.required_inputs(),
        [PathBuf::from("lib/zstd/entropy.c")]
    );
}

#[test]
fn linking_without_dash_c_is_not_a_compile() {
    let arguments = argv(&["-o", "app", "main.c"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "not-a-compile"
    );
}

#[test]
fn preprocess_and_assembly_modes_bypass_as_non_object_output() {
    for mode in ["-E", "-S"] {
        let arguments = argv(&[mode, "-o", "out", "main.c"]);
        assert_eq!(
            CcInvocation::parse(&arguments).unwrap_err().kind(),
            "non-object-output"
        );
    }
}

#[test]
fn caller_dependency_flags_bypass_before_injection() {
    for flag in ["-MD", "-MMD", "-MP", "-M", "-MM"] {
        let arguments = argv(&[flag, "-c", "-o", "a.o", "a.c"]);
        assert_eq!(
            CcInvocation::parse(&arguments).unwrap_err().kind(),
            "caller-dependency-flags",
            "{flag} should bypass"
        );
    }
}

#[test]
fn repeated_output_flags_follow_the_driver_and_the_last_one_wins() {
    let arguments = argv(&["-c", "-o", "first.o", "-o", "second.o", "a.c"]);
    let invocation = CcInvocation::parse(&arguments).expect("invocation should be admitted");
    assert_eq!(invocation.output(), Path::new("second.o"));
    let keyed = invocation
        .arguments
        .iter()
        .filter(|argument| matches!(argument, Argument::Path { flag, .. } if flag == "-o"))
        .count();
    assert_eq!(keyed, 2, "both outputs must enter the key");
}

#[test]
fn unlisted_f_and_m_flags_bypass_with_the_flag_text() {
    for flag in ["-fmagic", "-mmagic"] {
        let arguments = argv(&[flag, "-c", "-o", "a.o", "a.c"]);
        let reason = CcInvocation::parse(&arguments).unwrap_err();
        assert_eq!(reason.kind(), "unknown-flag");
        assert!(reason.to_string().contains(flag), "{reason}");
    }
}

#[test]
fn tool_passthrough_flags_bypass_even_with_admitted_prefixes() {
    for flag in ["-Wl,-z,now", "-Wa,--noexecstack", "-Wp,-D_FORTIFY_SOURCE=2"] {
        let arguments = argv(&[flag, "-c", "-o", "a.o", "a.c"]);
        assert_eq!(
            CcInvocation::parse(&arguments).unwrap_err().kind(),
            "tool-passthrough",
            "{flag} should bypass"
        );
    }
    let arguments = argv(&["-Xclang", "-ffake", "-c", "-o", "a.o", "a.c"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "tool-passthrough"
    );
    let arguments = argv(&["-mllvm", "-inline-threshold=0", "-c", "-o", "a.o", "a.c"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "tool-passthrough"
    );
}

#[test]
fn plain_warning_flags_stay_admitted_key_material() {
    let arguments = argv(&["-Wall", "-Werror", "-Wno-unused", "-c", "-o", "a.o", "a.c"]);
    let invocation = CcInvocation::parse(&arguments).expect("warnings should be admitted");
    assert!(
        invocation
            .arguments
            .contains(&Argument::Plain("-Werror".into()))
    );
}

#[test]
fn assembly_and_objective_c_sources_bypass_as_unsupported_languages() {
    for source in ["a.S", "a.s", "a.m", "a.mm", "a.C", "a.rs"] {
        let arguments = argv(&["-c", "-o", "a.o", source]);
        assert_eq!(
            CcInvocation::parse(&arguments).unwrap_err().kind(),
            "unsupported-language",
            "{source} should bypass"
        );
    }
}

#[test]
fn dash_x_overrides_source_extension_detection() {
    let arguments = argv(&["-x", "c++", "-c", "-o", "a.o", "a.inl"]);
    let invocation = CcInvocation::parse(&arguments).expect("explicit language should be admitted");
    assert_eq!(invocation.language(), CcLanguage::Cxx);

    let arguments = argv(&["-x", "assembler", "-c", "-o", "a.o", "a.s"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "unsupported-language"
    );
}

#[test]
fn response_file_arguments_bypass_with_the_stable_reason_kind() {
    let arguments = argv(&["@args.rsp"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "response-file"
    );
}

#[test]
fn apple_arch_flags_are_admitted_key_material() {
    let arguments = argv(&[
        "-arch", "arm64", "-arch", "x86_64", "-c", "-o", "a.o", "a.c",
    ]);
    let invocation = CcInvocation::parse(&arguments).expect("arch flags should be admitted");
    assert!(
        invocation
            .arguments
            .contains(&Argument::Plain("-arch=arm64".into()))
    );
    assert!(
        invocation
            .arguments
            .contains(&Argument::Plain("-arch=x86_64".into()))
    );
}

#[test]
fn multiple_sources_and_missing_pieces_bypass() {
    let arguments = argv(&["-c", "-o", "a.o", "a.c", "b.c"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "multiple-inputs"
    );
    let arguments = argv(&["-c", "-o", "a.o"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "missing-input"
    );
    let arguments = argv(&["-c", "a.c"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "missing-output"
    );
    let arguments = argv(&["-c", "-o", "a.o", "-"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "standard-input"
    );
}

#[test]
fn driver_queries_bypass_rather_than_compiling() {
    for flag in ["-v", "--version", "-dumpmachine", "-print-search-dirs"] {
        let arguments = argv(&[flag]);
        assert_eq!(
            CcInvocation::parse(&arguments).unwrap_err().kind(),
            "compiler-query",
            "{flag} should bypass"
        );
    }
}

fn identity() -> CcCompilerIdentity {
    CcCompilerIdentity {
        family: CcCompilerFamily::Clang,
        version_text: "clang version 17.0.6\nTarget: aarch64-apple-darwin\n".into(),
        target: "aarch64-apple-darwin".into(),
        assembler: String::new(),
    }
}

fn context(workspace: &Path, target: &Path) -> CcActionContext {
    CcActionContext {
        compiler: identity(),
        working_dir: workspace.to_path_buf(),
        path_mappings: vec![
            PathMapping::new(target, "target"),
            PathMapping::new(workspace, "workspace"),
        ],
        environment: BTreeMap::new(),
        inputs: Vec::new(),
    }
}

/// A checkout and the target directory inside it.
fn checkout(name: &str) -> (PathBuf, PathBuf) {
    let workspace = absolute(&["work", name]);
    let target = workspace.join("target");
    (workspace, target)
}

fn digest_of(value: &str) -> CacheDigest {
    CacheDigest::blake3(value.as_bytes())
}

#[test]
fn equivalent_worktrees_produce_the_same_portable_action_key() {
    let build = |name: &str| {
        let (workspace, target) = checkout(name);
        let arguments = argv(&["-O2", "-c", "-o", "out.o", "src/a.c"]);
        let invocation = CcInvocation::parse(&arguments).expect("admitted");
        let mut context = context(&workspace, &target);
        context.inputs = vec![
            CcActionInput {
                path: workspace.join("src/a.c"),
                digest: digest_of("source"),
            },
            CcActionInput {
                path: workspace.join("src/a.h"),
                digest: digest_of("header"),
            },
        ];
        invocation.action(context).expect("action").digest
    };
    assert_eq!(build("one"), build("two"));
}

#[test]
fn registry_sources_normalize_under_the_cargo_home_placeholder() {
    // Cargo runs a registry crate's build script with its cwd inside the
    // registry checkout, which is nowhere near the user's workspace.
    let (workspace, target) = checkout("one");
    let cargo_home = absolute(&["home", "u", ".cargo"]);
    let package = cargo_home.join("registry/src/index/zstd-sys-2.0");
    let arguments = argv(&["-c", "-o", "out.o", "zstd.c"]);
    let invocation = CcInvocation::parse(&arguments).expect("admitted");
    let mut context = context(&workspace, &target);
    context.working_dir = package.clone();
    context
        .path_mappings
        .push(PathMapping::new(cargo_home, "cargo_home"));
    context.inputs = vec![CcActionInput {
        path: package.join("zstd.c"),
        digest: digest_of("source"),
    }];
    let action = invocation.action(context).expect("action");
    let descriptor = String::from_utf8(action.bytes).expect("utf-8");
    assert!(
        descriptor.contains("${cargo_home}/registry/src/index/zstd-sys-2.0/zstd.c"),
        "{descriptor}"
    );
}

/// The system roots are POSIX paths, and the shims are only installed on unix.
#[cfg(unix)]
#[test]
fn system_headers_are_keyed_verbatim_under_admitted_roots() {
    let (workspace, target) = checkout("one");
    let arguments = argv(&["-c", "-o", "out.o", "src/a.c"]);
    let invocation = CcInvocation::parse(&arguments).expect("admitted");
    let mut context = context(&workspace, &target);
    context.inputs = vec![
        CcActionInput {
            path: workspace.join("src/a.c"),
            digest: digest_of("source"),
        },
        CcActionInput {
            path: PathBuf::from("/usr/include/stdio.h"),
            digest: digest_of("stdio"),
        },
    ];
    let action = invocation.action(context).expect("action");
    let descriptor = String::from_utf8(action.bytes).expect("utf-8");
    assert!(descriptor.contains("/usr/include/stdio.h"), "{descriptor}");
}

#[cfg(unix)]
#[test]
fn unmapped_absolute_include_paths_bypass() {
    let (workspace, target) = checkout("one");
    let arguments = argv(&["-I/opt/homebrew/include", "-c", "-o", "out.o", "src/a.c"]);
    let invocation = CcInvocation::parse(&arguments).expect("admitted");
    let mut context = context(&workspace, &target);
    context.inputs = vec![CcActionInput {
        path: workspace.join("src/a.c"),
        digest: digest_of("source"),
    }];
    assert_eq!(
        invocation.action(context).unwrap_err().kind(),
        "unmapped-absolute-path"
    );
}

#[test]
fn a_missing_required_input_is_not_publishable() {
    let (workspace, target) = checkout("one");
    let arguments = argv(&["-c", "-o", "out.o", "src/a.c"]);
    let invocation = CcInvocation::parse(&arguments).expect("admitted");
    let context = context(&workspace, &target);
    assert_eq!(
        invocation.action(context).unwrap_err().kind(),
        "missing-required-input"
    );
}

#[test]
fn include_path_environment_variables_bypass_when_set() {
    for name in BYPASS_ENVIRONMENT {
        let reason = environment_inputs(
            |candidate| (candidate == *name).then(|| "/opt/include".to_string()),
            None,
        )
        .unwrap_err();
        assert_eq!(reason.kind(), "unsupported-environment");
        assert!(reason.to_string().contains(name), "{reason}");
    }
}

#[test]
fn deployment_target_and_sdkroot_enter_the_key_verbatim() {
    let environment = environment_inputs(
        |name| match name {
            "MACOSX_DEPLOYMENT_TARGET" => Some("11.0".into()),
            "SDKROOT" => Some("/sdk".into()),
            _ => None,
        },
        None,
    )
    .expect("environment should be admitted");
    assert_eq!(
        environment.get("MACOSX_DEPLOYMENT_TARGET"),
        Some(&Some("11.0".to_string()))
    );
    assert_eq!(environment.get("SDKROOT"), Some(&Some("/sdk".to_string())));
    // Unset variables are still recorded, so setting one later is a miss
    // rather than an unnoticed change.
    assert_eq!(environment.get("SOURCE_DATE_EPOCH"), Some(&None));
}

#[test]
fn an_explicit_sysroot_argument_replaces_the_sdkroot_variable_in_the_key() {
    let environment = environment_inputs(
        |name| (name == "SDKROOT").then(|| "/sdk".into()),
        Some(Path::new("/other")),
    )
    .expect("environment should be admitted");
    assert!(!environment.contains_key("SDKROOT"));
}

#[test]
fn compiler_families_classify_from_probe_text() {
    assert_eq!(
        CcCompilerFamily::classify("Apple clang version 15.0.0 (clang-1500.3.9.4)").unwrap(),
        CcCompilerFamily::AppleClang
    );
    assert_eq!(
        CcCompilerFamily::classify("clang version 17.0.6").unwrap(),
        CcCompilerFamily::Clang
    );
    assert_eq!(
        CcCompilerFamily::classify("gcc version 13.2.0 (Debian 13.2.0-1)").unwrap(),
        CcCompilerFamily::Gcc
    );
}

#[test]
fn msvc_style_probe_output_bypasses_as_an_unsupported_driver() {
    let probe = "Microsoft (R) C/C++ Optimizing Compiler Version 19.38\n";
    assert_eq!(
        CcCompilerFamily::classify(probe).unwrap_err().kind(),
        "unsupported-compiler-driver"
    );
}

#[test]
fn only_gcc_carries_an_external_assembler_in_its_identity() {
    assert!(CcCompilerFamily::Gcc.uses_external_assembler());
    assert!(!CcCompilerFamily::Clang.uses_external_assembler());
    assert!(!CcCompilerFamily::AppleClang.uses_external_assembler());
}

#[test]
fn the_assembler_identity_changes_the_action_key() {
    let build = |assembler: &str| {
        let (workspace, target) = checkout("one");
        let arguments = argv(&["-c", "-o", "out.o", "src/a.c"]);
        let invocation = CcInvocation::parse(&arguments).expect("admitted");
        let mut context = context(&workspace, &target);
        context.compiler.family = CcCompilerFamily::Gcc;
        context.compiler.assembler = assembler.into();
        context.inputs = vec![CcActionInput {
            path: workspace.join("src/a.c"),
            digest: digest_of("source"),
        }];
        invocation.action(context).expect("action").digest
    };
    assert_ne!(
        build("/usr/bin/as; GNU assembler 2.40"),
        build("/usr/bin/as; GNU assembler 2.42"),
    );
}

#[test]
fn predictions_round_trip_through_normalized_names() {
    let (workspace, target) = checkout("one");
    let arguments = argv(&["-c", "-o", "out.o", "src/a.c"]);
    let invocation = CcInvocation::parse(&arguments).expect("admitted");
    let mut context = context(&workspace, &target);
    context.inputs = vec![
        CcActionInput {
            path: workspace.join("src/a.c"),
            digest: digest_of("source"),
        },
        CcActionInput {
            path: PathBuf::from(format!(
                "{INCLUDE_MANIFEST_PREFIX}{}",
                text(&workspace.join("src"))
            )),
            digest: digest_of("manifest"),
        },
    ];
    let prediction = invocation.prediction(&context, 42).expect("prediction");
    assert_eq!(prediction.version, 1);
    assert_eq!(prediction.compiler_duration_ns, 42);
    assert_eq!(prediction.source_name, "a.c");
    assert!(
        prediction
            .inputs
            .contains(&"${workspace}/src/a.c".to_string())
    );
    assert!(
        prediction
            .inputs
            .contains(&format!("{INCLUDE_MANIFEST_PREFIX}${{workspace}}/src"))
    );
}

#[cfg(unix)]
#[test]
fn predicted_system_paths_only_denormalize_under_admitted_roots() {
    let mappings = PathMapping::ordered(&[PathMapping::new("/work/one", "workspace")]);
    assert_eq!(
        denormalize_path("${workspace}/src/a.c", &mappings).unwrap(),
        PathBuf::from("/work/one/src/a.c")
    );
    assert_eq!(
        denormalize_path("/usr/include/stdio.h", &mappings).unwrap(),
        PathBuf::from("/usr/include/stdio.h")
    );
    for hostile in ["/etc/passwd", "${workspace}/../escape", "relative/path"] {
        assert_eq!(
            denormalize_path(hostile, &mappings).unwrap_err().kind(),
            "invalid-predicted-input",
            "{hostile} should not resolve"
        );
    }
}

#[test]
fn a_prediction_from_a_future_schema_bypasses() {
    let prediction = CcInputPrediction {
        version: 2,
        inputs: Vec::new(),
        environment: Vec::new(),
        compiler_duration_ns: 0,
        source_name: String::new(),
    };
    assert_eq!(
        prediction
            .discover(Path::new("/work/one"), &[])
            .unwrap_err()
            .kind(),
        "unsupported-prediction"
    );
}

/// Flags observed on real sys-crate builds, which the first cut of the
/// allowlist rejected. Each one is deterministic key material.
#[test]
fn flags_real_sys_crates_pass_are_admitted() {
    let cases = [
        "--include=/registry/aws-lc-sys-0.44.0/generated-include/prefix.h",
        "-mno-omit-leaf-frame-pointer",
        "-fmerge-all-constants",
        "--param=ssp-buffer-size=4",
    ];
    for flag in cases {
        let arguments = argv(&[flag, "-c", "-o", "a.o", "a.c"]);
        assert!(
            CcInvocation::parse(&arguments).is_ok(),
            "{flag} should be admitted"
        );
    }
}

/// The `cc` crate probes drivers with `-?` to tell MSVC from gcc. That is a
/// question, not a compilation, and must not read as an unmodeled flag.
#[test]
fn the_msvc_probe_flag_reads_as_a_compiler_query() {
    let arguments = argv(&["-?"]);
    assert_eq!(
        CcInvocation::parse(&arguments).unwrap_err().kind(),
        "compiler-query"
    );
}

/// A prefix map rewrites paths inside the object. Normalizing its left side is
/// what lets two checkouts agree on the key, since the path being rewritten is
/// itself checkout-specific.
#[test]
fn prefix_map_sources_normalize_while_replacements_stay_verbatim() {
    let build = |name: &str| {
        let (workspace, target) = checkout(name);
        let flag = format!("-ffile-prefix-map={}=", text(&workspace));
        let arguments = argv(&[&flag, "-c", "-o", "out.o", "src/a.c"]);
        let invocation = CcInvocation::parse(&arguments).expect("admitted");
        let mut context = context(&workspace, &target);
        context.inputs = vec![CcActionInput {
            path: workspace.join("src/a.c"),
            digest: digest_of("source"),
        }];
        invocation.action(context).expect("action")
    };
    let one = build("one");
    let two = build("two");
    assert_eq!(one.digest, two.digest);
    let descriptor = String::from_utf8(one.bytes).expect("utf-8");
    assert!(
        descriptor.contains("-ffile-prefix-map=${workspace}="),
        "{descriptor}"
    );
}

/// A separate `--param` consumes its value rather than leaving it to be
/// mistaken for the source file.
#[test]
fn a_separate_param_value_is_not_mistaken_for_a_source() {
    let arguments = argv(&["--param", "ssp-buffer-size=4", "-c", "-o", "a.o", "a.c"]);
    let invocation = CcInvocation::parse(&arguments).expect("admitted");
    assert_eq!(invocation.source(), Path::new("a.c"));
}

/// `-I` is the glued include flag, and the lowercase `-i…` family is a
/// different set of flags entirely. Prefix-stripping the first must not
/// swallow the second, so each one still reaches the handler that knows
/// whether it names a search directory, a forced include, or a sysroot.
#[test]
fn lowercase_include_flags_are_not_swallowed_by_the_include_path_prefix() {
    let (workspace, target) = checkout("one");
    let sysroot = text(&workspace.join("sdk"));
    let arguments = argv(&[
        "-isystem",
        "vendor/include",
        "-iquote",
        "quoted",
        "-idirafter",
        "after",
        "-isysroot",
        &sysroot,
        "-include",
        "forced.h",
        "-imacros",
        "macros.h",
        "-c",
        "-o",
        "out.o",
        "src/a.c",
    ]);
    let invocation = CcInvocation::parse(&arguments).expect("admitted");

    // The source is still the source: a swallowed flag would have consumed it.
    assert_eq!(invocation.source(), Path::new("src/a.c"));
    assert_eq!(invocation.sysroot(), Some(workspace.join("sdk").as_path()));
    assert_eq!(
        invocation.include_dirs(),
        [
            PathBuf::from("vendor/include"),
            PathBuf::from("quoted"),
            PathBuf::from("after"),
        ]
    );
    assert!(
        invocation
            .required_inputs()
            .contains(&PathBuf::from("forced.h"))
    );
    assert!(
        invocation
            .required_inputs()
            .contains(&PathBuf::from("macros.h"))
    );

    // Each one is keyed under its own flag rather than as an include path.
    let mut context = context(&workspace, &target);
    context.inputs = ["src/a.c", "forced.h", "macros.h"]
        .into_iter()
        .map(|name| CcActionInput {
            path: workspace.join(name),
            digest: digest_of(name),
        })
        .collect();
    let action = invocation.action(context).expect("action");
    let descriptor = String::from_utf8(action.bytes).expect("utf-8");
    for flag in [
        "-isystem=",
        "-iquote=",
        "-idirafter=",
        "-include=",
        "-imacros=",
    ] {
        assert!(
            descriptor.contains(flag),
            "{flag} missing from {descriptor}"
        );
    }
}

/// A flag that resolves against the machine's own CPU makes the object depend
/// on something the key cannot name, so another machine must not restore it.
#[test]
fn tuning_for_the_local_cpu_bypasses() {
    for flag in [
        "-march=native",
        "-mcpu=native",
        "-mtune=native",
        "-march=host",
    ] {
        let arguments = argv(&[flag, "-c", "-o", "a.o", "a.c"]);
        assert_eq!(
            CcInvocation::parse(&arguments).unwrap_err().kind(),
            "local-cpu-target",
            "{flag} should bypass"
        );
    }
    // A named architecture is ordinary key material.
    let arguments = argv(&["-march=armv8-a", "-mtune=generic", "-c", "-o", "a.o", "a.c"]);
    assert!(CcInvocation::parse(&arguments).is_ok());
}
