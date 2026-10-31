//! Which managed target directories running programs are using.
//!
//! A view's lease covers the commands mbx starts. A program run from
//! `target/` directly takes none: a benchmark harness that executes
//! `target/debug/app` for an hour, or an end-to-end suite that runs
//! `target/debug/mise`, holds nothing collection can see once the build that
//! produced the binary has exited. Removing the view then leaves every later
//! run failing with "No such file or directory".
//!
//! So a budget collection also asks the operating system. On Linux one walk of
//! `/proc` reads, for each of this user's processes, the executable it runs
//! (`exe`), the files it maps (`maps`, which covers shared libraries and
//! proc-macros) and the files it has open (`fd`). On macOS only the executable
//! is read, through `proc_pidpath`; a file a process there merely has open or
//! mapped is protected by the lease alone. Windows needs neither: it refuses
//! to rename a directory that holds a running executable or an open file, and
//! collection treats that refusal as the directory being in use.
//!
//! The walk is a hint, never a requirement. Anything it cannot read is
//! skipped, and a walk that runs past [`SCAN_LIMIT`] is given up as a whole:
//! a collection that could not finish deciding would free nothing at all,
//! which on a filling disk is the worse failure.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

/// The longest a walk may take before it is abandoned and nothing counts as in
/// use. A walk on a busy machine takes tens of milliseconds.
pub(crate) const SCAN_LIMIT: Duration = Duration::from_secs(2);

/// The view directories, by name, that some running process is using.
#[derive(Debug, Default)]
pub(crate) struct InUse {
    views: HashSet<OsString>,
}

impl InUse {
    /// Whether a process was found using `directory`, a view directly under
    /// the root the scan was given.
    pub(crate) fn holds(&self, directory: &Path) -> bool {
        directory
            .file_name()
            .is_some_and(|name| self.views.contains(name))
    }
}

/// Find the views under `views_root` this user's processes are using.
pub(crate) fn scan(views_root: &Path) -> InUse {
    let started = Instant::now();
    let found = scan_by(Path::new(PROC_ROOT), views_root, started + SCAN_LIMIT);
    log::debug!(
        "found {} target directories in use by running processes in {:?}",
        found.views.len(),
        started.elapsed()
    );
    found
}

/// [`scan`] reading the process table under `proc_root` (Linux only; other
/// platforms ignore it), giving up at `deadline`.
pub(crate) fn scan_by(proc_root: &Path, views_root: &Path, deadline: Instant) -> InUse {
    // Processes report resolved paths: the checkout's `target` link, and any
    // link in the cache path itself, are already followed.
    let Ok(views_root) = std::fs::canonicalize(views_root) else {
        return InUse::default();
    };
    let mut views = HashSet::new();
    let walked = for_each_process_path(proc_root, deadline, &mut |path| {
        if let Some(view) = view_name(&views_root, path) {
            views.insert(view);
        }
    });
    match walked {
        Ok(()) => InUse { views },
        Err(Incomplete::TimedOut) => {
            log::warn!(
                "gave up looking for programs running from managed target directories after {}s; this collection does not protect them",
                SCAN_LIMIT.as_secs()
            );
            InUse::default()
        }
        // Where the process table is never readable, a sandbox or a container
        // without `/proc`, every collection would say so: not a warning.
        Err(Incomplete::Unreadable(error)) => {
            log::debug!(
                "could not list processes to find programs running from managed target directories ({error}); this collection does not protect them"
            );
            InUse::default()
        }
    }
}

/// Why a walk of the process table did not finish. Windows has no walk.
#[derive(Debug)]
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
enum Incomplete {
    /// The deadline passed first.
    TimedOut,
    /// The processes could not be listed at all.
    Unreadable(std::io::Error),
}

/// The name of the view `path` lies in, if it is inside one.
///
/// Views are named by a bare hex digest. Anything else directly under the
/// root, a lock file or a view already moved aside for removal, is not a view
/// collection could still choose.
fn view_name(views_root: &Path, path: &Path) -> Option<OsString> {
    let rest = path.strip_prefix(views_root).ok()?;
    let Some(Component::Normal(name)) = rest.components().next() else {
        return None;
    };
    // `/proc` ends the path of a file unlinked since it was opened with
    // " (deleted)". A binary a rebuild has since replaced still keeps its
    // view, whose name comes before that suffix.
    let name = name.to_str()?;
    (!name.is_empty() && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| OsString::from(name))
}

#[cfg(target_os = "linux")]
const PROC_ROOT: &str = "/proc";
#[cfg(not(target_os = "linux"))]
const PROC_ROOT: &str = "";

