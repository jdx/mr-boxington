//! Disk usage counted once per file rather than once per path.
//!
//! Hard links put one file at several paths: incremental sessions share
//! immutable files, a seeded target directory links read-only outputs from
//! the view it was seeded from, and on Linux a restored output can be a link
//! to the store object it came from. Summing the length at every path counts
//! such a file once per link. A [`Ledger`] counts each file once however many
//! measured paths reach it, and says what deleting some of those paths frees.
//!
//! Two figures come out of a deletion. The logical one is the length of every
//! file none of the ledger's measured paths reach any more, which is what the
//! measured total drops by. The physical one counts only files whose last link
//! anywhere went, which is what the disk gets back: a file still linked from
//! the store, or from anything else the ledger never measured, stays on disk.
//! Clones and reflinks share blocks without sharing a file, and no stable
//! interface says so, so both figures count those in full.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One file, however many paths reach it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FileKey {
    Object(u64, u64),
    #[cfg(not(unix))]
    Path(PathBuf),
}

/// The file at `path` (built only where the key needs it), and how many links
/// it has.
#[cfg(unix)]
fn file_key(_path: impl FnOnce() -> PathBuf, metadata: &std::fs::Metadata) -> (FileKey, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (
        FileKey::Object(metadata.dev(), metadata.ino()),
        metadata.nlink(),
    )
}

#[cfg(windows)]
fn file_key(path: impl FnOnce() -> PathBuf, _metadata: &std::fs::Metadata) -> (FileKey, u64) {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let path = path();
    if let Ok(file) = std::fs::File::open(&path) {
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // The handle stays open and the output points to a correctly sized buffer.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } != 0 {
            let info = unsafe { info.assume_init() };
            return (
                FileKey::Object(
                    u64::from(info.dwVolumeSerialNumber),
                    (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
                ),
                u64::from(info.nNumberOfLinks),
            );
        }
    }
    (FileKey::Path(path), 1)
}

#[cfg(not(any(unix, windows)))]
fn file_key(path: impl FnOnce() -> PathBuf, _metadata: &std::fs::Metadata) -> (FileKey, u64) {
    (FileKey::Path(path()), 1)
}

/// A file with more than one link, as the ledger holds it.
#[derive(Debug, Clone)]
struct Counted {
    len: u64,
    /// The most links the file was seen with.
    nlink: u32,
    /// Measured paths that still reach the file.
    refs: u32,
    /// Paths ever measured to it, never decremented: when it is at least
    /// `nlink`, every link the file had was inside what this ledger measured.
    seen: u32,
}

/// What a deletion freed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Release {
    /// Lengths of the files no measured path reaches any more.
    pub logical: u64,
    /// The part of `logical` whose files had no link outside the ledger.
    pub physical: u64,
}

impl Release {
    pub(crate) fn add(&mut self, other: Release) {
        self.logical = self.logical.saturating_add(other.logical);
        self.physical = self.physical.saturating_add(other.physical);
    }
}

/// What one [`Ledger::measure`] found: the bytes of files with a single link,
/// which no other path can reach, and a key per path to any other file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Measured {
    pub unshared: u64,
    pub shared: Vec<FileKey>,
}

impl Measured {
    /// Drop what `removal` deleted from this measure, which covered it.
    pub(crate) fn forget(&mut self, removal: &Removal) {
        self.unshared = self.unshared.saturating_sub(removal.unshared);
        if removal.gone.is_empty() {
            return;
        }
        let mut gone_paths = HashMap::<&FileKey, usize>::new();
        for key in &removal.gone {
            *gone_paths.entry(key).or_default() += 1;
        }
        self.shared.retain(|key| match gone_paths.get_mut(key) {
            Some(count) if *count > 0 => {
                *count -= 1;
                false
            }
            _ => true,
        });
    }
}

/// Files measured under some set of roots, each counted once.
///
/// Only files with more than one link are kept by key: a file with a single
/// link is reached by one path, so its length is summed and nothing else.
#[derive(Debug, Default, Clone)]
pub(crate) struct Ledger {
    files: HashMap<FileKey, Counted>,
    /// The lengths of the kept files some measured path still reaches.
    shared_bytes: u64,
    /// The lengths of the single-link files measured and not yet released.
    unshared_bytes: u64,
}

