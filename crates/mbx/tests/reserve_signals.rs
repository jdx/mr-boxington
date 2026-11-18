//! A signal sent to a whole process group must not release a reservation
//! while the reserved command is still stopping.

#[cfg(unix)]
mod support;

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::path::Path;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::support;

    /// Traps the signals, announces it is stopping, and only exits once told.
    const SLOW_STOP: &str = r#"
trap 'touch "$1/stopping"; while [ ! -e "$1/finish" ]; do sleep 0.05; done; exit 0' HUP INT QUIT TERM
touch "$1/ready"
while :; do sleep 0.05; done
"#;

    fn mbx(root: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mbx"));
        command.env_clear();
        for name in ["PATH", "TMPDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        support::isolate_host(&mut command);
        command
            .current_dir(root)
            .env("MBX_CACHE_DIR", root.join("cache"))
            .env("MBX_SCHEDULER", "1")
            .env("MBX_SCHEDULER_CPUS", "2")
            .env("MBX_SCHEDULER_MEMORY", "none")
            .env("MBX_SCHEDULER_PRESSURE", "0")
            .env("MBX_GC_AUTO", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// A child in its own process group. Dropping it kills every process left
    /// in the group, including a worker and command that outlive the invoker,
    /// and reaps the invoker, so a failed assertion leaves nothing running.
    struct Group(Child);

    impl Group {
        fn spawn(command: &mut Command) -> Self {
            Self(command.process_group(0).spawn().unwrap())
        }

        fn signal(&self, signal: i32) {
            unsafe { libc::kill(-(self.0.id() as i32), signal) };
        }
    }

    impl Drop for Group {
        fn drop(&mut self) {
            self.signal(libc::SIGKILL);
            let _ = self.0.wait();
        }
    }

    impl std::ops::Deref for Group {
        type Target = Child;
        fn deref(&self) -> &Child {
            &self.0
        }
    }

    impl std::ops::DerefMut for Group {
        fn deref_mut(&mut self) -> &mut Child {
            &mut self.0
        }
    }

    /// The scheduler touches `priority-wait` after a failed admission attempt,
    /// which is the evidence that a reservation reached the capacity wait.
    fn stamps(root: &Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![root.join("cache")];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.file_name().is_some_and(|name| name == "priority-wait") {
                    found.push(path);
                }
            }
        }
        found
    }

    fn clear_stamps(root: &Path) {
        for stamp in stamps(root) {
            let _ = fs::remove_file(stamp);
        }
    }

    fn wait_for_queue(root: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while stamps(root).is_empty() {
            assert!(
                Instant::now() < deadline,
                "the reservation never reached the capacity wait"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("{} was not created", path.display());
    }

    fn wait_child(child: &mut Child) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child did not exit");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether a second whole-budget reservation is admitted. `false` means it
    /// was seen waiting for capacity, not merely that it was slow to start.
    fn probe_admitted(root: &Path) -> bool {
        let marker = root.join("probe");
        let _ = fs::remove_file(&marker);
        clear_stamps(root);
        let _probe = Group::spawn(
            mbx(root)
                .args(["reserve", "--cpus", "2", "--", "touch"])
                .arg(&marker),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if marker.exists() {
                return true;
            }
            if !stamps(root).is_empty() {
                // Queued. Give a wrongly released lease time to admit it.
                thread::sleep(Duration::from_millis(500));
                return marker.exists();
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the probe neither ran nor reached the capacity wait");
    }

    #[test]
    fn group_signals_keep_the_lease_until_the_command_exits() {
        for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let mut job = Group::spawn(
                mbx(root)
                    .args(["reserve", "--cpus", "2", "--", "sh", "-c", SLOW_STOP, "sh"])
                    .arg(root),
            );
            wait_for(&root.join("ready"));

            job.signal(signal);
            let status = wait_child(&mut job);
            assert_eq!(
                status.signal(),
                Some(signal),
                "signal {signal}: the invoker ends by the signal, got {status}"
            );
            wait_for(&root.join("stopping"));
            assert!(
                !probe_admitted(root),
                "signal {signal}: the lease was released while the command was stopping"
            );

            fs::write(root.join("finish"), "").unwrap();
            // Once the command is gone the lease is free.
            let deadline = Instant::now() + Duration::from_secs(10);
            while !probe_admitted(root) {
                assert!(
                    Instant::now() < deadline,
                    "signal {signal}: the lease was never released"
                );
            }
        }
    }

    #[test]
    fn a_signal_the_command_sends_itself_still_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let status = mbx(dir.path())
            .args(["reserve", "--cpus", "1", "--", "sh", "-c", "kill -TERM $$"])
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(143));
    }

    #[test]
    fn an_ignored_signal_stays_ignored_in_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // `nohup`-style: HUP is ignored before mbx starts, so it must stay
        // ignored in the command.
        let status = Command::new("sh")
            .arg("-c")
            .arg("trap '' HUP; exec \"$@\"")
            .arg("sh")
            .arg(env!("CARGO_BIN_EXE_mbx"))
            .args(["reserve", "--cpus", "1", "--", "sh", "-c"])
            .arg("kill -HUP $$; touch \"$0/survived\"")
            .arg(root)
            .env_clear()
            .envs(
                mbx(root)
                    .get_envs()
                    .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned()))),
            )
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "got {status}");
        assert!(root.join("survived").exists());
    }

    #[test]
    fn a_signal_while_waiting_for_capacity_ends_at_once_without_running() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let holder = Group::spawn(
            mbx(root)
                .args(["reserve", "--cpus", "2", "--", "sh", "-c", SLOW_STOP, "sh"])
                .arg(root),
        );
        wait_for(&root.join("ready"));

        clear_stamps(root);
        let mut waiter = Group::spawn(
            mbx(root)
                .args(["reserve", "--cpus", "2", "--", "touch"])
                .arg(root.join("ran")),
        );
        wait_for_queue(root);
        waiter.signal(libc::SIGTERM);
        let status = wait_child(&mut waiter);
        assert_eq!(status.signal(), Some(libc::SIGTERM), "got {status}");
        // The waiter's own worker must be gone too, or it would run the
        // command once the holder lets go.
        drop(holder);
        thread::sleep(Duration::from_millis(500));
        assert!(!root.join("ran").exists());
    }
}
