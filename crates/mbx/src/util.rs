use eyre::{Context, Result};
use mbx_cache_core::FileIdentity;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Capture clock-independent identities for compiler inputs known up front.
#[cfg(unix)]
pub(crate) fn snapshot_file_identities<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
) -> BTreeMap<PathBuf, FileIdentity> {
    paths
        .into_iter()
        .filter_map(|path| {
            let metadata = std::fs::metadata(path).ok()?;
            let identity = FileIdentity::describe(path, &metadata)?;
            Some((path.to_path_buf(), identity))
        })
        .collect()
}

#[cfg(not(unix))]
pub(crate) fn snapshot_file_identities<'a>(
    _paths: impl IntoIterator<Item = &'a Path>,
) -> BTreeMap<PathBuf, FileIdentity> {
    BTreeMap::new()
}

/// Find the workspace root above `start`.
///
/// The outermost directory holding a `Cargo.lock` is preferred, since that is
/// the one Cargo resolves against; a directory holding only a `Cargo.toml`
/// stands in when there is no lockfile yet.
pub fn workspace_root(start: &Path) -> std::path::PathBuf {
    let mut lockfile = None;
    let mut manifest = None;
    for directory in start.ancestors() {
        if directory.join("Cargo.lock").is_file() {
            lockfile = Some(directory.to_path_buf());
        }
        if directory.join("Cargo.toml").is_file() {
            manifest = Some(directory.to_path_buf());
        }
        // Delta checkouts are independent source trees nested below the
        // original repository's `.delta/worktrees` directory. Do not let a
        // lockfile in that enclosing repository replace the checkout's own
        // workspace root. Ordinary VCS markers are not boundaries here: a
        // Cargo workspace may legitimately contain a nested repository.
        if is_delta_worktree_root(directory) {
            break;
        }
    }
    lockfile.or(manifest).unwrap_or_else(|| start.to_path_buf())
}

/// Whether `directory` is the root of a supported VCS or managed checkout.
///
/// Marker existence, rather than file type, covers linked/shared checkouts
/// whose metadata marker may be a file or symbolic link. Delta worktrees have
/// no marker of their own, so their managed location establishes the boundary.
pub fn is_checkout_root(directory: &Path) -> bool {
    [".git", ".jj", ".hg", ".sl"]
        .iter()
        .any(|marker| directory.join(marker).exists())
        || is_delta_worktree_root(directory)
}

fn is_delta_worktree_root(directory: &Path) -> bool {
    directory
        .parent()
        .is_some_and(|parent| parent.file_name() == Some(std::ffi::OsStr::new("worktrees")))
        && directory
            .parent()
            .and_then(Path::parent)
            .is_some_and(|parent| parent.file_name() == Some(std::ffi::OsStr::new(".delta")))
}

/// Write `contents` to `path` so readers never observe a partial file.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| eyre::eyre!("path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".mbx-")
        .tempfile_in(parent)?;
    temporary.write_all(contents)?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .wrap_err_with(|| format!("failed atomic write: {}", path.display()))?;
    Ok(())
}

/// Format a duration for humans, widening precision as the duration grows.
pub fn format_duration(duration: Duration) -> String {
    if duration < Duration::from_millis(1) {
        format!("{duration:.0?}")
    } else if duration < Duration::from_secs(1) {
        format!("{duration:.1?}")
    } else {
        format!("{duration:.2?}")
    }
}

/// Format a long span the way a retention policy is written.
///
/// [`format_duration`] measures what a build spent and would render a month as
/// `2592000.00s`. A policy is stated in coarse units, so this says "30 days".
///
/// The largest unit that divides evenly, never one that would round: this
/// describes when files get deleted, and calling 36 hours "1 day" would
/// understate that by a third.
pub fn format_span(duration: Duration) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let seconds = duration.as_secs();
    let (amount, unit) = [(DAY, "day"), (HOUR, "hour"), (MINUTE, "minute")]
        .into_iter()
        .find(|(size, _)| seconds >= *size && seconds.is_multiple_of(*size))
        .map_or((seconds, "second"), |(size, unit)| (seconds / size, unit));
    if amount == 1 {
        format!("1 {unit}")
    } else {
        format!("{amount} {unit}s")
    }
}

