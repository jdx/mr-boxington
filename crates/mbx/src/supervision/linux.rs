use eyre::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(250);
const TIMEOUT: u64 = 3_000;

#[derive(Serialize, Deserialize)]
struct Heartbeat {
    generation: String,
    time: u64,
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
fn write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    crate::util::write_advisory(path, &serde_json::to_vec(value)?)
}
fn token(s: &str) -> bool {
    s.len() == 24 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn fresh(time: u64) -> bool {
    crate::pressure::now_ms()
        .checked_sub(time)
        .is_some_and(|age| age < TIMEOUT)
}
fn helper(
    role: &str,
    state: &Path,
    group: &Path,
    generation: Option<&str>,
) -> Result<std::process::Child> {
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("__mbx-control").arg(role).arg(state).arg(group);
    if let Some(generation) = generation {
        command.arg(generation);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // A terminal's cancellation targets the compiler/shim, not its recovery worker.
    command.process_group(0);
    Ok(command.spawn()?)
}

pub(super) fn prepare(command: &mut Command, pool: &Path, root: &Path) -> Result<Action> {
    if !root.is_absolute() {
        bail!("delegated cgroup root must be absolute");
    }
    let root = root.canonicalize()?;
    if !root.join("cgroup.controllers").is_file() {
        bail!("not a cgroup v2 directory");
    }
    let pool = pool.canonicalize()?;
    let pool_key = super::key(pool.as_os_str().as_encoded_bytes());
    let root_key = super::key(root.as_os_str().as_encoded_bytes());
    let group = root.join(format!("mbx-{}", &pool_key[..24]));
    std::fs::create_dir_all(&group)?;
    enable_memory(&group)?;
    let state = pool.join(format!("supervision-{}", &root_key[..24]));
    std::fs::create_dir_all(&state)?;
    // Serialize launches; the helper also holds a lifetime election lock.
    let mut launch = fslock::LockFile::open(&state.join("launch.lock"))?;
    let startup = Instant::now();
    while !launch.try_lock()? {
        if startup.elapsed() > Duration::from_secs(2) {
            bail!("supervisor startup is busy");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let current = || {
        read::<Heartbeat>(&state.join("current.json")).filter(|h| {
            token(&h.generation)
                && fresh(h.time)
                && !state.join(&h.generation).join("disabled").exists()
                && read::<u64>(&state.join(&h.generation).join("watchdog.json")).is_some_and(fresh)
        })
    };
    let heartbeat = if let Some(current) = current() {
        current
    } else {
        let mut child = helper("supervisor", &state, &group, None)?;
        let start = Instant::now();
        loop {
            if let Some(current) = current() {
                // Reap if it exits during this shim's life without blocking compiler startup.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                break current;
            }
            if child.try_wait()?.is_some() || start.elapsed() > Duration::from_secs(2) {
                bail!("supervisor did not become ready");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    drop(launch);
    let registry = state.join(&heartbeat.generation);
    let id = super::key(crate::util::random_string(32).as_bytes())[..24].to_owned();
    let path = group.join(&heartbeat.generation).join(&id);
    let lease_path = registry.join(format!("{id}.lease"));
    let mut lease = fslock::LockFile::open(&lease_path)?;
    lease.lock()?;
    std::fs::create_dir(&path)?;
    let membership = std::fs::OpenOptions::new()
        .write(true)
        .open(path.join("cgroup.procs"))?;
    let freeze = std::fs::OpenOptions::new()
        .write(true)
        .open(path.join("cgroup.freeze"))?;
    drop(freeze);
    let action = Action {
        registry,
        path,
        id,
        _lease: lease,
    };
    command.env("MBX_CONTROL_CHILD", "1");
    // SAFETY: only an async-signal-safe write is performed between fork and exec.
    // The inherited descriptor pins this cgroup; no PID lookup or path allocation
    // occurs in the child. Writing zero moves the calling process before exec.
    unsafe {
        command.pre_exec(move || {
            // If delegation disappears during startup, run without supervision.
            // `started` will not register an empty group with the controller.
            libc::write(membership.as_raw_fd(), b"0".as_ptr().cast(), 1);
            Ok(())
        });
    }
    Ok(action)
}

pub(crate) struct Action {
    registry: PathBuf,
    path: PathBuf,
    id: String,
    _lease: fslock::LockFile,
}
impl Action {
    /// Publish only after spawn's exec handshake. Freezing before then could
    /// block spawn itself and strand the supervisor's view of live work.
    pub(crate) fn started(&mut self) {
        if !populated(&self.path) {
            return;
        }
        let _ = write(
            &self.registry.join(format!("{}.json", self.id)),
            &crate::pressure::now_ms(),
        );
    }
}
impl Drop for Action {
    fn drop(&mut self) {
        let _ = thaw(&self.path);
        let _ = std::fs::remove_file(self.registry.join(format!("{}.json", self.id)));
        // All processes normally exited with the compiler. Cancellation may
        // leave descendants: thaw first, then terminate this owned tree only.
        if populated(&self.path) {
            let _ = std::fs::write(self.path.join("cgroup.kill"), "1");
        }
        let _ = std::fs::remove_dir(&self.path);
        let _ = std::fs::remove_file(self.registry.join(format!("{}.lease", self.id)));
    }
}
fn enable_memory(group: &Path) -> Result<()> {
    let controllers = std::fs::read_to_string(group.join("cgroup.controllers"))?;
    if controllers.split_whitespace().any(|name| name == "memory") {
        std::fs::write(group.join("cgroup.subtree_control"), "+memory")?;
    }
    Ok(())
}
fn populated(group: &Path) -> bool {
    std::fs::read_to_string(group.join("cgroup.events"))
        .is_ok_and(|s| s.lines().any(|l| l == "populated 1"))
}
fn thaw(group: &Path) -> std::io::Result<()> {
    std::fs::write(group.join("cgroup.freeze"), "0")
}
fn groups(group: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(group)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            (entry.file_type().ok()?.is_dir() && token(entry.file_name().to_str()?))
                .then(|| entry.path())
        })
        .collect()
}
fn thaw_all(group: &Path) {
    for path in groups(group) {
        let _ = thaw(&path);
    }
}
fn clean_orphans(registry: &Path, group: &Path) -> Result<usize> {
    let mut live = 0;
    for path in groups(group) {
        let id = path.file_name().unwrap().to_string_lossy();
        let lease_path = registry.join(format!("{id}.lease"));
        let mut lease = fslock::LockFile::open(&lease_path)?;
        if !lease.try_lock()? {
            live += 1;
            continue;
        }
        thaw(&path)?;
        if populated(&path) {
            std::fs::write(path.join("cgroup.kill"), "1")?;
        }
        let _ = std::fs::remove_dir(&path);
        let _ = std::fs::remove_file(registry.join(format!("{id}.json")));
        // Keep the locked inode until this scan finishes; action IDs never repeat.
        let _ = std::fs::remove_file(lease_path);
    }
    Ok(live)
}

pub(super) fn dispatch() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(2).collect();
    if args.len() < 3 {
        bail!("invalid control arguments");
    }
    let state = Path::new(&args[1]);
    let group = Path::new(&args[2]);
    if args[0] == "supervisor" {
        supervisor(state, group)
    } else if args[0] == "watchdog" {
        let generation = args
            .get(3)
            .and_then(|s| s.to_str())
            .filter(|s| token(s))
            .ok_or_else(|| eyre::eyre!("invalid generation"))?;
        watchdog(state, group, generation)
    } else {
        bail!("unknown control role")
    }
}

fn supervisor(state: &Path, group: &Path) -> Result<()> {
    let mut election = fslock::LockFile::open(&state.join("supervisor.lock"))?;
    if !election.try_lock()? {
        return Ok(());
    }
    let generation = super::key(crate::util::random_string(32).as_bytes())[..24].to_owned();
    let registry = state.join(&generation);
    let actions = group.join(&generation);
    std::fs::create_dir_all(&registry)?;
    std::fs::create_dir_all(&actions)?;
    enable_memory(&actions)?;
    let mut watchdog = helper("watchdog", state, group, Some(&generation))?;
    let start = Instant::now();
    while !read::<u64>(&registry.join("watchdog.json")).is_some_and(fresh) {
        if watchdog.try_wait()?.is_some() || start.elapsed() > Duration::from_secs(2) {
            bail!("watchdog did not become ready");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = supervise(state, &registry, &actions, &generation);
    let _ = std::fs::write(registry.join("disabled"), "supervisor exited");
    thaw_all(&actions);
    let _ = std::fs::write(registry.join("finished"), "1");
    let _ = watchdog.wait();
    result
}
fn supervise(state: &Path, registry: &Path, actions: &Path, generation: &str) -> Result<()> {
    let mut idle = Instant::now();
    loop {
        if registry.join("disabled").exists() {
            thaw_all(actions);
            return Ok(());
        }
        if !read::<u64>(&registry.join("watchdog.json")).is_some_and(fresh) {
            bail!("watchdog heartbeat lost");
        }
        write(
            &state.join("current.json"),
            &Heartbeat {
                generation: generation.to_owned(),
                time: crate::pressure::now_ms(),
            },
        )?;
        if clean_orphans(registry, actions)? > 0 {
            idle = Instant::now();
        } else if idle.elapsed() > Duration::from_secs(5) {
            return Ok(());
        }
        std::thread::sleep(TICK);
    }
}
fn watchdog(state: &Path, group: &Path, generation: &str) -> Result<()> {
    let registry = state.join(generation);
    let actions = group.join(generation);
    let start = Instant::now();
    loop {
        write(&registry.join("watchdog.json"), &crate::pressure::now_ms())
            .context("watchdog heartbeat")?;
        let heartbeat = read::<Heartbeat>(&state.join("current.json"));
        let healthy = heartbeat
            .as_ref()
            .is_some_and(|h| h.generation == generation && fresh(h.time));
        if registry.join("finished").exists() {
            thaw_all(&actions);
            return Ok(());
        }
        if !healthy && start.elapsed() > Duration::from_millis(TIMEOUT) {
            std::fs::write(registry.join("disabled"), "supervisor heartbeat lost")?;
            // Repeat while a stalled supervisor might wake up in an actuation.
            // No election/registrar lock is needed to rescue a frozen action.
            thaw_all(&actions);
            let _ = clean_orphans(&registry, &actions);
            let mut election = fslock::LockFile::open(&state.join("supervisor.lock"))?;
            if election.try_lock()? {
                return Ok(());
            }
        }
        std::thread::sleep(TICK);
    }
}
