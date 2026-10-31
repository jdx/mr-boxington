use super::*;

fn wait_until(description: &str, mut ready: impl FnMut() -> bool) {
    // Wait for the event itself; the deadline only catches a stuck worker.
    let started = Instant::now();
    while !ready() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "timed out waiting for {description}"
        );
        std::thread::yield_now();
    }
}

fn pool_at(dir: &Path, capacity: u64, bytes_per_permit: u64) -> Pool {
    let mut pool = Pool::new(
        dir.to_path_buf(),
        capacity,
        bytes_per_permit,
        SchedulerPriority::Normal,
    );
    // Tests must not depend on how much memory the machine running them has
    // free; anything that wants the gate closed injects its own probe.
    pool.domain_capacity = capacity;
    pool.legacy_domain_capacity = capacity;
    pool.domain_memory_capacity = capacity.saturating_mul(bytes_per_permit);
    pool.available_memory = || None;
    pool.pressure = false;
    pool
}

/// A low-priority pool with the same host independence as `pool_at`.
fn low_pool_at(dir: &Path, capacity: u64) -> Pool {
    let mut pool = pool_at(dir, capacity, 0);
    pool.priority = SchedulerPriority::Low;
    pool
}

fn calibrate_kind(pool: &Pool, kind: &str) {
    let mut ledger = read_ledger(&pool.ledger_path());
    for index in 0..10 {
        let key = match kind {
            "link" => format!("calibration-{index} [link]"),
            "test-binary" => format!("calibration-{index} [test]"),
            _ => format!("calibration-{index} [compile mode]"),
        };
        ledger.memory_windows.insert(key, vec![100, 145, 213, 320]);
    }
    // These fixtures establish only the calibration threshold; keep recorded
    // spread values stable for tests that check exact estimator vectors.
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
}

fn encode_history_map(binary: &str, records: &BTreeMap<u64, TestMeasurement>) -> Vec<u8> {
    let records: Vec<_> = records
        .iter()
        .map(|(hash, measurement)| TestHistoryRecord {
            hash: *hash,
            measurement: *measurement,
        })
        .collect();
    encode_test_history(binary, &records)
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
    // This test's own counter: the process-wide one is advanced by every test
    // running in parallel, which could hand the stand-in's name to a lease
    // below or skip past it, and then the test would not exercise a collision.
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let directory = tempfile::tempdir().unwrap();
    let mut pool = pool_at(directory.path(), 8, 0);
    pool.lease_nonce = || NONCE.fetch_add(1, Ordering::Relaxed);
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
    let taken = NONCE.load(Ordering::Relaxed);
    let squatted = leases.join(format!(
        "{}-{}-{taken}",
        std::process::id(),
        process_token()
    ));
    std::fs::write(&squatted, b"another namespace's lease").unwrap();
    let mut elsewhere = fslock::LockFile::open(&squatted).unwrap();
    assert!(
        elsewhere.try_lock().unwrap(),
        "the stand-in holds its lease"
    );

    let next = pool.try_admit(1, None).unwrap().expect("permit");
    assert_eq!(
        NONCE.load(Ordering::Relaxed),
        taken + 2,
        "the grant tried the taken name and moved on to the next"
    );
    assert!(
        leases
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                process_token(),
                taken + 1
            ))
            .exists(),
        "the successful retry uses the next nonce"
    );
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
fn a_reservation_charges_cpu_and_memory_separately() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 1024);

    let reservation = pool.reserve(Some(1), Some(3 * 1024)).unwrap();
    let cpu_only =
        vector_attempt(&pool, 2, 0).expect("memory reservations leave CPU capacity free");
    assert!(
        vector_attempt(&pool, 1, 2 * 1024).is_none(),
        "memory still has its own limit"
    );
    drop(cpu_only);
    drop(reservation);
    assert!(pool.try_admit(4, None).unwrap().is_some());
}

#[test]
fn a_reservation_requires_a_requested_resource() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 1024);

    assert!(pool.reserve(None, None).is_err());
}

#[test]
fn a_memory_reservation_needs_a_memory_budget() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 0);

    assert!(pool.reserve(None, Some(1024)).is_err());
}

#[test]
fn a_waiting_normal_reservation_holds_low_priority_work_back() {
    let directory = tempfile::tempdir().unwrap();
    let normal = pool_at(directory.path(), 1, 0);
    let held = normal.try_admit(1, None).unwrap().expect("first permit");
    let waiting_dir = directory.path().to_path_buf();
    let waiter = std::thread::spawn(move || {
        let pool = pool_at(&waiting_dir, 1, 0);
        pool.reserve(Some(1), None).unwrap()
    });

    wait_until(
        "the normal reservation to announce it is waiting for capacity",
        || directory.path().join(PRIORITY_WAIT_STAMP).exists(),
    );
    drop(held);
    drop(waiter.join().unwrap());
}

#[test]
fn a_build_limit_counts_only_that_builds_leases() {
    let directory = tempfile::tempdir().unwrap();
    let mut other = pool_at(directory.path(), 6, 0);
    other.build = Some(("other".into(), 6));
    let elsewhere = other.try_admit(2, None).unwrap().expect("other build");

    let mut limited = pool_at(directory.path(), 6, 0);
    limited.build = Some(("limited".into(), 2));
    let own = limited
        .try_admit(2, None)
        .unwrap()
        .expect("two permits beside another build");
    assert!(
        limited.try_admit(1, None).unwrap().is_none(),
        "the build cannot exceed its own ceiling even with machine capacity free"
    );

    drop(own);
    assert!(
        limited.try_admit(3, None).unwrap().is_some(),
        "one demand heavier than the build ceiling still makes progress"
    );
    drop(elsewhere);
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
            name: None,
            mode_key: None,
            version: LEASE_VERSION,
            weight: 1,
            cpu: None,
            memory_bytes: None,
            priority: "normal".into(),
            build_id: None,
            domain: None,
            bytes_per_permit: 0,
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
fn low_priority_leaves_the_reserve_even_on_an_idle_pool() {
    let directory = tempfile::tempdir().unwrap();
    let low = low_pool_at(directory.path(), 4);
    std::fs::write(directory.path().join(PRIORITY_WAIT_STAMP), b"").unwrap();

    // An idle pool is the ordinary state between two compilations, so it is
    // no licence to stop yielding: three of the four permits are what a
    // low-priority build may take while somebody is waiting.
    let admitted = low.try_admit(3, None).unwrap();
    assert!(admitted.is_some(), "what fits beside the reserve is taken");
    let leases = std::fs::read_dir(directory.path().join(LEASES_DIR))
        .unwrap()
        .count();
    assert_eq!(leases, 1);
    drop(admitted);
}

#[test]
fn a_weight_the_reserve_would_never_admit_still_runs_alone() {
    let directory = tempfile::tempdir().unwrap();
    let low = low_pool_at(directory.path(), 4);
    std::fs::write(directory.path().join(PRIORITY_WAIT_STAMP), b"").unwrap();

    // Four permits out of four never fits beside a reserve of one, and a
    // weight is already clamped to the capacity, so refusing here would not
    // be yielding -- it would be a compilation that never runs at all for as
    // long as anybody keeps waiting. It goes alone instead.
    assert!(
        low.try_admit(4, None).unwrap().is_some(),
        "a demand the reserve excludes outright runs by itself"
    );
}

#[test]
fn a_predicted_heavy_compile_does_not_wait_out_the_gate_on_an_idle_machine() {
    let directory = tempfile::tempdir().unwrap();
    let bytes_per_permit = 1024;
    let mut pool = pool_at(directory.path(), 4, bytes_per_permit);
    // Short of memory, and nothing running that could give any back.
    pool.available_memory = || Some(1);
    // Through `plan`, because that is the only weight production ever asks
    // for -- and it clamps to the capacity, so a rule written against
    // heavier-than-capacity weights would never fire for a real compilation.
    pool.record_peak(&ledger_key("enormous", true), bytes_per_permit * 1000, true)
        .unwrap();
    let (weight, predicted) = pool.plan(&Demand::new("enormous", true));
    assert_eq!(
        weight, 4,
        "a clamped weight is what admission actually sees"
    );

    assert!(
        pool.try_admit(weight, predicted).unwrap().is_some(),
        "an idle machine admits it rather than waiting out the gate deadline"
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
        let low = low_pool_at(directory.path(), capacity);
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
fn a_prediction_that_exactly_fits_available_memory_is_admitted() {
    let directory = tempfile::tempdir().unwrap();
    let mut pool = pool_at(directory.path(), 8, 1024);
    pool.available_memory = || Some(1024);
    let running = pool.try_admit(1, None).unwrap().expect("first permit");
    let fitting = pool.try_admit(1, Some(1024)).unwrap();
    assert!(fitting.is_some(), "all predicted bytes are available");
    drop(running);
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
    let low = low_pool_at(directory.path(), 4);

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
    pool.record_peak("heavy", 3500, false).unwrap();
    pool.record_peak("enormous", 1024 * 1024, false).unwrap();

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

    // "heavy" was measured compiling to metadata, and a link of that same
    // crate must not inherit it: `cargo check` and `cargo build` name one
    // crate for two very different workloads, and the metadata-only number
    // is the smaller. Inheriting it retires the link floor with a
    // measurement that was never a link's -- over-admission with a
    // plausible number attached. An unmeasured link keeps the floor.
    let (weight, predicted) = pool.plan(&Demand::new("heavy", true));
    assert_eq!(
        (weight, predicted),
        (LINK_WEIGHT, Some(LINK_WEIGHT * bytes_per_permit)),
        "a metadata-only measurement never speaks for the link beside it"
    );

    // A link measured *as a link* is weighted by what it used, whichever
    // side of the floor that falls on -- above it,
    pool.record_peak(&ledger_key("heavy", true), 9000, true)
        .unwrap();
    let (weight, predicted) = pool.plan(&Demand::new("heavy", true));
    assert_eq!(
        (weight, predicted),
        (8, Some(9000)),
        "a link's own history above the floor wins"
    );
    // and below it, which is the floor being retired rather than raised.
    pool.record_peak(&ledger_key("light-link", true), 700, true)
        .unwrap();
    let (weight, predicted) = pool.plan(&Demand::new("light-link", true));
    assert_eq!(
        (weight, predicted),
        (1, Some(700)),
        "a link's own history below the floor also wins"
    );
    // The two entries are genuinely separate, not one overwriting the other.
    assert_eq!(
        pool.plan(&Demand::new("heavy", false)),
        (4, Some(3500)),
        "the metadata entry survives the link entry beside it"
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

/// A test binary is charged for its threads as well as its memory, and never
/// shares a ledger entry with the crate compiled under the same name.
#[test]
fn test_binaries_weigh_their_threads_and_their_own_history() {
    let directory = tempfile::tempdir().unwrap();
    let bytes_per_permit = 1000;
    let pool = pool_at(directory.path(), 8, bytes_per_permit);

    assert_eq!(
        pool.plan(&Demand::test("suite", None)),
        (4, None),
        "an unstated thread count takes half the pool"
    );
    assert_eq!(pool.plan(&Demand::test("suite", Some(3))), (3, None));
    assert_eq!(
        pool.plan(&Demand::test("suite", Some(64))),
        (8, None),
        "threads clamp to the capacity"
    );

    // The compile of `suite` measured light and the run measured heavy; each
    // is planned from its own entry.
    pool.record_peak("suite", 1500, false).unwrap();
    pool.record_peak("suite [test, 1 threads]", 6500, false)
        .unwrap();
    assert_eq!(pool.plan(&Demand::new("suite", false)), (2, Some(1500)));
    assert_eq!(
        pool.plan(&Demand::test("suite", Some(1))),
        (7, Some(6500)),
        "memory outweighs a single thread"
    );

    let odd = pool_at(directory.path(), 5, 0);
    assert_eq!(
        odd.plan(&Demand::test("suite", None)),
        (3, None),
        "half an odd pool rounds up"
    );
    assert_eq!(
        pool_at(directory.path(), 1, 0).plan(&Demand::test("suite", Some(0))),
        (1, None),
        "a demand never weighs nothing"
    );
}

/// A test binary's measured core count replaces the guess, is kept per thread
/// setting, and is never lowered by a reading taken on a busier machine.
#[test]
fn test_binaries_are_weighed_by_the_cores_they_were_measured_using() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 0);

    pool.record_cpu("suite [test]", 2).unwrap();
    assert_eq!(
        pool.plan(&Demand::test("suite", None)),
        (2, None),
        "a measurement replaces half the pool, even without a memory budget"
    );
    assert_eq!(
        pool.plan(&Demand::test("suite", Some(6))),
        (6, None),
        "a stated thread count has its own history"
    );
    assert_eq!(
        pool.plan(&Demand::new("suite", false)),
        (1, None),
        "compilations are not measured for cores"
    );

    pool.record_cpu("suite [test]", 1).unwrap();
    assert_eq!(pool.plan(&Demand::test("suite", None)), (2, None));
    pool.record_cpu("suite [test]", 12).unwrap();
    assert_eq!(
        pool.plan(&Demand::test("suite", None)),
        (8, None),
        "a higher reading raises it, clamped to the capacity"
    );
}

#[test]
fn average_cores_round_to_the_nearest_core() {
    let seconds = Duration::from_secs_f64;
    assert_eq!(average_cores(seconds(0.01), seconds(2.0)), 1);
    assert_eq!(average_cores(seconds(2.8), seconds(2.0)), 1);
    assert_eq!(average_cores(seconds(3.2), seconds(2.0)), 2);
    assert_eq!(average_cores(seconds(15.0), seconds(2.0)), 8);
    assert_eq!(average_cores(seconds(1.0), Duration::ZERO), 1);
}

/// Every test binary is its own crate name, so a cold `cargo test --no-run`
/// never finds a per-crate entry for the link in front of it. What the machine
/// has learned about links in general is what it has to go on instead.
#[test]
fn an_unmeasured_link_is_weighed_by_what_this_machine_s_links_have_cost() {
    let directory = tempfile::tempdir().unwrap();
    let bytes_per_permit = 1000;
    let pool = pool_at(directory.path(), 8, bytes_per_permit);

    // A cold key can use the existing profile window from its first sample.
    pool.record_peak(&ledger_key("first", true), 5000, true)
        .unwrap();
    pool.record_peak(&ledger_key("second", true), 4000, true)
        .unwrap();
    assert_eq!(
        pool.plan(&Demand::new("unseen", true)),
        (5, Some(5000)),
        "any available profile samples supply the cold link fallback"
    );

    // More samples preserve the maximum as the cold-key guess.
    pool.record_peak(&ledger_key("third", true), 3000, true)
        .unwrap();
    assert_eq!(
        pool.plan(&Demand::new("unseen", true)),
        (5, Some(5000)),
        "an unmeasured link is weighed by the heaviest link this machine made"
    );

    // A link this machine has actually measured still speaks for itself.
    pool.record_peak(&ledger_key("known", true), 1500, true)
        .unwrap();
    assert_eq!(
        pool.plan(&Demand::new("known", true)),
        (2, Some(1500)),
        "a crate's own calibrated window outranks the machine's"
    );

    // And nothing about this reaches a compilation that never links.
    assert_eq!(pool.plan(&Demand::new("unseen", false)), (1, None));
}

/// The window is what links cost *lately*, so it forgets in order and keeps
/// recording even when the crate's own entry has nothing to learn.
#[test]
fn the_link_window_is_bounded_and_records_every_link() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 1000);

    for index in 0..MAX_LINK_SAMPLES + 4 {
        pool.record_peak(&ledger_key("crate", true), 1000 + index as u64, true)
            .unwrap();
    }
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.link_peaks.len(), MAX_LINK_SAMPLES);
    assert_eq!(
        ledger.link_peaks.first().copied(),
        Some(1000 + 4),
        "the oldest measurements are the ones dropped"
    );

    // A measurement no higher than the crate's recorded peak leaves the entry
    // alone, but the window still saw a link happen.
    let before = read_ledger(&pool.ledger_path()).link_peaks.len();
    pool.record_peak(&ledger_key("crate", true), 1, true)
        .unwrap();
    let after = read_ledger(&pool.ledger_path());
    assert_eq!(after.link_peaks.len(), before);
    assert_eq!(after.link_peaks.last().copied(), Some(1));
    assert_eq!(
        after.crates.get(&ledger_key("crate", true)).copied(),
        Some(1000 + MAX_LINK_SAMPLES as u64 + 3),
        "a lower measurement never shrinks the recorded peak"
    );
}

/// A repeat compilation that teaches the ledger nothing writes nothing: the
/// file is rewritten under a lock, and every crate in a build would otherwise
/// pay for it on every build after the first.
#[test]
fn a_measurement_that_changes_nothing_leaves_the_ledger_alone() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 1000);
    pool.record_peak("crate", 4000, false).unwrap();

    let written = || {
        std::fs::metadata(pool.ledger_path())
            .and_then(|metadata| metadata.modified())
            .unwrap()
    };
    let before = written();
    // Coarse filesystem timestamps would hide a rewrite that happened within
    // the same tick, so the check is on the bytes as well.
    let bytes = std::fs::read(pool.ledger_path()).unwrap();
    pool.record_peak("crate", 1000, false).unwrap();
    assert_eq!(std::fs::read(pool.ledger_path()).unwrap(), bytes);
    assert_eq!(written().duration_since(before).unwrap().as_nanos(), 0);

    // A link, though, always has a window entry to add.
    pool.record_peak(&ledger_key("crate", true), 1, true)
        .unwrap();
    assert_eq!(
        read_ledger(&pool.ledger_path()).link_peaks,
        vec![1],
        "a link's measurement lands in the window even when it teaches the entry nothing"
    );
}

#[test]
fn the_ledger_only_remembers_what_matters_and_never_shrinks_a_peak() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 1000);

    // Under one permit's worth of memory: not worth a ledger entry.
    assert_eq!(ledger_peak(900, false, false, true, 1000), None);
    // Over it: recorded as measured.
    assert_eq!(ledger_peak(1500, false, false, true, 1000), Some(1500));
    // A link that succeeded is recorded even light, because that is what
    // retires its floor.
    assert_eq!(ledger_peak(900, false, true, true, 1000), Some(900));

    pool.record_peak("crate", 1500, false).unwrap();
    pool.record_peak("crate", 1200, false).unwrap();
    assert_eq!(
        read_ledger(&pool.ledger_path()).crates.get("crate"),
        Some(&1500),
        "a lower later measurement never shrinks the recorded peak"
    );

    let garbage = pool.ledger_path();
    std::fs::write(&garbage, b"not json").unwrap();
    assert!(read_ledger(&garbage).crates.is_empty());
    pool.record_peak("crate", 1500, false).unwrap();
    assert_eq!(read_ledger(&garbage).crates.get("crate"), Some(&1500));
}

#[test]
fn an_oom_kill_escalates_past_what_was_measured() {
    // The killer stopped it at 1500; the record says it needs more than that.
    assert_eq!(ledger_peak(1500, true, false, false, 1000), Some(3000));
    // Even a tiny measurement escalates past one permit, so the weight rises.
    assert_eq!(ledger_peak(10, true, false, false, 1000), Some(1001));
    // A killed link escalates too, rather than recording what it reached.
    assert_eq!(ledger_peak(1500, true, true, false, 1000), Some(3000));
}

#[test]
fn a_link_that_never_reached_the_linker_cannot_cheapen_the_ones_that_do() {
    // Whether an invocation links is its crate type, not its outcome: a
    // binary that dies in type checking is a "link" that never ran one. Its
    // small peak must not retire the floor -- machine-wide, for every later
    // build sharing the cache -- on the strength of a compilation that never
    // did the expensive thing.
    assert_eq!(
        ledger_peak(900, false, true, false, 1000),
        None,
        "a failed link records nothing it could later be admitted on"
    );
    // The failure can still say something is heavy, which is the direction
    // that keeps a link that died reaching for memory from repeating itself.
    assert_eq!(ledger_peak(4000, false, true, false, 1000), Some(4000));
    // And the successful link of the same shape still retires its floor.
    assert_eq!(ledger_peak(900, false, true, true, 1000), Some(900));
}

#[test]
fn the_ledger_drops_its_smallest_entries_at_the_cap() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 1);

    for index in 0..MAX_LEDGER_ENTRIES + 10 {
        pool.record_peak(&format!("crate-{index}"), 100 + index as u64, false)
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

#[cfg(target_os = "linux")]
#[test]
fn a_flight_leaves_its_prediction_for_the_next_build() {
    let directory = tempfile::tempdir().unwrap();

    let first = flight_at(directory.path(), "rustc", "abc123").unwrap();
    assert!(!first.waited(), "an empty pool has nothing to wait on");
    assert_eq!(first.inherited(), None, "nothing was left before");
    first.leave("the prediction");
    drop(first);

    let second = flight_at(directory.path(), "rustc", "abc123").unwrap();
    assert!(!second.waited(), "the first flight released its lock");
    assert_eq!(
        second.inherited(),
        Some("the prediction"),
        "what a flight leaves outlives it"
    );
}

#[test]
fn a_waiter_wakes_to_what_its_holder_left() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path().to_path_buf();

    let holder = flight_at(&dir, "rustc", "abc123").unwrap();
    holder.leave("previous prediction");
    let blocked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter_blocked = blocked.clone();
    let waiter = std::thread::spawn(move || {
        flight_at_inner(
            &dir,
            "rustc",
            "abc123",
            "crate",
            FlightWait::default(),
            || {
                waiter_blocked.store(true, Ordering::Release);
            },
            |_| {},
        )
        .unwrap()
    });
    wait_until(
        "the second flight to find the first flight's lock held",
        || blocked.load(Ordering::Acquire),
    );
    holder.leave("freshly published");
    drop(holder);

    let waiter = waiter.join().unwrap();
    assert!(waiter.waited(), "the second flight blocked on the first");
    assert_eq!(waiter.inherited(), Some("freshly published"));
    assert!(waiter.published_while_waiting());
}

