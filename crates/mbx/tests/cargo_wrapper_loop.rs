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

fn track_loop_note(wrapper: &Path) {
    let script = std::fs::read_to_string(wrapper).unwrap();
    std::fs::write(
        wrapper,
        script.replacen(
            "#!/bin/sh\n",
            "#!/bin/sh\numask 022\nprintf '%s' \"$MBX_CARGO_SHIM_NOTE\" > \"$MBX_TEST_NOTE_PATH\"\nif [ -e \"$MBX_CARGO_SHIM_NOTE\" ]; then ls -l \"$MBX_CARGO_SHIM_NOTE\" > \"$MBX_TEST_NOTE_MODE\"; fi\n",
            1,
        ),
    )
    .unwrap();
}

fn assert_loop_note_removed(root: &Path) {
    let note = read(&root.join("note-path"));
    assert!(!note.is_empty());
    assert!(
        !Path::new(&note).exists(),
        "loop note was not removed: {note}"
    );
    assert!(
        !Path::new(&format!("{note}.retries")).exists(),
        "retry counter was not removed"
    );
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
        .env("MBX_TEST_NOTE_PATH", root.join("note-path"))
        .env("MBX_TEST_NOTE_MODE", root.join("note-mode"))
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
        // macOS filesystems reject invalid UTF-8 names before mbx can run.
        // The Cargo-chain unit test still covers lossless serialization there.
        #[cfg(not(target_os = "macos"))]
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
                "#!/bin/sh\ncount=$(cat '{}' 2>/dev/null || echo 0)\ncount=$((count + 1))\necho $count > '{}'\n[ $count -le 512 ] || {{ touch '{}'; echo 'fixture stopped unguarded recursion' >&2; exit 99; }}\n{directory_change}exec '{mbx_path}' {prefix}\"$@\"\n",
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
        track_loop_note(&wrapper);
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
        // The failed-tree budget bounds retries across all sibling probes.
        assert!(calls <= 256, "wrapper launched {calls} times: {output}");
        assert!(output.contains("re-entered its Cargo shim"), "{output}");
        assert!(
            !output.contains("fixture stopped unguarded recursion"),
            "{output}"
        );
        assert_loop_note_removed(root.path());
        if !prefix.is_empty() {
            assert!(read(&root.path().join("note-mode")).starts_with("-rw-------"));
        }
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
    track_loop_note(&wrapper);
    let mut command = mbx(root.path(), &wrapper);
    command.env_remove("MBX_CARGO_SHIM_MODE");
    command.args(["metadata", "--no-deps", "--format-version", "1"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "{output}");
    assert!(output.contains("workspace_root"), "{output}");
    assert_loop_note_removed(root.path());

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

#[test]
fn direct_cargo_fallback_can_run_external_commands_without_a_manifest() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let real = std::env::var_os("CARGO").unwrap();
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\nfor arg do if [ \"$arg\" = probe ]; then exec '{}' \"$@\"; fi; done\ncase \" $* \" in *\" not-the-workspace.toml \"*) exec '{mbx_path}' \"$@\";; esac\nfor arg do if [ \"$arg\" = helper ]; then CARGO=\"$0\" exec '{mbx_path}' metadata --no-deps --format-version 1; fi; done\nif [ -n \"$MBX_TEST_RECURSIVE_BRANCH\" ]; then exec '{mbx_path}' \"$@\"; fi\nMBX_TEST_RECURSIVE_BRANCH=1 '{mbx_path}' \"$@\" && exit 0\nexec '{}' \"$@\"\n",
            Path::new(&real).display(),
            Path::new(&real).display()
        ),
    );
    let external = wrapper.parent().unwrap().join("cargo-probe");
    std::fs::write(&external, "#!/bin/sh\necho recovered-direct\n").unwrap();
    std::fs::set_permissions(&external, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .current_dir(outside.path())
        .env(
            "PATH",
            std::env::join_paths(
                std::iter::once(wrapper.parent().unwrap().to_path_buf())
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap(),
        )
        .arg("probe");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "{output}");
    assert!(output.contains("recovered-direct"), "{output}");

    let path = std::env::join_paths(
        std::iter::once(wrapper.parent().unwrap().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("PATH", &path)
        .args(["probe", "--manifest-path", "not-the-workspace.toml"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(
        success,
        "external arguments prevented metadata recovery: {output}"
    );
    assert!(output.contains("recovered-direct"), "{output}");

    std::fs::write(&external, format!(
        "#!/bin/sh\nmkdir -p generated/src\nprintf '%s\\n' '[package]' \"name='generated'\" \"version='0.0.0'\" > generated/Cargo.toml\nprintf '%s\\n' 'fn main() {{}}' > generated/src/main.rs\ncd generated || exit 98\nCARGO='{}' exec '{mbx_path}' build\n", wrapper.display()
    )).unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("PATH", &path)
        .current_dir(outside.path())
        .arg("probe");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(30));
    assert!(
        success,
        "passthrough retained the failed probe's note: {output}"
    );
    assert!(outside.path().join("generated/Cargo.toml").exists());

    std::fs::create_dir(outside.path().join(".cargo")).unwrap();
    let manifest = root.path().join("Cargo.toml");
    std::fs::write(
        outside.path().join(".cargo/config.toml"),
        format!(
            "[alias]\nbapp = ['build', '--manifest-path', '{}']\nnt = ['probe', 'run', '--manifest-path', '{}']\n",
            manifest.display(), manifest.display()
        ),
    )
    .unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .current_dir(outside.path())
        .arg("bapp");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(30));
    assert!(success, "alias did not recover its manifest: {output}");

    std::fs::write(
        &external,
        "#!/bin/sh\n[ -n \"$MBX_SOCKET\" ] || exit 96\necho managed-alias\n",
    )
    .unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("PATH", &path)
        .current_dir(outside.path())
        .arg("nt");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(30));
    assert!(
        success,
        "external alias bypassed its manifest's cache session: {output}"
    );
    assert!(output.contains("managed-alias"), "{output}");

    // Successful metadata must not carry an abandoned failure into the
    // actual command, which starts another invocation through the wrapper.
    let mut command = mbx(root.path(), &wrapper);
    command.env_remove("MBX_CARGO_SHIM_MODE").arg("helper");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(
        success,
        "successful metadata left a stale loop note: {output}"
    );
    assert!(output.contains("workspace_root"), "{output}");
}

#[test]
fn tracked_note_paths_can_contain_apostrophes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("O'Neil");
    let wrapper = fixture(&root, "#!/bin/sh\nexec \"$MBX_TEST_BIN\" cargo \"$@\"\n");
    track_loop_note(&wrapper);
    let mut command = mbx(&root, &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("MBX_TEST_BIN", env!("CARGO_BIN_EXE_mbx"))
        .args(["cargo", "fmt", "--all", "--", "--check"]);
    let (success, output) = output_within(command, &root, Duration::from_secs(30));
    assert!(!success, "{output}");
    assert!(output.contains("re-entered its Cargo shim"), "{output}");
    assert_loop_note_removed(&root);
    assert!(read(&root.join("note-mode")).starts_with("-rw-------"));
}

#[test]
fn explain_last_ignores_an_exhausted_cargo_retry_budget() {
    let root = tempfile::tempdir().unwrap();
    let wrapper = fixture(root.path(), "#!/bin/sh\necho cargo-must-not-run\nexit 99\n");
    let note = root.path().join("guard");
    std::fs::write(
        root.path().join("guard.retries"),
        ((1_u64 << 63) | 128).to_le_bytes(),
    )
    .unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("MBX_CARGO_SHIM_DEPTH", "12")
        .env("MBX_CARGO_SHIM_NOTE", &note)
        .args(["explain", "--last"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(!success, "an empty cache has no build to explain: {output}");
    assert!(output.contains("no recorded build was found"), "{output}");
    assert!(!output.contains("re-entered its Cargo shim"), "{output}");
    assert!(!output.contains("cargo-must-not-run"), "{output}");
}

#[test]
fn sibling_probes_keep_their_own_errors_and_cannot_clear_a_loop_diagnosis() {
    let root = tempfile::tempdir().unwrap();
    let bad = root.path().join("bad");
    std::fs::create_dir(&bad).unwrap();
    std::fs::write(bad.join("Cargo.toml"), "[package\n").unwrap();
    let wrapper = fixture(
        root.path(),
        r##"#!/bin/sh
if [ -n "$MBX_TEST_RECURSIVE_BRANCH" ]; then exec "$MBX_TEST_BIN" "$@"; fi
MBX_TEST_RECURSIVE_BRANCH=1 "$MBX_TEST_BIN" "$@" >/dev/null 2>&1
CARGO="$MBX_TEST_REAL_CARGO" "$MBX_TEST_BIN" check --manifest-path "$MBX_TEST_BAD_MANIFEST" > "$MBX_TEST_BAD_OUTPUT" 2>&1
CARGO="$MBX_TEST_REAL_CARGO" "$MBX_TEST_BIN" metadata --no-deps --format-version 1 --manifest-path "$MBX_TEST_GOOD_MANIFEST" >/dev/null 2>&1 || exit 96
exit 2
"##,
    );
    let bad_output = root.path().join("manifest-error");
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("MBX_TEST_BIN", env!("CARGO_BIN_EXE_mbx"))
        .env("MBX_TEST_REAL_CARGO", std::env::var_os("CARGO").unwrap())
        .env("MBX_TEST_BAD_MANIFEST", bad.join("Cargo.toml"))
        .env("MBX_TEST_GOOD_MANIFEST", root.path().join("Cargo.toml"))
        .env("MBX_TEST_BAD_OUTPUT", &bad_output)
        .arg("build");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(30));
    assert!(!success, "{output}");
    assert!(output.contains("re-entered its Cargo shim"), "{output}");
    let manifest_error = read(&bad_output);
    assert!(manifest_error.contains("Cargo.toml"), "{manifest_error}");
    assert!(manifest_error.contains("expected"), "{manifest_error}");
    assert!(
        !manifest_error.contains("re-entered its Cargo shim"),
        "{manifest_error}"
    );
}

#[test]
fn alias_recovery_reports_both_a_loop_and_its_own_manifest_error() {
    let root = tempfile::tempdir().unwrap();
    let bad = root.path().join("bad");
    std::fs::create_dir_all(root.path().join(".cargo")).unwrap();
    std::fs::create_dir(&bad).unwrap();
    let bad_manifest = bad.join("Cargo.toml");
    std::fs::write(&bad_manifest, "[package\n").unwrap();
    std::fs::write(
        root.path().join(".cargo/config.toml"),
        format!(
            "[alias]\nbad = [\"build\", \"--manifest-path\", {}]\n",
            serde_json::to_string(&bad_manifest).unwrap()
        ),
    )
    .unwrap();
    let wrapper = fixture(
        root.path(),
        r##"#!/bin/sh
for arg do
    if [ "$arg" = "$MBX_TEST_BAD_MANIFEST" ]; then exec "$MBX_TEST_REAL_CARGO" "$@"; fi
done
exec "$MBX_TEST_BIN" "$@"
"##,
    );
    for shim in [false, true] {
        let mut command = mbx(root.path(), &wrapper);
        if !shim {
            command.env_remove("MBX_CARGO_SHIM_MODE");
        }
        command
            .env("MBX_TEST_BIN", env!("CARGO_BIN_EXE_mbx"))
            .env("MBX_TEST_REAL_CARGO", std::env::var_os("CARGO").unwrap())
            .env("MBX_TEST_BAD_MANIFEST", &bad_manifest)
            .arg("bad");
        let (success, output) = output_within(command, root.path(), Duration::from_secs(30));
        assert!(!success, "{output}");
        assert!(output.contains("re-entered its Cargo shim"), "{output}");
        assert!(
            output.contains("Expanded alias metadata also failed"),
            "{output}"
        );
        assert!(output.contains("Cargo.toml"), "{output}");
        assert!(output.contains("expected"), "{output}");
    }
}

#[test]
fn translated_wrapper_failure_keeps_the_loop_diagnosis() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let wrapper = fixture(
        root.path(),
        &format!("#!/bin/sh\nMBX_CARGO_SHIM_MODE=1 '{mbx_path}' \"$@\" >/dev/null 2>&1\nexit 2\n"),
    );
    let mut command = mbx(root.path(), &wrapper);
    command.arg("build");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(!success, "{output}");
    assert!(output.contains("re-entered its Cargo shim"), "{output}");
}

#[test]
fn a_failed_branch_does_not_reject_a_different_sibling_command() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let real = std::env::var_os("CARGO").unwrap();
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\ncase \" $* \" in *\" locate-project \"*) exec '{}' \"$@\";; esac\nif [ -n \"$MBX_TEST_RECURSIVE_BRANCH\" ]; then exec '{mbx_path}' \"$@\"; fi\nMBX_TEST_RECURSIVE_BRANCH=1 '{mbx_path}' \"$@\" >/dev/null 2>&1\nCARGO=\"$0\" '{mbx_path}' locate-project >/dev/null || exit 97\nexec '{}' \"$@\"\n",
            Path::new(&real).display(),
            Path::new(&real).display()
        ),
    );
    let mut command = mbx(root.path(), &wrapper);
    command.env_remove("MBX_CARGO_SHIM_MODE").args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "{output}");
    assert!(output.contains("workspace_root"), "{output}");
}

