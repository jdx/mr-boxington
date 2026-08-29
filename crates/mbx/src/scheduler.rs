//! Machine-wide scheduling of real compiler processes.
//!
//! Cargo plans one build at a time: three simultaneous worktree builds each
//! believe they own the machine and multiply `-j`. The shims sit in front of
//! every expensive process across all of those builds at once, so this module
//! coordinates what Cargo cannot -- a permit pool under the cache directory
//! that real compilations draw from. Cache hits never touch it, and Cargo
//! keeps its own dependency scheduling; only the processes that actually cost
//! CPU and memory wait their turn.
//!
//! The pool is a directory of *lease* files, one per admitted compilation,
//! each held under an OS file lock for the life of the compile. The kernel
//! releases those locks when their holder dies, so a killed shim can never
//! leak capacity; a lease whose lock can be taken is stale and is reclaimed
//! by the next admission scan. Admission itself serializes on one registrar
//! lock, held only long enough to scan and grant.
//!
//! Weights make links and historically memory-heavy crates cost more than one
//! permit: the sum of admitted weights never exceeds the capacity, so the
//! *predicted* memory of everything running stays inside the configured
//! budget. Prediction comes from a small ledger of peak RSS per crate name,
//! measured after every real compile on Unix. Two things remain out of reach
//! and are accepted: a single compilation larger than the machine will still
//! be too large when it runs alone, and a simultaneous pile-up of
//! never-measured crates can overshoot before the ledger has learned them.
//!
//! Concurrent sessions may be configured with different capacities; each
//! enforces its own bound against the same leases, so the loosest
//! configuration wins and nothing deadlocks. Every failure here degrades to
//! compiling without a permit -- the pool is a courtesy, never a dependency.
//!
//! This sits above Cargo's jobserver rather than beside it. A shim waiting
//! for a permit is holding the implicit jobserver token Cargo gave it, so the
//! two systems do hold each other's resources -- but they cannot deadlock,
//! because rustc never needs a *second* token to finish: it keeps the
//! implicit one and falls back to compiling its codegen units on fewer
//! threads. The worst case is a permit holder running single-threaded, which
//! costs time and always ends.

use crate::config::{Config, SchedulerPriority};
use eyre::{Context, Result};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Directory under the cache root holding the pool.
const SCHEDULER_DIR: &str = "scheduler";
/// Subdirectory holding one locked lease file per admitted compilation.
const LEASES_DIR: &str = "leases";
/// Registrar lock serializing admission scans.
const POOL_LOCK: &str = "pool.lock";
/// Stamp a waiting normal-priority build touches so low-priority builds yield.
const PRIORITY_WAIT_STAMP: &str = "priority-wait";
/// Ledger of peak compiler RSS by crate name.
const LEDGER_FILE: &str = "memory.json";
/// Lock serializing ledger updates.
const LEDGER_LOCK: &str = "memory.lock";

/// Where the pool lives; empty means the session turned scheduling off.
pub(crate) const SCHED_DIR_ENV: &str = "MBX_SCHED_DIR";
/// Machine-wide concurrent compile permits.
pub(crate) const SCHED_SLOTS_ENV: &str = "MBX_SCHED_SLOTS";
/// Memory one permit stands for; zero disables memory weighting.
pub(crate) const SCHED_SLOT_BYTES_ENV: &str = "MBX_SCHED_SLOT_BYTES";
/// Permit priority of this build's compilations.
pub(crate) const SCHED_PRIORITY_ENV: &str = "MBX_SCHED_PRIORITY";