impl Ledger {
    /// The total length of the files some measured path still reaches.
    pub(crate) fn total(&self) -> u64 {
        self.shared_bytes.saturating_add(self.unshared_bytes)
    }

    /// Measure every file under `root`, skipping `excluded` paths, so that
    /// releasing the result undoes this measure. Symlinks are not followed: a
    /// link's target is either inside a measured tree and counted there, or
    /// outside it and not ours to count.
    pub(crate) fn measure(&mut self, root: &Path, excluded: &HashSet<PathBuf>) -> Measured {
        let mut measured = Measured::default();
        if excluded.contains(root) {
            return measured;
        }
        let Ok(metadata) = std::fs::symlink_metadata(root) else {
            return measured;
        };
        if metadata.is_file() {
            self.add(&mut measured, || root.to_path_buf(), &metadata);
            return measured;
        }
        if !metadata.is_dir() {
            return measured;
        }
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in entries(&directory, warn_walk_error) {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    let path = entry.path();
                    if !excluded.contains(&path) {
                        pending.push(path);
                    }
                } else if kind.is_file() {
                    if !excluded.is_empty() && excluded.contains(&entry.path()) {
                        continue;
                    }
                    if let Ok(metadata) = entry.metadata() {
                        self.add(&mut measured, || entry.path(), &metadata);
                    }
                }
            }
        }
        measured
    }

    fn add(
        &mut self,
        measured: &mut Measured,
        path: impl FnOnce() -> PathBuf,
        metadata: &std::fs::Metadata,
    ) {
        match self.count(path, metadata) {
            Some(key) => measured.shared.push(key),
            None => measured.unshared = measured.unshared.saturating_add(metadata.len()),
        }
    }

    /// Count one path to a regular file. Returns the file's key when it has
    /// other links, and `None` when only its length was summed.
    pub(crate) fn count(
        &mut self,
        path: impl FnOnce() -> PathBuf,
        metadata: &std::fs::Metadata,
    ) -> Option<FileKey> {
        let (key, nlink) = file_key(path, metadata);
        self.count_key(key, nlink, metadata.len())
    }

    fn count_key(&mut self, key: FileKey, nlink: u64, len: u64) -> Option<FileKey> {
        // A file already kept stays kept, even if its other links went since.
        if nlink <= 1 && !self.files.contains_key(&key) {
            self.unshared_bytes = self.unshared_bytes.saturating_add(len);
            return None;
        }
        let nlink = u32::try_from(nlink).unwrap_or(u32::MAX);
        let counted = self.files.entry(key.clone()).or_insert(Counted {
            len,
            nlink,
            refs: 0,
            seen: 0,
        });
        counted.nlink = counted.nlink.max(nlink);
        if counted.refs == 0 {
            self.shared_bytes = self.shared_bytes.saturating_add(counted.len);
        }
        counted.refs = counted.refs.saturating_add(1);
        counted.seen = counted.seen.saturating_add(1);
        Some(key)
    }

    /// Release what `measured` found, as gone.
    pub(crate) fn release(&mut self, measured: &Measured) -> Release {
        self.unshared_bytes = self.unshared_bytes.saturating_sub(measured.unshared);
        let mut release = Release {
            logical: measured.unshared,
            physical: measured.unshared,
        };
        for key in &measured.shared {
            release.add(self.release_one(key, None));
        }
        release
    }

    /// Release one path to `key`. `links` is the file's link count just
    /// before that path was unlinked, when it is known.
    fn release_one(&mut self, key: &FileKey, links: Option<u64>) -> Release {
        let Some(counted) = self.files.get_mut(key) else {
            log::debug!("released a file the disk usage ledger never measured: {key:?}");
            return Release::default();
        };
        if counted.refs == 0 {
            log::debug!("released a file the disk usage ledger already released: {key:?}");
            return Release::default();
        }
        counted.refs -= 1;
        if counted.refs > 0 {
            return Release::default();
        }
        self.shared_bytes = self.shared_bytes.saturating_sub(counted.len);
        // Physical bytes are freed when the unlink removed the file's last
        // link: by the filesystem's count when the unlink was seen, else by
        // the ledger's.
        let freed = match links {
            Some(links) => links <= 1,
            None => counted.nlink <= counted.seen,
        };
        Release {
            logical: counted.len,
            physical: if freed { counted.len } else { 0 },
        }
    }

    /// Release one path a walk reached, by its key when the ledger keeps the
    /// file and by its length otherwise.
    fn release_path(
        &mut self,
        key: FileKey,
        len: u64,
        links: u64,
        unlinked: bool,
        removal: &mut Removal,
    ) {
        if self.files.contains_key(&key) {
            let release = self.release_one(&key, unlinked.then_some(links));
            removal.release.add(release);
            removal.gone.push(key);
        } else {
            self.unshared_bytes = self.unshared_bytes.saturating_sub(len);
            removal.unshared = removal.unshared.saturating_add(len);
            removal.release.add(Release {
                logical: len,
                physical: if links <= 1 { len } else { 0 },
            });
        }
    }

    /// Fold in a ledger measured separately, as if one ledger had measured
    /// both: a file both reached counts once.
    pub(crate) fn merge(&mut self, other: Ledger) {
        self.unshared_bytes = self.unshared_bytes.saturating_add(other.unshared_bytes);
        for (key, theirs) in other.files {
            match self.files.get_mut(&key) {
                Some(ours) => {
                    if ours.refs == 0 && theirs.refs > 0 {
                        self.shared_bytes = self.shared_bytes.saturating_add(ours.len);
                    }
                    ours.nlink = ours.nlink.max(theirs.nlink);
                    ours.refs = ours.refs.saturating_add(theirs.refs);
                    ours.seen = ours.seen.saturating_add(theirs.seen);
                }
                None => {
                    if theirs.refs > 0 {
                        self.shared_bytes = self.shared_bytes.saturating_add(theirs.len);
                    }
                    self.files.insert(key, theirs);
                }
            }
        }
    }
}