#[test]
fn wrapper_can_retry_the_same_command_in_a_different_checkout() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let other_root = other.path().canonicalize().unwrap();
    fixture(other.path(), "#!/bin/sh\nexit 0\n");
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let real = std::env::var_os("CARGO").unwrap();
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\nif [ \"$PWD\" = '{}' ]; then exec '{}' \"$@\"; fi\nif [ -n \"$MBX_TEST_RECURSIVE_BRANCH\" ]; then exec '{mbx_path}' \"$@\"; fi\nMBX_TEST_RECURSIVE_BRANCH=1 '{mbx_path}' \"$@\" >/dev/null 2>&1\ncd '{}' || exit 98\nCARGO=\"$0\" exec '{mbx_path}' \"$@\"\n",
            other_root.display(),
            Path::new(&real).display(),
            other_root.display()
        ),
    );
    let mut command = mbx(root.path(), &wrapper);
    command.env_remove("MBX_CARGO_SHIM_MODE").args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "{output}");
    assert!(
        output.contains(&other_root.display().to_string()),
        "{output}"
    );
}

#[test]
fn wrapper_can_retry_the_same_command_with_a_different_environment() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let real = std::env::var_os("CARGO").unwrap();
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\nif [ -n \"$MBX_TEST_ROUTE_REAL\" ]; then exec '{}' \"$@\"; fi\nif [ -n \"$MBX_TEST_RECURSIVE_BRANCH\" ]; then exec '{mbx_path}' \"$@\"; fi\nMBX_TEST_RECURSIVE_BRANCH=1 '{mbx_path}' \"$@\" >/dev/null 2>&1\nMBX_TEST_ROUTE_REAL=1 exec '{mbx_path}' \"$@\"\n",
            Path::new(&real).display()
        ),
    );
    let mut command = mbx(root.path(), &wrapper);
    command.env_remove("MBX_CARGO_SHIM_MODE").args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "{output}");
    assert!(output.contains("workspace_root"), "{output}");
}

