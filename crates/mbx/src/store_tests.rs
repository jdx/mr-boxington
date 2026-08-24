use super::*;
use mbx_cache_core::{ActionPrediction, CacheFileNode, LocalActionCache};

fn store_object(store: &Path, contents: &[u8]) -> CacheDigest {
    let digest = CacheDigest::blake3(contents);
    LocalCas::new(store).store_bytes(&digest, contents).unwrap();
    digest
}

/// Backdate an object so eviction order is deterministic.
fn age(store: &Path, digest: &CacheDigest, age: Duration) {
    let path = LocalCas::new(store).path_for(digest).unwrap();
    let when = SystemTime::now() - age;
    let time = filetime::FileTime::from_system_time(when);
    filetime::set_file_times(path, time, time).unwrap();
}

/// Publish an action result whose output tree holds `outputs`.
fn store_result(store: &Path, name: &str, outputs: &[CacheDigest]) -> CacheDigest {
    let directory = CacheDirectory {
        directories: Vec::new(),
        files: outputs
            .iter()
            .enumerate()
            .map(|(index, digest)| CacheFileNode {
                digest: digest.clone(),
                executable: false,
                mode: 0o644,
                name: format!("output-{index}"),
            })
            .collect(),
        symlinks: Vec::new(),
        version: 1,
    };
    let encoded = serde_json::to_vec(&directory).unwrap();
    let output_root = store_object(store, &encoded);
    let action = store_object(store, name.as_bytes());
    LocalActionCache::new(store)
        .store(&RemoteActionResult {
            version: 1,
            action: action.clone(),
            metadata: None,
            output_root: Some(output_root),
        })
        .unwrap();
    action
}

/// Record a checkout of `identity` and the manifest that roots `actions`.
///
/// The agent only accepts predictions over its socket, so the manifest is
/// written here instead. The predictions come from the public type rather
/// than hand-rolled JSON, leaving only the two wrapper fields to drift --
/// and `mbx_cache_core`'s own tests write a manifest through the agent and
/// read it back with the same accessor this uses, so drift shows up there.
fn record_build(store: &Path, identity: &str, workspace_root: &Path, actions: &[CacheDigest]) {
    record_checkout(
        store,
        identity,
        workspace_root,
        &workspace_root.join("target"),
    )
    .unwrap();
    let predictions = actions
        .iter()
        .enumerate()
        .map(|(index, action)| ActionPrediction {
            invocation: CacheDigest::blake3(format!("{identity}-{index}").as_bytes()),
            action: action.clone(),
            adapter: "rustc".into(),
            payload: "{}".into(),
        })
        .collect::<Vec<_>>();
    let manifest = serde_json::json!({
        "version": 1,
        "task": identity,
        "predictions": predictions,
    });
    let path = store
        .join("task-manifests")
        .join("v1")
        .join(format!("{identity}.json"));
    crate::util::write_atomic(&path, &serde_json::to_vec(&manifest).unwrap()).unwrap();
}

#[test]
fn reports_an_empty_store() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(stats(directory.path()).unwrap(), StoreStats::default());
}

#[test]
fn counts_objects_and_results() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    store_object(store, b"first");
    store_object(store, b"second object");

    let stats = stats(store).unwrap();

    assert_eq!(stats.objects, 2);
    assert_eq!(stats.object_bytes, 5 + 13);
    assert_eq!(stats.total_bytes(), 18);
}

#[test]
fn keeps_the_store_under_its_budget_evicting_oldest_first() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    let old = store_object(store, b"0123456789");
    let recent = store_object(store, b"abcdefghij");
    age(store, &old, Duration::from_secs(60 * 60));
    age(store, &recent, Duration::from_secs(1));

    let outcome = gc(store, 10).unwrap();

    assert_eq!(outcome.removed_objects, 1);
    assert_eq!(outcome.removed_bytes, 10);
    assert_eq!(outcome.remaining_bytes, 10);
    let cas = LocalCas::new(store);
    assert!(cas.find(&old).unwrap().is_none());
    assert!(cas.find(&recent).unwrap().is_some());
}

