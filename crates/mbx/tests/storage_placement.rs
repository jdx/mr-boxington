//! Exercise startup rejection without requiring an NFS mount or mount privileges.
#![cfg(all(target_os = "linux", target_env = "gnu"))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

/// Only the test process tree sees this statfs interposer. Actual file I/O
/// remains local; the designated directory reports the NFS filesystem type.
fn interposer() -> &'static Path {
    static LIBRARY: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &LIBRARY
        .get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("statfs.c");
            let library = directory.path().join("statfs.so");
            std::fs::write(
                &source,
                r#"
#define _GNU_SOURCE
#include <sys/vfs.h>
#include <dlfcn.h>
#include <stdlib.h>
#include <string.h>
int statfs(const char *path, struct statfs *buf) {
    int (*real_statfs)(const char *, struct statfs *) = dlsym(RTLD_NEXT, "statfs");
    int result = real_statfs(path, buf);
    const char *root = getenv("TEST_NFS_ROOT");
    if (result == 0 && root && strncmp(path, root, strlen(root)) == 0
        && (path[strlen(root)] == '/' || path[strlen(root)] == '\0')) {
        buf->f_type = 0x6969;
    }
    return result;
}
"#,
            )
            .unwrap();
            let output = Command::new("cc")
                .args(["-shared", "-fPIC"])
                .arg(&source)
                .arg("-o")
                .arg(&library)
                .arg("-ldl")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            (directory, library)
        })
        .1
}

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    nfs: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let nfs = root.join("nfs");
        let project = root.join("project");
        std::fs::create_dir(&nfs).unwrap();
        write_project(&project);
        Self {
            _directory: directory,
            root,
            nfs,
            project,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
        command
            .current_dir(&self.project)
            .env("LD_PRELOAD", interposer())
            .env("TEST_NFS_ROOT", &self.nfs)
            .env("MBX_CACHE_DIR", self.root.join("cache"))
            .env("MBX_TARGET_ROOT", self.root.join("targets"))
            .env("MBX_TARGET_VIEWS", "1")
            .env("MBX_GC_AUTO", "0")
            .env("MBX_INCREMENTAL", "0")
            .env("MBX_SUMMARY", "off")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR")
            .env_remove("MBX_DISABLE")
            .env_remove("MBX_SOCKET")
            .stdin(Stdio::null());
        command
    }
}

fn write_project(project: &Path) {
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"storage-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        project.join("src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    )
    .unwrap();
}

fn rejected(output: Output, path: &Path, setting: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("is on NFS"), "{stderr}");
    assert!(stderr.contains(&path.display().to_string()), "{stderr}");
    assert!(stderr.contains(setting), "{stderr}");
    assert!(!stderr.contains("Compiling storage-fixture"), "{stderr}");
}

#[test]
fn rejects_cache_before_cargo_or_exec_initializes_it() {
    let fixture = Fixture::new();
    let cache = fixture.nfs.join("new-cache");
    rejected(
        fixture
            .command()
            .env("MBX_CACHE_DIR", &cache)
            .args(["build", "--offline"])
            .output()
            .unwrap(),
        &cache,
        "MBX_CACHE_DIR",
    );
    let marker = fixture.root.join("child-ran");
    rejected(
        fixture
            .command()
            .env("MBX_CACHE_DIR", &cache)
            .args(["exec", "touch"])
            .arg(&marker)
            .output()
            .unwrap(),
        &cache,
        "MBX_CACHE_DIR",
    );
    assert!(!cache.exists());
    assert!(!marker.exists());
    assert!(!fixture.project.join("target").exists());
}

#[test]
fn rejects_explicit_cargo_target_overrides() {
    let fixture = Fixture::new();
    let target = fixture.nfs.join("target");
    for mode in ["flag", "environment", "config"] {
        let mut command = fixture.command();
        command.args(["build", "--offline"]);
        match mode {
            "flag" => {
                command.arg("--target-dir").arg(&target);
            }
            "environment" => {
                command.env("CARGO_TARGET_DIR", &target);
            }
            _ => {
                command
                    .arg("--config")
                    .arg(format!("build.target-dir='{}'", target.display()));
            }
        }
        rejected(command.output().unwrap(), &target, "CARGO_TARGET_DIR");
    }
    assert!(!target.exists());
}

#[test]
fn rejects_managed_destination_and_placement_fallback() {
    let fixture = Fixture::new();
    rejected(
        fixture
            .command()
            .env("MBX_TARGET_ROOT", fixture.nfs.join("views"))
            .args(["build", "--offline"])
            .output()
            .unwrap(),
        &fixture.nfs,
        "MBX_TARGET_ROOT",
    );
    assert!(!fixture.project.join("target").exists());
    assert!(!fixture.nfs.join("views").exists());

    let project = fixture.nfs.join("source");
    write_project(&project);
    // An existing real target makes managed placement decline.
    std::fs::create_dir(project.join("target")).unwrap();
    rejected(
        fixture
            .command()
            .current_dir(&project)
            .args(["build", "--offline"])
            .output()
            .unwrap(),
        &project.join("target"),
        "CARGO_TARGET_DIR",
    );
}

