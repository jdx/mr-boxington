//! Cargo invocation resolution shared by mbx and embedded cache clients.
//!
//! This crate is intentionally unstable while the first embedders converge.
//! Breaking changes are made in a new pre-1.0 minor release.
#![deny(missing_docs)]

use mbx_cache_core::{CacheDigest, canonical_json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

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
    resolve_in(
        cache_root().as_deref(),
        cargo,
        arguments,
        working_dir,
        target_dir_env,
    )
}

fn resolve_in(
    cache: Option<&Path>,
    cargo: &OsStr,
    arguments: &[String],
    working_dir: &Path,
    target_dir_env: Option<OsString>,
) -> CargoInvocation {
    let cargo_args = cargo_arguments(arguments);
    let reported = recalled_cargo_roots(
        cache,
        cargo,
        cargo_args,
        working_dir,
        target_dir_env.as_deref(),
    );
    resolve_with_reported(arguments, working_dir, target_dir_env, reported)
}

/// Resolve one Cargo invocation only when Cargo successfully reports its roots.
///
/// Persistent Cargo shims use this form to distinguish a usable build from an
/// invocation they should pass through, while retaining the successful
/// metadata result for the session instead of probing twice.
pub fn resolve_reported(
    cargo: &OsStr,
    arguments: &[String],
    working_dir: &Path,
    target_dir_env: Option<OsString>,
) -> Option<CargoInvocation> {
    resolve_reported_in(
        cache_root().as_deref(),
        cargo,
        arguments,
        working_dir,
        target_dir_env,
    )
}

fn resolve_reported_in(
    cache: Option<&Path>,
    cargo: &OsStr,
    arguments: &[String],
    working_dir: &Path,
    target_dir_env: Option<OsString>,
) -> Option<CargoInvocation> {
    let reported = recalled_cargo_roots(
        cache,
        cargo,
        cargo_arguments(arguments),
        working_dir,
        target_dir_env.as_deref(),
    )?;
    Some(resolve_with_reported(
        arguments,
        working_dir,
        target_dir_env,
        Some(reported),
    ))
}

fn resolve_with_reported(
    arguments: &[String],
    working_dir: &Path,
    target_dir_env: Option<OsString>,
    reported: Option<(PathBuf, PathBuf)>,
) -> CargoInvocation {
    let cargo_args = cargo_arguments(arguments);
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
    os: &'static str,
    arch: &'static str,
}

/// Derive the prediction-manifest identity used by mbx for a Cargo command.
///
/// The command is accepted for API compatibility but deliberately does not
/// enter the identity. rustc invocation digests distinguish profiles,
/// features, targets, and compiler toolchains; keeping the surrounding Cargo
/// command here would prevent equivalent dependency compilations from sharing
/// predictions across `build`, `test`, and Clippy.
pub fn build_identity(workspace_root: &Path, _command: &[String]) -> String {
    identity_for_workspace(&workspace_marker(workspace_root))
}

