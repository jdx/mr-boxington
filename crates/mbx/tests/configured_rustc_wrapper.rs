//! `build.rustc-wrapper = "mbx"` in Cargo configuration makes Cargo run
//! `mbx <rustc> <args>`. Real Cargo is the oracle that mbx reports the
//! unsupported setting instead of Cargo's `-Zscript` error.
#![cfg(unix)]

use std::process::Command;

#[test]
fn cargo_reports_mbx_configured_as_the_rustc_wrapper() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='wrapper-fixture'\nversion='0.0.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .arg("build")
        .arg("--config")
        .arg(format!(
            "build.rustc-wrapper='{}'",
            env!("CARGO_BIN_EXE_mbx")
        ))
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "build should fail:\n{stderr}");
    assert!(stderr.contains("Remove that setting"), "{stderr}");
    assert!(!stderr.contains("-Zscript"), "{stderr}");
}
