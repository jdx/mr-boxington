use super::*;
use crate::config::TargetSettings;

fn test_config(root: &Path, views: bool) -> Config {
    Config {
        cache_dir: root.join("cache"),
        stats_report: None,
        verify: false,
        incremental: false,
        share_out_dir: false,
        remote: Default::default(),
        http: Default::default(),
        gc: Default::default(),
        target: TargetSettings {
            views,
            root: root.join("targets"),
            max_bytes: None,
            max_age: None,
        },
    }
}

/// A checkout with the default target directory, ready to be managed.
fn checkout(root: &Path, name: &str) -> PathBuf {
    let workspace = root.join(name);
    std::fs::create_dir_all(&workspace).unwrap();
    workspace
}

#[test]
fn places_the_default_target_directory_under_the_managed_root() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");

    let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();

    assert!(managed.is_absolute(), "the shim maps only absolute roots");
    assert!(managed.starts_with(views_root(&config.target.root)));
    assert!(managed.is_dir());
    assert_eq!(
        std::fs::read_link(workspace.join("target")).unwrap(),
        managed,
        "the workspace should still have a target directory to reach"
    );
    assert_eq!(stats(&config.target.root).unwrap().views, 1);
}

#[test]
fn placing_a_target_directory_twice_reaches_the_same_one() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");

    let first = place(&config, &workspace, &workspace.join("target"), false).unwrap();
    let second = place(&config, &workspace, &workspace.join("target"), false).unwrap();

    assert_eq!(first, second);
    assert_eq!(stats(&config.target.root).unwrap().views, 1);
}

#[test]
fn replaces_an_outdated_managed_target_link() {
    let directory = tempfile::tempdir().unwrap();
    let first = test_config(&directory.path().join("first"), true);
    let second = test_config(&directory.path().join("second"), true);
    let workspace = checkout(directory.path(), "project");
    let old = place(&first, &workspace, &workspace.join("target"), false).unwrap();
    std::fs::write(old.join("artifact"), b"outputs").unwrap();

    let new = place(&second, &workspace, &workspace.join("target"), false).unwrap();

    assert_ne!(old, new);
    assert_eq!(std::fs::read_link(workspace.join("target")).unwrap(), new);
    assert!(new.join("artifact").is_file(), "the old view should move");
    assert!(!old.exists(), "the old root must not retain an orphan");
    assert_eq!(stats(&first.target.root).unwrap(), ViewStats::default());
    assert_eq!(stats(&second.target.root).unwrap().views, 1);
}

#[test]
fn leaves_somebody_elses_dangling_link_alone() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let target = workspace.join("target");
    let elsewhere = directory.path().join("missing");
    symlink_dir(&elsewhere, &target).unwrap();

    assert!(place(&config, &workspace, &target, false).is_none());
    assert_eq!(std::fs::read_link(target).unwrap(), elsewhere);
}

#[test]
fn leaves_another_live_checkouts_view_alone() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let original = checkout(directory.path(), "original");
    let copied = checkout(directory.path(), "copied");
    let managed = place(&config, &original, &original.join("target"), false).unwrap();
    std::fs::write(managed.join("artifact"), b"outputs").unwrap();
    symlink_dir(&managed, &copied.join("target")).unwrap();

    assert!(place(&config, &copied, &copied.join("target"), false).is_none());

    assert_eq!(std::fs::read_link(copied.join("target")).unwrap(), managed);
    assert!(managed.join("artifact").exists());
    assert_eq!(stats(&config.target.root).unwrap().views, 1);
}

#[test]
fn leaves_the_target_directory_alone_when_disabled() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), false);
    let workspace = checkout(directory.path(), "project");

    assert!(place(&config, &workspace, &workspace.join("target"), false).is_none());
    assert!(!workspace.join("target").exists());
}

#[test]
fn only_an_unrequested_real_default_target_can_be_removed() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let target = workspace.join("target");
    std::fs::create_dir_all(&target).unwrap();

    assert!(can_remove_existing(&config, &workspace, &target, false));
    assert!(!can_remove_existing(&config, &workspace, &target, true));
    assert!(!can_remove_existing(
        &config,
        &workspace,
        &workspace.join("somewhere-else"),
        false
    ));
    assert!(!can_remove_existing(
        &test_config(directory.path(), false),
        &workspace,
        &target,
        false
    ));
}

