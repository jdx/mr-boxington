use crate::CacheDigest;
use serde::{Deserialize, Serialize};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

const DIGEST_BUFFER_BYTES: usize = 64 * 1024;
const TIMESTAMP_MACROS: &[&[u8]] = &[b"__DATE__", b"__TIME__", b"__TIMESTAMP__"];

#[cfg(target_os = "linux")]
const STATX_IDENTITY_MASK: u32 = 0x100 | 0x200 | 0x40 | 0x1000;

/// Stable Linux UAPI layout. `libc` omits statx on older musl headers even
/// though the kernel syscall and wire structure are available there.
#[cfg(target_os = "linux")]
#[repr(C)]
struct LinuxStatxTimestamp {
    seconds: i64,
    nanos: u32,
    reserved: i32,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct LinuxStatx {
    mask: u32,
    block_size: u32,
    attributes: u64,
    links: u32,
    uid: u32,
    gid: u32,
    mode: u16,
    reserved0: u16,
    inode: u64,
    size: u64,
    blocks: u64,
    attributes_mask: u64,
    accessed: LinuxStatxTimestamp,
    created: LinuxStatxTimestamp,
    changed: LinuxStatxTimestamp,
    modified: LinuxStatxTimestamp,
    rdev_major: u32,
    rdev_minor: u32,
    device_major: u32,
    device_minor: u32,
    mount_id: u64,
    direct_io_memory_alignment: u32,
    direct_io_offset_alignment: u32,
    subvolume: u64,
    atomic_write_unit_min: u32,
    atomic_write_unit_max: u32,
    atomic_write_segments_max: u32,
    direct_io_read_offset_alignment: u32,
    atomic_write_unit_max_opt: u32,
    reserved1: u32,
    reserved2: [u64; 8],
}

#[cfg(target_os = "linux")]
const _: () = assert!(std::mem::size_of::<LinuxStatx>() == 256);

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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    /// Stable object identity used when an NFS client's change time is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<FileObjectIdentity>,
}

/// The kernel identity of one file object on one mounted filesystem.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileObjectIdentity {
    /// Device major number reported by `statx`.
    pub device_major: u32,
    /// Device minor number reported by `statx`.
    pub device_minor: u32,
    /// Mount identifier in this mount namespace.
    pub mount_id: u64,
    /// Inode number within the mounted filesystem.
    pub inode: u64,
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
            object: None,
        })
    }

    /// Describe a file only when metadata can safely stand in for its digest.
    ///
    /// Linux NFS may revise cached change times without a write, so it uses a
    /// cached object/length/mtime identity that deliberately omits ctime. This
    /// is the same timestamp-freshness contract as Cargo and [`VerifiedBlob`];
    /// forcing a server round trip per input would serialize large NFS builds.
    /// If the kernel cannot supply every required field, callers hash instead.
    pub fn for_digest_cache(path: &Path, metadata: &std::fs::Metadata) -> io::Result<Option<Self>> {
        digest_cache_identity(
            path,
            metadata,
            metadata_identity_is_unreliable(path, metadata)?,
        )
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
        Ok(Self::for_digest_cache(&self.path, &metadata)?.as_ref() == Some(self))
    }

    /// Whether this identity is strong enough to stand in for a second read.
    pub fn can_skip_content_verification(&self) -> bool {
        self.changed.is_some() || self.object.is_some()
    }
}

/// A pre-operation snapshot that can prove whether a file's contents changed.
///
/// Most filesystems provide a stable metadata-change token, so the inexpensive
/// identity is sufficient. Linux NFS can reconcile the client and server
/// change times after a writer has closed the file, making two metadata reads
/// disagree without any intervening write. Those files carry a content digest
/// instead, while retaining length and modification time as an independent
/// signal for a write that restored the original bytes. Callers use the same
/// comparison either way and do not need to know which filesystem supplied the
/// file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    identity: FileIdentity,
    content: Option<CacheDigest>,
}

impl FileSnapshot {
    /// Capture the strongest comparison the file's filesystem can support.
    pub fn capture(path: &Path) -> io::Result<Option<Self>> {
        let metadata = std::fs::metadata(path)?;
        capture_file_snapshot(
            path,
            &NoFileDigestCache,
            metadata_identity_is_unreliable(path, &metadata)?,
            metadata,
        )
    }

