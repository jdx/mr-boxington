//! A compile-time reminder to delete code that was only meant to live for a
//! while, such as a fallback for a tool's older releases.

/// Whether the dotted number `a` is older than `b`, comparing each number in
/// order; a missing number counts as 0 and a suffix such as `-rc.1` is ignored.
/// For mbx's own `X.Y.Z` versions, at compile time.
pub(crate) const fn version_lt(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let (mut x, mut y) = (0u64, 0u64);
        while i < a.len() && a[i].is_ascii_digit() {
            x = x * 10 + (a[i] - b'0') as u64;
            i += 1;
        }
        while j < b.len() && b[j].is_ascii_digit() {
            y = y * 10 + (b[j] - b'0') as u64;
            j += 1;
        }
        if x != y {
            return x < y;
        }
        // Past this number's suffix, if any, and its dot.
        while i < a.len() && a[i] != b'.' {
            i += 1;
        }
        while j < b.len() && b[j] != b'.' {
            j += 1;
        }
        i += 1;
        j += 1;
    }
    false
}

/// Fails the build once mbx's own version reaches `remove_at`, so the release
/// that bumps the version cannot build until the code carrying the marker is
/// removed. Remove every `remove_by!` with the same id along with it.
///
/// ```ignore
/// remove_by!("2.0.0", "ra-jit-fallback");
/// ```
macro_rules! remove_by {
    ($remove_at:literal, $id:literal) => {
        const _: () = assert!(
            $crate::remove_by::version_lt(env!("CARGO_PKG_VERSION"), $remove_at),
            concat!(
                "[",
                $id,
                "] should have been removed by mbx ",
                $remove_at,
                ". Remove it, and every other remove_by! with this id."
            )
        );
    };
}
pub(crate) use remove_by;

#[cfg(test)]
mod tests {
    use super::version_lt;

    #[test]
    fn version_lt_compares_each_number() {
        assert!(version_lt("1.29.2", "2.0.0"));
        assert!(version_lt("1.9.9", "1.10.0"));
        assert!(version_lt("2.0", "2.0.1"));
        assert!(version_lt("1.99.0-rc.1", "2.0.0"));
        assert!(!version_lt("2.0.0", "2.0.0"));
        assert!(!version_lt("2.0.0-rc.1", "2.0.0"));
        assert!(!version_lt("2.0", "2.0.0"));
        assert!(!version_lt("2.1.0", "2.0.9"));
    }
}