#[test]
fn a_failed_migration_restores_existing_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let target = workspace.join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("artifact"), b"old output").unwrap();

    let outcome = migrate_existing_with(&config, &workspace, &target, false, || None).unwrap();

    assert_eq!(outcome, MigrationOutcome::default());
    assert_eq!(
        std::fs::read(target.join("artifact")).unwrap(),
        b"old output"
    );
}

#[test]
fn a_preparation_failure_restores_existing_outputs_and_record_state() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let target = workspace.join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("artifact"), b"old output").unwrap();
    let managed = view_dir(&config.target.root, &workspace);
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, b"not a directory").unwrap();

    let outcome = migrate_existing(&config, &workspace, &target, false).unwrap();

    assert_eq!(outcome, MigrationOutcome::default());
    assert_eq!(
        std::fs::read(target.join("artifact")).unwrap(),
        b"old output"
    );
    assert_eq!(std::fs::read(&managed).unwrap(), b"not a directory");
    assert!(!view_record_path(&config.target.root, &workspace).exists());
}

#[cfg(unix)]
#[test]
fn a_successful_migration_removes_old_outputs_after_placement() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let target = workspace.join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("artifact"), b"old output").unwrap();
    let elsewhere = directory.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("not-cleaned"), vec![0; 1024]).unwrap();
    symlink_dir(&elsewhere, &target.join("external-link")).unwrap();

    let outcome = migrate_existing(&config, &workspace, &target, false).unwrap();
    let managed = outcome.managed.unwrap();

    assert_eq!(std::fs::read_link(&target).unwrap(), managed);
    assert!(!target.join("artifact").exists());
    assert_eq!(outcome.removed_bytes, Some(10));
    assert!(elsewhere.join("not-cleaned").is_file());
    assert_eq!(stats(&config.target.root).unwrap().views, 1);
}

#[cfg(unix)]
#[test]
fn migrating_an_existing_target_never_follows_a_replacement_link() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let target = workspace.join("target");
    let elsewhere = directory.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("keep"), b"not a build output").unwrap();
    symlink_dir(&elsewhere, &target).unwrap();

    assert!(migrate_existing(&config, &workspace, &target, false).is_err());
    assert!(elsewhere.join("keep").is_file());
}

#[test]
fn leaves_a_requested_target_directory_alone_even_at_the_default_place() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");

    // `--target-dir target` names the default location and still means the
    // caller chose it. Cargo prefers that flag over the CARGO_TARGET_DIR
    // placement would set, so relocating would leave cargo writing one
    // place while the shim mapped another -- measured as a build that
    // looked nothing up and stored almost nothing.
    assert!(place(&config, &workspace, &workspace.join("target"), true).is_none());

    assert!(!workspace.join("target").exists());
    assert_eq!(stats(&config.target.root).unwrap(), ViewStats::default());
}

#[test]
fn leaves_a_target_directory_someone_else_chose_where_it_is() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");

    // A flag, the environment, or a cargo configuration put it here, and
    // that outranks any placement of ours.
    let elsewhere = directory.path().join("chosen");
    assert!(place(&config, &workspace, &elsewhere, false).is_none());
    assert_eq!(stats(&config.target.root).unwrap(), ViewStats::default());
}

#[test]
fn refuses_to_displace_real_build_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let existing = workspace.join("target");
    std::fs::create_dir_all(existing.join("debug")).unwrap();
    std::fs::write(existing.join("debug/libfixture.rlib"), b"outputs").unwrap();

    assert!(place(&config, &workspace, &existing, false).is_none());

    assert!(
        existing.join("debug/libfixture.rlib").exists(),
        "somebody's build outputs are not ours to move or delete"
    );
    assert_eq!(
        stats(&config.target.root).unwrap(),
        ViewStats::default(),
        "a refusal that leaves a directory and a record behind would report \
             a managed target directory for a checkout nothing manages"
    );
}

#[test]
fn a_refusal_leaves_an_earlier_placement_alone() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();
    std::fs::write(managed.join("artifact"), b"outputs").unwrap();

    // Somebody replaced the link with a directory of their own. The
    // placement already on disk still owns a full target directory, and
    // dropping its record would leave that directory untraceable -- which
    // means never collected.
    remove_link(&workspace.join("target")).unwrap();
    std::fs::create_dir_all(workspace.join("target")).unwrap();

    assert!(place(&config, &workspace, &workspace.join("target"), false).is_none());

    assert_eq!(stats(&config.target.root).unwrap().views, 1);
    assert!(managed.join("artifact").exists());
    std::fs::remove_dir_all(&workspace).unwrap();
    assert_eq!(
        prune(&config.target.root).unwrap().removed_views,
        1,
        "the earlier placement must still be collectable"
    );
}