/// Parse a duration written as a bare number of seconds or with a unit suffix.
pub fn parse_duration(value: &str) -> Result<Duration> {
    let value = value.trim();
    let (digits, multiplier) = if let Some(digits) = value.strip_suffix("ms") {
        (digits, 1)
    } else if let Some(digits) = value.strip_suffix('s') {
        (digits, 1_000)
    } else if let Some(digits) = value.strip_suffix('m') {
        (digits, 60 * 1_000)
    } else if let Some(digits) = value.strip_suffix('h') {
        (digits, 60 * 60 * 1_000)
    } else if let Some(digits) = value.strip_suffix('d') {
        (digits, 24 * 60 * 60 * 1_000)
    } else {
        (value, 1_000)
    };
    let amount: u64 = digits
        .trim()
        .parse()
        .wrap_err_with(|| format!("invalid duration: {value}"))?;
    Ok(Duration::from_millis(amount.saturating_mul(multiplier)))
}

pub fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

/// The size of the filesystem holding `path`, or the nearest existing ancestor.
///
/// Budgets scale with the disk, so this answers "how big is the disk mbx is
/// about to fill". The nearest existing ancestor is what gets measured because
/// the cache directory usually does not exist yet the first time this is asked.
/// `None` means the question could not be answered, and every caller has a
/// fixed budget to fall back on: guessing a disk size would be worse than
/// admitting the probe failed.
pub fn disk_total_bytes(path: &Path) -> Option<u64> {
    let existing = path.ancestors().find(|ancestor| ancestor.exists())?;
    disk_total_bytes_at(existing)
}

/// Apple's `statvfs` counts blocks in a 32-bit field, which wraps somewhere
/// above 16TiB and would answer with a smaller disk than the one it measured --
/// quietly sizing a budget from a fraction of a large volume. `statfs` is the
/// native call there and counts in 64 bits.
#[cfg(all(unix, target_vendor = "apple"))]
fn disk_total_bytes_at(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid NUL-terminated C string that outlives the call,
    // and `statfs` only writes into the buffer it is given.
    let stats = unsafe {
        let mut stats = std::mem::zeroed::<libc::statfs>();
        if libc::statfs(path.as_ptr(), &mut stats) != 0 {
            return None;
        }
        stats
    };
    // A zero product falls back rather than claiming an empty disk.
    u64::from(stats.f_bsize)
        .checked_mul(stats.f_blocks)
        .filter(|total| *total > 0)
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn disk_total_bytes_at(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid NUL-terminated C string that outlives the call,
    // and `statvfs` only writes into the buffer it is given.
    let stats = unsafe {
        let mut stats = std::mem::zeroed::<libc::statvfs>();
        if libc::statvfs(path.as_ptr(), &mut stats) != 0 {
            return None;
        }
        stats
    };
    // `f_frsize` is the fundamental block size `f_blocks` counts in. It is
    // reported as 0 by some filesystems, which would silently answer "0 bytes",
    // so a zero product falls back rather than claiming an empty disk.
    //
    // Both fields are already `u64` on 64-bit glibc, where these conversions do
    // nothing, but they narrow on 32-bit targets -- the conversions are what let
    // one body compile everywhere CI builds it.
    #[allow(clippy::useless_conversion)]
    let block_size = u64::try_from(stats.f_frsize).ok()?;
    #[allow(clippy::useless_conversion)]
    let blocks = u64::try_from(stats.f_blocks).ok()?;
    block_size.checked_mul(blocks).filter(|total| *total > 0)
}

#[cfg(windows)]
fn disk_total_bytes_at(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut total = 0_u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call, and the out
    // parameters are either null (ignored by the API) or a valid `u64`.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            &mut total,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        None
    } else {
        Some(total).filter(|total| *total > 0)
    }
}

/// Physical memory installed in this machine, when it can be measured.
#[cfg(target_vendor = "apple")]
pub fn memory_total_bytes() -> Option<u64> {
    let mut mib = [libc::CTL_HW, libc::HW_MEMSIZE];
    let mut total = 0_u64;
    let mut length = size_of::<u64>();
    // SAFETY: the MIB names a u64 sysctl, `total` is a valid u64 out buffer of
    // exactly `length` bytes, and no new value is being set.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            (&raw mut total).cast(),
            &raw mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    (ok == 0).then_some(total).filter(|total| *total > 0)
}