/// The bytes under `root`, each file counted once.
pub(crate) fn tree_bytes(root: &Path) -> u64 {
    let mut ledger = Ledger::default();
    ledger.measure(root, &HashSet::new());
    ledger.total()
}

/// What a counted removal did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Removal {
    pub release: Release,
    /// Paths other than directories deleted, whether or not they freed
    /// anything. A preview deletes nothing.
    pub deleted_files: u64,
    /// Bytes of single-link files released.
    pub unshared: u64,
    /// A key per released path to a file with other links, kept only when
    /// asked for.
    pub gone: Vec<FileKey>,
}

impl Removal {
    pub(crate) fn add(&mut self, other: Removal) {
        self.release.add(other.release);
        self.deleted_files += other.deleted_files;
        self.unshared = self.unshared.saturating_add(other.unshared);
        self.gone.extend(other.gone);
    }
}

#[derive(Clone, Copy)]
struct Walk {
    /// Whether to delete, or only release as if deleted.
    delete: bool,
    /// Whether `ledger` measured the tree; otherwise each file is counted
    /// just before it goes.
    measured: bool,
    /// Whether to keep the keys released, for [`Measured::forget`].
    keys: bool,
}

/// Delete `root` and everything under it, which `ledger` measured,
/// releasing each file only once it is gone. The first failure stops the
/// removal, leaving what survived counted. Without `delete`, releases the
/// files as if they had gone and deletes nothing.
pub(crate) fn remove_tree_counting(root: &Path, ledger: &mut Ledger, delete: bool) -> Removal {
    walk_tree(
        root,
        ledger,
        Walk {
            delete,
            measured: true,
            keys: false,
        },
    )
}

/// [`remove_tree_counting`], keeping the released keys so the measure that
/// covered `root` can [forget](Measured::forget) them.
pub(crate) fn remove_tree_counting_with_keys(
    root: &Path,
    ledger: &mut Ledger,
    delete: bool,
) -> Removal {
    walk_tree(
        root,
        ledger,
        Walk {
            delete,
            measured: true,
            keys: true,
        },
    )
}

/// Delete `root`, which `ledger` never measured, counting each file just
/// before it goes: the ledger ends where it started, and the credit comes from
/// the same stat as the deletion.
pub(crate) fn remove_unmeasured_tree(root: &Path, ledger: &mut Ledger) -> Removal {
    walk_tree(
        root,
        ledger,
        Walk {
            delete: true,
            measured: false,
            keys: false,
        },
    )
}

