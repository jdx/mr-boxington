use super::*;

#[test]
fn setup_installs_the_cargo_shim() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory
        .path()
        .join(if cfg!(windows) { "mbx.exe" } else { "mbx" });
    std::fs::write(&executable, b"mbx binary").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
    }
    let install = directory.path().join("data/bin");
    setup_at(&executable, &install).unwrap();

    let wrapper = install.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    assert!(wrapper.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert!(
            !std::fs::symlink_metadata(&wrapper)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_ne!(
            std::fs::metadata(&wrapper).unwrap().permissions().mode() & 0o100,
            0
        );
        std::fs::remove_file(&executable).unwrap();
        std::fs::write(&executable, b"new mbx binary").unwrap();
        assert_eq!(std::fs::read(&wrapper).unwrap(), CARGO_SHIM_LAUNCHER);
    }
}

#[test]
fn setup_puts_rust_analyzer_checks_through_the_stable_cargo_shim() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory
        .path()
        .join(if cfg!(windows) { "mbx.exe" } else { "mbx" });
    std::fs::write(&executable, b"mbx binary").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
    }
    let install = directory.path().join("data/bin");
    let config = directory
        .path()
        .join("config/rust-analyzer/rust-analyzer.toml");

    setup_with_rust_analyzer(
        &executable,
        &install,
        &MiseScope::None,
        &config,
        SetupAction::Install,
    )
    .unwrap();

    let document = std::fs::read_to_string(&config)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    let command = document["check"]["overrideCommand"].as_array().unwrap();
    let shim = install.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    assert!(command.iter().map(|value| value.as_str().unwrap()).eq([
        shim.to_str().unwrap(),
        "check",
        "--workspace",
        "--all-targets",
        "--message-format=json",
    ]));
    assert_eq!(
        configure_rust_analyzer(&config, &shim, SetupAction::Status).unwrap(),
        ExitCode::SUCCESS
    );
}

#[test]
fn setup_preserves_an_existing_rust_analyzer_command() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("rust-analyzer.toml");
    let original = "# keep me\n[check]\noverrideCommand = [\"cargo\", \"clippy\"]\n";
    std::fs::write(&config, original).unwrap();

    configure_rust_analyzer(
        &config,
        &directory.path().join("bin/cargo"),
        SetupAction::Install,
    )
    .unwrap();
    configure_rust_analyzer(
        &config,
        &directory.path().join("bin/cargo"),
        SetupAction::Uninstall,
    )
    .unwrap();

    assert_eq!(std::fs::read_to_string(config).unwrap(), original);
}

#[test]
fn setup_preserves_existing_rust_analyzer_check_settings() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("rust-analyzer.toml");
    let original = "# keep me\n[check]\ncommand = \"clippy\"\nfeatures = [\"editor\"]\n";
    std::fs::write(&config, original).unwrap();

    configure_rust_analyzer(
        &config,
        &directory.path().join("bin/cargo"),
        SetupAction::Install,
    )
    .unwrap();

    assert_eq!(std::fs::read_to_string(config).unwrap(), original);
}

#[test]
fn project_rust_analyzer_config_follows_the_active_cargo_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    let crate_root = project.join("crates/app");
    let source = crate_root.join("src");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(crate_root.join("Cargo.toml"), "[workspace]\n").unwrap();
    let mise_config = project.join("mise.toml");
    std::fs::write(&mise_config, "").unwrap();

    assert_eq!(
        rust_analyzer_config_path_from(&MiseScope::File(mise_config), &source,).unwrap(),
        crate_root.join("rust-analyzer.toml")
    );
}

#[test]
fn setup_uninstall_removes_only_its_rust_analyzer_command() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("rust-analyzer.toml");
    let shim = directory.path().join("bin/cargo");
    configure_rust_analyzer(&config, &shim, SetupAction::Install).unwrap();
    let mut document = std::fs::read_to_string(&config)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    document["check"]["ignore"] = toml_edit::value(toml_edit::Array::from_iter(["dead_code"]));
    std::fs::write(&config, document.to_string()).unwrap();

    configure_rust_analyzer(&config, &shim, SetupAction::Uninstall).unwrap();

    let written = std::fs::read_to_string(config).unwrap();
    assert!(written.contains("ignore"));
    assert!(!written.contains("overrideCommand"));
}