#[test]
fn a_record_that_does_not_describe_this_flight_is_not_inherited() {
    let directory = tempfile::tempdir().unwrap();

    let holder = flight_at(directory.path(), "rustc", "abc123").unwrap();
    holder.leave("a rustc prediction");
    drop(holder);

    // Same file name could never collide across these, but the record itself
    // must also say what it is: a version bump or a corrupt write reads as
    // nothing rather than as a prediction.
    let other = flight_at(directory.path(), "cc", "abc123").unwrap();
    assert_eq!(other.inherited(), None);
    drop(other);

    let path = directory
        .path()
        .join(INFLIGHT_DIR)
        .join("rustc-abc123.prediction");
    std::fs::write(&path, b"not json").unwrap();
    let corrupt = flight_at(directory.path(), "rustc", "abc123").unwrap();
    assert_eq!(corrupt.inherited(), None, "garbage reads as nothing");
}

#[test]
fn the_sweep_keeps_fresh_and_held_flights_and_drops_the_rest() {
    let cache = tempfile::tempdir().unwrap();
    let scheduler = cache.path().join(SCHEDULER_DIR);
    let inflight = scheduler.join(INFLIGHT_DIR);

    let old = flight_at(&scheduler, "rustc", "old").unwrap();
    old.leave("stale");
    drop(old);
    let fresh = flight_at(&scheduler, "rustc", "fresh").unwrap();
    fresh.leave("current");
    drop(fresh);
    let held = flight_at(&scheduler, "rustc", "held").unwrap();

    let backdate = |name: &str| {
        let ancient = std::time::SystemTime::now() - FLIGHT_MAX_AGE - Duration::from_secs(60);
        for path in [
            inflight.join(name),
            inflight.join(format!("{name}.prediction")),
            inflight.join(format!("{name}.attempt")),
        ] {
            if path.exists() {
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(ancient)
                    .unwrap();
            }
        }
    };
    backdate("rustc-old");
    backdate("rustc-held");

    prune_flights(cache.path());

    assert!(!inflight.join("rustc-old").exists(), "stale flights go");
    assert!(!inflight.join("rustc-old.prediction").exists());
    assert!(!inflight.join("rustc-old.attempt").exists());
    assert!(inflight.join("rustc-fresh").exists(), "fresh flights stay");
    assert!(inflight.join("rustc-fresh.prediction").exists());
    assert!(
        inflight.join("rustc-held").exists(),
        "a lock that cannot be taken is a live compilation, whatever its age"
    );
    drop(held);
}

#[test]
fn an_oversized_payload_is_not_left_behind() {
    let directory = tempfile::tempdir().unwrap();

    let holder = flight_at(directory.path(), "rustc", "abc123").unwrap();
    holder.leave(&"x".repeat(MAX_FLIGHT_PAYLOAD + 1));
    drop(holder);

    let next = flight_at(directory.path(), "rustc", "abc123").unwrap();
    assert_eq!(next.inherited(), None);
}

#[test]
fn session_environment_states_off_explicitly() {
    let config = crate::config::Config::for_test(Path::new("/cache"));
    assert_eq!(
        session_environment_with_jobs(&config, None),
        vec![(SCHED_DIR_ENV.to_string(), String::new())]
    );

    let mut config = crate::config::Config::for_test(Path::new("/cache"));
    config.scheduler.enabled = true;
    config.scheduler.cpus = 8;
    config.scheduler.reserve_cpus = 2;
    config.scheduler.memory_bytes = Some(8000);
    let environment = session_environment_with_jobs(&config, None);
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
    assert_eq!(value(SCHED_SLOTS_ENV), "6");
    assert_eq!(value(SCHED_SLOT_BYTES_ENV), "1333");
    assert_eq!(value(SCHED_PRIORITY_ENV), "normal");
    assert!(!value(SCHED_BUILD_ID_ENV).is_empty());
    assert_eq!(value(SCHED_BUILD_SLOTS_ENV), "6");

    let environment = session_environment_with_jobs(&config, Some(3));
    let value = |name: &str| {
        environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap()
    };
    assert_eq!(value(SCHED_SLOTS_ENV), "6", "the machine pool stays global");
    assert_eq!(value(SCHED_SLOT_BYTES_ENV), "1333");
    assert_eq!(value(SCHED_BUILD_SLOTS_ENV), "3");
}

#[test]
fn pressure_blocks_unknown_demands_but_keeps_idle_progress() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CLOCK: AtomicU64 = AtomicU64::new(1_000);
    let dir = tempfile::tempdir().unwrap();
    let mut pool = pool_at(dir.path(), 4, 100);
    pool.pressure = true;
    pool.clock = || CLOCK.load(Ordering::SeqCst);
    pool.pressure_probe = || crate::pressure::Reading {
        total: Some(100),
        available: Some(0),
        ..Default::default()
    };
    let first = pool.try_admit(1, None).unwrap().unwrap();
    CLOCK.store(1_500, Ordering::SeqCst);
    assert!(pool.try_admit(1, None).unwrap().is_none());
    // Even a demand with no history, or an expired prediction gate, waits.
    CLOCK.store(2_000, Ordering::SeqCst);
    assert!(pool.try_admit(1, None).unwrap().is_none());
    drop(first);
    let first = pool.try_admit(1, None).unwrap().unwrap();
    pool.pressure = false;
    let second = pool.try_admit(1, None).unwrap().unwrap();
    drop((first, second));
}

#[test]
fn recovery_spaces_admissions_and_probe_failure_fails_open() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CLOCK: AtomicU64 = AtomicU64::new(1_000);
    let dir = tempfile::tempdir().unwrap();
    let mut pool = pool_at(dir.path(), 8, 100);
    pool.pressure = true;
    pool.clock = || CLOCK.load(Ordering::SeqCst);
    pool.pressure_probe = || crate::pressure::Reading {
        total: Some(100),
        available: Some(0),
        ..Default::default()
    };
    let first = pool.try_admit(1, None).unwrap().unwrap();
    CLOCK.store(1_500, Ordering::SeqCst);
    assert!(pool.try_admit(1, None).unwrap().is_none());
    pool.pressure_probe = || crate::pressure::Reading {
        total: Some(100),
        available: Some(50),
        ..Default::default()
    };
    for now in (2_000..7_000).step_by(500) {
        CLOCK.store(now, Ordering::SeqCst);
        assert!(pool.try_admit(1, None).unwrap().is_none());
    }
    CLOCK.store(7_000, Ordering::SeqCst);
    let second = pool.try_admit(1, None).unwrap().unwrap();
    assert!(pool.try_admit(1, None).unwrap().is_none());
    CLOCK.store(7_500, Ordering::SeqCst);
    let third = pool.try_admit(1, None).unwrap().unwrap();
    pool.pressure_probe = Default::default;
    CLOCK.store(8_000, Ordering::SeqCst);
    assert!(pool.try_admit(1, None).unwrap().is_some());
    drop((first, second, third));
}

#[test]
fn containers_sharing_a_pool_are_gated_by_their_own_pressure() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CLOCK: AtomicU64 = AtomicU64::new(1_000);
    fn container(counter: u64) -> crate::pressure::Reading {
        crate::pressure::Reading {
            total: Some(100),
            available: Some(50),
            // The same pathname names a different cgroup in each container.
            stalls: [("/sys/fs/cgroup/memory.pressure".into(), counter)].into(),
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let container_pool = |domain, probe| {
        let mut pool = pool_at(directory.path(), 64, 100);
        pool.pressure = true;
        pool.pressure_domain = domain;
        pool.pressure_probe = probe;
        pool.clock = || CLOCK.load(Ordering::SeqCst);
        pool
    };
    // Three idle containers whose stall counters never move, as in the
    // reported build, plus one that is genuinely short of memory.
    let idle = [
        container_pool("one", || container(21_764_622)),
        container_pool("two", || container(200_000_000)),
        container_pool("three", || container(393_795_795)),
    ];
    let short = container_pool("short", || crate::pressure::Reading {
        total: Some(100),
        available: Some(0),
        ..Default::default()
    });
    let held = short
        .try_admit(1, None)
        .unwrap()
        .expect("an idle pool admits");
    CLOCK.store(1_500, Ordering::SeqCst);
    assert!(short.try_admit(1, None).unwrap().is_none());
    for (step, now) in (2_000..60_000).step_by(500).enumerate() {
        CLOCK.store(now, Ordering::SeqCst);
        let permit = idle[step % 3].try_admit(1, None).unwrap();
        assert!(
            permit.is_some(),
            "another container's pressure gated this one"
        );
        assert!(short.try_admit(1, None).unwrap().is_none());
    }
    drop(held);
}

#[test]
fn an_oversized_idle_grant_spaces_the_next_pools_admission() {
    let directory = tempfile::tempdir().unwrap();
    let mut low = pool_at(directory.path(), 4, 100);
    low.pressure = true;
    low.priority = SchedulerPriority::Low;
    low.clock = || 7_000;
    std::fs::write(directory.path().join(PRIORITY_WAIT_STAMP), b"").unwrap();
    // Establish a recovered state without granting a compilation in the ramp.
    let state = crate::pressure::domain_state(directory.path(), low.pressure_domain);
    crate::pressure::sample(&state, 1_000, || crate::pressure::Reading {
        total: Some(100),
        available: Some(0),
        ..Default::default()
    })
    .unwrap();
    crate::pressure::sample(&state, 1_500, || crate::pressure::Reading {
        total: Some(100),
        available: Some(0),
        ..Default::default()
    })
    .unwrap();
    for now in (2_000..=7_000).step_by(500) {
        crate::pressure::sample(&state, now, || crate::pressure::Reading {
            total: Some(100),
            available: Some(50),
            ..Default::default()
        })
        .unwrap();
    }
    let first = low.try_admit(4, None).unwrap().unwrap();
    let mut wider = pool_at(directory.path(), 8, 100);
    wider.pressure = true;
    wider.clock = || 7_000;
    assert!(wider.try_admit(1, None).unwrap().is_none());
    drop(first);
}

#[test]
fn frozen_work_has_admission_priority_until_its_heartbeat_expires() {
    let directory = tempfile::tempdir().unwrap();
    let mut pool = pool_at(directory.path(), 4, 100);
    pool.pressure = true;
    pool.pressure_probe = Default::default;
    pool.clock = || 1_000;
    let first = pool.try_admit(1, None).unwrap().unwrap();
    let pending = directory.path().join("suspended");
    std::fs::create_dir(&pending).unwrap();
    std::fs::write(pending.join("generation"), "1000").unwrap();
    assert!(pool.try_admit(1, None).unwrap().is_none());
    // A dead supervisor cannot hold admissions closed indefinitely.
    pool.clock = || 4_000;
    assert!(pool.try_admit(1, None).unwrap().is_some());
    drop(first);
}

#[test]
fn link_profile_separates_release_from_dev_links() {
    let args = |flags: &[&str]| -> Vec<std::ffi::OsString> {
        flags.iter().map(std::ffi::OsString::from).collect()
    };
    let release = args(&["-C", "opt-level=3", "-C", "lto", "-C", "codegen-units=1"]);
    let dev = args(&["-C", "debuginfo=0", "-C", "opt-level=0"]);
    let release_demand = Demand::new("fallow", true).with_link_profile(&release);
    let dev_demand = Demand::new("fallow", true).with_link_profile(&dev);
    assert_ne!(
        ledger_key(&release_demand.name, true),
        ledger_key(&dev_demand.name, true)
    );

    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 1024);
    pool.record_peak(&ledger_key(&release_demand.name, true), 10_000_000, true)
        .unwrap();
    let (_, predicted) = pool.plan(&dev_demand);
    assert_ne!(
        predicted,
        Some(10_000_000),
        "a release peak must not charge a dev link"
    );
    let (_, predicted) = pool.plan(&release_demand);
    // Before link calibration, the window peak is the warmup cap.
    assert_eq!(predicted, Some(10_000_000));

    // Spelled `-Clto=fat` or as separate arguments, the profile is the same;
    // non-links are untouched.
    assert_eq!(
        link_profile(&args(&["-Clto=fat", "-Copt-level=3", "-Ccodegen-units=1"])),
        link_profile(&release)
    );
    assert_eq!(
        Demand::new("lib", false).with_link_profile(&release).name,
        "lib"
    );
}

#[test]
fn link_profile_reads_every_spelling_and_keeps_windows_apart() {
    let args = |flags: &[&str]| -> Vec<std::ffi::OsString> {
        flags.iter().map(std::ffi::OsString::from).collect()
    };
    assert_eq!(
        link_profile(&args(&["-O"])),
        link_profile(&args(&["-C", "opt-level=3"]))
    );
    assert_eq!(
        link_profile(&args(&["--codegen=lto", "--codegen", "codegen-units=1"])),
        link_profile(&args(&["-Clto", "-Ccodegen-units=1"]))
    );
    assert_ne!(
        link_profile(&args(&[])),
        link_profile(&args(&["-C", "lto=off"])),
        "an absent lto setting is not an explicit off"
    );

    // Three release links must not make a never-measured dev link
    // inherit a release-sized prediction.
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 1024);
    let release = Demand::new("bin", true).with_link_profile(&args(&["-Clto", "-Copt-level=3"]));
    for _ in 0..3 {
        pool.record_link_peak(
            &ledger_key(&release.name, true),
            1_000_000,
            true,
            release.profile.as_deref(),
        )
        .unwrap();
    }
    let dev = Demand::new("bin", true).with_link_profile(&args(&["-Copt-level=0"]));
    let (_, predicted) = pool.plan(&dev);
    assert_eq!(predicted, Some(LINK_WEIGHT * 1024));
    let (_, predicted) = pool.plan(&release);
    assert!(predicted.unwrap() >= 1_000_000);
}

#[test]
fn nextest_history_only_raises_measurements() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 100);
    let binary = "project/package::integration";
    pool.record_test_history(binary, "test_a", Some(20), Some(2))
        .unwrap();
    pool.record_test_history(binary, "test_a", Some(10), Some(1))
        .unwrap();
    let path = test_history_path(&directory.path().join(TEST_HISTORY_DIR), binary);
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        TEST_HISTORY_HEADER_LEN + TEST_HISTORY_RECORD_LEN
    );

    pool.record_test_history(binary, "test_a", Some(25), Some(3))
        .unwrap();
    let records = read_test_history_records(&path, binary);
    assert_eq!(
        records
            .iter()
            .find(|record| record.hash == test_history_hash("test_a"))
            .map(|record| record.measurement),
        Some(TestMeasurement {
            peak: Some(25),
            cores: Some(3),
        })
    );
}

#[test]
fn nextest_history_binary_search_finds_first_last_and_missing_records() {
    let directory = tempfile::tempdir().unwrap();
    for count in [1, 2, 512] {
        let binary = format!("project/package-{count}");
        let names: Vec<_> = (0..count).map(|index| format!("test_{index}")).collect();
        let mut records = BTreeMap::new();
        for (index, name) in names.iter().enumerate() {
            records.insert(
                test_history_hash(name),
                TestMeasurement {
                    peak: Some(index as u64 + 10),
                    cores: Some(index as u64 % 8 + 1),
                },
            );
        }
        assert_eq!(records.len(), count);
        let path = test_history_path(directory.path(), &binary);
        std::fs::write(&path, encode_history_map(&binary, &records)).unwrap();

        let mut ordered: Vec<_> = names
            .iter()
            .map(|name| (test_history_hash(name), name.as_str()))
            .collect();
        ordered.sort_unstable_by_key(|(hash, _)| *hash);
        for (_, name) in [ordered[0], ordered[count - 1]] {
            let lookup = lookup_test_history(&path, &binary, name);
            assert_eq!(lookup.test, Some(records[&test_history_hash(name)]));
            assert_eq!(lookup.header.count, count as u64);
        }
        let missing = "not-in-this-file";
        assert!(!records.contains_key(&test_history_hash(missing)));
        assert_eq!(
            lookup_test_history(&path, &binary, missing).test,
            None,
            "missing hash in {count}-record file"
        );
    }
}

#[test]
fn nextest_plan_uses_header_maxima_for_a_new_test() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 100);
    let binary = "project/bin";
    pool.record_test_history(binary, "heavy", Some(450), Some(3))
        .unwrap();
    pool.record_test_history(binary, "small", Some(100), Some(1))
        .unwrap();

    let new_test = Demand::nextest(binary, "new");
    assert_eq!(pool.plan(&new_test), (5, Some(450)));
    assert_eq!(
        new_test.per_test.as_ref().unwrap().known.get(),
        Some(TestMeasurement::default()),
        "a missing test keeps its own measurement unknown"
    );
}

#[test]
fn nextest_history_rejects_bad_headers_and_ignores_partial_tail() {
    let directory = tempfile::tempdir().unwrap();
    let binary = "binary-a";
    let path = test_history_path(directory.path(), binary);
    let records = BTreeMap::from([
        (
            test_history_hash("first"),
            TestMeasurement {
                peak: Some(12),
                cores: Some(2),
            },
        ),
        (
            test_history_hash("last"),
            TestMeasurement {
                peak: Some(42),
                cores: Some(4),
            },
        ),
    ]);
    let bytes = encode_history_map(binary, &records);

    std::fs::write(&path, []).unwrap();
    assert_eq!(lookup_test_history(&path, binary, "first").header.count, 0);
    std::fs::write(&path, &bytes[..17]).unwrap();
    assert_eq!(lookup_test_history(&path, binary, "first").header.count, 0);

    let mut bad_magic = bytes.clone();
    bad_magic[0] ^= 1;
    std::fs::write(&path, bad_magic).unwrap();
    assert_eq!(lookup_test_history(&path, binary, "first").header.count, 0);

    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        lookup_test_history(&path, "binary-b", "first").header.count,
        0
    );

    let mut partial_tail = bytes;
    partial_tail.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7]);
    std::fs::write(&path, partial_tail).unwrap();
    let lookup = lookup_test_history(&path, binary, "last");
    assert_eq!(lookup.header.count, 2);
    assert_eq!(lookup.test, Some(records[&test_history_hash("last")]));
    assert_eq!(read_test_history_records(&path, binary).len(), 2);

    let pool = pool_at(directory.path(), 8, 100);
    let collision_path = test_history_path(&pool.dir.join(TEST_HISTORY_DIR), binary);
    std::fs::create_dir_all(collision_path.parent().unwrap()).unwrap();
    std::fs::write(&collision_path, encode_history_map("binary-b", &records)).unwrap();
    pool.record_test_history(binary, "replacement", Some(7), None)
        .unwrap();
    let rewritten = read_test_history_records(&collision_path, binary);
    assert_eq!(rewritten.len(), 1);
    assert_eq!(rewritten[0].hash, test_history_hash("replacement"));
    assert_eq!(
        read_test_history_header(&File::open(collision_path).unwrap(), binary)
            .unwrap()
            .maximum
            .peak,
        Some(7)
    );
}

#[test]
fn nextest_history_cap_drops_the_smallest_peak_first() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 100);
    let binary = "binary-with-many-tests";
    let path = test_history_path(&pool.dir.join(TEST_HISTORY_DIR), binary);
    let records: BTreeMap<_, _> = (1..=MAX_TEST_HISTORY_ENTRIES as u64 + 1)
        .map(|hash| {
            (
                hash,
                TestMeasurement {
                    peak: Some(hash),
                    cores: Some(1),
                },
            )
        })
        .collect();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, encode_history_map(binary, &records)).unwrap();

    let new_test = "new-test";
    let new_hash = test_history_hash(new_test);
    assert!(!records.contains_key(&new_hash));
    pool.record_test_history(
        binary,
        new_test,
        Some(MAX_TEST_HISTORY_ENTRIES as u64 + 10),
        None,
    )
    .unwrap();
    let capped = read_test_history_records(&path, binary);
    assert_eq!(capped.len(), MAX_TEST_HISTORY_ENTRIES);
    assert!(
        !capped.iter().any(|record| record.hash == 1),
        "the smallest peak should be dropped"
    );
    assert!(capped.iter().any(|record| record.hash == new_hash));
    assert_eq!(
        std::fs::metadata(path).unwrap().len(),
        TEST_HISTORY_HEADER_LEN + TEST_HISTORY_RECORD_LEN * MAX_TEST_HISTORY_ENTRIES as u64
    );
}

#[test]
fn nextest_history_cap_breaks_equal_peak_ties_by_cores() {
    let mut records = vec![
        TestHistoryRecord {
            hash: 1,
            measurement: TestMeasurement {
                peak: Some(10),
                cores: Some(1),
            },
        },
        TestHistoryRecord {
            hash: 2,
            measurement: TestMeasurement {
                peak: Some(10),
                cores: Some(2),
            },
        },
        TestHistoryRecord {
            hash: 3,
            measurement: TestMeasurement {
                peak: Some(20),
                cores: Some(1),
            },
        },
    ];
    assert!(enforce_test_history_cap(&mut records, 2));
    assert!(!records.iter().any(|record| record.hash == 1));
    assert!(records.iter().any(|record| record.hash == 2));
    assert!(records.iter().any(|record| record.hash == 3));
}

#[test]
fn nextest_plan_uses_test_then_sibling_history_and_cold_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 8, 100);
    let measured = Demand::nextest("project/bin", "known");
    pool.record_test_history("project/bin", "known", Some(350), Some(3))
        .unwrap();
    assert_eq!(pool.plan(&measured), (4, Some(350)));

    let sibling = Demand::nextest("project/bin", "new");
    assert_eq!(pool.plan(&sibling), (4, Some(350)));

    let cold = Demand::nextest("project/cold", "first");
    assert_eq!(pool.plan(&cold), (2, Some(200)));
    let no_memory = pool_at(directory.path(), 8, 0);
    assert_eq!(no_memory.plan(&cold), (1, None));
}

