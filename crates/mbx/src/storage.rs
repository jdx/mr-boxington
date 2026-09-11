//! Validate build-storage placement before starting a cache session.

use crate::config::Config;
use eyre::{Context, Result};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn check_cache(config: &Config) -> Result<()> {
    // Check existing submounts and symlinks as well as the configured root.
    for path in [
        config.cache_dir.clone(),
        config.store_dir(),
        config.cache_dir.join("shims"),
        config.cache_dir.join("incremental"),
        config.cache_dir.join("scheduler"),
        config.cache_dir.join("cargo-roots"),
    ] {
        require_local(&path, "mbx cache directory", "MBX_CACHE_DIR")?;
    }
    Ok(())
}

pub(crate) fn require_local(path: &Path, role: &str, setting: &str) -> Result<()> {
    require_local_with(path, role, setting, is_nfs)
}

fn require_local_with(
    path: &Path,
    role: &str,
    setting: &str,
    probe: impl FnOnce(&Path) -> io::Result<bool>,
) -> Result<()> {
    let absolute = std::path::absolute(path)?;
    let existing = existing_directory(&absolute, 0)
        .wrap_err_with(|| format!("could not inspect {role} {}", path.display()))?;
    if probe(&existing).wrap_err_with(|| {
        format!(
            "could not identify the filesystem for {role} {}",
            path.display()
        )
    })? {
        eyre::bail!(
            "{role} {} is on NFS, which is unsupported for build storage. Set {setting} to a local filesystem. Remote caches remain supported.",
            path.display()
        );
    }
    Ok(())
}

/// Follow even dangling directory links before choosing an existing ancestor.
/// `Path::exists` alone would mistake a dangling link to NFS for local storage.
fn existing_directory(path: &Path, depth: usize) -> io::Result<PathBuf> {
    if depth > 256 {
        return Err(io::Error::other(
            "too many directory or symbolic-link components",
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_symlink() => {
            let destination = std::fs::read_link(path)?;
            let destination = if destination.is_absolute() {
                destination
            } else {
                path.parent().unwrap_or(Path::new(".")).join(destination)
            };
            existing_directory(&destination, depth + 1)
        }
        Ok(metadata) if metadata.is_dir() => std::fs::canonicalize(path),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| *parent != path)
                .ok_or(error)?;
            existing_directory(parent, depth + 1)
        }
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn is_nfs(path: &Path) -> io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: path is NUL-terminated and stats has the ABI's size and alignment.
    if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statfs initialized stats on success.
    let stats = unsafe { stats.assume_init() };
    #[cfg(target_os = "linux")]
    {
        Ok(stats.f_type as u64 == 0x6969)
    }
    #[cfg(target_os = "macos")]
    {
        Ok(stats.f_fstypename[..4]
            == [
                b'n' as libc::c_char,
                b'f' as libc::c_char,
                b's' as libc::c_char,
                0,
            ])
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn is_nfs(_path: &Path) -> io::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nfs_with_the_path_and_remedy_without_creating_storage() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("not/created");
        let error = require_local_with(&path, "mbx cache directory", "MBX_CACHE_DIR", |existing| {
            assert_eq!(existing, temp.path().canonicalize().unwrap());
            Ok(true)
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains(&path.display().to_string()));
        assert!(error.contains("NFS"));
        assert!(error.contains("MBX_CACHE_DIR"));
        assert!(!path.exists());
    }

    #[test]
    fn accepts_local_storage_and_reports_inspection_errors() {
        let temp = tempfile::tempdir().unwrap();
        require_local_with(temp.path(), "cache", "MBX_CACHE_DIR", |_| Ok(false)).unwrap();
        let error = require_local_with(temp.path(), "cache", "MBX_CACHE_DIR", |_| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("permission denied"));
        let file = temp.path().join("file");
        std::fs::write(&file, "data").unwrap();
        assert!(
            require_local_with(&file.join("child"), "cache", "setting", |_| Ok(false)).is_err()
        );
    }

    #[test]
    #[cfg(unix)]
    fn follows_links_in_both_directions_including_dangling_destinations() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("local");
        let nfs = temp.path().join("nfs");
        std::fs::create_dir(&local).unwrap();
        std::fs::create_dir(&nfs).unwrap();
        let nfs = nfs.canonicalize().unwrap();
        let probe = |path: &Path| Ok(path.starts_with(&nfs));
        symlink("../nfs/missing", local.join("out")).unwrap();
        assert!(require_local_with(&local.join("out/deeper"), "target", "setting", probe).is_err());
        symlink(&local, nfs.join("target")).unwrap();
        require_local_with(&nfs.join("target/missing"), "target", "setting", probe).unwrap();
        symlink("loop", local.join("loop")).unwrap();
        assert!(require_local_with(&local.join("loop"), "target", "setting", probe).is_err());
    }

    #[test]
    fn probes_the_local_test_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!is_nfs(temp.path()).unwrap());
    }
}
