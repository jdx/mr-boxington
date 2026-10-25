use crate::{CacheDigest, RemoteActionResult, canonical_json};
use eyre::{Result, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Why [`LocalActionCache::store`] refuses to replace an intact result.
pub const ACTION_CONFLICT_MESSAGE: &str = "local action key already has a different result";

/// A validated, content-addressed store on the local filesystem.
#[derive(Debug, Clone)]
pub struct LocalCas {
    root: PathBuf,
}

/// A local index from action digests to their referenced cache objects.
#[derive(Debug, Clone)]
pub struct LocalActionCache {
    root: PathBuf,
    cas: LocalCas,
}

/// The result of publishing an action with first-writer semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStoreOutcome {
    /// Published a new result or repaired an invalid existing result.
    Stored,
    /// The same result was already present.
    Unchanged,
    /// An intact, restorable, different result was already present and was kept.
    KeptExisting,
}

impl LocalCas {
    /// Create a local content-addressed store beneath `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Return the root shared by this store and its action-result index.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve the storage path for a validated digest.
    pub fn path_for(&self, digest: &CacheDigest) -> Result<PathBuf> {
        digest.validate()?;
        Ok(self
            .root
            .join("cas/v1")
            .join(&digest.algorithm)
            .join(&digest.hash[..2])
            .join(format!("{}-{}", digest.hash, digest.size)))
    }

    /// Find and verify a stored object.
    pub fn find(&self, digest: &CacheDigest) -> Result<Option<PathBuf>> {
        let path = self.path_for(digest)?;
        if !path.exists() {
            return Ok(None);
        }
        if !digest.matches_file(&path)? {
            bail!(
                "local CAS blob failed digest verification: {}",
                path.display()
            );
        }
        Ok(Some(path))
    }

    /// Atomically store bytes after verifying their declared digest.
    pub fn store_bytes(&self, digest: &CacheDigest, bytes: &[u8]) -> Result<PathBuf> {
        if !digest.matches_bytes(bytes)? {
            bail!("bytes do not match the declared CAS digest");
        }
        self.store_with(digest, |temporary| {
            temporary.write_all(bytes)?;
            Ok(())
        })
    }

    /// Atomically store a file after verifying its declared digest.
    pub fn store_file(&self, digest: &CacheDigest, source: &Path) -> Result<PathBuf> {
        self.store_file_inner(digest, source, true)
    }

    /// Store a file whose digest was already verified by this crate.
    pub(crate) fn store_verified_file(
        &self,
        digest: &CacheDigest,
        source: &Path,
    ) -> Result<PathBuf> {
        self.store_file_inner(digest, source, false)
    }

    /// Move an already-verified temporary file into the CAS when possible.
    ///
    /// Remote downloads are staged beneath the cache root, so the usual path
    /// is an atomic same-filesystem rename. If a caller supplies a path on a
    /// different filesystem, fall back to the copy-based store operation.
    pub fn adopt_verified_file(&self, digest: &CacheDigest, source: &Path) -> Result<PathBuf> {
        if fs::metadata(source)?.len() != digest.size {
            bail!("staged blob size does not match the declared CAS digest");
        }
        let destination = self.path_for(digest)?;
        match self.find(digest) {
            Ok(Some(existing)) => return Ok(existing),
            // Preserve the established repair path for a poisoned destination;
            // replacing it portably needs the copy-based temporary file flow.
            Err(_) => return self.store_file_inner(digest, source, false),
            Ok(None) => {}
        }
        fs::create_dir_all(destination.parent().expect("CAS path has a parent"))?;
        make_owner_writable(source)?;
        match fs::rename(source, &destination) {
            Ok(()) => Ok(destination),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => self
                .find(digest)?
                .ok_or_else(|| eyre::eyre!("concurrent CAS write did not publish a valid blob")),
            Err(_) => self.store_file_inner(digest, source, false),
        }
    }