    /// Capture a snapshot while reusing or publishing a content digest through
    /// the session ledger when the filesystem requires one.
    pub fn capture_with_cache(
        path: &Path,
        digests: &dyn FileDigestCache,
    ) -> io::Result<Option<Self>> {
        let metadata = std::fs::metadata(path)?;
        capture_file_snapshot(
            path,
            digests,
            metadata_identity_is_unreliable(path, &metadata)?,
            metadata,
        )
    }

    /// Whether `identity` and `content` still describe this snapshot.
    ///
    /// A content-backed snapshot deliberately ignores the unreliable change
    /// token, but still requires the modification time to remain stable. That
    /// prevents a write followed by restoration of the original bytes from
    /// passing only because the endpoint digests agree.
    pub fn matches(&self, identity: Option<&FileIdentity>, content: &CacheDigest) -> bool {
        self.content.as_ref().map_or_else(
            || identity == Some(&self.identity),
            |before| {
                before == content
                    && identity.is_some_and(|after| {
                        self.identity.path == after.path
                            && self.identity.len == after.len
                            && self.identity.modified == after.modified
                            && self.identity.object == after.object
                    })
            },
        )
    }

    /// Whether a mismatch proves the file's contents changed.
    pub fn proves_content_change(&self) -> bool {
        self.content.is_some() || self.identity.changed.is_some()
    }
}

impl From<FileIdentity> for FileSnapshot {
    fn from(identity: FileIdentity) -> Self {
        Self {
            identity,
            content: None,
        }
    }
}

#[cfg(target_os = "linux")]
fn metadata_identity_is_unreliable(path: &Path, metadata: &std::fs::Metadata) -> io::Result<bool> {
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::MetadataExt as _;

    static FILESYSTEMS: OnceLock<Mutex<std::collections::BTreeMap<u64, bool>>> = OnceLock::new();
    let filesystems = FILESYSTEMS.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()));
    if let Some(unreliable) = filesystems.lock().unwrap().get(&metadata.dev()).copied() {
        return Ok(unreliable);
    }

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
    let unreliable = status.f_type == 0x6969;
    filesystems
        .lock()
        .unwrap()
        .insert(metadata.dev(), unreliable);
    Ok(unreliable)
}

#[cfg(not(target_os = "linux"))]
fn metadata_identity_is_unreliable(
    _path: &Path,
    _metadata: &std::fs::Metadata,
) -> io::Result<bool> {
    Ok(false)
}

fn capture_file_snapshot(
    path: &Path,
    digests: &dyn FileDigestCache,
    content_identity: bool,
    metadata: std::fs::Metadata,
) -> io::Result<Option<FileSnapshot>> {
    let cache_identity = digest_cache_identity(path, &metadata, content_identity)?;
    let Some(identity) = cache_identity
        .clone()
        .or_else(|| FileIdentity::describe(path, &metadata))
    else {
        return Ok(None);
    };
    let content = if content_identity {
        let resolved = cache_identity
            .as_ref()
            .and_then(|identity| {
                digests
                    .resolve(FileDigestScope::Content, std::slice::from_ref(identity))
                    .pop()
            })
            .unwrap_or(FileDigestResolution::Unresolved);
        let (digest, fresh) = match resolved {
            FileDigestResolution::Digest(digest) => (digest, false),
            FileDigestResolution::EmbeddedTimestampMacro | FileDigestResolution::Unresolved => {
                let digest = digest_file(FileDigestScope::Content, path)?
                    .into_digest()
                    .ok_or_else(|| {
                        io::Error::other("content digest resolution returned no digest")
                    })?;
                (digest, true)
            }
        };
        if fresh
            && let Some(file) = cache_identity
            && file.len == digest.size
        {
            digests.record(
                FileDigestScope::Content,
                vec![RecordedFileDigest {
                    file,
                    digest: digest.clone(),
                }],
            );
        }
        Some(digest)
    } else {
        None
    };
    Ok(Some(FileSnapshot { identity, content }))
}

fn digest_cache_identity(
    path: &Path,
    metadata: &std::fs::Metadata,
    unreliable: bool,
) -> io::Result<Option<FileIdentity>> {
    if unreliable {
        nfs_file_identity(path)
    } else {
        Ok(FileIdentity::describe(path, metadata))
    }
}

