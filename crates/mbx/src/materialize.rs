//! Cache machinery shared by every compiler adapter.
//!
//! Restoring an action result is the same problem whatever produced it: verify
//! blobs against their digests, stage them beside their destination, and rename
//! them into place or roll the whole set back. Only deciding *which* files an
//! action should produce is adapter-specific, so that part stays with each
//! adapter and everything here is driven by a plain file list.

use crate::session::{self, STAGING_ENV};
use eyre::{Result, WrapErr as _, bail};
use mbx_cache_core::{
    ActionDiagnostic, AgentRequest, AgentResponse, CacheDigest, CacheFileNode, RestoreStats,
    canonical_json,
};
use mbx_cache_rustc::PathMapping;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::ffi::OsStr;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, ExitStatus};

/// A compilation reconstructed from the cache.
pub(crate) struct CachedCompilation {
    pub(crate) action: CacheDigest,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) outputs: Vec<CachedOutput>,
    pub(crate) restore: RestoreStats,
}

/// One restored file and the properties it must have on disk.
pub(crate) struct CachedOutput {
    pub(crate) path: PathBuf,
    pub(crate) digest: CacheDigest,
    pub(crate) executable: bool,
    pub(crate) mode: u32,
}

/// Restored files waiting to be renamed into place.
pub(crate) struct StagedOutputs {
    /// Kept alive so the staging directory outlives the files inside it.
    pub(crate) directory: tempfile::TempDir,
    pub(crate) files: Vec<(tempfile::TempPath, PathBuf)>,
}

/// How a cached output reached its staging path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Materialization {
    Reflink,
    Copy,
}

/// Look up every blob at once, failing if the action is missing any of them.
pub(crate) fn find_blobs(digests: &[CacheDigest]) -> Result<Vec<PathBuf>> {
    let responses = session::request_agent(&[AgentRequest::FindBlobs {
        digests: digests.to_vec(),
    }])?;
    let Some(response) = responses.into_iter().next() else {
        bail!("cache agent did not return a blob lookup response");
    };
    match response {
        AgentResponse::Blobs { paths } if paths.len() == digests.len() => paths
            .into_iter()
            .zip(digests)
            .map(|(path, digest)| match path {
                Some(path) => Ok(path),
                None => bail!("cached action is missing blob {}", digest.hash),
            })
            .collect(),
        AgentResponse::Blobs { .. } => {
            bail!("cache agent returned an incomplete blob lookup response")
        }
        AgentResponse::Blob { path: Some(path) } if digests.len() == 1 => Ok(vec![path]),
        AgentResponse::Blob { path: None } if digests.len() == 1 => {
            let digest = &digests[0];
            bail!("cached action is missing blob {}", digest.hash)
        }
        AgentResponse::Error { message } => bail!(message),
        _ => bail!("cache agent returned an unexpected blob lookup response"),
    }
}

/// Read a JSON record, requiring the exact canonical bytes its digest names.
pub(crate) fn read_canonical_blob<T>(
    path: &Path,
    digest: &CacheDigest,
    description: &str,
) -> Result<T>
where
    T: DeserializeOwned + Serialize,
{
    let bytes = read_verified_blob(path, digest, description)?;
    let value = serde_json::from_slice(&bytes)
        .wrap_err_with(|| format!("cached {description} is not valid JSON"))?;
    if canonical_json(&value)? != bytes {
        bail!("cached {description} is not canonical JSON");
    }
    Ok(value)
}

/// Read a blob and confirm it hashes to the digest that named it.
pub(crate) fn read_verified_blob(
    path: &Path,
    digest: &CacheDigest,
    description: &str,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    if !digest.matches_bytes(&bytes)? {
        bail!("cached {description} failed digest verification");
    }
    Ok(bytes)
}

