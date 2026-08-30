use super::*;

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
fn cargo_help_does_not_trigger_target_migration() {
    assert!(cargo_help_requested(&["build".into(), "--help".into()]));
    assert!(cargo_help_requested(&["help".into(), "build".into()]));
    assert!(!cargo_help_requested(&["build".into(), "--release".into()]));
}

#[test]
fn the_first_run_notice_states_the_resolved_caps() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = managed_target_config(directory.path());
    config.gc.max_bytes = 12 * 1024 * 1024 * 1024;
    let retention = RetentionSettings {
        target_max_bytes: Some(25 * 1024 * 1024 * 1024),
        target_max_age: Some(std::time::Duration::from_secs(30 * 86_400)),
        max_total_bytes: None,
    };

    let notice = first_run_notice(&config, &retention, false);

    assert!(notice.contains("first build on this machine"));
    assert!(notice.contains(&config.cache_dir.display().to_string()));
    // The budgets scale with the disk, so the notice has to report what was
    // resolved rather than a number written into the sentence.
    assert!(notice.contains("12.0 GiB"), "{notice}");
    assert!(notice.contains("25.0 GiB"), "{notice}");
    assert!(notice.contains("30 days"), "{notice}");
    assert!(notice.contains("its checkout is gone"), "{notice}");
}

#[test]
fn the_first_run_notice_omits_limits_that_are_off() {
    let directory = tempfile::tempdir().unwrap();
    let config = managed_target_config(directory.path());
    let retention = RetentionSettings {
        target_max_bytes: None,
        target_max_age: None,
        max_total_bytes: None,
    };

    let notice = first_run_notice(&config, &retention, false);

    assert!(notice.contains("its checkout is gone"), "{notice}");
    assert!(!notice.contains("unused for"), "{notice}");
    assert!(!notice.contains("GiB total"), "{notice}");
}

#[test]
fn the_first_run_notice_does_not_promise_collection_that_is_off() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = managed_target_config(directory.path());
    config.gc.auto = false;

    let notice = first_run_notice(&config, &RetentionSettings::default(), false);

    assert!(notice.contains("automatic collection is off"), "{notice}");
    assert!(!notice.contains("pruned to"), "{notice}");
    assert!(!notice.contains("target/ is managed"), "{notice}");
}

#[test]
fn the_first_run_notice_skips_targets_it_does_not_manage() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = managed_target_config(directory.path());
    config.target.views = false;

    let notice = first_run_notice(&config, &RetentionSettings::default(), false);

    assert!(!notice.contains("target/ is managed"), "{notice}");
    assert!(notice.contains("pruned to"), "{notice}");
}

#[test]
fn the_first_run_notice_promises_reflinks_only_when_proven() {
    let directory = tempfile::tempdir().unwrap();
    let config = managed_target_config(directory.path());

    let with = first_run_notice(&config, &RetentionSettings::default(), true);
    let without = first_run_notice(&config, &RetentionSettings::default(), false);

    assert!(with.contains("supports reflinks"), "{with}");
    assert!(with.contains("instead of copying"), "{with}");
    // A machine whose filesystem copies must not be told its restores are
    // free; silence beats a promise the disk will break.
    assert!(!without.contains("reflink"), "{without}");
}

#[test]
fn reasons_read_as_prose() {
    assert_eq!(join_clauses(&["one".to_string()]), "one");
    assert_eq!(
        join_clauses(&["one".to_string(), "two".to_string()]),
        "one or two"
    );
    assert_eq!(
        join_clauses(&["one".to_string(), "two".to_string(), "three".to_string()]),
        "one, two, or three"
    );
}

pub(super) fn managed_target_config(root: &Path) -> Config {
    Config {
        cache_dir: root.join("cache"),
        stats_report: None,
        verify: false,
        incremental: false,
        share_out_dir: false,
        events: false,
        cc: false,
        remote: Default::default(),
        http: Default::default(),
        gc: Default::default(),
        scheduler: Default::default(),
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