#[test]
fn rejects_separate_intermediates_and_invalidates_cached_probe() {
    let fixture = Fixture::new();
    let intermediate = fixture.nfs.join("intermediate");
    let local = fixture.root.join("intermediate");
    // First populate the probe using a local intermediate directory. Changing
    // the environment must not reuse that answer on the following invocation.
    let first = fixture
        .command()
        .env("CARGO_BUILD_BUILD_DIR", &local)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    rejected(
        fixture
            .command()
            .env("CARGO_BUILD_BUILD_DIR", &intermediate)
            .args(["build", "--offline"])
            .output()
            .unwrap(),
        &intermediate,
        "CARGO_BUILD_BUILD_DIR",
    );
    rejected(
        fixture
            .command()
            .args(["build", "--offline", "--config"])
            .arg(format!("build.build-dir='{}'", intermediate.display()))
            .output()
            .unwrap(),
        &intermediate,
        "CARGO_BUILD_BUILD_DIR",
    );
    assert!(!intermediate.exists());
}

#[test]
fn rejects_separate_intermediates_before_placing_the_target() {
    let fixture = Fixture::new();
    let intermediate = fixture.nfs.join("intermediate");
    rejected(
        fixture
            .command()
            .env("CARGO_BUILD_BUILD_DIR", &intermediate)
            .args(["build", "--offline"])
            .output()
            .unwrap(),
        &intermediate,
        "CARGO_BUILD_BUILD_DIR",
    );
    assert!(std::fs::symlink_metadata(fixture.project.join("target")).is_err());

    // An intermediate directory nested in the target follows it to the
    // managed destination, even from an NFS checkout.
    let project = fixture.nfs.join("source");
    write_project(&project);
    let nested = project.join("target/intermediate");
    let output = fixture
        .command()
        .current_dir(&project)
        .env("CARGO_BUILD_BUILD_DIR", &nested)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        nested
            .canonicalize()
            .unwrap()
            .starts_with(fixture.root.join("targets"))
    );
}

#[test]
fn allows_nfs_sources_with_local_outputs_and_ignores_unused_managed_root() {
    let fixture = Fixture::new();
    let project = fixture.nfs.join("source");
    write_project(&project);
    let output = fixture
        .command()
        .current_dir(&project)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        project
            .join("target")
            .canonicalize()
            .unwrap()
            .starts_with(fixture.root.join("targets"))
    );
    let output = fixture
        .command()
        .env("MBX_TARGET_ROOT", &fixture.nfs)
        .env("CARGO_TARGET_DIR", fixture.root.join("explicit-target"))
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A foreign symlink is not managed by mbx; its local destination wins
    // over an unused NFS managed-root setting.
    std::os::unix::fs::symlink(
        fixture.root.join("explicit-target"),
        fixture.project.join("target"),
    )
    .unwrap();
    let output = fixture
        .command()
        .env("MBX_TARGET_ROOT", &fixture.nfs)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn follows_dangling_cache_links_and_checks_the_persistent_shim() {
    let fixture = Fixture::new();
    let cache = fixture.root.join("linked-cache");
    std::os::unix::fs::symlink(fixture.nfs.join("not-created"), &cache).unwrap();
    rejected(
        fixture
            .command()
            .env("MBX_CACHE_DIR", &cache)
            .args(["build", "--offline"])
            .output()
            .unwrap(),
        &cache,
        "MBX_CACHE_DIR",
    );
    rejected(
        fixture
            .command()
            .env("MBX_CACHE_DIR", &cache)
            .env("MBX_CARGO_SHIM_MODE", "1")
            .args(["build", "--offline"])
            .output()
            .unwrap(),
        &cache,
        "MBX_CACHE_DIR",
    );
    assert!(!fixture.nfs.join("not-created").exists());
}

#[test]
fn help_inspection_cleanup_and_disabled_shim_remain_available() {
    let fixture = Fixture::new();
    let cache = fixture.nfs.join("cache");
    for arguments in [
        vec!["build", "--help"],
        vec!["cache", "dir"],
        vec!["gc", "--dry-run"],
        vec!["clean"],
    ] {
        let output = fixture
            .command()
            .env("MBX_CACHE_DIR", &cache)
            .args(&arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = fixture
        .command()
        .env("MBX_CACHE_DIR", &cache)
        .env("MBX_CARGO_SHIM_MODE", "1")
        .env("MBX_DISABLE", "1")
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