#[test]
fn nextest_plan_clamps_to_capacity_and_ignores_suite_ledger_history() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 100);
    let nextest = Demand::nextest("project/bin", "case");
    assert_eq!(pool.plan(&nextest), (2, Some(200)));

    pool.record_test_history("project/bin", "case", Some(900), Some(12))
        .unwrap();
    assert!(read_ledger(&pool.ledger_path()).crates.is_empty());
    assert!(read_ledger(&pool.ledger_path()).cpus.is_empty());
    assert_eq!(pool.plan(&nextest), (4, Some(900)));

    pool.record_peak(&nextest.name, 9_000, false).unwrap();
    pool.record_cpu(&nextest.name, 9).unwrap();
    assert_eq!(pool.plan(&nextest), (4, Some(900)));

    let suite = Demand::test("project/bin", None);
    pool.record_peak(&suite.name, 100, false).unwrap();
    pool.record_cpu(&suite.name, 3).unwrap();
    assert_eq!(pool.plan(&suite), (3, Some(100)));
    let history = read_test_history_records(
        &test_history_path(&pool.dir.join(TEST_HISTORY_DIR), "project/bin"),
        "project/bin",
    );
    assert_eq!(history.len(), 1);
    assert!(
        history
            .iter()
            .any(|record| record.hash == test_history_hash("case"))
    );
}

#[test]
fn nextest_plan_clamps_heavy_measurements_to_pool_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 100);
    pool.record_test_history("bin", "heavy", Some(900), Some(12))
        .unwrap();
    assert_eq!(pool.plan(&Demand::nextest("bin", "heavy")), (4, Some(900)));
}

/// Set a file's modification time. Opened for writing, because Windows
/// refuses to change the times of a file opened read-only.
fn backdate(path: &Path, time: std::time::SystemTime) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap();
}

#[test]
fn nextest_history_pruning_removes_old_regular_files() {
    use std::time::{Duration, SystemTime};

    let directory = tempfile::tempdir().unwrap();
    let scheduler = directory.path().join(SCHEDULER_DIR);
    let history_dir = scheduler.join(TEST_HISTORY_DIR);
    std::fs::create_dir_all(&history_dir).unwrap();
    let stale = history_dir.join("stale.hist");
    let legacy = history_dir.join("legacy.jsonl");
    let recent = history_dir.join("recent.hist");
    std::fs::write(&stale, b"old\n").unwrap();
    std::fs::write(&legacy, b"old\n").unwrap();
    std::fs::write(&recent, b"new\n").unwrap();
    let old = SystemTime::now() - FLIGHT_MAX_AGE - Duration::from_secs(1);
    backdate(&stale, old);
    backdate(&legacy, old);

    // A writer holding the history lock keeps even stale history for now.
    let mut held = fslock::LockFile::open(&scheduler.join(TEST_HISTORY_LOCK)).unwrap();
    held.lock().unwrap();
    prune_test_history(directory.path());
    assert!(stale.exists());
    assert!(legacy.exists());
    drop(held);

    prune_test_history(directory.path());
    assert!(!stale.exists());
    assert!(!legacy.exists());
    assert!(recent.exists());
    // The lock is shared by every writer and is never removed.
    assert!(scheduler.join(TEST_HISTORY_LOCK).exists());
}

#[test]
fn planning_keeps_history_a_stable_suite_still_uses_from_looking_unused() {
    use std::time::{Duration, SystemTime};

    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 100);
    pool.record_test_history("bin", "case", Some(50), Some(1))
        .unwrap();
    let path = test_history_path(&pool.dir.join(TEST_HISTORY_DIR), "bin");
    let age = |path: &Path| {
        std::fs::metadata(path)
            .unwrap()
            .modified()
            .unwrap()
            .elapsed()
            .unwrap_or_default()
    };
    let old = SystemTime::now() - FLIGHT_MAX_AGE + Duration::from_secs(60);
    backdate(&path, old);

    pool.plan(&Demand::nextest("bin", "case"));
    assert!(age(&path) < TEST_HISTORY_REFRESH);
    prune_test_history(directory.path());
    assert!(path.exists());
}

#[test]
fn a_test_that_raises_nothing_records_without_the_history_lock() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 100);
    pool.record_test_history("bin", "case", Some(50), Some(2))
        .unwrap();
    let demand = Demand::nextest("bin", "case");
    pool.plan(&demand);
    let identity = demand.per_test.as_ref().unwrap();

    // Held elsewhere for the whole call: a recording that needed it would
    // block here instead of returning.
    let mut held = fslock::LockFile::open(&pool.dir.join(TEST_HISTORY_LOCK)).unwrap();
    held.lock().unwrap();
    pool.record_test_measurement(identity, Some(40), Some(2))
        .unwrap();
    pool.record_test_measurement(identity, None, None).unwrap();
    drop(held);

    pool.record_test_measurement(identity, Some(80), None)
        .unwrap();
    let history = read_test_history_records(
        &test_history_path(&pool.dir.join(TEST_HISTORY_DIR), "bin"),
        "bin",
    );
    assert_eq!(
        history
            .iter()
            .find(|record| record.hash == test_history_hash("case"))
            .map(|record| record.measurement),
        Some(TestMeasurement {
            peak: Some(80),
            cores: Some(2)
        })
    );
}

// Regression: S40's eight-permit analysis compile waited over an hour while
// newer small compiles repeatedly refilled the shared pool.
#[test]
fn a_heavy_waiter_drains_the_pool_before_new_small_compilations() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 0);
    let first = pool.try_admit(2, None).unwrap().unwrap();
    let second = pool.try_admit(1, None).unwrap().unwrap();
    let mut heavy = None;
    assert!(
        pool.try_admit_queued(4, None, &mut heavy)
            .unwrap()
            .is_none()
    );
    for _ in 0..10 {
        assert!(pool.try_admit(1, None).unwrap().is_none());
    }
    drop(first);
    assert!(pool.try_admit(1, None).unwrap().is_none());
    assert!(
        pool.try_admit_queued(4, None, &mut heavy)
            .unwrap()
            .is_none()
    );
    drop(second);
    let admitted = pool.try_admit_queued(4, None, &mut heavy).unwrap().unwrap();
    assert!(heavy.is_none());
    drop(admitted);
    assert!(pool.try_admit(1, None).unwrap().is_some());
}

#[test]
fn cancelled_and_crashed_waiters_release_their_queue_positions() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 4, 0);
    let held = pool.try_admit(3, None).unwrap().unwrap();
    let mut queued = None;
    assert!(
        pool.try_admit_queued(2, None, &mut queued)
            .unwrap()
            .is_none()
    );
    // The only live lease returns three permits; the head needs two, so
    // using the remaining one cannot delay it.
    assert!(pool.try_admit(1, None).unwrap().is_some());
    drop(queued.take());
    assert!(
        scan_waiters(&directory.path().join(WAITERS_DIR))
            .unwrap()
            .is_empty()
    );
    assert!(pool.try_admit(1, None).unwrap().is_some());
    // A full-pool waiter has zero slack, preserving the crash-recovery
    // assertion that removing its queue position lets a small request pass.
    assert!(
        pool.try_admit_queued(4, None, &mut queued)
            .unwrap()
            .is_none()
    );
    assert!(pool.try_admit(1, None).unwrap().is_none());
    // Leave the file behind with its lock released, as after process death.
    let mut crashed = queued.take().unwrap();
    drop(crashed._guard.lock.take());
    assert!(pool.try_admit(1, None).unwrap().is_some());
    drop(held);
}

#[test]
fn container_limits_cover_separate_builds_without_blocking_other_containers() {
    let directory = tempfile::tempdir().unwrap();
    let mut a = pool_at(directory.path(), 8, 0);
    a.pressure_domain = "container-a";
    a.domain_capacity = 3;
    a.build = Some(("build-a1".into(), 8));
    let mut a2 = pool_at(directory.path(), 8, 0);
    a2.pressure_domain = "container-a";
    a2.domain_capacity = 3;
    a2.build = Some(("build-a2".into(), 8));
    let mut b = pool_at(directory.path(), 8, 0);
    b.pressure_domain = "container-b";
    b.domain_capacity = 3;
    let held = a.try_admit(2, None).unwrap().unwrap();
    let mut queued = None;
    assert!(a2.try_admit_queued(3, None, &mut queued).unwrap().is_none());
    // Do not refill a's last slot; let its oldest request run. b can progress.
    assert!(a.try_admit(1, None).unwrap().is_none());
    let other = b.try_admit(3, None).unwrap().unwrap();
    assert!(b.try_admit(1, None).unwrap().is_none());
    drop(held);
    assert!(a2.try_admit_queued(3, None, &mut queued).unwrap().is_some());
    drop(other);
}

#[test]
fn a_build_ceiling_does_not_reserve_the_host_for_that_build() {
    let directory = tempfile::tempdir().unwrap();
    let mut a = pool_at(directory.path(), 8, 0);
    a.build = Some(("a".into(), 3));
    let b = pool_at(directory.path(), 8, 0);
    let held = a.try_admit(2, None).unwrap().unwrap();
    let mut queued = None;
    assert!(a.try_admit_queued(3, None, &mut queued).unwrap().is_none());
    assert!(a.try_admit(1, None).unwrap().is_none());
    assert!(b.try_admit(3, None).unwrap().is_some());
    drop(held);
    assert!(a.try_admit_queued(3, None, &mut queued).unwrap().is_some());
}

#[test]
fn an_available_memory_gate_releases_the_waiters_place() {
    let directory = tempfile::tempdir().unwrap();
    let mut pool = pool_at(directory.path(), 4, 1024);
    let held = pool.try_admit(3, None).unwrap().unwrap();
    let mut queued = None;
    assert!(
        pool.try_admit_queued(2, None, &mut queued)
            .unwrap()
            .is_none()
    );
    pool.available_memory = || Some(0);
    assert!(
        pool.try_admit_queued(2, Some(2048), &mut queued)
            .unwrap()
            .is_none()
    );
    assert!(queued.is_none());
    assert!(pool.try_admit(1, None).unwrap().is_some());
    drop(held);
}

#[test]
fn a_budget_change_preserves_the_memory_and_cpu_charges_of_live_leases() {
    let directory = tempfile::tempdir().unwrap();
    let old = pool_at(directory.path(), 4, 1024);
    let new = pool_at(directory.path(), 16, 4096);
    let large_unit = new.try_admit(1, None).unwrap().unwrap();
    // One new permit consumes all four old permits, not just one.
    assert!(old.try_admit(1, None).unwrap().is_none());
    drop(large_unit);
    let cores = old.try_admit(4, None).unwrap().unwrap();
    // Conversely, a four-core reservation cannot shrink to one core merely
    // because the memory budget per permit grew.
    assert!(new.try_admit(13, None).unwrap().is_none());
    drop(cores);
    assert!(new.try_admit(13, None).unwrap().is_some());
}

#[test]
fn queued_domain_limits_use_the_waiters_original_byte_budget() {
    let directory = tempfile::tempdir().unwrap();
    let mut old = pool_at(directory.path(), 4, 1024);
    old.pressure_domain = "a";
    let mut new = pool_at(directory.path(), 16, 4096);
    new.pressure_domain = "a";
    let held = new.try_admit(1, None).unwrap().unwrap();
    let mut queued = None;
    assert!(
        old.try_admit_queued(2, None, &mut queued)
            .unwrap()
            .is_none()
    );
    assert!(new.try_admit(1, None).unwrap().is_none());
    // The old request is waiting for container a, not for the host pool.
    new.pressure_domain = "b";
    assert!(new.try_admit(1, None).unwrap().is_some());
    drop(held);
    assert!(
        old.try_admit_queued(2, None, &mut queued)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_waiter_removed_after_its_lock_check_has_left_the_queue() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("released");
    assert!(read_waiter(&path).unwrap().is_none());
    std::fs::write(&path, b"{").unwrap();
    assert!(
        read_waiter(&path).is_err(),
        "a record that is present but unreadable must not be skipped"
    );
}

#[test]
fn an_unlocked_malformed_waiter_is_reclaimed_before_parsing() {
    let directory = tempfile::tempdir().unwrap();
    let waiters = directory.path().join(WAITERS_DIR);
    std::fs::create_dir_all(&waiters).unwrap();
    let path = waiters.join("abandoned");
    std::fs::write(&path, b"{").unwrap();

    assert!(scan_waiters(&waiters).unwrap().is_empty());
    assert!(!path.exists());
}

#[test]
fn a_locked_empty_waiter_has_left_the_queue() {
    let directory = tempfile::tempdir().unwrap();
    let waiters = directory.path().join(WAITERS_DIR);
    std::fs::create_dir_all(&waiters).unwrap();
    let path = waiters.join("released");
    std::fs::write(&path, b"").unwrap();
    let mut lock = fslock::LockFile::open(&path).unwrap();
    assert!(lock.try_lock().unwrap());

    assert!(scan_waiters(&waiters).unwrap().is_empty());
    assert!(path.exists(), "a locked waiter must not be reclaimed");
}

const GIB: u64 = 1 << 30;

#[test]
fn a_cpu_only_local_ceiling_ignores_memory() {
    assert_eq!(
        local_ceiling(16, 0, 4, Some(GIB)),
        Resources {
            cpu: 4,
            memory_bytes: 0
        }
    );
    assert_eq!(local_ceiling(16, 0, 32, Some(GIB)).cpu, 16);
}

#[test]
fn the_local_ceiling_limits_cpu_and_memory_independently() {
    let ceiling = local_ceiling(16, 64 * GIB, 8, Some(20 * GIB));
    assert_eq!(ceiling.cpu, 8);
    assert_eq!(ceiling.memory_bytes, 20 * GIB / 100 * 85);
    assert_eq!(local_ceiling(16, GIB, 2, Some(64 * GIB)).cpu, 2);
    assert_eq!(local_ceiling(3, GIB, 8, Some(64 * GIB)).cpu, 3);
    assert_eq!(local_ceiling(16, GIB, 8, None).memory_bytes, GIB);
}

#[test]
fn an_explicit_local_memory_budget_keeps_its_full_capacity() {
    let budget = 98_590_438_452;
    assert_eq!(
        local_ceiling(120, budget, 120, Some(105_444_319_232)).memory_bytes,
        budget
    );
}

#[test]
fn a_budget_equal_to_local_memory_keeps_its_full_capacity() {
    assert_eq!(
        local_ceiling(10, 100 * GIB, 10, Some(100 * GIB)).memory_bytes,
        100 * GIB
    );
}

#[test]
fn a_budget_above_local_memory_uses_the_85_percent_ceiling() {
    assert_eq!(
        local_ceiling(10, 100 * GIB + 1, 10, Some(100 * GIB)).memory_bytes,
        100 * GIB / 100 * 85
    );
}

#[test]
fn a_container_smaller_than_one_memory_permit_still_admits_one_compilation() {
    let budget = local_ceiling(16, 128 * GIB, 8, Some(4 * GIB));
    assert_eq!(budget.cpu, 8);
    assert!(budget.fits(
        Resources::default(),
        Resources {
            cpu: 1,
            memory_bytes: 8 * GIB
        }
    ));
    assert!(!budget.fits(
        Resources {
            cpu: 1,
            memory_bytes: 8 * GIB
        },
        Resources {
            cpu: 1,
            memory_bytes: GIB
        }
    ));
}

fn compile_demand(name: &str, arguments: &[&str]) -> Demand {
    Demand::new(name, false).with_link_profile(
        &arguments
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>(),
    )
}

#[test]
fn compile_mode_keys_cover_effective_rustc_settings() {
    let mode = |args: &[&str]| compile_demand("crate", args).compile_mode.unwrap();
    assert_eq!(
        mode(&[]),
        "crate [compile emit=link crate-type= opt0 lto-default cgu- debug0 incremental-off]"
    );
    assert_eq!(
        mode(&["--emit=dep-info,metadata"]),
        "crate [compile emit=metadata crate-type= opt0 lto-default cgu- debug0 incremental-off]"
    );
    assert!(
        mode(&["--emit", "metadata=out,dep-info=dep"])
            .contains("compile emit=metadata crate-type=")
    );
    assert!(
        mode(&["--emit=metadata,llvm-ir"]).contains("compile emit=llvm-ir,metadata crate-type=")
    );
    assert_eq!(
        mode(&[
            "--emit=metadata",
            "--emit=link",
            "--emit",
            "metadata",
            "--test",
            "-O",
            "-Copt-level=1",
            "--codegen=opt-level=2",
            "-C",
            "opt-level=0",
            "-Clto=fat",
            "--codegen",
            "lto=off",
            "-Ccodegen-units=16",
            "-Ccodegen-units=1",
            "-g",
            "-Cdebuginfo=line-tables-only",
            "-Cincremental=first",
            "--codegen=incremental=",
            "-Zthreads=8",
            "-Z",
            "threads=2"
        ]),
        "crate [compile emit=link,metadata crate-type= test opt0 lto-off cgu1 debugline-tables-only incremental-off threads=2]"
    );
    assert!(mode(&["-C", "incremental=second"]).contains("incremental-on"));
    for args in [
        vec!["--emit=link"],
        vec!["--test"],
        vec!["-Copt-level=3"],
        vec!["-Clto"],
        vec!["-Ccodegen-units=2"],
        vec!["-g"],
        vec!["-Cincremental=dir"],
        vec!["-Zthreads=4"],
    ] {
        let baseline = if args == ["--emit=link"] {
            mode(&["--emit=metadata"])
        } else {
            mode(&[])
        };
        assert_ne!(mode(&args), baseline, "{args:?}");
    }
}

#[test]
fn compile_keys_do_not_collide_with_existing_workloads() {
    let demand = compile_demand("crate", &["--test", "--emit=metadata"]);
    let key = demand.compile_mode.unwrap();
    let link = Demand::new("crate", true).with_link_profile(&[]);
    for old in [
        ledger_key("crate", false),
        ledger_key("crate", true),
        ledger_key(&link.name, true),
        Demand::test("crate", None).name,
        Demand::test("crate", Some(2)).name,
    ] {
        assert_ne!(key, old);
    }
    assert!(link.compile_mode.is_none());
    assert!(Demand::test("crate", None).compile_mode.is_none());
}

#[test]
fn compile_modes_use_the_window_from_the_first_successful_sample() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let demand = compile_demand("heavy", &["--emit=metadata"]);
    pool.record_peak("heavy", 1000, false).unwrap();
    assert_eq!(pool.plan(&demand), (10, Some(1000)));
    for count in 1..=4 {
        pool.record_compile(&demand, 81, false, true).unwrap();
        // After three successes, the previous rule's mode headroom caps the
        // uncalibrated estimate at 122 bytes.
        let charge = [366_u64, 245, 122, 122][count as usize - 1];
        let expected = (charge.div_ceil(100), Some(charge));
        assert_eq!(pool.plan(&demand), expected);
        let ledger = read_ledger(&pool.ledger_path());
        assert_eq!(
            ledger.compile_samples[demand.compile_mode.as_ref().unwrap()],
            count.min(3)
        );
        assert_eq!(ledger.crates["heavy"], 1000);
    }
}

#[test]
fn compile_window_ignores_legacy_caps_and_handles_a_missing_bare_peak() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let demand = compile_demand("heavy", &[]);
    for _ in 0..3 {
        pool.record_compile(&demand, 401, false, true).unwrap();
    }
    // The bare crate entry keeps this window at its measured 401-byte peak.
    assert_eq!(pool.plan(&demand), (5, Some(401)));
    let key = demand.compile_mode.as_ref().unwrap();
    let mut ledger = read_ledger(&pool.ledger_path());
    ledger.crates.remove("heavy");
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
    // Without the bare entry, the previous mode rule supplies 602 bytes.
    assert_eq!(pool.plan(&demand), (7, Some(602)));
    ledger.crates.insert(key.clone(), u64::MAX);
    assert_eq!(
        legacy_compile_prediction(&ledger, "heavy", key),
        (Some(u64::MAX), true)
    );
}

#[test]
fn recreating_a_missing_fallback_covers_all_retained_modes() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let old = compile_demand("heavy", &["--emit=metadata"]);
    for _ in 0..3 {
        pool.record_compile(&old, 1000, false, true).unwrap();
    }
    let mut ledger = read_ledger(&pool.ledger_path());
    ledger.crates.remove("heavy");
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
    let new = compile_demand("heavy", &[]);
    pool.record_compile(&new, 200, false, true).unwrap();
    assert_eq!(read_ledger(&pool.ledger_path()).crates["heavy"], 1000);
    assert_eq!(pool.plan(&old), (10, Some(1000)));
}

#[test]
fn compile_failures_and_oom_raise_peaks_without_successful_samples() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let demand = compile_demand("heavy", &[]);
    let key = demand.compile_mode.as_ref().unwrap();
    pool.record_compile(&demand, 200, false, false).unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.crates["heavy"], 200);
    assert_eq!(ledger.crates[key], 200);
    assert!(!ledger.compile_samples.contains_key(key));
    pool.record_compile(&demand, 301, true, false).unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.crates["heavy"], 602);
    assert_eq!(ledger.crates[key], 602);
    assert!(!ledger.compile_samples.contains_key(key));
    pool.record_compile(&demand, 1, true, false).unwrap();
    assert_eq!(read_ledger(&pool.ledger_path()).crates[key], 602);
    assert_eq!(pool.plan(&demand), (7, Some(602)));
}

#[test]
fn light_crates_learn_windows_without_legacy_peaks_and_cpu_only_does_not_learn() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let demand = compile_demand("light", &[]);
    for _ in 0..3 {
        pool.record_compile(&demand, 99, false, true).unwrap();
    }
    assert!(read_ledger(&pool.ledger_path()).crates.is_empty());
    assert!(directory.path().join(LEDGER_LOCK).exists());
    // Warm-up caps the light crate at the one-permit default the previous rule
    // charged; its uncapped bound would be 248 bytes.
    assert_eq!(pool.plan(&demand), (1, Some(100)));
    let cpu = pool_at(directory.path(), 100, 0);
    cpu.record_compile(&demand, 999, false, true).unwrap();
    assert!(read_ledger(&pool.ledger_path()).crates.is_empty());
}

