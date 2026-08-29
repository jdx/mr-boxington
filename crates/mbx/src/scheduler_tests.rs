use super::*;

fn pool_at(dir: &Path, capacity: u64, bytes_per_permit: u64) -> Pool {
    let mut pool = Pool::new(
        dir.to_path_buf(),
        capacity,
        bytes_per_permit,
        SchedulerPriority::Normal,
    );
    // Tests must not depend on how much memory the machine running them has
    // free; anything that wants the gate closed injects its own probe.
    pool.available_memory = || None;
    pool
}

#[test]
fn capacity_is_enforced_and_released() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 2, 0);

    let first = pool.try_admit(1, None).unwrap().expect("first permit");
    let second = pool.try_admit(1, None).unwrap().expect("second permit");
    assert!(
        pool.try_admit(1, None).unwrap().is_none(),
        "a full pool must refuse"
    );

    drop(first);
    let third = pool.try_admit(1, None).unwrap();
    assert!(third.is_some(), "a released permit frees capacity");
    drop(second);
    drop(third);

    // Not merely unlocked: a released lease leaves no file behind. Windows
    // refuses to delete a file anyone still holds open, so a permit that only
    // unlocked its handle would leave one of these per compilation there --
    // capacity would stay correct and the directory would grow forever.
    let leases: Vec<_> = std::fs::read_dir(directory.path().join(LEASES_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(leases.is_empty(), "released leases are removed: {leases:?}");
}

#[test]
fn leases_never_collide_with_a_name_already_taken() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 0);
    let leases = directory.path().join(LEASES_DIR);

    let held: Vec<_> = (0..4)
        .map(|_| pool.try_admit(1, None).unwrap().expect("permit"))
        .collect();
    let names: std::collections::BTreeSet<_> = std::fs::read_dir(&leases)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 4, "every live lease has its own file");

    // A pid is unique only inside its namespace, so a name that is merely
    // "pid plus counter" collides between two containers sharing a cache --
    // silently truncating a live holder's record. Stood in for by a lease
    // somebody else holds the lock on, since that is what makes the name
    // genuinely taken: an unlocked file of the same name is a dead holder's,
    // and reclaiming it is correct.
    let squatted = leases.join(format!("{}-{}-{}", std::process::id(), process_token(), 99));
    std::fs::write(&squatted, b"another namespace's lease").unwrap();
    let mut elsewhere = fslock::LockFile::open(&squatted).unwrap();
    assert!(
        elsewhere.try_lock().unwrap(),
        "the stand-in holds its lease"
    );

    LEASE_NONCE.store(99, Ordering::Relaxed);
    let next = pool.try_admit(1, None).unwrap().expect("permit");
    assert_eq!(
        std::fs::read(&squatted).unwrap(),
        b"another namespace's lease",
        "a name in use is left exactly as it was found"
    );
    drop(next);
    drop(held);
}

#[test]
fn weights_count_against_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 0);

    let heavy = pool.try_admit(3, None).unwrap().expect("heavy permit");
    let light = pool.try_admit(1, None).unwrap().expect("last permit");
    assert!(pool.try_admit(1, None).unwrap().is_none());
    drop(heavy);
    assert!(pool.try_admit(3, None).unwrap().is_some());
    drop(light);
}

#[test]
fn a_dead_holders_lease_is_reclaimed() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 1, 0);

    // A lease file nobody holds a lock on is what a killed shim leaves behind.
    let leases = directory.path().join(LEASES_DIR);
    std::fs::create_dir_all(&leases).unwrap();
    let stale = leases.join("999999-0");
    std::fs::write(
        &stale,
        serde_json::to_vec(&Lease {
            version: LEASE_VERSION,
            weight: 1,
            priority: "normal".into(),
        })
        .unwrap(),
    )
    .unwrap();

    let permit = pool.try_admit(1, None).unwrap();
    assert!(permit.is_some(), "a stale lease must not hold capacity");
    assert!(!stale.exists(), "the stale lease is removed");
}