/// Schema version of one lease record.
const LEASE_VERSION: u8 = 1;
/// Schema version of the peak-RSS ledger.
const LEDGER_VERSION: u8 = 1;
/// Most crates the ledger retains; the smallest are dropped first.
const MAX_LEDGER_ENTRIES: usize = 1024;
/// Permits a native link costs before it has ever been measured.
///
/// Links are the out-of-memory class -- rust-lang/cargo#12912 is about little
/// else -- so they start heavy rather than waiting for history to say so.
const LINK_WEIGHT: u64 = 2;
/// First delay after a refused admission.
const POLL_INITIAL: Duration = Duration::from_millis(2);
/// Longest delay between admission attempts.
///
/// A permit is released the instant its compiler exits and nothing wakes the
/// waiters, so this bounds how long a core sits idle with work queued for
/// it. Kept short for that reason: one attempt is a readdir and a handful of
/// `try_lock`s, which even a full pool of waiters can afford at this rate.
const POLL_MAX: Duration = Duration::from_millis(25);
/// How recently the stamp must have been touched to hold low priority back.
const PRIORITY_WAIT_FRESHNESS: Duration = Duration::from_secs(2);
/// How often one waiter refreshes that stamp, well inside its freshness.
const PRIORITY_STAMP_INTERVAL: Duration = Duration::from_millis(500);
/// How long one compilation waits before saying so in the debug log.
const SLOW_WAIT_NOTICE: Duration = Duration::from_secs(60);
/// How long the available-memory gate may defer one compilation.
///
/// The gate exists to avoid piling predicted-heavy work onto a machine that is
/// already short, but a machine that stays short must not starve a build
/// forever: past this deadline the permit arithmetic alone decides.
const GATE_DEADLINE: Duration = Duration::from_secs(120);

/// Distinguishes leases created by one process, which may hold several.
static LEASE_NONCE: AtomicU64 = AtomicU64::new(0);

/// What one compilation asks the pool for.
pub(crate) struct Demand {
    /// Compiler crate name, keying the peak-RSS ledger.
    name: String,
    /// Whether the invocation links a native program.
    links: bool,
}

impl Demand {
    pub(crate) fn new(name: &str, links: bool) -> Self {
        Self {
            name: name.to_string(),
            links,
        }
    }
}

/// One admitted compilation's capacity, released on drop or process death.
pub(crate) struct Permit {
    /// Taken before the file is removed. Windows refuses to delete a file
    /// anyone still holds open, so closing the handle -- not merely unlocking
    /// it -- is what lets the lease go away rather than accumulate.
    lock: Option<fslock::LockFile>,
    path: PathBuf,
}

