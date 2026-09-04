//! Cache Cargo build-script execution after the script has declared its inputs.

use crate::materialize::{
    apply_file_mode, denormalize_output_text, file_mode, find_blobs, normalize_output_text,
    read_canonical_blob, read_verified_blob, record_action_hit, replay_bytes, staging_directory,
    validate_file_mode,
};
use crate::session;
use eyre::{Context as _, Result, bail};
use mbx_cache_core::{
    ActionPrediction, AgentRequest, AgentResponse, CacheDigest, CacheDirectory, CacheDirectoryNode,
    CacheFileNode, CacheSymlinkNode, RemoteActionResult, RestoreStats, canonical_json,
};
use mbx_cache_rustc::PathMapping;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

const ADAPTER: &str = "build-script";

#[derive(Debug, Serialize)]
struct Invocation<'a> {
    binary_action: &'a CacheDigest,
    kind: &'static str,
    version: u8,
}

#[derive(Debug, Serialize)]
struct Action<'a> {
    binary_action: &'a CacheDigest,
    cargo_environment: &'a BTreeMap<String, Option<String>>,
    environment: &'a BTreeMap<String, Option<String>>,
    inputs: &'a BTreeMap<String, InputState>,
    kind: &'static str,
    out_dir: Option<&'a str>,
    version: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum InputState {
    Missing,
    File {
        digest: CacheDigest,
    },
    Directory {
        digest: CacheDigest,
    },
    Symlink {
        target: String,
        referent: Box<InputState>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prediction {
    #[serde(default)]
    default_package: bool,
    environment: Vec<String>,
    inputs: Vec<String>,
    portable_out_dir: bool,
    version: u8,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    kind: String,
    stderr: CacheDigest,
    stdout: CacheDigest,
    version: u8,
}

struct Restored {
    stats: RestoreStats,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Preserve the real program beside Cargo's expected path and replace it with mbx.
pub(crate) fn install(executable: &Path, binary_action: &CacheDigest) -> Result<()> {
    // Cargo decides whether a build-script compilation is fresh partly by
    // comparing this path's mtime with its dependencies. The path becomes a
    // shim below, but it still has to look as new as the binary it represents:
    // copying the mbx executable can otherwise give it mbx's older mtime and
    // make Cargo compile the build script again on every invocation.
    let modified = std::fs::metadata(executable)?
        .modified()
        .wrap_err("failed to inspect the build-script modification time")?;
    let real = session::build_script_real_path(executable);
    let temporary = real.with_extension("mbx-real-new");
    let _ = std::fs::remove_file(&temporary);
    std::fs::copy(executable, &temporary).wrap_err("failed to preserve the build script")?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&temporary)?
        .set_times(std::fs::FileTimes::new().set_modified(modified))?;
    let _ = std::fs::remove_file(&real);
    std::fs::rename(&temporary, &real)?;
    std::fs::write(
        build_script_action_path(&real),
        canonical_json(binary_action)?,
    )?;

    // Unix needs only a tiny launcher at each Cargo-owned path. Pin one mbx
    // binary beside the profile and let every launcher exec it while carrying
    // its own path out of band. Besides saving local disk, keeping the launchers
    // distinct preserves the mtimes Cargo uses for freshness. Windows keeps the
    // self-contained executable because Cargo needs a PE binary there.
    let mbx = std::env::current_exe().wrap_err("failed to locate the mbx shim")?;
    let _ = std::fs::remove_file(executable);
    let installed = install_launcher(&mbx, executable).and_then(|_| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(executable)?
            .set_times(std::fs::FileTimes::new().set_modified(modified))
    });
    if let Err(error) = installed {
        let _ = std::fs::rename(&real, executable);
        return Err(error).wrap_err("failed to install the build-script shim");
    }
    Ok(())
}

#[cfg(unix)]
fn install_launcher(mbx: &Path, executable: &Path) -> std::io::Result<()> {
    let profile = executable
        .ancestors()
        .nth(3)
        .ok_or_else(|| std::io::Error::other("build-script path has no Cargo profile directory"))?;
    let metadata = std::fs::metadata(mbx)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    let absolute = std::path::absolute(mbx)?;
    let mut identity = absolute.as_os_str().as_encoded_bytes().to_vec();
    identity.push(0);
    identity.extend_from_slice(&metadata.len().to_le_bytes());
    identity.extend_from_slice(&modified.map_or(0, |time| time.as_nanos()).to_le_bytes());
    let identity = CacheDigest::blake3(&identity);
    // Build-script executables always sit at `<profile>/build/<unit>/<name>`;
    // put the pinned binary under that profile so the launcher stays portable
    // when a target directory moves between checkouts or CI runners.
    let relative = PathBuf::from(".mbx-build-script-shims")
        .join(&identity.hash)
        .join("mbx");
    let pinned = profile.join(&relative);
    install_pinned_binary(mbx, &pinned)?;

    let launcher = format!(
        "#!/bin/sh\n{}=\"$0\" exec \"$(dirname \"$0\")/../../{}\" \"$@\"\n",
        session::BUILD_SCRIPT_SHIM_PATH_ENV,
        relative.to_string_lossy(),
    );
    std::fs::write(executable, launcher)?;
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o755))
}