#[test]
fn a_demand_heavier_than_the_pool_runs_alone_rather_than_never() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 2, 0);

    // Heavier than the whole machine, and gated by a probe that reports no
    // memory at all: it still compiles, alone.
    let mut short = pool_at(directory.path(), 2, 0);
    short.available_memory = || Some(0);
    let permit = short.try_admit(10, Some(u64::MAX)).unwrap();
    assert!(permit.is_some(), "an oversized demand is admitted alone");
    // And while it runs, nothing else fits.
    assert!(pool.try_admit(1, None).unwrap().is_none());
}

#[test]
fn low_priority_does_not_take_an_idle_pool_out_from_under_a_waiter() {
    let directory = tempfile::tempdir().unwrap();
    let mut low = Pool::new(directory.path().to_path_buf(), 4, 0, SchedulerPriority::Low);
    low.available_memory = || None;
    std::fs::write(directory.path().join(PRIORITY_WAIT_STAMP), b"").unwrap();

    // An idle pool is the ordinary state between two compilations, so it is
    // no licence to take the whole machine: the reserve still applies.
    assert!(
        low.try_admit(4, None).unwrap().is_none(),
        "the reserve holds even when nothing is running"
    );
    assert!(
        low.try_admit(3, None).unwrap().is_some(),
        "what fits beside the reserve is still admitted"
    );
}

#[test]
fn the_reserve_never_swallows_the_whole_pool() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join(PRIORITY_WAIT_STAMP), b"").unwrap();

    // A quarter of one permit rounds up to one, which would leave a
    // single-permit machine reserving everything -- and then the only way a
    // low-priority build could ever run would be the oversized-demand escape,
    // which hands it the whole machine at once. Yielding must not invert.
    for capacity in 1..=8 {
        let mut low = Pool::new(
            directory.path().to_path_buf(),
            capacity,
            0,
            SchedulerPriority::Low,
        );
        low.available_memory = || None;
        assert!(
            low.reserved() < capacity,
            "capacity {capacity} reserved all of itself"
        );
        let permit = low.try_admit(1, None).unwrap();
        assert!(
            permit.is_some(),
            "low priority makes progress at capacity {capacity}"
        );
    }
}

#[test]
fn the_memory_gate_defers_predicted_heavy_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut pool = pool_at(directory.path(), 8, 0);
    pool.available_memory = || Some(1024);

    let running = pool.try_admit(1, None).unwrap().expect("first permit");
    assert!(
        pool.try_admit(2, Some(2048)).unwrap().is_none(),
        "a predicted-heavy job defers while memory is short"
    );
    assert!(
        pool.try_admit(2, Some(512)).unwrap().is_some(),
        "a prediction that fits is admitted"
    );
    assert!(
        pool.try_admit(2, None).unwrap().is_some(),
        "an unpredicted job is not gated"
    );
    drop(running);
}

#[test]
fn low_priority_leaves_the_reserve_while_a_normal_build_waits() {
    let directory = tempfile::tempdir().unwrap();
    let normal = pool_at(directory.path(), 4, 0);
    let mut low = Pool::new(directory.path().to_path_buf(), 4, 0, SchedulerPriority::Low);
    low.available_memory = || None;

    let held = normal.try_admit(2, None).unwrap().expect("two permits");
    std::fs::write(directory.path().join(PRIORITY_WAIT_STAMP), b"").unwrap();

    // capacity 4 minus a reserve of 1: the two held plus one more would fit,
    // but low priority must leave the reserve for whoever stamped.
    assert!(low.try_admit(2, None).unwrap().is_none());
    let admitted = low.try_admit(1, None).unwrap();
    assert!(admitted.is_some(), "low priority still uses spare capacity");
    // The normal build takes the reserved permit low priority left alone.
    assert!(normal.try_admit(1, None).unwrap().is_some());
    drop(held);
    drop(admitted);
}

