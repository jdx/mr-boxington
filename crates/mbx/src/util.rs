use eyre::{Context, Result};
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

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
    }
    lockfile.or(manifest).unwrap_or_else(|| start.to_path_buf())
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

/// Whether the filesystem holding `dir` can reflink.
///
/// Answered by doing one, not by guessing from the platform: reflink support
/// is a property of the mounted filesystem, and XFS on one machine has it
/// where ext4 on the next does not. Any failure -- including `dir` not being
/// creatable -- answers no, because every caller is deciding whether to
/// promise sharing, and a promise needs more than a maybe.
pub fn reflinks_work(dir: &Path) -> bool {
    fn probe(dir: &Path) -> Option<()> {
        std::fs::create_dir_all(dir).ok()?;
        let directory = tempfile::tempdir_in(dir).ok()?;
        let source = directory.path().join("source");
        std::fs::write(&source, b"mbx reflink probe").ok()?;
        reflink_copy::reflink(&source, directory.path().join("destination")).ok()
    }
    probe(dir).is_some()
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