#[cfg(target_os = "linux")]
fn nfs_file_identity(path: &Path) -> io::Result<Option<FileIdentity>> {
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt as _;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    let mut status = MaybeUninit::<LinuxStatx>::zeroed();
    // SAFETY: `path` is NUL-terminated and `status` names writable storage.
    let result = unsafe {
        libc::syscall(
            libc::SYS_statx,
            libc::AT_FDCWD,
            path.as_ptr(),
            // AT_STATX_DONT_SYNC: the surrounding build already trusts cached
            // mtimes, and FORCE_SYNC costs one NFS RPC for every dependency
            // edge rather than every distinct file.
            0x4000,
            STATX_IDENTITY_MASK,
            status.as_mut_ptr(),
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP) => Ok(None),
            _ => Err(error),
        };
    }
    // SAFETY: statx returned success and initialized `status`.
    let status = unsafe { status.assume_init() };
    nfs_identity_from_statx(
        Path::new(std::ffi::OsStr::from_bytes(path.to_bytes())),
        &status,
    )
}

#[cfg(target_os = "linux")]
fn nfs_identity_from_statx(path: &Path, status: &LinuxStatx) -> io::Result<Option<FileIdentity>> {
    if status.mask & STATX_IDENTITY_MASK != STATX_IDENTITY_MASK
        || status.modified.nanos >= 1_000_000_000
    {
        return Ok(None);
    }
    let modified = system_time(status.modified.seconds, status.modified.nanos)?;
    Ok(Some(FileIdentity {
        path: path.to_path_buf(),
        len: status.size,
        modified,
        changed: None,
        object: Some(FileObjectIdentity {
            device_major: status.device_major,
            device_minor: status.device_minor,
            mount_id: status.mount_id,
            inode: status.inode,
        }),
    }))
}

#[cfg(not(target_os = "linux"))]
fn nfs_file_identity(_path: &Path) -> io::Result<Option<FileIdentity>> {
    Ok(None)
}

#[cfg(target_os = "linux")]
fn system_time(seconds: i64, nanos: u32) -> io::Result<SystemTime> {
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::new(seconds as u64, nanos))
    } else {
        SystemTime::UNIX_EPOCH
            .checked_sub(std::time::Duration::from_secs(seconds.unsigned_abs()))
            .and_then(|time| time.checked_add(std::time::Duration::from_nanos(nanos.into())))
    }
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "file timestamp is out of range"))
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

/// The outcome of resolving one file under a digest scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileDigestResolution {
    /// The file's content digest, with any scope-specific checks complete.
    Digest(CacheDigest),
    /// A C/C++ input contains a time-dependent preprocessor macro.
    EmbeddedTimestampMacro,
    /// No shared resolver was available; the caller must read the file.
    Unresolved,
}

impl FileDigestResolution {
    /// Extract the digest when resolution succeeded.
    pub fn into_digest(self) -> Option<CacheDigest> {
        match self {
            Self::Digest(digest) => Some(digest),
            Self::EmbeddedTimestampMacro | Self::Unresolved => None,
        }
    }
}

/// Read and hash a file once, applying the checks required by `scope` in that
/// same pass.
pub fn digest_file(scope: FileDigestScope, path: &Path) -> io::Result<FileDigestResolution> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let longest_macro = TIMESTAMP_MACROS
        .iter()
        .map(|macro_name| macro_name.len())
        .max()
        .unwrap_or_default();
    let mut window = Vec::with_capacity(DIGEST_BUFFER_BYTES + longest_macro);
    let mut chunk = vec![0_u8; DIGEST_BUFFER_BYTES];
    let mut found_timestamp_macro = false;
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("file length overflowed u64"))?;
        if scope == FileDigestScope::CcInput && !found_timestamp_macro {
            window.extend_from_slice(&chunk[..read]);
            found_timestamp_macro = TIMESTAMP_MACROS
                .iter()
                .any(|macro_name| contains_subslice(&window, macro_name));
            let keep = window.len().saturating_sub(longest_macro.saturating_sub(1));
            window.drain(..keep);
        }
    }
    if found_timestamp_macro {
        Ok(FileDigestResolution::EmbeddedTimestampMacro)
    } else {
        Ok(FileDigestResolution::Digest(CacheDigest {
            algorithm: "blake3".into(),
            hash: hasher.finalize().to_hex().to_string(),
            size,
        }))
    }
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Recorded digests a session may consult instead of rehashing a file.
///
/// The agent's file-digest ledger answers through this everywhere a caller
/// holds one; the no-op implementation stands in where reuse must not happen,
/// such as under verification.
pub trait FileDigestCache: Send + Sync {
    /// Resolve these identities, coalescing concurrent misses where supported.
    fn resolve(&self, scope: FileDigestScope, files: &[FileIdentity]) -> Vec<FileDigestResolution> {
        self.find(scope, files)
            .into_iter()
            .map(|digest| {
                digest.map_or(
                    FileDigestResolution::Unresolved,
                    FileDigestResolution::Digest,
                )
            })
            .collect()
    }
    /// Recorded digests for these identities, in request order.
    fn find(&self, scope: FileDigestScope, files: &[FileIdentity]) -> Vec<Option<CacheDigest>>;
    /// Record digests for files read under these identities.
    fn record(&self, scope: FileDigestScope, entries: Vec<RecordedFileDigest>);
}

