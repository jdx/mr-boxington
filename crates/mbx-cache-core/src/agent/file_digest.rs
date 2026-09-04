use crate::CacheDigest;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The on-disk identity a recorded file digest describes.
///
/// The same trade [`VerifiedBlob`] makes for CAS reads, offered to shims for
/// the files they hash: an overwrite moves the modification time and a
/// truncation changes the length, so a digest recorded against both stands
/// until either does. Where the platform reports a metadata-change time the
/// identity carries that too, and it is the part a writer cannot restore: a
/// rewrite that puts the modification time back still moves the change time,
/// so only filesystems without one fall back to the freshness model the
/// surrounding build tool already lives on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Length of the file in bytes.
    pub len: u64,
    /// Modification time of the file.
    pub modified: SystemTime,
    /// Platform metadata-change token, where one exists.
    pub changed: Option<(i64, i64)>,
}

impl FileIdentity {
    /// Describe a file from metadata already in hand, or nothing when the
    /// filesystem reports no modification time to compare against later.
    pub fn describe(path: &Path, metadata: &std::fs::Metadata) -> Option<Self> {
        Some(Self {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            changed: change_token(metadata),
        })
    }

    /// Describe a file only when metadata can safely stand in for its digest.
    ///
    /// Linux NFS may revise cached timestamps without a write and may delay a
    /// real writer's final timestamps. Returning no identity makes digest
    /// users hash the file instead of trusting that ambiguous metadata.
    pub fn for_digest_cache(path: &Path, metadata: &std::fs::Metadata) -> io::Result<Option<Self>> {
        Ok(digest_cache_identity(
            path,
            metadata,
            metadata_identity_is_unreliable(path)?,
        ))
    }

    /// Whether the file at this identity's path still has exactly this
    /// identity, so the digest recorded against it still describes the bytes on
    /// disk without reading them again.
    ///
    /// Length alone would miss an overwrite that keeps the size, and the
    /// modification time can be put back by whoever rewrote the file. The
    /// change time cannot be set from user space, so where the platform reports
    /// one a rewrite that restores the old modification time still shows. A
    /// file that has vanished is an error rather than a change, so the caller
    /// can tell the two apart.
    pub fn still_describes(&self) -> std::io::Result<bool> {
        let metadata = std::fs::metadata(&self.path)?;
        Ok(Self::describe(&self.path, &metadata).as_ref() == Some(self))
    }
}

/// A pre-operation snapshot that can prove whether a file's contents changed.
///
/// Most filesystems provide a stable metadata-change token, so the inexpensive
/// identity is sufficient. Linux NFS can reconcile client and server
/// timestamps after a writer has closed the file, making two metadata reads
/// disagree without any intervening write. Those files carry a content digest
/// instead. Callers use the same comparison either way and do not need to know
/// which filesystem supplied the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    identity: FileIdentity,
    content: Option<CacheDigest>,
}

impl FileSnapshot {
    /// Capture the strongest comparison the file's filesystem can support.
    pub fn capture(path: &Path) -> io::Result<Option<Self>> {
        capture_file_snapshot(path, metadata_identity_is_unreliable(path)?)
    }

    /// Whether `identity` and `content` still describe this snapshot.
    pub fn matches(&self, identity: Option<&FileIdentity>, content: &CacheDigest) -> bool {
        self.content
            .as_ref()
            .map_or(identity == Some(&self.identity), |before| before == content)
    }

    /// Whether a mismatch proves the file's contents changed.
    pub fn proves_content_change(&self) -> bool {
        self.content.is_some() || self.identity.changed.is_some()
    }
}

#[cfg(target_os = "linux")]
fn metadata_identity_is_unreliable(path: &Path) -> io::Result<bool> {
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt as _;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    let mut status = MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: `path` is NUL-terminated and `status` points to writable,
    // correctly sized storage. statfs initializes it before returning success.
    let result = unsafe { libc::statfs(path.as_ptr(), status.as_mut_ptr()) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statfs returned success and initialized `status`.
    let status = unsafe { status.assume_init() };
    // libc gives NFS_SUPER_MAGIC a different signedness from statfs::f_type on musl.
    Ok(status.f_type == 0x6969)
}

#[cfg(not(target_os = "linux"))]
fn metadata_identity_is_unreliable(_path: &Path) -> io::Result<bool> {
    Ok(false)
}

fn capture_file_snapshot(path: &Path, content_identity: bool) -> io::Result<Option<FileSnapshot>> {
    let metadata = std::fs::metadata(path)?;
    let Some(identity) = FileIdentity::describe(path, &metadata) else {
        return Ok(None);
    };
    let content = content_identity
        .then(|| {
            CacheDigest::blake3_file(path).map_err(|error| io::Error::other(error.to_string()))
        })
        .transpose()?;
    Ok(Some(FileSnapshot { identity, content }))
}

fn digest_cache_identity(
    path: &Path,
    metadata: &std::fs::Metadata,
    unreliable: bool,
) -> Option<FileIdentity> {
    if unreliable {
        None
    } else {
        FileIdentity::describe(path, metadata)
    }
}

/// The metadata-change time as an opaque token, where the platform has one.
#[cfg(unix)]
fn change_token(metadata: &std::fs::Metadata) -> Option<(i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.ctime(), metadata.ctime_nsec()))
}

