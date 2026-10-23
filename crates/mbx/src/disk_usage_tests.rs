use super::*;

fn write(path: &Path, bytes: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn measure(ledger: &mut Ledger, root: &Path) -> Measured {
    ledger.measure(root, &HashSet::new())
}

/// Makes a directory writable again when dropped, after a test took its
/// permissions away.
#[cfg(unix)]
struct Restore(PathBuf);

#[cfg(unix)]
impl Drop for Restore {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn distinct_files_count_their_lengths() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a"), 3);
    write(&temp.path().join("nested/b"), 5);
    assert_eq!(tree_bytes(temp.path()), 8);
    assert_eq!(tree_bytes(&temp.path().join("a")), 3, "a root file counts");
}

#[test]
fn an_unshared_file_never_enters_the_map_and_still_releases() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a"), 3);
    let mut ledger = Ledger::default();
    let measured = measure(&mut ledger, temp.path());
    assert!(ledger.files.is_empty());
    assert_eq!(
        measured,
        Measured {
            unshared: 3,
            unknown: 0,
            shared: Vec::new()
        }
    );
    assert_eq!(ledger.total(), 3);
    assert_eq!(
        ledger.release(&measured),
        Release {
            logical: 3,
            physical: 3
        }
    );
    assert_eq!(ledger.total(), 0);
}

#[test]
fn excluded_paths_are_not_measured() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a"), 3);
    write(&temp.path().join("skip/b"), 5);
    write(&temp.path().join("c"), 7);
    let mut ledger = Ledger::default();
    let excluded = HashSet::from([temp.path().join("skip"), temp.path().join("c")]);
    assert_eq!(ledger.measure(temp.path(), &excluded).unshared, 3);
    assert_eq!(ledger.total(), 3);
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("outside/a"), 7);
    std::fs::create_dir(temp.path().join("tree")).unwrap();
    std::os::unix::fs::symlink(temp.path().join("outside"), temp.path().join("tree/link")).unwrap();
    assert_eq!(tree_bytes(&temp.path().join("tree")), 0);
}

#[cfg(unix)]
#[test]
fn a_hard_linked_pair_in_one_tree_counts_once() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a"), 10);
    std::fs::hard_link(temp.path().join("a"), temp.path().join("b")).unwrap();
    assert_eq!(tree_bytes(temp.path()), 10);
}

/// Two trees, `first` and `second`, sharing a 10-byte file and each holding
/// one of its own: 1 byte in `first`, `second_own` bytes in `second`.
#[cfg(unix)]
fn sharing_trees(root: &Path, second_own: usize) {
    write(&root.join("first/shared"), 10);
    write(&root.join("first/own"), 1);
    if second_own > 0 {
        write(&root.join("second/own"), second_own);
    } else {
        std::fs::create_dir_all(root.join("second")).unwrap();
    }
    std::fs::hard_link(root.join("first/shared"), root.join("second/shared")).unwrap();
}

#[cfg(unix)]
#[test]
fn owners_sharing_a_file_release_it_with_the_last_owner() {
    let temp = tempfile::tempdir().unwrap();
    sharing_trees(temp.path(), 0);
    let mut ledger = Ledger::default();
    let first = measure(&mut ledger, &temp.path().join("first"));
    let second = measure(&mut ledger, &temp.path().join("second"));
    assert_eq!(ledger.total(), 11);
    assert_eq!(ledger.files.len(), 1, "only the shared file is kept");

    let release = ledger.release(&first);
    assert_eq!(release.logical, 1, "the shared file is still reached");
    assert_eq!(ledger.total(), 10);
    let release = ledger.release(&second);
    assert_eq!(release.logical, 10);
    assert_eq!(release.physical, 10, "every link was measured");
    assert_eq!(ledger.total(), 0);
}

#[cfg(unix)]
#[test]
fn a_file_linked_from_outside_releases_nothing_physical() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("store/object"), 10);
    std::fs::create_dir(temp.path().join("tree")).unwrap();
    std::fs::hard_link(
        temp.path().join("store/object"),
        temp.path().join("tree/output"),
    )
    .unwrap();
    write(&temp.path().join("tree/own"), 4);
    let mut ledger = Ledger::default();
    let files = measure(&mut ledger, &temp.path().join("tree"));
    let release = ledger.release(&files);
    assert_eq!(release.logical, 14);
    assert_eq!(release.physical, 4);
}

