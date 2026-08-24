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
    fn parses_durations_with_and_without_units() {
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7_200));
        assert!(parse_duration("soon").is_err());
    }
}