#[cfg(unix)]
fn install_pinned_binary(mbx: &Path, destination: &Path) -> std::io::Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| std::io::Error::other("pinned shim has no parent directory"))?;
    std::fs::create_dir_all(parent)?;

    // macOS can SIGKILL an exec through a hard link created moments earlier.
    // A copied inode published by rename does not hit that kernel race. Races
    // between installers are harmless because this identity names identical
    // mbx bytes, and rename keeps the destination complete at every instant.
    #[cfg(target_os = "macos")]
    {
        if destination.exists() {
            return Ok(());
        }
        let temporary = tempfile::NamedTempFile::new_in(parent)?;
        std::fs::copy(mbx, temporary.path())?;
        let mut permissions = std::fs::metadata(temporary.path())?.permissions();
        permissions.set_mode(permissions.mode() | 0o100);
        std::fs::set_permissions(temporary.path(), permissions)?;
        let (file, temporary) = temporary.into_parts();
        drop(file);
        temporary
            .persist(destination)
            .map_err(|error| error.error)?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        match std::fs::hard_link(mbx, destination) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(_) => {}
        }

        let temporary = tempfile::NamedTempFile::new_in(parent)?;
        std::fs::copy(mbx, temporary.path())?;
        let mut permissions = std::fs::metadata(temporary.path())?.permissions();
        permissions.set_mode(permissions.mode() | 0o100);
        std::fs::set_permissions(temporary.path(), permissions)?;
        let (file, temporary) = temporary.into_parts();
        drop(file);
        match std::fs::hard_link(&temporary, destination) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }
}

#[cfg(not(unix))]
fn install_launcher(mbx: &Path, executable: &Path) -> std::io::Result<()> {
    std::fs::copy(mbx, executable).map(|_| ())
}

