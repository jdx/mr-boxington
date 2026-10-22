//! Object files a successful compilation made redundant.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How much older than a compilation an object file must be for that
/// compilation to remove it. A filesystem that keeps change times to the
/// second, or a clock that lags, could otherwise hide an object rewritten in
/// place, at its old length, in the same tick as the snapshot.
const OBJECT_AGE_MARGIN: Duration = Duration::from_secs(2);

/// What identifies one object file: rustc writing, replacing, or linking it
/// changes at least its inode or its change time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ObjectIdentity {
    device: u64,
    inode: u64,
    changed: (i64, i64),
    len: u64,
}

impl ObjectIdentity {
    #[cfg(unix)]
    fn of(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = std::fs::symlink_metadata(path).ok()?;
        metadata.is_file().then(|| Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            len: metadata.len(),
        })
    }

    /// Windows keeps no loose object files, so nothing is ever recorded.
    #[cfg(not(unix))]
    fn of(_path: &Path) -> Option<Self> {
        None
    }

    fn changed_at(&self) -> Option<SystemTime> {
        let (seconds, nanoseconds) = self.changed;
        let seconds = u64::try_from(seconds).ok()?;
        let nanoseconds = u32::try_from(nanoseconds).ok()?;
        SystemTime::UNIX_EPOCH.checked_add(Duration::new(seconds, nanoseconds))
    }
}

/// The object files one unit had in its output directory just before a
/// compilation of it started.
///
/// rustc writes a complete new set of `.rcgu.o` files on every compilation of
/// a unit, each name carrying a fresh suffix, and deletes none of the earlier
/// sets where debug info stays in them (`split-debuginfo=unpacked`, which
/// Cargo passes on macOS for profiles with debug info). The linked program's
/// debug map names only the newest set, so the
/// rest only take space: tens of thousands of files for a test unit compiled
/// all day.
#[derive(Debug)]
pub(crate) struct ObjectSnapshot {
    started: SystemTime,
    directory: PathBuf,
    prefix: String,
    objects: HashMap<PathBuf, ObjectIdentity>,
}

impl ObjectSnapshot {
    /// Record the unit's objects for the rustc command line `arguments`, run
    /// in `working_dir`, which is about to start. `None` when the unit cannot
    /// be identified, when the command asks rustc to keep its temporary
    /// files, or when it cannot leave object files behind.
    pub(crate) fn before_compiling(arguments: &[OsString], working_dir: &Path) -> Option<Self> {
        let (directory, prefix) = unit_objects(arguments, working_dir)?;
        Some(Self::take(&directory, &prefix, SystemTime::now()))
    }

    /// Record the object files in `directory` whose names start with
    /// `prefix`, for a compilation that starts at `started`.
    pub(crate) fn take(directory: &Path, prefix: &str, started: SystemTime) -> Self {
        Self {
            started,
            directory: directory.to_path_buf(),
            prefix: prefix.to_string(),
            objects: unit_object_files(directory, prefix),
        }
    }

    /// Once the compilation exited with `status`, delete the recorded
    /// objects it made redundant, if it succeeded. A failed compilation keeps
    /// them: its own may be incomplete, and the next attempt finds the earlier
    /// ones as they were. Returns how many went. A recorded name goes when
    ///
    /// - it is as it was and older than the compilation by
    ///   [`OBJECT_AGE_MARGIN`]: whatever the compilation wrote, replaced, or
    ///   linked again has another inode or a later change time, or
    /// - it is now another name of a file the compilation created a name for:
    ///   rustc links a reused object under a new name and hands the linker
    ///   that one, so the file stays reachable and nothing is lost.
    ///
    /// Whatever the compilation created was never recorded.
    pub(crate) fn remove_superseded(self, status: &std::process::ExitStatus) -> u64 {
        if !status.success() || self.objects.is_empty() {
            return 0;
        }
        let Some(cutoff) = self.started.checked_sub(OBJECT_AGE_MARGIN) else {
            return 0;
        };
        let current = unit_object_files(&self.directory, &self.prefix);
        let produced: std::collections::HashSet<(u64, u64)> = current
            .iter()
            .filter(|(path, _)| !self.objects.contains_key(*path))
            .map(|(_, identity)| (identity.device, identity.inode))
            .collect();
        let mut removed = 0;
        for (path, identity) in &self.objects {
            let Some(now) = current.get(path) else {
                continue;
            };
            let untouched_and_old = now == identity
                && identity
                    .changed_at()
                    .is_some_and(|changed| changed <= cutoff);
            let renamed = produced.contains(&(now.device, now.inode));
            if !(untouched_and_old || renamed) {
                continue;
            }
            match std::fs::remove_file(path) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    crate::session::report_shim_warning(&format!(
                        "could not remove the superseded object file {}: {error}",
                        path.display()
                    ));
                    break;
                }
            }
        }
        removed
    }
}