#[cfg(unix)]
#[test]
fn a_cloned_ledger_credits_shared_files_to_the_last_release() {
    let temp = tempfile::tempdir().unwrap();
    sharing_trees(temp.path(), 2);
    let mut ledger = Ledger::default();
    let first = measure(&mut ledger, &temp.path().join("first"));
    let second = measure(&mut ledger, &temp.path().join("second"));

    let mut projection = ledger.clone();
    assert_eq!(projection.release(&first).logical, 1);
    assert_eq!(projection.release(&second).logical, 12);

    let mut projection = ledger.clone();
    assert_eq!(projection.release(&second).logical, 2);
    assert_eq!(ledger.total(), 13, "the copy changes nothing");
}

#[cfg(unix)]
#[test]
fn merged_ledgers_match_a_single_measure() {
    let temp = tempfile::tempdir().unwrap();
    sharing_trees(temp.path(), 2);
    let mut single = Ledger::default();
    let first = measure(&mut single, &temp.path().join("first"));
    let second = measure(&mut single, &temp.path().join("second"));

    let mut merged = Ledger::default();
    let mut other = Ledger::default();
    measure(&mut merged, &temp.path().join("first"));
    measure(&mut other, &temp.path().join("second"));
    merged.merge(other);
    assert_eq!(merged.total(), single.total());
    assert_eq!(merged.total(), 13);
    assert_eq!(merged.release(&first), single.release(&first));
    assert_eq!(merged.release(&second), single.release(&second));
    assert_eq!(merged.total(), 0);
}

#[test]
fn a_counted_removal_releases_what_went() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    write(&tree.join("a"), 3);
    write(&tree.join("nested/b"), 5);
    write(&temp.path().join("kept"), 7);
    let mut ledger = Ledger::default();
    measure(&mut ledger, &tree);
    measure(&mut ledger, &temp.path().join("kept"));

    let removal = remove_tree_counting(&tree, &mut ledger, true);
    assert!(!tree.exists());
    assert_eq!(removal.deleted_files, 2);
    assert_eq!(removal.unshared, 8);
    assert_eq!(
        removal.release,
        Release {
            logical: 8,
            physical: 8
        }
    );
    assert_eq!(ledger.total(), 7);
}

#[test]
fn a_preview_releases_without_deleting() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a"), 3);
    let mut ledger = Ledger::default();
    measure(&mut ledger, temp.path());
    let removal = remove_tree_counting(temp.path(), &mut ledger, false);
    assert!(temp.path().join("a").exists());
    assert_eq!(removal.deleted_files, 0, "nothing was deleted");
    assert_eq!(removal.release.logical, 3);
    assert_eq!(ledger.total(), 0);
}

#[test]
fn removing_a_missing_tree_does_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let mut ledger = Ledger::default();
    let removal = remove_tree_counting(&temp.path().join("gone"), &mut ledger, true);
    assert_eq!(removal, Removal::default());
}

#[cfg(unix)]
#[test]
fn removing_a_store_linked_file_frees_it_logically_only() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("store/object"), 10);
    let tree = temp.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::hard_link(temp.path().join("store/object"), tree.join("output")).unwrap();
    let mut ledger = Ledger::default();
    measure(&mut ledger, &tree);
    let removal = remove_tree_counting(&tree, &mut ledger, true);
    assert_eq!(removal.deleted_files, 1);
    assert_eq!(
        removal.release,
        Release {
            logical: 10,
            physical: 0
        }
    );
}

#[cfg(unix)]
#[test]
fn removing_one_of_two_links_in_a_tree_waits_for_the_other() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    write(&tree.join("a"), 10);
    std::fs::hard_link(tree.join("a"), tree.join("b")).unwrap();
    let mut ledger = Ledger::default();
    measure(&mut ledger, &tree);
    let removal = remove_tree_counting_with_keys(&tree, &mut ledger, true);
    assert_eq!(removal.deleted_files, 2);
    assert_eq!(removal.gone.len(), 2);
    assert_eq!(
        removal.release,
        Release {
            logical: 10,
            physical: 10
        }
    );
}