impl Drop for Permit {
    fn drop(&mut self) {
        // Between the close and the removal this lease reads as stale to
        // anyone scanning, which is exactly what it is: its holder has
        // finished. The worst a concurrent scan does is remove it first.
        drop(self.lock.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One lease record, stored beside the lock that proves its holder is alive.
#[derive(Debug, Serialize, Deserialize)]
struct Lease {
    version: u8,
    weight: u64,
    priority: String,
}

/// Peak compiler RSS by crate name, machine-wide.
///
/// Read the way the savings tally is: anything unreadable is an empty ledger,
/// because refusing to schedule over a corrupt file would help nobody.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MemoryLedger {
    version: u8,
    crates: BTreeMap<String, u64>,
}

/// The machine-wide permit pool one process draws from.
pub(crate) struct Pool {
    dir: PathBuf,
    capacity: u64,
    /// Memory one permit stands for. Zero turns memory weighting and the
    /// available-memory gate off, leaving plain CPU permits.
    bytes_per_permit: u64,
    priority: SchedulerPriority,
    /// Probe for currently available memory, injectable for tests.
    available_memory: fn() -> Option<u64>,
}

/// The pool this process schedules against, or `None` when scheduling is off.
///
/// Resolved once: from the session's environment when one is running, and from
/// configuration for a persistent wrapper that has no session. Resolution
/// failures turn scheduling off rather than failing the compilation.
pub(crate) fn pool() -> Option<&'static Pool> {
    static POOL: OnceLock<Option<Pool>> = OnceLock::new();
    POOL.get_or_init(resolve_pool).as_ref()
}

fn resolve_pool() -> Option<Pool> {
    match std::env::var_os(SCHED_DIR_ENV) {
        Some(dir) if dir.is_empty() => None,
        Some(dir) => Some(Pool::new(
            PathBuf::from(dir),
            env_number(SCHED_SLOTS_ENV).unwrap_or_else(default_capacity),
            env_number(SCHED_SLOT_BYTES_ENV).unwrap_or(0),
            std::env::var(SCHED_PRIORITY_ENV)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
        )),
        None => match Config::load() {
            Ok(config) => Pool::from_config(&config),
            Err(error) => {
                debug!("compile scheduling is off; configuration did not load: {error:#}");
                None
            }
        },
    }
}

fn env_number(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}

fn default_capacity() -> u64 {
    std::thread::available_parallelism().map_or(1, |cpus| cpus.get() as u64)
}

/// The environment a session hands its shims so they schedule consistently.
///
/// An explicit empty directory means "off", stated rather than omitted for the
/// reason `VERIFY_ENV` always is: an absent key would leave a shim resolving
/// configuration on its own, which is the standalone path, not a disabled one.
pub(crate) fn session_environment(config: &Config) -> Vec<(String, String)> {
    let scheduler = &config.scheduler;
    if !scheduler.enabled {
        return vec![(SCHED_DIR_ENV.into(), String::new())];
    }
    vec![
        (
            SCHED_DIR_ENV.into(),
            config
                .cache_dir
                .join(SCHEDULER_DIR)
                .to_string_lossy()
                .into_owned(),
        ),
        (SCHED_SLOTS_ENV.into(), scheduler.cpus.to_string()),
        (
            SCHED_SLOT_BYTES_ENV.into(),
            bytes_per_permit(scheduler.memory_bytes, scheduler.cpus).to_string(),
        ),
        (
            SCHED_PRIORITY_ENV.into(),
            scheduler.priority.as_str().into(),
        ),
    ]
}

fn bytes_per_permit(memory_bytes: Option<u64>, cpus: u64) -> u64 {
    memory_bytes.map_or(0, |memory| memory / cpus.max(1))
}

/// Record what the compiler that just ran cost, for the next admission.
///
/// Only crates that would weigh more than one permit are recorded, which keeps
/// the ledger to the genuinely heavy. A compiler killed by SIGKILL -- the
/// Linux OOM killer's signature -- is recorded *heavier* than it measured, so
/// the retry runs with more room instead of repeating the crash.
pub(crate) fn record_compiler_memory(demand: &Demand, status: &ExitStatus) {
    let Some(pool) = pool() else {
        return;
    };
    pool.note_compilation(demand, status);
}

impl Pool {
    fn new(
        dir: PathBuf,
        capacity: u64,
        bytes_per_permit: u64,
        priority: SchedulerPriority,
    ) -> Self {
        Self {
            dir,
            capacity: capacity.max(1),
            bytes_per_permit,
            priority,
            available_memory: crate::util::memory_available_bytes,
        }
    }

    fn from_config(config: &Config) -> Option<Self> {
        let scheduler = &config.scheduler;
        scheduler.enabled.then(|| {
            Self::new(
                config.cache_dir.join(SCHEDULER_DIR),
                scheduler.cpus,
                bytes_per_permit(scheduler.memory_bytes, scheduler.cpus),
                scheduler.priority,
            )
        })
    }

    /// Wait for capacity to run one real compilation.
    ///
    /// `None` means the pool could not be used and the compilation should run
    /// unscheduled; it never means the compilation should not run.
    pub(crate) fn admit(&self, demand: &Demand) -> Option<Permit> {
        let (weight, predicted) = self.plan(demand);
        let started = Instant::now();
        let mut delay = POLL_INITIAL;
        let mut stamped: Option<Instant> = None;
        let mut complained = false;
        loop {
            // The gate stops applying at its deadline so a machine that stays
            // short of memory delays this compilation rather than starving it.
            let gate = predicted.filter(|_| started.elapsed() < GATE_DEADLINE);
            match self.try_admit(weight, gate) {
                Ok(Some(permit)) => return Some(permit),
                Ok(None) => {}
                Err(error) => {
                    debug!("compiling without a machine-wide permit: {error:#}");
                    return None;
                }
            }
            // Refreshed well inside the freshness window rather than on every
            // attempt: this says "somebody is waiting", which does not become
            // truer for being written forty times a second by every waiter.
            if self.priority == SchedulerPriority::Normal
                && stamped.is_none_or(|last| last.elapsed() >= PRIORITY_STAMP_INTERVAL)
            {
                let _ = std::fs::write(self.dir.join(PRIORITY_WAIT_STAMP), b"");
                stamped = Some(Instant::now());
            }
            // A pool that is genuinely full is a pool doing its job, so this
            // waits rather than giving up. Said once, because the alternative
            // is a build that looks hung with nothing to explain it: the
            // permits are held by compilers that are really running.
            if !complained && started.elapsed() >= SLOW_WAIT_NOTICE {
                complained = true;
                debug!(
                    "waiting for {weight} of {} machine-wide compile permits ({})",
                    self.capacity, demand.name
                );
            }
            std::thread::sleep(jittered(delay));
            delay = (delay * 2).min(POLL_MAX);
        }
    }