#[test]
fn leaves_a_store_within_budget_alone() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    store_object(store, b"kept");

    let outcome = gc(store, 1024).unwrap();

    assert_eq!(
        outcome,
        GcOutcome {
            remaining_bytes: 4,
            ..GcOutcome::default()
        }
    );
    assert_eq!(stats(store).unwrap().objects, 1);
}

#[test]
fn a_blocked_unrooted_object_does_not_cost_a_rooted_one() {
    let locked = PathBuf::from("locked-unrooted");
    let removable = PathBuf::from("removable-unrooted");
    let protected = PathBuf::from("rooted");
    let objects = [&locked, &removable, &protected]
        .into_iter()
        .map(|path| Entry {
            path: path.clone(),
            size: 10,
            used: SystemTime::UNIX_EPOCH,
        })
        .collect::<Vec<_>>();
    let rooted = HashSet::from([protected.clone()]);
    let mut attempted = Vec::new();

    let outcome = evict_objects(&objects, &rooted, 30, 5, |path| {
        attempted.push(path.to_path_buf());
        Ok(if path == locked {
            Removal::Blocked
        } else {
            Removal::Removed
        })
    })
    .unwrap();

    assert_eq!(attempted, vec![locked, removable]);
    assert_eq!(
        outcome,
        ObjectEvictions {
            removed_objects: 1,
            removed_bytes: 10,
            remaining_bytes: 20,
        },
        "a locked unrooted blob should leave the store over budget before protected objects are evicted"
    );
}

#[test]
fn drops_an_action_result_whose_descriptor_blob_is_gone() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    let action = store_object(store, b"action key");
    let output_root = store_object(store, b"output root blob");
    let result = RemoteActionResult {
        version: 1,
        action: action.clone(),
        metadata: None,
        output_root: Some(output_root.clone()),
    };
    let cache = LocalActionCache::new(store);
    cache.store(&result).unwrap();

    // Evict only the descriptor blob, leaving the output root in place.
    std::fs::remove_file(LocalCas::new(store).find(&action).unwrap().unwrap()).unwrap();

    let outcome = gc(store, u64::MAX).unwrap();

    // Left behind, this entry would report a hit that `store` could never
    // republish, since publication requires the descriptor blob.
    assert_eq!(outcome.removed_action_results, 1);
    assert!(cache.find(&action).unwrap().is_none());
}

#[test]
fn keeps_an_action_result_whose_blob_is_present_but_corrupt() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    let action = store_object(store, b"action key");
    LocalActionCache::new(store)
        .store(&RemoteActionResult {
            version: 1,
            action: action.clone(),
            metadata: None,
            output_root: None,
        })
        .unwrap();
    let path = LocalCas::new(store).path_for(&action).unwrap();
    std::fs::write(&path, b"corrupted!").unwrap();

    let outcome = gc(store, u64::MAX).unwrap();

    // The sweep does not read content, so this result survives and costs a
    // miss on restore. Verifying instead would re-hash the whole store.
    assert_eq!(outcome.removed_action_results, 0);
    assert!(path.exists());
}

#[test]
fn drops_action_results_left_without_their_objects() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    let action = store_object(store, b"action key");
    let metadata = store_object(store, b"metadata blob");
    let result = RemoteActionResult {
        version: 1,
        action: action.clone(),
        metadata: Some(metadata),
        output_root: None,
    };
    LocalActionCache::new(store).store(&result).unwrap();

    // A budget of zero evicts every object, orphaning the result.
    let outcome = gc(store, 0).unwrap();

    assert!(outcome.removed_objects >= 1);
    assert_eq!(outcome.removed_action_results, 1);
    assert!(
        LocalActionCache::new(store)
            .find(&action)
            .unwrap()
            .is_none()
    );
}