/// Run the preserved program without consulting the cache.
pub(crate) fn run_real() -> ExitCode {
    let Some(invoked) = session::build_script_invocation_path() else {
        return ExitCode::FAILURE;
    };
    let Some(real) = session::find_build_script_real_path(&invoked) else {
        eprintln!("mbx[error]: the preserved build script is missing");
        return ExitCode::FAILURE;
    };
    let mut command = Command::new(real);
    command.args(std::env::args_os().skip(1));
    command.env_remove(session::BUILD_SCRIPT_SHIM_PATH_ENV);
    match command.status() {
        Ok(status) => crate::materialize::exit_code(status),
        Err(error) => {
            eprintln!("mbx[error]: failed to execute the build script: {error}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn run() -> Result<ExitCode> {
    let invoked = session::build_script_invocation_path()
        .ok_or_else(|| eyre::eyre!("build-script shim has no argv0"))?;
    let real = session::find_build_script_real_path(&invoked)
        .ok_or_else(|| eyre::eyre!("preserved build script is missing"))?;
    let action_path = build_script_action_path(&real);
    let action_bytes = std::fs::read(&action_path)?;
    let binary_action: CacheDigest = serde_json::from_slice(&action_bytes)?;
    if binary_action.validate().is_err() || canonical_json(&binary_action)? != action_bytes {
        bail!("build-script binary action is not canonical");
    }
    let invocation_bytes = canonical_json(&Invocation {
        binary_action: &binary_action,
        kind: ADAPTER,
        version: 1,
    })?;
    let invocation = CacheDigest::blake3(&invocation_bytes);

    if let Some(prediction) = find_prediction(&invocation)? {
        let (action_bytes, action) = build_action(&binary_action, &prediction)?;
        if let Some(restored) = restore(&action, &action_bytes)? {
            record_action_hit(&action, restored.stats, cargo_package_name());
            replay_bytes(&restored.stdout, &restored.stderr)?;
            return Ok(ExitCode::SUCCESS);
        }
    }

    let mut command = Command::new(&real);
    command.args(std::env::args_os().skip(1));
    command.env_remove(session::BUILD_SCRIPT_SHIM_PATH_ENV);
    let output = command
        .output()
        .wrap_err("failed to execute the build script")?;
    replay_bytes(&output.stdout, &output.stderr)?;
    if !output.status.success() {
        return Ok(crate::materialize::exit_code(output.status));
    }

    let caching = (|| -> Result<()> {
        let Some(prediction) = parse_prediction(&output.stdout)? else {
            record_bypass("build-script-always-rerun");
            return Ok(());
        };
        let (action_bytes, action) = build_action(&binary_action, &prediction)?;
        publish(&action, &action_bytes, &output.stdout, &output.stderr)?;
        record_prediction(invocation, action, &prediction)
    })();
    if let Err(error) = caching {
        // The script has already run and its streams have already reached
        // Cargo. Cache bookkeeping may fail, but retrying the program would
        // duplicate arbitrary side effects and every emitted directive.
        session::report_shim_warning(&format!("build-script result was not stored: {error:#}"));
    }
    Ok(ExitCode::SUCCESS)
}

fn record_bypass(kind: &str) {
    let _ = session::request_agent(&[AgentRequest::RecordBypass { kind: kind.into() }]);
}

fn cargo_package_name() -> &'static str {
    // Statistics only need a stable, bounded label. The rustc convention is
    // retained when Cargo did not provide one.
    "build_script_build"
}

fn build_script_action_path(real: &Path) -> PathBuf {
    let mut path = real.as_os_str().to_os_string();
    path.push(".action");
    PathBuf::from(path)
}

fn find_prediction(invocation: &CacheDigest) -> Result<Option<Prediction>> {
    let task = std::env::var(session::BUILD_ENV).wrap_err("build session has no task identity")?;
    let responses = session::request_agent(&[AgentRequest::FindActionPrediction {
        task,
        invocation: invocation.clone(),
    }])?;
    match responses.into_iter().next() {
        Some(AgentResponse::ActionPrediction {
            prediction: Some(found),
        }) if found.adapter == ADAPTER && found.invocation == *invocation => {
            Ok(Some(decode_prediction(&found.payload)?))
        }
        Some(AgentResponse::ActionPrediction { prediction: None }) => Ok(None),
        Some(AgentResponse::Error { message }) => bail!(message),
        _ => bail!("cache agent returned an unexpected build-script prediction response"),
    }
}

fn decode_prediction(payload: &str) -> Result<Prediction> {
    let value: serde_json::Value = serde_json::from_str(payload)?;
    if canonical_json(&value)? != payload.as_bytes() {
        bail!("cached build-script prediction is unsupported");
    }
    let has_default_package = value
        .as_object()
        .is_some_and(|value| value.contains_key("default_package"));
    let prediction: Prediction = serde_json::from_value(value)?;
    let supported_version = match prediction.version {
        // Version 1 predates Cargo-default package inputs. Every prediction it
        // stored came from explicit rerun directives, so false is exact.
        1 => !has_default_package && !prediction.default_package,
        2 => has_default_package,
        _ => false,
    };
    if !supported_version
        || (prediction.default_package
            && (prediction.inputs != ["${build_script_manifest_dir}"]
                || !prediction.environment.is_empty()))
    {
        bail!("cached build-script prediction is unsupported");
    }
    Ok(prediction)
}

fn record_prediction(
    invocation: CacheDigest,
    action: CacheDigest,
    prediction: &Prediction,
) -> Result<()> {
    let task = std::env::var(session::BUILD_ENV)?;
    let payload = String::from_utf8(canonical_json(prediction)?)?;
    let responses = session::request_agent(&[AgentRequest::RecordActionPrediction {
        task,
        prediction: ActionPrediction {
            invocation,
            action,
            adapter: ADAPTER.into(),
            payload,
        },
    }])?;
    match responses.into_iter().next() {
        Some(AgentResponse::ActionPredictionRecorded) => Ok(()),
        Some(AgentResponse::Error { message }) => bail!(message),
        _ => bail!("cache agent did not record the build-script prediction"),
    }
}

fn parse_prediction(stdout: &[u8]) -> Result<Option<Prediction>> {
    let text = std::str::from_utf8(stdout).wrap_err("build-script stdout is not UTF-8")?;
    let mappings = build_script_mappings();
    parse_prediction_with_mappings(text, &mappings)
}

fn parse_prediction_with_mappings(
    text: &str,
    mappings: &[PathMapping],
) -> Result<Option<Prediction>> {
    let mut inputs = BTreeSet::new();
    let mut environment = BTreeSet::new();
    for line in text.lines() {
        let directive = line
            .strip_prefix("cargo::")
            .or_else(|| line.strip_prefix("cargo:"));
        let Some(directive) = directive else { continue };
        if let Some(path) = directive.strip_prefix("rerun-if-changed=") {
            // Cargo treats the empty path as an always-rerun declaration. No
            // finite action key can preserve that contract.
            if path.is_empty() {
                return Ok(None);
            }
            let path = normalize_environment_value(path, mappings);
            // A generated file cannot also be an input to the execution that
            // generated it. Build scripts use this shape to make Cargo run
            // them every time (shadow-rs does so in release builds), and
            // publishing it under a key derived after execution makes two
            // different results contend for one local action. Preserve the
            // script's always-rerun contract instead of trying to cache it.
            if path.starts_with("${build_script_out_dir}")
                || path.starts_with("${build_script_out_dir:")
            {
                return Ok(None);
            }
            inputs.insert(path);
        } else if let Some(name) = directive.strip_prefix("rerun-if-env-changed=")
            && !name.is_empty()
        {
            environment.insert(name.to_string());
        }
    }
    let default_package = inputs.is_empty() && environment.is_empty();
    if default_package {
        // With no rerun directives Cargo treats the complete package source as
        // the build script's input. Keep that implicit declaration explicit in
        // the prediction so equivalent package trees can share the execution.
        inputs.insert("${build_script_manifest_dir}".into());
    }
    Ok(Some(Prediction {
        default_package,
        inputs: inputs.into_iter().collect(),
        environment: environment.into_iter().collect(),
        portable_out_dir: out_dir_is_portable()?,
        version: 2,
    }))
}

fn build_action(
    binary_action: &CacheDigest,
    prediction: &Prediction,
) -> Result<(Vec<u8>, CacheDigest)> {
    let working_dir = std::env::current_dir()?;
    let mappings = build_script_mappings();
    let mut inputs = BTreeMap::new();
    for declared in &prediction.inputs {
        let resolved_name =
            String::from_utf8(denormalize_output_text(declared.as_bytes(), &mappings))
                .expect("denormalizing UTF-8 paths preserves UTF-8");
        let path = Path::new(&resolved_name);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            working_dir.join(path)
        };
        let state = if prediction.default_package {
            package_input_state(&resolved, &mappings)?
        } else {
            input_state(&resolved, &mappings)?
        };
        inputs.insert(declared.clone(), state);
    }
    let environment = prediction
        .environment
        .iter()
        .map(|name| {
            let value = std::env::var_os(name)
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| eyre::eyre!("environment input is not UTF-8: {name}"))
                })
                .transpose()?
                .map(|value| normalize_environment_value(&value, &mappings));
            Ok((name.clone(), value))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let cargo_environment = cargo_environment(&mappings);
    let out_dir = (!prediction.portable_out_dir)
        .then(|| std::env::var("OUT_DIR"))
        .transpose()?;
    let bytes = canonical_json(&Action {
        binary_action,
        cargo_environment: &cargo_environment,
        environment: &environment,
        inputs: &inputs,
        kind: ADAPTER,
        out_dir: out_dir.as_deref(),
        version: 2,
    })?;
    let digest = CacheDigest::blake3(&bytes);
    Ok((bytes, digest))
}

