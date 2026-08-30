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
    File { digest: CacheDigest },
    Directory { digest: CacheDigest },
    Symlink { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prediction {
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
    let real = session::build_script_real_path(executable);
    let temporary = real.with_extension("mbx-real-new");
    let _ = std::fs::remove_file(&temporary);
    std::fs::copy(executable, &temporary).wrap_err("failed to preserve the build script")?;
    let _ = std::fs::remove_file(&real);
    std::fs::rename(&temporary, &real)?;
    std::fs::write(
        build_script_action_path(&real),
        canonical_json(binary_action)?,
    )?;

    let mbx = std::env::current_exe().wrap_err("failed to locate the mbx shim")?;
    let _ = std::fs::remove_file(executable);
    let installed = std::fs::copy(&mbx, executable).map(|_| ());
    if let Err(error) = installed {
        let _ = std::fs::rename(&real, executable);
        return Err(error).wrap_err("failed to install the build-script shim");
    }
    Ok(())
}

/// Run the preserved program without consulting the cache.
pub(crate) fn run_real() -> ExitCode {
    let Some(invoked) = std::env::args_os().next().map(PathBuf::from) else {
        return ExitCode::FAILURE;
    };
    let Some(real) = session::find_build_script_real_path(&invoked) else {
        eprintln!("mbx[error]: the preserved build script is missing");
        return ExitCode::FAILURE;
    };
    let mut command = Command::new(real);
    command.args(std::env::args_os().skip(1));
    match command.status() {
        Ok(status) => crate::materialize::exit_code(status),
        Err(error) => {
            eprintln!("mbx[error]: failed to execute the build script: {error}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn run() -> Result<ExitCode> {
    let invoked = std::env::args_os()
        .next()
        .map(PathBuf::from)
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
    let output = command
        .output()
        .wrap_err("failed to execute the build script")?;
    replay_bytes(&output.stdout, &output.stderr)?;
    if !output.status.success() {
        return Ok(crate::materialize::exit_code(output.status));
    }

    let caching = (|| -> Result<()> {
        let Some(prediction) = parse_prediction(&output.stdout)? else {
            record_bypass("build-script-no-declared-inputs");
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
            let prediction: Prediction = serde_json::from_str(&found.payload)?;
            if prediction.version != 1 || canonical_json(&prediction)? != found.payload.as_bytes() {
                bail!("cached build-script prediction is unsupported");
            }
            Ok(Some(prediction))
        }
        Some(AgentResponse::ActionPrediction { prediction: None }) => Ok(None),
        Some(AgentResponse::Error { message }) => bail!(message),
        _ => bail!("cache agent returned an unexpected build-script prediction response"),
    }
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
    let mut inputs = BTreeSet::new();
    let mut environment = BTreeSet::new();
    for line in text.lines() {
        let directive = line
            .strip_prefix("cargo::")
            .or_else(|| line.strip_prefix("cargo:"));
        let Some(directive) = directive else { continue };
        if let Some(path) = directive.strip_prefix("rerun-if-changed=") {
            if !path.is_empty() {
                inputs.insert(path.to_string());
            }
        } else if let Some(name) = directive.strip_prefix("rerun-if-env-changed=")
            && !name.is_empty()
        {
            environment.insert(name.to_string());
        }
    }
    if inputs.is_empty() && environment.is_empty() {
        return Ok(None);
    }
    Ok(Some(Prediction {
        inputs: inputs.into_iter().collect(),
        environment: environment.into_iter().collect(),
        portable_out_dir: out_dir_is_portable()?,
        version: 1,
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
        let path = Path::new(declared);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            working_dir.join(path)
        };
        inputs.insert(declared.clone(), input_state(&resolved)?);
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

fn input_state(path: &Path) -> Result<InputState> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InputState::Missing);
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Ok(InputState::Symlink {
            target: std::fs::read_link(path)?.to_string_lossy().into_owned(),
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
            .map(|entry| {
                Ok((
                    entry.file_name().to_string_lossy().into_owned(),
                    input_state(&entry.path())?,
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
    let needle = out_dir.to_string_lossy();
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
            } else if kind.is_symlink()
                && std::fs::read_link(entry.path())?
                    .to_string_lossy()
                    .contains(needle.as_ref())
            {
                return Ok(false);
            }
        }
    }
    for path in walk_files(&root)? {
        if memchr::memmem::find(&std::fs::read(path)?, needle.as_bytes()).is_some() {
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

fn build_script_mappings() -> Vec<PathMapping> {
    let mut mappings = Vec::new();
    for (name, placeholder) in [
        ("OUT_DIR", "build_script_out_dir"),
        ("CARGO_MANIFEST_DIR", "build_script_manifest_dir"),
        (session::WORKSPACE_ROOT_ENV, "workspace"),
        (session::TARGET_DIR_ENV, "target"),
        ("CARGO_HOME", "cargo_home"),
    ] {
        if let Some(root) = std::env::var_os(name) {
            mappings.push(PathMapping::new(root, placeholder));
        }
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
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_PATH",
        "DEBUG",
        "HOST",
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

    #[test]
    fn directives_are_deduplicated_and_both_cargo_spellings_are_accepted() {
        let parsed = parse_prediction(b"cargo:rerun-if-changed=src/a.h\ncargo::rerun-if-changed=src/a.h\ncargo:rerun-if-env-changed=MODE\n").unwrap().unwrap();
        assert_eq!(parsed.inputs, ["src/a.h"]);
        assert_eq!(parsed.environment, ["MODE"]);
    }

    #[test]
    fn no_declared_inputs_bypasses_execution_caching() {
        assert!(
            parse_prediction(b"cargo:rustc-cfg=hello\n")
                .unwrap()
                .is_none()
        );
    }
}
