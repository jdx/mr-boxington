//! Cargo invocation resolution shared by mbx and embedded cache clients.
//!
//! This crate is intentionally unstable while the first embedders converge.
//! Breaking changes are made in a new pre-1.0 minor release.
#![deny(missing_docs)]

use mbx_cache_core::{CacheDigest, canonical_json};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

const CARGO_TARGET_DIR_ENV: &str = "CARGO_TARGET_DIR";
const PROBE_GLOBAL_FLAGS: [&str; 3] = ["-C", "--config", "-Z"];
const PROBE_MANIFEST_TOGGLES: [&str; 3] = ["--offline", "--frozen", "--locked"];

/// Cargo-resolved roots and the stable prediction-manifest identity for one invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoInvocation {
    /// Cargo's resolved workspace root.
    pub workspace_root: PathBuf,
    /// Cargo's resolved output directory.
    pub target_dir: PathBuf,
    /// Whether a flag, environment value, or Cargo configuration explicitly selected the target.
    pub target_dir_requested: bool,
    /// Cross-checkout identity used to select the action-prediction manifest.
    pub build_identity: String,
}

/// Resolve one Cargo invocation exactly enough for a cache session.
pub fn resolve(
    cargo: &OsStr,
    arguments: &[String],
    working_dir: &Path,
    target_dir_env: Option<OsString>,
) -> CargoInvocation {
    let cargo_args = cargo_arguments(arguments);
    let reported = cargo_roots(cargo, cargo_args, target_dir_env.as_deref());
    let invocation_dir = invocation_dir(cargo_args, working_dir);
    let workspace_root = reported
        .as_ref()
        .map(|roots| roots.0.clone())
        .unwrap_or_else(|| workspace_root(&invocation_dir));
    let flagged = target_dir_argument(cargo_args);
    let target_dir_requested = flagged.is_some()
        || target_dir_env
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        || cargo_config_may_set_target_dir(cargo_args, &invocation_dir);
    let target_dir = flagged
        .map(|value| absolute(&invocation_dir, value))
        .or_else(|| reported.map(|roots| roots.1))
        .or_else(|| {
            target_dir_env
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .map(|value| absolute(&invocation_dir, &value.to_string_lossy()))
        })
        .unwrap_or_else(|| workspace_root.join("target"));
    let build_identity = build_identity(&workspace_root, arguments);
    CargoInvocation {
        workspace_root,
        target_dir,
        target_dir_requested,
        build_identity,
    }
}

#[derive(Serialize)]
struct ActionIdentity<'a> {
    version: u8,
    workspace: &'a str,
    command: &'a [String],
    os: &'static str,
    arch: &'static str,
}

/// Derive the prediction-manifest identity used by mbx for a Cargo command.
pub fn build_identity(workspace_root: &Path, command: &[String]) -> String {
    let workspace = workspace_marker(workspace_root);
    let bytes = canonical_json(&ActionIdentity {
        version: 2,
        workspace: &workspace,
        command,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })
    .expect("Cargo build identity must serialize");
    CacheDigest::blake3(&bytes).hash
}

fn workspace_marker(workspace_root: &Path) -> String {
    std::fs::read(workspace_root.join("Cargo.lock")).map_or_else(
        |_| {
            workspace_root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        },
        |lock| CacheDigest::blake3(&lock).hash,
    )
}

/// Resolve the shared mbx cache root from the environment, machine config, or platform default.
pub fn cache_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("MBX_CACHE_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(root));
    }
    if let Some(config) = dirs::config_dir().map(|root| root.join("mbx/config.toml"))
        && let Ok(contents) = std::fs::read_to_string(config)
        && let Ok(document) = toml::from_str::<toml::Value>(&contents)
        && let Some(root) = document.get("cache_dir").and_then(toml::Value::as_str)
    {
        return Some(PathBuf::from(root));
    }
    dirs::cache_dir().map(|root| root.join("mbx"))
}