fn input_state(path: &Path, mappings: &[PathMapping]) -> Result<InputState> {
    input_state_at(path, mappings, &[], 0)
}

fn package_input_state(path: &Path, mappings: &[PathMapping]) -> Result<InputState> {
    let mut excluded = vec![path.join(".git"), path.join("target")];
    if let Some(target) = std::env::var_os(session::TARGET_DIR_ENV) {
        let target = std::path::absolute(target)?;
        if target.starts_with(path) {
            excluded.push(target);
        }
    }
    input_state_at(path, mappings, &excluded, 0)
}

fn input_state_at(
    path: &Path,
    mappings: &[PathMapping],
    excluded: &[PathBuf],
    symlink_depth: usize,
) -> Result<InputState> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InputState::Missing);
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        if symlink_depth >= 64 {
            bail!("declared build-script input contains a symlink cycle");
        }
        let target = std::fs::read_link(path)?;
        let resolved = if target.is_absolute() {
            target.clone()
        } else {
            path.parent().unwrap_or_else(|| Path::new("")).join(&target)
        };
        return Ok(InputState::Symlink {
            target: normalize_environment_value(&target.to_string_lossy(), mappings),
            referent: Box::new(input_state_at(
                &resolved,
                mappings,
                excluded,
                symlink_depth + 1,
            )?),
        });
    }
    if metadata.is_file() {
        return Ok(InputState::File {
            digest: CacheDigest::blake3_file(path)?,
        });
    }
    if metadata.is_dir() {
        let mut entries = std::fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let children = entries
            .into_iter()
            .filter(|entry| !excluded.iter().any(|excluded| entry.path() == *excluded))
            .map(|entry| {
                Ok((
                    entry.file_name().to_string_lossy().into_owned(),
                    input_state_at(&entry.path(), mappings, excluded, symlink_depth)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        return Ok(InputState::Directory {
            digest: CacheDigest::blake3(&canonical_json(&children)?),
        });
    }
    bail!(
        "declared build-script input is not a file or directory: {}",
        path.display()
    )
}

fn out_dir_is_portable() -> Result<bool> {
    let Some(out_dir) = std::env::var_os("OUT_DIR") else {
        return Ok(false);
    };
    let needles = [
        "OUT_DIR",
        "CARGO_MANIFEST_DIR",
        session::WORKSPACE_ROOT_ENV,
        session::TARGET_DIR_ENV,
        "CARGO_HOME",
    ]
    .into_iter()
    .filter_map(std::env::var_os)
    .map(|value| value.to_string_lossy().into_owned())
    .filter(|value| !value.is_empty())
    .collect::<BTreeSet<_>>();
    let root = PathBuf::from(&out_dir);
    if !root.is_dir() {
        return Ok(true);
    }
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_symlink() {
                let target = std::fs::read_link(entry.path())?;
                if needles
                    .iter()
                    .any(|needle| target.to_string_lossy().contains(needle))
                {
                    return Ok(false);
                }
            }
        }
    }
    for path in walk_files(&root)? {
        let contents = std::fs::read(path)?;
        if needles
            .iter()
            .any(|needle| memchr::memmem::find(&contents, needle.as_bytes()).is_some())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}

fn publish(action: &CacheDigest, action_bytes: &[u8], stdout: &[u8], stderr: &[u8]) -> Result<()> {
    let out_dir = PathBuf::from(
        std::env::var_os("OUT_DIR").ok_or_else(|| eyre::eyre!("build script has no OUT_DIR"))?,
    );
    let staging = staging_directory()?;
    let mut blobs = Vec::new();
    let root = store_directory(&out_dir, staging.path(), &mut blobs)?;
    let mappings = build_script_mappings();
    let stdout = stage_bytes(
        staging.path(),
        "stdout",
        &normalize_output_text(stdout, &mappings),
    )?;
    let stderr = stage_bytes(
        staging.path(),
        "stderr",
        &normalize_output_text(stderr, &mappings),
    )?;
    let action_blob = stage_bytes(staging.path(), "action", action_bytes)?;
    let metadata = canonical_json(&Metadata {
        kind: ADAPTER.into(),
        version: 1,
        stdout: stdout.0.clone(),
        stderr: stderr.0.clone(),
    })?;
    let metadata = stage_bytes(staging.path(), "metadata", &metadata)?;
    blobs.extend([stdout, stderr, action_blob, metadata.clone()]);
    let mut requests = Vec::new();
    let mut seen = BTreeSet::new();
    for (digest, source) in blobs {
        if seen.insert(digest.clone()) {
            requests.push(AgentRequest::StoreBlob { digest, source });
        }
    }
    requests.push(AgentRequest::StoreActionResult {
        result: RemoteActionResult {
            action: action.clone(),
            metadata: Some(metadata.0),
            output_root: Some(root),
            version: 1,
        },
    });
    for response in session::request_agent(&requests)? {
        match response {
            AgentResponse::Stored { .. } | AgentResponse::ActionStored { .. } => {}
            AgentResponse::Error { message } => bail!(message),
            _ => bail!("cache agent returned an unexpected build-script store response"),
        }
    }
    Ok(())
}

fn stage_bytes(directory: &Path, name: &str, bytes: &[u8]) -> Result<(CacheDigest, PathBuf)> {
    let path = directory.join(name);
    std::fs::write(&path, bytes)?;
    Ok((CacheDigest::blake3(bytes), path))
}

fn store_directory(
    directory: &Path,
    staging: &Path,
    blobs: &mut Vec<(CacheDigest, PathBuf)>,
) -> Result<CacheDigest> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut symlinks = Vec::new();
    let mut entries = std::fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| eyre::eyre!("OUT_DIR entry name is not UTF-8"))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(path)?;
            if target.is_absolute()
                || target
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                bail!("OUT_DIR contains a symlink that may escape it");
            }
            symlinks.push(CacheSymlinkNode {
                name,
                mode: 0,
                target: target.to_string_lossy().into_owned(),
            });
        } else if metadata.is_dir() {
            directories.push(CacheDirectoryNode {
                name,
                mode: file_mode(&metadata),
                digest: store_directory(&path, staging, blobs)?,
            });
        } else if metadata.is_file() {
            let digest = CacheDigest::blake3_file(&path)?;
            blobs.push((digest.clone(), path));
            files.push(CacheFileNode {
                name,
                mode: file_mode(&metadata),
                executable: is_executable(&metadata),
                digest,
            });
        } else {
            bail!("OUT_DIR contains an unsupported entry");
        }
    }
    let bytes = canonical_json(&CacheDirectory {
        directories,
        files,
        symlinks,
        version: 1,
    })?;
    let name = format!("directory-{}", blobs.len());
    let staged = stage_bytes(staging, &name, &bytes)?;
    blobs.push(staged.clone());
    Ok(staged.0)
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}
#[cfg(not(unix))]
fn is_executable(_: &std::fs::Metadata) -> bool {
    false
}