#[test]
fn old_ledgers_drop_compile_trust_but_keep_the_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let demand = compile_demand("heavy", &[]);
    let key = demand.compile_mode.as_ref().unwrap();
    std::fs::write(
        pool.ledger_path(),
        serde_json::to_vec(&serde_json::json!({
            "version": LEDGER_VERSION, "crates": {"heavy": 1000, key: 200}, "link_peaks": []
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(pool.plan(&demand), (10, Some(1000)));
}

#[test]
fn evicted_compile_peaks_cannot_reuse_old_sample_counts() {
    let mut ledger = MemoryLedger::default();
    let key =
        "heavy [compile emit=metadata crate-type= opt0 lto-default cgu- debug0 incremental-off]";
    ledger.crates.insert("heavy".into(), 1000);
    ledger.crates.insert(key.into(), 200);
    ledger.compile_samples.insert(key.into(), 3);
    for n in 0..MAX_LEDGER_ENTRIES {
        ledger.crates.insert(format!("other{n}"), 2000);
    }
    trim_compile_ledger(&mut ledger);
    assert!(!ledger.crates.contains_key(key));
    assert!(!ledger.compile_samples.contains_key(key));
    assert!(!ledger.crates.contains_key("heavy"));
    ledger.crates.insert("heavy".into(), 1000);
    ledger.crates.insert(key.into(), 1);
    assert_eq!(
        legacy_compile_prediction(&ledger, "heavy", key),
        (Some(1000), false)
    );
}

#[test]
fn scheduler_stats_read_old_records_without_locking_or_reclaiming_them() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let demand = compile_demand("heavy", &["--emit=metadata"]);
    pool.record_peak("heavy", 1000, false).unwrap();
    for _ in 0..3 {
        pool.record_compile(&demand, 81, false, true).unwrap();
    }
    let leases = directory.path().join(LEASES_DIR);
    let waiters = directory.path().join(WAITERS_DIR);
    std::fs::create_dir_all(&leases).unwrap();
    std::fs::create_dir_all(&waiters).unwrap();
    let old_lease = br#"{"version":1,"weight":2,"priority":"normal"}"#;
    let old_waiter = br#"{"ticket":1,"weight":3,"priority":"normal","domain":"old-domain","domain_capacity":10,"bytes_per_permit":100,"build":null}"#;
    assert!(
        serde_json::from_slice::<Lease>(old_lease)
            .unwrap()
            .name
            .is_none()
    );
    assert!(
        serde_json::from_slice::<Waiting>(old_waiter)
            .unwrap()
            .name
            .is_none()
    );
    std::fs::write(leases.join("stale"), old_lease).unwrap();
    std::fs::write(waiters.join("stale"), old_waiter).unwrap();
    let held = pool
        .try_admit_named(
            Resources {
                cpu: 2,
                memory_bytes: 200,
            },
            2,
            None,
            &mut None,
            Some(&demand),
        )
        .unwrap()
        .unwrap();
    // Admission reclaimed the deliberately stale records; restore them before
    // reading stats to prove the read-only path leaves them in place.
    std::fs::write(leases.join("stale"), old_lease).unwrap();
    std::fs::write(waiters.join("stale"), old_waiter).unwrap();
    let mut queued = None;
    assert!(
        pool.try_admit_named(
            Resources {
                cpu: 100,
                memory_bytes: 10000
            },
            100,
            None,
            &mut queued,
            Some(&demand)
        )
        .unwrap()
        .is_none()
    );
    // Queue admission also reclaims stale records. Restore once more so the
    // stats reader sees both old and current records of each kind.
    std::fs::write(leases.join("stale"), old_lease).unwrap();
    std::fs::write(waiters.join("stale"), old_waiter).unwrap();
    let snapshot = scheduler_snapshot(directory.path(), 100, 100, true);
    let text = snapshot.text();
    assert!(
        text.contains("n=3, window max=81 B, bound=203 B, charge=1 cores + 122 B (window)"),
        "{text}"
    );
    assert!(text.contains("lease unknown: mode=unknown, build=unknown, domain=unknown"));
    assert!(text.contains("waiter unknown: mode=unknown, build=unknown, domain=old-domain"));
    assert!(text.contains("lease heavy: mode=heavy [compile emit=metadata crate-type="));
    assert!(text.contains("waiter heavy: mode=heavy [compile emit=metadata crate-type="));
    assert!(text.contains("age="));
    assert!(leases.join("stale").exists());
    assert!(waiters.join("stale").exists());
    let mut lock = fslock::LockFile::open(&held.path).unwrap();
    assert!(
        !lock.try_lock().unwrap(),
        "stats must leave held locks alone"
    );
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["entries"][1]["source"], "window");
    assert_eq!(json["activity"].as_array().unwrap().len(), 4);
    let cpu = scheduler_snapshot(directory.path(), 100, 0, true);
    assert_eq!(cpu.entries[1].charge_permits, None);
    assert_eq!(cpu.entries[1].charge_bytes, None);
    let mut ledger = read_ledger(&pool.ledger_path());
    ledger.compile_samples.clear();
    ledger.memory_windows.clear();
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
    let fallback = scheduler_snapshot(directory.path(), 100, 100, true);
    assert!(fallback.text().contains("charge=1 cores + 1000 B (legacy)"));
}

#[test]
fn scheduler_stats_report_links_and_defer_test_charges_to_admission() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 10, 100);
    pool.record_peak("crate [link]", 200, true).unwrap();
    pool.record_peak("crate [test]", 200, false).unwrap();
    pool.record_peak("crate [test, 3 threads]", 200, false)
        .unwrap();
    pool.record_cpu("crate [test]", 4).unwrap();
    for unit in [0, 100] {
        let report = scheduler_snapshot(directory.path(), 10, unit, true);
        for entry in &report.entries {
            if unit > 0 && entry.key.ends_with(" [link]") {
                assert_eq!(entry.charge_bytes, Some(200));
                assert_eq!(entry.charge_permits, Some(2));
            } else {
                assert_eq!(entry.charge_bytes, None, "{}", entry.key);
                assert_eq!(entry.charge_permits, None, "{}", entry.key);
            }
            assert_eq!(entry.recorded_peak_bytes, 200);
        }
        assert!(report.text().contains(
            "peak=200 B, samples=0, n=0, window max=unknown, bound=unknown, charge=see admission"
        ));
        assert!(!report.text().contains("see admission (see admission)"));
    }
}

#[test]
fn compile_emit_sets_combine_normalize_and_distinguish_codegen_outputs() {
    let mode = |args: &[&str]| compile_demand("crate", args).compile_mode.unwrap();
    assert_eq!(mode(&[]), mode(&["--emit=link"]));
    assert_eq!(
        mode(&["--emit=metadata"]),
        mode(&["--emit", "dep-info=dep,metadata=one", "--emit=metadata=two"])
    );
    assert_eq!(
        mode(&["--emit=link", "--emit", "metadata"]),
        mode(&["--emit=metadata,link,link"])
    );
    assert!(mode(&["--emit=link", "--emit=metadata"]).contains("emit=link,metadata "));
    assert_ne!(
        mode(&["--emit=link", "--emit=metadata"]),
        mode(&["--emit=metadata"])
    );
    let mut keys = std::collections::BTreeSet::new();
    for emit in [
        "metadata", "link", "mir", "obj", "asm", "llvm-ir", "llvm-bc", "dep-info",
    ] {
        assert!(keys.insert(mode(&[&format!("--emit={emit}")])), "{emit}");
    }
    assert!(mode(&["--emit=dep-info"]).contains("emit= crate-type="));
}

#[test]
fn compile_modes_include_all_crate_types_and_every_target() {
    let mode = |args: &[&str]| compile_demand("crate", args).compile_mode.unwrap();
    let types = mode(&["--crate-type=rlib,bin", "--crate-type", "rlib,cdylib"]);
    assert!(types.contains("crate-type=bin,cdylib,rlib "));
    assert_eq!(types, mode(&["--crate-type", "cdylib,bin,rlib"]));
    assert_ne!(mode(&["--crate-type=rlib"]), mode(&["--crate-type=bin"]));
    let repeated = mode(&["--target", "first", "--target=wasm32-unknown-unknown"]);
    assert_ne!(repeated, mode(&["--target=wasm32-unknown-unknown"]));
    assert_ne!(repeated, mode(&["--target=first"]));
    assert!(repeated.contains("targets=[\"first\", \"wasm32-unknown-unknown\"]"));
    assert!(
        mode(&["--target=wasm32-unknown-unknown"]).contains("target=\"wasm32-unknown-unknown\"")
    );
    assert_ne!(mode(&[]), mode(&["--target=wasm32-unknown-unknown"]));
    assert_ne!(
        mode(&["--target=host test"]),
        mode(&["--target=host", "--test"])
    );
}

#[test]
fn unsampled_compile_modes_use_the_sibling_bound_above_the_charge_floor() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    pool.record_peak("crate", 1000, false).unwrap();
    let metadata = compile_demand("crate", &["--crate-type=rlib", "--emit=metadata"]);
    let mir = compile_demand("crate", &["--crate-type=rlib", "--emit=mir"]);
    for _ in 0..3 {
        pool.record_compile(&metadata, 100, false, true).unwrap();
        pool.record_compile(&mir, 100, false, true).unwrap();
    }
    assert_eq!(pool.plan(&metadata), (2, Some(150)));
    for args in [
        vec!["--crate-type=rlib", "--emit=link", "--emit=metadata"],
        vec!["--crate-type=rlib", "--emit=link"],
        vec![
            "--crate-type=bin",
            "--emit=metadata",
            "--target=wasm32-unknown-unknown",
        ],
        vec![
            "--crate-type=rlib",
            "--emit=metadata",
            "--target=wasm32-unknown-unknown",
        ],
        vec!["--crate-type=bin", "--emit=metadata"],
    ] {
        assert_eq!(
            pool.plan(&compile_demand("crate", &args)),
            (4, Some(302)),
            "{args:?}"
        );
    }
}

#[test]
fn wrapper_observations_raise_peaks_but_never_train_a_light_mode() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    pool.record_peak("crate", 1000, false).unwrap();
    let args = vec![std::ffi::OsString::from("--emit=metadata")];
    let wrapped = Demand::new("crate", false).with_compiler_execution(&args, None, true, false);
    let direct = Demand::new("crate", false).with_compiler_execution(&args, None, false, false);
    let key = direct.compile_mode.as_ref().unwrap();
    assert_eq!(wrapped.compile_mode, direct.compile_mode);
    for _ in 0..3 {
        pool.record_compile(&wrapped, 81, false, true).unwrap();
    }
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.crates[key], 81);
    assert!(!ledger.compile_samples.contains_key(key));
    assert_eq!(pool.plan(&direct), (10, Some(1000)));
    pool.record_compile(&direct, 81, false, true).unwrap();
    pool.record_compile(&wrapped, 1200, false, true).unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.compile_samples[key], 1);
    assert_eq!(ledger.crates[key], 1200);
    assert_eq!(ledger.crates["crate"], 1200);
    pool.record_compile(&wrapped, 800, true, false).unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.compile_samples[key], 1);
    assert_eq!(ledger.crates[key], 1600);
    assert_eq!(ledger.crates["crate"], 1600);
}

#[test]
fn compile_keys_do_not_reuse_legacy_classification_samples() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let mut ledger = MemoryLedger::default();
    let old = "crate [compile metadata opt0 lto-default cgu- debug0 incremental-off]";
    ledger.crates.insert("crate".into(), 1000);
    ledger.crates.insert(old.into(), 100);
    ledger.compile_samples.insert(old.into(), 3);
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
    assert_eq!(
        pool.plan(&compile_demand("crate", &["--emit=metadata"])),
        (10, Some(1000))
    );
}

#[test]
fn scheduler_stats_compile_charges_match_admission_predictions() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 100, 100);
    let demand = compile_demand("crate", &["--emit=metadata"]);
    pool.record_peak("crate", 1000, false).unwrap();
    for _ in 0..4 {
        let snapshot = scheduler_snapshot(directory.path(), 100, 100, true);
        let planned = pool.plan(&demand);
        for entry in &snapshot.entries {
            if entry.key == demand.compile_mode.as_ref().unwrap().as_str() {
                assert_eq!(
                    (entry.charge_permits, entry.charge_bytes),
                    (Some(planned.0), planned.1)
                );
            } else if entry.key == "crate" {
                assert_eq!(entry.charge_bytes, Some(1000));
                assert_eq!(entry.charge_permits, Some(10));
            }
        }
        pool.record_compile(&demand, 81, false, true).unwrap();
    }
}

#[test]
fn idle_capacity_slack_uses_every_possible_completion_subset() {
    assert!(can_pass_waiters(120, &[54, 54], &[54], 12));
    assert!(!can_pass_waiters(120, &[54, 54], &[54], 13));
    assert!(can_pass_waiters(120, &[54, 54, 1], &[54], 11));
    assert!(!can_pass_waiters(120, &[54, 54, 1], &[54], 12));
    let mut live = vec![54];
    live.extend([1; 54]);
    assert!(!can_pass_waiters(120, &live, &[54], 1));
}

#[test]
fn idle_capacity_protects_each_prefix_instead_of_individual_or_summed_heads() {
    // Individual checks would admit two permits, delaying the second waiter
    // when the eight-permit lease ends: both older requests need all ten.
    assert!(!can_pass_waiters(10, &[8], &[6, 4], 2));
    // Summing the heads would admit one permit, delaying the first waiter
    // when the six-permit lease ends while the two-permit lease stays live.
    assert!(!can_pass_waiters(10, &[6, 2], &[8, 1], 1));
}

#[test]
fn idle_capacity_bitset_handles_word_boundaries_and_refuses_excessive_work() {
    assert!(can_pass_waiters(160, &[64, 64], &[64], 32));
    assert!(!can_pass_waiters(160, &[64, 64], &[64], 33));
    assert!(can_pass_waiters(65_536, &[65_534], &[1], 1));
    assert!(!can_pass_waiters(65_537, &[65_535], &[1], 1));
    assert!(!can_pass_waiters(u64::MAX, &[u64::MAX], &[1], 1));
    assert!(!can_pass_waiters(4, &[5], &[4], 1));
    assert!(!can_pass_waiters(4, &[3], &[5], 1));
    assert!(
        can_pass_waiters(4, &[], &[], 5),
        "no waiters leaves admission to fits"
    );
}

#[test]
fn queued_builds_share_only_capacity_they_cannot_use_in_each_scope() {
    for scope in ["global", "domain", "build"] {
        let directory = tempfile::tempdir().unwrap();
        let mut pool = pool_at(
            directory.path(),
            if scope == "global" { 120 } else { 240 },
            0,
        );
        pool.domain_capacity = if scope == "domain" { 120 } else { 240 };
        if scope == "build" {
            pool.build = Some(("bounded-build".into(), 120));
        }
        let first = pool.try_admit(54, None).unwrap().unwrap();
        let second = pool.try_admit(54, None).unwrap().unwrap();
        let mut head = None;
        assert!(
            pool.try_admit_queued(54, None, &mut head)
                .unwrap()
                .is_none(),
            "{scope}"
        );
        let mut newer = None;
        let small = pool
            .try_admit_queued(1, None, &mut newer)
            .unwrap()
            .expect(scope);
        assert!(newer.is_none());
        let additional: Vec<_> = (0..11)
            .map(|_| {
                pool.try_admit_queued(1, None, &mut None)
                    .unwrap()
                    .expect(scope)
            })
            .collect();
        assert!(pool.try_admit_queued(1, None, &mut None).unwrap().is_none());
        drop(first);
        // The passer is still live when the head starts, at exactly the same
        // completion that would have admitted it without any passing.
        let admitted = pool
            .try_admit_queued(54, None, &mut head)
            .unwrap()
            .expect(scope);
        assert!(head.is_none());
        drop((admitted, small, additional, second));
    }
}

#[test]
fn queued_builds_refuse_passing_when_small_live_leases_can_fill_the_head() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 120, 0);
    let large = pool.try_admit(54, None).unwrap().unwrap();
    let small: Vec<_> = (0..54)
        .map(|_| pool.try_admit(1, None).unwrap().unwrap())
        .collect();
    let mut head = None;
    assert!(
        pool.try_admit_queued(54, None, &mut head)
            .unwrap()
            .is_none()
    );
    let mut newer = None;
    assert!(
        pool.try_admit_queued(1, None, &mut newer)
            .unwrap()
            .is_none()
    );
    drop((large, small));
}

#[derive(Clone)]
struct SimRequest {
    arrival: u64,
    duration: u64,
    record: Waiting,
}

fn sim_request(
    ticket: u64,
    arrival: u64,
    weight: u64,
    duration: u64,
    domain: (&str, u64),
    build: Option<(&str, u64)>,
) -> SimRequest {
    SimRequest {
        arrival,
        duration,
        record: Waiting {
            first_arrival_ms: None,
            pid: None,
            name: None,
            mode_key: None,
            ticket,
            weight,
            cpu: None,
            memory_bytes: None,
            cpu_cap: None,
            mem_cap: None,
            domain_memory_cap: None,
            domain_cpu_cap: None,
            priority: "normal".into(),
            domain: domain.0.into(),
            domain_capacity: domain.1,
            global_capacity: None,
            bytes_per_permit: 0,
            build: build.map(|(id, capacity)| (id.into(), capacity)),
        },
    }
}

fn sim_lease(record: &Waiting) -> Lease {
    Lease {
        version: LEASE_VERSION,
        name: None,
        mode_key: None,
        weight: record.weight,
        cpu: record.cpu,
        memory_bytes: record.memory_bytes,
        priority: record.priority.clone(),
        build_id: record.build.as_ref().map(|(id, _)| id.clone()),
        domain: Some(record.domain.clone()),
        bytes_per_permit: record.bytes_per_permit,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SimPolicy {
    Today,
    TodayOrPass,
    StrictFifo,
}

#[derive(Clone)]
struct AdmissionSimulation<'a> {
    capacity: u64,
    legacy_readers: bool,
    now: u64,
    live: Vec<Lease>,
    finishes: Vec<u64>,
    queue: Vec<usize>,
    requests: &'a [SimRequest],
    started: Vec<Option<u64>>,
}

impl<'a> AdmissionSimulation<'a> {
    fn new(capacity: u64, initial: &[(u64, Waiting)], requests: &'a [SimRequest]) -> Self {
        Self {
            capacity,
            legacy_readers: false,
            now: 0,
            live: initial
                .iter()
                .map(|(_, record)| sim_lease(record))
                .collect(),
            finishes: initial.iter().map(|(finish, _)| *finish).collect(),
            queue: Vec::new(),
            requests,
            started: vec![None; requests.len()],
        }
    }

    fn expire(&mut self) {
        for index in (0..self.live.len()).rev() {
            if self.finishes[index] <= self.now {
                self.live.swap_remove(index);
                self.finishes.swap_remove(index);
            }
        }
    }

    fn decision(&self, position: usize) -> AdmissionDecision {
        let request = &self.requests[self.queue[position]].record;
        self.reader_decision(
            request.global_capacity.unwrap_or(self.capacity),
            &self.live,
            self.queue.iter().map(|id| &self.requests[*id].record),
            request,
        )
    }

    fn reader_decision<'b>(
        &self,
        capacity: u64,
        live: &[Lease],
        waiting: impl Iterator<Item = &'b Waiting>,
        request: &Waiting,
    ) -> AdmissionDecision {
        if !self.legacy_readers || request.cpu.is_some() || request.memory_bytes.is_some() {
            let waiting: Vec<_> = waiting.collect();
            let without_logging =
                admission_decision(capacity, live, waiting.iter().copied(), request);
            let trace = std::cell::RefCell::new(decisions::Trace::default());
            let with_logging = admission_decision_traced(
                capacity,
                live,
                waiting.iter().copied(),
                request,
                Some(&trace),
            );
            assert_eq!(
                without_logging, with_logging,
                "logging must not change admission"
            );
            return without_logging;
        }
        // Use the independent fe73826 reader for the legacy process itself,
        // including in both counterfactual forks and strict FIFO admission.
        let live: Vec<legacy::Lease> = live
            .iter()
            .map(|record| serde_json::from_value(serde_json::to_value(record).unwrap()).unwrap())
            .collect();
        let waiting: Vec<legacy::Waiting> = waiting
            .map(|record| serde_json::from_value(serde_json::to_value(record).unwrap()).unwrap())
            .collect();
        let request = serde_json::from_value(serde_json::to_value(request).unwrap()).unwrap();
        match legacy::admission_decision(capacity, &live, waiting.iter(), &request) {
            legacy::AdmissionDecision::Ordered => AdmissionDecision::Ordered,
            legacy::AdmissionDecision::Passing => AdmissionDecision::Passing,
            legacy::AdmissionDecision::Waiting => AdmissionDecision::Waiting,
        }
    }

    fn grant(&mut self, position: usize) {
        let id = self.queue.remove(position);
        let request = &self.requests[id];
        self.live.push(sim_lease(&request.record));
        self.finishes.push(self.now + request.duration);
        self.started[id] = Some(self.now);
    }

    fn check_fork(&self, position: usize, strict_fifo: bool) {
        let mut a = self.clone();
        let mut b = self.clone();
        let passer = self.queue[position];
        a.grant(position);
        let policy = if strict_fifo {
            SimPolicy::StrictFifo
        } else {
            SimPolicy::Today
        };
        a.drain(policy);
        b.drain(policy);
        let protected = if strict_fifo {
            &self.queue[..position]
        } else {
            &self.queue[..]
        };
        for id in protected {
            if *id != passer {
                assert!(
                    a.started[*id].unwrap() <= b.started[*id].unwrap(),
                    "passer ticket {} at {} delayed ticket {}: A={:?}, B={:?}",
                    self.requests[passer].record.ticket,
                    self.now,
                    self.requests[*id].record.ticket,
                    a.started[*id],
                    b.started[*id],
                );
            }
        }
    }

    fn step(&mut self, policy: SimPolicy, check_forks: Option<bool>) -> usize {
        let mut position = 0;
        let mut passes = 0;
        while position < self.queue.len() {
            let decision = if policy == SimPolicy::StrictFifo {
                if position != 0 {
                    break;
                }
                // The real decision with an empty older queue still enforces
                // every physical ceiling, without any overtaking exceptions.
                let request = &self.requests[self.queue[position]].record;
                self.reader_decision(
                    request.global_capacity.unwrap_or(self.capacity),
                    &self.live,
                    std::iter::empty(),
                    request,
                )
            } else {
                self.decision(position)
            };
            if decision == AdmissionDecision::Waiting
                || (policy == SimPolicy::Today && decision == AdmissionDecision::Passing)
            {
                position += 1;
                continue;
            }
            if decision == AdmissionDecision::Passing {
                passes += 1;
                if let Some(strict_fifo) = check_forks {
                    self.check_fork(position, strict_fifo);
                }
            }
            self.grant(position);
        }
        passes
    }

    fn drain(&mut self, policy: SimPolicy) {
        while !self.queue.is_empty() {
            self.expire();
            self.step(policy, None);
            if !self.queue.is_empty() {
                self.now = *self
                    .finishes
                    .iter()
                    .min()
                    .expect("a live lease can unblock the queue");
            }
        }
    }
}