fn cargo_arguments(arguments: &[String]) -> &[String] {
    &arguments[..arguments
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(arguments.len())]
}

fn absolute(working_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        working_dir.join(path)
    }
}

fn forwarded_flags(arguments: &[String], flags: &[&str]) -> Vec<String> {
    let mut forwarded = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if let Some((flag, value)) = argument
            .split_once('=')
            .filter(|(flag, _)| flags.contains(flag))
        {
            forwarded.extend([flag.to_string(), value.to_string()]);
        } else if flags.contains(&argument.as_str()) {
            if let Some(value) = remaining.next() {
                forwarded.extend([argument.clone(), value.clone()]);
            }
        } else if flags
            .iter()
            .any(|flag| flag.len() == 2 && argument.len() > 2 && argument.starts_with(flag))
        {
            forwarded.push(argument.clone());
        }
    }
    forwarded
}

fn invocation_dir(arguments: &[String], working_dir: &Path) -> PathBuf {
    flag_value(arguments, "-C")
        .map(|value| absolute(working_dir, value))
        .unwrap_or_else(|| working_dir.to_path_buf())
}

fn cargo_roots(
    cargo: &OsStr,
    arguments: &[String],
    target_dir_env: Option<&OsStr>,
) -> Option<(PathBuf, PathBuf)> {
    let mut command = Command::new(cargo);
    match target_dir_env {
        Some(value) => command.env(CARGO_TARGET_DIR_ENV, value),
        None => command.env_remove(CARGO_TARGET_DIR_ENV),
    };
    command.args(forwarded_flags(arguments, &PROBE_GLOBAL_FLAGS));
    command.args(["metadata", "--no-deps", "--format-version", "1"]);
    if let Some(manifest) = flag_value(arguments, "--manifest-path") {
        command.args(["--manifest-path", manifest]);
    }
    command.args(
        arguments
            .iter()
            .filter(|argument| PROBE_MANIFEST_TOGGLES.contains(&argument.as_str())),
    );
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    Some((
        PathBuf::from(metadata.get("workspace_root")?.as_str()?),
        PathBuf::from(metadata.get("target_directory")?.as_str()?),
    ))
}

fn target_dir_argument(arguments: &[String]) -> Option<&str> {
    flag_value(arguments, "--target-dir")
}

fn cargo_config_may_set_target_dir(arguments: &[String], invocation_dir: &Path) -> bool {
    if std::env::var_os("CARGO_BUILD_TARGET_DIR").is_some_and(|value| !value.is_empty()) {
        return true;
    }
    if config_arguments(arguments).any(|value| {
        value
            .split_once('=')
            .is_none_or(|(key, _)| matches!(key.trim(), "build.target-dir" | "include"))
    }) {
        return true;
    }
    let project = invocation_dir.ancestors().flat_map(|directory| {
        let cargo = directory.join(".cargo");
        [cargo.join("config.toml"), cargo.join("config")]
    });
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|root| root.join(".cargo")))
        .into_iter()
        .flat_map(|cargo| [cargo.join("config.toml"), cargo.join("config")]);
    project.chain(home).any(|path| config_may_set_target(&path))
}

fn config_arguments(arguments: &[String]) -> impl Iterator<Item = &str> {
    let mut values = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if let Some(value) = argument.strip_prefix("--config=") {
            values.push(value);
        } else if argument == "--config"
            && let Some(value) = remaining.next()
        {
            values.push(value);
        }
    }
    values.into_iter()
}

fn config_may_set_target(path: &Path) -> bool {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let Ok(config) = toml::from_str::<toml::Value>(&contents) else {
        return true;
    };
    config
        .get("build")
        .and_then(|build| build.get("target-dir"))
        .is_some()
        || config.get("include").is_some()
}

fn flag_value<'a>(arguments: &'a [String], flag: &str) -> Option<&'a str> {
    let joined = format!("{flag}=");
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if let Some(value) = argument.strip_prefix(&joined) {
            return Some(value);
        }
        if argument == flag {
            return arguments.next().map(String::as_str);
        }
    }
    None
}