    fn store_file_inner(
        &self,
        digest: &CacheDigest,
        source: &Path,
        verify: bool,
    ) -> Result<PathBuf> {
        let destination = self.path_for(digest)?;
        // A blob that fails verification cannot be restored from, and nothing
        // else repairs it: readers treat it as a miss, so without republishing
        // over it the digest stays poisoned until eviction happens to reclaim
        // it. `LocalActionCache::store` already recovers this way one layer up.
        let replace_invalid = match self.find(digest) {
            Ok(Some(existing)) => return Ok(existing),
            Ok(None) => false,
            Err(_) => true,
        };
        // On Unix the source is hashed before it is copied, and the copy is
        // only trusted if the source still looks the same afterwards. A
        // compiler output was written moments ago, so its pages are hot; the
        // reflinked copy is a fresh inode whose pages are not, and hashing it
        // instead read every published byte back from disk. A source
        // rewritten between the hash and the copy shows in its change time,
        // which nothing in user space can set back, and is refused, so what
        // lands under the digest is what was hashed. Windows reports no
        // change time and lets a writer restore the modification time, so
        // there the copy itself is hashed, as it always was.
        let hash_source = verify && cfg!(unix);
        let before = hash_source.then(|| fs::metadata(source)).transpose()?;
        if hash_source && !digest.matches_file(source)? {
            bail!("staged blob does not match the declared CAS digest");
        }
        let parent = destination.parent().expect("CAS path has a parent");
        fs::create_dir_all(parent)?;
        let staging = tempfile::tempdir_in(parent)?;
        let temporary = staging.path().join("blob");
        reflink_copy::reflink_or_copy(source, &temporary)?;
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        make_owner_writable(&temporary)?;
        if let Some(before) = before
            && !same_identity(&before, &fs::metadata(source)?)
        {
            bail!("source changed while it was being stored in the CAS");
        }
        if verify && !hash_source && !digest.matches_file(&temporary)? {
            bail!("staged blob does not match the declared CAS digest");
        }
        // Not fsynced: every read verifies the digest, so a blob torn by a
        // crash is detected and treated as absent rather than trusted.
        if fs::metadata(&temporary)?.len() != digest.size {
            bail!("staged blob size does not match the declared CAS digest");
        }
        if replace_invalid {
            temporary
                .persist(&destination)
                .map_err(|error| error.error)?;
            return Ok(destination);
        }
        match temporary.persist_noclobber(&destination) {
            Ok(()) => Ok(destination),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => self
                .find(digest)?
                .ok_or_else(|| eyre::eyre!("concurrent CAS write did not publish a valid blob")),
            Err(error) => Err(error.error.into()),
        }
    }

    fn store_with(
        &self,
        digest: &CacheDigest,
        write: impl FnOnce(&mut tempfile::NamedTempFile) -> Result<()>,
    ) -> Result<PathBuf> {
        let destination = self.path_for(digest)?;
        // See `store_file_inner` for why an invalid blob is replaced.
        let replace_invalid = match self.find(digest) {
            Ok(Some(existing)) => return Ok(existing),
            Ok(None) => false,
            Err(_) => true,
        };
        let parent = destination.parent().expect("CAS path has a parent");
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        write(&mut temporary)?;
        temporary.flush()?;
        if !digest.matches_file(temporary.path())? {
            bail!("staged blob does not match the declared CAS digest");
        }
        if replace_invalid {
            temporary
                .persist(&destination)
                .map_err(|error| error.error)?;
            return Ok(destination);
        }
        match temporary.persist_noclobber(&destination) {
            Ok(_) => Ok(destination),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => self
                .find(digest)?
                .ok_or_else(|| eyre::eyre!("concurrent CAS write did not publish a valid blob")),
            Err(error) => Err(error.error.into()),
        }
    }
}

/// Whether two views of a file could describe the same unmodified bytes.
///
/// Length and modification time are what a rewrite through any path changes,
/// and the change time is what a writer cannot set back. Only Unix reports
/// one, which is why the store trusts this check only there.
fn same_identity(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return false;
        }
    }
    true
}

