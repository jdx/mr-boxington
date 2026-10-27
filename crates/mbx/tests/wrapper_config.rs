#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use tempfile::TempDir;

const MBX_TEST_BINARY_ENV: &str = "MBX_TEST_MBX";
static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Serializes the builds. It guards no data, so a test that failed while
/// holding it leaves nothing behind for the next one to distrust.
fn build_lock() -> std::sync::MutexGuard<'static, ()> {
    BUILD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct BuildPaths<'a> {
    workspace_wrapper: Option<&'a Path>,
    shared_cache: Option<&'a Path>,
    target_dir: &'a Path,
}

struct Fixture {
    _root: TempDir,
    root: PathBuf,
    project: PathBuf,
    home: PathBuf,
    cargo_home: PathBuf,
    wrapper_log: PathBuf,
    cargo: OsString,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().to_path_buf();
        let project = root_path.join("project");
        let home = root_path.join("home");
        let cargo_home = root_path.join("cargo-home");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cargo_home).unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"wrapper-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            project.join("src/main.rs"),
            r#"#![allow(unexpected_cfgs)]
#[warn(clippy::bool_comparison)]
fn clippy_probe(value: bool) -> bool { value == true }
fn main() {
    let _ = clippy_probe(true);
    let mut markers = Vec::new();
    if cfg!(mbx_file) { markers.push("file"); }
    if cfg!(mbx_env) { markers.push("env"); }
    if cfg!(mbx_raw_env) { markers.push("raw-env"); }
    if cfg!(mbx_cli) { markers.push("cli"); }
    if cfg!(mbx_global) { markers.push("global"); }
    if cfg!(mbx_workspace) { markers.push("workspace"); }
    if cfg!(mbx_workspace_a) { markers.push("workspace-a"); }
    if cfg!(mbx_workspace_b) { markers.push("workspace-b"); }
    if cfg!(mbx_primary) { markers.push("primary"); }
    println!("{}", markers.join(","));
}
"#,
        )
        .unwrap();

        Self {
            _root: root,
            root: root_path.clone(),
            project,
            home,
            cargo_home,
            wrapper_log: root_path.join("wrappers.log"),
            cargo: std::env::var_os("CARGO")
                .or_else(|| option_env!("CARGO").map(OsString::from))
                .unwrap_or_else(|| OsString::from("cargo")),
        }
    }

    fn wrapper(&self, label: &str, cfg: &str) -> PathBuf {
        let bin = self.root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = bin.join(format!("{label}-wrapper"));
        let primary_dispatch = if label == "primary" {
            r#"if [ -n "${MBX_TEST_WORKSPACE_WRAPPER:-}" ] && [ "$compiler" = "$MBX_TEST_WORKSPACE_WRAPPER" ]; then
    if [ "$kind" = "compile" ]; then set -- "$@" --cfg=mbx_primary; fi
    exec "$compiler" "$@"
fi
"#
        } else {
            ""
        };
        let script = format!(
            "#!/bin/sh\nkind=probe\nhas_crate_name=0\nhas_query=0\nfor argument in \"$@\"; do\n    case \"$argument\" in\n        --crate-name) has_crate_name=1 ;;\n        --print|--print=*|-vV|--version) has_query=1 ;;\n    esac\ndone\nif [ \"$has_crate_name\" = 1 ] && [ \"$has_query\" = 0 ]; then kind=compile; fi\nprintf '%s:%s\\n' '{label}' \"$kind\" >> \"$MBX_TEST_WRAPPER_LOG\"\nprintf '%s:%s:compiler=%s\\n' '{label}' \"$kind\" \"$1\" >> \"$MBX_TEST_WRAPPER_LOG\"\nindex=0\nfor argument in \"$@\"; do\n    printf '%s:%s:argv[%s]=%s\\n' '{label}' \"$kind\" \"$index\" \"$argument\" >> \"$MBX_TEST_WRAPPER_LOG\"\n    index=$((index + 1))\ndone\ncompiler=\"$1\"\nshift\n{primary_dispatch}if [ \"$kind\" = \"compile\" ]; then\n    exec \"$compiler\" --cfg={cfg} \"$@\"\nelse\n    exec \"$compiler\" \"$@\"\nfi\n"
        );
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn write_project_config(&self, text: &str) {
        let config_dir = self.project.join(".cargo");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), text).unwrap();
    }

    fn write_home_config(&self, text: &str) {
        std::fs::write(self.cargo_home.join("config.toml"), text).unwrap();
    }

    fn config(main: Option<&Path>, workspace: Option<&Path>) -> String {
        let mut text = String::from("[build]\n");
        if let Some(path) = main {
            text.push_str(&format!("rustc-wrapper = \"{}\"\n", path.display()));
        }
        if let Some(path) = workspace {
            text.push_str(&format!(
                "rustc-workspace-wrapper = \"{}\"\n",
                path.display()
            ));
        }
        text
    }

    fn build(
        &self,
        through_mbx: bool,
        name: &str,
        arguments: &[OsString],
        environment: &[(&str, &OsStr)],
        workspace_wrapper: Option<&Path>,
    ) -> Output {
        self.build_with_cache(
            through_mbx,
            name,
            arguments,
            environment,
            workspace_wrapper,
            None,
        )
    }

    fn build_with_cache(
        &self,
        through_mbx: bool,
        name: &str,
        arguments: &[OsString],
        environment: &[(&str, &OsStr)],
        workspace_wrapper: Option<&Path>,
        shared_cache: Option<&Path>,
    ) -> Output {
        let target_dir = self.root.join(format!("target-{name}"));
        self.build_with_cache_at_target(
            through_mbx,
            name,
            arguments,
            environment,
            BuildPaths {
                workspace_wrapper,
                shared_cache,
                target_dir: &target_dir,
            },
        )
    }

    fn build_with_cache_at_target(
        &self,
        through_mbx: bool,
        name: &str,
        arguments: &[OsString],
        environment: &[(&str, &OsStr)],
        paths: BuildPaths<'_>,
    ) -> Output {
        self.build_subcommand_with_cache_at_target(
            through_mbx,
            name,
            "build",
            arguments,
            environment,
            paths,
        )
    }

    fn build_subcommand(
        &self,
        through_mbx: bool,
        name: &str,
        subcommand: &str,
        arguments: &[OsString],
        environment: &[(&str, &OsStr)],
    ) -> Output {
        let target_dir = self.root.join(format!("target-{name}"));
        self.build_subcommand_with_cache_at_target(
            through_mbx,
            name,
            subcommand,
            arguments,
            environment,
            BuildPaths {
                workspace_wrapper: None,
                shared_cache: None,
                target_dir: &target_dir,
            },
        )
    }

    fn build_subcommand_with_cache_at_target(
        &self,
        through_mbx: bool,
        name: &str,
        subcommand: &str,
        arguments: &[OsString],
        environment: &[(&str, &OsStr)],
        paths: BuildPaths<'_>,
    ) -> Output {
        let BuildPaths {
            workspace_wrapper,
            shared_cache,
            target_dir,
        } = paths;
        let mut command = if through_mbx {
            Command::new(mbx_binary())
        } else {
            Command::new(&self.cargo)
        };
        command
            .current_dir(&self.project)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("CARGO_HOME", &self.cargo_home)
            .env("CARGO", &self.cargo)
            .env("CARGO_BUILD_JOBS", "1")
            .env("CARGO_INCREMENTAL", "0")
            .env("CARGO_PROFILE_DEV_DEBUG", "0")
            .env("CARGO_TARGET_DIR", target_dir)
            .env("MBX_TEST_WRAPPER_LOG", &self.wrapper_log);
        for variable in ["RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "RUSTC"] {
            if let Some(value) = std::env::var_os(variable) {
                command.env(variable, value);
            }
        }
        if let Some(wrapper) = workspace_wrapper {
            command.env("MBX_TEST_WORKSPACE_WRAPPER", wrapper);
        }
        for (key, value) in environment {
            command.env(key, value);
        }
        command.args([subcommand, "--offline", "--jobs=1"]);
        if through_mbx {
            let report = self.root.join(format!("stats-{name}.json"));
            let bypass_log = self.root.join(format!("bypasses-{name}.log"));
            let cache = shared_cache
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.join(format!("cache-{name}")));
            command
                .env("MBX_CACHE_DIR", cache)
                .env("MBX_GC_AUTO", "0")
                .env("MBX_STATS_REPORT", report)
                .env("MBX_BYPASS_LOG", bypass_log);
        }
        command.args(arguments).output().unwrap()
    }

    fn binary_output(&self, name: &str) -> String {
        self.binary_output_at_target(&self.root.join(format!("target-{name}")))
    }

    fn binary_output_at_target(&self, target_dir: &Path) -> String {
        let mut command = Command::new(target_dir.join("debug").join("wrapper-fixture"));
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "fixture binary failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn clear_wrapper_log(&self) {
        let _ = std::fs::remove_file(&self.wrapper_log);
    }

    fn wrapper_log(&self) -> String {
        std::fs::read_to_string(&self.wrapper_log).unwrap_or_default()
    }

    fn wrapper_compiled(&self, label: &str) -> bool {
        let expected = format!("{label}:compile");
        self.wrapper_log()
            .lines()
            .any(|line| line == expected.as_str())
    }
}

