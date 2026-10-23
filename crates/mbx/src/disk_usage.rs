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

/// The link count given a file whose identity could not be read. Such a file
/// is counted by its length, as one no other path reaches, which survives a
/// rename where a key by its path would not; but it is never one, so the file
/// is never reported as released.
const UNKNOWN_LINKS: u64 = u64::MAX;

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
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let path = path();
    // No access rights, only attributes: a file another process holds open
    // without sharing can still be identified.
    let opened = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path);
    if let Ok(file) = opened {
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
    (FileKey::Path(path), UNKNOWN_LINKS)
}

#[cfg(not(any(unix, windows)))]
fn file_key(path: impl FnOnce() -> PathBuf, _metadata: &std::fs::Metadata) -> (FileKey, u64) {
    (FileKey::Path(path()), UNKNOWN_LINKS)
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
    /// The part of `unshared` whose files could not be identified.
    pub unknown: u64,
    pub shared: Vec<FileKey>,
}

impl Measured {
    /// Drop what `removal` deleted from this measure, which covered it.
    pub(crate) fn forget(&mut self, removal: &Removal) {
        self.unshared = self.unshared.saturating_sub(removal.unshared);
        self.unknown = self.unknown.saturating_sub(removal.unknown);
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
        let (key, nlink) = file_key(path, metadata);
        match self.count_key(key, nlink, metadata.len()) {
            Some(key) => measured.shared.push(key),
            None => {
                measured.unshared = measured.unshared.saturating_add(metadata.len());
                if nlink == UNKNOWN_LINKS {
                    measured.unknown = measured.unknown.saturating_add(metadata.len());
                }
            }
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
        if (nlink <= 1 || nlink == UNKNOWN_LINKS) && !self.files.contains_key(&key) {
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
            physical: measured.unshared.saturating_sub(measured.unknown),
        };
        for key in &measured.shared {
            release.add(self.release_one(key));
        }
        release
    }

    /// Release one path to `key`, estimating whether its data leaves the
    /// disk from the links the ledger measured.
    fn release_one(&mut self, key: &FileKey) -> Release {
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
        // Freed on disk when every link the file had was measured. Sweeps
        // measure each root once, so `seen` counts distinct links.
        let freed = counted.nlink <= counted.seen;
        Release {
            logical: counted.len,
            physical: if freed { counted.len } else { 0 },
        }
    }

    /// Release one path a walk reached, by its key when the ledger keeps the
    /// file and by its length otherwise. `freed` is a real removal's verdict
    /// on whether the file left the disk; a preview leaves it to the ledger's
    /// estimate.
    fn release_path(
        &mut self,
        key: FileKey,
        len: u64,
        links: u64,
        freed: Option<bool>,
        removal: &mut Removal,
    ) {
        let mut release = if self.files.contains_key(&key) {
            let release = self.release_one(&key);
            removal.gone.push(key);
            release
        } else {
            self.unshared_bytes = self.unshared_bytes.saturating_sub(len);
            removal.unshared = removal.unshared.saturating_add(len);
            if links == UNKNOWN_LINKS {
                removal.unknown = removal.unknown.saturating_add(len);
            }
            Release {
                logical: len,
                physical: if links <= 1 { len } else { 0 },
            }
        };
        if let Some(freed) = freed {
            release.physical = if freed { release.logical } else { 0 };
        }
        removal.release.add(release);
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
    /// The part of `unshared` whose files could not be identified.
    pub unknown: u64,
    /// A key per released path to a file with other links, kept only when
    /// asked for.
    pub gone: Vec<FileKey>,
}

impl Removal {
    pub(crate) fn add(&mut self, other: Removal) {
        self.release.add(other.release);
        self.deleted_files += other.deleted_files;
        self.unshared = self.unshared.saturating_add(other.unshared);
        self.unknown = self.unknown.saturating_add(other.unknown);
        self.gone.extend(other.gone);
    }
}

#[derive(Clone, Copy)]
struct Walk {
    /// Whether to delete, or only release as if deleted.
    delete: bool,
    /// Whether to keep the keys released, for [`Measured::forget`].
    keys: bool,
}

/// Delete `root` and everything under it, which `ledger` measured,
/// releasing each file only once it is gone. The first failure stops the
/// removal, leaving what survived counted. Without `delete`, releases the
/// files as if they had gone and deletes nothing.
#[cfg(test)]
pub(crate) fn remove_tree_counting(root: &Path, ledger: &mut Ledger, delete: bool) -> Removal {
    walk_tree(
        root,
        ledger,
        Walk {
            delete,
            keys: false,
        },
    )
}

/// Delete `root`, which `ledger` measured as `measured`, crediting what that
/// measure counted rather than whatever the deletion finds: a file written
/// after the measure was never counted, so removing it frees nothing counted.
/// Should the deletion stop partway, what survived is measured back in.
pub(crate) fn remove_measured_tree(
    root: &Path,
    ledger: &mut Ledger,
    measured: &Measured,
) -> Removal {
    let removed = remove_tree_reporting(root, identify_file, |(key, _, _)| key.clone());
    if let Err(error) = &removed.result {
        log::warn!("could not remove {}: {error}", root.display());
    }
    let before = ledger.total();
    ledger.release(measured);
    if removed.result.is_err() {
        ledger.measure(root, &HashSet::new());
    }
    let logical = before.saturating_sub(ledger.total());
    let gone = links_gone(&removed.files, |(key, _, _)| key);
    let mut freed = HashSet::new();
    let physical = removed
        .files
        .iter()
        .filter(|(key, _, links)| gone[key] >= *links && freed.insert(key))
        .map(|(_, len, _)| len)
        .sum::<u64>();
    Removal {
        release: Release {
            logical,
            // Part of what was counted: a file written since is not.
            physical: physical.min(logical),
        },
        deleted_files: removed.entries,
        ..Removal::default()
    }
}

/// A file's key, length and link count, for a removal to account for it.
fn identify_file(path: &Path, metadata: &std::fs::Metadata) -> Option<(FileKey, u64, u64)> {
    metadata.is_file().then(|| {
        let (key, links) = file_key(|| path.to_path_buf(), metadata);
        (key, metadata.len(), links)
    })
}

/// Delete `root` and everything under it, which `ledger` measured, crediting
/// each file once it is gone, and keep the released keys so the measure that
/// covered `root` can [forget](Measured::forget) them. Without `delete`,
/// releases the files as if they had gone and deletes nothing.
pub(crate) fn remove_tree_counting_with_keys(
    root: &Path,
    ledger: &mut Ledger,
    delete: bool,
) -> Removal {
    walk_tree(root, ledger, Walk { delete, keys: true })
}

fn walk_tree(root: &Path, ledger: &mut Ledger, walk: Walk) -> Removal {
    let mut removal = Removal::default();
    // Identified before anything goes, while each file can still be opened.
    let identify = identify_file;
    if walk.delete {
        let removed = remove_tree_reporting(root, identify, |(key, _, _)| key.clone());
        if let Err(error) = removed.result {
            log::warn!("could not remove {}: {error}", root.display());
        }
        removal.deleted_files = removed.entries;
        let gone = links_gone(&removed.files, |(key, _, _)| key);
        for (key, len, links) in removed.files {
            let freed = gone.get(&key).is_some_and(|paths| *paths >= links);
            ledger.release_path(key, len, links, Some(freed), &mut removal);
        }
    } else {
        for (key, len, links) in list_tree(root, identify).0 {
            ledger.release_path(key, len, links, None, &mut removal);
        }
    }
    if !walk.keys {
        removal.gone = Vec::new();
    }
    removal
}

/// How many paths to each file a removal deleted. A file left the disk when
/// they are at least the links it had just before: none survives anywhere.
fn links_gone<T>(files: &[T], key: impl Fn(&T) -> &FileKey) -> HashMap<FileKey, u64> {
    let mut gone = HashMap::new();
    for file in files {
        *gone.entry(key(file).clone()).or_default() += 1;
    }
    gone
}

/// What [`remove_tree_reporting`] deleted.
pub(crate) struct TreeRemoval<T> {
    /// The account of each file that went, as given before the removal.
    pub files: Vec<T>,
    /// Entries other than directories that went: files, and links that hold
    /// no file of their own.
    pub entries: u64,
    pub result: std::io::Result<()>,
}

/// Delete `root` and everything under it, and report what went.
///
/// The deletion is [`std::fs::remove_dir_all`], which never follows a
/// symbolic link or junction, even one swapped in for a directory while it
/// runs; a walk that opens each directory by path could be led outside
/// `root`. Each file is identified beforehand, while it can still be opened,
/// and only that account is kept, not its path. A removal that stops partway
/// lists what survived and reports the rest, matching files by `key`.
pub(crate) fn remove_tree_reporting<T, K: Eq + std::hash::Hash>(
    root: &Path,
    mut identify: impl FnMut(&Path, &std::fs::Metadata) -> Option<T>,
    key: impl Fn(&T) -> K,
) -> TreeRemoval<T> {
    let (mut files, mut entries) = list_tree(root, &mut identify);
    let result = match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(root),
        // On Windows std deletes a read-only file with the disposition that
        // ignores the attribute, and never clears it: a seeded output's
        // surviving link in another view must stay read-only, or the
        // clean-up before a compile would leave it for rustc to overwrite in
        // place, corrupting every link to it.
        Ok(_) => std::fs::remove_file(root).or_else(|error| {
            // A directory symlink or junction on Windows.
            if std::fs::remove_dir(root).is_ok() {
                Ok(())
            } else {
                Err(error)
            }
        }),
        Err(error) => Err(error),
    };
    let result = match result {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    };
    if result.is_err() {
        let (survivors, surviving_entries) = list_tree(root, &mut identify);
        let mut surviving = HashMap::<K, usize>::new();
        for file in &survivors {
            *surviving.entry(key(file)).or_default() += 1;
        }
        files.retain(|file| match surviving.get_mut(&key(file)) {
            Some(count) if *count > 0 => {
                *count -= 1;
                false
            }
            _ => true,
        });
        entries = entries.saturating_sub(surviving_entries);
    }
    TreeRemoval {
        files,
        entries,
        result,
    }
}

/// The account `identify` gives of each file under `root`, and how many
/// entries other than directories there are. Links are counted, never
/// followed; one swapped in during the listing can only make it count
/// something it should not.
fn list_tree<T>(
    root: &Path,
    mut identify: impl FnMut(&Path, &std::fs::Metadata) -> Option<T>,
) -> (Vec<T>, u64) {
    let mut files = Vec::new();
    let mut entries = 0;
    let Ok(metadata) = std::fs::symlink_metadata(root) else {
        return (files, entries);
    };
    if !metadata.is_dir() {
        files.extend(identify(root, &metadata));
        return (files, 1);
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in entries_of(&directory) {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if let Ok(metadata) = entry.metadata() {
                entries += 1;
                files.extend(identify(&entry.path(), &metadata));
            }
        }
    }
    (files, entries)
}

fn entries_of(directory: &Path) -> Vec<std::fs::DirEntry> {
    entries(directory, warn_walk_error)
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