#[cfg(unix)]
#[test]
fn a_measure_forgets_what_a_removal_took() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    write(&tree.join("own"), 3);
    write(&tree.join("unit/own"), 4);
    write(&tree.join("unit/shared"), 10);
    std::fs::hard_link(tree.join("unit/shared"), tree.join("shared")).unwrap();
    let mut ledger = Ledger::default();
    let mut measured = measure(&mut ledger, &tree);

    let removal = remove_tree_counting_with_keys(&tree.join("unit"), &mut ledger, true);
    measured.forget(&removal);

    assert_eq!(measured.unshared, 3);
    assert_eq!(
        measured.shared.len(),
        1,
        "one link to the shared file is left"
    );
    assert_eq!(ledger.release(&measured).logical, 13);
    assert_eq!(ledger.total(), 0);
}

#[cfg(unix)]
#[test]
fn a_partial_removal_keeps_the_survivors_counted() {
    use std::os::unix::fs::PermissionsExt as _;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    write(&tree.join("free/a"), 3);
    write(&tree.join("locked/b"), 5);
    write(&tree.join("more/c"), 7);
    let mut ledger = Ledger::default();
    measure(&mut ledger, &tree);
    let locked = tree.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let restore = Restore(locked.clone());
    let removal = remove_tree_counting(&tree, &mut ledger, true);
    drop(restore);

    assert!(tree.exists());
    assert!(locked.join("b").exists());
    let left = tree_bytes(&tree);
    let went = [("free/a", 3), ("more/c", 7)]
        .into_iter()
        .filter(|(path, _)| !tree.join(path).exists())
        .collect::<Vec<_>>();
    assert_eq!(removal.deleted_files, went.len() as u64);
    assert_eq!(
        removal.release.logical,
        went.iter().map(|(_, bytes)| bytes).sum::<u64>()
    );
    assert_eq!(ledger.total(), left);
}

#[cfg(unix)]
#[test]
fn releasing_an_unknown_file_does_nothing() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a"), 3);
    std::fs::hard_link(temp.path().join("a"), temp.path().join("b")).unwrap();
    let mut other = Ledger::default();
    let files = measure(&mut other, temp.path());
    let unknown = Measured {
        unshared: 0,
        unknown: 0,
        shared: files.shared.clone(),
    };
    let mut ledger = Ledger::default();
    assert_eq!(ledger.release(&unknown), Release::default());
    assert_eq!(other.release(&files).logical, 3);
    assert_eq!(other.release(&files), Release::default(), "released once");
}

#[cfg(not(unix))]
#[test]
fn path_keys_count_each_length() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a"), 3);
    write(&temp.path().join("b"), 5);
    let mut ledger = Ledger::default();
    let files = measure(&mut ledger, temp.path());
    assert_eq!(ledger.total(), 8);
    let unknown = Measured {
        unshared: 0,
        unknown: 0,
        shared: vec![FileKey::Path(temp.path().join("never"))],
    };
    assert_eq!(ledger.release(&unknown), Release::default());
    assert_eq!(ledger.release(&files).logical, 8);
}

#[cfg(windows)]
#[test]
fn removing_a_read_only_link_leaves_the_other_link_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    write(&tree.join("a"), 4);
    let mut permissions = std::fs::metadata(tree.join("a")).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(tree.join("a"), permissions).unwrap();
    let survivor = temp.path().join("survivor");
    std::fs::hard_link(tree.join("a"), &survivor).unwrap();
    let mut ledger = Ledger::default();
    measure(&mut ledger, &tree);

    let removal = remove_tree_counting(&tree, &mut ledger, true);

    assert!(!tree.exists());
    assert_eq!(removal.release.logical, 4);
    assert_eq!(removal.release.physical, 0, "the other link keeps it");
    assert!(
        std::fs::metadata(&survivor)
            .unwrap()
            .permissions()
            .readonly()
    );
}

