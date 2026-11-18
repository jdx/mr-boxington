//! Files and locks for cache entries that several users may share.

use std::path::Path;

/// A [`tempfile::Builder`] whose files follow the process umask.
///
/// `tempfile` creates files with mode `0600` unless told otherwise, and a
/// rename keeps that mode. In a cache directory shared by a group of users
/// (a setgid directory, or `MBX_CACHE_DIR` on a shared volume), the second
/// user then cannot read what the first stored, and fails with a permission
/// error. Asking for `0666` lets the umask decide: `022` gives `0644` and
/// `002` gives `0664`, the same result a plain `File::create` would give.
#[must_use]
pub fn shared_tempfile_builder() -> tempfile::Builder<'static, 'static> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut builder = tempfile::Builder::new();
        builder.permissions(std::fs::Permissions::from_mode(0o666));
        builder
    }
    #[cfg(not(unix))]
    {
        tempfile::Builder::new()
    }
}

/// Open a lock file that every user of a shared cache can reopen.
///
/// `fslock` creates a missing lock file with mode `0644`, so with two users in
/// one group the second cannot open the first's lock for writing. Creating the
/// file first with `0666` lets the umask decide, as for cache entries.
pub fn open_shared_lock(path: &Path) -> std::io::Result<fslock::LockFile> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o666);
    }
    drop(options.open(path)?);
    fslock::LockFile::open(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{open_shared_lock, shared_tempfile_builder};
    use std::os::unix::fs::PermissionsExt as _;

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn tempfiles_get_the_mode_a_plain_create_would() {
        let directory = tempfile::tempdir().unwrap();
        let plain = directory.path().join("plain");
        drop(std::fs::File::create(&plain).unwrap());
        let file = shared_tempfile_builder()
            .tempfile_in(directory.path())
            .unwrap();
        // Both calls see the same process umask, so this holds under 022,
        // 002 and 077 alike.
        assert_eq!(mode(file.path()), mode(&plain));
    }

    #[test]
    fn locks_get_the_mode_a_plain_create_would() {
        let directory = tempfile::tempdir().unwrap();
        let plain = directory.path().join("plain");
        drop(std::fs::File::create(&plain).unwrap());
        let lock = directory.path().join("a.lock");
        drop(open_shared_lock(&lock).unwrap());
        assert_eq!(mode(&lock), mode(&plain));
    }
}