/// Call `found` with every path a process of this user executes, maps or has
/// open, unless `deadline` passes first or the process table cannot be read.
#[cfg(target_os = "linux")]
fn for_each_process_path(
    proc_root: &Path,
    deadline: Instant,
    found: &mut dyn FnMut(&Path),
) -> Result<(), Incomplete> {
    use std::ffi::OsStr;
    use std::io::{BufRead, BufReader};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    let processes = std::fs::read_dir(proc_root).map_err(Incomplete::Unreadable)?;
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::geteuid() };
    for process in processes.flatten() {
        if Instant::now() > deadline {
            return Err(Incomplete::TimedOut);
        }
        if !process
            .file_name()
            .as_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        let process = process.path();
        // `/proc/<pid>` belongs to the process's effective user. Another
        // user's processes are theirs to protect, and mostly unreadable.
        if !std::fs::metadata(&process).is_ok_and(|metadata| metadata.uid() == uid) {
            continue;
        }
        match std::fs::read_link(process.join("exe")) {
            Ok(executable) => found(&executable),
            Err(error) => note_unreadable(&process, "exe", &error),
        }
        match std::fs::File::open(process.join("maps")) {
            Ok(maps) => {
                // Read a line at a time: the kernel produces the file as it
                // is read, so a process with many mappings costs time here,
                // and the deadline has to be able to stop it partway.
                let mut maps = BufReader::new(maps);
                let mut line = Vec::new();
                loop {
                    if Instant::now() > deadline {
                        return Err(Incomplete::TimedOut);
                    }
                    line.clear();
                    match maps.read_until(b'\n', &mut line) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(error) => {
                            note_unreadable(&process, "maps", &error);
                            break;
                        }
                    }
                    // `address perms offset dev inode path`: only the path
                    // holds a slash, and anonymous mappings have none.
                    if let Some(start) = line.iter().position(|byte| *byte == b'/') {
                        let path = line[start..].strip_suffix(b"\n").unwrap_or(&line[start..]);
                        found(Path::new(OsStr::from_bytes(path)));
                    }
                }
            }
            Err(error) => note_unreadable(&process, "maps", &error),
        }
        let descriptors = match std::fs::read_dir(process.join("fd")) {
            Ok(descriptors) => descriptors,
            Err(error) => {
                note_unreadable(&process, "fd", &error);
                continue;
            }
        };
        for descriptor in descriptors.flatten() {
            if Instant::now() > deadline {
                return Err(Incomplete::TimedOut);
            }
            match std::fs::read_link(descriptor.path()) {
                Ok(file) => found(&file),
                Err(error) => note_unreadable(&process, "fd", &error),
            }
        }
    }
    // The last process may have taken the walk past its limit.
    if Instant::now() > deadline {
        return Err(Incomplete::TimedOut);
    }
    Ok(())
}

/// Say why a process of this user could not be read, unless it simply exited
/// mid-walk. A security policy refusing the read leaves that process's views
/// unprotected, and this is the only trace of it.
#[cfg(target_os = "linux")]
fn note_unreadable(process: &Path, entry: &str, error: &std::io::Error) {
    if error.kind() != std::io::ErrorKind::NotFound && error.raw_os_error() != Some(libc::ESRCH) {
        log::debug!("could not read {}/{entry}: {error}", process.display());
    }
}

/// Call `found` with the executable of every process of this user, unless
/// `deadline` passes first or the processes cannot be listed.
#[cfg(target_os = "macos")]
fn for_each_process_path(
    _proc_root: &Path,
    deadline: Instant,
    found: &mut dyn FnMut(&Path),
) -> Result<(), Incomplete> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // `PROC_UID_ONLY` from `<sys/proc_info.h>`: processes whose effective
    // user is the one given. The libc crate does not define it.
    const PROC_UID_ONLY: u32 = 4;
    const PID_SIZE: usize = std::mem::size_of::<libc::pid_t>();
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: a null buffer asks only for the size the list needs.
    let needed = unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, std::ptr::null_mut(), 0) };
    let needed = usize::try_from(needed)
        .map_err(|_| Incomplete::Unreadable(std::io::Error::last_os_error()))?;
    // Room for processes started between the two calls.
    let mut pids = vec![0 as libc::pid_t; needed / PID_SIZE + 64];
    let capacity = libc::c_int::try_from(pids.len() * PID_SIZE)
        .map_err(|_| Incomplete::Unreadable(std::io::Error::other("too many processes to list")))?;
    // SAFETY: `pids` is writable for `capacity` bytes, and the call writes no
    // more than that.
    let written =
        unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, pids.as_mut_ptr().cast(), capacity) };
    let written = usize::try_from(written)
        .map_err(|_| Incomplete::Unreadable(std::io::Error::last_os_error()))?;
    pids.truncate(written / PID_SIZE);
    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    for pid in pids {
        if Instant::now() > deadline {
            return Err(Incomplete::TimedOut);
        }
        if pid <= 0 {
            continue;
        }
        // SAFETY: `buffer` is writable for its whole length, which is what
        // the call is told and the most it writes.
        let length =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if let Ok(length) = usize::try_from(length)
            && length > 0
        {
            found(Path::new(OsStr::from_bytes(&buffer[..length])));
        }
    }
    if Instant::now() > deadline {
        return Err(Incomplete::TimedOut);
    }
    Ok(())
}

/// Nothing to read: on Windows the rename collection moves a view aside with
/// fails while anything inside is running or open.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn for_each_process_path(
    _proc_root: &Path,
    _deadline: Instant,
    _found: &mut dyn FnMut(&Path),
) -> Result<(), Incomplete> {
    Ok(())
}

#[cfg(test)]
#[path = "target_processes_tests.rs"]
mod tests;