#[test]
fn recursive_wrapper_outside_a_workspace_stays_bounded() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let entries = root.path().join("entries");
    let cutoff = root.path().join("cutoff");
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\ncount=$(cat '{}' 2>/dev/null || echo 0)\ncount=$((count + 1))\necho $count > '{}'\n[ $count -le 512 ] || {{ touch '{}'; exit 99; }}\nexec '{mbx_path}' \"$@\"\n",
            entries.display(),
            entries.display(),
            cutoff.display()
        ),
    );
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .current_dir(outside.path())
        .arg("build");
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(!success, "{output}");
    assert!(
        !cutoff.exists(),
        "outside wrapper restarted the loop: {output}"
    );
    assert!(
        read(&entries).trim().parse::<usize>().unwrap() <= 256,
        "{output}"
    );
    assert!(output.contains("re-entered its Cargo shim"), "{output}");
}

#[test]
fn real_cargo_fallback_blocked_at_the_limit_can_run_as_the_stack_unwinds() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let real = std::env::var_os("CARGO").unwrap();
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\n'{mbx_path}' \"$@\" 2>/dev/null && exit 0\nCARGO='{}' exec '{mbx_path}' \"$@\"\n",
            Path::new(&real).display()
        ),
    );
    let mut command = mbx(root.path(), &wrapper);
    command.env_remove("MBX_CARGO_SHIM_MODE").args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "{output}");
    assert!(output.contains("workspace_root"), "{output}");
}