/// The object files in `directory` whose names start with `prefix`.
fn unit_object_files(directory: &Path, prefix: &str) -> HashMap<PathBuf, ObjectIdentity> {
    let mut objects = HashMap::new();
    if cfg!(unix)
        && let Ok(listing) = std::fs::read_dir(directory)
    {
        for entry in listing.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !(name.starts_with(prefix) && name.ends_with(".rcgu.o")) {
                continue;
            }
            let path = entry.path();
            if let Some(identity) = ObjectIdentity::of(&path) {
                objects.insert(path, identity);
            }
        }
    }
    objects
}

/// The directory a rustc command line writes its objects to, and the prefix
/// every object of its unit has there: `<crate>-<hash>.` in a directory units
/// share, or `<crate>.` in the one a unit owns from Cargo 1.100. `None` when
/// the unit is neither, when `-C save-temps` asks to keep them, or
/// when the compilation leaves none: it emits neither objects nor a link, or
/// keeps debug info somewhere other than the objects, as everything but
/// `split-debuginfo=unpacked` does. Without the flag, an Apple target is taken
/// as unpacked; where rustc packs instead, it deletes its objects itself and
/// the snapshot finds nothing to remove.
fn unit_objects(arguments: &[OsString], working_dir: &Path) -> Option<(PathBuf, String)> {
    let mut extra_filename = None;
    let mut split_debuginfo = None;
    for option in crate::unit_graph::codegen_options(arguments) {
        let (name, value) = option.split_once('=').unwrap_or((option, ""));
        match name {
            "extra-filename" => extra_filename = Some(value),
            "split-debuginfo" => split_debuginfo = Some(value),
            "save-temps" if !matches!(value, "n" | "no" | "off" | "false") => return None,
            _ => {}
        }
    }
    let unpacked = match split_debuginfo {
        Some(value) => value == "unpacked",
        None => {
            cfg!(target_os = "macos")
                && crate::rustc::flag_value(arguments, "--target")
                    .is_none_or(|target| target.contains("apple"))
        }
    };
    if !unpacked || !emits_objects(arguments) {
        return None;
    }
    let crate_name = crate::session::crate_name_argument(arguments)?;
    let out_dir = working_dir.join(crate::rustc::flag_value(arguments, "--out-dir")?);
    let prefix = match extra_filename {
        // Through Cargo 1.99 units share `deps/`; the hash tells them apart.
        Some(hash) => {
            if !crate::unit_graph::is_unit_hash(hash.strip_prefix('-')?) {
                return None;
            }
            format!("{crate_name}{hash}.")
        }
        // From Cargo 1.100 a unit owns its output directory and its files
        // carry no hash.
        None if crate::unit_graph::is_unit_out_dir(&out_dir) => format!("{crate_name}."),
        None => return None,
    };
    Some((out_dir, prefix))
}

/// Whether a rustc command line writes object files: it emits them or links,
/// as it does when it names nothing to emit.
fn emits_objects(arguments: &[OsString]) -> bool {
    let mut emits = None::<bool>;
    let mut arguments = arguments.iter().filter_map(|argument| argument.to_str());
    while let Some(argument) = arguments.next() {
        let value = match argument {
            "--emit" => arguments.next(),
            _ => argument.strip_prefix("--emit="),
        };
        if let Some(value) = value {
            let objects = value
                .split(',')
                .any(|kind| matches!(kind.split('=').next(), Some("link" | "obj")));
            emits = Some(emits.unwrap_or(false) || objects);
        }
    }
    emits.unwrap_or(true)
}

#[cfg(test)]
#[path = "object_cleanup_tests.rs"]
mod tests;