    /// The permits this demand costs, and its predicted memory when known.
    fn plan(&self, demand: &Demand) -> (u64, Option<u64>) {
        let link_floor = demand.links.then_some(LINK_WEIGHT);
        if self.bytes_per_permit == 0 {
            return (link_floor.unwrap_or(1).min(self.capacity), None);
        }
        let recorded = read_ledger(&self.ledger_path())
            .crates
            .get(&demand.name)
            .copied();
        let link_bytes = link_floor.map(|weight| weight.saturating_mul(self.bytes_per_permit));
        let predicted = match (recorded, link_bytes) {
            (Some(recorded), Some(link)) => Some(recorded.max(link)),
            (one, other) => one.or(other),
        };
        let weight = predicted
            .map_or(1, |bytes| bytes.div_ceil(self.bytes_per_permit))
            .clamp(1, self.capacity);
        (weight, predicted)
    }

    /// One admission attempt under the registrar lock.
    fn try_admit(&self, weight: u64, predicted: Option<u64>) -> Result<Option<Permit>> {
        let leases = self.dir.join(LEASES_DIR);
        std::fs::create_dir_all(&leases)
            .wrap_err_with(|| format!("failed to create {}", leases.display()))?;
        let registrar_path = self.dir.join(POOL_LOCK);
        let mut registrar = fslock::LockFile::open(&registrar_path)?;
        registrar.lock()?;
        let live = scan_leases(&leases)?;
        // An empty pool admits anything: a crate heavier than the whole
        // machine still has to compile, and it compiles alone.
        if live.is_empty() {
            return self.grant(&leases, weight).map(Some);
        }
        let mut capacity = self.capacity;
        if self.priority == SchedulerPriority::Low && self.priority_wait_is_fresh() {
            capacity = capacity.saturating_sub((self.capacity / 4).max(1));
        }
        let used: u64 = live.iter().sum();
        if used.saturating_add(weight) > capacity {
            return Ok(None);
        }
        // The permit arithmetic bounds what history predicted; this bounds it
        // against the machine as it is right now, catching pressure the
        // ledger cannot see -- unmeasured crates already running, or memory
        // that was never the build's to spend.
        if let Some(needed) = predicted
            && let Some(available) = (self.available_memory)()
            && available < needed
        {
            return Ok(None);
        }
        self.grant(&leases, weight).map(Some)
    }

