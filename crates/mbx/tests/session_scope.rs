use std::path::Path;
use std::process::Command;

fn project(path: &Path, source: &str) {
    std::fs::create_dir_all(path.join("src")).unwrap();
    std::fs::write(
        path.join("Cargo.toml"),
        "[package]\nname='scope-fixture'\nversion='0.0.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(path.join("src/main.rs"), source).unwrap();
}

fn mbx(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
    command.env_clear();
    // rustc's MSVC discovery needs the Visual Studio/SDK environment as well
    // as PATH. Without it Git's unrelated link.exe can win resolution.
    #[cfg(windows)]
    for (name, value) in std::env::vars_os() {
        let key = name.to_string_lossy().to_ascii_uppercase();
        if !key.starts_with("MBX_")
            && !matches!(
                key.as_str(),
                "RUSTC_WRAPPER"
                    | "RUSTC_WORKSPACE_WRAPPER"
                    | "RUSTDOC"
                    | "CARGO_TARGET_DIR"
                    | "CARGO_INCREMENTAL"
                    | "CARGO"
            )
        {
            command.env(name, value);
        }
    }
    for name in [
        "PATH",
        "HOME",
        "RUSTUP_HOME",
        "CARGO_HOME",
        "RUSTUP_TOOLCHAIN",
        "SystemRoot",
        "TEMP",
        "TMP",
        "TMPDIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            let key = if cfg!(windows) && name == "PATH" {
                "Path"
            } else {
                name
            };
            command.env(key, value);
        }
    }
    command
        .current_dir(root)
        .env("MBX_CACHE_DIR", root.join("cache"))
        .env("MBX_LINKER", "system")
        .env("CI", "1")
        .env("MBX_SUMMARY", "off");
    command
}