fn mbx_binary() -> OsString {
    std::env::var_os(MBX_TEST_BINARY_ENV)
        .or_else(|| option_env!("CARGO_BIN_EXE_mbx").map(OsString::from))
        .expect("set MBX_TEST_MBX or run this as a Cargo integration test")
}

fn clippy_driver() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join("clippy-driver"))
            .find(|candidate| candidate.is_file())
    })
}

fn toml_config_value(path: &Path) -> OsString {
    OsString::from(format!("build.rustc-wrapper='{}'", path.display()))
}

#[test]
fn configured_primary_wrappers_preserve_cargo_effects() {
    let _build_lock = build_lock();
    let mut failures = Vec::new();

    for case in ["file", "cargo-env", "cli", "raw-env"] {
        let fixture = Fixture::new();
        let file_wrapper = fixture.wrapper("file", "mbx_file");
        let env_wrapper = fixture.wrapper("env", "mbx_env");
        let raw_env_wrapper = fixture.wrapper("raw-env", "mbx_raw_env");
        let cli_wrapper = fixture.wrapper("cli", "mbx_cli");
        fixture.write_project_config(&Fixture::config(Some(&file_wrapper), None));

        let mut environment = Vec::new();
        let mut arguments = Vec::new();
        let (selected_label, expected_output) = match case {
            "file" => ("file", "file"),
            "cargo-env" => {
                environment.push(("CARGO_BUILD_RUSTC_WRAPPER", env_wrapper.as_os_str()));
                ("env", "env")
            }
            "cli" => {
                arguments.extend(["--config".into(), toml_config_value(&cli_wrapper)]);
                ("cli", "cli")
            }
            "raw-env" => {
                environment.push(("RUSTC_WRAPPER", raw_env_wrapper.as_os_str()));
                ("raw-env", "raw-env")
            }
            _ => unreachable!(),
        };

        let cargo = fixture.build(
            false,
            &format!("{case}-cargo"),
            &arguments,
            &environment,
            None,
        );
        if !cargo.status.success() {
            failures.push(format!(
                "Cargo pure failed for {case}: {}",
                String::from_utf8_lossy(&cargo.stderr)
            ));
            continue;
        }
        let cargo_log = fixture.wrapper_log();
        if !fixture.wrapper_compiled(selected_label) {
            failures.push(format!(
                "Cargo pure did not run the {case} wrapper for a compilation: {cargo_log}"
            ));
        }
        let cargo_output = fixture.binary_output(&format!("{case}-cargo"));
        if cargo_output != expected_output {
            failures.push(format!(
                "Cargo pure {case} build produced {cargo_output:?}, expected {expected_output:?}"
            ));
        }

        fixture.clear_wrapper_log();
        let mbx = fixture.build(true, case, &arguments, &environment, None);
        if !mbx.status.success() {
            failures.push(format!(
                "mbx failed for {case}: {}",
                String::from_utf8_lossy(&mbx.stderr)
            ));
            continue;
        }
        let mbx_log = fixture.wrapper_log();
        let wrapper_compiled = fixture.wrapper_compiled(selected_label);
        let stderr = String::from_utf8_lossy(&mbx.stderr).to_ascii_lowercase();
        if wrapper_compiled {
            let output = fixture.binary_output(case);
            if output != expected_output {
                failures.push(format!(
                    "mbx ran the wrong effective wrapper for {case}: {output:?}, log={mbx_log}"
                ));
            }
        } else {
            failures.push(format!(
                "mbx did not execute the effective {case} rustc-wrapper: log={mbx_log}; stderr={stderr}"
            ));
        }
    }

    // An empty workspace setting cancels the lower-priority Cargo-home value.
    let fixture = Fixture::new();
    let global_wrapper = fixture.wrapper("global", "mbx_global");
    fixture.write_home_config(&Fixture::config(Some(&global_wrapper), None));
    fixture.write_project_config("[build]\nrustc-wrapper = \"\"\n");
    for (through_mbx, name) in [(false, "empty-cargo"), (true, "empty-mbx")] {
        fixture.clear_wrapper_log();
        let output = fixture.build(through_mbx, name, &[], &[], None);
        if !output.status.success() {
            failures.push(format!(
                "{} failed for the empty project override: {}",
                if through_mbx { "mbx" } else { "Cargo pure" },
                String::from_utf8_lossy(&output.stderr)
            ));
            continue;
        }
        if fixture.wrapper_compiled("global") {
            failures.push(format!(
                "{} ran the Cargo-home wrapper despite the empty project override",
                if through_mbx { "mbx" } else { "Cargo pure" }
            ));
        }
        if !fixture.binary_output(name).is_empty() {
            failures.push(format!(
                "{} applied the masked Cargo-home wrapper",
                if through_mbx { "mbx" } else { "Cargo pure" }
            ));
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn cli_primary_wrapper_without_file_config_is_preserved() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let wrapper = fixture.wrapper("cli-only", "mbx_cli");
    let arguments = ["--config".into(), toml_config_value(&wrapper)];

    let cargo = fixture.build(false, "cli-only-cargo", &arguments, &[], None);
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert!(
        fixture.wrapper_compiled("cli-only"),
        "{}",
        fixture.wrapper_log()
    );
    assert_eq!(fixture.binary_output("cli-only-cargo"), "cli");

    fixture.clear_wrapper_log();
    let mbx = fixture.build(true, "cli-only-mbx", &arguments, &[], None);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert!(
        fixture.wrapper_compiled("cli-only"),
        "mbx did not preserve the CLI-only wrapper: {}; stderr={}",
        fixture.wrapper_log(),
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("cli-only-mbx"), "cli");
}

#[test]
fn cli_config_file_wrapper_is_preserved() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let wrapper = fixture.wrapper("cli-file", "mbx_cli");
    let config_file = fixture.root.join("cli-wrapper.toml");
    std::fs::write(&config_file, Fixture::config(Some(&wrapper), None)).unwrap();
    let arguments = ["--config".into(), config_file.clone().into_os_string()];

    let cargo = fixture.build(false, "cli-file-cargo", &arguments, &[], None);
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert!(
        fixture.wrapper_compiled("cli-file"),
        "{}",
        fixture.wrapper_log()
    );
    assert_eq!(fixture.binary_output("cli-file-cargo"), "cli");

    fixture.clear_wrapper_log();
    let mbx = fixture.build(true, "cli-file-mbx", &arguments, &[], None);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert!(
        fixture.wrapper_compiled("cli-file"),
        "mbx did not preserve the CLI file wrapper: {}; stderr={}",
        fixture.wrapper_log(),
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("cli-file-mbx"), "cli");
}

#[test]
fn cli_config_include_wrapper_is_preserved() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let wrapper = fixture.wrapper("cli-include", "mbx_cli");
    let included = fixture.root.join("cli-included.toml");
    std::fs::write(&included, Fixture::config(Some(&wrapper), None)).unwrap();
    let arguments = [
        "--config".into(),
        OsString::from(format!("include=['{}']", included.display())),
    ];

    let cargo = fixture.build(false, "cli-include-cargo", &arguments, &[], None);
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert!(
        fixture.wrapper_compiled("cli-include"),
        "{}",
        fixture.wrapper_log()
    );
    assert_eq!(fixture.binary_output("cli-include-cargo"), "cli");

    fixture.clear_wrapper_log();
    let mbx = fixture.build(true, "cli-include-mbx", &arguments, &[], None);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert!(
        fixture.wrapper_compiled("cli-include"),
        "mbx did not preserve a wrapper from a command-line include: {}; stderr={}",
        fixture.wrapper_log(),
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("cli-include-mbx"), "cli");
}