/// Unit pruning removes output files one at a time; outputs restored from
/// the store are read-only on Windows.
#[cfg(windows)]
#[test]
fn removing_a_read_only_file_leaves_its_other_link_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("libwidget.rlib");
    write(&file, 4);
    let mut permissions = std::fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&file, permissions).unwrap();
    let survivor = temp.path().join("survivor");
    std::fs::hard_link(&file, &survivor).unwrap();
    let mut ledger = Ledger::default();
    measure(&mut ledger, &file);

    let removal = remove_tree_counting(&file, &mut ledger, true);

    assert!(!file.exists());
    assert_eq!(removal.deleted_files, 1);
    assert!(
        std::fs::metadata(&survivor)
            .unwrap()
            .permissions()
            .readonly()
    );
}

#[test]
fn a_counted_removal_never_follows_a_directory_link() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    write(&outside.join("kept"), 4);
    let tree = temp.path().join("tree");
    write(&tree.join("own"), 3);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, tree.join("link"));
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&outside, tree.join("link"));
    if let Err(error) = linked {
        eprintln!("skipped: cannot create a directory symlink here: {error}");
        return;
    }
    let mut ledger = Ledger::default();
    measure(&mut ledger, &tree);
    measure(&mut ledger, &outside);

    let removal = remove_tree_counting(&tree, &mut ledger, true);

    assert!(!tree.exists());
    assert!(outside.join("kept").exists());
    assert_eq!(removal.deleted_files, 2, "the file and the link itself");
    assert_eq!(removal.release.logical, 3);
    assert_eq!(ledger.total(), 4);
}

/// A file whose identity could not be read is counted by its length, so a
/// rename before its removal, as unit pruning does, cannot separate it from
/// what was counted; and it is never taken for the last link to its data.
#[cfg(not(unix))]
#[test]
fn a_file_of_unknown_identity_is_counted_by_length_and_never_released() {
    let mut ledger = Ledger::default();
    assert!(
        ledger
            .count_key(FileKey::Path(PathBuf::from("before")), UNKNOWN_LINKS, 10)
            .is_none()
    );
    assert_eq!(ledger.total(), 10);

    // Removed under the name it was moved aside to.
    let renamed = FileKey::Path(PathBuf::from("aside"));
    let gone = links_gone(std::slice::from_ref(&renamed), |key| key);
    assert!(
        gone[&renamed] < UNKNOWN_LINKS,
        "a removal cannot prove it left"
    );
    let mut removal = Removal::default();
    ledger.release_path(renamed, 10, UNKNOWN_LINKS, Some(false), &mut removal);

    assert_eq!(removal.release.logical, 10);
    assert_eq!(removal.release.physical, 0);
    assert_eq!(removal.unknown, 10);
    assert_eq!(ledger.total(), 0);
}

/// A file another process holds open without sharing is still identified.
#[cfg(windows)]
#[test]
fn a_file_held_open_without_sharing_is_identified() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("held");
    write(&path, 4);
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let metadata = std::fs::symlink_metadata(&path).unwrap();

    let (key, links) = file_key(|| path.clone(), &metadata);

    assert!(matches!(key, FileKey::Object(..)), "{key:?}");
    assert_eq!(links, 1);
}

/// A real removal judges by the filesystem, not the ledger's estimate, which
/// takes every measured path for a distinct link: here the one path is
/// measured twice while the file's other link lives outside the ledger.
#[cfg(unix)]
#[test]
fn a_removal_releases_on_disk_only_files_that_lost_every_link() {
    let temp = tempfile::tempdir().unwrap();
    let tree = temp.path().join("tree");
    write(&tree.join("a"), 4);
    std::fs::hard_link(tree.join("a"), temp.path().join("store-object")).unwrap();
    let mut ledger = Ledger::default();
    let first = measure(&mut ledger, &tree);
    measure(&mut ledger, &tree);
    ledger.release(&first);

    let removal = remove_tree_counting(&tree, &mut ledger, true);

    assert_eq!(removal.release.logical, 4);
    assert_eq!(removal.release.physical, 0, "the store's link keeps it");
}

/// A preview never reports bytes of unknown identity as released either.
#[test]
fn a_release_of_unknown_bytes_frees_nothing_on_disk() {
    let mut ledger = Ledger::default();
    let measured = Measured {
        unshared: 10,
        unknown: 4,
        shared: Vec::new(),
    };
    let release = ledger.release(&measured);
    assert_eq!(release.logical, 10);
    assert_eq!(release.physical, 6);
}