/// Memory this process may actually use, when it can be measured.
///
/// The container's limit, where there is one, rather than the host's RAM. A
/// build inside a 4GiB container on a 128GiB machine is budgeted by the 4GiB,
/// because the other 124GiB are not its to spend and reaching for them is an
/// out-of-memory kill rather than a slow build.
#[cfg(target_os = "linux")]
pub fn memory_total_bytes() -> Option<u64> {
    // SAFETY: `sysinfo` only writes into the zeroed struct it is given.
    let info = unsafe {
        let mut info = std::mem::zeroed::<libc::sysinfo>();
        if libc::sysinfo(&mut info) != 0 {
            return None;
        }
        info
    };
    // `mem_unit` narrows from u32 and `totalram` is u64 on 64-bit glibc but
    // u32 on 32-bit targets; the conversions let one body compile everywhere.
    #[allow(clippy::useless_conversion)]
    let unit = u64::from(info.mem_unit);
    #[allow(clippy::useless_conversion)]
    let total = u64::try_from(info.totalram).ok()?;
    let physical = total.checked_mul(unit).filter(|total| *total > 0)?;
    Some(match cgroup_memory_limit() {
        Some(limit) => physical.min(limit),
        None => physical,
    })
}

/// The memory ceiling this process's cgroup imposes, if it imposes one.
///
/// Read at the root of the cgroup filesystem, which is what a container sees
/// of its own limit through a cgroup namespace -- the ordinary Docker,
/// Podman, and Kubernetes arrangement. A process placed in a nested cgroup
/// *without* such a namespace reads the root's limit instead and is budgeted
/// as though the nesting were not there; that is the same answer it had
/// before any of this, so it loses nothing.
#[cfg(target_os = "linux")]
fn cgroup_memory_limit() -> Option<u64> {
    // v2 states "max" for unlimited; v1 states a number so large it means the
    // same thing, so anything at or above the host's addressable range is
    // treated as no limit rather than as a budget.
    read_cgroup_value("/sys/fs/cgroup/memory.max")
        .or_else(|| read_cgroup_value("/sys/fs/cgroup/memory/memory.limit_in_bytes"))
}

/// Memory the cgroup is holding that the kernel would not simply reclaim.
///
/// Not `memory.current`, which counts the page cache. A build fills that
/// immediately -- every source it reads and every artifact it writes lands
/// there -- and nearly all of it is evictable, so a container that had merely
/// compiled something would look as though it were out of memory. Subtracting
/// the inactive file pages leaves the working set, which is the same figure
/// Kubernetes reports for a container and the one worth comparing a
/// compilation's appetite against.
#[cfg(target_os = "linux")]
fn cgroup_memory_usage() -> Option<u64> {
    let current = read_cgroup_amount("/sys/fs/cgroup/memory.current")
        .or_else(|| read_cgroup_amount("/sys/fs/cgroup/memory/memory.usage_in_bytes"))?;
    // Absent or unreadable stats leave the page cache counted, which errs
    // toward deferring a compilation rather than toward an OOM kill.
    let reclaimable = read_cgroup_stat("/sys/fs/cgroup/memory.stat", "inactive_file")
        .or_else(|| read_cgroup_stat("/sys/fs/cgroup/memory/memory.stat", "total_inactive_file"))
        .unwrap_or(0);
    Some(current.saturating_sub(reclaimable))
}

/// One cgroup limit, or `None` for absent, unparseable, or "no limit".
#[cfg(target_os = "linux")]
fn read_cgroup_value(path: &str) -> Option<u64> {
    read_cgroup_text(&std::fs::read_to_string(path).ok()?)
}

/// One plain cgroup number, where zero is a reading rather than an absence.
#[cfg(target_os = "linux")]
fn read_cgroup_amount(path: &str) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// One field of a cgroup `memory.stat`, which is `name value` per line.
#[cfg(target_os = "linux")]
fn read_cgroup_stat(path: &str, field: &str) -> Option<u64> {
    parse_cgroup_stat(&std::fs::read_to_string(path).ok()?, field)
}

/// The parsing half, separated so it can be tested without a cgroup mount.
#[cfg(any(target_os = "linux", test))]
fn parse_cgroup_stat(contents: &str, field: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let (name, value) = line.split_once(' ')?;
        (name == field).then(|| value.trim().parse().ok())?
    })
}

/// The parsing half, separated so it can be tested without a cgroup mount.
#[cfg(any(target_os = "linux", test))]
fn read_cgroup_text(contents: &str) -> Option<u64> {
    let value = contents.trim();
    if value == "max" {
        return None;
    }
    // cgroup v1 spells "unlimited" as a page-aligned number near i64::MAX
    // rather than in words. The largest machines built hold tens of
    // tebibytes, so anything past a pebibyte is that idiom rather than a
    // budget worth dividing into permits.
    const NO_LIMIT: u64 = 1 << 50;
    value
        .parse::<u64>()
        .ok()
        .filter(|bytes| *bytes > 0 && *bytes < NO_LIMIT)
}