/// The identities recorded for earlier states of this workspace's
/// `Cargo.lock`, newest first.
///
/// The identity is the lockfile's digest, so a dependency bump starts a
/// manifest with nothing in it although most of the graph is unchanged and
/// its results are already cached. Version control remembers what the lockfile
/// was before: the committed copy first, for an edit that has not been
/// committed yet, then the copy in `HEAD`'s first parent, which on a pull
/// request's merge commit is the base branch, then the copy each commit that
/// touched the file replaced. A shallow clone offers what it can reach, which
/// on a `fetch-depth: 1` checkout is nothing beyond `HEAD` itself. A checkout
/// that is not under Git, or a lockfile it does not track, yields nothing.
/// Every prediction a manifest holds is rehashed before it is trusted, so an
/// inherited one can only fail to match, never restore the wrong result.
pub fn previous_build_identities(workspace_root: &Path) -> Vec<String> {
    let Ok(lock) = std::fs::read(workspace_root.join("Cargo.lock")) else {
        return Vec::new();
    };
    // History belongs to the tracked file. A lockfile Cargo generated after a
    // tracked one was deleted has none, whatever the deleted one's was.
    if git_output(
        workspace_root,
        &["ls-files", "--error-unmatch", "--", "Cargo.lock"],
    )
    .is_none()
    {
        return Vec::new();
    }
    let limit = PREVIOUS_LOCKFILE_STATES.to_string();
    let Some(revisions) = git_output(
        workspace_root,
        &["rev-list", "-n", &limit, "HEAD", "--", "Cargo.lock"],
    ) else {
        return Vec::new();
    };
    // A commit's own copy of the lockfile is what its successor replaced, so
    // the parent of each commit that touched the file is one state further
    // back; a root commit has no parent and is skipped.
    let revisions = String::from_utf8_lossy(&revisions);
    let candidates = ["HEAD", "HEAD^"].into_iter().map(str::to_string).chain(
        revisions
            .lines()
            .map(|revision| format!("{}^", revision.trim())),
    );
    let mut seen = BTreeSet::from([CacheDigest::blake3(&lock).hash]);
    let mut identities = Vec::new();
    for revision in candidates {
        let Some(lock) = git_output(
            workspace_root,
            &["show", &format!("{revision}:./Cargo.lock")],
        ) else {
            continue;
        };
        let marker = CacheDigest::blake3(&lock).hash;
        if seen.insert(marker.clone()) {
            identities.push(identity_for_workspace(&marker));
        }
        if identities.len() == PREVIOUS_LOCKFILE_STATES {
            break;
        }
    }
    identities
}

/// How many earlier lockfile states are offered as prediction sources.
///
/// Each one costs a manifest lookup when it is consulted, which only happens
/// once nothing was recorded under the current identity, and a run of
/// dependency bumps rarely goes deeper than this before a build of the trunk
/// records the newest state.
const PREVIOUS_LOCKFILE_STATES: usize = 8;