fn simulate_admission(
    capacity: u64,
    initial: &[(u64, Waiting)],
    requests: &[SimRequest],
    policy: SimPolicy,
    check_forks: Option<bool>,
) -> (Vec<u64>, usize) {
    let mut simulation = AdmissionSimulation::new(capacity, initial, requests);
    let mut next = 0;
    let mut passes = 0;
    loop {
        simulation.expire();
        while next < requests.len() && requests[next].arrival <= simulation.now {
            simulation.queue.push(next);
            next += 1;
        }
        passes += simulation.step(policy, check_forks);
        let event = simulation
            .finishes
            .iter()
            .copied()
            .chain(requests.get(next).map(|request| request.arrival))
            .min();
        let Some(event) = event else { break };
        simulation.now = event;
    }
    assert!(simulation.queue.is_empty());
    (
        simulation.started.into_iter().map(Option::unwrap).collect(),
        passes,
    )
}

#[test]
fn ordered_admission_prefix_regressions_keep_original_admission_times() {
    for (initial, requests) in [
        (vec![(2, 8), (1, 2)], vec![(0, 6, 5), (0, 4, 5), (1, 2, 10)]),
        (
            vec![(2, 6), (10, 2)],
            vec![(0, 8, 5), (0, 1, 5), (1, 1, 10)],
        ),
    ] {
        let initial: Vec<_> = initial
            .into_iter()
            .map(|(end, weight)| (end, sim_request(0, 0, weight, 1, ("host", 10), None).record))
            .collect();
        let requests: Vec<_> = requests
            .into_iter()
            .enumerate()
            .map(|(id, (arrival, weight, duration))| {
                sim_request(id as u64 + 1, arrival, weight, duration, ("host", 10), None)
            })
            .collect();
        let (old, _) = simulate_admission(10, &initial, &requests, SimPolicy::Today, None);
        let (new, _) =
            simulate_admission(10, &initial, &requests, SimPolicy::TodayOrPass, Some(false));
        assert_eq!(new, old);
    }
}

#[test]
fn a_passer_must_protect_the_global_pool_even_when_the_head_is_domain_blocked() {
    let directory = tempfile::tempdir().unwrap();
    let mut a = pool_at(directory.path(), 4, 0);
    a.pressure_domain = "A";
    let mut b = pool_at(directory.path(), 4, 0);
    b.pressure_domain = "B";
    let first = a.try_admit(2, None).unwrap().unwrap();
    let second = b.try_admit(1, None).unwrap().unwrap();
    let mut head = None;
    assert!(a.try_admit_queued(3, None, &mut head).unwrap().is_none());
    assert!(a.try_admit_queued(1, None, &mut None).unwrap().is_none());
    drop(first);
    assert!(a.try_admit_queued(3, None, &mut head).unwrap().is_some());
    drop(second);
}

#[test]
fn today_or_pass_keeps_the_unrelated_domain_admission_at_time_one() {
    let initial = vec![(3, sim_request(0, 0, 2, 1, ("A", 2), Some(("a", 4))).record)];
    let requests = vec![
        sim_request(1, 0, 1, 1, ("A", 2), Some(("a", 4))),
        sim_request(2, 1, 2, 4, ("B", 2), Some(("b", 4))),
        sim_request(3, 5, 1, 1, ("B", 2), Some(("b", 4))),
        sim_request(4, 6, 1, 1, ("A", 2), Some(("a", 4))),
    ];
    let (old, _) = simulate_admission(4, &initial, &requests, SimPolicy::Today, None);
    let (new, _) = simulate_admission(4, &initial, &requests, SimPolicy::TodayOrPass, None);
    assert_eq!(new, vec![3, 1, 5, 6]);
    assert_eq!(new, old);
}

#[test]
fn todays_build_exception_can_refill_capacity_after_a_passer_leaves_the_queue() {
    let initial = vec![(2, sim_request(0, 0, 2, 1, ("D", 4), Some(("h", 4))).record)];
    let requests = vec![
        sim_request(1, 0, 3, 1, ("D", 4), Some(("h", 4))),
        sim_request(2, 0, 1, 1, ("D", 4), Some(("h", 4))),
        sim_request(3, 0, 2, 2, ("D", 4), Some(("r", 4))),
    ];
    let mut before = AdmissionSimulation::new(4, &initial, &requests);
    before.queue = vec![0, 1, 2];
    assert_eq!(before.decision(1), AdmissionDecision::Passing);
    let mut a = before.clone();
    a.grant(1);
    a.drain(SimPolicy::Today);
    let mut b = before.clone();
    b.drain(SimPolicy::Today);
    assert_eq!(a.started, vec![Some(3), Some(0), Some(1)]);
    assert_eq!(b.started, vec![Some(2), Some(2), Some(3)]);
    // Today's build exception allows R from another build to refill the
    // capacity after C finishes. It delays H one tick; the pass check itself
    // would refuse that refill. Preserve this known scope-exception behavior.
    assert_eq!(a.started[0].unwrap() - b.started[0].unwrap(), 1);
    before.check_fork(1, true);
}

fn sim_random(state: &mut u64, low: u64, high: u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    low + (*state >> 16) % (high - low + 1)
}

fn check_random_admission_forks(multi_scope: bool, two_dimensions: bool) {
    let mut total_passes = 0;
    let mut mixed_budget_workloads = 0;
    for seed in 0..512_u64 {
        let mut state = seed + 1;
        let capacity = sim_random(&mut state, 4, 128);
        let domain_count = if multi_scope {
            sim_random(&mut state, 2, 4)
        } else {
            1
        } as usize;
        let domain_capacities: Vec<_> = (0..domain_count)
            .map(|_| {
                if multi_scope {
                    sim_random(&mut state, 1, capacity)
                } else {
                    capacity
                }
            })
            .collect();
        let mut build_domains = Vec::new();
        for domain in 0..domain_count {
            let count = if multi_scope {
                sim_random(&mut state, 1, 3)
            } else {
                1
            };
            build_domains.extend(std::iter::repeat_n(domain, count as usize));
        }
        let build_capacities: Vec<_> = build_domains
            .iter()
            .map(|_| {
                if multi_scope {
                    sim_random(&mut state, 1, capacity)
                } else {
                    capacity
                }
            })
            .collect();
        let make_request = |id, arrival, weight, duration, build: usize| {
            let domain = build_domains[build];
            sim_request(
                id,
                arrival,
                weight,
                duration,
                (&format!("domain-{domain}"), domain_capacities[domain]),
                Some((&format!("build-{build}"), build_capacities[build])),
            )
        };
        let mut initial = Vec::new();
        let mut used = 0;
        let mut domain_used = vec![0; domain_count];
        let mut build_used = vec![0; build_domains.len()];
        for _ in 0..capacity {
            if sim_random(&mut state, 0, 99) < 15 {
                break;
            }
            let build = sim_random(&mut state, 0, build_domains.len() as u64 - 1) as usize;
            let domain = build_domains[build];
            let free = (capacity - used)
                .min(domain_capacities[domain] - domain_used[domain])
                .min(build_capacities[build] - build_used[build]);
            if free != 0 {
                let weight = sim_random(&mut state, 1, free);
                let end = sim_random(&mut state, 1, 50);
                initial.push((end, make_request(0, 0, weight, 1, build).record));
                used += weight;
                domain_used[domain] += weight;
                build_used[build] += weight;
            }
        }
        let mut requests = Vec::new();
        for arrival in std::iter::repeat_n(0, sim_random(&mut state, 1, 8) as usize).chain(1..=100)
        {
            if arrival != 0 && sim_random(&mut state, 0, 99) >= 65 {
                continue;
            }
            let build = sim_random(&mut state, 0, build_domains.len() as u64 - 1) as usize;
            let limit = domain_capacities[build_domains[build]]
                .min(build_capacities[build])
                .min(if arrival == 0 { capacity } else { 3 });
            let weight = sim_random(&mut state, 1, limit);
            let duration = sim_random(&mut state, 1, 50);
            let mut request =
                make_request(requests.len() as u64 + 1, arrival, weight, duration, build);
            request.record.global_capacity = Some(if multi_scope {
                sim_random(&mut state, weight, capacity)
            } else {
                capacity
            });
            requests.push(request);
        }
        if two_dimensions {
            let global_memory = capacity * 8 * MEMORY_QUANTUM + 123;
            let mut domain_memory_used = vec![0; domain_count];
            let mut used_memory = 0;
            for (_, record) in &mut initial {
                let domain = record
                    .domain
                    .strip_prefix("domain-")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                let local_memory = domain_capacities[domain] * 8 * MEMORY_QUANTUM + 123;
                let free =
                    (global_memory - used_memory).min(local_memory - domain_memory_used[domain]);
                let memory = sim_random(&mut state, 1, free.max(1)).min(free);
                record.cpu = Some(record.weight);
                record.memory_bytes = Some(memory);
                record.mem_cap = Some(global_memory);
                record.domain_memory_cap = Some(local_memory);
                used_memory += memory;
                domain_memory_used[domain] += memory;
            }
            for request in &mut requests {
                let record = &mut request.record;
                let domain = record
                    .domain
                    .strip_prefix("domain-")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                let local_memory = domain_capacities[domain] * 8 * MEMORY_QUANTUM + 123;
                record.cpu = Some(record.weight);
                record.memory_bytes =
                    Some(sim_random(&mut state, 1, local_memory.min(global_memory)));
                record.mem_cap = Some(if multi_scope {
                    sim_random(&mut state, 1, global_memory)
                } else {
                    global_memory
                });
                record.domain_memory_cap = Some(local_memory);
            }
        }
        let budgets: std::collections::BTreeSet<_> = requests
            .iter()
            .map(|request| request.record.global_capacity)
            .collect();
        mixed_budget_workloads += usize::from(budgets.len() > 1);
        let (_, passes) = simulate_admission(
            capacity,
            &initial,
            &requests,
            SimPolicy::TodayOrPass,
            Some(multi_scope),
        );
        total_passes += passes;
    }
    assert!(total_passes > 0, "random workloads must exercise passing");
    if multi_scope {
        assert!(
            mixed_budget_workloads > 0,
            "random workloads must mix global budgets"
        );
    }
}

#[test]
fn single_scope_passes_preserve_all_waiters_in_counterfactuals() {
    check_random_admission_forks(false, false);
}

#[test]
fn shared_scope_passes_preserve_older_waiters_with_fifo_continuations() {
    check_random_admission_forks(true, false);
}

#[test]
fn real_queue_protects_both_prefix_counterexamples() {
    for (weights, heads, drop_before_pass, passer) in
        [([8, 2], [6, 4], true, 2), ([6, 2], [8, 1], false, 1)]
    {
        let directory = tempfile::tempdir().unwrap();
        let mut pool = pool_at(directory.path(), 10, 0);
        pool.domain_capacity = 20;
        let first = pool.try_admit(weights[0], None).unwrap().unwrap();
        let mut second = pool.try_admit(weights[1], None).unwrap();
        let mut head = None;
        let mut next = None;
        assert!(
            pool.try_admit_queued(heads[0], None, &mut head)
                .unwrap()
                .is_none()
        );
        assert!(
            pool.try_admit_queued(heads[1], None, &mut next)
                .unwrap()
                .is_none()
        );
        if drop_before_pass {
            drop(second.take());
        }
        assert!(
            pool.try_admit_queued(passer, None, &mut None)
                .unwrap()
                .is_none()
        );
        drop(first);
        let admitted = pool
            .try_admit_queued(heads[0], None, &mut head)
            .unwrap()
            .unwrap();
        if drop_before_pass {
            assert!(
                pool.try_admit_queued(heads[1], None, &mut next)
                    .unwrap()
                    .is_some()
            );
        }
        drop((admitted, second));
    }
}

#[test]
fn passing_protects_the_waiting_processes_smaller_global_budget() {
    let directory = tempfile::tempdir().unwrap();
    let mut older = pool_at(directory.path(), 10, 0);
    let newer = pool_at(directory.path(), 12, 0);
    // Keep the domain ceiling above both global budgets to isolate the
    // process's global limit, independent of its container ceiling.
    older.domain_capacity = 12;
    let first = newer.try_admit(4, None).unwrap().unwrap();
    let second = newer.try_admit(4, None).unwrap().unwrap();
    let mut head = None;
    assert!(
        older
            .try_admit_queued(6, None, &mut head)
            .unwrap()
            .is_none()
    );
    let waiting = scan_waiters(&directory.path().join(WAITERS_DIR)).unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].global_capacity, Some(10));
    let mut small = None;
    assert!(
        newer
            .try_admit_queued(2, None, &mut small)
            .unwrap()
            .is_none()
    );
    // Against capacity 12 the passer would have two permits of slack, but
    // the head's capacity 10 has zero. It starts on this first completion.
    drop(first);
    let admitted = older.try_admit_queued(6, None, &mut head).unwrap().unwrap();
    assert!(head.is_none());
    drop((admitted, second));
}

#[test]
fn old_waiter_records_fall_back_to_the_request_global_capacity() {
    let legacy: Waiting = serde_json::from_str(
        r#"{"ticket":1,"weight":6,"priority":"normal","domain":"host","domain_capacity":12,"bytes_per_permit":0,"build":null}"#,
    ).unwrap();
    assert_eq!(legacy.global_capacity, None);
    let live = vec![sim_lease(&sim_request(0, 0, 4, 1, ("host", 12), None).record); 2];
    let request = sim_request(2, 0, 2, 1, ("host", 12), None).record;
    assert_eq!(
        admission_decision(12, &live, [&legacy].into_iter(), &request),
        AdmissionDecision::Passing
    );
    let protected = Waiting {
        global_capacity: Some(10),
        ..legacy
    };
    assert_eq!(
        admission_decision(12, &live, [&protected].into_iter(), &request),
        AdmissionDecision::Waiting
    );
}

#[test]
fn two_budget_single_scope_passes_preserve_all_waiters() {
    check_random_admission_forks(false, true);
}

#[test]
fn two_budget_shared_scope_passes_preserve_older_waiters() {
    check_random_admission_forks(true, true);
}

fn vector_attempt(pool: &Pool, cpu: u64, memory_bytes: u64) -> Option<Permit> {
    let weight = if pool.bytes_per_permit == 0 {
        cpu
    } else {
        cpu.max(memory_bytes.div_ceil(pool.bytes_per_permit))
    }
    .max(1);
    pool.try_admit_named(
        Resources { cpu, memory_bytes },
        weight,
        None,
        &mut None,
        None,
    )
    .unwrap()
}

#[test]
fn memory_heavy_compiles_leave_cpu_capacity_for_light_requests() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 32, GIB);
    pool.record_peak("large", 24 * GIB, false).unwrap();
    let large = pool.admit(&Demand::new("large", false)).unwrap();
    let small = pool.admit(&Demand::new("small", false)).unwrap();
    let leases = scan_leases(&dir.path().join(LEASES_DIR)).unwrap();
    assert_eq!(
        scope_used(leases.iter()),
        Resources {
            cpu: 2,
            memory_bytes: 25 * GIB
        }
    );
    assert!(
        vector_attempt(&pool, 1, 8 * GIB).is_none(),
        "free CPUs cannot replace full memory"
    );
    drop((large, small));
}

#[test]
fn oversize_alone_applies_independently_to_each_dimension() {
    let cap = Resources {
        cpu: 4,
        memory_bytes: 100,
    };
    assert!(cap.fits(
        Resources::default(),
        Resources {
            cpu: 8,
            memory_bytes: 200
        }
    ));
    assert!(!cap.fits(
        Resources {
            cpu: 1,
            memory_bytes: 0
        },
        Resources {
            cpu: 8,
            memory_bytes: 200
        }
    ));
    assert!(!cap.fits(
        Resources {
            cpu: 0,
            memory_bytes: 1
        },
        Resources {
            cpu: 8,
            memory_bytes: 200
        }
    ));
    assert!(cap.fits(
        Resources {
            cpu: 0,
            memory_bytes: 50
        },
        Resources {
            cpu: 8,
            memory_bytes: 50
        }
    ));
    assert!(cap.fits(
        Resources {
            cpu: 2,
            memory_bytes: 0
        },
        Resources {
            cpu: 2,
            memory_bytes: 200
        }
    ));
}

#[test]
fn domain_budgets_are_independent_and_build_slots_only_limit_cpu() {
    let dir = tempfile::tempdir().unwrap();
    let mut pool = pool_at(dir.path(), 16, GIB);
    pool.domain_capacity = 4;
    pool.domain_memory_capacity = 8 * GIB;
    pool.build = Some(("build".into(), 2));
    let first = vector_attempt(&pool, 1, 7 * GIB).unwrap();
    let second = vector_attempt(&pool, 1, GIB).expect("build slots do not count memory permits");
    assert!(vector_attempt(&pool, 1, 0).is_none());
    drop(second);
    assert!(vector_attempt(&pool, 1, 2 * GIB).is_none());
    let second = vector_attempt(&pool, 1, GIB).unwrap();
    pool.build = Some(("other".into(), 2));
    let cpu_only = vector_attempt(&pool, 2, 0).unwrap();
    assert!(vector_attempt(&pool, 1, 0).is_none());
    drop((first, second, cpu_only));
}

#[test]
fn reservation_vector_and_legacy_charge_are_both_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 8, GIB);
    let lease = pool.reserve(Some(2), Some(5 * GIB + 1)).unwrap();
    let record: Lease = serde_json::from_slice(&std::fs::read(&lease.path).unwrap()).unwrap();
    assert_eq!(
        record.resources(),
        Resources {
            cpu: 2,
            memory_bytes: 5 * GIB + 1
        }
    );
    assert_eq!(record.weight, 6);
    assert_eq!(record.bytes_per_permit, GIB);
    drop(lease);
    let lease = pool.reserve(Some(2), None).unwrap();
    let record: Lease = serde_json::from_slice(&std::fs::read(&lease.path).unwrap()).unwrap();
    assert_eq!(
        record.resources(),
        Resources {
            cpu: 2,
            memory_bytes: 0
        }
    );
    drop(lease);
    let lease = pool.reserve(None, Some(GIB)).unwrap();
    let record: Lease = serde_json::from_slice(&std::fs::read(&lease.path).unwrap()).unwrap();
    assert_eq!(
        record.resources(),
        Resources {
            cpu: 0,
            memory_bytes: GIB
        }
    );
}

#[test]
fn legacy_records_charge_full_weight_on_both_axes() {
    let lease: Lease = serde_json::from_str(
        r#"{"version":1,"weight":4,"priority":"normal","bytes_per_permit":1073741824}"#,
    )
    .unwrap();
    let waiter: Waiting = serde_json::from_str(r#"{"ticket":1,"weight":4,"priority":"normal","domain":"host","domain_capacity":8,"bytes_per_permit":1073741824,"build":null}"#).unwrap();
    let expected = Resources {
        cpu: 4,
        memory_bytes: 4 * GIB,
    };
    assert_eq!(lease.resources(), expected);
    assert_eq!(waiter.resources(), expected);
    let mut request = sim_request(2, 0, 1, 1, ("host", 8), None).record;
    request.cpu = Some(1);
    request.memory_bytes = Some(1);
    request.cpu_cap = Some(8);
    request.mem_cap = Some(4 * GIB);
    request.domain_memory_cap = Some(8 * GIB);
    assert_eq!(
        admission_decision(
            8,
            std::slice::from_ref(&lease),
            std::iter::empty(),
            &request
        ),
        AdmissionDecision::Waiting
    );
    request.mem_cap = Some(8 * GIB);
    request.cpu_cap = Some(4);
    assert_eq!(
        admission_decision(4, &[lease], std::iter::empty(), &request),
        AdmissionDecision::Waiting
    );
}

#[test]
fn demand_vectors_keep_legacy_weights_and_unmeasured_memory_guesses() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 8, GIB);
    for demand in [
        Demand::new("cold", false),
        Demand::new("cold-link", true),
        Demand::test("cold-suite", None),
        Demand::nextest("cold-bin", "case"),
    ] {
        let plan = pool.demand_vector(&demand);
        assert!(plan.resources.memory_bytes >= plan.weight * GIB);
        assert_eq!(
            (plan.weight, plan.predicted, plan.source),
            pool.prediction(&demand)
        );
    }
    pool.record_peak("crate", GIB, false).unwrap();
    let mut demand = Demand::new("crate", false);
    demand.threads = Some(4);
    let plan = pool.demand_vector(&demand);
    assert_eq!(
        plan.resources,
        Resources {
            cpu: 4,
            memory_bytes: GIB
        }
    );
    assert_eq!(plan.weight, 4);
    pool.record_cpu(&Demand::test("suite", None).name, 3)
        .unwrap();
    let suite = Demand::test("suite", None);
    pool.record_peak(&suite.name, 2 * GIB, false).unwrap();
    assert_eq!(
        pool.demand_vector(&suite).resources,
        Resources {
            cpu: 3,
            memory_bytes: 2 * GIB
        }
    );
    pool.record_test_history("bin", "case", Some(123), Some(2))
        .unwrap();
    assert_eq!(
        pool.demand_vector(&Demand::nextest("bin", "case"))
            .resources,
        Resources {
            cpu: 2,
            memory_bytes: 123
        }
    );
    let cpu_only = pool_at(dir.path(), 8, 0);
    assert_eq!(
        cpu_only.demand_vector(&demand).resources,
        Resources {
            cpu: 4,
            memory_bytes: 0
        }
    );
}