#[test]
fn unrelated_cli_config_keeps_the_mbx_session() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let arguments = ["--config".into(), OsString::from("net.offline=true")];
    let mbx = fixture.build(true, "unrelated-cli-config", &arguments, &[], None);

    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("unrelated-cli-config"), "");
    assert!(
        fixture
            .root
            .join("stats-unrelated-cli-config.json")
            .is_file(),
        "an unrelated CLI config should keep the cache session"
    );
}

#[test]
fn configuration_includes_keep_the_session_unless_they_set_a_wrapper() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let config_dir = fixture.project.join(".cargo");
    fixture.write_project_config("include = ['extra.toml']\n");

    std::fs::write(config_dir.join("extra.toml"), "[net]\noffline = true\n").unwrap();
    let unrelated = fixture.build(true, "include-unrelated", &[], &[], None);
    assert!(
        unrelated.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&unrelated.stderr)
    );
    assert!(
        fixture.root.join("stats-include-unrelated.json").is_file(),
        "an include without wrapper keys should keep the cache session"
    );

    let wrapper = fixture.wrapper("file", "mbx_file");
    std::fs::write(
        config_dir.join("extra.toml"),
        Fixture::config(Some(&wrapper), None),
    )
    .unwrap();
    let cargo = fixture.build(false, "include-wrapper-cargo", &[], &[], None);
    assert!(
        cargo.status.success(),
        "Cargo failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    let cargo_output = fixture.binary_output("include-wrapper-cargo");
    fixture.clear_wrapper_log();
    let mbx = fixture.build(true, "include-wrapper-mbx", &[], &[], None);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(
        fixture.binary_output("include-wrapper-mbx"),
        cargo_output,
        "mbx must match Cargo for a wrapper set in an included file: {}",
        fixture.wrapper_log()
    );
}