/// Physical memory installed in this machine, when it can be measured.
#[cfg(windows)]
pub fn memory_total_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    // SAFETY: the struct is a valid out parameter with its length declared,
    // which is all the API requires.
    let status = unsafe {
        let mut status = std::mem::zeroed::<MEMORYSTATUSEX>();
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 {
            return None;
        }
        status
    };
    Some(status.ullTotalPhys).filter(|total| *total > 0)
}

/// Fallback for hosts with no supported probe.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", windows)))]
pub fn memory_total_bytes() -> Option<u64> {
    None
}

/// Memory the machine could hand a new process right now, when measurable.
///
/// "Available" deliberately counts reclaimable memory rather than free pages
/// alone: a machine whose RAM is full of page cache is not out of memory, and
/// treating it as such would defer compilations that were going to run fine.
#[cfg(target_os = "linux")]
pub fn memory_available_bytes() -> Option<u64> {
    // `MemAvailable` is the kernel's own estimate of exactly this question,
    // accounting for reclaimable caches; summing fields by hand would just
    // reimplement it worse. It describes the host, though -- `/proc/meminfo`
    // is not namespaced -- so a container's own headroom bounds it below.
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?;
    let kibibytes = line
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .ok()?;
    let host = kibibytes.checked_mul(1024)?;
    let available = match (cgroup_memory_limit(), cgroup_memory_usage()) {
        (Some(limit), Some(used)) => host.min(limit.saturating_sub(used)),
        (Some(limit), None) => host.min(limit),
        _ => host,
    };
    // Zero is a real answer here -- a cgroup at its limit has nothing left --
    // and the caller has to be able to tell it from "cannot measure".
    Some(available)
}

/// Memory the machine could hand a new process right now, when measurable.
//
// libc deprecates its Mach bindings in favor of the `mach2` crate, but these
// four symbols are stable kernel ABI and not worth a dependency that exists
// to re-export them.
#[allow(deprecated)]
#[cfg(target_vendor = "apple")]
pub fn memory_available_bytes() -> Option<u64> {
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: the buffer is a zeroed `vm_statistics64` and `count` declares
    // its size in `integer_t` units, which is what the call requires.
    let stats = unsafe {
        let mut stats = std::mem::zeroed::<libc::vm_statistics64>();
        if libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            (&raw mut stats).cast(),
            &mut count,
        ) != libc::KERN_SUCCESS
        {
            return None;
        }
        stats
    };
    // Free plus what the kernel can take back without swapping: inactive pages
    // and purgeable memory. Speculative pages are already inside `free_count`.
    // SAFETY: reading a kernel-exported integer.
    let page_size = unsafe { libc::vm_page_size } as u64;
    let pages = u64::from(stats.free_count)
        .checked_add(u64::from(stats.inactive_count))?
        .checked_add(u64::from(stats.purgeable_count))?;
    pages.checked_mul(page_size).filter(|bytes| *bytes > 0)
}

/// Fallback for hosts with no supported probe.
#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
pub fn memory_available_bytes() -> Option<u64> {
    None
}

/// Whether a reflink from inside `source_dir` can land inside
/// `destination_dir`.
///
/// Answered by doing one, not by guessing from the platform: reflink support
/// is a property of the mounted filesystem, and it takes both ends -- a store
/// on btrfs cannot reflink into a target directory on ext4, so probing one
/// side alone proves nothing about the copy that actually happens. The
/// destination is anchored at its nearest existing ancestor rather than
/// created, because the real directory may be one cargo has not made yet and
/// making it here would change what target placement later sees. Any failure
/// answers no: every caller is deciding whether to promise sharing, and a
/// promise needs more than a maybe.
pub fn reflinks_work(source_dir: &Path, destination_dir: &Path) -> bool {
    fn probe(source_dir: &Path, destination_dir: &Path) -> Option<()> {
        std::fs::create_dir_all(source_dir).ok()?;
        let anchor = destination_dir
            .ancestors()
            .find(|ancestor| ancestor.exists())?;
        let source_temp = tempfile::tempdir_in(source_dir).ok()?;
        let destination_temp = tempfile::tempdir_in(anchor).ok()?;
        let source = source_temp.path().join("source");
        std::fs::write(&source, b"mbx reflink probe").ok()?;
        reflink_copy::reflink(&source, destination_temp.path().join("destination")).ok()
    }
    probe(source_dir, destination_dir).is_some()
}

