use super::*;

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
    assert!(document.get("build").is_none());
    let wrapper = install.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    assert!(wrapper.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_ne!(
            std::fs::metadata(&wrapper).unwrap().permissions().mode() & 0o100,
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
    assert!(
        install
            .join(if cfg!(windows) { "cargo.exe" } else { "cargo" })
            .is_file()
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
fn setup_status_detects_and_setup_refreshes_a_stale_wrapper() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("mbx");
    std::fs::write(&executable, b"first mbx binary").unwrap();
    let install = directory.path().join("data/bin");
    let config = directory.path().join("cargo/config.toml");

    setup_at_action(
        &executable,
        &install,
        &config,
        &MiseScope::None,
        SetupAction::Install,
    )
    .unwrap();
    assert_eq!(
        setup_at_action(
            &executable,
            &install,
            &config,
            &MiseScope::None,
            SetupAction::Status,
        )
        .unwrap(),
        ExitCode::SUCCESS
    );

    std::fs::remove_file(&executable).unwrap();
    std::fs::write(&executable, b"new mbx binary with another size").unwrap();
    assert_eq!(
        setup_at_action(
            &executable,
            &install,
            &config,
            &MiseScope::None,
            SetupAction::Status,
        )
        .unwrap(),
        ExitCode::FAILURE
    );
    setup_at_action(
        &executable,
        &install,
        &config,
        &MiseScope::None,
        SetupAction::Install,
    )
    .unwrap();
    let shim = install.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
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

    setup_at_action(
        &executable,
        &install,
        &config,
        &MiseScope::None,
        SetupAction::Install,
    )
    .unwrap();
    let shim = install.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    assert!(shim.is_file());
    setup_at_action(
        &executable,
        &install,
        &config,
        &MiseScope::None,
        SetupAction::Uninstall,
    )
    .unwrap();

    let contents = std::fs::read_to_string(&config).unwrap();
    assert!(contents.contains("# keep me"));
    assert!(contents.contains("offline = true"));
    assert!(!contents.contains("rustc-wrapper"));
    assert!(shim.exists());
    setup_at_action(
        &executable,
        &install,
        &config,
        &MiseScope::None,
        SetupAction::Uninstall,
    )
    .unwrap();
}
