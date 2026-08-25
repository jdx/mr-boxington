use super::*;

#[test]
fn combined_budget_reserves_the_full_action_store_allowance() {
    let retention = RetentionSettings {
        target_max_bytes: Some(80),
        target_max_age: None,
        max_total_bytes: Some(100),
    };

    assert_eq!(target_budget(&retention, 70), Some(30));
    assert_eq!(target_budget(&retention, 120), Some(0));
}

#[test]
fn prefers_the_outermost_lockfile_as_the_workspace_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let member = root.join("crates").join("member");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    std::fs::write(member.join("Cargo.toml"), "[package]\n").unwrap();

    assert_eq!(workspace_root(&member), root);
}

#[test]
fn falls_back_to_a_manifest_without_a_lockfile() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let nested = root.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();

    assert_eq!(workspace_root(&nested), root);
}

#[test]
fn falls_back_to_the_starting_directory() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(workspace_root(directory.path()), directory.path());
}

#[test]
fn reads_both_flag_spellings() {
    let joined = ["build".to_string(), "--target-dir=/tmp/out".to_string()];
    let split = [
        "build".to_string(),
        "--target-dir".to_string(),
        "/tmp/out".to_string(),
    ];
    let dangling = ["build".to_string(), "--target-dir".to_string()];

    assert_eq!(target_dir_argument(&joined), Some("/tmp/out"));
    assert_eq!(target_dir_argument(&split), Some("/tmp/out"));
    assert_eq!(target_dir_argument(&dangling), None);
    assert_eq!(target_dir_argument(&["build".to_string()]), None);
}

#[test]
fn reads_the_roots_cargo_reports() {
    let metadata = br#"{
            "workspace_root": "/elsewhere/project",
            "target_directory": "/var/cache/shared-target",
            "packages": []
        }"#;

    assert_eq!(
        parse_cargo_roots(metadata).unwrap(),
        Roots {
            workspace_root: PathBuf::from("/elsewhere/project"),
            target_dir: PathBuf::from("/var/cache/shared-target"),
            target_dir_requested: false,
        }
    );
}

#[test]
fn records_whether_anyone_asked_where_the_target_directory_goes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let plain = ["build".to_string(), "--offline".to_string()];

    assert!(
        !resolve_roots_with(std::ffi::OsStr::new("cargo"), &plain, root, None).target_dir_requested,
        "nothing asked, so placement is free to move it"
    );

    // The value is the default location, and the flag still means the caller
    // chose it. Cargo prefers the flag over the `CARGO_TARGET_DIR` a
    // placement would set, so this is the case that must not be moved.
    let flagged = [
        "build".to_string(),
        "--offline".to_string(),
        "--target-dir".to_string(),
        "target".to_string(),
    ];
    assert!(
        resolve_roots_with(std::ffi::OsStr::new("cargo"), &flagged, root, None)
            .target_dir_requested
    );

    assert!(
        resolve_roots_with(
            std::ffi::OsStr::new("cargo"),
            &plain,
            root,
            Some("/somewhere/else".into())
        )
        .target_dir_requested
    );
    assert!(
        !resolve_roots_with(std::ffi::OsStr::new("cargo"), &plain, root, Some("".into()))
            .target_dir_requested,
        "an empty variable names nothing, which is how cargo reads it too"
    );
}