fn walk_tree(root: &Path, ledger: &mut Ledger, walk: Walk) -> Removal {
    let mut removal = Removal::default();
    let result = match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => walk_directory(root, ledger, walk, &mut removal),
        Ok(metadata) => walk_entry(
            root,
            metadata.file_type(),
            || Ok(metadata),
            ledger,
            walk,
            &mut removal,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        log::warn!("could not remove {}: {error}", root.display());
    }
    if !walk.keys {
        removal.gone = Vec::new();
    }
    removal
}

fn walk_directory(
    directory: &Path,
    ledger: &mut Ledger,
    walk: Walk,
    removal: &mut Removal,
) -> std::io::Result<()> {
    let listing = match std::fs::read_dir(directory) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in listing {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            walk_directory(&path, ledger, walk, removal)?;
        } else {
            walk_entry(&path, kind, || entry.metadata(), ledger, walk, removal)?;
        }
    }
    if walk.delete {
        match std::fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn walk_entry(
    path: &Path,
    kind: std::fs::FileType,
    metadata: impl FnOnce() -> std::io::Result<std::fs::Metadata>,
    ledger: &mut Ledger,
    walk: Walk,
    removal: &mut Removal,
) -> std::io::Result<()> {
    if !kind.is_file() {
        // A symlink, or something else that holds no file of its own.
        if walk.delete {
            remove_link(path)?;
            removal.deleted_files += 1;
        }
        return Ok(());
    }
    let metadata = match metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let (key, links) = file_key(|| path.to_path_buf(), &metadata);
    if !walk.measured {
        ledger.count_key(key.clone(), links, metadata.len());
    }
    if walk.delete {
        match remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        removal.deleted_files += 1;
    }
    ledger.release_path(key, metadata.len(), links, walk.delete, removal);
    Ok(())
}

/// Delete the file at `path`.
///
/// Windows refuses to delete a read-only file, which outputs restored or
/// seeded from the store are, so there the file is deleted with the
/// disposition that ignores the attribute, as [`std::fs::remove_dir_all`]
/// does. Clearing the attribute first is not an option: it belongs to the
/// file, not to this link, so a seeded output's surviving link in another view
/// would turn writable, and the clean-up before a compile, which unlinks only
/// read-only outputs, would then leave it for rustc to overwrite in place,
/// corrupting every link to it.
fn remove_file(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        remove_file_ignoring_read_only(path)
    }
    #[cfg(not(windows))]
    {
        std::fs::remove_file(path)
    }
}

#[cfg(windows)]
fn remove_file_ignoring_read_only(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::{
        ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX,
        FILE_FLAG_OPEN_REPARSE_POINT, FileDispositionInfoEx, SetFileInformationByHandle,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // The handle stays open and the input points to a correctly sized value.
    let deleted = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            std::ptr::from_ref(&info).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } != 0;
    if deleted {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    drop(file);
    match error.raw_os_error().map(|code| code as u32) {
        // Windows before 10 1809, or a filesystem without the extended
        // disposition: delete the ordinary way, and leave a read-only file
        // in place rather than touch its attribute.
        Some(ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION) => {
            std::fs::remove_file(path)
        }
        _ => Err(error),
    }
}

fn remove_link(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        // A directory symlink or junction on Windows.
        #[cfg(windows)]
        Err(_) if std::fs::remove_dir(path).is_ok() => Ok(()),
        result => result,
    }
}

/// The entries of `directory`, telling `warn` about any it cannot read.
pub(crate) fn entries(
    directory: &Path,
    warn: impl Fn(&Path, &std::io::Error),
) -> Vec<std::fs::DirEntry> {
    match std::fs::read_dir(directory) {
        Ok(listing) => listing
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    warn(directory, &error);
                    None
                }
            })
            .collect(),
        Err(error) => {
            warn(directory, &error);
            Vec::new()
        }
    }
}

fn warn_walk_error(path: &Path, error: &std::io::Error) {
    if error.kind() != std::io::ErrorKind::NotFound {
        log::warn!("could not measure {}: {error}", path.display());
    }
}

#[cfg(test)]
#[path = "disk_usage_tests.rs"]
mod tests;
