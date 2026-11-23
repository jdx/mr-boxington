//! Test helpers that set file times through `std`.
//!
//! `filetime` deprecates its setters and the `from_*_time` readers on Rust 1.99
//! and later, so tests call these instead. Each one takes and returns
//! [`FileTime`] so call sites keep their existing values.

#![allow(dead_code)]

use filetime::FileTime;
use std::fs::{self, FileTimes};
use std::io;
use std::path::Path;
use std::time::SystemTime;

/// Open `path` the way `futimens` and `SetFileTime` accept: a file or a
/// directory, without needing write permission on its contents.
///
/// `fs::set_times` would do this in one call, but it is stable only since Rust
/// 1.99 and the workspace supports 1.91.
fn open_for_times(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    options.open(path)
}

fn set_times(path: impl AsRef<Path>, times: FileTimes) -> io::Result<()> {
    open_for_times(path.as_ref())?.set_times(times)
}

/// Set the access and modification times of `path`.
pub fn set_file_times(path: impl AsRef<Path>, atime: FileTime, mtime: FileTime) -> io::Result<()> {
    set_times(
        path,
        FileTimes::new()
            .set_accessed(SystemTime::from(atime))
            .set_modified(SystemTime::from(mtime)),
    )
}

/// Set the modification time of `path`, leaving its access time alone.
pub fn set_file_mtime(path: impl AsRef<Path>, mtime: FileTime) -> io::Result<()> {
    set_times(path, FileTimes::new().set_modified(SystemTime::from(mtime)))
}

/// Set the access time of `path`, leaving its modification time alone.
pub fn set_file_atime(path: impl AsRef<Path>, atime: FileTime) -> io::Result<()> {
    set_times(path, FileTimes::new().set_accessed(SystemTime::from(atime)))
}

/// The modification time recorded in `meta`.
pub fn modified(meta: &fs::Metadata) -> FileTime {
    FileTime::from(
        meta.modified()
            .expect("platform records modification times"),
    )
}

/// The access time recorded in `meta`.
pub fn accessed(meta: &fs::Metadata) -> FileTime {
    FileTime::from(meta.accessed().expect("platform records access times"))
}