#[test]
fn builtin_alias_with_unrelated_cli_config_keeps_the_mbx_session() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let arguments = ["--config".into(), OsString::from("net.offline=true")];

    let cargo = fixture.build_subcommand(false, "builtin-alias-cargo", "b", &arguments, &[]);
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert_eq!(fixture.binary_output("builtin-alias-cargo"), "");

    let mbx = fixture.build_subcommand(true, "builtin-alias-mbx", "b", &arguments, &[]);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("builtin-alias-mbx"), "");
    assert!(
        fixture.root.join("stats-builtin-alias-mbx.json").is_file(),
        "an unrelated CLI config on Cargo's built-in `b` alias should keep the cache session"
    );
}

#[test]
fn existing_alias_uses_pre_cli_config_expansion() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    fixture.write_project_config("[alias]\nselected = \"build\"\n");
    let arguments = ["--config".into(), OsString::from("alias.selected='check'")];

    let cargo =
        fixture.build_subcommand(false, "alias-override-cargo", "selected", &arguments, &[]);
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert_eq!(fixture.binary_output("alias-override-cargo"), "");

    let mbx = fixture.build_subcommand(true, "alias-override-mbx", "selected", &arguments, &[]);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("alias-override-mbx"), "");
    assert!(
        fixture.root.join("stats-alias-override-mbx.json").is_file(),
        "a CLI alias override is applied after Cargo expands the existing alias"
    );
}

