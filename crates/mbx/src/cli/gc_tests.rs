use super::*;
use std::time::Duration;

#[test]
fn combined_budget_respects_component_caps_and_occupied_reserves() {
    let retention = RetentionSettings {
        target_max_bytes: Some(80),
        target_max_age: None,
        incremental_max_bytes: Some(20),
        incremental_max_age: None,
        max_total_bytes: Some(100),
        target_precedence: Default::default(),
        min_free: None,
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
    let left = std::fs::read_dir(store.join("gc/v1"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("last-sweep-report")
        })
        .count();
    assert_eq!(
        left, 0,
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

#[test]
fn an_automatic_sweep_stands_down_while_another_collector_holds_the_store() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = super::cargo_tests::managed_target_config(directory.path());
    config.gc.auto = true;
    config.gc.interval = std::time::Duration::ZERO;
    let store = config.store_dir();
    let mut other = collector_lock(&store).unwrap();
    assert!(other.try_lock().unwrap());

    run_automatic(&config, &RetentionSettings::default()).unwrap();

    assert!(
        !store.join("gc/v1/last-sweep").exists(),
        "the sweep was neither claimed nor run"
    );
}

#[test]
fn sweep_reports_accumulate_until_a_build_says_them() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    for report in ["first\n", "second\n"] {
        std::fs::create_dir_all(store.join("gc/v1")).unwrap();
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(store.join(SWEEP_REPORT))
            .and_then(|mut file| std::io::Write::write_all(&mut file, report.as_bytes()))
            .unwrap();
    }

    assert_eq!(take_sweep_report(store), ["first", "second"]);
}

#[test]
fn a_build_leaves_the_report_alone_while_a_collector_writes_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path();
    crate::util::write_atomic(&store.join(SWEEP_REPORT), b"evicted 3 objects\n").unwrap();
    let mut collector = collector_lock(store).unwrap();
    assert!(collector.try_lock().unwrap());

    assert!(
        take_sweep_report(store).is_empty(),
        "not while the collector holds it"
    );

    collector.unlock().unwrap();
    assert_eq!(take_sweep_report(store), ["evicted 3 objects"]);
}

#[test]
fn leftovers_of_an_interrupted_collection_are_reported() {
    let units = crate::target::CollectionOutcome {
        removed_units: 3,
        removed_unit_bytes: 2048,
        ..Default::default()
    };
    let leftovers = crate::target::CollectionOutcome {
        removed_unit_bytes: 2048,
        ..Default::default()
    };

    assert_eq!(
        target_removals(&units, false),
        ["removed 3 unused build units from live target directories (2.0 KiB logical)"]
    );
    assert_eq!(
        target_removals(&leftovers, true),
        [
            "would remove build units an interrupted collection left in live target directories (2.0 KiB logical)"
        ]
    );
    assert!(target_removals(&crate::target::CollectionOutcome::default(), false).is_empty());
}

#[test]
fn relief_lowers_a_budget_by_the_shortfall_and_never_raises_it() {
    assert_eq!(
        relieve(Some(50), 80, 0),
        Some(50),
        "no shortfall, no change"
    );
    assert_eq!(
        relieve(Some(50), 80, 10),
        Some(50),
        "the budget already frees more"
    );
    assert_eq!(relieve(Some(50), 80, 40), Some(40));
    assert_eq!(
        relieve(None, 80, 40),
        Some(40),
        "an unlimited budget is limited"
    );
    assert_eq!(
        relieve(Some(50), 30, 40),
        Some(0),
        "everything collectable goes"
    );
}

/// A minimum no disk can meet, so every probe finds the disk short.
fn always_short() -> RetentionSettings {
    RetentionSettings {
        target_max_bytes: None,
        target_max_age: None,
        incremental_max_bytes: None,
        incremental_max_age: None,
        max_total_bytes: None,
        min_free: Some(crate::config::MinFree::Bytes(u64::MAX / 2)),
        target_precedence: Default::default(),
    }
}

#[test]
fn a_short_disk_is_found_only_when_a_minimum_is_set() {
    let directory = tempfile::tempdir().unwrap();
    let disk = low_disk(&always_short(), directory.path()).expect("the disk is short");
    assert!(disk.shortfall() > 0);
    assert!(
        disk.describe(false).contains("under the"),
        "{}",
        disk.describe(false)
    );

    let off = RetentionSettings {
        min_free: None,
        ..always_short()
    };
    assert_eq!(low_disk(&off, directory.path()), None);
    let met = RetentionSettings {
        min_free: Some(crate::config::MinFree::Bytes(0)),
        ..always_short()
    };
    assert_eq!(low_disk(&met, directory.path()), None);
}