#[test]
fn plans_weigh_history_links_and_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let bytes_per_permit = 1024;
    let pool = pool_at(directory.path(), 8, bytes_per_permit);
    pool.record_peak("heavy", 3500).unwrap();
    pool.record_peak("enormous", 1024 * 1024).unwrap();

    let (weight, predicted) = pool.plan(&Demand::new("unseen", false));
    assert_eq!((weight, predicted), (1, None));

    let (weight, predicted) = pool.plan(&Demand::new("unseen", true));
    assert_eq!(
        (weight, predicted),
        (LINK_WEIGHT, Some(LINK_WEIGHT * bytes_per_permit)),
        "an unmeasured link starts at the link floor"
    );

    let (weight, predicted) = pool.plan(&Demand::new("heavy", false));
    assert_eq!((weight, predicted), (4, Some(3500)));

    let (weight, predicted) = pool.plan(&Demand::new("heavy", true));
    assert_eq!(
        (weight, predicted),
        (4, Some(3500)),
        "history above the link floor wins"
    );

    let (weight, _) = pool.plan(&Demand::new("enormous", false));
    assert_eq!(weight, 8, "weights clamp to the capacity");

    // Without a memory budget there is no ledger and no prediction; only the
    // static link floor remains.
    let unweighted = pool_at(directory.path(), 8, 0);
    assert_eq!(unweighted.plan(&Demand::new("heavy", false)), (1, None));
    assert_eq!(
        unweighted.plan(&Demand::new("heavy", true)),
        (LINK_WEIGHT, None)
    );
}

#[test]
fn the_ledger_only_remembers_what_matters_and_never_shrinks_a_peak() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 1000);

    // Under one permit's worth of memory: not worth a ledger entry.
    assert_eq!(ledger_peak(900, false, 1000), None);
    // Over it: recorded as measured.
    assert_eq!(ledger_peak(1500, false, 1000), Some(1500));

    pool.record_peak("crate", 1500).unwrap();
    pool.record_peak("crate", 1200).unwrap();
    assert_eq!(
        read_ledger(&pool.ledger_path()).crates.get("crate"),
        Some(&1500),
        "a lower later measurement never shrinks the recorded peak"
    );

    let garbage = pool.ledger_path();
    std::fs::write(&garbage, b"not json").unwrap();
    assert!(read_ledger(&garbage).crates.is_empty());
    pool.record_peak("crate", 1500).unwrap();
    assert_eq!(read_ledger(&garbage).crates.get("crate"), Some(&1500));
}

#[test]
fn an_oom_kill_escalates_past_what_was_measured() {
    // The killer stopped it at 1500; the record says it needs more than that.
    assert_eq!(ledger_peak(1500, true, 1000), Some(3000));
    // Even a tiny measurement escalates past one permit, so the weight rises.
    assert_eq!(ledger_peak(10, true, 1000), Some(1001));
}

#[test]
fn the_ledger_drops_its_smallest_entries_at_the_cap() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 1);

    for index in 0..MAX_LEDGER_ENTRIES + 10 {
        pool.record_peak(&format!("crate-{index}"), 100 + index as u64)
            .unwrap();
    }
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.crates.len(), MAX_LEDGER_ENTRIES);
    assert!(
        !ledger.crates.contains_key("crate-0"),
        "the smallest entries go first"
    );
    let largest = format!("crate-{}", MAX_LEDGER_ENTRIES + 9);
    assert!(ledger.crates.contains_key(&largest));
}

#[test]
fn session_environment_states_off_explicitly() {
    let config = crate::config::Config::for_test(Path::new("/cache"));
    assert_eq!(
        session_environment(&config),
        vec![(SCHED_DIR_ENV.to_string(), String::new())]
    );

    let mut config = crate::config::Config::for_test(Path::new("/cache"));
    config.scheduler.enabled = true;
    config.scheduler.cpus = 8;
    config.scheduler.memory_bytes = Some(8000);
    let environment = session_environment(&config);
    let value = |name: &str| {
        environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap()
    };
    assert_eq!(
        value(SCHED_DIR_ENV),
        Path::new("/cache").join("scheduler").to_str().unwrap()
    );
    assert_eq!(value(SCHED_SLOTS_ENV), "8");
    assert_eq!(value(SCHED_SLOT_BYTES_ENV), "1000");
    assert_eq!(value(SCHED_PRIORITY_ENV), "normal");
}