#[test]
fn evicts_objects_no_live_checkout_needs_before_older_rooted_ones() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let live = directory.path().join("live");
    let deleted = directory.path().join("deleted");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::create_dir_all(&deleted).unwrap();

    let kept = store_object(&store, b"0123456789");
    let dropped = store_object(&store, b"abcdefghij");
    record_build(
        &store,
        &"a".repeat(64),
        &live,
        &[store_result(
            &store,
            "live action",
            std::slice::from_ref(&kept),
        )],
    );
    record_build(
        &store,
        &"b".repeat(64),
        &deleted,
        &[store_result(
            &store,
            "deleted action",
            std::slice::from_ref(&dropped),
        )],
    );
    // The rooted object is the older one, so plain LRU would take it first.
    age(&store, &kept, Duration::from_secs(60 * 60));
    age(&store, &dropped, Duration::from_secs(1));
    std::fs::remove_dir_all(&deleted).unwrap();

    gc(&store, stats(&store).unwrap().total_bytes() - 10).unwrap();

    let cas = LocalCas::new(&store);
    assert!(
        cas.find(&kept).unwrap().is_some(),
        "a live checkout still needs this object"
    );
    assert!(
        cas.find(&dropped).unwrap().is_none(),
        "nothing that still exists needs this object"
    );
}

#[test]
fn keeps_rooting_when_a_sibling_worktree_survives() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();

    // Worktrees of one dependency graph share an identity by design, so the
    // survivor's claim has to keep the whole identity rooted.
    let identity = "c".repeat(64);
    let shared = store_object(&store, b"shared artifact");
    let action = store_result(&store, "shared action", std::slice::from_ref(&shared));
    record_build(&store, &identity, &first, std::slice::from_ref(&action));
    record_build(&store, &identity, &second, &[action]);
    let spare = store_object(&store, b"unrooted spare");
    std::fs::remove_dir_all(&second).unwrap();

    let outcome = gc(&store, stats(&store).unwrap().total_bytes() - 1).unwrap();

    assert_eq!(outcome.removed_checkout_records, 1);
    let cas = LocalCas::new(&store);
    assert!(cas.find(&shared).unwrap().is_some());
    assert!(cas.find(&spare).unwrap().is_none());
}

#[test]
fn roots_objects_nested_in_an_output_tree() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let checkout = directory.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();

    // The leaf artifact, not the descriptor, is where a build's bytes are.
    let artifact = store_object(&store, b"the compiled artifact");
    let action = store_result(&store, "an action", std::slice::from_ref(&artifact));
    record_build(&store, &"d".repeat(64), &checkout, &[action]);
    let spare = store_object(&store, b"unrooted spare");

    gc(&store, stats(&store).unwrap().total_bytes() - 1).unwrap();

    let cas = LocalCas::new(&store);
    assert!(cas.find(&artifact).unwrap().is_some());
    assert!(cas.find(&spare).unwrap().is_none());
}

#[test]
fn treats_a_store_with_no_checkout_records_as_plain_lru() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    // A store written before checkouts were recorded roots nothing, so it
    // has to keep collecting exactly as it did.
    let old = store_object(store, b"0123456789");
    let recent = store_object(store, b"abcdefghij");
    age(store, &old, Duration::from_secs(60 * 60));
    age(store, &recent, Duration::from_secs(1));

    gc(store, 10).unwrap();

    let cas = LocalCas::new(store);
    assert!(cas.find(&old).unwrap().is_none());
    assert!(cas.find(&recent).unwrap().is_some());
}

#[test]
fn drops_checkout_records_whose_worktree_is_gone() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let gone = directory.path().join("gone");
    std::fs::create_dir_all(&gone).unwrap();
    record_checkout(&store, &"e".repeat(64), &gone, &gone.join("target")).unwrap();

    assert_eq!(stats(&store).unwrap().live_checkouts, 1);
    std::fs::remove_dir_all(&gone).unwrap();
    let after = stats(&store).unwrap();
    assert_eq!(after.stale_checkouts, 1);
    assert_eq!(after.live_checkouts, 0);

    let outcome = gc(&store, u64::MAX).unwrap();

    assert_eq!(outcome.removed_checkout_records, 1);
    assert_eq!(stats(&store).unwrap(), StoreStats::default());
}

#[test]
fn keeps_a_checkout_recorded_while_its_worktree_exists() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let live = directory.path().join("live");
    std::fs::create_dir_all(&live).unwrap();
    record_checkout(&store, &"f".repeat(64), &live, &live.join("target")).unwrap();

    let outcome = gc(&store, u64::MAX).unwrap();

    assert_eq!(outcome.removed_checkout_records, 0);
    assert_eq!(stats(&store).unwrap().live_checkouts, 1);
}