#[test]
fn local_alias_keeps_cli_file_wrapper_effect() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    fixture.write_project_config("[alias]\nwrapped = \"build\"\n");
    let wrapper = fixture.wrapper("alias-cli-file", "mbx_cli");
    let config_file = fixture.root.join("alias-cli-wrapper.toml");
    std::fs::write(&config_file, Fixture::config(Some(&wrapper), None)).unwrap();
    let arguments = ["--config".into(), config_file.clone().into_os_string()];

    let cargo = fixture.build_subcommand(false, "alias-cli-file-cargo", "wrapped", &arguments, &[]);
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert!(
        fixture.wrapper_compiled("alias-cli-file"),
        "{}",
        fixture.wrapper_log()
    );
    assert_eq!(fixture.binary_output("alias-cli-file-cargo"), "cli");

    fixture.clear_wrapper_log();
    let mbx = fixture.build_subcommand(true, "alias-cli-file-mbx", "wrapped", &arguments, &[]);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert!(
        fixture.wrapper_compiled("alias-cli-file"),
        "mbx did not preserve the wrapper on a local alias: {}; stderr={}",
        fixture.wrapper_log(),
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("alias-cli-file-mbx"), "cli");
}

#[test]
fn alias_expansion_keeps_its_cli_wrapper_config_for_cargo() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let wrapper = fixture.wrapper("alias-inline", "mbx_cli");
    let config_value = toml_config_value(&wrapper).to_string_lossy().into_owned();
    fixture.write_project_config(&format!(
        "[alias]\nwrapped = [\"--config\", \"{config_value}\", \"build\"]\n"
    ));

    let cargo = fixture.build_subcommand(false, "alias-inline-cargo", "wrapped", &[], &[]);
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert!(
        fixture.wrapper_compiled("alias-inline"),
        "{}",
        fixture.wrapper_log()
    );
    assert_eq!(fixture.binary_output("alias-inline-cargo"), "cli");

    fixture.clear_wrapper_log();
    let mbx = fixture.build_subcommand(true, "alias-inline-mbx", "wrapped", &[], &[]);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert!(
        fixture.wrapper_compiled("alias-inline"),
        "mbx did not preserve the wrapper embedded in the alias expansion: {}; stderr={}",
        fixture.wrapper_log(),
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("alias-inline-mbx"), "cli");
}

#[test]
fn an_included_alias_keeps_its_cli_wrapper_config_for_cargo() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let wrapper = fixture.wrapper("include-alias", "mbx_cli");
    let config_value = toml_config_value(&wrapper).to_string_lossy().into_owned();
    fixture.write_project_config("include = ['aliases.toml']\n");
    std::fs::write(
        fixture.project.join(".cargo").join("aliases.toml"),
        format!("[alias]\nwrapped = [\"build\", \"--config\", \"{config_value}\"]\n"),
    )
    .unwrap();

    let cargo = fixture.build_subcommand(false, "include-alias-cargo", "wrapped", &[], &[]);
    assert!(
        cargo.status.success(),
        "Cargo failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert!(
        fixture.wrapper_compiled("include-alias"),
        "{}",
        fixture.wrapper_log()
    );
    assert_eq!(fixture.binary_output("include-alias-cargo"), "cli");

    fixture.clear_wrapper_log();
    let mbx = fixture.build_subcommand(true, "include-alias-mbx", "wrapped", &[], &[]);
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert!(
        fixture.wrapper_compiled("include-alias"),
        "mbx did not preserve the wrapper of an alias defined in an included file: {}; stderr={}",
        fixture.wrapper_log(),
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert_eq!(fixture.binary_output("include-alias-mbx"), "cli");
}