    /// Create and lock this process's lease. Runs under the registrar lock,
    /// which is what keeps the moment between creation and locking private.
    fn grant(&self, leases: &Path, weight: u64) -> Result<Permit> {
        let path = leases.join(format!(
            "{}-{}",
            std::process::id(),
            LEASE_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        let lease = Lease {
            version: LEASE_VERSION,
            weight,
            priority: self.priority.as_str().into(),
        };
        std::fs::write(&path, serde_json::to_vec(&lease)?)
            .wrap_err_with(|| format!("failed to write {}", path.display()))?;
        let mut lock = fslock::LockFile::open(&path)?;
        if !lock.try_lock()? {
            eyre::bail!("a freshly created lease was already locked");
        }
        Ok(Permit {
            lock: Some(lock),
            path,
        })
    }

    fn priority_wait_is_fresh(&self) -> bool {
        std::fs::metadata(self.dir.join(PRIORITY_WAIT_STAMP))
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|since| since < PRIORITY_WAIT_FRESHNESS)
    }

    fn note_compilation(&self, demand: &Demand, status: &ExitStatus) {
        if self.bytes_per_permit == 0 {
            return;
        }
        let Some(measured) = child_peak_rss_bytes() else {
            return;
        };
        let Some(peak) = ledger_peak(measured, oom_killed(status), self.bytes_per_permit) else {
            return;
        };
        if let Err(error) = self.record_peak(&demand.name, peak) {
            debug!("compiler memory was not recorded: {error:#}");
        }
    }

    fn ledger_path(&self) -> PathBuf {
        self.dir.join(LEDGER_FILE)
    }

    /// Raise the recorded peak for one crate; never lower it.
    fn record_peak(&self, name: &str, peak: u64) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .wrap_err_with(|| format!("failed to create {}", self.dir.display()))?;
        let mut lock = fslock::LockFile::open(&self.dir.join(LEDGER_LOCK))?;
        lock.lock()?;
        let path = self.ledger_path();
        let mut ledger = read_ledger(&path);
        ledger.version = LEDGER_VERSION;
        if ledger.crates.get(name).is_some_and(|known| *known >= peak) {
            return Ok(());
        }
        ledger.crates.insert(name.to_string(), peak);
        while ledger.crates.len() > MAX_LEDGER_ENTRIES {
            let smallest = ledger
                .crates
                .iter()
                .min_by_key(|(_, peak)| **peak)
                .map(|(name, _)| name.clone())
                .expect("an overfull ledger has entries");
            ledger.crates.remove(&smallest);
        }
        let mut contents = serde_json::to_vec(&ledger)?;
        contents.push(b'\n');
        crate::util::write_atomic(&path, &contents)
    }
}

fn read_ledger(path: &Path) -> MemoryLedger {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The live lease weights, reclaiming any lease whose holder has died.
fn scan_leases(leases: &Path) -> Result<Vec<u64>> {
    let mut live = Vec::new();
    for entry in std::fs::read_dir(leases)? {
        let path = entry?.path();
        let mut lock = fslock::LockFile::open(&path)?;
        if lock.try_lock()? {
            // Nobody holds it: the process that created it is gone. Closed
            // before the removal, because Windows will not delete a file that
            // is still open anywhere.
            drop(lock);
            let _ = std::fs::remove_file(&path);
            continue;
        }
        // A live lease that cannot be read still occupies its holder, so it
        // counts as the smallest thing it could be rather than nothing.
        let weight = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Lease>(&bytes).ok())
            .map_or(1, |lease| lease.weight.max(1));
        live.push(weight);
    }
    Ok(live)
}

/// What the ledger should remember about one finished compilation.
///
/// `None` means nothing worth writing: the compile fit comfortably inside a
/// single permit. An OOM kill is recorded past what was measured -- the
/// measurement is where the killer stopped it, not what it needed -- and past
/// one permit, so the retry provably weighs more.
fn ledger_peak(measured: u64, oom_killed: bool, bytes_per_permit: u64) -> Option<u64> {
    if oom_killed {
        return Some(
            measured
                .saturating_mul(2)
                .max(bytes_per_permit.saturating_add(1)),
        );
    }
    (measured > bytes_per_permit).then_some(measured)
}

/// Peak RSS of the compiler this shim ran, from the child rusage counters.
///
/// Exact because a shim process runs exactly one compiler: `RUSAGE_CHILDREN`
/// reports the largest `ru_maxrss` among reaped children, and the identity
/// probes that also ran are orders of magnitude smaller.
#[cfg(unix)]
fn child_peak_rss_bytes() -> Option<u64> {
    // SAFETY: `getrusage` only writes into the zeroed struct it is given.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        if libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage) != 0 {
            return None;
        }
        usage
    };
    let maxrss = u64::try_from(usage.ru_maxrss).ok()?;
    // Apple counts bytes where everyone else counts kibibytes.
    let bytes = if cfg!(target_vendor = "apple") {
        maxrss
    } else {
        maxrss.saturating_mul(1024)
    };
    Some(bytes).filter(|bytes| *bytes > 0)
}

#[cfg(not(unix))]
fn child_peak_rss_bytes() -> Option<u64> {
    None
}

/// Whether the compiler died the way the Linux OOM killer kills.
#[cfg(unix)]
fn oom_killed(status: &ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal() == Some(libc::SIGKILL)
}

#[cfg(not(unix))]
fn oom_killed(_status: &ExitStatus) -> bool {
    false
}

/// Spread waiters out so they do not retry in lockstep.
fn jittered(delay: Duration) -> Duration {
    use rand::RngExt as _;
    delay + delay.mul_f64(rand::rng().random_range(0.0..0.5))
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
