#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run `command` in its own process group and kill the group at the deadline,
/// so a broken guard fails the test instead of leaving a process chain behind.
fn output_within(mut command: Command, root: &Path, limit: Duration) -> (bool, String) {
    let log = root.join("output.log");
    let file = std::fs::File::create(&log).unwrap();
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .spawn()
        .unwrap();
    let start = Instant::now();
    let success = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.success();
        }
        if start.elapsed() > limit {
            // SAFETY: signals the group this test created.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            child.wait().unwrap();
            panic!("the wrapper loop was not stopped:\n{}", read(&log));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    (success, read(&log))
}

fn read(path: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned()
}

fn fixture(root: &Path, wrapper_body: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='loop-fixture'\nversion='0.0.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let wrapper = bin.join("cargo");
    std::fs::write(&wrapper, wrapper_body).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    wrapper
}

fn mbx(root: &Path, wrapper: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        // rustup's proxy finds its toolchains through these.
        .env("HOME", std::env::var_os("HOME").unwrap())
        .envs(
            ["RUSTUP_HOME", "CARGO_HOME", "RUSTUP_TOOLCHAIN"]
                .into_iter()
                .filter_map(|name| Some((name, std::env::var_os(name)?))),
        )
        .env("MBX_CACHE_DIR", root.join("cache"))
        .env("MBX_CARGO_SHIM_MODE", "1")
        .env("CARGO", wrapper);
    command
}

/// A Cargo wrapper that execs mbx, selected as the real Cargo, must produce a
/// diagnosis instead of a process chain that never ends.
#[test]
fn wrapper_that_runs_mbx_again_reports_the_loop() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let wrapper = fixture(
        root.path(),
        &format!("#!/bin/sh\nMBX_CARGO_SHIM_MODE=1 exec '{mbx_path}' \"$@\"\n"),
    );
    let mut command = mbx(root.path(), &wrapper);
    command.args(["build", "--offline"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(60));
    assert!(!success, "{output}");
    assert!(output.contains("re-entered its Cargo shim"), "{output}");
    assert!(output.contains("CARGO environment variable"), "{output}");
}

/// The roots probe succeeds when the wrapper forwards `metadata` to the real
/// Cargo, and a `--config` run then takes the plain launch path, which rebuilds
/// the environment. The guard's counters must survive it.
#[test]
fn wrapper_loop_through_a_plain_launch_still_stops() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    // Cargo sets CARGO for the tests it runs; otherwise search PATH.
    let real = std::env::var_os("CARGO")
        .map(std::path::PathBuf::from)
        .filter(|cargo| cargo.is_file())
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|dir| dir.join("cargo"))
                // Developer machines put mbx's own shim and mise's wrappers
                // ahead of the toolchain; neither is a real Cargo.
                .find(|candidate| {
                    candidate.is_file()
                        && !candidate
                            .components()
                            .any(|part| matches!(part.as_os_str().to_str(), Some("mbx" | "mise")))
                })
        })
        .unwrap();
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\ncase \" $* \" in *\" metadata \"*) exec '{}' \"$@\";; esac\nMBX_CARGO_SHIM_MODE=1 exec '{mbx_path}' \"$@\"\n",
            real.display()
        ),
    );
    let mut command = mbx(root.path(), &wrapper);
    command.args(["run", "--offline", "--config", "build.jobs=1"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(60));
    assert!(!success, "{output}");
    assert!(output.contains("re-entered its Cargo shim"), "{output}");
}

