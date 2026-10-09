//! End-to-end coverage for cargo-nextest's per-test scheduler integration.

#[cfg(unix)]
mod support;

#[cfg(unix)]
mod unix {
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::fs::{self, File};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::support;

    fn project(root: &Path, name: &str, library: &str) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = '{name}'\nversion = '0.0.0'\nedition = '2021'\n\n[workspace]\n"
            ),
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), library).unwrap();
    }

    fn mbx(root: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
        command.env_clear();
        for name in [
            "PATH",
            "HOME",
            "RUSTUP_HOME",
            "CARGO_HOME",
            "RUSTUP_TOOLCHAIN",
            "SystemRoot",
            "TEMP",
            "TMP",
            "TMPDIR",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        support::isolate_host(&mut command);
        command
            .current_dir(root)
            .env("CARGO_HOME", root.join("cargo-home"))
            .env("MBX_CACHE_DIR", root.join("cache"))
            .env("MBX_LINKER", "system")
            .env("CI", "1")
            .env("MBX_SUMMARY", "off");
        command
    }

    fn nextest(root: &Path, cache: &Path) -> Command {
        let mut command = mbx(root);
        command
            .env("MBX_CACHE_DIR", cache)
            .env("MBX_SCHEDULER", "1")
            .env("MBX_SCHEDULER_TESTS", "1")
            .env("MBX_SCHEDULER_CPUS", "2")
            .env("MBX_SCHEDULER_MEMORY", "none")
            .arg("nextest");
        command
    }

    fn nextest_available() -> bool {
        match Command::new("cargo")
            .args(["nextest", "--version"])
            .output()
        {
            Ok(output) if output.status.success() => true,
            Ok(output) => {
                eprintln!(
                    "skipping native nextest scheduling test: `cargo nextest --version` failed: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                false
            }
            Err(error) => {
                eprintln!(
                    "skipping native nextest scheduling test: `cargo nextest --version` is not runnable: {error}"
                );
                false
            }
        }
    }

    fn prepare_nextest(root: &Path, cache: &Path) {
        let output = nextest(root, cache)
            .env("MBX_SCHEDULER_TESTS", "0")
            .args(["run", "--offline", "--no-run"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture prebuild failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn output_text(output: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn wait_for(path: &Path, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("{} was not created within {timeout:?}", path.display());
    }

    fn wait_child(child: &mut Child, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child did not exit within {timeout:?}");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    struct LoggedChild {
        child: Child,
        stdout: PathBuf,
        stderr: PathBuf,
    }

    impl LoggedChild {
        fn spawn(command: &mut Command, root: &Path, name: &str) -> Self {
            let stdout = root.join(format!("{name}.stdout"));
            let stderr = root.join(format!("{name}.stderr"));
            command
                .stdout(Stdio::from(File::create(&stdout).unwrap()))
                .stderr(Stdio::from(File::create(&stderr).unwrap()));
            let child = command.spawn().unwrap();
            Self {
                child,
                stdout,
                stderr,
            }
        }

        fn wait(mut self, timeout: Duration) -> (ExitStatus, String) {
            let status = wait_child(&mut self.child, timeout);
            let stdout = fs::read_to_string(self.stdout).unwrap_or_default();
            let stderr = fs::read_to_string(self.stderr).unwrap_or_default();
            (status, format!("{stdout}{stderr}"))
        }
    }

    struct Reservation {
        child: Option<Child>,
        release: PathBuf,
    }

    impl Reservation {
        fn start(root: &Path, cache: &Path, cpus: u64, name: &str) -> Self {
            let ready = root.join(format!("{name}.ready"));
            let release = root.join(format!("{name}.release"));
            let cpus = cpus.to_string();
            let mut command = mbx(root);
            command
                .env("MBX_CACHE_DIR", cache)
                .env("MBX_SCHEDULER", "1")
                .env("MBX_SCHEDULER_CPUS", &cpus)
                .env("MBX_SCHEDULER_MEMORY", "none")
                .args([
                    "reserve",
                    "--cpus",
                    &cpus,
                    "--",
                    "sh",
                    "-c",
                    "touch \"$RESERVATION_READY\"; while [ ! -e \"$RESERVATION_RELEASE\" ]; do sleep 0.02; done",
                ])
                .env("RESERVATION_READY", &ready)
                .env("RESERVATION_RELEASE", &release)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let child = command.spawn().unwrap();
            wait_for(&ready, Duration::from_secs(5));
            Self {
                child: Some(child),
                release,
            }
        }

        fn release(&mut self) {
            fs::write(&self.release, "release").unwrap();
            let status = wait_child(self.child.as_mut().unwrap(), Duration::from_secs(5));
            assert!(status.success(), "reserve exited with {status}");
            self.child = None;
        }
    }

    impl Drop for Reservation {
        fn drop(&mut self) {
            if self.child.is_some() {
                let _ = fs::write(&self.release, "release");
                let mut child = self.child.take().unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while Instant::now() < deadline {
                    if child.try_wait().ok().flatten().is_some() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn lease_dir(cache: &Path) -> PathBuf {
        cache.join("scheduler/leases")
    }

    fn lease_files(cache: &Path) -> Vec<PathBuf> {
        fs::read_dir(lease_dir(cache))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_file())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn all_leases_are_lockable(cache: &Path) -> bool {
        locked_lease_files(cache).is_empty()
    }

    fn locked_lease_files(cache: &Path) -> Vec<PathBuf> {
        lease_files(cache)
            .into_iter()
            .filter(|path| {
                let Ok(file) = File::open(path) else {
                    return true;
                };
                // SAFETY: flock only borrows the live descriptor; it is unlocked
                // before the file is dropped.
                let locked =
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if locked != 0 {
                    return true;
                }
                // SAFETY: this releases the lock acquired above.
                (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) }) != 0
            })
            .collect()
    }

    fn assert_no_live_leases(cache: &Path, stage: &str) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if all_leases_are_lockable(cache) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let live = locked_lease_files(cache);
        panic!("scheduler leases are still locked {stage}: {live:?}");
    }

    fn timestamp() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn independent_nextest_invocations_share_capacity_and_allow_overlap() {
        if !nextest_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let cache = temp.path().join("shared-cache");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let source = r#"
#[allow(dead_code)]
fn event(kind: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::path::PathBuf::from(std::env::var_os("MBX_SCHEDULE_LOG").unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let name = format!("{}-{}-{kind}", env!("CARGO_PKG_NAME"), std::env::var("NEXTEST_TEST_NAME").unwrap());
    std::fs::write(dir.join(name), now.to_string()).unwrap();
}
#[allow(dead_code)]
fn work() { event("start"); std::thread::sleep(std::time::Duration::from_millis(650)); event("end"); }
#[test] fn one() { work(); }
#[test] fn two() { work(); }
#[test] fn three() { work(); }
#[test] fn four() { work(); }
"#;
        project(&first, "schedule-first", source);
        project(&second, "schedule-second", source);
        prepare_nextest(&first, &cache);
        prepare_nextest(&second, &cache);

        let log = temp.path().join("events.log");
        let mut a = nextest(&first, &cache);
        a.env("MBX_SCHEDULE_LOG", &log)
            .args(["run", "--offline", "--test-threads", "4"]);
        let mut b = nextest(&second, &cache);
        b.env("MBX_SCHEDULE_LOG", &log)
            .args(["run", "--offline", "--test-threads", "4"]);
        let a = a.spawn().unwrap();
        let b = b.spawn().unwrap();
        let a_output = a.wait_with_output().unwrap();
        let b_output = b.wait_with_output().unwrap();
        assert!(a_output.status.success(), "{}", output_text(&a_output));
        assert!(b_output.status.success(), "{}", output_text(&b_output));
        let overlap = max_overlap(&log);
        assert!(
            overlap > 1 && overlap <= 2,
            "scheduled overlap must exceed one without exceeding capacity: {overlap}"
        );

        fs::remove_dir_all(&log).unwrap();
        let mut a = nextest(&first, &cache);
        a.env("MBX_SCHEDULER_TESTS", "0")
            .env("MBX_SCHEDULE_LOG", &log)
            .args(["run", "--offline", "--test-threads", "4"]);
        let mut b = nextest(&second, &cache);
        b.env("MBX_SCHEDULER_TESTS", "0")
            .env("MBX_SCHEDULE_LOG", &log)
            .args(["run", "--offline", "--test-threads", "4"]);
        let a = a.spawn().unwrap();
        let b = b.spawn().unwrap();
        let a_output = a.wait_with_output().unwrap();
        let b_output = b.wait_with_output().unwrap();
        assert!(a_output.status.success(), "{}", output_text(&a_output));
        assert!(b_output.status.success(), "{}", output_text(&b_output));
        assert!(
            max_overlap(&log) > 2,
            "unscheduled control did not exceed capacity"
        );
    }

    fn max_overlap(path: &Path) -> usize {
        let mut events: Vec<(u128, bool)> = fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                let start = name.ends_with("-start");
                let time = fs::read_to_string(entry.path()).unwrap().parse().unwrap();
                (time, start)
            })
            .collect();
        events.sort_by_key(|(time, start)| (*time, *start));
        let mut active = 0usize;
        let mut peak = 0usize;
        for (_, start) in events {
            if start {
                active += 1;
                peak = peak.max(active);
            } else {
                active -= 1;
            }
        }
        peak
    }

    #[test]
    fn listing_never_waits_for_a_permit_but_execution_does() {
        if !nextest_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("fixture");
        fs::create_dir_all(&root).unwrap();
        let cache = temp.path().join("shared-cache");
        project(
            &root,
            "schedule-listing",
            r#"#[test] fn starts_after_release() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    std::fs::write(std::env::var_os("TEST_STARTED").unwrap(), now.to_string()).unwrap();
}"#,
        );
        prepare_nextest(&root, &cache);
        let mut reservation = Reservation::start(temp.path(), &cache, 2, "list-reservation");
        let mut list = nextest(&root, &cache);
        list.args(["list", "--offline"]);
        let started_listing = Instant::now();
        let (status, output) =
            LoggedChild::spawn(&mut list, temp.path(), "list").wait(Duration::from_secs(4));
        assert!(status.success(), "{output}");
        assert!(started_listing.elapsed() < Duration::from_secs(2));

        let started = temp.path().join("started");
        let release_time = temp.path().join("release-time");
        let mut run = nextest(&root, &cache);
        run.env("TEST_STARTED", &started).args([
            "run",
            "--offline",
            "-E",
            "test(starts_after_release)",
        ]);
        let running = LoggedChild::spawn(&mut run, temp.path(), "queued-run");
        thread::sleep(Duration::from_millis(2500));
        assert!(
            !started.exists(),
            "test started while reserve held the pool"
        );
        fs::write(&release_time, timestamp().to_string()).unwrap();
        reservation.release();
        let (status, output) = running.wait(Duration::from_secs(8));
        assert!(status.success(), "{output}");
        wait_for(&started, Duration::from_secs(2));
        let released: u128 = fs::read_to_string(release_time).unwrap().parse().unwrap();
        let start: u128 = fs::read_to_string(&started).unwrap().parse().unwrap();
        assert!(
            start >= released,
            "test started at {start} before release at {released}"
        );
    }

    #[test]
    fn nextest_history_is_per_test_and_cargo_test_keeps_suite_history() {
        if !nextest_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("fixture");
        fs::create_dir_all(root.join("tests")).unwrap();
        let cache = temp.path().join("shared-cache");
        let history_log = temp.path().join("observed.jsonl");
        project(
            &root,
            "schedule_identity",
            r#"
pub fn observe(name: &str, heavy: bool) {
    use std::{fs::OpenOptions, io::Write};
    let Ok(test) = std::env::var("NEXTEST_TEST_NAME") else { return };
    let binary = std::env::var("NEXTEST_BINARY_ID").unwrap();
    assert_eq!(std::env::var("NEXTEST_TEST_PHASE").unwrap(), "run");
    assert_eq!(std::env::var("MBX_SCHEDULER").unwrap(), "0");
    assert_eq!(std::env::var("MBX_SCHED_DIR").unwrap(), "");
    assert_eq!(std::env::var("MBX_CONTROL_CHILD").unwrap(), "1");
    let cache = std::path::PathBuf::from(std::env::var_os("MBX_CACHE_DIR").unwrap());
    let leases = std::fs::read_dir(cache.join("scheduler/leases")).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(leases.len(), 1, "only this test should hold a lease: {leases:?}");
    let lease = std::fs::read_to_string(leases[0].path()).unwrap();
    let weight = lease.split("\"weight\":").nth(1).unwrap().split(|ch: char| !ch.is_ascii_digit()).next().unwrap();
    let mut file = OpenOptions::new().create(true).append(true).open(std::env::var_os("MBX_TEST_WEIGHT_LOG").unwrap()).unwrap();
    writeln!(file, "{binary}\t{test}\t{name}\t{heavy}\t{weight}").unwrap();
}
#[cfg(test)] mod unit {
    #[test] fn unit_heavy() {
        super::observe("unit_heavy", true);
        let mut data = vec![0u8; 256 * 1024 * 1024];
        for page in data.iter_mut().step_by(4096) { *page = 1; }
        std::hint::black_box(&data);
        std::thread::sleep(std::time::Duration::from_millis(1200));
    }
    #[test] fn unit_light() {
        super::observe("unit_light", false);
        std::thread::sleep(std::time::Duration::from_millis(1200));
    }
}
"#,
        );
        fs::write(
            root.join("tests/it.rs"),
            r#"#[test] fn integration_light() {
    schedule_identity::observe("integration_light", false);
    std::thread::sleep(std::time::Duration::from_millis(1200));
}"#,
        )
        .unwrap();
        prepare_nextest(&root, &cache);
        let memory_path = cache.join("scheduler/memory.json");
        let suite_ledger_before = fs::read(&memory_path).ok();
        let first = nextest(&root, &cache)
            .env("MBX_SCHEDULER_CPUS", "1")
            .env("MBX_TEST_WEIGHT_LOG", &history_log)
            .args(["run", "--offline", "--test-threads", "1"])
            .output()
            .unwrap();
        assert!(first.status.success(), "{}", output_text(&first));

        let observations = read_observations(&history_log);
        let heavy_id = observation(&observations, "unit_heavy").binary.clone();
        let light_id = observation(&observations, "integration_light")
            .binary
            .clone();
        assert_ne!(
            heavy_id, light_id,
            "unit and integration targets need distinct binary IDs"
        );

        let histories = history_entries(&cache);
        let mut peaks = BTreeMap::new();
        for name in ["unit_heavy", "unit_light", "integration_light"] {
            let observation = observation(&observations, name);
            let binary_id = &observation.binary;
            let test = &observation.test;
            assert!(test.contains(name), "{test:?} does not identify {name}");
            let history = histories
                .iter()
                .find(|(file, entry)| {
                    let key = entry["binary"].as_str().unwrap_or_default();
                    file == &history_file_name(key)
                        && key.rsplit('/').next() == Some(binary_id.as_str())
                        && entry["test"] == test.as_str()
                })
                .unwrap_or_else(|| {
                    panic!("missing per-test history for {binary_id}/{test}: {histories:?}")
                });
            let binary_key = history.1["binary"].as_str().unwrap();
            assert_eq!(history.0, history_file_name(binary_key));
            let history = &history.1;
            peaks.insert(
                name,
                history["peak"].as_u64().expect("recorded peak memory"),
            );
            if root
                .ancestors()
                .any(|ancestor| ancestor.join(".git").exists())
            {
                let (repo_id, history_id) = binary_key
                    .split_once('/')
                    .expect("Git checkout history key must include repo identity");
                assert_eq!(repo_id.len(), 12, "repo identity is not 12 hex chars");
                assert!(repo_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
                assert_eq!(history_id, binary_id);
            } else {
                assert_eq!(binary_key, binary_id);
            }
            if name == "unit_heavy" {
                assert!(
                    history["peak"].as_u64().unwrap_or_default() > 200 * 1024 * 1024,
                    "{history}"
                );
            }
        }
        assert!(
            peaks["unit_heavy"] > peaks["unit_light"],
            "heavy peak {} did not exceed trivial peak {}",
            peaks["unit_heavy"],
            peaks["unit_light"]
        );
        for name in ["unit_heavy", "integration_light"] {
            let selected = nextest(&root, &cache)
                .env("MBX_SCHEDULER_CPUS", "8")
                .env("MBX_SCHEDULER_MEMORY", "512MiB")
                .env("MBX_TEST_WEIGHT_LOG", &history_log)
                .args(["run", "--offline", "-E", &format!("test({name})")])
                .output()
                .unwrap();
            assert!(selected.status.success(), "{}", output_text(&selected));
        }
        let observations = read_observations(&history_log);
        let heavy_weight = latest_weight(&observations, "unit_heavy");
        let light_weight = latest_weight(&observations, "integration_light");
        assert!(
            heavy_weight > light_weight,
            "heavy={heavy_weight}, light={light_weight}"
        );
        assert_eq!(
            suite_ledger_before,
            fs::read(&memory_path).ok(),
            "nextest runs must not update the suite ledger"
        );

        let before_suite = history_files(&cache);
        let suite = mbx(&root)
            .env("MBX_CACHE_DIR", &cache)
            .env("MBX_SCHEDULER", "1")
            .env("MBX_SCHEDULER_TESTS", "1")
            .env("MBX_SCHEDULER_CPUS", "8")
            .env("MBX_SCHEDULER_MEMORY", "none")
            .args(["test", "--offline"])
            .output()
            .unwrap();
        assert!(suite.status.success(), "{}", output_text(&suite));
        assert_eq!(
            before_suite,
            history_files(&cache),
            "cargo test must not update per-test history"
        );
        let ledger: Value = serde_json::from_slice(&fs::read(&memory_path).unwrap()).unwrap();
        assert!(
            ledger["cpus"]
                .as_object()
                .is_some_and(|cpus| cpus.keys().any(|key| key.contains("schedule_identity"))),
            "cargo test did not record suite CPU history: {ledger}"
        );
    }

    #[derive(Debug)]
    struct Observation {
        binary: String,
        test: String,
        name: String,
        weight: u64,
    }

    fn read_observations(path: &Path) -> Vec<Observation> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| {
                let mut fields = line.split('\t');
                let binary = fields.next().unwrap().to_owned();
                let test = fields.next().unwrap().to_owned();
                let name = fields.next().unwrap().to_owned();
                let _heavy = fields.next().unwrap().parse::<bool>().unwrap();
                Observation {
                    binary,
                    test,
                    name,
                    weight: fields.next().unwrap().parse().unwrap(),
                }
            })
            .collect()
    }

    fn observation<'a>(observations: &'a [Observation], name: &str) -> &'a Observation {
        observations
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("missing observation for {name}: {observations:?}"))
    }

    fn latest_weight(observations: &[Observation], name: &str) -> u64 {
        observations
            .iter()
            .rev()
            .find(|entry| entry.name == name)
            .unwrap()
            .weight
    }

    fn history_file_name(binary: &str) -> String {
        let digest = Sha256::digest(binary.as_bytes());
        format!("{}.jsonl", &hex::encode(digest)[..16])
    }

    fn history_entries(cache: &Path) -> Vec<(String, Value)> {
        let dir = cache.join("scheduler/tests");
        let mut entries = Vec::new();
        for file in fs::read_dir(dir).unwrap() {
            let file = file.unwrap().path();
            if file
                .extension()
                .is_none_or(|extension| extension != "jsonl")
            {
                continue;
            }
            for line in fs::read_to_string(&file).unwrap().lines() {
                entries.push((
                    file.file_name().unwrap().to_string_lossy().into_owned(),
                    serde_json::from_str(line).unwrap(),
                ));
            }
        }
        entries
    }

    fn history_files(cache: &Path) -> BTreeMap<String, Vec<u8>> {
        let dir = cache.join("scheduler/tests");
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| {
                        (
                            entry.file_name().to_string_lossy().into_owned(),
                            fs::read(entry.path()).unwrap(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn nextest_timeout_queue_and_signal_release_every_permit() {
        if !nextest_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("fixture");
        fs::create_dir_all(root.join(".config")).unwrap();
        let cache = temp.path().join("shared-cache");
        project(
            &root,
            "schedule_cancel",
            r#"
#[test] fn hangs() {
    if let Some(path) = std::env::var_os("TEST_STARTED") { std::fs::write(path, "started").unwrap(); }
    std::thread::sleep(std::time::Duration::from_secs(60));
}
#[test] fn signal_hold() {
    let marker = std::env::var_os("TEST_STARTED").unwrap();
    std::fs::write(marker, std::process::id().to_string()).unwrap();
    loop { std::thread::sleep(std::time::Duration::from_secs(1)); }
}
"#,
        );
        fs::write(
            root.join(".config/nextest.toml"),
            "[profile.default]\nslow-timeout = { period = '1s', terminate-after = 2 }\n",
        )
        .unwrap();
        prepare_nextest(&root, &cache);

        let mut timeout = nextest(&root, &cache);
        timeout
            .env("TEST_STARTED", temp.path().join("timeout-started"))
            .args(["run", "--offline", "-E", "test(hangs)"]);
        let timeout = LoggedChild::spawn(&mut timeout, temp.path(), "timeout-run");
        let (status, output) = timeout.wait(Duration::from_secs(8));
        assert!(
            !status.success(),
            "timed out test unexpectedly passed: {output}"
        );
        assert!(
            output.contains("TIMEOUT"),
            "missing nextest timeout report:\n{output}"
        );
        assert_no_live_leases(&cache, "after nextest timeout");

        let mut reservation = Reservation::start(temp.path(), &cache, 2, "queued-reservation");
        let queued_started = temp.path().join("queued-started");
        let mut queued = nextest(&root, &cache);
        queued
            .env("TEST_STARTED", &queued_started)
            .args(["run", "--offline", "-E", "test(hangs)"]);
        let queued = LoggedChild::spawn(&mut queued, temp.path(), "queued-timeout");
        let (status, output) = queued.wait(Duration::from_secs(8));
        assert!(
            !status.success(),
            "queued test unexpectedly passed: {output}"
        );
        assert!(
            !queued_started.exists(),
            "queued test executed despite the full pool"
        );
        assert!(
            output.contains("TIMEOUT"),
            "missing queued timeout report:\n{output}"
        );
        assert!(
            output.contains("waiting for machine-wide scheduler capacity"),
            "missing scheduler wait note:\n{output}"
        );
        reservation.release();
        assert_no_live_leases(&cache, "after queued timeout");

        fs::write(
            root.join(".config/nextest.toml"),
            "[profile.default]\nslow-timeout = { period = '30s', terminate-after = 2 }\n",
        )
        .unwrap();
        let signal_started = temp.path().join("signal-started");
        let mut signal = nextest(&root, &cache);
        signal.env("TEST_STARTED", &signal_started).args([
            "run",
            "--offline",
            "-E",
            "test(signal_hold)",
        ]);
        signal.process_group(0);
        let mut signal = signal.spawn().unwrap();
        wait_for(&signal_started, Duration::from_secs(5));
        let active_leases = locked_lease_files(&cache);
        assert_eq!(active_leases.len(), 1, "expected one active test lease");
        let test_runner = active_leases[0]
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split('-').next())
            .and_then(|pid| pid.parse::<i32>().ok())
            .expect("test lease filename should start with the runner PID");
        // Nextest runs each test command in its own process group. The command
        // is mbx-test-runner, which owns the lease while it waits on the test.
        // SAFETY: the lease confirms this runner is live and the group's PID
        // equals the command PID for Nextest's Unix process-per-test mode.
        let test_group = unsafe { libc::getpgid(test_runner) };
        assert_eq!(
            test_group, test_runner,
            "unexpected Nextest test process group"
        );
        let result = unsafe { libc::kill(-test_group, libc::SIGTERM) };
        assert_eq!(
            result, 0,
            "failed to send SIGTERM to the Nextest test group"
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        let status = loop {
            if let Some(status) = signal.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            thread::sleep(Duration::from_millis(20));
        };
        let lease_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < lease_deadline && !all_leases_are_lockable(&cache) {
            thread::sleep(Duration::from_millis(25));
        }
        let leases_clear_after_signal = all_leases_are_lockable(&cache);
        let live_after_signal = locked_lease_files(&cache);

        // If cancellation failed, terminate the isolated fixture groups after
        // saving the state the regression assertion needs to report.
        if status.is_none() || !leases_clear_after_signal {
            unsafe {
                libc::kill(-test_group, libc::SIGKILL);
                libc::kill(-(signal.id() as i32), libc::SIGKILL);
            }
            if let Ok(pid) = fs::read_to_string(&signal_started)
                .and_then(|pid| pid.trim().parse::<i32>().map_err(std::io::Error::other))
            {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            let _ = signal.kill();
            let _ = signal.wait();
        }
        let status = status.expect("SIGTERM did not stop nextest within four seconds");
        assert!(!status.success(), "SIGTERM did not interrupt nextest");
        assert!(
            leases_clear_after_signal,
            "SIGTERM left live scheduler leases: {live_after_signal:?}"
        );
    }

    #[test]
    fn configured_and_environment_runners_compose_and_filters_remain_intact() {
        if !nextest_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("fixture");
        let cache = temp.path().join("shared-cache");
        fs::create_dir_all(root.join(".cargo")).unwrap();
        let runner = root.join("runner.sh");
        fs::write(
            &runner,
            "#!/bin/sh\nmarker=$1\nshift\nif [ -n \"${RUNNER_LOG:-}\" ]; then\nprintf '%s %s %s\\n' \"${NEXTEST_TEST_PHASE:-none}\" \"$marker\" \"$*\" >> \"$RUNNER_LOG\"\nfi\nexec \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o755)).unwrap();
        let host = Command::new("rustc").args(["-vV"]).output().unwrap();
        assert!(host.status.success(), "rustc -vV failed");
        let host = String::from_utf8_lossy(&host.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap()
            .to_owned();
        fs::write(
            root.join(".cargo/config.toml"),
            format!(
                "[target.\"{host}\"]\nrunner = [{:?}, '--configured']\n",
                runner.to_string_lossy()
            ),
        )
        .unwrap();
        fs::create_dir_all(&root).unwrap();
        project(
            &root,
            "schedule_runner",
            r#"
fn record(name: &str) {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(std::env::var_os("TEST_EXECUTIONS").unwrap()).unwrap();
    use std::io::Write;
    writeln!(file, "{name}").unwrap();
    assert_eq!(std::env::var("MBX_SCHEDULER").unwrap(), "0");
    assert_eq!(std::env::var("MBX_SCHED_DIR").unwrap(), "");
    assert_eq!(std::env::var("MBX_CONTROL_CHILD").unwrap(), "1");
    assert_eq!(std::env::var("MBX_USER_VALUE").unwrap(), "reached");
}
#[test] fn selected() { record("selected"); }
#[test] fn environment_runner() { record("environment_runner"); }
#[test] fn filtered_out() { panic!("filter did not exclude this test"); }
#[test] fn failure_for_exit() { panic!("intentional nextest failure"); }
"#,
        );
        let runner_log = temp.path().join("runner.log");
        let executions = temp.path().join("executions.log");
        prepare_nextest(&root, &cache);
        fs::write(&runner_log, "").unwrap();
        fs::write(&executions, "").unwrap();
        let selected = nextest(&root, &cache)
            .env("RUNNER_LOG", &runner_log)
            .env("TEST_EXECUTIONS", &executions)
            .env("MBX_USER_VALUE", "reached")
            .args(["run", "--offline", "selected"])
            .output()
            .unwrap();
        assert!(selected.status.success(), "{}", output_text(&selected));
        assert_eq!(fs::read_to_string(&executions).unwrap(), "selected\n");
        let config_log = fs::read_to_string(&runner_log).unwrap();
        let invocations: Vec<_> = config_log.lines().collect();
        assert!(
            invocations.iter().any(|line| {
                line.starts_with("list --configured ")
                    && line.contains("--list")
                    && line.contains("--format")
                    && line.contains("terse")
            }),
            "configured runner missed or corrupted nextest listing args:\n{config_log}"
        );
        assert!(
            invocations.iter().any(|line| {
                line.starts_with("run --configured ")
                    && line.contains("--exact selected --nocapture")
            }),
            "configured runner missed or corrupted execution args:\n{config_log}"
        );
        assert!(!config_log.contains("--exact filtered_out"), "{config_log}");

        let host_key = format!(
            "CARGO_TARGET_{}_RUNNER",
            host.replace('-', "_").to_ascii_uppercase()
        );
        let mut environment_runner = nextest(&root, &cache);
        environment_runner
            .env("RUNNER_LOG", &runner_log)
            .env("TEST_EXECUTIONS", &executions)
            .env("MBX_USER_VALUE", "reached")
            .env(host_key, format!("{} --environment", runner.display()))
            .args(["run", "--offline", "-E", "test(environment_runner)"]);
        let environment_runner = environment_runner.output().unwrap();
        assert!(
            environment_runner.status.success(),
            "{}",
            output_text(&environment_runner)
        );
        let environment_log = fs::read_to_string(&runner_log).unwrap();
        assert!(
            environment_log.lines().any(|line| {
                line.starts_with("run --environment ")
                    && line.contains("--exact environment_runner --nocapture")
            }),
            "environment runner did not replace the configured runner:\n{environment_log}"
        );
        assert_eq!(
            fs::read_to_string(&executions).unwrap(),
            "selected\nenvironment_runner\n"
        );

        let failing = nextest(&root, &cache)
            .env("RUNNER_LOG", &runner_log)
            .env("TEST_EXECUTIONS", &executions)
            .env("MBX_USER_VALUE", "reached")
            .args(["run", "--offline", "-E", "test(failure_for_exit)"])
            .output()
            .unwrap();
        let failure = output_text(&failing);
        assert!(!failing.status.success(), "failing test passed:\n{failure}");
        assert!(
            failure.contains("FAIL"),
            "missing nextest FAIL line:\n{failure}"
        );
        assert!(
            failure.contains("failure_for_exit"),
            "failure does not name the test:\n{failure}"
        );
        let log = fs::read_to_string(&runner_log).unwrap();
        assert!(
            log.contains("--exact failure_for_exit --nocapture"),
            "{log}"
        );
    }
}
