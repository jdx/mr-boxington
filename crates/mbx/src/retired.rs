//! Trees detached from their live names before slow deletion.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) struct RetiredTree(PathBuf);

impl RetiredTree {
    /// Call while holding the lock that owns the live name. Rename only
    /// after the final probe; finish bookkeeping before releasing that lock.
    pub(crate) fn retire(live: &Path) -> io::Result<Option<Self>> {
        let Some(name) = live.file_name() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tree has no name",
            ));
        };
        let mut retired = name.to_os_string();
        retired.push(format!(
            ".retired-{}-{}",
            std::process::id(),
            crate::util::random_string(24)
        ));
        // macOS requires a writable directory for rename. Do not change
        // file modes: incremental files can share inodes with other trees.
        #[cfg(unix)]
        if let Ok(metadata) = std::fs::symlink_metadata(live)
            && metadata.is_dir()
        {
            use std::os::unix::fs::PermissionsExt as _;
            let permissions = metadata.permissions();
            std::fs::set_permissions(
                live,
                std::fs::Permissions::from_mode(permissions.mode() | 0o700),
            )?;
            if let Err(error) = std::fs::rename(live, live.with_file_name(&retired)) {
                let _ = std::fs::set_permissions(live, permissions);
                return if error.kind() == io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            return Ok(Some(Self(live.with_file_name(retired))));
        }
        match std::fs::rename(live, live.with_file_name(&retired)) {
            Ok(()) => Ok(Some(Self(live.with_file_name(retired)))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn bytes(&self) -> u64 {
        crate::incremental::tree_bytes(self.path())
    }

    pub(crate) fn remove(self) -> io::Result<()> {
        remove_tree(self.path())
    }
}

pub(crate) fn is_retired_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.rsplit_once(".retired-").is_some_and(|(live, suffix)| {
            !live.is_empty()
                && suffix.split_once('-').is_some_and(|(pid, random)| {
                    !pid.is_empty()
                        && pid.bytes().all(|byte| byte.is_ascii_digit())
                        && !random.is_empty()
                        && random.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
        })
    })
}

#[derive(Default)]
pub(crate) struct SweepOutcome {
    pub(crate) removed: usize,
    pub(crate) bytes: u64,
}

/// Retired names have no live users. Callers must serialize sweeps with the
/// collector lock: concurrent sweeps could credit the same tree twice.
pub(crate) fn sweep(parent: &Path) -> SweepOutcome {
    sweep_with(parent, RetiredTree::remove)
}

/// The removal callback allows tests to inject a failed deletion.
pub(crate) fn sweep_with(
    parent: &Path,
    mut remove: impl FnMut(RetiredTree) -> io::Result<()>,
) -> SweepOutcome {
    let mut outcome = SweepOutcome::default();
    let Ok(entries) = std::fs::read_dir(parent) else {
        return outcome;
    };
    for entry in entries.flatten() {
        if !is_retired_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let tree = RetiredTree(path.clone());
        let bytes = tree.bytes();
        if remove(tree).is_ok() {
            outcome.removed += 1;
            outcome.bytes = outcome.bytes.saturating_add(bytes);
        } else {
            let remaining = crate::incremental::tree_bytes(&path);
            outcome.bytes = outcome
                .bytes
                .saturating_add(bytes.saturating_sub(remaining));
        }
    }
    outcome
}

/// Unlinking needs writable directories on Unix. Windows also requires
/// clearing the read-only attribute on files before removing them.
pub(crate) fn remove_tree(path: &Path) -> io::Result<()> {
    let result = (|| {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            #[cfg(not(unix))]
            if metadata.is_file() {
                let mut permissions = metadata.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                std::fs::set_permissions(path, permissions)?;
            }
            return std::fs::remove_file(path);
        }
        let mut pending = vec![path.to_path_buf()];
        while let Some(directory) = pending.pop() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))?;
            }
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    pending.push(entry.path());
                } else {
                    #[cfg(not(unix))]
                    if kind.is_file() {
                        let mut permissions = entry.metadata()?.permissions();
                        #[allow(clippy::permissions_set_readonly_false)]
                        permissions.set_readonly(false);
                        std::fs::set_permissions(entry.path(), permissions)?;
                    }
                }
            }
        }
        std::fs::remove_dir_all(path)
    })();
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn removal_does_not_follow_file_symlinks() {
        use std::os::windows::fs::symlink_file;

        let directory = tempfile::tempdir().unwrap();
        let outside = directory.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        let tree = directory.path().join("tree");
        std::fs::create_dir(&tree).unwrap();
        // Windows requires Developer Mode or symlink privileges.
        if let Err(error) = symlink_file(&outside, tree.join("link")) {
            assert_eq!(error.raw_os_error(), Some(1314), "{error}");
            return;
        }
        symlink_file(directory.path().join("missing"), tree.join("broken")).unwrap();
        let mut permissions = std::fs::metadata(&outside).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&outside, permissions).unwrap();

        let result = remove_tree(&tree);
        let still_readonly = std::fs::metadata(&outside)
            .unwrap()
            .permissions()
            .readonly();
        // Restore the attribute so TempDir can clean up even if an assertion fails.
        let mut permissions = std::fs::metadata(&outside).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&outside, permissions).unwrap();
        result.unwrap();
        assert!(!tree.exists());
        assert!(still_readonly);
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    }
}