#[test]
fn ignores_unusable_cargo_metadata() {
    assert!(parse_cargo_roots(b"not json").is_none());
    assert!(parse_cargo_roots(br#"{"packages": []}"#).is_none());
}

#[test]
fn resolves_a_relative_directory_against_the_working_directory() {
    let cwd = Path::new("/workspace/crates/member");
    assert_eq!(
        absolute(cwd, "out"),
        Path::new("/workspace/crates/member/out")
    );
    assert_eq!(absolute(cwd, "/tmp/out"), Path::new("/tmp/out"));
}

#[test]
fn carries_an_inherited_wrapper_into_the_session() {
    let cwd = Path::new("/workspace");
    let with = inherited_environment(
        |name| (name == "RUSTC_WRAPPER").then(|| "/usr/bin/sccache".to_string()),
        cwd,
    );
    assert_eq!(with.get("RUSTC_WRAPPER").unwrap(), "/usr/bin/sccache");

    // An empty value is how a shell unsets it in practice.
    let empty = inherited_environment(|name| (name == "RUSTC_WRAPPER").then(String::new), cwd);
    assert!(empty.is_empty());
    assert!(inherited_environment(|_| None, cwd).is_empty());
}

#[test]
fn absolutizes_the_bypass_log_before_the_shims_inherit_it() {
    let cwd = Path::new("/workspace");
    let relative = inherited_environment(
        |name| (name == crate::session::BYPASS_LOG_ENV).then(|| "bypass.log".to_string()),
        cwd,
    );
    // Left relative, each shim would resolve this against whichever crate
    // directory cargo happened to give it. Compare as paths: the separator
    // is not the same on every platform.
    assert_eq!(
        Path::new(relative.get(crate::session::BYPASS_LOG_ENV).unwrap()),
        cwd.join("bypass.log")
    );

    // An absolute destination is passed through untouched. Ask the platform
    // for one -- a leading slash is not absolute on Windows.
    let already = std::env::temp_dir().join("bypass.log");
    assert!(
        already.is_absolute(),
        "{} should be absolute",
        already.display()
    );
    let given = already.display().to_string();
    let absolute_path = inherited_environment(
        |name| (name == crate::session::BYPASS_LOG_ENV).then(|| given.clone()),
        cwd,
    );
    assert_eq!(
        Path::new(absolute_path.get(crate::session::BYPASS_LOG_ENV).unwrap()),
        already
    );

    assert!(
        !inherited_environment(
            |name| (name == crate::session::BYPASS_LOG_ENV).then(String::new),
            cwd
        )
        .contains_key(crate::session::BYPASS_LOG_ENV)
    );
}

#[test]
fn forwards_repeated_and_attached_global_flags() {
    let arguments = [
        "build",
        "--config",
        "build.target-dir=\"/one\"",
        "--config=net.offline=true",
        "-Zunstable-options",
        "-C",
        "/tree",
        "--release",
    ]
    .map(String::from);

    assert_eq!(
        forwarded_flags(&arguments, &PROBE_GLOBAL_FLAGS),
        [
            "--config",
            "build.target-dir=\"/one\"",
            "--config",
            "net.offline=true",
            "-Zunstable-options",
            "-C",
            "/tree",
        ]
    );
    // A flag the probe does not understand must not leak into it.
    assert!(
        forwarded_flags(&arguments, &PROBE_GLOBAL_FLAGS)
            .iter()
            .all(|argument| argument != "--release")
    );
}

#[test]
fn rustc_flags_after_the_separator_are_not_cargo_globals() {
    let arguments = [
        "build",
        "--config",
        "build.jobs=2",
        "--",
        "-C",
        "opt-level=3",
        "-Zunstable-thing",
        "--config",
        "not.cargos=1",
    ]
    .map(String::from);

    // Only the flag before `--` belongs to cargo. Forwarding rustc's `-C`
    // would send the probe looking for a directory called "opt-level=3".
    assert_eq!(
        forwarded_flags(cargo_arguments(&arguments), &PROBE_GLOBAL_FLAGS),
        ["--config", "build.jobs=2"]
    );
    let working_dir = Path::new("/workspace");
    assert_eq!(
        invocation_dir(cargo_arguments(&arguments), working_dir),
        working_dir
    );
    // Without the separator the same tokens are cargo's, and -C wins.
    let no_separator: Vec<String> = arguments
        .iter()
        .filter(|argument| *argument != "--")
        .cloned()
        .collect();
    assert_eq!(
        invocation_dir(cargo_arguments(&no_separator), working_dir),
        Path::new("/workspace/opt-level=3")
    );
}

#[test]
fn a_config_set_target_dir_reaches_the_probe() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();
    let manifest = root.join("Cargo.toml");
    let configured = root.join("configured-target");

    // Without the override cargo reports the default; with it the probe has
    // to report the same directory the build will write to, or the outputs
    // go unmapped and every action bypasses the cache.
    //
    // Both halves resolve as if `CARGO_TARGET_DIR` were unset. Cargo lets
    // that variable outrank a config-set `build.target-dir`, so leaving the
    // ambient one in place would make the probe correctly report it instead
    // of what is under test -- and the test would fail for anyone who
    // exports it, which plenty of developers do.
    let arguments = [
        "build".to_string(),
        "--offline".to_string(),
        "--manifest-path".to_string(),
        manifest.display().to_string(),
    ];
    let default = resolve_roots_with(std::ffi::OsStr::new("cargo"), &arguments, root, None);
    assert_eq!(default.target_dir, root.join("target"));

    let overridden = [
        arguments.to_vec(),
        vec![
            "--config".to_string(),
            // A TOML literal string: a Windows path's backslashes are
            // escape sequences inside a basic string, which silently
            // mangled the value and left the probe reporting the default.
            format!("build.target-dir='{}'", configured.display()),
        ],
    ]
    .concat();
    let roots = resolve_roots_with(std::ffi::OsStr::new("cargo"), &overridden, root, None);
    assert_eq!(roots.target_dir, configured);
    assert!(roots.target_dir_requested);
}

#[test]
fn a_cargo_config_that_names_the_default_target_is_still_explicit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::write(
        root.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"target\"\n",
    )
    .unwrap();
    let arguments = ["build".to_string(), "--offline".to_string()];

    let roots = resolve_roots_with(
        std::ffi::OsStr::new("cargo-that-does-not-exist"),
        &arguments,
        root,
        None,
    );

    assert_eq!(roots.target_dir, root.join("target"));
    assert!(roots.target_dir_requested);
}