#[test]
fn path_installs_read_wrappers_from_the_installed_package() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    // Cargo reads an install's configuration from the package it installs,
    // not from the directory it was started in.
    let package = fixture.root.join("package");
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::create_dir_all(package.join(".cargo")).unwrap();
    for file in ["Cargo.toml", "src/main.rs"] {
        std::fs::copy(fixture.project.join(file), package.join(file)).unwrap();
    }
    let package_wrapper = fixture.wrapper("package", "mbx_file");
    std::fs::write(
        package.join(".cargo/config.toml"),
        Fixture::config(Some(&package_wrapper), None),
    )
    .unwrap();
    let caller_wrapper = fixture.wrapper("caller", "mbx_env");
    fixture.write_project_config(&Fixture::config(Some(&caller_wrapper), None));

    let install = |through_mbx: bool, name: &str| {
        let destination = fixture.root.join(format!("installed-{name}"));
        let output = fixture.build_subcommand(
            through_mbx,
            name,
            "install",
            &[
                "--path".into(),
                package.clone().into(),
                "--root".into(),
                destination.clone().into(),
            ],
            &[],
        );
        assert!(
            output.status.success(),
            "{name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let run = Command::new(destination.join("bin/wrapper-fixture"))
            .output()
            .unwrap();
        String::from_utf8(run.stdout).unwrap().trim().to_owned()
    };

    assert_eq!(install(false, "path-install-cargo"), "file");
    fixture.clear_wrapper_log();
    assert_eq!(
        install(true, "path-install-mbx"),
        "file",
        "mbx must use the installed package's wrapper: {}",
        fixture.wrapper_log()
    );
    assert!(
        !fixture.wrapper_compiled("caller"),
        "mbx used the caller's wrapper for a path install: {}",
        fixture.wrapper_log()
    );
}

#[test]
fn cli_config_file_does_not_define_an_alias_before_expansion() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let wrapper = fixture.wrapper("cli-only-alias", "mbx_cli");
    let config_file = fixture.root.join("cli-alias-and-wrapper.toml");
    std::fs::write(
        &config_file,
        format!(
            "[alias]\nfrom-cli = \"build\"\n\n{}",
            Fixture::config(Some(&wrapper), None)
        ),
    )
    .unwrap();
    let arguments = ["--config".into(), config_file.clone().into_os_string()];

    let cargo =
        fixture.build_subcommand(false, "cli-only-alias-cargo", "from-cli", &arguments, &[]);
    assert!(
        !cargo.status.success(),
        "Cargo should not use an alias supplied only by a CLI config file"
    );

    let mbx = fixture.build_subcommand(true, "cli-only-alias-mbx", "from-cli", &arguments, &[]);
    assert!(
        !mbx.status.success(),
        "mbx should preserve Cargo's unresolved alias result; stderr={}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    assert!(
        !fixture.wrapper_compiled("cli-only-alias"),
        "Cargo did not resolve the CLI-file alias, so its wrapper should not run"
    );
}