#[test]
fn a_short_disk_brings_the_next_sweep_forward() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = super::cargo_tests::managed_target_config(directory.path());
    config.gc.interval = Duration::from_secs(3600);

    assert_eq!(sweep_interval(&config, &always_short()), LOW_DISK_INTERVAL);
    let off = RetentionSettings {
        min_free: None,
        ..always_short()
    };
    assert_eq!(sweep_interval(&config, &off), config.gc.interval);
    config.gc.interval = Duration::ZERO;
    assert_eq!(
        sweep_interval(&config, &always_short()),
        Duration::ZERO,
        "a shorter configured interval stands"
    );
}

#[test]
fn a_short_disk_collects_live_targets_past_their_budget() {
    let directory = tempfile::tempdir().unwrap();
    let config = super::cargo_tests::managed_target_config(directory.path());
    let mut views = Vec::new();
    for (name, updated_secs) in [("older", 1), ("newer", 2)] {
        let workspace = directory.path().join(name);
        std::fs::create_dir_all(&workspace).unwrap();
        let view = crate::target::place(&config, &workspace, &workspace.join("target"), false)
            .expect("the target is managed");
        std::fs::write(view.join("artifact"), vec![0_u8; 64]).unwrap();
        // Long ago, so neither reads as a build that is starting, and in
        // order, so `newer` is the most recently used.
        let record = view.with_extension("json");
        let mut fields: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
        fields["updated_secs"] = updated_secs.into();
        std::fs::write(&record, serde_json::to_vec(&fields).unwrap()).unwrap();
        views.push(view);
    }

    let unlimited = RetentionSettings {
        min_free: None,
        ..always_short()
    };
    let report = prune_targets(&config, &unlimited, config.gc.max_bytes);
    assert_eq!(report.freed_bytes, 0, "no budget and room to spare");
    assert!(views.iter().all(|view| view.exists()));

    let report = prune_targets(&config, &always_short(), config.gc.max_bytes);

    assert!(!views[0].exists(), "the older checkout's target went");
    assert!(views[1].exists(), "the most recently used one is kept");
    assert_eq!(report.freed_bytes, 64);
    assert!(
        report.removals[0].contains("under the"),
        "the report says why: {:?}",
        report.removals
    );
}

#[test]
fn a_directory_and_one_inside_it_are_on_the_same_disk() {
    let directory = tempfile::tempdir().unwrap();
    let nested = directory.path().join("not/created/yet");

    assert!(crate::util::same_disk(directory.path(), &nested));
    #[cfg(target_os = "linux")]
    assert!(
        !crate::util::same_disk(directory.path(), Path::new("/proc")),
        "procfs is a filesystem of its own"
    );
}

#[test]
fn a_combined_budget_shares_unoccupied_store_capacity() {
    for automatic in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut config = super::cargo_tests::managed_target_config(directory.path());
        config.gc.auto = true;
        config.gc.interval = Duration::ZERO;
        config.gc.max_bytes = 160;
        let retention = RetentionSettings {
            max_total_bytes: Some(160),
            min_free: None,
            ..always_short()
        };
        let mut views = Vec::new();
        for (name, updated_secs) in [("older", 1), ("newer", 2)] {
            let workspace = directory.path().join(name);
            std::fs::create_dir_all(&workspace).unwrap();
            let view = crate::target::place(&config, &workspace, &workspace.join("target"), false)
                .unwrap();
            std::fs::write(view.join("artifact"), [0_u8; 64]).unwrap();
            let record = view.with_extension("json");
            let mut fields: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
            fields["updated_secs"] = updated_secs.into();
            std::fs::write(&record, serde_json::to_vec(&fields).unwrap()).unwrap();
            views.push(view);
        }
        let collect = || {
            if automatic {
                sweep_store(&config, &retention);
            } else {
                gc::run(&config, config.gc.max_bytes, false, false, &retention).unwrap();
            }
        };
        collect();
        assert!(
            views.iter().all(|view| view.exists()),
            "an empty store leaves all 128 target bytes within budget"
        );

        // Store occupancy, rather than its potential 160-byte allowance,
        // now puts the combined cache over budget.
        let objects = config.store_dir().join("cas/v1");
        std::fs::create_dir_all(&objects).unwrap();
        std::fs::write(objects.join("object"), [0_u8; 64]).unwrap();
        assert_eq!(occupied_store_budget(&config, &retention, 160), 64);
        assert_eq!(occupied_store_budget(&config, &retention, 32), 32);
        gc::run(&config, config.gc.max_bytes, true, false, &retention).unwrap();
        assert!(
            views.iter().all(|view| view.exists()),
            "dry runs preserve targets"
        );
        collect();
        assert!(
            !views[0].exists(),
            "the older target makes room for the occupied store"
        );
        assert!(views[1].exists(), "the newest target remains protected");
        let tight = RetentionSettings {
            max_total_bytes: Some(32),
            ..retention
        };
        let sweep = sweep_store(&config, &tight);
        assert!(
            views[1].exists(),
            "even an impossible budget preserves the newest target"
        );
        assert!(
            sweep
                .lines
                .iter()
                .any(|line| line.contains("over gc.max_total_size")),
            "the next build receives the budget warning: {:?}",
            sweep.lines
        );
    }
}
