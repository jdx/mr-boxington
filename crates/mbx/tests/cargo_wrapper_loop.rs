#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// A Cargo wrapper that execs mbx, selected as the real Cargo, must produce a
/// diagnosis instead of a process chain that never ends.
#[test]
fn wrapper_that_runs_mbx_again_reports_the_loop() {
    let root = tempfile::tempdir().unwrap();
    let mbx = env!("CARGO_BIN_EXE_mbx");
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='loop-fixture'\nversion='0.0.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let wrapper = bin.join("cargo");
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\nMBX_CARGO_SHIM_MODE=1 exec '{mbx}' \"$@\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(mbx)
        .current_dir(root.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.path())
        .env("MBX_CACHE_DIR", root.path().join("cache"))
        .env("MBX_CARGO_SHIM_MODE", "1")
        .env("CARGO", &wrapper)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("re-entered its Cargo shim"), "{stderr}");
    assert!(stderr.contains("CARGO environment variable"), "{stderr}");
}
