//! Temporary files for cache entries that several users may share.

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
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    builder
}

#[cfg(all(test, unix))]
mod tests {
    use super::shared_tempfile_builder;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn files_are_not_private_to_the_creator() {
        let directory = tempfile::tempdir().unwrap();
        let file = shared_tempfile_builder()
            .tempfile_in(directory.path())
            .unwrap();
        let mode = file.as_file().metadata().unwrap().permissions().mode() & 0o777;
        // Whatever the umask is, the helper must not force owner-only access
        // beyond it: the mode is exactly 0666 with the umask removed.
        assert_eq!(mode & !0o666, 0);
        assert_ne!(
            mode, 0o600,
            "umask of 077 would hide this check; run under 022"
        );
    }
}