#[test]
fn low_priority_preserves_headroom_on_both_axes() {
    let dir = tempfile::tempdir().unwrap();
    let mut pool = pool_at(dir.path(), 8, GIB);
    pool.priority = SchedulerPriority::Low;
    std::fs::write(dir.path().join(PRIORITY_WAIT_STAMP), b"").unwrap();
    assert_eq!(
        pool.effective_budget(),
        Resources {
            cpu: 6,
            memory_bytes: 6 * GIB
        }
    );
    pool.capacity = 1;
    assert_eq!(
        pool.effective_budget(),
        Resources {
            cpu: 1,
            memory_bytes: 6 * GIB
        }
    );
    pool.priority = SchedulerPriority::Normal;
    assert_eq!(
        pool.effective_budget(),
        Resources {
            cpu: 1,
            memory_bytes: 8 * GIB
        }
    );
}

#[test]
fn quantized_memory_slack_never_exceeds_exact_byte_slack() {
    let u = MEMORY_QUANTUM;
    assert!(can_pass_quantized(
        120 * u,
        &[54 * u, 54 * u],
        &[54 * u],
        10 * u,
        u
    ));
    assert!(!can_pass_quantized(u64::MAX, &[65535 * u], &[u], 1, u));
    let mut state = 73;
    let mut accepted = 0;
    for _ in 0..2000 {
        let cap = sim_random(&mut state, 2, 120) * u + sim_random(&mut state, 1, u - 1);
        let live = [
            sim_random(&mut state, 1, cap / 2),
            sim_random(&mut state, 1, cap / 2),
        ];
        let older = [
            sim_random(&mut state, 1, cap),
            sim_random(&mut state, 1, cap),
        ];
        let charge = sim_random(&mut state, 1, cap / 8);
        if can_pass_quantized(cap, &live, &older, charge, u) {
            accepted += 1;
            let mut prefix = 0;
            let mut items = live.to_vec();
            for head in older {
                prefix += head;
                let free = cap - live.iter().sum::<u64>();
                let slack = (0..(1 << items.len()))
                    .filter_map(|mask| {
                        let removed: u64 = items
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| mask & (1 << i) != 0)
                            .map(|(_, w)| *w)
                            .sum();
                        (free + removed).checked_sub(prefix)
                    })
                    .min()
                    .unwrap();
                assert!(charge <= slack);
                items.push(head);
            }
        }
    }
    assert!(accepted > 0);
}

#[test]
fn demand_vectors_keep_mode_link_window_and_nextest_predictions_in_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 32, 100);
    let demand = compile_demand("crate", &["--emit=metadata"]);
    pool.record_peak("crate", 1000, false).unwrap();
    for _ in 0..3 {
        pool.record_compile(&demand, 200, false, true).unwrap();
    }
    let plan = pool.demand_vector(&demand);
    assert_eq!(plan.source, "window");
    assert_eq!(
        plan.resources,
        Resources {
            cpu: 1,
            memory_bytes: 300
        }
    );
    assert_eq!(plan.weight, 3);
    for _ in 0..MIN_LINK_SAMPLES {
        pool.record_link_peak("known [link]", 301, true, None)
            .unwrap();
    }
    let plan = pool.demand_vector(&Demand::new("unknown", true));
    assert_eq!(plan.source, "link-window");
    assert_eq!(
        plan.resources,
        Resources {
            cpu: 1,
            memory_bytes: 301
        }
    );
    assert_eq!(plan.weight, 4);
    pool.record_test_history("bin", "case", Some(123), Some(64))
        .unwrap();
    let plan = pool.demand_vector(&Demand::nextest("bin", "case"));
    assert_eq!(
        plan.resources,
        Resources {
            cpu: 64,
            memory_bytes: 123
        }
    );
    assert_eq!(plan.weight, 32, "legacy charge retains its capacity clamp");
}

#[test]
fn exact_memory_budget_and_legacy_transport_defaults_are_conservative() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_test(dir.path());
    config.scheduler.enabled = true;
    config.scheduler.cpus = 8;
    config.scheduler.reserve_cpus = 2;
    config.scheduler.memory_bytes = Some(12001);
    let pool = Pool::from_config(&config).unwrap();
    assert_eq!(pool.capacity, 6);
    assert_eq!(pool.bytes_per_permit, 2000);
    assert_eq!(pool.memory_capacity, 12001);
    assert_eq!(scheduler_report(&config).memory_capacity, 12001);
    let env = session_environment_with_jobs(&config, None);
    assert!(env.contains(&(SCHED_MEMORY_BYTES_ENV.into(), "12001".into())));
    assert!(env.contains(&(SCHED_SLOT_BYTES_ENV.into(), "2000".into())));
    assert!(env.contains(&(SCHED_SLOTS_ENV.into(), "6".into())));
    assert_eq!(
        Pool::new(dir.path().into(), 6, 2000, SchedulerPriority::Normal).memory_capacity,
        12000
    );
}

#[test]
fn old_and_new_readers_share_records_without_losing_legacy_charges() {
    #[derive(Deserialize)]
    struct OldLease {
        weight: u64,
        bytes_per_permit: u64,
    }
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 8, GIB);
    pool.record_peak("measured", GIB + 1, false).unwrap();
    let mut demand = Demand::new("measured", false);
    demand.threads = Some(4);
    let permit = pool.admit(&demand).unwrap();
    let bytes = std::fs::read(&permit.path).unwrap();
    let old: OldLease = serde_json::from_slice(&bytes).unwrap();
    let new: Lease = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(old.weight, 4);
    assert_eq!(old.bytes_per_permit, GIB);
    assert!(old.weight >= new.resources().cpu);
    assert!(old.weight * old.bytes_per_permit >= new.resources().memory_bytes);
    // Strip the vector to stand in for an old process's live lease.
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value.as_object_mut().unwrap().remove("cpu");
    value.as_object_mut().unwrap().remove("memory_bytes");
    let legacy: Lease = serde_json::from_value(value).unwrap();
    assert_eq!(
        legacy.resources(),
        Resources {
            cpu: 4,
            memory_bytes: 4 * GIB
        }
    );
    let mut queued = None;
    pool.try_admit_named(
        Resources {
            cpu: 8,
            memory_bytes: GIB,
        },
        8,
        None,
        &mut queued,
        Some(&demand),
    )
    .unwrap();
    let waiting = scan_waiters(&dir.path().join(WAITERS_DIR)).unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].cpu_cap, Some(8));
    assert_eq!(waiting[0].mem_cap, Some(8 * GIB));
    let snapshot = scheduler_snapshot(dir.path(), 8, GIB, true);
    assert!(snapshot.text().contains("charge=4 cores + 1.0 GiB"));
    let activity = snapshot
        .activity
        .iter()
        .find(|item| item.kind == "lease")
        .unwrap();
    assert_eq!(activity.charged_cpu, 4);
    assert_eq!(activity.charged_memory_bytes, GIB + 1);
}

#[test]
fn passing_protects_the_waiting_processes_smaller_memory_budget() {
    let u = MEMORY_QUANTUM;
    let mut older = sim_request(1, 0, 1, 1, ("host", 100), None).record;
    older.cpu = Some(1);
    older.memory_bytes = Some(6 * u);
    older.cpu_cap = Some(100);
    older.mem_cap = Some(10 * u);
    older.domain_memory_cap = Some(100 * u);
    let mut request = older.clone();
    request.ticket = 2;
    request.memory_bytes = Some(2 * u);
    request.mem_cap = Some(12 * u);
    let mut active = older.clone();
    active.memory_bytes = Some(4 * u);
    let live = vec![sim_lease(&active); 2];
    assert_eq!(
        admission_decision(100, &live, [&older].into_iter(), &request),
        AdmissionDecision::Waiting
    );
    assert_eq!(
        admission_decision(100, &live[1..], std::iter::empty(), &older),
        AdmissionDecision::Ordered
    );
}

#[test]
fn resource_admission_refuses_arithmetic_overflow() {
    assert!(!fits(u64::MAX, 1, u64::MAX));
    assert!(!fits(u64::MAX - 1, 2, u64::MAX));
    assert!(fits(0, u64::MAX, 1));
}

#[test]
fn new_processes_with_different_capacities_read_exact_oversize_memory() {
    let dir = tempfile::tempdir().unwrap();
    let small = pool_at(dir.path(), 8, GIB);
    let large = pool_at(dir.path(), 128, GIB);
    small.record_peak("oversize", 100 * GIB, false).unwrap();
    let demand = Demand::new("oversize", false);
    let held = small.admit(&demand).expect("oversize request runs alone");
    let records = scan_leases(&dir.path().join(LEASES_DIR)).unwrap();
    assert_eq!(records[0].weight, 8, "retain the old writer-capacity clamp");
    assert_eq!(
        records[0].resources(),
        Resources {
            cpu: 1,
            memory_bytes: 100 * GIB
        }
    );
    assert!(
        vector_attempt(&large, 1, 29 * GIB).is_none(),
        "the larger pool reads 100 GiB, not eight legacy permits"
    );
    let admitted = vector_attempt(&large, 1, 28 * GIB).expect("exactly the remaining memory fits");
    drop((held, admitted));
}

// Pure scalar policy and prediction copied from fe73826 for compatibility
// assertions. This reader deliberately knows no vector or exact-cap fields.
mod legacy {
    use super::*;
    include!("scheduler_legacy_tests.rs");
}

#[test]
fn old_reader_refuses_passing_a_new_memory_limited_domain_waiter() {
    let dir = tempfile::tempdir().unwrap();
    let mut pool = pool_at(dir.path(), 16, GIB);
    pool.pressure_domain = "host";
    pool.domain_capacity = 8;
    pool.domain_memory_capacity = local_memory_ceiling(16 * GIB, Some(5 * GIB));
    pool.legacy_domain_capacity = legacy_local_ceiling(16, GIB, 8, Some(5 * GIB));
    let first = vector_attempt(&pool, 1, GIB).unwrap();
    let second = vector_attempt(&pool, 1, GIB).unwrap();
    pool.record_peak("head", 3 * GIB, false).unwrap();
    let demand = Demand::new("head", false);
    let plan = pool.demand_vector(&demand);
    let mut queued = None;
    assert!(
        pool.try_admit_named(
            plan.resources,
            plan.weight,
            None,
            &mut queued,
            Some(&demand)
        )
        .unwrap()
        .is_none()
    );
    let waiting = scan_waiters(&dir.path().join(WAITERS_DIR)).unwrap();
    let mut value = serde_json::to_value(&waiting[0]).unwrap();
    assert_eq!(value["domain_capacity"], 4);
    assert_eq!(value["domain_cpu_cap"], 8);
    assert_eq!(
        waiting[0].domain_budget(),
        Resources {
            cpu: 8,
            memory_bytes: pool.domain_memory_capacity
        }
    );
    let older: legacy::Waiting = serde_json::from_value(value.clone()).unwrap();
    let live: Vec<legacy::Lease> = scan_leases(&dir.path().join(LEASES_DIR))
        .unwrap()
        .into_iter()
        .map(|lease| serde_json::from_value(serde_json::to_value(lease).unwrap()).unwrap())
        .collect();
    let younger: legacy::Waiting = serde_json::from_value(serde_json::json!({
        "ticket": 2, "weight": 2, "bytes_per_permit": GIB / 2,
        "priority": "normal", "domain": "host", "domain_capacity": 8,
        "global_capacity": 16, "build": null,
    }))
    .unwrap();
    assert_eq!(
        legacy::admission_decision(16, &live, [&older].into_iter(), &younger),
        legacy::AdmissionDecision::Waiting
    );
    // The reviewer observed this unsafe pass before the legacy field was fixed.
    value["domain_capacity"] = serde_json::json!(8);
    let incorrect: legacy::Waiting = serde_json::from_value(value).unwrap();
    assert_eq!(
        legacy::admission_decision(16, &live, [&incorrect].into_iter(), &younger),
        legacy::AdmissionDecision::Passing
    );
    drop((first, second, queued));
}

#[test]
fn missing_exact_domain_caps_fall_back_to_the_legacy_scalar_on_both_axes() {
    let mut record = sim_request(1, 0, 3, 1, ("host", 4), None).record;
    record.bytes_per_permit = GIB;
    let mut value = serde_json::to_value(record).unwrap();
    value.as_object_mut().unwrap().remove("domain_cpu_cap");
    value.as_object_mut().unwrap().remove("domain_memory_cap");
    let mut record: Waiting = serde_json::from_value(value).unwrap();
    assert_eq!(record.domain_cpu_cap, None);
    assert_eq!(record.domain_memory_cap, None);
    assert_eq!(
        record.domain_budget(),
        Resources {
            cpu: 4,
            memory_bytes: 4 * GIB
        }
    );
    record.domain_cpu_cap = Some(8);
    record.domain_memory_cap = Some(17 * GIB / 4);
    assert_eq!(
        record.domain_budget(),
        Resources {
            cpu: 8,
            memory_bytes: 17 * GIB / 4
        }
    );
}

fn assert_legacy_record_fields(
    pool: &Pool,
    demand: Option<&Demand>,
    weight: u64,
    resources: Resources,
) {
    std::fs::write(pool.dir.join(PRIORITY_WAIT_STAMP), b"").unwrap();
    let held = vector_attempt(pool, pool.capacity, 0).unwrap();
    let mut queued = None;
    assert!(
        pool.try_admit_named(resources, weight, None, &mut queued, demand)
            .unwrap()
            .is_none()
    );
    let records = scan_waiters(&pool.dir.join(WAITERS_DIR)).unwrap();
    assert_eq!(records.len(), 1);
    let waiting = &records[0];
    let legacy_waiting: legacy::Waiting =
        serde_json::from_value(serde_json::to_value(waiting).unwrap()).unwrap();
    let name = demand.map(|d| d.name.clone());
    let mode = demand.map(|d| {
        d.compile_mode
            .clone()
            .unwrap_or_else(|| ledger_key(&d.name, d.links))
    });
    let global = pool.capacity
        - if pool.priority == SchedulerPriority::Low {
            (pool.capacity / 4).max(1).min(pool.capacity - 1)
        } else {
            0
        };
    assert_eq!(
        serde_json::to_value(legacy_waiting).unwrap(),
        serde_json::json!({
            "name": name, "mode_key": mode, "ticket": 1, "weight": weight,
            "priority": pool.priority.as_str(), "domain": pool.pressure_domain,
            "domain_capacity": pool.legacy_domain_capacity, "global_capacity": global,
            "bytes_per_permit": pool.bytes_per_permit, "build": pool.build,
        })
    );
    assert_eq!(waiting.domain_cpu_cap, Some(pool.domain_capacity));
    assert_eq!(waiting.domain_memory_cap, Some(pool.domain_memory_capacity));
    assert_eq!(waiting.domain_budget().cpu, pool.domain_capacity);
    drop((held, queued));
    let permit = pool
        .try_admit_named(resources, weight, None, &mut None, demand)
        .unwrap()
        .unwrap();
    let lease: legacy::Lease =
        serde_json::from_slice(&std::fs::read(&permit.path).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(lease).unwrap(),
        serde_json::json!({
            "version": 1, "name": name, "mode_key": mode, "weight": weight,
            "priority": pool.priority.as_str(), "build_id": pool.build.as_ref().map(|(id, _)| id),
            "domain": pool.pressure_domain, "bytes_per_permit": pool.bytes_per_permit,
        })
    );
}

#[test]
fn legacy_lease_and_waiter_schema_is_preserved_across_estimator_demands_and_limits() {
    for (unit, local_cpus, local_memory) in [
        (GIB, 8, Some(5 * GIB)),
        (GIB / 2, 8, Some(5 * GIB)),
        (0, 8, Some(5 * GIB)),
        (GIB, 8, Some(64 * GIB)),
        (GIB, 32, Some(GIB)),
        (GIB, 32, None),
        (13, 32, Some(208)),
    ] {
        for priority in [SchedulerPriority::Normal, SchedulerPriority::Low] {
            let dir = tempfile::tempdir().unwrap();
            let mut pool = Pool::new(dir.path().into(), 16, unit, priority);
            assert_eq!(
                pool.legacy_domain_capacity,
                legacy::local_ceiling(
                    16,
                    unit,
                    default_capacity(),
                    crate::util::memory_total_bytes()
                )
            );
            pool.available_memory = || None;
            pool.pressure = false;
            std::fs::write(dir.path().join(PRIORITY_WAIT_STAMP), b"").unwrap();
            pool.pressure_domain = "host";
            pool.build = Some(("build".into(), 12));
            pool.domain_capacity = 16.min(local_cpus);
            // The old ceiling uses the rounded permit budget even when the
            // exact byte budget crosses the local-memory boundary.
            pool.memory_capacity = 16 * unit + u64::from(unit == 13);
            pool.domain_memory_capacity = local_memory_ceiling(pool.memory_capacity, local_memory);
            pool.legacy_domain_capacity = legacy_local_ceiling(16, unit, local_cpus, local_memory);
            assert_eq!(
                pool.legacy_domain_capacity,
                legacy::local_ceiling(16, unit, local_cpus, local_memory)
            );
            pool.record_peak("measured", 7 * GIB + 1, false).unwrap();
            let mode = compile_demand("mode", &["--emit=metadata"]);
            pool.record_peak("mode", 7 * GIB, false).unwrap();
            for _ in 0..3 {
                pool.record_compile(&mode, GIB + 1, false, true).unwrap();
            }
            pool.record_peak("measured-link", 3 * GIB + 1, true)
                .unwrap();
            for _ in 0..MIN_LINK_SAMPLES {
                pool.record_link_peak("window-sample [link]", 2 * GIB + 1, true, None)
                    .unwrap();
            }
            let suite = Demand::test("suite", None);
            pool.record_peak(&suite.name, 2 * GIB + 1, false).unwrap();
            pool.record_cpu(&suite.name, 3).unwrap();
            pool.record_test_history("bin", "case", Some(GIB + 1), Some(2))
                .unwrap();
            let mut measured = Demand::new("measured", false);
            measured.threads = Some(4);
            for demand in [
                Demand::new("cold", false),
                Demand::new("cold-link", true)
                    .with_link_profile(&[std::ffi::OsString::from("-Clto=fat")]),
                Demand::test("cold-suite", Some(3)),
                Demand::test("default-suite", None),
                Demand::nextest("cold-bin", "case"),
                measured,
                mode,
                Demand::new("measured-link", true),
                Demand::new("window-link", true),
                suite,
                Demand::nextest("bin", "case"),
            ] {
                let expected = legacy::prediction(&pool, &demand);
                let plan = pool.demand_vector(&demand);
                if demand.compile_mode.is_none() && !demand.links {
                    assert_eq!((plan.weight, plan.predicted), (expected.0, expected.1));
                }
                assert_legacy_record_fields(&pool, Some(&demand), plan.weight, plan.resources);
            }
            let memory = (unit > 0).then_some(3 * unit + 1);
            let reservation_weight = 2.max(memory.map_or(0, |bytes| bytes.div_ceil(unit)));
            assert_eq!(
                pool.reservation_weight(Some(2), memory).unwrap(),
                reservation_weight
            );
            assert_legacy_record_fields(
                &pool,
                None,
                reservation_weight,
                Resources {
                    cpu: 2,
                    memory_bytes: memory.unwrap_or(0),
                },
            );
        }
    }
}

fn mixed_legacy_pass_fixture() -> (Vec<Lease>, Waiting, Waiting) {
    let mut head = sim_request(1, 0, 4, 1, ("host", 12), None).record;
    head.bytes_per_permit = GIB;
    head.global_capacity = Some(12);
    let mut request = head.clone();
    request.ticket = 2;
    request.weight = 1;
    request.cpu = Some(1);
    request.memory_bytes = Some(GIB);
    let live = [(4, 1), (1, 4), (2, 2)]
        .map(|(cpu, mem)| {
            let mut record = request.clone();
            record.weight = cpu.max(mem);
            record.cpu = Some(cpu);
            record.memory_bytes = Some(mem * GIB);
            sim_lease(&record)
        })
        .to_vec();
    (live, head, request)
}

#[test]
fn mixed_legacy_waiter_requires_scalar_and_vector_slack() {
    let (live, head, request) = mixed_legacy_pass_fixture();
    // Marking the head as a new writer leaves the vector-only policy in place.
    let mut vector_head = head.clone();
    vector_head.cpu = Some(4);
    vector_head.memory_bytes = Some(4 * GIB);
    assert_eq!(
        admission_decision(12, &live, [&vector_head].into_iter(), &request),
        AdmissionDecision::Passing
    );
    assert_eq!(
        admission_decision(12, &live, [&head].into_iter(), &request),
        AdmissionDecision::Waiting
    );
    // The weight-2 completion frees exactly enough scalar room without a passer.
    let old_head: legacy::Waiting =
        serde_json::from_value(serde_json::to_value(&head).unwrap()).unwrap();
    let mut remaining: Vec<legacy::Lease> = live[..2]
        .iter()
        .map(|lease| serde_json::from_value(serde_json::to_value(lease).unwrap()).unwrap())
        .collect();
    assert_eq!(
        legacy::admission_decision(12, &remaining, std::iter::empty(), &old_head),
        legacy::AdmissionDecision::Ordered
    );
    remaining
        .push(serde_json::from_value(serde_json::to_value(sim_lease(&request)).unwrap()).unwrap());
    assert_eq!(
        legacy::admission_decision(12, &remaining, std::iter::empty(), &old_head),
        legacy::AdmissionDecision::Waiting
    );
    // Scalar slack alone is insufficient when the exact CPU ceiling is tighter.
    let mut tight = request.clone();
    tight.cpu_cap = Some(11);
    tight.domain_capacity = 20;
    assert!(can_pass_waiters(20, &[4, 4, 2], &[4], 1));
    let mut roomy_head = head.clone();
    roomy_head.global_capacity = Some(20);
    roomy_head.domain_capacity = 20;
    assert_eq!(
        admission_decision(20, &live, [&roomy_head].into_iter(), &tight),
        AdmissionDecision::Waiting
    );
    let mut roomy = request.clone();
    roomy.global_capacity = Some(20);
    roomy.domain_capacity = 20;
    roomy_head.domain_capacity = 20;
    assert_eq!(
        admission_decision(20, &live, [&roomy_head].into_iter(), &roomy),
        AdmissionDecision::Passing
    );
}

