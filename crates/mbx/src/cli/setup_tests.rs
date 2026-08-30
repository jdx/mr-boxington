use super::*;

#[test]
fn setup_installs_the_cargo_shim() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory
        .path()
        .join(if cfg!(windows) { "mbx.exe" } else { "mbx" });
    std::fs::write(&executable, b"mbx binary").unwrap();
    let install = directory.path().join("data/bin");
    setup_at(&executable, &install).unwrap();

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

    std::fs::remove_file(&executable).unwrap();
    std::fs::write(&executable, b"new mbx binary with another size").unwrap();
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
    let shim = install.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    assert!(same_file_contents(&executable, &shim).unwrap());
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