#[test]
fn changing_path_cannot_restart_sibling_probe_trees() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let entries = root.path().join("entries");
    let cutoff = root.path().join("cutoff");
    let observed = root.path().join("retries-observed");
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\ncount=$(cat '{}' 2>/dev/null || echo 0)\ncount=$((count + 1))\necho $count > '{}'\n[ $count -le 512 ] || {{ touch '{}'; exit 99; }}\nif [ -f \"$MBX_CARGO_SHIM_NOTE.retries\" ]; then cp \"$MBX_CARGO_SHIM_NOTE.retries\" '{}'; fi\nPATH='{}':\"$PATH\" exec '{mbx_path}' nextest run\n",
            entries.display(),
            entries.display(),
            cutoff.display(),
            observed.display(),
            root.path().join("bin").display()
        ),
    );
    track_loop_note(&wrapper);
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .args(["nextest", "run"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(30));
    assert!(!success, "{output}");
    assert!(
        !cutoff.exists(),
        "probe tree reached the fixture cutoff: {output}"
    );
    assert!(
        read(&entries).trim().parse::<usize>().unwrap() <= 256,
        "{output}"
    );
    let bytes: [u8; 8] = std::fs::read(&observed).unwrap().try_into().unwrap();
    assert_eq!(u64::from_le_bytes(bytes) & !(1 << 63), 128);
    assert_loop_note_removed(root.path());
}

