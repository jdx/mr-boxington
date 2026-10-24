//! Coordination for replacing and removing action records.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

use same_file::Handle;
use tempfile::NamedTempFile;

/// An action record locked and checked against its current pathname.
///
/// Never hold two slots at once, and do no CAS writes while holding a slot.
/// Reading CAS objects to validate a record is allowed.
///
/// Callers first inspect records without locking and claim only records that
/// need repair or removal, then recheck that decision through the locked file.
/// Records found intact and restorable by that first inspection need no claim.
/// On Windows the kernel lock is mandatory, so lock-free readers of a repair
/// or removal candidate may get a transient read error. Candidates were observed
/// unusable before claiming; the locked recheck may discover a concurrent repair.
pub struct ActionSlot {
    path: PathBuf,
    file: Option<File>,
}

/// The outcome of publishing into a vacant slot.
pub enum Publish {
    /// The record was published.
    Stored,
    /// Another writer published first; claim again and inspect its record.
    Taken(NamedTempFile),
}

impl ActionSlot {
    /// Wait for the incumbent record and verify its identity under the lock.
    pub fn claim(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::claim_inner(path.as_ref(), false, &mut || {}).map(|slot| slot.unwrap())
    }

    /// Claim a record without waiting; return None when it is locked.
    pub fn try_claim(path: impl AsRef<Path>) -> io::Result<Option<Self>> {
        Self::claim_inner(path.as_ref(), true, &mut || {})
    }

    fn claim_inner(
        path: &Path,
        nonblocking: bool,
        opened: &mut impl FnMut(),
    ) -> io::Result<Option<Self>> {
        for _ in 0..64 {
            // Linux NFS implements flock with fcntl locks, which require a
            // writable descriptor for an exclusive lock.
            let file = match OpenOptions::new().read(true).write(true).open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(Some(Self {
                        path: path.into(),
                        file: None,
                    }));
                }
                Err(error) => return Err(error),
            };
            opened();
            #[cfg(test)]
            tests::record_opened();
            if nonblocking {
                match file.try_lock() {
                    Ok(()) => {}
                    Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
                    Err(std::fs::TryLockError::Error(error)) => return Err(error),
                }
            } else {
                file.lock()?;
            }
            let locked = Handle::from_file(file.try_clone()?)?;
            match Handle::from_path(path) {
                Ok(current) if current == locked => {
                    return Ok(Some(Self {
                        path: path.into(),
                        file: Some(file),
                    }));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other(
            "action record kept changing while claiming it",
        ))
    }

    /// Whether there was no record at the pathname.
    pub fn is_vacant(&self) -> bool {
        self.file.is_none()
    }

    /// The pathname checked by this claim.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the checked inode, or return None for a vacant slot.
    pub fn read(&self) -> io::Result<Option<Vec<u8>>> {
        let Some(mut file) = self.file.as_ref() else {
            return Ok(None);
        };
        file.rewind()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    /// Replace an occupied record with a staged record.
    pub fn replace(self, temporary: NamedTempFile) -> io::Result<()> {
        if self.is_vacant() {
            return Err(io::Error::other("cannot replace a vacant action slot"));
        }
        // The incumbent is open here, and may be open in a process waiting to
        // claim it. `std::fs::rename` replaces it anyway on Windows, using
        // POSIX rename semantics where the system supports them; tempfile's
        // `persist` uses MoveFileExW, which refuses to replace an open file.
        // A failure is reported to the caller rather than retried.
        // `keep` first: on Windows it clears the temporary-file attribute by
        // path, which only works while the staged name still exists.
        let staged = temporary
            .into_temp_path()
            .keep()
            .map_err(|error| error.error)?;
        if let Err(error) = std::fs::rename(&staged, &self.path) {
            let _ = std::fs::remove_file(&staged);
            return Err(error);
        }
        Ok(())
    }

    /// Publish into a vacant slot without overwriting a concurrent writer.
    pub fn publish(self, temporary: NamedTempFile) -> io::Result<Publish> {
        if !self.is_vacant() {
            return Err(io::Error::other(
                "cannot publish into an occupied action slot",
            ));
        }
        match temporary.persist_noclobber(&self.path) {
            Ok(_) => Ok(Publish::Stored),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                Ok(Publish::Taken(error.file))
            }
            Err(error) => Err(error.error),
        }
    }

