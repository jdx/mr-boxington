use std::process::{Command, Output};

fn gc(cache: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mbx"))
        .env_clear()
        .env("HOME", cache)
        .env("MBX_CACHE_DIR", cache)
        .env("MBX_GC_AUTO", "0")
        .arg("gc")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn the_automatic_sweep_rejects_a_named_compiler() {
    let cache = tempfile::tempdir().unwrap();

    let output = gc(cache.path(), &["--automatic", "--rustc-version", "1.97.1"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("takes no other option"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_named_compiler_is_accepted_by_an_explicit_dry_run() {
    let cache = tempfile::tempdir().unwrap();

    let output = gc(cache.path(), &["--dry-run", "--rustc-version", "1.97.1"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