/// Windows reports creation rather than metadata-change time, which a rewrite
/// does not move, so no token is better than a misleading one.
#[cfg(not(unix))]
fn change_token(_metadata: &std::fs::Metadata) -> Option<(i64, i64)> {
    None
}

/// A file digest recorded against the identity it was read under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedFileDigest {
    /// Identity of the file when its contents were hashed.
    pub file: FileIdentity,
    /// Digest of those contents.
    pub digest: CacheDigest,
}

/// What a recorded file digest may stand in for.
///
/// Adapters prove different things when they read a file: the cc adapter's
/// input scan also establishes that a source embeds no timestamp macro, which
/// a digest recorded by the rustc adapter never checked. Scoping the ledger
/// keeps one adapter's shortcut from resting on a property another adapter
/// never established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileDigestScope {
    /// The digest describes the file's contents and nothing more.
    Content,
    /// The digest describes a cc compiler input that also passed the
    /// timestamp-macro scan.
    CcInput,
}

/// Recorded digests a session may consult instead of rehashing a file.
///
/// The agent's file-digest ledger answers through this everywhere a caller
/// holds one; the no-op implementation stands in where reuse must not happen,
/// such as under verification.
pub trait FileDigestCache: Send + Sync {
    /// Recorded digests for these identities, in request order.
    fn find(&self, scope: FileDigestScope, files: &[FileIdentity]) -> Vec<Option<CacheDigest>>;
    /// Record digests for files read under these identities.
    fn record(&self, scope: FileDigestScope, entries: Vec<RecordedFileDigest>);
}

/// A [`FileDigestCache`] that remembers nothing and finds nothing.
pub struct NoFileDigestCache;

impl FileDigestCache for NoFileDigestCache {
    fn find(&self, _scope: FileDigestScope, files: &[FileIdentity]) -> Vec<Option<CacheDigest>> {
        vec![None; files.len()]
    }

    fn record(&self, _scope: FileDigestScope, _entries: Vec<RecordedFileDigest>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_describes_the_file_until_it_is_written_or_removed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.rs");
        std::fs::write(&path, b"fn main() {}").unwrap();
        let identity = FileIdentity::describe(&path, &std::fs::metadata(&path).unwrap()).unwrap();
        assert!(identity.still_describes().unwrap());

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"fn main() { }").unwrap();
        assert!(!identity.still_describes().unwrap());

        std::fs::remove_file(&path).unwrap();
        assert!(identity.still_describes().is_err());
    }

    #[test]
    fn a_metadata_snapshot_detects_a_metadata_change() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.rs");
        std::fs::write(&path, b"fn main() {}").unwrap();
        let snapshot = capture_file_snapshot(&path, false).unwrap().unwrap();

        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
        let identity = FileIdentity::describe(&path, &std::fs::metadata(&path).unwrap());
        let digest = CacheDigest::blake3_file(&path).unwrap();

        assert!(!snapshot.matches(identity.as_ref(), &digest));
        assert_eq!(snapshot.proves_content_change(), cfg!(unix));
    }

    #[test]
    fn a_content_snapshot_ignores_metadata_churn_but_detects_changed_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.rs");
        std::fs::write(&path, b"fn main() {}").unwrap();
        let snapshot = capture_file_snapshot(&path, true).unwrap().unwrap();

        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
        let identity = FileIdentity::describe(&path, &std::fs::metadata(&path).unwrap());
        let digest = CacheDigest::blake3_file(&path).unwrap();
        assert!(snapshot.matches(identity.as_ref(), &digest));
        assert!(snapshot.proves_content_change());

        std::fs::write(&path, b"fn main(){ }").unwrap();
        let identity = FileIdentity::describe(&path, &std::fs::metadata(&path).unwrap());
        let digest = CacheDigest::blake3_file(&path).unwrap();
        assert!(!snapshot.matches(identity.as_ref(), &digest));
    }

    #[test]
    fn unreliable_metadata_is_not_used_as_a_digest_cache_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.rs");
        std::fs::write(&path, b"fn main() {}").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();

        let identity = digest_cache_identity(&path, &metadata, false).unwrap();

        assert_eq!(identity.path, path);
        assert_eq!(identity.len, 12);
        assert!(digest_cache_identity(&identity.path, &metadata, true).is_none());
    }
}