/// Generate an unpredictable alphanumeric string.
///
/// Only the Windows named-pipe endpoint needs this, but it is compiled
/// everywhere so that every platform's CI type-checks it.
pub fn random_string(length: usize) -> String {
    use rand::RngExt as _;
    use rand::distr::Alphanumeric;

    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgroup_usage_discounts_the_page_cache() {
        // The shape of a real `memory.stat`: the field wanted is neither
        // first nor last, and a prefix of its name must not match it.
        let stat = "anon 1000\nfile 8000\nkernel 12\ninactive_file 7000\nslab 40\n";
        assert_eq!(parse_cgroup_stat(stat, "inactive_file"), Some(7000));
        assert_eq!(parse_cgroup_stat(stat, "anon"), Some(1000));
        // `file` must not be answered by `inactive_file`.
        assert_eq!(parse_cgroup_stat(stat, "file"), Some(8000));
        assert_eq!(parse_cgroup_stat(stat, "absent"), None);
        assert_eq!(parse_cgroup_stat("malformed\n", "anon"), None);
    }

    #[test]
    fn cgroup_limits_are_read_and_no_limit_is_recognized() {
        assert_eq!(
            read_cgroup_text("4294967296\n"),
            Some(4 * 1024 * 1024 * 1024)
        );
        // cgroup v2 spells "unlimited" in words.
        assert_eq!(read_cgroup_text("max\n"), None);
        // cgroup v1 spells it as a number nothing real could reach. Budgeting
        // permits out of it would be the same as having no limit, but with
        // arithmetic in between.
        assert_eq!(read_cgroup_text("9223372036854771712"), None);
        assert_eq!(read_cgroup_text("0"), None);
        assert_eq!(read_cgroup_text("not a number"), None);
        assert_eq!(read_cgroup_text(""), None);
    }

    #[test]
    fn writes_files_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("report.json");
        write_atomic(&path, b"{}\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}\n");

        write_atomic(&path, b"[]\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"[]\n");
    }

    #[test]
    fn random_strings_have_the_requested_length_and_differ() {
        let first = random_string(12);
        assert_eq!(first.chars().count(), 12);
        assert!(first.chars().all(|character| character.is_alphanumeric()));
        assert_ne!(first, random_string(12));
        assert!(random_string(0).is_empty());
    }

    #[test]
    fn measures_the_disk_holding_a_path() {
        let directory = tempfile::tempdir().unwrap();
        let total = disk_total_bytes(directory.path()).expect("the test disk has a size");
        assert!(total > 0);
        // The directory need not exist: budgets are resolved before the cache
        // directory is created.
        let missing = directory.path().join("not").join("created").join("yet");
        assert_eq!(disk_total_bytes(&missing), Some(total));
    }

    #[test]
    fn the_reflink_probe_anchors_a_destination_nobody_created_yet() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("cache");
        // The target directory does not exist before the first build; the
        // probe must answer for the filesystem it will be created on, without
        // creating it.
        let unmade = directory.path().join("checkout").join("target");
        assert_eq!(
            reflinks_work(&store, &unmade),
            reflinks_work(&store, directory.path()),
            "a missing destination answers as its filesystem, not as a failure"
        );
        assert!(!unmade.exists(), "the probe must not create the directory");
    }

    #[test]
    fn formats_spans_in_the_units_a_policy_is_written_in() {
        assert_eq!(format_span(Duration::from_secs(30 * 86_400)), "30 days");
        assert_eq!(format_span(Duration::from_secs(86_400)), "1 day");
        // A policy of 36 hours is not "1 day": this text says when files go.
        assert_eq!(format_span(Duration::from_secs(36 * 3_600)), "36 hours");
        assert_eq!(format_span(Duration::from_secs(90 * 60)), "90 minutes");
        assert_eq!(format_span(Duration::from_secs(45)), "45 seconds");
        assert_eq!(format_span(Duration::from_secs(7_200)), "2 hours");
        assert_eq!(format_span(Duration::from_secs(60)), "1 minute");
        assert_eq!(format_span(Duration::from_secs(90)), "90 seconds");
        assert_eq!(format_span(Duration::from_secs(5)), "5 seconds");
    }

    #[test]
    fn parses_durations_with_and_without_units() {
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7_200));
        assert!(parse_duration("soon").is_err());
    }
}