#[test]
fn cargo_run_restores_settings_before_starting_application() {
    let root = tempfile::tempdir().unwrap();
    project(
        root.path(),
        r#"
fn main() {
    for key in ["MBX_SOCKET", "MBX_BUILD", "MBX_SESSION_RESTORE", "MBX_SESSION_LEASE", "MBX_LAUNCH_CAPTURE", "MBX_WORKSPACE_ROOT", "MBX_TARGET_DIR", "RUSTDOC", "RUSTC_WRAPPER", "CARGO_TARGET_DIR"] {
        assert!(std::env::var_os(key).is_none(), "{key} leaked");
    }
    assert_eq!(std::env::var("CARGO_INCREMENTAL").unwrap(), "1");
    assert_eq!(std::env::var("MBX_VERIFY").unwrap(), "0");
    assert_eq!(std::env::var_os("PATH"), std::env::var_os("EXPECTED_PATH"));
    assert_eq!(std::env::args().nth(1).unwrap(), "argument with spaces");
    println!("application started");
    std::process::exit(23);
}
"#,
    );
    let output = mbx(root.path())
        .env("CARGO_INCREMENTAL", "1")
        .env("MBX_VERIFY", "0")
        .env("EXPECTED_PATH", std::env::var_os("PATH").unwrap())
        .args(["run", "--offline", "--", "argument with spaces"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("application started"));
}

#[cfg(unix)]
#[test]
fn cargo_run_preserves_configured_runner_and_wrapper() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    project(
        root.path(),
        r#"
fn main() {
    assert!(std::env::var_os("MBX_SOCKET").is_none());
    assert!(std::env::var("RUSTC_WRAPPER").unwrap().ends_with("caller-wrapper"));
    assert_eq!(std::env::var("RUNNER_ARGUMENT").unwrap(), "runner arg");
}
"#,
    );
    let wrapper = root.path().join("caller-wrapper");
    std::fs::write(&wrapper, "#!/bin/sh\nexec \"$@\"\n").unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let runner = root.path().join("caller-runner");
    std::fs::write(
        &runner,
        "#!/bin/sh\nexport RUNNER_ARGUMENT=\"$1\"\nshift\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir(root.path().join(".cargo")).unwrap();
    std::fs::write(
        root.path().join(".cargo/config.toml"),
        "[target.'cfg(unix)']\nrunner = ['./caller-runner', 'runner arg']\n",
    )
    .unwrap();
    let output = mbx(root.path())
        .env("RUSTC_WRAPPER", wrapper)
        .args(["run", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn application_daemon_outlives_session_and_captures_an_independent_build() {
    let root = tempfile::tempdir().unwrap();
    let other = root.path().join("other");
    project(&other, "fn main() {}\n");
    project(
        root.path(),
        r#"
use std::process::{Command, Stdio};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("daemon") {
        assert!(std::os::unix::net::UnixStream::connect(env!("OUTER_SOCKET")).is_err(), "session still alive at application launch");
        Command::new(std::env::current_exe().unwrap()).arg("daemon").args(&args[1..])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        return;
    }
    for _ in 0..200 {
        if std::path::Path::new(&args[4]).exists() { break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(std::path::Path::new(&args[4]).exists());
    assert!(std::env::var_os("MBX_SOCKET").is_none());
    assert!(std::env::var_os("RUSTC_WRAPPER").is_none());
    let output = Command::new(&args[2]).args(["check", "--offline"])
        .current_dir(&args[3]).env("MBX_LOG", "debug").output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(!output.stderr.is_empty());
    std::fs::write(&args[5], "independent build captured").unwrap();
}
"#,
    );
    std::fs::write(root.path().join("build.rs"),
        "fn main() { println!(\"cargo:rustc-env=OUTER_SOCKET={}\", std::env::var(\"MBX_SOCKET\").unwrap()); }\n").unwrap();
    let gate = root.path().join("launcher-exited");
    let result = root.path().join("daemon-result");
    let output = mbx(root.path())
        .args(["run", "--offline", "--", env!("CARGO_BIN_EXE_mbx")])
        .arg(&other)
        .arg(&gate)
        .arg(&result)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(&gate, "go").unwrap();
    for _ in 0..200 {
        if result.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        std::fs::read_to_string(result).unwrap(),
        "independent build captured"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Checking scope-fixture"));
}

#[test]
fn stale_session_recovers_original_environment_before_building() {
    use std::ffi::OsString;
    let root = tempfile::tempdir().unwrap();
    project(root.path(), "fn main() {}\n");
    std::fs::write(
        root.path().join("build.rs"),
        r#"
fn main() {
    assert!(!std::env::var("MBX_SOCKET").unwrap().contains("missing-session"));
    assert!(std::env::var("MBX_WORKSPACE_ROOT").unwrap().contains(".tmp"));
}
"#,
    )
    .unwrap();
    let restore: Vec<(OsString, Option<OsString>)> = [
        "MBX_SOCKET",
        "MBX_SESSION_LEASE",
        "MBX_SESSION_RESTORE",
        "RUSTC_WRAPPER",
        "MBX_WORKSPACE_ROOT",
    ]
    .into_iter()
    .map(|key| (key.into(), None))
    .collect();
    let output = mbx(root.path())
        .env("MBX_SOCKET", "missing-session")
        .env("MBX_SESSION_LEASE", root.path().join("missing-owner"))
        .env(
            "MBX_SESSION_RESTORE",
            serde_json::to_string(&restore).unwrap(),
        )
        .env("RUSTC_WRAPPER", "missing-wrapper")
        .env("MBX_WORKSPACE_ROOT", "wrong-workspace")
        .args(["check", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn command_line_runner_override_uses_original_environment() {
    let root = tempfile::tempdir().unwrap();
    project(
        root.path(),
        "fn main() { assert!(std::env::var_os(\"MBX_SOCKET\").is_none()); }\n",
    );
    let output = mbx(root.path())
        .args([
            "run",
            "--offline",
            "--config",
            "target.'cfg(unix)'.runner = ['env', 'CALLER_RUNNER=1']",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn nested_build_orchestrators_keep_the_live_session() {
    use std::os::unix::fs::PermissionsExt;
    for orchestrator in ["watch", "nextest"] {
        let root = tempfile::tempdir().unwrap();
        project(root.path(), "fn main() {}\n");
        std::fs::write(
            root.path().join("build.rs"),
            r#"
fn main() {
    assert_eq!(std::env::var("MBX_SOCKET").unwrap(), std::env::var("EXPECTED_SOCKET").unwrap());
    assert_eq!(std::env::var("MBX_BUILD").unwrap(), std::env::var("EXPECTED_BUILD").unwrap());
}
"#,
        )
        .unwrap();
        let program = root.path().join(format!("cargo-{orchestrator}"));
        std::fs::write(&program, "#!/bin/sh\nexport EXPECTED_SOCKET=\"$MBX_SOCKET\" EXPECTED_BUILD=\"$MBX_BUILD\"\nMBX_CARGO_SHIM_MODE=1 exec \"$TEST_MBX\" check --offline\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(root.path().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let output = mbx(root.path())
            .env("PATH", path)
            .env("TEST_MBX", env!("CARGO_BIN_EXE_mbx"))
            .arg(orchestrator)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{orchestrator}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn launch_preserves_non_unicode_environment_and_temporary_paths_with_spaces() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    project(
        root.path(),
        r#"
use std::os::unix::ffi::OsStrExt;
fn main() { assert_eq!(std::env::var_os("OPAQUE").unwrap().as_bytes(), b"\xff"); }
"#,
    );
    let temporary = tempfile::Builder::new()
        .prefix("mbx space ")
        .tempdir_in("/tmp")
        .unwrap();
    let output = mbx(root.path())
        .env("TMPDIR", temporary.path())
        .env("MBX_CC", "0")
        .env("OPAQUE", std::ffi::OsString::from_vec(vec![255]))
        .args(["run", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
