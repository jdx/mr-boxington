use super::*;

#[test]
fn combined_budget_reserves_the_full_action_store_allowance() {
    let retention = RetentionSettings {
        target_max_bytes: Some(80),
        target_max_age: None,
        incremental_max_bytes: Some(20),
        incremental_max_age: None,
        max_total_bytes: Some(100),
    };

    assert_eq!(target_budget(&retention, 70, 10), Some(20));
    assert_eq!(target_budget(&retention, 120, 10), Some(0));
    assert_eq!(incremental_budget(&retention, 70), Some(20));
    assert_eq!(incremental_budget(&retention, 90), Some(10));
    assert_eq!(incremental_budget(&retention, 120), Some(0));
    assert_eq!(store_budget(&retention, 70, 30), 70);
    assert_eq!(store_budget(&retention, 70, 60), 40);
}

#[test]
fn the_sweep_report_is_said_once() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    crate::util::write_atomic(
        &store.join(SWEEP_REPORT),
        b"removed 2 target directories\n\nevicted 3 objects\n",
    )
    .unwrap();

    assert_eq!(
        take_sweep_report(store),
        vec![
            "removed 2 target directories".to_string(),
            "evicted 3 objects".to_string()
        ]
    );
    assert!(
        take_sweep_report(store).is_empty(),
        "the second reader finds nothing"
    );
    assert!(
        std::fs::read_dir(store.join("gc/v1"))
            .unwrap()
            .next()
            .is_none(),
        "neither the report nor its claimed copy is left behind"
    );
}

#[test]
fn no_report_means_nothing_to_say() {
    let directory = tempfile::tempdir().unwrap();

    assert!(take_sweep_report(directory.path()).is_empty());
}

#[test]
fn an_automatic_sweep_that_is_not_due_leaves_no_report() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = super::cargo_tests::managed_target_config(directory.path());
    config.gc.auto = true;
    config.gc.interval = std::time::Duration::from_secs(3600);
    assert!(crate::store::claim_sweep(&config.store_dir(), config.gc.interval).unwrap());

    run_automatic(&config, &RetentionSettings::default()).unwrap();

    assert!(!config.store_dir().join(SWEEP_REPORT).exists());
}