/// Git's standard output for a command run in `workspace_root`, or nothing
/// when Git is absent or the command fails.
fn git_output(workspace_root: &Path, arguments: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// The manifest identity for one workspace marker on this platform.
fn identity_for_workspace(workspace: &str) -> String {
    let bytes = canonical_json(&ActionIdentity {
        version: 3,
        workspace,
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

/// The roots a `cargo metadata` probe reports, remembered under `cache`.
///
/// The probe is a Cargo process per build, and it costs more than the shim
/// work around a hot compile once the rest has been trimmed. Its answer is a
/// function of things this crate can watch: the Cargo binary, the manifests
/// and configuration files Cargo reads on the way from the invocation
/// directory to the root, and the environment that selects a target
/// directory. A record stands while every one of those is as the probing
/// run saw it, including the ones that were absent; anything else, or no
/// cache to remember in, runs the probe.
fn recalled_cargo_roots(
    cache: Option<&Path>,
    cargo: &OsStr,
    arguments: &[String],
    working_dir: &Path,
    target_dir_env: Option<&OsStr>,
) -> Option<(PathBuf, PathBuf)> {
    let probe = cache.and_then(|cache| {
        ProbeRecord::describe(cache, cargo, arguments, working_dir, target_dir_env)
    });
    if let Some(recalled) = probe.as_ref().and_then(ProbeRecord::recall) {
        return Some(recalled);
    }
    let roots = cargo_roots(cargo, arguments, target_dir_env)?;
    if let Some(probe) = probe {
        probe.remember(&roots);
    }
    Some(roots)
}

const PROBE_RECORD_VERSION: u8 = 1;

/// Everything a probe's answer was a function of, and the answer.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ProbeRecord {
    version: u8,
    key: ProbeKey,
    /// The files Cargo consulted, present or absent, as they were when the
    /// probe ran. The root manifest joins them once the probe has named it.
    pins: Vec<Pin>,
    #[serde(skip)]
    path: PathBuf,
    workspace_root: PathBuf,
    target_dir: PathBuf,
}

/// The inputs that select a record: an identical key with intact pins is
/// the same question, so it gets the same answer.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ProbeKey {
    cargo: PathBuf,
    arguments: Vec<String>,
    working_dir: PathBuf,
    target_dir_env: Option<String>,
    build_target_dir_env: Option<String>,
    cargo_home: Option<String>,
}

/// A file as the probe found it: absent, or present with a length and
/// modification time. Length alone would miss an edit that kept the size.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Pin {
    path: PathBuf,
    state: Option<(u64, u64, u32)>,
}

impl Pin {
    /// Describe `path`, or nothing when the filesystem cannot say enough
    /// about it to notice a change.
    fn describe(path: PathBuf) -> Option<Self> {
        let state = match std::fs::metadata(&path) {
            Ok(metadata) => {
                let modified = metadata
                    .modified()
                    .ok()?
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .ok()?;
                Some((metadata.len(), modified.as_secs(), modified.subsec_nanos()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return None,
        };
        Some(Self { path, state })
    }

    fn holds(&self) -> bool {
        Pin::describe(self.path.clone()).as_ref() == Some(self)
    }
}

impl ProbeRecord {
    /// Describe the probe about to run, or nothing when some input cannot be
    /// pinned, in which case the probe runs and is not remembered.
    fn describe(
        cache: &Path,
        cargo: &OsStr,
        arguments: &[String],
        working_dir: &Path,
        target_dir_env: Option<&OsStr>,
    ) -> Option<Self> {
        let cargo = resolve_program(cargo)?;
        let env =
            |name: &str| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned());
        let key = ProbeKey {
            cargo: cargo.clone(),
            arguments: probe_arguments(arguments),
            working_dir: working_dir.to_path_buf(),
            target_dir_env: target_dir_env.map(|value| value.to_string_lossy().into_owned()),
            build_target_dir_env: env("CARGO_BUILD_TARGET_DIR"),
            cargo_home: env("CARGO_HOME"),
        };
        let invocation_dir = invocation_dir(arguments, working_dir);
        // Cargo finds the manifest nearest the invocation directory, or the
        // one named outright, and walks up from there for the workspace. Its
        // configuration it reads from every `.cargo` above the invocation
        // directory itself, wherever the manifest is, and from home last.
        let manifest_start = flag_value(arguments, "--manifest-path")
            .map(|manifest| absolute(&invocation_dir, manifest))
            .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| invocation_dir.clone());
        let mut watched = vec![cargo];
        watched.extend(
            manifest_start
                .ancestors()
                .map(|directory| directory.join("Cargo.toml")),
        );
        for directory in invocation_dir.ancestors() {
            let dot_cargo = directory.join(".cargo");
            watched.push(dot_cargo.join("config.toml"));
            watched.push(dot_cargo.join("config"));
        }
        if let Some(home) = key
            .cargo_home
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
        {
            watched.push(home.join("config.toml"));
            watched.push(home.join("config"));
        }
        watched.extend(
            config_arguments(arguments)
                .map(|value| absolute(&invocation_dir, value))
                .filter(|path| path.is_file()),
        );
        // A configuration file may `include` others this list does not
        // name, and a command-line `include=` does the same. Their target
        // directory cannot be pinned, so it is not remembered. A `--config`
        // naming a file is on the list above and read like the rest.
        if config_arguments(arguments).any(|value| {
            value
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == "include")
        }) || watched
            .iter()
            .skip(1)
            .filter(|path| path.file_name().is_some_and(|name| name != "Cargo.toml"))
            .any(|path| config_includes_files(path))
        {
            return None;
        }
        let pins = watched
            .into_iter()
            .map(Pin::describe)
            .collect::<Option<Vec<_>>>()?;
        let selector = canonical_json(&key).ok()?;
        let path = cache
            .join("cargo-roots")
            .join("v1")
            .join(format!("{}.json", CacheDigest::blake3(&selector).hash));
        Some(Self {
            version: PROBE_RECORD_VERSION,
            key,
            pins,
            path,
            workspace_root: PathBuf::new(),
            target_dir: PathBuf::new(),
        })
    }

    /// The answer an earlier probe left for this key, if its pins all hold.
    fn recall(&self) -> Option<(PathBuf, PathBuf)> {
        let bytes = std::fs::read(&self.path).ok()?;
        let recorded: ProbeRecord = serde_json::from_slice(&bytes).ok()?;
        if recorded.version != PROBE_RECORD_VERSION
            || recorded.key != self.key
            || !recorded.pins.iter().all(Pin::holds)
        {
            return None;
        }
        Some((recorded.workspace_root, recorded.target_dir))
    }

    /// Leave the answer behind for the next invocation. Best-effort: a
    /// record that cannot be written costs the next build a probe.
    fn remember(mut self, roots: &(PathBuf, PathBuf)) {
        let root_manifest = roots.0.join("Cargo.toml");
        if !self.pins.iter().any(|pin| pin.path == root_manifest) {
            let Some(pin) = Pin::describe(root_manifest) else {
                return;
            };
            self.pins.push(pin);
        }
        self.workspace_root = roots.0.clone();
        self.target_dir = roots.1.clone();
        let Ok(bytes) = serde_json::to_vec(&self) else {
            return;
        };
        let Some(directory) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(directory).is_err() {
            return;
        }
        let staged = directory.join(format!(
            ".{}.{}",
            self.path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        if std::fs::write(&staged, bytes).is_ok() && std::fs::rename(&staged, &self.path).is_err() {
            let _ = std::fs::remove_file(&staged);
        }
    }
}

/// Whether a Cargo configuration file names others through `include`.
///
/// A file that cannot be read or parsed is treated as though it did: the
/// question is whether the probe can be pinned, and a file this cannot see
/// into is one it cannot pin.
fn config_includes_files(path: &Path) -> bool {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    toml::from_str::<toml::Value>(&contents).is_ok_and(|config| config.get("include").is_some())
        || toml::from_str::<toml::Value>(&contents).is_err()
}

/// The probe command's arguments, in the order it runs them: Cargo's global
/// flags, then `metadata`, then the options that pick the manifest.
fn probe_arguments(arguments: &[String]) -> Vec<String> {
    let mut probe = forwarded_flags(arguments, &PROBE_GLOBAL_FLAGS);
    probe.extend(
        ["metadata", "--no-deps", "--format-version", "1"]
            .iter()
            .map(|argument| (*argument).to_string()),
    );
    if let Some(manifest) = flag_value(arguments, "--manifest-path") {
        probe.push("--manifest-path".into());
        probe.push(manifest.into());
    }
    probe.extend(
        arguments
            .iter()
            .filter(|argument| PROBE_MANIFEST_TOGGLES.contains(&argument.as_str()))
            .cloned(),
    );
    probe
}

/// Where `program` is, the way `Command::new` would find it.
fn resolve_program(program: &OsStr) -> Option<PathBuf> {
    let candidate = Path::new(program);
    if candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    let names = if cfg!(windows) {
        vec![candidate.as_os_str().to_os_string(), {
            let mut exe = candidate.as_os_str().to_os_string();
            exe.push(".exe");
            exe
        }]
    } else {
        vec![candidate.as_os_str().to_os_string()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_file())
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
    command.args(probe_arguments(arguments));
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
    fn cargo_commands_and_toolchain_selectors_share_predictions() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("Cargo.lock"), "same").unwrap();

        let build = vec!["build".to_string()];
        let test = vec!["test".to_string(), "--workspace".to_string()];
        let msrv = vec!["+1.91".to_string(), "check".to_string()];

        assert_eq!(
            build_identity(project.path(), &build),
            build_identity(project.path(), &test),
        );
        assert_eq!(
            build_identity(project.path(), &build),
            build_identity(project.path(), &msrv),
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

        let resolved =
            resolve_reported_in(None, OsStr::new("cargo"), &arguments, root, None).unwrap();

        assert_eq!(resolved.target_dir, configured);
        assert!(resolved.target_dir_requested);
    }

    /// A stand-in Cargo that answers `metadata` and logs every call.
    #[cfg(unix)]
    fn logging_cargo(directory: &Path, project: &Path, log: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let script = directory.join("cargo");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\ncase \" $* \" in *' metadata '*) printf '{{\"workspace_root\":\"{root}\",\"target_directory\":\"{root}/target\",\"packages\":[]}}';; esac\n",
                log = log.display(),
                root = project.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    fn probes(log: &Path) -> usize {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("metadata"))
            .count()
    }

    #[test]
    #[cfg(unix)]
    fn a_probe_is_remembered_while_what_cargo_read_stands() {
        let directory = cargo_fixture();
        let root = directory.path();
        let cache = tempfile::tempdir().unwrap();
        let log = root.join("cargo.log");
        let cargo = logging_cargo(root, root, &log);
        let arguments = ["build".to_string(), "--locked".to_string()];
        let resolve = |target_dir_env: Option<&str>| {
            resolve_reported_in(
                Some(cache.path()),
                cargo.as_os_str(),
                &arguments,
                root,
                target_dir_env.map(OsString::from),
            )
            .unwrap()
        };

        let first = resolve(None);
        assert_eq!(first.workspace_root, root);
        assert_eq!(probes(&log), 1);
        // The same question again is answered from the record.
        assert_eq!(resolve(None), first);
        assert_eq!(probes(&log), 1);

        // An edit to a manifest Cargo read runs the probe again.
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        assert_eq!(resolve(None), first);
        assert_eq!(probes(&log), 2);
        assert_eq!(resolve(None), first);
        assert_eq!(probes(&log), 2);

        // So does a configuration file appearing where Cargo would look.
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::write(root.join(".cargo/config.toml"), "[build]\njobs = 2\n").unwrap();
        assert_eq!(resolve(None), first);
        assert_eq!(probes(&log), 3);

        // The environment that selects a target directory is part of the key.
        assert!(resolve(Some("elsewhere")).target_dir_requested);
        assert_eq!(probes(&log), 4);
        assert_eq!(resolve(None), first);
        assert_eq!(probes(&log), 4);

        // A configuration that includes files this cannot see is never
        // remembered: every build probes.
        std::fs::write(
            root.join(".cargo/config.toml"),
            "include = \"other.toml\"\n",
        )
        .unwrap();
        assert_eq!(resolve(None).workspace_root, first.workspace_root);
        assert_eq!(resolve(None).workspace_root, first.workspace_root);
        assert_eq!(probes(&log), 6);
    }

    #[test]
    #[cfg(unix)]
    fn a_config_file_named_on_the_command_line_is_pinned() {
        let directory = cargo_fixture();
        let root = directory.path();
        let cache = tempfile::tempdir().unwrap();
        let log = root.join("cargo.log");
        let cargo = logging_cargo(root, root, &log);
        let extra = root.join("extra.toml");
        std::fs::write(&extra, "[build]\njobs = 2\n").unwrap();
        let arguments = [
            "build".to_string(),
            "--config".to_string(),
            extra.display().to_string(),
        ];
        let resolve = || {
            resolve_reported_in(
                Some(cache.path()),
                cargo.as_os_str(),
                &arguments,
                root,
                None,
            )
            .unwrap()
        };

        let first = resolve();
        assert_eq!(resolve(), first);
        assert_eq!(probes(&log), 1);
        // The file is watched like any other configuration.
        std::fs::write(&extra, "[build]\njobs = 3\n").unwrap();
        assert_eq!(resolve(), first);
        assert_eq!(probes(&log), 2);
        // Until it includes something that cannot be.
        std::fs::write(&extra, "include = \"more.toml\"\n").unwrap();
        resolve();
        resolve();
        assert_eq!(probes(&log), 4);
    }

    #[test]
    #[cfg(unix)]
    fn a_failed_probe_is_not_remembered() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = cargo_fixture();
        let root = directory.path();
        let cache = tempfile::tempdir().unwrap();
        let script = root.join("cargo");
        std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let arguments = ["build".to_string()];

        assert!(
            resolve_reported_in(
                Some(cache.path()),
                script.as_os_str(),
                &arguments,
                root,
                None
            )
            .is_none()
        );
        assert!(!cache.path().join("cargo-roots").exists());
    }

    #[test]
    fn reported_resolution_requires_a_successful_metadata_probe() {
        let directory = cargo_fixture();
        assert!(
            resolve_reported(
                OsStr::new("cargo-that-does-not-exist"),
                &["build".into()],
                directory.path(),
                None,
            )
            .is_none()
        );
    }

    fn git(directory: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["-c", "commit.gpgsign=false", "-c", "user.name=t"])
            .args(["-c", "user.email=t@example.com"])
            .args(arguments)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?} failed");
    }

    #[test]
    fn earlier_lockfile_states_are_offered_newest_first() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let command = Vec::new();
        git(root, &["init", "-q"]);
        let mut committed = Vec::new();
        for state in ["one", "two", "three"] {
            std::fs::write(root.join("Cargo.lock"), state).unwrap();
            committed.push(build_identity(root, &command));
            git(root, &["add", "Cargo.lock"]);
            git(root, &["commit", "-q", "-m", state]);
        }
        committed.reverse();

        // An uncommitted edit: the committed copy is the nearest earlier state.
        std::fs::write(root.join("Cargo.lock"), "four").unwrap();
        assert!(!committed.contains(&build_identity(root, &command)));
        assert_eq!(previous_build_identities(root), committed);

        // Once committed, the working copy matches HEAD and is not offered as
        // its own fallback.
        git(root, &["commit", "-q", "-am", "four"]);
        assert_eq!(previous_build_identities(root), committed);
    }

    #[test]
    fn a_shallow_clone_offers_the_states_it_can_reach() {
        let origin = tempfile::tempdir().unwrap();
        let root = origin.path();
        let command = Vec::new();
        git(root, &["init", "-q"]);
        let mut committed = Vec::new();
        for state in ["one", "two", "three", "four"] {
            std::fs::write(root.join("Cargo.lock"), state).unwrap();
            committed.push(build_identity(root, &command));
            git(root, &["add", "Cargo.lock"]);
            git(root, &["commit", "-q", "-m", state]);
        }
        let clones = tempfile::tempdir().unwrap();
        let shallow = clones.path().join("shallow");
        git(
            clones.path(),
            &[
                "clone",
                "-q",
                "--depth",
                "2",
                &format!("file://{}", root.display()),
                shallow.to_str().unwrap(),
            ],
        );
        // HEAD holds "four"; its parent "three" was fetched and "two" was not.
        assert_eq!(
            previous_build_identities(&shallow),
            vec![committed[2].clone()]
        );
    }

    #[test]
    fn an_untracked_lockfile_has_no_history_to_borrow_from() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join("Cargo.lock"), "tracked").unwrap();
        git(root, &["add", "Cargo.lock"]);
        git(root, &["commit", "-q", "-m", "tracked"]);
        git(root, &["rm", "-q", "Cargo.lock"]);
        git(root, &["commit", "-q", "-m", "removed"]);
        // Cargo generated a replacement nobody tracks.
        std::fs::write(root.join("Cargo.lock"), "generated").unwrap();
        assert!(previous_build_identities(root).is_empty());
    }

    #[test]
    fn a_workspace_outside_version_control_has_no_earlier_states() {
        let directory = cargo_fixture();
        std::fs::write(directory.path().join("Cargo.lock"), "one").unwrap();
        assert!(previous_build_identities(directory.path()).is_empty());
    }
}