/// Clone a verified cache object into a staging directory beside its
/// destination.
pub(crate) fn stage_verified_cached_output(
    directory: &Path,
    index: usize,
    source: &Path,
    node: &CacheFileNode,
) -> Result<(tempfile::TempPath, Materialization)> {
    let temporary = directory.join(format!("output-{index}"));
    let copied_bytes = reflink_copy::reflink_or_copy(source, &temporary)
        .wrap_err_with(|| format!("failed to materialize cached output {}", node.name))?;
    let materialization = match copied_bytes {
        None => Materialization::Reflink,
        Some(written) if written == node.digest.size => Materialization::Copy,
        Some(_) => bail!(
            "materialized cached output has the wrong size: {}",
            node.name
        ),
    };
    let temporary = tempfile::TempPath::try_from_path(temporary)?;
    make_owner_writable(&temporary)?;
    // Deliberately not fsynced. These are build artifacts in a target
    // directory, and cargo does not sync its own outputs either, so syncing
    // here buys no durability the build relies on -- it only costs one fsync
    // per restored file, which on a large workspace is most of the restore.
    // `source` is a session-verified CAS path returned by `FindBlobs`. Hashing
    // the result again would read every output a second time and, for a
    // reflink, eagerly fault the shared data blocks that cloning was intended
    // to leave deferred. A reflink is a CoW snapshot and the copy fallback
    // reports the number of bytes it wrote, so checking the staged length is
    // sufficient after the agent's content verification.
    if std::fs::metadata(&temporary)?.len() != node.digest.size {
        bail!(
            "materialized cached output has the wrong size: {}",
            node.name
        );
    }
    apply_file_mode(&temporary, node.mode, node.executable)?;
    Ok((temporary, materialization))
}