#[test]
fn raw_wrapper_environment_precedence_over_cli_config_control() {
    let _build_lock = build_lock();
    for (case, raw_wrapper) in [("nonempty", Some("raw-env")), ("empty", None)] {
        let fixture = Fixture::new();
        let raw = fixture.wrapper("raw-env", "mbx_raw_env");
        let cli = fixture.wrapper("cli", "mbx_cli");
        let arguments = ["--config".into(), toml_config_value(&cli)];
        let environment = match raw_wrapper {
            Some(_) => vec![("RUSTC_WRAPPER", raw.as_os_str())],
            None => vec![("RUSTC_WRAPPER", OsStr::new(""))],
        };
        let output = fixture.build(false, case, &arguments, &environment, None);
        assert!(
            output.status.success(),
            "Cargo pure failed for {case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let log = fixture.wrapper_log();
        let binary_output = fixture.binary_output(case);
        println!("raw wrapper {case}: output={binary_output:?}; log={log}");
        match raw_wrapper {
            Some(_) => {
                assert!(fixture.wrapper_compiled("raw-env"), "{log}");
                assert!(!fixture.wrapper_compiled("cli"), "{log}");
                assert_eq!(binary_output, "raw-env");
            }
            None => {
                assert!(!fixture.wrapper_compiled("raw-env"), "{log}");
                assert!(!fixture.wrapper_compiled("cli"), "{log}");
                assert!(binary_output.is_empty());
            }
        }

        fixture.clear_wrapper_log();
        let mbx = fixture.build(true, &format!("{case}-mbx"), &arguments, &environment, None);
        assert!(
            mbx.status.success(),
            "mbx failed for {case}: {}",
            String::from_utf8_lossy(&mbx.stderr)
        );
        let log = fixture.wrapper_log();
        let binary_output = fixture.binary_output(&format!("{case}-mbx"));
        println!("mbx raw wrapper {case}: output={binary_output:?}; log={log}");
        match raw_wrapper {
            Some(_) => {
                assert!(fixture.wrapper_compiled("raw-env"), "{log}");
                assert!(!fixture.wrapper_compiled("cli"), "{log}");
                assert_eq!(binary_output, "raw-env");
            }
            None => {
                assert!(!fixture.wrapper_compiled("raw-env"), "{log}");
                assert!(!fixture.wrapper_compiled("cli"), "{log}");
                assert!(binary_output.is_empty());
            }
        }
    }
}

#[test]
fn workspace_wrapper_from_config_keeps_its_effect_without_caching() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let workspace = fixture.wrapper("workspace", "mbx_workspace");
    fixture.write_project_config(&Fixture::config(None, Some(&workspace)));

    let cargo = fixture.build(false, "workspace-cargo", &[], &[], Some(&workspace));
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    assert!(
        fixture.wrapper_compiled("workspace"),
        "{}",
        fixture.wrapper_log()
    );
    assert_eq!(fixture.binary_output("workspace-cargo"), "workspace");

    fixture.clear_wrapper_log();
    let mbx = fixture.build(true, "workspace-mbx", &[], &[], Some(&workspace));
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    let log = fixture.wrapper_log();
    let bypasses = std::fs::read_to_string(fixture.root.join("bypasses-workspace-mbx.log"))
        .unwrap_or_default();
    let stats: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.root.join("stats-workspace-mbx.json")).unwrap(),
    )
    .unwrap();
    assert!(
        fixture.wrapper_compiled("workspace"),
        "mbx did not execute the Cargo-configured workspace wrapper: {log}"
    );
    assert_eq!(fixture.binary_output("workspace-mbx"), "workspace");
    assert_eq!(
        stats["hits"].as_u64(),
        Some(0),
        "an external workspace wrapper must not reuse a cached compile: bypasses={bypasses}; stats={stats}"
    );
}

#[test]
fn configured_primary_and_workspace_wrappers_keep_the_cargo_chain() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let primary = fixture.wrapper("primary", "mbx_primary");
    let workspace = fixture.wrapper("workspace", "mbx_workspace");
    fixture.write_project_config(&Fixture::config(Some(&primary), Some(&workspace)));

    let cargo = fixture.build(false, "chain-cargo", &[], &[], Some(&workspace));
    assert!(
        cargo.status.success(),
        "Cargo pure failed: {}",
        String::from_utf8_lossy(&cargo.stderr)
    );
    let cargo_log = fixture.wrapper_log();
    assert!(fixture.wrapper_compiled("primary"), "{cargo_log}");
    assert!(fixture.wrapper_compiled("workspace"), "{cargo_log}");
    assert_eq!(fixture.binary_output("chain-cargo"), "workspace,primary");

    fixture.clear_wrapper_log();
    let mbx = fixture.build(true, "chain-mbx", &[], &[], Some(&workspace));
    assert!(
        mbx.status.success(),
        "mbx failed: {}",
        String::from_utf8_lossy(&mbx.stderr)
    );
    let log = fixture.wrapper_log();
    let stderr = String::from_utf8_lossy(&mbx.stderr).to_ascii_lowercase();
    let output = fixture.binary_output("chain-mbx");
    assert!(
        fixture.wrapper_compiled("workspace") && output.contains("workspace"),
        "mbx lost the workspace wrapper effect: output={output:?}; log={log}"
    );
    assert!(
        fixture.wrapper_compiled("primary"),
        "mbx did not execute the primary Cargo-configured wrapper: output={output:?}; log={log}; stderr={stderr}"
    );
    assert_eq!(output, "workspace,primary");
}