/// Legacy launchers can invoke mbx explicitly rather than enter Cargo shim
/// mode. Bound the fixture itself so a regression cannot exhaust the host.
#[test]
fn explicit_wrapper_loops_report_the_loop() {
    for (configured, prefix, change_dir, non_utf8) in [
        (false, "cargo ", false, false),
        (true, "cargo ", false, false),
        (false, "", false, false),
        (true, "", false, false),
        (false, "cargo ", true, false),
        (true, "cargo ", false, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mbx_path = env!("CARGO_BIN_EXE_mbx");
        let entries = root.path().join("entries");
        let cutoff = root.path().join("cutoff");
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let directory_change = if change_dir {
            format!("cd '{}' || exit 98\n", nested.display())
        } else {
            String::new()
        };
        let mut wrapper = fixture(
            root.path(),
            &format!(
                "#!/bin/sh\ncount=$(cat '{}' 2>/dev/null || echo 0)\ncount=$((count + 1))\necho $count > '{}'\n[ $count -le 20 ] || {{ touch '{}'; echo 'fixture stopped unguarded recursion' >&2; exit 99; }}\n{directory_change}exec '{mbx_path}' {prefix}\"$@\"\n",
                entries.display(),
                entries.display(),
                cutoff.display(),
            ),
        );
        if non_utf8 {
            use std::os::unix::ffi::OsStringExt;
            let directory = root
                .path()
                .join(std::ffi::OsString::from_vec(b"bin-\xff".to_vec()));
            std::fs::rename(wrapper.parent().unwrap(), &directory).unwrap();
            wrapper = directory.join("cargo");
        }
        let mut command = mbx(root.path(), &wrapper);
        command.env_remove("MBX_CARGO_SHIM_MODE");
        if !configured {
            command.env_remove("CARGO");
        }
        if change_dir {
            command.env("MBX_CACHE_DIR", "cache");
        }
        command.env(
            "PATH",
            std::env::join_paths(
                std::iter::once(wrapper.parent().unwrap().to_path_buf())
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap(),
        );
        if !prefix.is_empty() {
            command.arg("cargo");
        }
        command.args(["fmt", "--all", "--", "--check"]);
        let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
        assert!(!success, "{output}");
        assert!(!cutoff.exists(), "fixture cutoff reached: {output}");
        let calls: usize = read(&entries).trim().parse().unwrap();
        assert!(calls <= 12, "wrapper launched {calls} times: {output}");
        assert!(output.contains("re-entered its Cargo shim"), "{output}");
        assert!(
            !output.contains("fixture stopped unguarded recursion"),
            "{output}"
        );
        assert!(
            std::fs::read_dir(root.path().join("cache"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("cargo-shim-loop-"))
        );
        if change_dir {
            assert!(
                !nested.join("cache").exists(),
                "relative note followed wrapper cwd"
            );
        }
    }
}

#[test]
fn wrapper_can_recover_a_recursive_branch_with_real_cargo() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let real = std::env::var_os("CARGO").unwrap();
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\ncase \" $* \" in *\" --list \"*) exec '{}' \"$@\";; esac\nif [ -n \"$MBX_TEST_RECURSIVE_BRANCH\" ]; then exec '{mbx_path}' \"$@\"; fi\nMBX_TEST_RECURSIVE_BRANCH=1 '{mbx_path}' \"$@\" && exit 0\nCARGO='{}' exec '{mbx_path}' \"$@\"\n",
            Path::new(&real).display(),
            Path::new(&real).display()
        ),
    );
    let mut command = mbx(root.path(), &wrapper);
    command.env_remove("MBX_CARGO_SHIM_MODE");
    command.args(["metadata", "--no-deps", "--format-version", "1"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "{output}");
    assert!(output.contains("workspace_root"), "{output}");
    assert!(
        std::fs::read_dir(root.path().join("cache"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("cargo-shim-loop-"))
    );

    // Failed metadata outside a project must still allow Cargo to run an
    // external command after the wrapper recovers with real Cargo.
    let outside = tempfile::tempdir().unwrap();
    let external = wrapper.parent().unwrap().join("cargo-probe");
    std::fs::write(&external, "#!/bin/sh\necho recovered-external\n").unwrap();
    std::fs::set_permissions(&external, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command.env_remove("MBX_CARGO_SHIM_MODE");
    command.current_dir(outside.path()).env(
        "PATH",
        std::env::join_paths(
            std::iter::once(wrapper.parent().unwrap().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap(),
    );
    command.arg("probe");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "{output}");
    assert!(output.contains("recovered-external"), "{output}");
}

#[test]
fn alternating_fallbacks_cannot_reset_the_depth_limit() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let alternate = root.path().join("alternate");
    let entries = root.path().join("entries");
    let cutoff = root.path().join("cutoff");
    let script = |fallback: &Path| {
        format!(
            "#!/bin/sh\ncount=$(cat '{}' 2>/dev/null || echo 0)\ncount=$((count + 1))\necho $count > '{}'\n[ $count -le 20 ] || {{ touch '{}'; exit 99; }}\n'{mbx_path}' \"$@\" && exit 0\nCARGO='{}' exec '{mbx_path}' \"$@\"\n",
            entries.display(),
            entries.display(),
            cutoff.display(),
            fallback.display()
        )
    };
    let wrapper = fixture(root.path(), &script(&alternate));
    std::fs::write(&alternate, script(&wrapper)).unwrap();
    std::fs::set_permissions(&alternate, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("MBX_CARGO_SHIM_DEPTH", "11");
    command.args(["metadata", "--no-deps", "--format-version", "1"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(!success, "{output}");
    assert!(output.contains("re-entered its Cargo shim"), "{output}");
    assert!(
        !cutoff.exists(),
        "fallback reset the recursion budget: {output}"
    );
    assert_eq!(read(&entries).trim(), "1", "{output}");
}