fn workspace_root(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|directory| directory.join("Cargo.toml").is_file())
        .unwrap_or(start)
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cargo_fixture() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(directory.path().join("src")).unwrap();
        std::fs::write(directory.path().join("src/lib.rs"), "").unwrap();
        directory
    }

    #[test]
    fn lockfile_makes_identity_independent_of_checkout_path() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        std::fs::write(left.path().join("Cargo.lock"), "same").unwrap();
        std::fs::write(right.path().join("Cargo.lock"), "same").unwrap();
        let command = vec!["build".to_string(), "--workspace".to_string()];
        assert_eq!(
            build_identity(left.path(), &command),
            build_identity(right.path(), &command)
        );
    }

    #[test]
    fn rustc_flags_after_separator_are_not_cargo_globals() {
        let args = vec![
            "rustc".into(),
            "--".into(),
            "-C".into(),
            "opt-level=3".into(),
        ];
        assert_eq!(
            invocation_dir(cargo_arguments(&args), Path::new("/work")),
            PathBuf::from("/work")
        );
    }

    #[test]
    fn target_directory_flags_and_environment_are_recorded_as_explicit() {
        let directory = cargo_fixture();
        let root = directory.path();
        let cargo = OsStr::new("cargo-that-does-not-exist");
        let plain = ["build".to_string()];

        let default = resolve(cargo, &plain, root, None);
        assert_eq!(default.target_dir, root.join("target"));
        assert!(!default.target_dir_requested);

        for arguments in [
            vec!["build".into(), "--target-dir=target".into()],
            vec!["build".into(), "--target-dir".into(), "target".into()],
        ] {
            let resolved = resolve(cargo, &arguments, root, None);
            assert_eq!(resolved.target_dir, root.join("target"));
            assert!(resolved.target_dir_requested);
        }

        let from_environment = resolve(cargo, &plain, root, Some("elsewhere".into()));
        assert_eq!(from_environment.target_dir, root.join("elsewhere"));
        assert!(from_environment.target_dir_requested);

        let empty_environment = resolve(cargo, &plain, root, Some("".into()));
        assert!(!empty_environment.target_dir_requested);

        let dangling = ["build".into(), "--target-dir".into()];
        assert!(!resolve(cargo, &dangling, root, None).target_dir_requested);
    }

    #[test]
    fn project_config_that_names_the_default_target_is_still_explicit() {
        let directory = cargo_fixture();
        let root = directory.path();
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::write(
            root.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"target\"\n",
        )
        .unwrap();

        let resolved = resolve(
            OsStr::new("cargo-that-does-not-exist"),
            &["build".into()],
            root,
            None,
        );

        assert_eq!(resolved.target_dir, root.join("target"));
        assert!(resolved.target_dir_requested);
    }

    #[test]
    fn command_line_config_include_may_set_the_target_directory() {
        let directory = cargo_fixture();
        let arguments = [
            "build".into(),
            "--config".into(),
            "include='target-config.toml'".into(),
        ];

        let resolved = resolve(
            OsStr::new("cargo-that-does-not-exist"),
            &arguments,
            directory.path(),
            None,
        );

        assert!(resolved.target_dir_requested);
    }

    #[test]
    fn command_line_target_config_reaches_the_cargo_probe() {
        let directory = cargo_fixture();
        let root = directory.path();
        let configured = root.join("configured-target");
        let arguments = [
            "build".into(),
            "--offline".into(),
            "--manifest-path".into(),
            root.join("Cargo.toml").display().to_string(),
            "--config".into(),
            format!("build.target-dir='{}'", configured.display()),
        ];

        let resolved = resolve(OsStr::new("cargo"), &arguments, root, None);

        assert_eq!(resolved.target_dir, configured);
        assert!(resolved.target_dir_requested);
    }
}
