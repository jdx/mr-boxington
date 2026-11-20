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

/// Clone `from` to `to` where the filesystem can, and copy it where it cannot.
///
/// `reflink_copy::reflink_or_copy` gives up instead of copying when the clone
/// fails with a permission error, and an unprivileged LXC container on ext4
/// answers `FICLONE` with `EPERM` where bare ext4 says it is unsupported. Every
/// clone failure here falls back to a copy except a missing source and an
/// existing destination, which a copy would not fix or would overwrite. A
/// genuine permission problem still surfaces, as the copy's own error.
pub fn clone_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    // Clone from an opened handle, not a path: `reflink_copy::reflink` reports
    // every failure of a source that is not itself a regular file (a symlink to
    // one, or a missing path) as `InvalidInput`, which would hide an existing
    // destination and let the copy overwrite it.
    let source = std::fs::File::open(from)?;
    clone_or_copy_with(from, to, |_, to| {
        reflink_copy::reflink_file_to_path(&source, to)
    })
}

fn clone_or_copy_with(
    from: &Path,
    to: &Path,
    clone: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match clone(from, to) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::AlreadyExists
            ) =>
        {
            Err(error)
        }
        // `fs::copy` truncates whatever a failed clone left at `to`.
        Err(_) => std::fs::copy(from, to).map(drop),
    }
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

#[cfg(test)]
mod clone_tests {
    use super::clone_or_copy_with;
    use std::io::{Error, ErrorKind};
    use std::path::Path;

    /// What an unprivileged LXC container on ext4 answers `FICLONE` with.
    fn refused() -> Error {
        Error::from(ErrorKind::PermissionDenied)
    }

    #[test]
    fn a_refused_clone_falls_back_to_a_copy() {
        let directory = tempfile::tempdir().unwrap();
        let from = directory.path().join("from");
        let to = directory.path().join("to");
        std::fs::write(&from, b"cached bytes").unwrap();
        // A failed clone may leave an empty destination behind.
        clone_or_copy_with(&from, &to, |_, to: &Path| {
            std::fs::write(to, b"")?;
            Err(refused())
        })
        .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"cached bytes");
    }

    #[test]
    fn a_missing_source_is_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let to = directory.path().join("to");
        let error = super::clone_or_copy(&directory.path().join("missing"), &to).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert!(!to.exists());
    }

    #[test]
    fn an_existing_destination_is_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let from = directory.path().join("from");
        let to = directory.path().join("to");
        std::fs::write(&from, b"cached bytes").unwrap();
        std::fs::write(&to, b"keep").unwrap();
        let error = clone_or_copy_with(&from, &to, |_, _| {
            Err(Error::from(ErrorKind::AlreadyExists))
        })
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&to).unwrap(), b"keep");
    }

    #[test]
    fn a_failed_copy_reports_the_copy_error() {
        let directory = tempfile::tempdir().unwrap();
        let from = directory.path().join("from");
        std::fs::write(&from, b"cached bytes").unwrap();
        let to = directory.path().join("no-such-directory/to");
        let error = clone_or_copy_with(&from, &to, |_, _| Err(refused())).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn the_real_clone_keeps_an_existing_destination_behind_a_symlinked_source() {
        let directory = tempfile::tempdir().unwrap();
        let from = directory.path().join("from");
        let link = directory.path().join("link");
        let to = directory.path().join("to");
        std::fs::write(&from, b"cached bytes").unwrap();
        std::os::unix::fs::symlink(&from, &link).unwrap();
        std::fs::write(&to, b"keep").unwrap();
        let error = super::clone_or_copy(&link, &to).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&to).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn a_copied_executable_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let from = directory.path().join("from");
        let to = directory.path().join("to");
        std::fs::write(&from, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o755)).unwrap();
        clone_or_copy_with(&from, &to, |_, _| Err(refused())).unwrap();
        let mode = std::fs::metadata(&to).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }
}
