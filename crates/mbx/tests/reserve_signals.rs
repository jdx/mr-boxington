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
                panic!("child did not exit");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether a second whole-budget reservation is admitted within a moment.
    fn probe_admitted(root: &Path) -> bool {
        let marker = root.join("probe");
        let _ = fs::remove_file(&marker);
        let mut probe = mbx(root)
            .args(["reserve", "--cpus", "2", "--", "touch"])
            .arg(&marker)
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut admitted = false;
        while Instant::now() < deadline {
            if marker.exists() {
                admitted = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        if !admitted {
            // Still queued behind the lease; do not leave it waiting.
            unsafe { libc::kill(-(probe.id() as i32), libc::SIGKILL) };
        }
        let _ = probe.wait();
        admitted
    }

    #[test]
    fn group_signals_keep_the_lease_until_the_command_exits() {
        for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let mut job = mbx(root)
                .args(["reserve", "--cpus", "2", "--", "sh", "-c", SLOW_STOP, "sh"])
                .arg(root)
                .process_group(0)
                .spawn()
                .unwrap();
            wait_for(&root.join("ready"));

            unsafe { libc::kill(-(job.id() as i32), signal) };
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
        let mut holder = mbx(root)
            .args(["reserve", "--cpus", "2", "--", "sh", "-c", SLOW_STOP, "sh"])
            .arg(root)
            .process_group(0)
            .spawn()
            .unwrap();
        wait_for(&root.join("ready"));

        let mut waiter = mbx(root)
            .args(["reserve", "--cpus", "2", "--", "touch"])
            .arg(root.join("ran"))
            .process_group(0)
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(500));
        unsafe { libc::kill(-(waiter.id() as i32), libc::SIGTERM) };
        let status = wait_child(&mut waiter);
        assert_eq!(status.signal(), Some(libc::SIGTERM), "got {status}");
        assert!(!root.join("ran").exists());

        unsafe { libc::kill(-(holder.id() as i32), libc::SIGKILL) };
        let _ = holder.wait();
    }
}