#[test]
fn setup_status_detects_a_missing_rust_analyzer_command() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        configure_rust_analyzer(
            &directory.path().join("rust-analyzer.toml"),
            &directory.path().join("bin/cargo"),
            SetupAction::Status,
        )
        .unwrap(),
        ExitCode::FAILURE
    );
}

#[test]
fn setup_flags_are_mutually_exclusive() {
    let args = SetupArgs {
        yes: false,
        global: false,
        local: false,
        status: true,
        uninstall: true,
    };
    assert!(args.action().is_err());
    assert!(
        SetupArgs {
            yes: false,
            global: true,
            local: true,
            status: false,
            uninstall: false,
        }
        .validate()
        .is_err()
    );
    assert_eq!(
        SetupArgs {
            yes: false,
            global: false,
            local: false,
            status: false,
            uninstall: true,
        }
        .action()
        .unwrap(),
        SetupAction::Uninstall
    );
}

#[test]
fn setup_status_detects_and_setup_refreshes_a_replaced_wrapper() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("mbx");
    std::fs::write(&executable, b"first mbx binary").unwrap();
    let install = directory.path().join("data/bin");
    setup_at_action(
        &executable,
        &install,
        &MiseScope::None,
        SetupAction::Install,
    )
    .unwrap();
    assert_eq!(
        setup_at_action(&executable, &install, &MiseScope::None, SetupAction::Status,).unwrap(),
        ExitCode::SUCCESS
    );

    let shim = install.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    std::fs::remove_file(&shim).unwrap();
    std::fs::write(&shim, b"stale mbx binary").unwrap();
    #[cfg(windows)]
    std::fs::remove_file(install.join(super::CARGO_SHIM_TARGET_FILE)).unwrap();
    assert_eq!(
        setup_at_action(&executable, &install, &MiseScope::None, SetupAction::Status,).unwrap(),
        ExitCode::FAILURE
    );
    setup_at_action(
        &executable,
        &install,
        &MiseScope::None,
        SetupAction::Install,
    )
    .unwrap();
    assert!(cargo_shim_is_current(&executable, &shim).unwrap());
}

#[test]
fn setup_uninstall_keeps_the_shared_shim() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("mbx");
    std::fs::write(&executable, b"mbx binary").unwrap();
    let install = directory.path().join("data/bin");
    setup_at_action(
        &executable,
        &install,
        &MiseScope::None,
        SetupAction::Install,
    )
    .unwrap();
    let shim = install.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    assert!(shim.is_file());
    setup_at_action(
        &executable,
        &install,
        &MiseScope::None,
        SetupAction::Uninstall,
    )
    .unwrap();

    assert!(shim.exists());
    setup_at_action(
        &executable,
        &install,
        &MiseScope::None,
        SetupAction::Uninstall,
    )
    .unwrap();
}

#[test]
fn mise_config_selection_finds_the_first_config_that_defines_mbx() {
    let configs = br#"[
        {"path":"/project/mise.toml","tools":["rust","github:jdx/mr-boxington"]},
        {"path":"/global/config.toml","tools":["mr-boxington"]}
    ]"#;

    assert_eq!(
        mbx_mise_config_from_json(configs),
        Some(PathBuf::from("/project/mise.toml"))
    );
    assert_eq!(mbx_mise_config_from_json(b"not json"), None);
}

#[test]
fn mise_wrapper_version_parsing_uses_the_calendar_version() {
    assert_eq!(
        mise_version_from_output(b"2026.8.16 linux-x64 (2026-08-31)"),
        Some((2026, 8, 16))
    );
    assert_eq!(
        mise_version_from_output(b"v2027.1.2 windows-x64"),
        Some((2027, 1, 2))
    );
    assert_eq!(mise_version_from_output(b"not-a-version"), None);
}

#[test]
fn mise_wrapper_detection_requires_mbx_shim_mode() {
    let configured = r#"
[wrappers.cargo]
command = "mbx"
env = { MBX_CARGO_SHIM_MODE = "1" }
"#
    .parse::<toml_edit::DocumentMut>()
    .unwrap();
    assert!(mise_wrapper_is_configured_in(&configured));

    for raw in [
        "[wrappers]\ncargo = \"mbx\"\n",
        "[wrappers.cargo]\ncommand = \"mbx\"\n",
        "[wrappers.cargo]\ncommand = \"other\"\nenv = { MBX_CARGO_SHIM_MODE = \"1\" }\n",
    ] {
        let document = raw.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(!mise_wrapper_is_configured_in(&document));
    }
}