#[cfg(unix)]
fn make_owner_writable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o200);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

// The lint warns that this makes a Unix file world-writable. Windows has no
// such mode: it clears only the read-only attribute.
#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn make_owner_writable(path: &Path) -> Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

impl LocalActionCache {
    /// Create an action-result index beneath `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            cas: LocalCas::new(root.clone()),
            root,
        }
    }

    /// Resolve the storage path for an action digest.
    pub fn path_for(&self, action: &CacheDigest) -> Result<PathBuf> {
        action.validate()?;
        if action.algorithm != "blake3" {
            bail!("local action keys must use blake3");
        }
        Ok(self
            .root
            .join("action-results/v1")
            .join(&action.algorithm)
            .join(&action.hash[..2])
            .join(format!("{}-{}.json", action.hash, action.size)))
    }

    /// Find and strictly validate a canonical action result.
    pub fn find(&self, action: &CacheDigest) -> Result<Option<RemoteActionResult>> {
        let path = self.path_for(action)?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        Ok(Some(Self::parse_record(&bytes, action)?))
    }

    fn parse_record(bytes: &[u8], action: &CacheDigest) -> Result<RemoteActionResult> {
        let result: RemoteActionResult = serde_json::from_slice(bytes)?;
        if result.version != 1 || result.action != *action || canonical_json(&result)? != bytes {
            bail!("local action result is invalid");
        }
        Ok(result)
    }

    /// Whether the objects `result` names that this index can see are all
    /// present and verified: its metadata, its output directory and the files
    /// that directory lists.
    fn is_intact(&self, result: &RemoteActionResult) -> bool {
        let present = |digest: &CacheDigest| matches!(self.cas.find(digest), Ok(Some(_)));
        present(&result.action)
            && result.metadata.as_ref().is_none_or(present)
            && result.output_root.as_ref().is_none_or(|root| {
                self.directory_is_intact(root, &mut std::collections::BTreeSet::new())
            })
    }

    /// Whether `digest` names a directory object whose files and nested
    /// directories are all present and verified.
    fn directory_is_intact(
        &self,
        digest: &CacheDigest,
        seen: &mut std::collections::BTreeSet<CacheDigest>,
    ) -> bool {
        if !seen.insert(digest.clone()) {
            return true;
        }
        let Ok(Some(path)) = self.cas.find(digest) else {
            return false;
        };
        let Ok(bytes) = fs::read(path) else {
            return false;
        };
        let Ok(directory) = serde_json::from_slice::<crate::CacheDirectory>(&bytes) else {
            return false;
        };
        directory
            .files
            .iter()
            .all(|file| matches!(self.cas.find(&file.digest), Ok(Some(_))))
            && directory
                .directories
                .iter()
                .all(|child| self.directory_is_intact(&child.digest, seen))
    }

    /// Atomically publish an action result after validating all referenced objects.
    pub fn store(&self, result: &RemoteActionResult) -> Result<PathBuf> {
        self.store_inner(result, false, &|_| true)?;
        self.path_for(&result.action)
    }

    /// Publish a result unless an intact existing result is also restorable.
    /// The caller checks any dependencies this index does not understand.
    /// Invalid or unrestorable existing records are repaired as in [`Self::store`].
    pub fn store_unless_restorable(
        &self,
        result: &RemoteActionResult,
        is_restorable: impl Fn(&RemoteActionResult) -> bool,
    ) -> Result<ActionStoreOutcome> {
        self.store_inner(result, true, &is_restorable)
    }

    fn store_inner(
        &self,
        result: &RemoteActionResult,
        keep_existing: bool,
        is_restorable: &impl Fn(&RemoteActionResult) -> bool,
    ) -> Result<ActionStoreOutcome> {
        if result.version != 1 {
            bail!("unsupported local action result version");
        }
        for digest in [
            Some(&result.action),
            result.metadata.as_ref(),
            result.output_root.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if self.cas.find(digest)?.is_none() {
                bail!("cannot publish an action result with a missing blob");
            }
        }
        let destination = self.path_for(&result.action)?;
        let parent = destination
            .parent()
            .expect("action-result path has a parent");
        fs::create_dir_all(parent)?;
        // These outcomes do not mutate the record and need no slot lock.
        let decide = |existing: &RemoteActionResult| -> Result<Option<ActionStoreOutcome>> {
            if existing == result {
                return Ok(Some(ActionStoreOutcome::Unchanged));
            }
            // A different intact record keeps the first-writer rule unless
            // the caller establishes that it cannot be restored.
            if self.is_intact(existing) {
                if !keep_existing {
                    bail!("{ACTION_CONFLICT_MESSAGE}");
                }
                if is_restorable(existing) {
                    return Ok(Some(ActionStoreOutcome::KeptExisting));
                }
            }
            Ok(None)
        };
        let mut pending = None;
        for _ in 0..8 {
            if let Ok(Some(existing)) = self.find(&result.action)
                && let Some(outcome) = decide(&existing)?
            {
                return Ok(outcome);
            }
            let slot = crate::ActionSlot::claim(&destination)?;
            // A writer may have repaired the record since our lock-free read.
            if let Some(bytes) = slot.read()?
                && let Ok(existing) = Self::parse_record(&bytes, &result.action)
                && let Some(outcome) = decide(&existing)?
            {
                return Ok(outcome);
            }
            let temporary = if let Some(temporary) = pending.take() {
                temporary
            } else {
                let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
                temporary.write_all(&canonical_json(result)?)?;
                temporary.flush()?;
                temporary
            };
            if slot.is_vacant() {
                match slot.publish(temporary)? {
                    crate::action_slot::Publish::Stored => return Ok(ActionStoreOutcome::Stored),
                    crate::action_slot::Publish::Taken(temporary) => pending = Some(temporary),
                }
            } else {
                slot.replace(temporary)?;
                return Ok(ActionStoreOutcome::Stored);
            }
        }
        bail!("action record kept changing while publishing")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_repair_keeps_the_winning_record() {
        use crate::action_slot::tests::Holder;
        use std::sync::mpsc;
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let actions = LocalActionCache::new(directory.path());
        let action = CacheDigest::blake3(b"action");
        cas.store_bytes(&action, b"action").unwrap();
        let metadata = CacheDigest::blake3(b"metadata");
        cas.store_bytes(&metadata, b"metadata").unwrap();
        let first = RemoteActionResult {
            version: 1,
            action: action.clone(),
            metadata: None,
            output_root: None,
        };
        let second = RemoteActionResult {
            metadata: Some(metadata),
            ..first.clone()
        };
        let path = actions.path_for(&action).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"damaged").unwrap();
        let replacement = directory.path().join("replacement");
        fs::write(&replacement, canonical_json(&first).unwrap()).unwrap();
        let mut holder = Holder::start(&path, &replacement);
        let writer = actions.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            crate::action_slot::tests::observe_claims(move || {
                let _ = started_tx.send(());
            });
            done_tx
                .send(writer.store_unless_restorable(&second, |_| true))
                .unwrap();
        });
        started_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("writer must open the incumbent for its repair claim");
        // A bounded receive checks that repair respects the other process's lock.
        // The replacement and release themselves are ordered by pipe messages.
        assert!(
            matches!(
                done_rx.recv_timeout(Duration::from_secs(1)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "repair completed while another process held the damaged incumbent"
        );
        holder.command("replace");
        holder.expect("replaced");
        holder.command("release");
        holder.expect("released");
        assert_eq!(
            done_rx
                .recv_timeout(Duration::from_secs(20))
                .unwrap()
                .unwrap(),
            ActionStoreOutcome::KeptExisting
        );
        thread.join().unwrap();
        assert_eq!(actions.find(&action).unwrap(), Some(first));
    }

    // An artificial holder makes even intact reads fail on Windows, where
    // record locks are mandatory. Production callers never claim intact records.
    #[cfg(unix)]
    #[test]
    fn intact_stores_do_not_claim_the_record() {
        use crate::action_slot::tests::Holder;
        use std::sync::mpsc;
        use std::time::Duration;

        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let actions = LocalActionCache::new(directory.path());
        let action = CacheDigest::blake3(b"action");
        cas.store_bytes(&action, b"action").unwrap();
        let metadata = CacheDigest::blake3(b"metadata");
        cas.store_bytes(&metadata, b"metadata").unwrap();
        let first = RemoteActionResult {
            version: 1,
            action,
            metadata: None,
            output_root: None,
        };
        let path = actions.store(&first).unwrap();
        let mut holder = Holder::start(&path, &path);
        let (done_tx, done_rx) = mpsc::channel();
        let expected = path.clone();
        let thread = std::thread::spawn(move || {
            crate::action_slot::tests::observe_claims(|| panic!("intact store claimed a slot"));
            assert_eq!(actions.store(&first).unwrap(), expected);
            assert_eq!(
                actions.store_unless_restorable(&first, |_| true).unwrap(),
                ActionStoreOutcome::Unchanged
            );
            let second = RemoteActionResult {
                metadata: Some(metadata),
                ..first
            };
            assert_eq!(
                actions.store_unless_restorable(&second, |_| true).unwrap(),
                ActionStoreOutcome::KeptExisting
            );
            assert!(actions.store(&second).is_err());
            done_tx.send(()).unwrap();
        });
        done_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        holder.command("release");
        holder.expect("released");
        thread.join().unwrap();
    }

    #[test]
    fn stores_and_validates_blobs_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let digest = CacheDigest::blake3(b"cached object");

        let path = cas.store_bytes(&digest, b"cached object").unwrap();
        assert_eq!(cas.find(&digest).unwrap(), Some(path.clone()));
        assert_eq!(fs::read(&path).unwrap(), b"cached object");
        assert_eq!(cas.store_bytes(&digest, b"cached object").unwrap(), path);
        assert!(cas.store_bytes(&digest, b"other object").is_err());
    }

    #[test]
    fn stored_files_are_independent_from_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path().join("cache"));
        let source = directory.path().join("source");
        fs::write(&source, b"cached object").unwrap();
        let digest = CacheDigest::blake3(b"cached object");

        let stored = cas.store_file(&digest, &source).unwrap();
        fs::write(source, b"other object!").unwrap();

        assert_eq!(fs::read(stored).unwrap(), b"cached object");
        assert!(cas.find(&digest).unwrap().is_some());
    }

    #[test]
    fn adopts_verified_files_without_leaving_the_staging_copy() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path().join("cache"));
        let staging = directory.path().join("remote");
        fs::create_dir(&staging).unwrap();
        let source = staging.join("blob");
        fs::write(&source, b"cached object").unwrap();
        let digest = CacheDigest::blake3(b"cached object");

        let stored = cas.adopt_verified_file(&digest, &source).unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read(&stored).unwrap(), b"cached object");
        assert_eq!(cas.find(&digest).unwrap(), Some(stored));
    }

    #[test]
    fn rejects_files_with_the_wrong_digest() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path().join("cache"));
        let source = directory.path().join("source");
        fs::write(&source, b"other object").unwrap();
        let digest = CacheDigest::blake3(b"cached object");

        assert!(cas.store_file(&digest, &source).is_err());
        assert!(!cas.path_for(&digest).unwrap().exists());
    }

    #[test]
    fn a_stored_file_is_hashed_from_the_source_and_matches_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path().join("cache"));
        let source = directory.path().join("source");
        let bytes = vec![7u8; 3 * 1024 * 1024];
        fs::write(&source, &bytes).unwrap();
        let digest = CacheDigest::blake3(&bytes);

        let stored = cas.store_file(&digest, &source).unwrap();

        assert_eq!(fs::read(&stored).unwrap(), bytes);
        assert!(digest.matches_file(&stored).unwrap());
        assert_eq!(cas.find(&digest).unwrap(), Some(stored));
    }

    #[test]
    fn a_source_that_changed_is_not_the_same_identity() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::write(&source, b"cached object").unwrap();
        let before = fs::metadata(&source).unwrap();
        assert!(same_identity(&before, &fs::metadata(&source).unwrap()));

        fs::write(&source, b"other object!").unwrap();
        // Windows only lets a handle opened for writing change the times.
        let file = fs::OpenOptions::new().write(true).open(&source).unwrap();
        file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
        drop(file);
        assert!(!same_identity(&before, &fs::metadata(&source).unwrap()));
    }

    #[test]
    fn stores_read_only_source_files() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path().join("cache"));
        let source = directory.path().join("source");
        fs::write(&source, b"cached object").unwrap();
        let mut permissions = fs::metadata(&source).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&source, permissions).unwrap();
        let digest = CacheDigest::blake3(b"cached object");

        let stored = cas.store_file(&digest, &source).unwrap();

        assert_eq!(fs::read(stored).unwrap(), b"cached object");
        assert!(fs::metadata(&source).unwrap().permissions().readonly());
        make_owner_writable(&source).unwrap();
    }

    #[test]
    fn rejects_corrupt_existing_blobs() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let digest = CacheDigest::blake3(b"cached object");
        let path = cas.store_bytes(&digest, b"cached object").unwrap();
        fs::write(path, b"corrupt").unwrap();

        assert!(cas.find(&digest).is_err());
    }

    #[test]
    fn republishes_over_a_corrupt_blob() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let digest = CacheDigest::blake3(b"cached object");
        let path = cas.store_bytes(&digest, b"cached object").unwrap();
        fs::write(&path, b"corrupt").unwrap();

        assert_eq!(cas.store_bytes(&digest, b"cached object").unwrap(), path);
        assert_eq!(fs::read(&path).unwrap(), b"cached object");
        assert_eq!(cas.find(&digest).unwrap(), Some(path));
    }

    #[test]
    fn republishes_a_file_over_a_corrupt_blob() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path().join("cache"));
        let source = directory.path().join("source");
        fs::write(&source, b"cached object").unwrap();
        let digest = CacheDigest::blake3(b"cached object");
        let path = cas.store_file(&digest, &source).unwrap();
        fs::write(&path, b"corrupt").unwrap();

        assert_eq!(cas.store_file(&digest, &source).unwrap(), path);
        assert_eq!(fs::read(&path).unwrap(), b"cached object");
        assert_eq!(cas.find(&digest).unwrap(), Some(path));
    }

    #[test]
    fn publishes_action_results_after_referenced_blobs() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let actions = LocalActionCache::new(directory.path());
        let action = CacheDigest::blake3(b"action");
        let metadata = CacheDigest::blake3(b"metadata");
        let output_root = CacheDigest::blake3(b"directory");
        let result = RemoteActionResult {
            action: action.clone(),
            metadata: Some(metadata.clone()),
            output_root: Some(output_root.clone()),
            version: 1,
        };

        assert!(actions.store(&result).is_err());
        assert!(actions.store_unless_restorable(&result, |_| true).is_err());
        cas.store_bytes(&action, b"action").unwrap();
        cas.store_bytes(&metadata, b"metadata").unwrap();
        cas.store_bytes(&output_root, b"directory").unwrap();
        actions.store(&result).unwrap();
        assert_eq!(actions.find(&action).unwrap(), Some(result));
    }

    #[test]
    fn atomically_replaces_a_corrupt_action_result() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let actions = LocalActionCache::new(directory.path());
        let action = CacheDigest::blake3(b"action");
        let result = RemoteActionResult {
            action: action.clone(),
            metadata: None,
            output_root: None,
            version: 1,
        };
        cas.store_bytes(&action, b"action").unwrap();
        let path = actions.path_for(&action).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"truncated").unwrap();

        assert!(actions.find(&action).is_err());
        assert_eq!(actions.store(&result).unwrap(), path);
        assert_eq!(actions.find(&action).unwrap(), Some(result));
    }

    #[test]
    fn keeps_only_existing_results_accepted_by_the_restore_predicate() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let actions = LocalActionCache::new(directory.path());
        let action = CacheDigest::blake3(b"action");
        cas.store_bytes(&action, b"action").unwrap();
        let metadata = CacheDigest::blake3(b"metadata");
        cas.store_bytes(&metadata, b"metadata").unwrap();
        let first = RemoteActionResult {
            action: action.clone(),
            metadata: None,
            output_root: None,
            version: 1,
        };
        let second = RemoteActionResult {
            metadata: Some(metadata),
            ..first.clone()
        };
        actions.store(&first).unwrap();
        assert_eq!(
            actions
                .store_unless_restorable(&second, |existing| {
                    assert_eq!(existing, &first);
                    true
                })
                .unwrap(),
            ActionStoreOutcome::KeptExisting
        );
        assert_eq!(actions.find(&action).unwrap(), Some(first.clone()));
        assert_eq!(
            actions
                .store_unless_restorable(&second, |existing| {
                    assert_eq!(existing, &first);
                    false
                })
                .unwrap(),
            ActionStoreOutcome::Stored
        );
        assert_eq!(actions.find(&action).unwrap(), Some(second));
    }

    #[test]
    fn keeps_a_different_result_until_its_objects_are_gone() {
        let directory = tempfile::tempdir().unwrap();
        let cas = LocalCas::new(directory.path());
        let actions = LocalActionCache::new(directory.path());
        let action = CacheDigest::blake3(b"action");
        cas.store_bytes(&action, b"action").unwrap();
        let publish = |name: &[u8], contents: &[u8]| {
            let file = CacheDigest::blake3(contents);
            cas.store_bytes(&file, contents).unwrap();
            let listing = canonical_json(&crate::CacheDirectory {
                directories: Vec::new(),
                files: vec![crate::CacheFileNode {
                    digest: file.clone(),
                    executable: false,
                    mode: 0o644,
                    name: String::from_utf8(name.to_vec()).unwrap(),
                }],
                symlinks: Vec::new(),
                version: 1,
            })
            .unwrap();
            let output_root = CacheDigest::blake3(&listing);
            let metadata = CacheDigest::blake3(name);
            cas.store_bytes(&output_root, &listing).unwrap();
            cas.store_bytes(&metadata, name).unwrap();
            (
                file,
                RemoteActionResult {
                    action: action.clone(),
                    metadata: Some(metadata),
                    output_root: Some(output_root),
                    version: 1,
                },
            )
        };
        let (file, first) = publish(b"libx.rlib", b"built in checkout a");
        let (_, second) = publish(b"libx.rlib", b"built in checkout bb");

        assert_eq!(
            actions.store_unless_restorable(&first, |_| true).unwrap(),
            ActionStoreOutcome::Stored
        );
        assert_eq!(
            actions.store_unless_restorable(&first, |_| true).unwrap(),
            ActionStoreOutcome::Unchanged
        );
        assert_eq!(
            actions.store_unless_restorable(&second, |_| true).unwrap(),
            ActionStoreOutcome::KeptExisting
        );
        let error = actions.store(&second).unwrap_err();
        assert_eq!(error.to_string(), ACTION_CONFLICT_MESSAGE);
        assert_eq!(actions.find(&action).unwrap(), Some(first));

        fs::remove_file(cas.path_for(&file).unwrap()).unwrap();
        assert_eq!(
            actions.store_unless_restorable(&second, |_| true).unwrap(),
            ActionStoreOutcome::Stored
        );
        assert_eq!(actions.find(&action).unwrap(), Some(second.clone()));

        fs::write(actions.path_for(&action).unwrap(), b"truncated").unwrap();
        assert_eq!(
            actions.store_unless_restorable(&second, |_| true).unwrap(),
            ActionStoreOutcome::Stored
        );
        assert_eq!(actions.find(&action).unwrap(), Some(second));
    }
}