#[test]
fn a_command_line_config_include_may_set_the_target_dir() {
    let arguments = [
        "build".to_string(),
        "--config".to_string(),
        "include='target-config.toml'".to_string(),
    ];

    assert!(cargo_config_may_set_target_dir(
        &arguments,
        Path::new("/workspace")
    ));
}

#[test]
fn all_non_reserved_subcommands_are_forwarded_directly() {
    for argv in [
        ["mbx", "new", "--vcs", "none"],
        ["mbx", "init", "--lib", "fixture"],
        ["mbx", "command-added-later", "--future-flag", "value"],
    ] {
        let argv = argv.map(std::ffi::OsStr::new);
        let cli = Cli::try_parse_from(&argv).unwrap();
        let Commands::Cargo(arguments) = cli.command else {
            panic!(
                "{} should be treated as a cargo subcommand",
                argv[1].to_string_lossy()
            );
        };
        let expected = argv[1..]
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(arguments, expected);
    }
}

#[test]
fn mbx_commands_still_take_precedence() {
    let argv = ["mbx", "gc", "--max-size", "1GiB"].map(std::ffi::OsStr::new);
    let cli = Cli::try_parse_from(&argv).unwrap();
    assert!(matches!(cli.command, Commands::Gc(_)));
}

#[test]
fn explain_forwards_cargo_flags_and_the_rustc_separator() {
    let argv = [
        "mbx",
        "explain",
        "clippy",
        "--workspace",
        "--",
        "-D",
        "warnings",
    ]
    .map(std::ffi::OsStr::new);
    let cli = Cli::try_parse_from(&argv).unwrap();
    let Commands::Explain(arguments) = cli.command else {
        panic!("explain should be reserved by mbx");
    };
    assert_eq!(
        arguments.arguments(),
        ["clippy", "--workspace", "--", "-D", "warnings"]
    );
}