#[test]
fn frees_the_target_directory_of_a_checkout_that_is_gone() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let gone = checkout(directory.path(), "gone");
    let staying = checkout(directory.path(), "staying");

    let removed = place(&config, &gone, &gone.join("target"), false).unwrap();
    let kept = place(&config, &staying, &staying.join("target"), false).unwrap();
    std::fs::write(removed.join("artifact"), vec![0_u8; 512]).unwrap();
    std::fs::write(kept.join("artifact"), vec![0_u8; 256]).unwrap();
    std::fs::remove_dir_all(&gone).unwrap();

    let outcome = prune(&config.target.root).unwrap();

    assert_eq!(
        outcome,
        PruneOutcome {
            removed_views: 1,
            removed_bytes: 512,
            removed_stale_views: 1,
            remaining_bytes: 256,
            ..PruneOutcome::default()
        }
    );
    assert!(!removed.exists());
    assert!(kept.join("artifact").exists());
    assert_eq!(stats(&config.target.root).unwrap().views, 1);
}

#[test]
fn keeps_the_target_directory_of_an_idle_checkout() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();
    std::fs::write(managed.join("artifact"), b"outputs").unwrap();

    let outcome = prune(&config.target.root).unwrap();

    assert_eq!(outcome.remaining_bytes, 7);
    assert_eq!(outcome.removed_views, 0);
    assert!(managed.join("artifact").exists());
}

#[test]
fn target_budget_collects_oldest_live_view_first() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let old_workspace = checkout(directory.path(), "old");
    let new_workspace = checkout(directory.path(), "new");
    let old = place(
        &config,
        &old_workspace,
        &old_workspace.join("target"),
        false,
    )
    .unwrap();
    let new = place(
        &config,
        &new_workspace,
        &new_workspace.join("target"),
        false,
    )
    .unwrap();
    std::fs::write(old.join("artifact"), vec![0_u8; 5]).unwrap();
    std::fs::write(new.join("artifact"), vec![0_u8; 10]).unwrap();
    let old_record = view_record_path(&config.target.root, &old_workspace);
    let mut record: ViewRecord =
        serde_json::from_slice(&std::fs::read(&old_record).unwrap()).unwrap();
    record.updated_secs = 1;
    std::fs::write(&old_record, serde_json::to_vec(&record).unwrap()).unwrap();

    let outcome = collect(&config.target.root, Some(10), None, false).unwrap();

    assert_eq!(outcome.removed_live_views, 1);
    assert_eq!(outcome.removed_bytes, 5);
    assert_eq!(outcome.remaining_bytes, 10);
    assert!(!old.exists());
    assert!(new.exists());
}

#[test]
fn dry_run_leaves_selected_target_views_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();
    std::fs::write(managed.join("artifact"), b"outputs").unwrap();

    let outcome = collect(&config.target.root, Some(0), None, true).unwrap();

    assert_eq!(outcome.removed_live_views, 1);
    assert!(managed.join("artifact").exists());
    assert!(view_record_path(&config.target.root, &workspace).exists());
}

#[test]
fn leaves_a_target_directory_it_cannot_trace() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_config(directory.path(), true);
    let workspace = checkout(directory.path(), "project");
    let managed = place(&config, &workspace, &workspace.join("target"), false).unwrap();
    std::fs::remove_dir_all(&workspace).unwrap();
    // `cargo clean` cannot reach the record, but a corrupt one still must
    // not turn into a licence to delete a directory full of outputs.
    std::fs::write(view_record_path(&config.target.root, &workspace), b"{").unwrap();

    assert_eq!(prune(&config.target.root).unwrap(), PruneOutcome::default());
    assert!(managed.exists());
}

#[test]
fn counts_nothing_before_anything_is_placed() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        stats(&directory.path().join("targets")).unwrap(),
        ViewStats::default()
    );
    assert_eq!(
        prune(&directory.path().join("targets")).unwrap(),
        PruneOutcome::default()
    );
}