    /// Remove an occupied record, returning false if it has disappeared.
    pub fn remove(self) -> io::Result<bool> {
        if self.is_vacant() {
            return Err(io::Error::other("cannot remove a vacant action slot"));
        }
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

impl Drop for ActionSlot {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = file.unlock();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, Write};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    thread_local! {
        static CLAIM_OPENED: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
            std::cell::RefCell::new(None);
    }

    pub(crate) fn observe_claims(callback: impl FnMut() + 'static) {
        CLAIM_OPENED.with(|hook| *hook.borrow_mut() = Some(Box::new(callback)));
    }

    pub(super) fn record_opened() {
        CLAIM_OPENED.with(|hook| {
            if let Some(callback) = hook.borrow_mut().as_mut() {
                callback();
            }
        });
    }

    pub(crate) struct Holder {
        child: Child,
        events: Receiver<String>,
    }

    impl Holder {
        pub(crate) fn start(path: &Path, replacement: &Path) -> Self {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "action_slot::tests::record_lock_helper",
                    "--exact",
                    "--nocapture",
                ])
                .env("MBX_ACTION_SLOT_TEST_PATH", path)
                .env("MBX_ACTION_SLOT_TEST_REPLACEMENT", replacement)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let stderr = child.stderr.take().unwrap();
            let (tx, events) = mpsc::channel();
            std::thread::spawn(move || {
                for line in io::BufReader::new(stderr).lines() {
                    if tx.send(line.unwrap()).is_err() {
                        break;
                    }
                }
            });
            let holder = Self { child, events };
            holder.expect("locked");
            holder
        }

        pub(crate) fn expect(&self, event: &str) {
            assert_eq!(
                self.events.recv_timeout(Duration::from_secs(20)).unwrap(),
                event
            );
        }

        pub(crate) fn command(&mut self, command: &str) {
            writeln!(self.child.stdin.as_mut().unwrap(), "{command}").unwrap();
        }
    }

    impl Drop for Holder {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn record_lock_helper() {
        let Some(path) = std::env::var_os("MBX_ACTION_SLOT_TEST_PATH") else {
            return;
        };
        let mut slot = Some(ActionSlot::claim(&path).unwrap());
        eprintln!("locked");
        for command in io::stdin().lock().lines() {
            match command.unwrap().as_str() {
                "replace" => {
                    let replacement = std::env::var_os("MBX_ACTION_SLOT_TEST_REPLACEMENT").unwrap();
                    let mut temporary =
                        NamedTempFile::new_in(Path::new(&path).parent().unwrap()).unwrap();
                    temporary
                        .write_all(&std::fs::read(replacement).unwrap())
                        .unwrap();
                    slot.take().unwrap().replace(temporary).unwrap();
                    slot = Some(ActionSlot::claim(&path).unwrap());
                    eprintln!("replaced");
                }
                "release" => {
                    drop(slot.take());
                    eprintln!("released");
                    return;
                }
                other => panic!("unexpected command: {other}"),
            }
        }
    }

    #[test]
    fn claim_opens_record_for_reading_and_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("record");
        std::fs::write(&path, b"record").unwrap();
        let slot = ActionSlot::claim(&path).unwrap();
        let mut file = slot.file.as_ref().unwrap();
        file.write_all(b"record").unwrap();
        assert_eq!(slot.read().unwrap().unwrap(), b"record");
    }

    #[test]
    fn vacant_and_occupied_claims() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("record");
        let slot = ActionSlot::claim(&path).unwrap();
        assert!(slot.is_vacant());
        assert_eq!(slot.path(), path);
        assert_eq!(slot.read().unwrap(), None);
        let mut temporary = NamedTempFile::new_in(directory.path()).unwrap();
        temporary.write_all(b"record").unwrap();
        assert!(matches!(slot.publish(temporary).unwrap(), Publish::Stored));
        let slot = ActionSlot::claim(&path).unwrap();
        assert!(!slot.is_vacant());
        assert_eq!(slot.read().unwrap().unwrap(), b"record");
        assert_eq!(slot.read().unwrap().unwrap(), b"record");
        assert!(slot.remove().unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn vacant_publication_returns_the_temporary_when_another_writer_wins() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("record");
        let slot = ActionSlot::claim(&path).unwrap();
        let mut temporary = NamedTempFile::new_in(directory.path()).unwrap();
        temporary.write_all(b"B").unwrap();
        std::fs::write(&path, b"A").unwrap();
        let Publish::Taken(temporary) = slot.publish(temporary).unwrap() else {
            panic!("publication overwrote the concurrent record");
        };
        let slot = ActionSlot::claim(&path).unwrap();
        assert_eq!(slot.read().unwrap().unwrap(), b"A");
        slot.replace(temporary).unwrap();
        let slot = ActionSlot::claim(&path).unwrap();
        assert_eq!(slot.read().unwrap().unwrap(), b"B");
        assert!(slot.remove().unwrap());
    }

    #[test]
    fn claim_rechecks_identity_after_waiting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("record");
        let replacement = directory.path().join("replacement");
        std::fs::write(&path, b"O").unwrap();
        std::fs::write(&replacement, b"A").unwrap();
        let mut holder = Holder::start(&path, &replacement);
        assert!(ActionSlot::try_claim(&path).unwrap().is_none());
        let (opened_tx, opened_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let slot = ActionSlot::claim_inner(&path, false, &mut || {
                opened_tx.send(()).unwrap();
            })
            .unwrap()
            .unwrap();
            done_tx.send(slot.read().unwrap()).unwrap();
        });
        opened_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        holder.command("replace");
        holder.expect("replaced");
        holder.command("release");
        holder.expect("released");
        assert_eq!(
            done_rx
                .recv_timeout(Duration::from_secs(20))
                .unwrap()
                .unwrap(),
            b"A"
        );
        thread.join().unwrap();
    }
}