#[test]
fn mixed_legacy_waiter_blocks_passer_in_real_pool() {
    let dir = tempfile::tempdir().unwrap();
    let mut pool = pool_at(dir.path(), 12, GIB);
    pool.pressure_domain = "host";
    let held: Vec<_> = [(4, 1), (1, 4), (2, 2)]
        .into_iter()
        .map(|(cpu, mem)| pool.reserve(Some(cpu), Some(mem * GIB)).unwrap())
        .collect();
    let (live, head, request) = mixed_legacy_pass_fixture();
    let waiters = dir.path().join(WAITERS_DIR);
    std::fs::create_dir_all(&waiters).unwrap();
    let _head_guard = pool
        .lock_record(&waiters, &serde_json::to_vec(&head).unwrap())
        .unwrap();
    let mut vector_head = head.clone();
    vector_head.cpu = Some(4);
    vector_head.memory_bytes = Some(4 * GIB);
    assert_eq!(
        admission_decision(12, &live, [&vector_head].into_iter(), &request),
        AdmissionDecision::Passing
    );
    assert!(pool.try_admit_queued(1, None, &mut None).unwrap().is_none());
    drop(_head_guard);
    let _vector_head_guard = pool
        .lock_record(&waiters, &serde_json::to_vec(&vector_head).unwrap())
        .unwrap();
    assert!(pool.try_admit_queued(1, None, &mut None).unwrap().is_some());
    drop(held);
}

#[test]
fn mixed_version_passes_preserve_legacy_waiters_in_counterfactuals() {
    let mut total_passes = 0;
    for seed in 0..512_u64 {
        let mut state = seed + 1;
        let capacity = sim_random(&mut state, 8, 24);
        let mut make_record = |ticket| {
            let unit = GIB * sim_random(&mut state, 1, 2);
            let weight = sim_random(&mut state, 1, 4);
            let mut record = sim_request(
                ticket,
                0,
                weight,
                1,
                ("host", capacity),
                Some(("build", capacity)),
            )
            .record;
            record.global_capacity = Some(sim_random(&mut state, 8, capacity));
            record.bytes_per_permit = unit;
            // Every third waiter is genuinely legacy-only. New records keep
            // a conservative scalar weight but vary their dominant resource.
            if ticket % 3 != 1 {
                record.cpu = Some(sim_random(&mut state, 1, weight));
                record.memory_bytes = Some(sim_random(&mut state, 1, weight) * unit);
            }
            record
        };
        let initial: Vec<_> = (0..3).map(|_| (1, make_record(0))).collect();
        let mut requests: Vec<_> = (1..=12)
            .map(|ticket| SimRequest {
                arrival: 0,
                duration: 1,
                record: make_record(ticket),
            })
            .collect();
        // Different completion times exercise every admitted passer's fork.
        let initial: Vec<_> = initial
            .into_iter()
            .map(|(_, record)| (sim_random(&mut state, 1, 20), record))
            .collect();
        for request in &mut requests {
            request.duration = sim_random(&mut state, 1, 20);
        }
        let mut simulation = AdmissionSimulation::new(capacity, &initial, &requests);
        simulation.legacy_readers = true;
        simulation.queue = (0..requests.len()).collect();
        while !simulation.queue.is_empty() {
            simulation.expire();
            total_passes += simulation.step(SimPolicy::TodayOrPass, Some(true));
            if !simulation.queue.is_empty() {
                simulation.now = *simulation
                    .finishes
                    .iter()
                    .min()
                    .expect("live work unblocks queue");
            }
        }
    }
    eprintln!("mixed-version seeds 0..512: {total_passes} passes checked");
    assert!(
        total_passes > 0,
        "mixed-version workloads must exercise passing"
    );
}

#[test]
fn legacy_waiter_scalar_slack_uses_its_units_and_each_shared_ceiling() {
    for scope in [
        "global",
        "missing-global",
        "domain",
        "build",
        "changed-unit",
    ] {
        let (mut live, mut head, mut request) = mixed_legacy_pass_fixture();
        head.global_capacity = Some(20);
        head.domain_capacity = 20;
        request.global_capacity = Some(20);
        request.domain_capacity = 20;
        match scope {
            "global" => head.global_capacity = Some(12),
            "missing-global" => head.global_capacity = None,
            "domain" => head.domain_capacity = 12,
            "build" => {
                head.build = Some(("build".into(), 12));
                request.build = Some(("build".into(), 20));
                for lease in &mut live {
                    lease.build_id = Some("build".into());
                }
            }
            "changed-unit" => {
                // The head's half-sized units double every lease and the
                // passer, while retaining the CPU floor of its own weight.
                head.bytes_per_permit = GIB / 2;
                head.weight = 8;
                head.global_capacity = Some(24);
                head.domain_capacity = 40;
            }
            _ => unreachable!(),
        }
        let fallback = if scope == "missing-global" { 12 } else { 20 };
        let mut vector_head = head.clone();
        vector_head.cpu = Some(head.weight);
        vector_head.memory_bytes = Some(head.weight * head.bytes_per_permit);
        assert_eq!(
            admission_decision(fallback, &live, [&vector_head].into_iter(), &request),
            AdmissionDecision::Passing,
            "vector policy: {scope}"
        );
        assert_eq!(
            admission_decision(fallback, &live, [&head].into_iter(), &request),
            AdmissionDecision::Waiting,
            "scalar policy: {scope}"
        );
    }
}

#[test]
fn calibrated_windows_preserve_legacy_ledger_fields_against_f47d416() {
    for unit in [0, 10, 100, 1000] {
        let old_dir = tempfile::tempdir().unwrap();
        let new_dir = tempfile::tempdir().unwrap();
        let old = pool_at(old_dir.path(), 100, unit);
        let new = pool_at(new_dir.path(), 100, unit);
        for (index, (peak, oom, success, wrapped)) in [
            (1, false, true, false),
            (99, false, true, false),
            (200, false, false, false),
            (81, false, true, true),
            (1000, false, true, false),
            (81, false, true, false),
            (81, false, true, false),
            (81, false, true, false),
            (80, true, false, false),
            (1200, false, true, true),
            (900, true, false, true),
            (1, true, false, false),
        ]
        .into_iter()
        .enumerate()
        {
            let mut demand = compile_demand(
                "crate",
                if index % 2 == 0 {
                    &["--emit=metadata"]
                } else {
                    &[]
                },
            );
            demand.count_compile_samples = !wrapped;
            legacy::record_compile(&old, &demand, peak, oom, success).unwrap();
            new.record_compile(&demand, peak, oom, success).unwrap();
            let before = read_ledger(&old.ledger_path());
            let after = read_ledger(&new.ledger_path());
            assert_eq!(before.crates, after.crates, "step={index}, unit={unit}");
            assert_eq!(before.compile_samples, after.compile_samples);
            assert_eq!(before.link_peaks, after.link_peaks);
            assert_eq!(before.profile_link_peaks, after.profile_link_peaks);
        }
        for peak in [1000, 200, 300, 400, 200, 1].into_iter().cycle().take(40) {
            for (key, links, profile) in [
                ("crate [link]", true, None),
                ("other [opt3] [link]", true, Some("opt3")),
                ("suite [test]", false, None),
            ] {
                legacy::record_link_peak(&old, key, peak, links, profile).unwrap();
                new.record_link_peak(key, peak, links, profile).unwrap();
                let before = read_ledger(&old.ledger_path());
                let after = read_ledger(&new.ledger_path());
                assert_eq!(before.crates, after.crates);
                assert_eq!(before.compile_samples, after.compile_samples);
                assert_eq!(before.link_peaks, after.link_peaks);
                assert_eq!(before.profile_link_peaks, after.profile_link_peaks);
            }
        }
    }
}

#[test]
fn calibrated_compile_oom_windows_roll_and_residual_features_are_frozen() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 100, 100);
    let demand = compile_demand("crate", &[]);
    let key = demand.compile_mode.as_ref().unwrap();
    pool.record_compile(&demand, 200, false, true).unwrap();
    assert_eq!(
        read_ledger(&pool.ledger_path()).memory_windows[key],
        vec![200]
    );
    pool.demand_vector(&demand);
    // A concurrent completion changes the window after this plan was admitted.
    let concurrent = compile_demand("crate", &[]);
    pool.record_compile(&concurrent, 100, false, true).unwrap();
    pool.record_compile(&demand, 400, false, true).unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    assert!((ledger.memory_residuals["compile"].last().unwrap() - 2.0_f64.ln()).abs() < 1e-12);
    pool.record_compile(&demand, 900, true, false).unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.memory_windows[key], vec![200, 100, 400, 1800]);
    calibrate_kind(&pool, "compile");
    assert!(pool.demand_vector(&demand).resources.memory_bytes >= 1800);
    let mut wrapped = compile_demand("crate", &[]);
    wrapped.count_compile_samples = false;
    pool.record_compile(&wrapped, 9999, false, true).unwrap();
    assert_eq!(
        read_ledger(&pool.ledger_path()).memory_windows[key],
        vec![200, 100, 400, 1800]
    );
    pool.record_compile(&wrapped, 2000, true, false).unwrap();
    assert_eq!(
        read_ledger(&pool.ledger_path()).memory_windows[key].last(),
        Some(&4000)
    );
    for _ in 0..16 {
        pool.record_compile(&demand, 100, false, true).unwrap();
    }
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.memory_windows[key], vec![100; 16]);
    assert!(pool.demand_vector(&demand).resources.memory_bytes < 1800);
    assert_eq!(ledger.crates["crate"], 9999);
}

#[test]
fn calibrated_link_hierarchy_and_wrapped_failures_preserve_legacy_windows() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 100, 100);
    let demand = Demand::new("crate", true);
    assert_eq!(pool.demand_vector(&demand).source, "link-floor");
    pool.record_link_measurement("crate [link]", Some(300), true, None, None)
        .unwrap();
    assert_eq!(pool.demand_vector(&demand).source, "legacy");
    assert_eq!(pool.demand_vector(&demand).resources.memory_bytes, 300);
    assert!(
        !read_ledger(&pool.ledger_path())
            .memory_windows
            .contains_key("crate [link]")
    );
    pool.record_link_measurement(
        "crate [link]",
        Some(200),
        true,
        None,
        Some(memory::Sample {
            peak: 200,
            features: None,
            calibrate: true,
        }),
    )
    .unwrap();
    calibrate_kind(&pool, "link");
    let plan = pool.demand_vector(&demand);
    assert_eq!(plan.source, "window");
    // One sample of its own: held to its legacy 300 bytes, not the 904-byte bound.
    assert_eq!(plan.resources.memory_bytes, 300);
    pool.record_link_measurement(
        "crate [link]",
        Some(1000),
        true,
        None,
        Some(memory::Sample {
            peak: 1000,
            features: demand.memory_features.take(),
            calibrate: false,
        }),
    )
    .unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(ledger.memory_windows["crate [link]"], vec![200, 1000]);
    assert!(!ledger.memory_residuals.contains_key("link"));
    let profiled = Demand::new("new", true).with_link_profile(&["-Clto=fat".into()]);
    assert_eq!(pool.demand_vector(&profiled).source, "link-floor");
}

#[test]
fn calibrated_stats_show_window_math_and_kind_calibration() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 100, 100);
    let demand = compile_demand("crate", &[]);
    pool.record_compile(&demand, 100, false, true).unwrap();
    calibrate_kind(&pool, "compile");
    let report = scheduler_snapshot(dir.path(), 100, 100, true);
    let json = serde_json::to_value(&report).unwrap();
    let entry = json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["key"] == demand.compile_mode.as_ref().unwrap().as_str())
        .unwrap();
    assert_eq!(entry["n"], 1);
    assert_eq!(entry["window_max_bytes"], 100);
    assert_eq!(entry["bound_bytes"], 452);
    // One sample of its own: a light crate is held to the one-permit default.
    assert_eq!(entry["charge_bytes"], 100);
    assert_eq!(entry["source"], "window");
    assert_eq!(
        json["memory_calibration"][0]["sigma_pool"],
        0.5002389229311026
    );
    assert_eq!(json["memory_calibration"][0]["target_p"], 0.95);
    assert_eq!(json["memory_calibration"][0]["residual_count"], 0);
    let text = report.text();
    assert!(
        text.contains("n=1, window max=100 B, bound=452 B"),
        "{text}"
    );
    assert!(text.contains("sigma_pool=0.500000, residuals=0, p=0.95"));
}

#[test]
fn native_filename_collisions_keep_the_legacy_maximum_and_never_teach_windows() {
    let dir = tempfile::tempdir().unwrap();
    let unit = 1024 * 1024;
    let pool = pool_at(dir.path(), 100, unit);
    for name in ["cc:common.cpp", "other-plain-key"] {
        let demand = Demand::new(name, false);
        assert_eq!(pool.demand_vector(&demand).source, "legacy");
        assert_eq!(pool.demand_vector(&demand).resources.memory_bytes, unit);
        for peak in std::iter::once(GIB).chain(std::iter::repeat_n(unit, 16)) {
            pool.record_link_measurement(
                name,
                ledger_peak(peak, false, false, true, unit),
                false,
                None,
                Some(memory::Sample {
                    peak,
                    features: None,
                    calibrate: true,
                }),
            )
            .unwrap();
        }
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(pool.ledger_path()).unwrap()).unwrap();
        assert!(
            !stored["memory_windows"]
                .as_object()
                .unwrap()
                .contains_key(name)
        );
        assert!(stored["memory_log_ratios"].as_object().unwrap().is_empty());
        // Even stale windows from the previous implementation must not retire
        // the filename's maximum or influence the pooled spread on read.
        let mut ledger = read_ledger(&pool.ledger_path());
        ledger.memory_windows.insert(name.into(), vec![unit; 16]);
        std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
        let plan = pool.demand_vector(&demand);
        assert_eq!(plan.predicted, Some(GIB));
        assert_eq!(plan.resources.memory_bytes, GIB);
        assert_eq!(plan.source, "legacy");
        let ledger = read_ledger(&pool.ledger_path());
        assert!(!ledger.memory_windows.contains_key(name));
        assert!(ledger.memory_residuals.is_empty());
        let report = scheduler_snapshot(dir.path(), 100, unit, true);
        let entry = report.entries.iter().find(|e| e.key == name).unwrap();
        assert_eq!(entry.charge_bytes, Some(GIB));
        assert_eq!(entry.source, "legacy");
        assert_eq!(entry.n, 0);
    }
}

#[test]
fn a_cold_compile_does_not_calibrate_from_another_completion_while_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 100, 100);
    let waiting = compile_demand("crate", &[]);
    assert_eq!(pool.demand_vector(&waiting).source, "default");
    let first = compile_demand("crate", &[]);
    pool.record_compile(&first, 200, false, true).unwrap();
    pool.record_compile(&waiting, 300, false, true).unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    assert_eq!(
        ledger.memory_windows[waiting.compile_mode.as_ref().unwrap()],
        vec![200, 300]
    );
    assert!(ledger.memory_residuals.is_empty());
}

#[test]
fn missing_windows_consult_legacy_keys_before_siblings_and_profile_links() {
    let dir = tempfile::tempdir().unwrap();
    let unit = 1024 * 1024;
    let pool = pool_at(dir.path(), 100, unit);
    let heavy_link = Demand::new("heavy", true).with_link_profile(&[]);
    let mode = compile_demand("crate", &["--emit=metadata"]);
    let sibling = compile_demand("crate", &["--emit=mir"]);
    pool.record_link_peak(
        &ledger_key(&heavy_link.name, true),
        GIB,
        true,
        heavy_link.profile.as_deref(),
    )
    .unwrap();
    for _ in 0..16 {
        pool.record_link_peak("tiny [link]", unit, true, heavy_link.profile.as_deref())
            .unwrap();
        pool.record_compile(&sibling, unit, false, true).unwrap();
    }
    pool.record_peak("crate", 2 * GIB, false).unwrap();
    for _ in 0..3 {
        pool.record_compile(&mode, GIB, false, true).unwrap();
    }
    let mut ledger = read_ledger(&pool.ledger_path());
    // Reproduce eviction without dropping the sibling or profile windows.
    ledger
        .memory_windows
        .remove(&ledger_key(&heavy_link.name, true));
    ledger
        .memory_windows
        .remove(mode.compile_mode.as_ref().unwrap());
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
    for (demand, expected) in [(&heavy_link, GIB), (&mode, GIB + GIB / 2)] {
        let plan = pool.demand_vector(demand);
        assert_eq!(plan.source, "legacy");
        assert_eq!(plan.resources.memory_bytes, expected);
        let report = scheduler_snapshot(dir.path(), 100, unit, true);
        let key = demand
            .compile_mode
            .clone()
            .unwrap_or_else(|| ledger_key(&demand.name, true));
        let entry = report
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .unwrap();
        assert_eq!(entry.source, plan.source);
        assert_eq!(entry.charge_bytes, plan.predicted);
    }
}

#[test]
fn an_old_writer_rewrite_preserves_at_least_f47d416_mode_and_link_charges() {
    let dir = tempfile::tempdir().unwrap();
    let unit = 1024 * 1024;
    let pool = pool_at(dir.path(), 100, unit);
    let mut demands = Vec::new();
    // Include untrusted modes (1/2 samples), trusted modes, and different
    // profile links. Old charges include the original headroom/capping rule.
    for count in 1..=4 {
        let name = format!("crate{count}");
        pool.record_peak(&name, 2 * GIB, false).unwrap();
        let mode = compile_demand(&name, &["--emit=metadata"]);
        for _ in 0..count {
            pool.record_compile(&mode, GIB, false, true).unwrap();
        }
        demands.push(mode);
    }
    for flags in [vec![], vec!["-Clto=fat".into()]] {
        let link = Demand::new("heavy", true).with_link_profile(&flags);
        pool.record_link_peak(
            &ledger_key(&link.name, true),
            GIB,
            true,
            link.profile.as_deref(),
        )
        .unwrap();
        for _ in 0..16 {
            pool.record_link_peak("tiny [link]", unit, true, link.profile.as_deref())
                .unwrap();
        }
        demands.push(link);
    }
    // Include the unrelated small link too: check every recorded link key.
    demands.push(Demand::new("tiny", true));
    let mut json = serde_json::to_value(read_ledger(&pool.ledger_path())).unwrap();
    let object = json.as_object_mut().unwrap();
    object.remove("memory_windows");
    object.remove("memory_residuals");
    assert!(!object.contains_key("pooled_spreads"));
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&json).unwrap()).unwrap();
    let report = scheduler_snapshot(dir.path(), 100, unit, true);
    for demand in demands {
        let (_, old_charge, _) = legacy::prediction(&pool, &demand);
        let plan = pool.demand_vector(&demand);
        assert!(
            plan.predicted.unwrap() >= old_charge.unwrap(),
            "{}",
            demand.name
        );
        assert_eq!(plan.source, "legacy");
        let key = demand
            .compile_mode
            .clone()
            .unwrap_or_else(|| ledger_key(&demand.name, true));
        let entry = report
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .unwrap();
        assert_eq!(entry.source, plan.source);
        assert_eq!(entry.charge_bytes, plan.predicted);
    }
    assert!(read_ledger(&pool.ledger_path()).memory_windows.is_empty());
}

#[test]
fn sibling_residuals_only_use_frozen_successful_unwrapped_admissions() {
    for (succeeded, oom, wrapped) in [
        (true, false, false),
        (false, false, false),
        (false, true, false),
        (true, false, true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_at(dir.path(), 100, 100);
        let sibling = compile_demand("crate", &["--emit=metadata"]);
        pool.record_compile(&sibling, 200, false, true).unwrap();
        let mut demand = compile_demand("crate", &[]);
        demand.count_compile_samples = !wrapped;
        assert_eq!(pool.demand_vector(&demand).source, "sibling");
        let features = demand.sibling_features.get().unwrap();
        assert_eq!(demand.memory_features.get().unwrap().scale, 0.0);
        // A concurrent completion changes both windows before this admission
        // finishes. Neither residual history may use those newer features.
        pool.record_compile(&sibling, 1000, false, true).unwrap();
        let concurrent = compile_demand("crate", &[]);
        pool.record_compile(&concurrent, 250, false, true).unwrap();
        let own_residual_count = read_ledger(&pool.ledger_path())
            .memory_residuals
            .get("compile")
            .map_or(0, Vec::len);
        pool.record_compile(&demand, 300, oom, succeeded).unwrap();
        let ledger = read_ledger(&pool.ledger_path());
        assert_eq!(
            ledger.memory_residuals.get("compile").map_or(0, Vec::len),
            own_residual_count
        );
        if succeeded && !oom && !wrapped {
            assert_eq!(ledger.sibling_residuals.len(), 1);
            assert!(
                (ledger.sibling_residuals[0] - (300.0_f64.ln() - features.mean) / features.scale)
                    .abs()
                    < 1e-12
            );
            assert_eq!(
                ledger.memory_windows[demand.compile_mode.as_ref().unwrap()].last(),
                Some(&300)
            );
        } else {
            assert!(ledger.sibling_residuals.is_empty());
        }
        // Direct recording and an own-window admission do not train siblings.
        let count = ledger.sibling_residuals.len();
        let own = compile_demand("crate", &[]);
        assert_eq!(pool.demand_vector(&own).source, "window");
        assert!(own.sibling_features.get().is_none());
        pool.record_compile(&own, 400, false, true).unwrap();
        assert_eq!(
            read_ledger(&pool.ledger_path()).sibling_residuals.len(),
            count
        );
        let report = scheduler_snapshot(dir.path(), 100, 100, true);
        let calibration = report
            .memory_calibration
            .iter()
            .find(|c| c.kind == "compile-sibling")
            .unwrap();
        assert_eq!(calibration.residual_count, count);
        assert_eq!(calibration.sigma_pool, 0.5);
        assert!(
            report
                .text()
                .contains("compile-sibling: between_mode_spread=0.500000")
        );
    }
}

#[test]
fn sibling_stats_include_the_bound_for_an_unsampled_active_mode() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 100, 100);
    let sibling = compile_demand("crate", &["--emit=metadata"]);
    pool.record_compile(&sibling, 200, false, true).unwrap();
    let demand = compile_demand("crate", &[]);
    let mode = demand.compile_mode.as_ref().unwrap();
    // Active modes need not have a ledger key yet.
    std::fs::create_dir_all(dir.path().join(WAITERS_DIR)).unwrap();
    std::fs::write(
        dir.path().join(WAITERS_DIR).join("active.json"),
        serde_json::to_vec(&serde_json::json!({
            "pid": std::process::id(), "weight": 2, "ticket": 1,
            "name": "crate", "mode_key": mode,
            "bytes_per_permit": 100, "domain": "test",
            "domain_capacity": 100, "priority": "background", "build": null
        }))
        .unwrap(),
    )
    .unwrap();
    let report = scheduler_snapshot(dir.path(), 100, 100, true);
    let entry = report.entries.iter().find(|e| &e.key == mode).unwrap();
    let ledger = read_ledger(&pool.ledger_path());
    let estimate = memory::sibling_estimate(&ledger, "crate", 100).unwrap();
    assert_eq!(entry.source, "sibling");
    assert_eq!(entry.bound_bytes, Some(estimate.bound));
    assert_eq!(entry.q, Some(estimate.q));
    assert_eq!(entry.charge_bytes, Some(estimate.charge));
    assert!(report.text().contains("(sibling)"));
}