#[test]
fn workspace_wrapper_change_does_not_reuse_stale_output() {
    let _build_lock = build_lock();
    let fixture = Fixture::new();
    let wrappers = [
        ("workspace-a", "mbx_workspace_a"),
        ("workspace-b", "mbx_workspace_b"),
    ];
    let shared_cache = fixture.root.join("shared-cache");
    let shared_target = fixture.root.join("target-shared-wrapper-identity");

    for (index, (label, cfg)) in wrappers.into_iter().enumerate() {
        if index > 0 {
            std::fs::remove_dir_all(&shared_target).unwrap();
        }
        let wrapper = fixture.wrapper(label, cfg);
        fixture.write_project_config(&Fixture::config(None, Some(&wrapper)));

        let cargo_name = format!("{label}-cargo");
        let cargo = fixture.build(false, &cargo_name, &[], &[], Some(&wrapper));
        assert!(
            cargo.status.success(),
            "Cargo pure failed for {label}: {}",
            String::from_utf8_lossy(&cargo.stderr)
        );
        assert!(
            fixture.wrapper_compiled(label),
            "Cargo pure did not execute {label} for a compilation: {}",
            fixture.wrapper_log()
        );
        assert_eq!(fixture.binary_output(&cargo_name), label);

        fixture.clear_wrapper_log();
        let mbx = fixture.build_with_cache_at_target(
            true,
            label,
            &[],
            &[],
            BuildPaths {
                workspace_wrapper: Some(&wrapper),
                shared_cache: Some(&shared_cache),
                target_dir: &shared_target,
            },
        );
        assert!(
            mbx.status.success(),
            "mbx failed for {label}: {}",
            String::from_utf8_lossy(&mbx.stderr)
        );
        assert!(
            fixture.wrapper_compiled(label),
            "mbx did not execute {label} for a compilation: {}",
            fixture.wrapper_log()
        );
        assert_eq!(fixture.binary_output_at_target(&shared_target), label);

        let stats: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.root.join(format!("stats-{label}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            stats["hits"].as_u64(),
            Some(0),
            "a changed external workspace wrapper must not reuse the previous output: {stats}"
        );
    }
}

#[test]
fn modeled_clippy_driver_from_config_and_environment_keeps_cache_hits() {
    let _build_lock = build_lock();
    let Some(driver) = clippy_driver() else {
        eprintln!("clippy-driver is unavailable; skipping the wrapper parity check");
        return;
    };
    let fixture = Fixture::new();
    let shared_cache = fixture.root.join("shared-clippy-cache");
    let config_wrapper = Fixture::config(None, Some(&driver));
    let environment = [("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", driver.as_os_str())];

    fixture.write_project_config(&config_wrapper);
    let cargo_config = fixture.build(false, "clippy-config-cargo", &[], &[], None);
    assert!(
        cargo_config.status.success(),
        "Cargo config control failed: {}",
        String::from_utf8_lossy(&cargo_config.stderr)
    );
    assert!(
        String::from_utf8_lossy(&cargo_config.stderr).contains("clippy::bool_comparison"),
        "Cargo config control did not run clippy-driver: {}",
        String::from_utf8_lossy(&cargo_config.stderr)
    );

    fixture.write_project_config("[build]\n");
    let cargo_env = fixture.build(false, "clippy-env-cargo", &[], &environment, None);
    assert!(
        cargo_env.status.success(),
        "Cargo environment control failed: {}",
        String::from_utf8_lossy(&cargo_env.stderr)
    );
    assert!(
        String::from_utf8_lossy(&cargo_env.stderr).contains("clippy::bool_comparison"),
        "Cargo environment control did not run clippy-driver: {}",
        String::from_utf8_lossy(&cargo_env.stderr)
    );

    fixture.write_project_config(&config_wrapper);
    let config_target = fixture.root.join("target-clippy-config-mbx");
    let mbx_config = fixture.build_with_cache_at_target(
        true,
        "clippy-config-mbx",
        &[],
        &[],
        BuildPaths {
            workspace_wrapper: None,
            shared_cache: Some(&shared_cache),
            target_dir: &config_target,
        },
    );
    assert!(
        mbx_config.status.success(),
        "mbx config wrapper failed: {}",
        String::from_utf8_lossy(&mbx_config.stderr)
    );
    assert!(
        String::from_utf8_lossy(&mbx_config.stderr).contains("clippy::bool_comparison"),
        "mbx config wrapper did not run clippy-driver: {}",
        String::from_utf8_lossy(&mbx_config.stderr)
    );
    let config_stats: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.root.join("stats-clippy-config-mbx.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config_stats["hits"].as_u64(), Some(0), "{config_stats}");
    assert!(
        config_stats["stored_bytes"].as_u64().unwrap_or(0) > 0,
        "the configured clippy-driver compile was not cached: {config_stats}"
    );

    fixture.write_project_config("[build]\n");
    let env_target = fixture.root.join("target-clippy-env-mbx");
    let mbx_env = fixture.build_with_cache_at_target(
        true,
        "clippy-env-mbx",
        &[],
        &environment,
        BuildPaths {
            workspace_wrapper: None,
            shared_cache: Some(&shared_cache),
            target_dir: &env_target,
        },
    );
    assert!(
        mbx_env.status.success(),
        "mbx environment wrapper failed: {}",
        String::from_utf8_lossy(&mbx_env.stderr)
    );
    assert!(
        String::from_utf8_lossy(&mbx_env.stderr).contains("clippy::bool_comparison"),
        "mbx environment wrapper did not run or restore clippy-driver output: {}",
        String::from_utf8_lossy(&mbx_env.stderr)
    );
    let env_stats: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.root.join("stats-clippy-env-mbx.json")).unwrap(),
    )
    .unwrap();
    assert!(
        env_stats["hits"].as_u64().unwrap_or(0) > 0,
        "the env and config forms of the same clippy-driver did not share the cached compile: {env_stats}"
    );
}