#[test]
fn prefetch_preserves_cargo_flags_and_the_rustc_separator() {
    let argv = [
        "mbx",
        "prefetch",
        "test",
        "--workspace",
        "--",
        "--nocapture",
    ]
    .map(std::ffi::OsStr::new);

    let cli = Cli::try_parse_from(&argv).unwrap();

    let Commands::Prefetch(args) = cli.command else {
        panic!("prefetch should remain an mbx command");
    };
    assert_eq!(args.cargo_args, ["test", "--workspace", "--nocapture"]);
    let original = argv
        .iter()
        .map(|argument| argument.to_os_string())
        .collect::<Vec<_>>();
    assert_eq!(
        original_prefetch_arguments(&original).unwrap(),
        ["test", "--workspace", "--", "--nocapture"]
    );
}

#[test]
fn prefetch_rejects_release_contexts_instead_of_succeeding_without_remote_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = managed_target_config(directory.path());
    config.remote.url = Some("https://cache.example.test".into());
    config.remote.mode = mbx_cache_core::RemoteCacheMode::ReadOnly;

    let error = validate_prefetch_config(&config, true).unwrap_err();

    assert!(error.to_string().contains("disabled in release contexts"));
    assert!(validate_prefetch_config(&config, false).is_ok());
}

#[test]
fn cli_exposes_its_usage_spec() {
    let spec = Cli::to_kdl();
    assert!(spec.contains("external_subcommand #true"));
    assert!(spec.contains("cmd setup"));
    assert!(spec.contains("cmd explain"));
    assert!(spec.contains("cmd doctor"));
    assert!(spec.contains("cmd gc"));
    assert!(spec.contains("cmd cache"));
    assert!(spec.contains("config {"));
    assert!(spec.contains(r#"prop "gc.max_size""#));
    assert!(spec.contains(r#"env "MBX_GC_MAX_SIZE""#));
}

#[test]
fn cargo_help_does_not_trigger_target_migration() {
    assert!(cargo_help_requested(&["build".into(), "--help".into()]));
    assert!(cargo_help_requested(&["help".into(), "build".into()]));
    assert!(!cargo_help_requested(&["build".into(), "--release".into()]));
}

fn managed_target_config(root: &Path) -> Config {
    Config {
        cache_dir: root.join("cache"),
        stats_report: None,
        verify: false,
        incremental: false,
        share_out_dir: false,
        remote: Default::default(),
        http: Default::default(),
        gc: Default::default(),
        target: crate::config::TargetSettings {
            views: true,
            root: root.join("targets"),
        },
    }
}

#[test]
fn accepting_the_target_prompt_requests_migration_without_removing_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("project");
    let target_dir = workspace.join("target");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(target_dir.join("artifact"), b"old output").unwrap();
    let config = managed_target_config(directory.path());
    let roots = Roots {
        workspace_root: workspace,
        target_dir: target_dir.clone(),
        target_dir_requested: false,
    };

    let accepted = prompt_to_manage_existing_target_with(&config, &roots, |_| Ok(true)).unwrap();

    assert!(accepted);
    assert!(target_dir.join("artifact").is_file());
}

#[test]
fn declining_the_target_prompt_preserves_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("project");
    let target_dir = workspace.join("target");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(target_dir.join("artifact"), b"old output").unwrap();
    let config = managed_target_config(directory.path());
    let roots = Roots {
        workspace_root: workspace,
        target_dir: target_dir.clone(),
        target_dir_requested: false,
    };

    let accepted = prompt_to_manage_existing_target_with(&config, &roots, |_| Ok(false)).unwrap();

    assert!(!accepted);
    assert!(target_dir.join("artifact").is_file());
}

#[test]
fn setup_preserves_cargo_configuration_and_installs_the_wrapper() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory
        .path()
        .join(if cfg!(windows) { "mbx.exe" } else { "mbx" });
    std::fs::write(&executable, b"mbx binary").unwrap();
    let install = directory.path().join("data/bin");
    let config = directory.path().join("cargo/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "# keep me\n[net]\noffline = true\n").unwrap();

    setup_at(&executable, &install, &config).unwrap();

    let written = std::fs::read_to_string(&config).unwrap();
    assert!(written.contains("# keep me"));
    let document = written.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(document["net"]["offline"].as_bool(), Some(true));
    let wrapper = document["build"]["rustc-wrapper"].as_str().unwrap();
    assert!(Path::new(wrapper).is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_ne!(
            std::fs::metadata(wrapper).unwrap().permissions().mode() & 0o100,
            0
        );
    }
}

#[test]
fn setup_never_displaces_an_existing_wrapper() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("mbx");
    std::fs::write(&executable, b"mbx binary").unwrap();
    let install = directory.path().join("data/bin");
    let config = directory.path().join("cargo/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = "[build]\nrustc-wrapper = \"sccache\"\n";
    std::fs::write(&config, original).unwrap();

    setup_at(&executable, &install, &config).unwrap();

    assert_eq!(std::fs::read_to_string(config).unwrap(), original);
    assert!(!install.exists());
}

#[test]
fn setup_flags_are_mutually_exclusive() {
    let args = SetupArgs {
        status: true,
        update: true,
        uninstall: false,
    };
    assert!(args.action().is_err());
    assert_eq!(
        SetupArgs {
            status: false,
            update: false,
            uninstall: true,
        }
        .action()
        .unwrap(),
        SetupAction::Uninstall
    );
}

#[test]
fn setup_status_detects_and_update_refreshes_a_stale_wrapper() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("mbx");
    std::fs::write(&executable, b"first mbx binary").unwrap();
    let install = directory.path().join("data/bin");
    let config = directory.path().join("cargo/config.toml");

    setup_at_action(&executable, &install, &config, SetupAction::Install).unwrap();
    assert_eq!(
        setup_at_action(&executable, &install, &config, SetupAction::Status).unwrap(),
        ExitCode::SUCCESS
    );

    std::fs::remove_file(&executable).unwrap();
    std::fs::write(&executable, b"new mbx binary with another size").unwrap();
    assert_eq!(
        setup_at_action(&executable, &install, &config, SetupAction::Status).unwrap(),
        ExitCode::FAILURE
    );
    setup_at_action(&executable, &install, &config, SetupAction::Update).unwrap();
    let shim = install.join(if cfg!(windows) {
        "mbx-rustc.exe"
    } else {
        "mbx-rustc"
    });
    assert!(same_file_contents(&executable, &shim).unwrap());
}

#[test]
fn setup_uninstall_removes_only_mbx_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("mbx");
    std::fs::write(&executable, b"mbx binary").unwrap();
    let install = directory.path().join("data/bin");
    let config = directory.path().join("cargo/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "# keep me\n[net]\noffline = true\n").unwrap();

    setup_at_action(&executable, &install, &config, SetupAction::Install).unwrap();
    let shim = install.join(if cfg!(windows) {
        "mbx-rustc.exe"
    } else {
        "mbx-rustc"
    });
    assert!(shim.is_file());
    setup_at_action(&executable, &install, &config, SetupAction::Uninstall).unwrap();

    let contents = std::fs::read_to_string(&config).unwrap();
    assert!(contents.contains("# keep me"));
    assert!(contents.contains("offline = true"));
    assert!(!contents.contains("rustc-wrapper"));
    assert!(!shim.exists());
    setup_at_action(&executable, &install, &config, SetupAction::Uninstall).unwrap();
}

#[test]
fn setup_update_requires_an_existing_installation() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("mbx");
    std::fs::write(&executable, b"mbx binary").unwrap();
    let error = setup_at_action(
        &executable,
        &directory.path().join("data/bin"),
        &directory.path().join("cargo/config.toml"),
        SetupAction::Update,
    )
    .unwrap_err();
    assert!(error.to_string().contains("not installed"));

    let config = directory.path().join("cargo/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[build]\nrustc-wrapper = \"sccache\"\n").unwrap();
    let error = setup_at_action(
        &executable,
        &directory.path().join("data/bin"),
        &config,
        SetupAction::Update,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("refusing to replace another wrapper")
    );
}