fn restore(action: &CacheDigest, action_bytes: &[u8]) -> Result<Option<Restored>> {
    let responses = session::request_agent(&[AgentRequest::FindActionResult {
        action: action.clone(),
    }])?;
    let result = match responses.into_iter().next() {
        Some(AgentResponse::ActionResult {
            result: Some(result),
        }) => result,
        Some(AgentResponse::ActionResult { result: None }) => return Ok(None),
        Some(AgentResponse::Error { message }) => bail!(message),
        _ => bail!("cache agent returned an unexpected build-script lookup response"),
    };
    if result.version != 1 || result.action != *action {
        bail!("cached build-script result has an invalid identity");
    }
    let metadata_digest = result
        .metadata
        .ok_or_else(|| eyre::eyre!("cached build-script result has no metadata"))?;
    let root = result
        .output_root
        .ok_or_else(|| eyre::eyre!("cached build-script result has no output tree"))?;
    let roots = find_blobs(&[action.clone(), metadata_digest.clone()])?;
    if read_verified_blob(&roots[0], action, "build-script action")? != action_bytes {
        bail!("cached build-script action descriptor differs");
    }
    let metadata: Metadata =
        read_canonical_blob(&roots[1], &metadata_digest, "build-script metadata")?;
    if metadata.version != 1 || metadata.kind != ADAPTER {
        bail!("cached build-script metadata is unsupported");
    }
    let streams = find_blobs(&[metadata.stdout.clone(), metadata.stderr.clone()])?;
    let stdout = read_verified_blob(&streams[0], &metadata.stdout, "build-script stdout")?;
    let stderr = read_verified_blob(&streams[1], &metadata.stderr, "build-script stderr")?;
    let out_dir = PathBuf::from(
        std::env::var_os("OUT_DIR").ok_or_else(|| eyre::eyre!("build script has no OUT_DIR"))?,
    );
    let mappings = build_script_mappings();
    let stdout = denormalize_output_text(&stdout, &mappings);
    let stderr = denormalize_output_text(&stderr, &mappings);
    let parent = out_dir
        .parent()
        .ok_or_else(|| eyre::eyre!("OUT_DIR has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let staged = tempfile::tempdir_in(parent)?;
    let started = Instant::now();
    let (files, bytes) = restore_directory(&root, staged.path())?;
    if out_dir.exists() {
        std::fs::remove_dir_all(&out_dir)?;
    }
    std::fs::rename(staged.path(), &out_dir)?;
    let _ = staged.keep();
    Ok(Some(Restored {
        stats: RestoreStats {
            duration_ns: started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX),
            output_files: files,
            output_bytes: bytes,
            copied_output_files: files,
            copied_output_bytes: bytes,
            ..RestoreStats::default()
        },
        stdout,
        stderr,
    }))
}

/// Roots whose machine-specific spellings may appear in build-script state.
fn build_script_mappings() -> Vec<PathMapping> {
    build_script_mappings_with_env(|name| std::env::var_os(name).map(PathBuf::from))
}

/// Construct build-script mappings from an injectable environment lookup.
fn build_script_mappings_with_env(
    environment: impl Fn(&str) -> Option<PathBuf>,
) -> Vec<PathMapping> {
    let mut mappings = Vec::new();
    for (name, placeholder) in [
        ("OUT_DIR", "build_script_out_dir"),
        ("CARGO_MANIFEST_DIR", "build_script_manifest_dir"),
        (session::WORKSPACE_ROOT_ENV, "workspace"),
        (session::TARGET_DIR_ENV, "target"),
    ] {
        if let Some(root) = environment(name) {
            mappings.push(PathMapping::new(root, placeholder));
        }
    }
    let cargo_home = environment("CARGO_HOME").or_else(|| {
        ["HOME", "USERPROFILE"]
            .into_iter()
            .find_map(|name| environment(name).map(|home| home.join(".cargo")))
    });
    if let Some(cargo_home) = cargo_home {
        mappings.push(PathMapping::new(
            cargo_home.join("registry"),
            "cargo_registry",
        ));
        mappings.push(PathMapping::new(cargo_home, "cargo_home"));
    }
    mappings
}

fn normalize_environment_value(value: &str, mappings: &[PathMapping]) -> String {
    String::from_utf8(normalize_output_text(value.as_bytes(), mappings))
        .expect("normalizing UTF-8 paths preserves UTF-8")
}

/// Cargo-provided values that are implicit inputs to the build-script unit.
/// Cargo reruns the script when these change even though scripts do not emit
/// `rerun-if-env-changed` for them, so the execution key must do the same.
fn cargo_environment(mappings: &[PathMapping]) -> BTreeMap<String, Option<String>> {
    const FIXED: &[&str] = &[
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_LINKS",
        "CARGO_MANIFEST_PATH",
        "DEBUG",
        "HOST",
        "NUM_JOBS",
        "OPT_LEVEL",
        "PROFILE",
        "TARGET",
    ];
    let mut names = FIXED
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    names.extend(std::env::vars().filter_map(|(name, _)| {
        ["CARGO_CFG_", "CARGO_FEATURE_", "CARGO_PKG_", "DEP_"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
            .then_some(name)
    }));
    names
        .into_iter()
        .map(|name| {
            let value = std::env::var(&name)
                .ok()
                .map(|value| normalize_environment_value(&value, mappings));
            (name, value)
        })
        .collect()
}

fn restore_directory(digest: &CacheDigest, destination: &Path) -> Result<(u64, u64)> {
    let path = find_blobs(std::slice::from_ref(digest))?.remove(0);
    let directory: CacheDirectory =
        read_canonical_blob(&path, digest, "build-script output directory")?;
    if directory.version != 1 {
        bail!("cached build-script output directory is unsupported");
    }
    std::fs::create_dir_all(destination)?;
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    for node in directory.files {
        validate_name(&node.name)?;
        validate_file_mode(&node, node.executable)?;
        let source = find_blobs(std::slice::from_ref(&node.digest))?.remove(0);
        let target = destination.join(&node.name);
        if std::fs::copy(&source, &target)? != node.digest.size {
            bail!("restored build-script output has the wrong size");
        }
        apply_file_mode(&target, node.mode, node.executable)?;
        count = count.saturating_add(1);
        bytes = bytes.saturating_add(node.digest.size);
    }
    for node in directory.directories {
        validate_name(&node.name)?;
        let (child_count, child_bytes) =
            restore_directory(&node.digest, &destination.join(node.name))?;
        count = count.saturating_add(child_count);
        bytes = bytes.saturating_add(child_bytes);
    }
    for node in directory.symlinks {
        validate_name(&node.name)?;
        let target = Path::new(&node.target);
        if target.is_absolute()
            || target
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("cached build-script output has an unsafe symlink");
        }
        restore_symlink(&node.target, &destination.join(node.name))?;
    }
    Ok((count, bytes))
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || Path::new(name).components().count() != 1 {
        bail!("cached build-script output has an unsafe name");
    }
    Ok(())
}

#[cfg(unix)]
fn restore_symlink(target: &str, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}
#[cfg(not(unix))]
fn restore_symlink(_: &str, _: &Path) -> Result<()> {
    bail!("cached OUT_DIR symlinks are unsupported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn build_script_shim_keeps_the_compiled_binary_mtime() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory
            .path()
            .join("target/debug/build/fixture/build-script-build");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"compiled build script").unwrap();
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        filetime::set_file_mtime(&executable, filetime::FileTime::from_system_time(modified))
            .unwrap();

        install(&executable, &CacheDigest::blake3(b"action")).unwrap();

        assert_eq!(
            std::fs::metadata(&executable).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(
            std::fs::metadata(session::build_script_real_path(&executable))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
    }

    #[test]
    fn directives_are_deduplicated_and_both_cargo_spellings_are_accepted() {
        let parsed = parse_prediction(b"cargo:rerun-if-changed=src/a.h\ncargo::rerun-if-changed=src/a.h\ncargo:rerun-if-env-changed=MODE\n").unwrap().unwrap();
        assert!(!parsed.default_package);
        assert_eq!(parsed.inputs, ["src/a.h"]);
        assert_eq!(parsed.environment, ["MODE"]);
        let encoded = String::from_utf8(canonical_json(&parsed).unwrap()).unwrap();
        assert_eq!(decode_prediction(&encoded).unwrap(), parsed);
    }

    #[test]
    fn no_declared_inputs_use_cargos_package_default() {
        let parsed = parse_prediction(b"cargo:rustc-cfg=hello\n")
            .unwrap()
            .unwrap();
        assert!(parsed.default_package);
        assert_eq!(parsed.inputs, ["${build_script_manifest_dir}"]);
    }

    #[test]
    fn build_script_mappings_use_the_default_cargo_home() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let mappings = build_script_mappings_with_env(|name| match name {
            "HOME" => Some(home.clone()),
            _ => None,
        });
        let registry_input = home
            .join(".cargo")
            .join("registry")
            .join("src")
            .join("index")
            .join("widget-1.0.0")
            .join("src")
            .join("lib.rs");
        let registry_key = PathBuf::from("${cargo_registry}")
            .join("src")
            .join("index")
            .join("widget-1.0.0")
            .join("src")
            .join("lib.rs");

        assert_eq!(
            normalize_environment_value(&registry_input.to_string_lossy(), &mappings),
            registry_key.to_string_lossy()
        );
        let cargo_bin = home.join(".cargo").join("bin");
        assert_eq!(
            normalize_environment_value(&cargo_bin.to_string_lossy(), &mappings),
            PathBuf::from("${cargo_home}").join("bin").to_string_lossy()
        );
    }

    #[test]
    fn version_one_predictions_remain_usable() {
        let parsed = decode_prediction(
            r#"{"environment":["MODE"],"inputs":["input.h"],"portable_out_dir":true,"version":1}"#,
        )
        .unwrap();
        assert!(!parsed.default_package);
        assert_eq!(parsed.inputs, ["input.h"]);
        assert_eq!(parsed.environment, ["MODE"]);
        assert!(
            decode_prediction(
                r#"{"environment":[],"inputs":["input.h"],"portable_out_dir":true,"version":2}"#
            )
            .is_err(),
            "version 2 must carry its default-package mode"
        );
    }

    #[test]
    fn cargos_package_default_excludes_generated_and_vcs_trees() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("input"), "same").unwrap();
        std::fs::create_dir(directory.path().join("target")).unwrap();
        std::fs::create_dir(directory.path().join(".git")).unwrap();
        std::fs::write(directory.path().join("target/output"), "first").unwrap();
        std::fs::write(directory.path().join(".git/HEAD"), "first").unwrap();
        let first = package_input_state(directory.path(), &[]).unwrap();
        std::fs::write(directory.path().join("target/output"), "second").unwrap();
        std::fs::write(directory.path().join(".git/HEAD"), "second").unwrap();
        let second = package_input_state(directory.path(), &[]).unwrap();
        assert_eq!(first, second);
        std::fs::write(directory.path().join("input"), "changed").unwrap();
        let changed = package_input_state(directory.path(), &[]).unwrap();
        assert_ne!(second, changed);
    }

    #[test]
    fn an_empty_changed_path_bypasses_even_beside_other_inputs() {
        assert!(
            parse_prediction(
                b"cargo:rerun-if-changed=input.h\ncargo:rerun-if-changed=\ncargo:rerun-if-env-changed=MODE\n"
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn an_out_dir_changed_path_bypasses_execution_caching() {
        let directory = tempfile::tempdir().unwrap();
        let out_dir = directory.path().join("target/build/package/out");
        let directive = format!("cargo:rerun-if-changed={}/shadow.rs\n", out_dir.display());
        let mappings = [PathMapping::new(&out_dir, "build_script_out_dir")];

        assert!(
            parse_prediction_with_mappings(&directive, &mappings)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_input_includes_its_referent_contents() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let referent = directory.path().join("referent");
        let link = directory.path().join("input");
        std::fs::write(&referent, "first").unwrap();
        symlink("referent", &link).unwrap();
        let first = input_state(&link, &[]).unwrap();
        std::fs::write(referent, "second").unwrap();
        let second = input_state(&link, &[]).unwrap();
        assert_ne!(first, second);
    }
}