#[test]
fn forgets_a_claim_no_build_has_renewed() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let checkout = directory.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();

    // The checkout is still there, but this identity names a command line
    // against a lockfile that has since moved on, so nothing renews it.
    let identity = "2".repeat(64);
    let orphan = store_object(&store, b"what that command built");
    record_build(
        &store,
        &identity,
        &checkout,
        &[store_result(
            &store,
            "stale action",
            std::slice::from_ref(&orphan),
        )],
    );
    let stale = CheckoutRecord {
        version: CHECKOUT_RECORD_VERSION,
        workspace_root: checkout.clone(),
        target_dir: checkout.join("target"),
        updated_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - CHECKOUT_RETENTION.as_secs()
            - 1,
    };
    crate::util::write_atomic(
        &checkout_record_path(&store, &identity, &checkout),
        &serde_json::to_vec(&stale).unwrap(),
    )
    .unwrap();
    // Filesystems with coarse timestamps can otherwise tie this object with
    // the action descriptor and output tree created immediately afterward.
    age(&store, &orphan, Duration::from_secs(1));

    assert_eq!(stats(&store).unwrap().stale_checkouts, 1);
    gc(&store, stats(&store).unwrap().total_bytes() - 1).unwrap();

    assert!(
        LocalCas::new(&store).find(&orphan).unwrap().is_none(),
        "an expired claim roots nothing"
    );
}

#[test]
fn ignores_what_it_did_not_write_beside_the_checkout_records() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let checkout = directory.path().join("project");
    std::fs::create_dir_all(&checkout).unwrap();
    record_checkout(&store, &"3".repeat(64), &checkout, &checkout.join("target")).unwrap();

    // A plain file whose name reads like an identity used to fail the scan,
    // and with it every sweep and every `cache stats`.
    let intruder = store.join(CHECKOUTS_DIR).join("4".repeat(64));
    std::fs::write(&intruder, b"not a directory of records").unwrap();
    std::fs::write(store.join(CHECKOUTS_DIR).join("notes.txt"), b"nor this").unwrap();

    assert_eq!(stats(&store).unwrap().live_checkouts, 1);
    assert_eq!(gc(&store, u64::MAX).unwrap().removed_checkout_records, 0);
    assert!(intruder.exists(), "and nothing of theirs is deleted either");
}

#[test]
fn keeps_a_checkout_whose_absence_it_cannot_corroborate() {
    let directory = tempfile::tempdir().unwrap();

    // A checkout that is gone from a parent that is still there is the one
    // case worth believing: that is what deleting a worktree looks like.
    let parent = directory.path().join("worktrees");
    std::fs::create_dir_all(&parent).unwrap();
    assert!(!checkout_is_live(&parent.join("removed")));

    // A checkout whose parent went with it looks like an ejected volume or a
    // mount that is temporarily away, and un-rooting those would throw away
    // a cache somebody is still using.
    assert!(checkout_is_live(
        &directory.path().join("gone/deeper/still")
    ));

    // And anything still on disk is live whatever its parent says.
    assert!(checkout_is_live(directory.path()));
}

#[test]
fn sweeps_only_once_within_the_interval() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    store_object(store, b"kept");

    assert!(
        sweep_if_due(store, u64::MAX, Duration::from_secs(3600))
            .unwrap()
            .is_some()
    );
    assert!(
        sweep_if_due(store, u64::MAX, Duration::from_secs(3600))
            .unwrap()
            .is_none(),
        "the interval has not passed"
    );
    assert!(
        sweep_if_due(store, u64::MAX, Duration::ZERO)
            .unwrap()
            .is_some(),
        "a zero interval always sweeps"
    );
}

#[test]
fn does_not_count_its_own_bookkeeping_against_the_budget() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store");
    let checkout = directory.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    record_checkout(&store, &"1".repeat(64), &checkout, &checkout.join("target")).unwrap();
    sweep_if_due(&store, u64::MAX, Duration::ZERO).unwrap();

    // Checkout records and the sweep stamp live outside the collected
    // trees; counting them would make the budget mean something else.
    assert_eq!(stats(&store).unwrap().total_bytes(), 0);
}