#[test]
fn unusable_temporary_storage_stops_before_recursive_children() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let entries = root.path().join("entries");
    let cutoff = root.path().join("cutoff");
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\ncount=$(cat '{}' 2>/dev/null || echo 0)\ncount=$((count + 1))\necho $count > '{}'\n[ $count -le 32 ] || {{ touch '{}'; exit 99; }}\nexec '{mbx_path}' nextest run\n",
            entries.display(),
            entries.display(),
            cutoff.display()
        ),
    );
    let unavailable = root.path().join("not-a-directory");
    std::fs::write(&unavailable, "cannot contain guard files").unwrap();
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("TMPDIR", &unavailable)
        .args(["nextest", "run"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(!success, "{output}");
    assert!(
        !cutoff.exists(),
        "unavailable storage restarted the probe tree: {output}"
    );
    assert!(
        read(&entries).trim().parse::<usize>().unwrap() <= 8,
        "{output}"
    );
    assert!(
        output.contains("could not maintain Cargo recursion guard state"),
        "{output}"
    );

    // An explicit query that reaches real Cargo directly needs no nested
    // coordination state, even when TMPDIR cannot hold guard files.
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("TMPDIR", &unavailable)
        .env("CARGO", std::env::var_os("CARGO").unwrap())
        .args(["metadata", "--no-deps", "--format-version", "1"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(15));
    assert!(success, "direct Cargo query needed guard storage: {output}");
}

#[test]
fn successful_independent_probes_cannot_reset_a_failed_trees_budget() {
    let root = tempfile::tempdir().unwrap();
    let mbx_path = env!("CARGO_BIN_EXE_mbx");
    let real = std::env::var_os("CARGO").unwrap();
    let entries = root.path().join("entries");
    let cutoff = root.path().join("cutoff");
    let observed = root.path().join("retries-observed");
    let independent = root.path().join("independent-cargo");
    let succeeded = root.path().join("metadata-succeeded");
    std::fs::write(
        &independent,
        r##"#!/bin/sh
"$MBX_TEST_REAL_CARGO" "$@"
status=$?
case " $* " in *" metadata "*)
    if [ "$status" -eq 0 ]; then : > "$MBX_TEST_PROBE_SUCCEEDED"; fi;;
esac
exit "$status"
"##,
    )
    .unwrap();
    std::fs::set_permissions(&independent, std::fs::Permissions::from_mode(0o755)).unwrap();
    let wrapper = fixture(
        root.path(),
        &format!(
            "#!/bin/sh\ncount=$(cat '{}' 2>/dev/null || echo 0)\ncount=$((count + 1))\necho $count > '{}'\n[ $count -le 512 ] || {{ touch '{}'; exit 99; }}\nif [ -f \"$MBX_CARGO_SHIM_NOTE.retries\" ]; then cp \"$MBX_CARGO_SHIM_NOTE.retries\" '{}'; fi\nCARGO=\"$MBX_TEST_INDEPENDENT_CARGO\" '{mbx_path}' check --mbx-test-invalid-flag >/dev/null 2>&1 || :\nPATH='{}':\"$PATH\" exec '{mbx_path}' nextest run\n",
            entries.display(),
            entries.display(),
            cutoff.display(),
            observed.display(),
            root.path().join("bin").display()
        ),
    );
    track_loop_note(&wrapper);
    let mut command = mbx(root.path(), &wrapper);
    command
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env("MBX_TEST_REAL_CARGO", real)
        .env("MBX_TEST_INDEPENDENT_CARGO", independent)
        .env("MBX_TEST_PROBE_SUCCEEDED", &succeeded)
        .env("MBX_GC_AUTO", "0")
        .env("MBX_TARGET_ROOT", root.path().join("targets"))
        .args(["nextest", "run"]);
    let (success, output) = output_within(command, root.path(), Duration::from_secs(30));
    assert!(!success, "{output}");
    assert!(
        !cutoff.exists(),
        "a successful probe reset the retry budget: {output}"
    );
    assert!(
        read(&entries).trim().parse::<usize>().unwrap() <= 256,
        "{output}"
    );
    let bytes: [u8; 8] = std::fs::read(&observed).unwrap().try_into().unwrap();
    assert_eq!(u64::from_le_bytes(bytes) & !(1 << 63), 128);
    assert!(
        succeeded.exists(),
        "no independent metadata probe succeeded: {output}"
    );
    assert_loop_note_removed(root.path());
}
