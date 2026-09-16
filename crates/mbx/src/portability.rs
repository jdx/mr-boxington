//! Whether a compilation's artifact really is independent of a remapped value.
//!
//! [`crate::rustc::Portable`] makes an action key ignore `OUT_DIR` by remapping
//! it and then reading the outputs back, publishing the portable key only when
//! no output carries the literal path. That is evidence, not proof: a crate
//! that *derives* a value from the path -- `env!("OUT_DIR").len()`, a byte of
//! it, a hash of it -- leaves no literal to find, so two checkouts agree on a
//! key while their artifacts differ, and the second restores the first's.
//!
//! The proof is a second compilation. Build the crate again with the value
//! spelled differently and compare the artifacts: identical means the artifact
//! does not depend on the value and the portable key is honest; differing means
//! it does, and the compilation is keyed literally from then on.
//!
//! The verdict is recorded under the portable action digest, which names the
//! compilation without naming a checkout, so each distinct action pays for the
//! second compilation once rather than once per checkout.

use eyre::{Context, Result};
use mbx_cache_core::CacheDigest;
use std::path::{Path, PathBuf};

/// What a differential compilation concluded about one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The artifact was identical under a different spelling of the value.
    Portable,
    /// It was not, or the comparison could not be trusted.
    CheckoutSpecific,
}

impl Verdict {
    fn marker(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::CheckoutSpecific => "checkout-specific",
        }
    }

    fn parse(contents: &str) -> Option<Self> {
        match contents.trim() {
            "portable" => Some(Self::Portable),
            "checkout-specific" => Some(Self::CheckoutSpecific),
            _ => None,
        }
    }
}

/// Where the verdict for `action` is recorded beneath `store`.
///
/// Sharded on the first byte of the hash: a large workspace records thousands
/// of these, and a single directory that wide is slow to read on every platform
/// that matters. The algorithm and length come along in the file name, so a
/// digest that changes shape cannot collide with one that has not.
pub(crate) fn verdict_path(store: &Path, action: &CacheDigest) -> Option<PathBuf> {
    let shard = action.hash.get(..2)?;
    let name = action.key().replace('/', "-");
    Some(store.join("portability").join(shard).join(name))
}

/// The recorded verdict for `action`, if one has been reached anywhere.
pub(crate) fn recorded(store: &Path, action: &CacheDigest) -> Option<Verdict> {
    let path = verdict_path(store, action)?;
    Verdict::parse(&std::fs::read_to_string(path).ok()?)
}

/// Record what a differential compilation concluded.
///
/// Advisory: a verdict that fails to write costs the next build the second
/// compilation, which is the same cost this session just paid, and never a
/// wrong answer.
pub(crate) fn record(store: &Path, action: &CacheDigest, verdict: Verdict) -> Result<()> {
    let path = verdict_path(store, action)
        .ok_or_else(|| eyre::eyre!("action digest is too short to shard"))?;
    crate::util::write_advisory(&path, verdict.marker().as_bytes()).wrap_err_with(|| {
        format!(
            "failed to record a portability verdict at {}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_survives_a_round_trip() {
        let store = tempfile::tempdir().unwrap();
        let action = CacheDigest::blake3(b"action");
        assert_eq!(recorded(store.path(), &action), None);

        record(store.path(), &action, Verdict::Portable).unwrap();
        assert_eq!(recorded(store.path(), &action), Some(Verdict::Portable));

        record(store.path(), &action, Verdict::CheckoutSpecific).unwrap();
        assert_eq!(
            recorded(store.path(), &action),
            Some(Verdict::CheckoutSpecific)
        );
    }

    /// A verdict file written by a newer mbx, or truncated by a crash, reads as
    /// "not decided yet" rather than as a claim this build has to honour.
    #[test]
    fn an_unreadable_verdict_is_no_verdict() {
        let store = tempfile::tempdir().unwrap();
        let action = CacheDigest::blake3(b"action");
        let path = verdict_path(store.path(), &action).unwrap();
        crate::util::write_advisory(&path, b"something-else").unwrap();

        assert_eq!(recorded(store.path(), &action), None);
    }

    #[test]
    fn verdicts_are_sharded_by_their_action() {
        let store = tempfile::tempdir().unwrap();
        let action = CacheDigest::blake3(b"action");
        let path = verdict_path(store.path(), &action).unwrap();

        assert!(path.starts_with(store.path().join("portability")));
        // One directory level, then one file: a digest whose key contains
        // separators must not turn into a tree of its own.
        let relative = path.strip_prefix(store.path().join("portability")).unwrap();
        assert_eq!(relative.components().count(), 2);
        assert!(
            relative
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&action.hash)
        );
    }
}