/// A [`FileDigestCache`] that remembers nothing and finds nothing.
pub struct NoFileDigestCache;

impl FileDigestCache for NoFileDigestCache {
    fn resolve(
        &self,
        _scope: FileDigestScope,
        files: &[FileIdentity],
    ) -> Vec<FileDigestResolution> {
        vec![FileDigestResolution::Unresolved; files.len()]
    }

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
        let snapshot = capture_file_snapshot(
            &path,
            &NoFileDigestCache,
            false,
            std::fs::metadata(&path).unwrap(),
        )
        .unwrap()
        .unwrap();

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
    fn a_content_snapshot_ignores_change_token_churn_but_detects_other_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.rs");
        std::fs::write(&path, b"fn main() {}").unwrap();
        let snapshot = capture_file_snapshot(
            &path,
            &NoFileDigestCache,
            true,
            std::fs::metadata(&path).unwrap(),
        )
        .unwrap()
        .unwrap();

        let mut identity = snapshot.identity.clone();
        identity.changed = identity
            .changed
            .map(|(seconds, nanos)| (seconds + 1, nanos));
        let digest = CacheDigest::blake3_file(&path).unwrap();
        assert!(snapshot.matches(Some(&identity), &digest));
        assert!(snapshot.proves_content_change());

        identity.modified = SystemTime::UNIX_EPOCH;
        assert!(!snapshot.matches(Some(&identity), &digest));

        std::fs::write(&path, b"fn main(){ }").unwrap();
        let identity = FileIdentity::describe(&path, &std::fs::metadata(&path).unwrap());
        let digest = CacheDigest::blake3_file(&path).unwrap();
        assert!(!snapshot.matches(identity.as_ref(), &digest));
    }

    #[test]
    fn reliable_metadata_keeps_the_native_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.rs");
        std::fs::write(&path, b"fn main() {}").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();

        let identity = digest_cache_identity(&path, &metadata, false)
            .unwrap()
            .unwrap();

        assert_eq!(identity.path, path);
        assert_eq!(identity.len, 12);
        assert!(identity.object.is_none());
    }

    #[test]
    fn content_snapshot_ignores_nfs_ctime_churn_but_not_object_replacement() {
        let digest = CacheDigest::blake3(b"nfs bytes");
        let identity = FileIdentity {
            path: PathBuf::from("/nfs/input.rlib"),
            len: digest.size,
            modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10),
            changed: Some((10, 1)),
            object: Some(FileObjectIdentity {
                device_major: 0,
                device_minor: 42,
                mount_id: 7,
                inode: 99,
            }),
        };
        let snapshot = FileSnapshot {
            identity: identity.clone(),
            content: Some(digest.clone()),
        };
        let mut after = identity;
        after.changed = Some((9, 500));
        assert!(snapshot.matches(Some(&after), &digest));

        after.object.as_mut().unwrap().inode += 1;
        assert!(!snapshot.matches(Some(&after), &digest));
        after.object.as_mut().unwrap().inode -= 1;
        after.modified += std::time::Duration::from_secs(1);
        assert!(!snapshot.matches(Some(&after), &digest));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn incomplete_statx_metadata_falls_back_to_content_hashing() {
        // SAFETY: all-zero is a valid unpopulated statx result for this parser.
        let status = unsafe { std::mem::zeroed::<LinuxStatx>() };
        assert!(
            nfs_identity_from_statx(Path::new("/nfs/input.rlib"), &status)
                .unwrap()
                .is_none()
        );
    }
}