/// Rename every staged output into place, rolling the whole set back if any one
/// of them fails.
pub(crate) fn persist_outputs(staged: StagedOutputs) -> Result<()> {
    let StagedOutputs {
        directory: _directory,
        files,
    } = staged;
    let destinations = files
        .iter()
        .map(|(_, destination)| destination.clone())
        .collect::<Vec<_>>();
    for (temporary, destination) in files {
        let persisted = temporary
            .persist(&destination)
            .map_err(|error| error.error)
            .wrap_err_with(|| format!("failed to atomically restore {}", destination.display()));
        if let Err(error) = persisted {
            for destination in &destinations {
                match std::fs::remove_file(destination) {
                    Ok(()) => {}
                    Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(remove_error) => session::report_shim_warning(&format!(
                        "failed to roll back {}: {remove_error}",
                        destination.display()
                    )),
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

/// Replay a cached compilation's diagnostics so a hit looks like a compile.
pub(crate) fn replay_bytes(stdout_bytes: &[u8], stderr_bytes: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(stdout_bytes)?;
    stdout.flush()?;
    let mut stderr = std::io::stderr().lock();
    stderr.write_all(stderr_bytes)?;
    stderr.flush()?;
    Ok(())
}

/// A directory to assemble blobs in before publishing them.
pub(crate) fn staging_directory() -> Result<tempfile::TempDir> {
    let root = match std::env::var_os(STAGING_ENV).filter(|root| !root.is_empty()) {
        Some(root) => PathBuf::from(root),
        None => crate::config::Config::load()?
            .store_dir()
            .join("standalone-staging"),
    };
    std::fs::create_dir_all(&root)?;
    Ok(tempfile::tempdir_in(root)?)
}

/// Resolve a compiler to an absolute path, so its identity does not depend on
/// how `PATH` happened to be spelled.
pub(crate) fn resolve_executable(executable: &OsStr) -> Result<PathBuf> {
    let executable = PathBuf::from(executable);
    if executable.is_absolute() {
        return Ok(executable);
    }
    which::which(&executable).wrap_err_with(|| {
        format!(
            "failed to resolve compiler executable {}",
            executable.display()
        )
    })
}

/// Tell the session a cached result was used.
pub(crate) fn record_action_hit(action: &CacheDigest, restore: RestoreStats, crate_name: &str) {
    record_action_hit_with_diagnostic(action, restore, crate_name, None);
}

pub(crate) fn record_action_hit_with_diagnostic(
    action: &CacheDigest,
    restore: RestoreStats,
    crate_name: &str,
    diagnostic: Option<ActionDiagnostic>,
) {
    let responses = session::request_agent(&[AgentRequest::RecordActionHit {
        action: action.clone(),
        restore,
        crate_name: Some(crate_name.to_string()),
        diagnostic,
    }]);
    match responses.map(|responses| responses.into_iter().next()) {
        Ok(Some(AgentResponse::ActionHitRecorded)) => {}
        Ok(Some(AgentResponse::Error { message })) => {
            session::report_shim_warning(&format!("hit was not recorded: {message}"));
        }
        Ok(_) => session::report_shim_warning("hit was not recorded"),
        Err(error) => {
            session::report_shim_warning(&format!("hit was not recorded: {error:#}"));
        }
    }
}

/// Tell the session whether a shadow compilation agreed with the cache.
pub(crate) fn record_verification(matched: bool, restore: RestoreStats) {
    let responses =
        session::request_agent(&[AgentRequest::RecordActionVerification { matched, restore }]);
    match responses.map(|responses| responses.into_iter().next()) {
        Ok(Some(AgentResponse::ActionVerificationRecorded)) => {}
        Ok(Some(AgentResponse::Error { message })) => {
            session::report_shim_warning(&format!("verification was not recorded: {message}"));
        }
        Ok(_) => session::report_shim_warning("verification was not recorded"),
        Err(error) => {
            session::report_shim_warning(&format!("verification was not recorded: {error:#}"));
        }
    }
}

#[cfg(unix)]
pub(crate) fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o644
}

#[cfg(windows)]
pub(crate) fn file_mode(_metadata: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
pub(crate) fn validate_file_mode(node: &CacheFileNode, executable: bool) -> Result<()> {
    if node.executable != executable
        || node.mode & !0o777 != 0
        || node.mode & 0o111 != 0
        || node.mode & 0o022 != 0
    {
        bail!("cached output has an unsafe file mode: {}", node.name);
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn validate_file_mode(node: &CacheFileNode, executable: bool) -> Result<()> {
    if node.executable != executable || node.mode != 0 {
        bail!("cached output has an unsafe file mode: {}", node.name);
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn apply_file_mode(temporary: &Path, mode: u32, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let executable_mode = if executable { 0o111 } else { 0 };
    std::fs::set_permissions(
        temporary,
        std::fs::Permissions::from_mode(mode | executable_mode),
    )?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn apply_file_mode(_temporary: &Path, _mode: u32, _executable: bool) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn executable_mode_matches(metadata: &std::fs::Metadata, executable: bool) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    (metadata.permissions().mode() & 0o111 != 0) == executable
}

#[cfg(windows)]
pub(crate) fn executable_mode_matches(_metadata: &std::fs::Metadata, _executable: bool) -> bool {
    true
}

#[cfg(unix)]
pub(crate) fn make_owner_writable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o200);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn make_owner_writable(path: &Path) -> Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

/// Rewrite this machine's paths in a cached text output into the placeholders
/// the action key already uses.
///
/// Two outputs of a compilation are text that names where the compilation ran:
/// the dep-info file, whose rules are keyed by absolute output paths, and the
/// compiler's stderr, which carries an artifact notification per emitted file
/// when cargo asks for one. Both are the same compilation wherever it runs, but
/// neither is byte-identical across target directories, so storing them
/// verbatim means a restore hands the next checkout paths belonging to the one
/// that published them.
///
/// Only these two are rewritten. Compiled artifacts are opaque bytes that do
/// not carry the target directory, and rewriting inside them would be a
/// corruption rather than a translation.
pub(crate) fn normalize_output_text(bytes: &[u8], mappings: &[PathMapping]) -> Vec<u8> {
    let mut normalized = bytes.to_vec();
    for (root, placeholder) in root_spellings(mappings) {
        normalized = replace_bytes(&normalized, root.as_bytes(), placeholder.as_bytes());
    }
    normalized
}

/// Rewrite placeholders in a cached text output back into this machine's paths.
pub(crate) fn denormalize_output_text(bytes: &[u8], mappings: &[PathMapping]) -> Vec<u8> {
    let mut text = bytes.to_vec();
    for (root, placeholder) in root_spellings(mappings) {
        text = replace_bytes(&text, placeholder.as_bytes(), root.as_bytes());
    }
    text
}

/// Every spelling a root can take in these outputs, paired with the
/// placeholder that stands for it.
///
/// rustc writes a path with the platform separator in some places and forward
/// slashes in others -- the reason [`carries`] searches both -- and stderr is
/// JSON, where a Windows separator arrives doubled. A spelling missed here is
/// a path from the publishing checkout left in place, so each is looked for,
/// and each gets its own placeholder because a restore has to know which one
/// to write back.
///
/// Deepest root first, so a target directory inside a workspace wins over the
/// workspace, and within a root the doubled spelling before the single one.
fn root_spellings(mappings: &[PathMapping]) -> Vec<(String, String)> {
    let mut spellings = Vec::new();
    for mapping in PathMapping::ordered(mappings) {
        let Some(root) = mapping.root.to_str() else {
            continue;
        };
        // A root arrives however its environment variable was written, and
        // `CARGO_TARGET_DIR=/work/target/` is as valid as the same path
        // without. Trailing separators are dropped so the boundary check sees
        // the separator before a child rather than the child's first letter,
        // which would otherwise reject every path under such a root and leave
        // it in the publishing checkout's spelling. A Windows drive root like
        // `C:\` trims to `C:`, which its own separator then follows.
        let root = root.trim_end_matches(['/', '\\']);
        if root.is_empty() {
            continue;
        }
        // The literal spelling first, so a platform whose separator needs no
        // escaping keeps the plain placeholder rather than an escaped one that
        // means the same thing.
        for (spelling, suffix) in [
            (root.to_string(), ""),
            (root.replace('\\', "\\\\"), ":escaped"),
            (root.replace('\\', "/"), ":slash"),
        ] {
            if spellings.iter().any(|(existing, _)| existing == &spelling) {
                continue;
            }
            spellings.push((spelling, format!("${{{}{suffix}}}", mapping.placeholder)));
        }
    }
    spellings
}

/// Whether a root matched at `end` stops there rather than running into a
/// longer name.
///
/// `/work/target` is not a prefix of `/work/target-backup` in any sense that
/// matters, but a plain substring search cannot tell them apart, and rewriting
/// the second would hand a restore a directory that never existed.
/// [`normalize_mapped_path`] gets this from comparing components; here the
/// following byte has to say it.
fn ends_at_boundary(haystack: &[u8], end: usize) -> bool {
    match haystack.get(end) {
        None => true,
        Some(byte) => !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
    }
}

fn replace_bytes(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut index = 0;
    while index <= haystack.len() - needle.len() {
        if &haystack[index..index + needle.len()] == needle
            && ends_at_boundary(haystack, index + needle.len())
        {
            out.extend_from_slice(replacement);
            index += needle.len();
        } else {
            out.push(haystack[index]);
            index += 1;
        }
    }
    out.extend_from_slice(&haystack[index..]);
    out
}

/// Reproduce a compiler's exit status as this process's own.
///
/// A shim that captured the compiler's output has to hand the status back
/// itself, and a compiler killed by a signal reports no exit code at all --
/// reporting 1 there would turn a crash into an ordinary failure.
#[cfg(unix)]
pub(crate) fn exit_code(status: ExitStatus) -> ExitCode {
    use std::os::unix::process::ExitStatusExt as _;
    ExitCode::from(
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)) as u8,
    )
}

#[cfg(windows)]
pub(crate) fn exit_code(status: ExitStatus) -> ExitCode {
    // SAFETY: this process is only a compiler wrapper and must preserve the
    // compiler's full Windows status code, which stable ExitCode cannot hold.
    unsafe { windows_sys::Win32::System::Threading::ExitProcess(status.code().unwrap_or(1) as u32) }
}