#[test]
fn waiting_prediction_refreshes_charge_and_admission_features() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 10, 100);
    let demand = compile_demand("crate", &[]);
    let key = demand.compile_mode.as_ref().unwrap();
    let mut ledger = MemoryLedger::default();
    ledger.memory_windows.insert(key.clone(), vec![500; 4]);
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
    let mut cache = LedgerCache::default();
    let plan = pool.demand_vector_with_ledger(&demand, cache.read(&pool.ledger_path()));
    let live = pool.try_admit(9, None).unwrap().unwrap();
    let mut queued = None;
    assert!(
        pool.try_admit_named(
            plan.resources,
            plan.weight,
            None,
            &mut queued,
            Some(&demand)
        )
        .unwrap()
        .is_none()
    );
    let ticket = queued.as_ref().unwrap().ticket;
    ledger.memory_windows.insert(key.clone(), vec![10; 4]);
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
    let refreshed = pool.demand_vector_with_ledger(&demand, cache.read(&pool.ledger_path()));
    assert!(refreshed.resources.memory_bytes < plan.resources.memory_bytes);
    // Still queued: publish the new charge while preserving queue order.
    assert!(
        pool.try_admit_named(
            Resources {
                cpu: 2,
                ..refreshed.resources
            },
            10,
            None,
            &mut queued,
            Some(&demand)
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(queued.as_ref().unwrap().ticket, ticket);
    let waiting = scan_waiters(&dir.path().join(WAITERS_DIR)).unwrap();
    assert_eq!(
        waiting[0].memory_bytes,
        Some(refreshed.resources.memory_bytes)
    );
    let permit = pool
        .try_admit_named(
            refreshed.resources,
            refreshed.weight,
            None,
            &mut queued,
            Some(&demand),
        )
        .unwrap()
        .unwrap();
    ledger.memory_windows.insert(key.clone(), vec![1000; 4]);
    std::fs::write(pool.ledger_path(), serde_json::to_vec(&ledger).unwrap()).unwrap();
    pool.record_compile(&demand, 20, false, true).unwrap();
    let recorded = read_ledger(&pool.ledger_path());
    assert!((recorded.memory_residuals["compile"].last().unwrap() - 2.0_f64.ln()).abs() < 1e-12);
    drop((permit, live));
}

#[test]
#[cfg(unix)]
fn ledger_cache_notices_atomic_replacement_with_same_time_and_size() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(LEDGER_FILE);
    let mut ledger = MemoryLedger::default();
    ledger.crates.insert("crate".into(), 100);
    crate::util::write_advisory(&path, &serde_json::to_vec(&ledger).unwrap()).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let mut cache = LedgerCache::default();
    assert_eq!(cache.read(&path).crates["crate"], 100);
    ledger.crates.insert("crate".into(), 200);
    let mut replacement = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    replacement
        .write_all(&serde_json::to_vec(&ledger).unwrap())
        .unwrap();
    filetime::set_file_mtime(
        replacement.path(),
        filetime::FileTime::from_last_modification_time(&metadata),
    )
    .unwrap();
    replacement.persist(&path).unwrap();
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(metadata.len(), after.len());
    assert_eq!(metadata.modified().unwrap(), after.modified().unwrap());
    assert_eq!(cache.read(&path).crates["crate"], 200);
}

#[test]
fn ledger_cache_bounds_staleness_when_file_metadata_does_not_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(LEDGER_FILE);
    let mut ledger = MemoryLedger::default();
    ledger.crates.insert("crate".into(), 100);
    std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let mut cache = LedgerCache::default();
    assert_eq!(cache.read(&path).crates["crate"], 100);
    ledger.crates.insert("crate".into(), 200);
    std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    filetime::set_file_mtime(
        &path,
        filetime::FileTime::from_last_modification_time(&metadata),
    )
    .unwrap();
    assert!(cache.stamp == LedgerStamp::read(&path));
    assert_eq!(cache.read(&path).crates["crate"], 100);
    cache.loaded_at = Some(Instant::now() - LEDGER_CACHE_MAX_AGE);
    assert_eq!(cache.read(&path).crates["crate"], 200);
}

#[test]
fn a_waiting_admission_plan_refreshes_when_history_changes() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 10, 100);
    let demand = compile_demand("crate", &[]);
    let key = demand.compile_mode.as_ref().unwrap();
    let mut ledger = MemoryLedger::default();
    ledger.memory_windows.insert(key.clone(), vec![500; 4]);
    crate::util::write_advisory(&pool.ledger_path(), &serde_json::to_vec(&ledger).unwrap())
        .unwrap();
    let mut cache = None;
    let first = pool.admission_plan(&demand, &mut cache);
    let held = pool.try_admit(9, None).unwrap().unwrap();
    let mut state = AdmissionState::new(100);
    assert!(
        pool.try_admit_state(
            first.resources,
            first.weight,
            first.predicted,
            &mut state,
            Some(&demand)
        )
        .unwrap()
        .is_none()
    );

    ledger.memory_windows.insert(key.clone(), vec![10; 4]);
    crate::util::write_advisory(&pool.ledger_path(), &serde_json::to_vec(&ledger).unwrap())
        .unwrap();
    let changed = pool.admission_plan(&demand, &mut cache);
    assert!(changed.resources.memory_bytes < first.resources.memory_bytes);
    assert_eq!(demand.memory_features.get().unwrap().mean, 10.0_f64.ln());
    assert!(
        pool.try_admit_state(
            changed.resources,
            changed.weight,
            changed.predicted,
            &mut state,
            Some(&demand)
        )
        .unwrap()
        .is_some()
    );
    drop(held);
}

#[test]
fn a_waiting_compile_refreshes_its_plan_before_admission() {
    let directory = tempfile::tempdir().unwrap();
    let pool = pool_at(directory.path(), 10, 100);
    let demand = compile_demand("crate", &[]);
    let key = demand.compile_mode.as_ref().unwrap().clone();
    let mut ledger = MemoryLedger::default();
    ledger.memory_windows.insert(key.clone(), vec![500; 4]);
    crate::util::write_advisory(&pool.ledger_path(), &serde_json::to_vec(&ledger).unwrap())
        .unwrap();
    let held = pool.try_admit(9, None).unwrap().unwrap();
    let ledger_path = pool.ledger_path();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _permit = pool.admit(&demand).unwrap();
        let admission = demand.observation.get().unwrap();
        send.send((
            admission.resources.memory_bytes,
            demand.memory_features.get().unwrap().mean,
        ))
        .unwrap();
    });
    wait_until("the compile to queue", || {
        directory.path().join(PRIORITY_WAIT_STAMP).exists()
    });
    ledger.memory_windows.insert(key, vec![10; 4]);
    // Windows refuses to replace a file the waiting compile has open while it
    // rereads history; production drops such a write, the test retries it.
    let contents = serde_json::to_vec(&ledger).unwrap();
    let mut written = crate::util::write_advisory(&ledger_path, &contents);
    for _ in 0..500 {
        if written.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        written = crate::util::write_advisory(&ledger_path, &contents);
    }
    written.unwrap();

    let admitted = receive.recv_timeout(Duration::from_secs(30));
    // Always release the blocker so a stale plan fails without stranding a worker.
    drop(held);
    worker.join().unwrap();
    let (memory, mean) = admitted.expect("the refreshed compile fits beside the held permit");
    assert!(memory <= 100);
    assert_eq!(mean, 10.0_f64.ln());
}

#[test]
fn waiting_prediction_is_reused_until_ledger_reload() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 10, 100);
    let demand = compile_demand("crate", &[]);
    let key = demand.compile_mode.as_ref().unwrap();
    let mut ledger = MemoryLedger::default();
    ledger.memory_windows.insert(key.clone(), vec![500; 4]);
    crate::util::write_advisory(&pool.ledger_path(), &serde_json::to_vec(&ledger).unwrap())
        .unwrap();
    let mut cache = LedgerCache::default();
    let predictions = std::cell::Cell::new(0);
    let predict = |ledger: &MemoryLedger| {
        predictions.set(predictions.get() + 1);
        pool.demand_vector_with_ledger(&demand, ledger)
    };
    let first = cache.prediction(&pool.ledger_path(), predict);
    let features = demand.memory_features.get().unwrap();
    for _ in 0..10 {
        let reused = cache.prediction(&pool.ledger_path(), predict);
        assert_eq!(reused.resources, first.resources);
        assert_eq!(reused.weight, first.weight);
        assert_eq!(reused.predicted, first.predicted);
        assert_eq!(reused.source, first.source);
    }
    assert_eq!(predictions.get(), 1);
    // Expiry with identical content recomputes once without changing the plan
    // or its admission-time calibration features.
    cache.loaded_at = Some(Instant::now() - LEDGER_CACHE_MAX_AGE);
    let same = cache.prediction(&pool.ledger_path(), predict);
    assert_eq!(predictions.get(), 2);
    assert_eq!(same.resources, first.resources);
    assert_eq!(same.weight, first.weight);
    assert_eq!(same.predicted, first.predicted);
    assert_eq!(same.source, first.source);
    assert_eq!(demand.memory_features.get().unwrap().mean, features.mean);
    assert_eq!(demand.memory_features.get().unwrap().scale, features.scale);
    cache.prediction(&pool.ledger_path(), predict);
    assert_eq!(predictions.get(), 2);
    // A changed ledger refreshes both the charge and the frozen denominator.
    ledger.memory_windows.insert(key.clone(), vec![10; 4]);
    crate::util::write_advisory(&pool.ledger_path(), &serde_json::to_vec(&ledger).unwrap())
        .unwrap();
    let changed = cache.prediction(&pool.ledger_path(), predict);
    assert_eq!(predictions.get(), 3);
    assert!(changed.resources.memory_bytes < first.resources.memory_bytes);
    assert_eq!(demand.memory_features.get().unwrap().mean, 10.0_f64.ln());
}

#[test]
fn repredicted_waiter_gate_logs_current_charge_and_dequeues_old_record() {
    let dir = tempfile::tempdir().unwrap();
    let mut pool = pool_at(dir.path(), 10, 100);
    pool.events = true;
    pool.clock = || 1_000;
    let demand = compile_demand("crate", &[]);
    let key = demand.compile_mode.as_ref().unwrap();
    let mut ledger = MemoryLedger::default();
    ledger.memory_windows.insert(key.clone(), vec![500; 4]);
    let plan = pool.demand_vector_with_ledger(&demand, &ledger);
    let held = pool.try_admit(9, None).unwrap().unwrap();
    let mut state = AdmissionState::new(100);
    assert!(
        pool.try_admit_state(
            plan.resources,
            plan.weight,
            plan.predicted,
            &mut state,
            Some(&demand)
        )
        .unwrap()
        .is_none()
    );
    let ticket = state.queued.as_ref().unwrap().ticket;
    ledger.memory_windows.insert(key.clone(), vec![10; 4]);
    let refreshed = pool.demand_vector_with_ledger(&demand, &ledger);
    assert!(refreshed.resources.memory_bytes < plan.resources.memory_bytes);
    pool.available_memory = || Some(0);
    for _ in 0..2 {
        assert!(
            pool.try_admit_state(
                refreshed.resources,
                refreshed.weight,
                refreshed.predicted,
                &mut state,
                Some(&demand)
            )
            .unwrap()
            .is_none()
        );
    }
    assert!(state.queued.is_none());
    let events: Vec<serde_json::Value> =
        std::fs::read_to_string(dir.path().join("decisions.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let gated: Vec<_> = events
        .iter()
        .filter(|event| event["event"] == "gated")
        .collect();
    assert_eq!(gated.len(), 1, "same-reason retry is suppressed");
    assert_eq!(gated[0]["memory_bytes"], refreshed.resources.memory_bytes);
    assert_eq!(gated[0]["first_arrival_ms"], 100);
    assert_eq!(gated[0]["reason"], "available-memory");
    let dequeued = events
        .iter()
        .find(|event| event["event"] == "dequeued")
        .unwrap();
    assert_eq!(dequeued["memory_bytes"], plan.resources.memory_bytes);
    assert_eq!(dequeued["ticket"], ticket);
    assert_eq!(dequeued["first_arrival_ms"], 100);
    assert_eq!(dequeued["waited_ms"], 900);
    drop(held);
}

#[test]
fn nextest_admission_reads_only_per_test_history() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 8, 100);
    pool.record_test_history("project/bin", "case", Some(350), Some(3))
        .unwrap();
    let demand = Demand::nextest("project/bin", "case");
    let mut cache = None;
    let plan = pool.admission_plan(&demand, &mut cache);
    assert!(cache.is_none());
    assert_eq!(
        plan.resources,
        Resources {
            cpu: 3,
            memory_bytes: 350
        }
    );
    LEDGER_READS.with(|reads| reads.set(0));
    let permit = pool.admit(&demand).unwrap();
    assert_eq!(LEDGER_READS.with(|reads| reads.get()), 0);
    let observation = demand.observation.get().unwrap();
    assert_eq!(observation.resources, plan.resources);
    assert_eq!(observation.source, "test-history");
    drop(permit);
    // Per-test history changes must be visible without a compiler-ledger reload.
    pool.record_test_history("project/bin", "case", Some(450), Some(4))
        .unwrap();
    let refreshed = pool.admission_plan(&demand, &mut cache);
    assert!(cache.is_none());
    assert_eq!(
        refreshed.resources,
        Resources {
            cpu: 4,
            memory_bytes: 450
        }
    );
}

#[test]
fn cpu_only_compile_admission_does_not_read_compiler_history() {
    let dir = tempfile::tempdir().unwrap();
    let pool = pool_at(dir.path(), 8, 0);
    let demand = compile_demand("crate", &[]);
    let mut cache = None;
    let plan = pool.admission_plan(&demand, &mut cache);
    assert!(cache.is_none());
    LEDGER_READS.with(|reads| reads.set(0));
    let permit = pool.admit(&demand).unwrap();
    assert_eq!(LEDGER_READS.with(|reads| reads.get()), 0);
    assert_eq!(demand.observation.get().unwrap().resources, plan.resources);
    assert_eq!(plan.resources.memory_bytes, 0);
    drop(permit);
}

#[test]
fn a_released_flight_can_still_publish() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = flight_at(dir.path(), "rustc", "release").unwrap();
    first.release();
    let second = flight_at(dir.path(), "rustc", "release").unwrap();
    assert!(!second.waited());
    first.leave("published after release");
    drop(second);
    assert_eq!(
        flight_at(dir.path(), "rustc", "release")
            .unwrap()
            .inherited(),
        Some("published after release")
    );
}

#[test]
fn an_unpublished_holder_does_not_release_its_waiter_on_a_miss() {
    for old_record in [None, Some("unchanged prediction")] {
        let dir = tempfile::tempdir().unwrap();
        if let Some(payload) = old_record {
            let previous = flight_at(dir.path(), "rustc", "unchanged").unwrap();
            previous.leave(payload);
        }
        let holder = flight_at(dir.path(), "rustc", "unchanged").unwrap();
        let mut waiter = flight_at_inner(
            dir.path(),
            "rustc",
            "unchanged",
            "crate",
            FlightWait::default(),
            || drop(holder),
            |_| {},
        )
        .unwrap();
        assert!(!waiter.published_while_waiting());
        waiter.finish_restore(false);
        let mut probe = fslock::LockFile::open(&waiter.path).unwrap();
        assert!(!probe.try_lock().unwrap());
    }
}

#[test]
fn a_flight_wait_warns_once() {
    let dir = tempfile::tempdir().unwrap();
    let holder = flight_at(dir.path(), "rustc", "notice").unwrap();
    let mut messages = Vec::new();
    // The notice is checked before each lock attempt, so a zero threshold
    // fires on the first pass whatever the scheduling, and the holder is gone
    // by then so the wait ends there.
    let waiter = flight_at_inner(
        dir.path(),
        "rustc",
        "notice",
        "example_crate",
        FlightWait {
            notice: Duration::ZERO,
            limit: None,
        },
        || drop(holder),
        |message| messages.push(message.to_owned()),
    )
    .unwrap();
    assert!(waiter.waited());
    assert_eq!(messages.len(), 1, "one notice: {messages:?}");
    assert!(messages[0].contains("example_crate"));
}

#[test]
fn a_flight_wait_times_out_without_a_phantom_holder() {
    let dir = tempfile::tempdir().unwrap();
    let holder = flight_at(dir.path(), "rustc", "timeout").unwrap();
    let mut messages = Vec::new();
    let result = flight_at_inner(
        dir.path(),
        "rustc",
        "timeout",
        "example_crate",
        FlightWait {
            notice: Duration::MAX,
            limit: Some(Duration::from_millis(35)),
        },
        || {},
        |message| messages.push(message.to_owned()),
    );
    assert!(result.is_err());
    assert_eq!(messages.len(), 1, "one timeout warning: {messages:?}");
    assert!(messages[0].contains("stopped waiting"));
    assert!(messages[0].contains("example_crate"));
    drop(holder);
    let next = flight_at(dir.path(), "rustc", "timeout").unwrap();
    assert!(!next.waited());
}

#[test]
fn flight_wait_limit_uses_the_previous_compiler_duration() {
    for (payload, expected) in [
        ("{}", 600),
        ("not json", 600),
        (r#"{"compiler_duration_ns":1000000000}"#, 600),
        (r#"{"compiler_duration_ns":200000000000}"#, 800),
        (r#"{"compiler_duration_ns":-1}"#, 600),
    ] {
        let record = serde_json::to_vec(&FlightRecord {
            version: FLIGHT_VERSION,
            adapter: "rustc".into(),
            invocation: "limit".into(),
            payload: payload.into(),
            attempt: None,
        })
        .unwrap();
        assert_eq!(
            flight_wait_limit(Some(&record)),
            Duration::from_secs(expected)
        );
    }
    assert_eq!(flight_wait_limit(None), FLIGHT_WAIT_MIN);
}

#[test]
fn a_holder_publishing_before_the_waiter_arrives_releases_it_on_a_miss() {
    for adapter in ["rustc", "cc"] {
        let dir = tempfile::tempdir().unwrap();
        let holder = flight_at(dir.path(), adapter, "early").unwrap();
        holder.leave("already published");
        let mut waiter = flight_at_inner(
            dir.path(),
            adapter,
            "early",
            "crate",
            FlightWait::default(),
            || drop(holder),
            |_| {},
        )
        .unwrap();
        assert!(waiter.waited());
        assert_eq!(waiter.inherited(), Some("already published"));
        assert!(waiter.published_while_waiting());
        waiter.finish_restore(false);
        let mut probe = fslock::LockFile::open(&waiter.path).unwrap();
        assert!(
            probe.try_lock().unwrap(),
            "{adapter} waiter must compile in parallel"
        );
    }
}

#[test]
fn legacy_predictions_do_not_release_waiters_on_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let holder = flight_at(dir.path(), "rustc", "legacy").unwrap();
    holder.leave("old format");
    let path = record_path(&holder.path);
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record.as_object_mut().unwrap().remove("attempt");
    std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    let mut waiter = flight_at_inner(
        dir.path(),
        "rustc",
        "legacy",
        "crate",
        FlightWait::default(),
        || drop(holder),
        |_| {},
    )
    .unwrap();
    assert_eq!(waiter.inherited(), Some("old format"));
    assert!(!waiter.published_while_waiting());
    waiter.finish_restore(false);
    let mut probe = fslock::LockFile::open(&waiter.path).unwrap();
    assert!(!probe.try_lock().unwrap());
}

#[test]
fn a_released_publish_is_not_attributed_to_the_current_holder() {
    let dir = tempfile::tempdir().unwrap();
    let mut released = flight_at(dir.path(), "cc", "late").unwrap();
    released.release();
    let holder = flight_at(dir.path(), "cc", "late").unwrap();
    released.leave("late publication from an earlier attempt");
    let mut waiter = flight_at_inner(
        dir.path(),
        "cc",
        "late",
        "file.c",
        FlightWait::default(),
        || drop(holder),
        |_| {},
    )
    .unwrap();
    assert!(waiter.inherited().is_some());
    assert!(!waiter.published_while_waiting());
    waiter.finish_restore(false);
    let mut probe = fslock::LockFile::open(&waiter.path).unwrap();
    assert!(!probe.try_lock().unwrap());
}
