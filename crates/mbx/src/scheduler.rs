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
//! measured after every real compile on Unix; the static link weight is only
//! the guess that stands in until one has been measured. Two things remain
//! out of reach and are accepted: a single compilation larger than the
//! machine will still be too large when it runs alone, and a simultaneous
//! pile-up of never-measured crates can overshoot before the ledger has
//! learned them.
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
//!
//! Beside the pool live the *flights*: one lock per invocation identity, so
//! concurrent builds that reach the same compilation deduplicate it instead
//! of merely pacing it -- see [`Flight`]. The lock ordering between the two
//! is fixed and acyclic: a flight is joined before its permit is requested,
//! never around the other way, and a waiter blocked on a flight holds no
//! permit.
//!
//! Waiting is a timer everywhere and a wakeup on Linux, where a released
//! permit is a deleted lease file and [`ReleaseWake`] watches for exactly
//! that. The timer stays underneath it as the backstop, so nothing depends
//! on the watch existing.

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
/// Subdirectory holding one locked flight file per compiling invocation.
const INFLIGHT_DIR: &str = "inflight";
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
/// Schema version of one flight record.
const FLIGHT_VERSION: u8 = 1;
/// Largest prediction one finished compilation leaves behind for the next.
///
/// A payload is one crate's input list, tens of kilobytes for anything real;
/// this cap is not a budget but a refusal to persist something that has
/// plainly stopped being that.
const MAX_FLIGHT_PAYLOAD: usize = 4 * 1024 * 1024;
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
/// On Linux a release wakes waiters through the leases watch (see
/// [`ReleaseWake`]) and this is only the backstop; elsewhere it bounds how
/// long a core sits idle with work queued for it. Kept short for that
/// reason: one attempt is a readdir and a handful of `try_lock`s, which even
/// a full pool of waiters can afford at this rate.
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

/// Names tried before giving up on finding an unused one.
///
/// A collision needs two processes to draw the same token, so one retry
/// already makes it vanishingly unlikely; the rest are for the pathological
/// case where the token source is not what it claims to be.
const LEASE_NAME_ATTEMPTS: usize = 8;

/// A token drawn once, distinguishing this process from one that shares its
/// pid in another namespace.
fn process_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| crate::util::random_string(8))
}

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

/// What one finished compilation leaves in its flight file: the prediction
/// that lets whoever comes next rebuild the action key and restore instead of
/// compiling.
#[derive(Debug, Serialize, Deserialize)]
struct FlightRecord {
    version: u8,
    adapter: String,
    invocation: String,
    payload: String,
}

/// Exclusive occupancy of one invocation identity, machine-wide.
///
/// Concurrent builds duplicate work the permit pool can only pace: four CI
/// jobs compiling the same dependency graph run the same `rustc` invocation
/// four times, each burning a permit on a result the store is about to hold
/// anyway. A flight is a lock keyed by the invocation digest, taken just
/// before a real compilation starts. Whoever holds it compiles; anyone else
/// arriving at the same invocation blocks on the lock instead of the
/// compiler, which is never slower -- the holder finishes no later than the
/// duplicate would have -- and wakes to a store that already has the result.
///
/// The handoff needs more than the lock, because a cold waiter has no way to
/// build the action key on its own: the key hashes inputs only the compiler's
/// dependency output names. So the holder leaves its input prediction in the
/// flight file when it publishes, and the waiter rehashes those inputs to
/// rebuild the key -- the same validation a manifest prediction gets, so a
/// stale or foreign record can only miss, never restore the wrong thing. The
/// file outlives the flight: a later build of the same invocation finds the
/// prediction even when its own manifest carries nothing, which is what turns
/// jobs that merely overlap -- rather than collide -- into hits.
///
/// Like the pool, this degrades to compiling: every failure returns `None`
/// and the compilation proceeds as if flights did not exist. A holder that
/// dies releases the lock with its process; a holder that publishes nothing
/// -- a failed or uncacheable compile -- wakes its waiters to a miss, and the
/// first of them compiles. That serializes what would have run in parallel,
/// which is the accepted cost: it is rare (the compile usually failed for
/// them all), and for the out-of-memory case it is the desirable behavior.
pub(crate) struct Flight {
    lock: Option<fslock::LockFile>,
    path: PathBuf,
    adapter: &'static str,
    invocation: String,
    /// Whether another process held this invocation when we arrived.
    waited: bool,
    /// The prediction found under the lock, when it describes this invocation.
    inherited: Option<String>,
}

impl Flight {
    /// Whether this process blocked behind another compiling the same thing.
    pub(crate) fn waited(&self) -> bool {
        self.waited
    }

    /// The prediction the previous holder left, if any.
    pub(crate) fn inherited(&self) -> Option<&str> {
        self.inherited.as_deref()
    }

    /// Leave this compilation's prediction for whoever comes next.
    ///
    /// Written beside the lock file rather than into it, because the lock
    /// file's contents belong to the lock: releasing one truncates it. The
    /// record is only ever read and written under the lock, and the write is
    /// atomic besides, so a reader can never see half of one.
    pub(crate) fn leave(&self, payload: &str) {
        if payload.len() > MAX_FLIGHT_PAYLOAD {
            return;
        }
        let record = FlightRecord {
            version: FLIGHT_VERSION,
            adapter: self.adapter.into(),
            invocation: self.invocation.clone(),
            payload: payload.into(),
        };
        let written = serde_json::to_vec(&record)
            .map_err(eyre::Report::from)
            .and_then(|bytes| crate::util::write_atomic(&record_path(&self.path), &bytes));
        if let Err(error) = written {
            debug!("a flight prediction was not left behind: {error:#}");
        }
    }
}

/// Where one flight's prediction record lives, beside its lock.
fn record_path(lock_path: &Path) -> PathBuf {
    let mut path = lock_path.as_os_str().to_owned();
    path.push(".prediction");
    PathBuf::from(path)
}

/// How long an untouched flight is kept for.
///
/// Both of a flight's files get a fresh timestamp every time its invocation
/// really compiles, so age here means nobody has compiled this identity in a
/// month -- and identities churn with every toolchain and dependency bump,
/// which is why the directory needs a sweep at all.
const FLIGHT_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Drop flight files whose invocations nobody compiles any more.
///
/// Run from `mbx gc` beside the store's own collection. Losing a record
/// costs at most one compilation that would have been a hit; a lock that
/// cannot be taken belongs to a live compilation and is left alone together
/// with its record.
pub(crate) fn prune_flights(cache_dir: &Path) {
    let dir = cache_dir.join(SCHEDULER_DIR).join(INFLIGHT_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let stale = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > FLIGHT_MAX_AGE)
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "prediction")
        {
            // Swept with its lock below while the lock exists; on its own it
            // is a leftover from an interrupted sweep.
            if stale(&path) && !path.with_extension("").exists() {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        let record = record_path(&path);
        if !stale(&path) || (record.exists() && !stale(&record)) {
            continue;
        }
        let Ok(mut lock) = fslock::LockFile::open(&path) else {
            continue;
        };
        if lock.try_lock().is_ok_and(|taken| taken) {
            let _ = std::fs::remove_file(&record);
            // Closed before the removal, because Windows will not delete a
            // file that is still open anywhere.
            drop(lock);
            let _ = std::fs::remove_file(&path);
        }
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        // The file stays: it holds the prediction the next build of this
        // invocation restores from. Only the lock is released.
        drop(self.lock.take());
    }
}

/// Join the flight for one invocation, becoming its compiler.
///
/// Returns holding the lock. If another process was already compiling this
/// invocation, that means having waited for it to finish -- which is the
/// point: on waking, `inherited` usually restores and the compiler never
/// runs. `None` means flights could not be used; the compilation just runs.
pub(crate) fn flight(adapter: &'static str, invocation: &str) -> Option<Flight> {
    let pool = pool()?;
    match flight_at(&pool.dir, adapter, invocation) {
        Ok(flight) => Some(flight),
        Err(error) => {
            debug!("compiling outside a flight: {error:#}");
            None
        }
    }
}

fn flight_at(pool_dir: &Path, adapter: &'static str, invocation: &str) -> Result<Flight> {
    let dir = pool_dir.join(INFLIGHT_DIR);
    std::fs::create_dir_all(&dir)
        .wrap_err_with(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{adapter}-{invocation}"));
    let mut lock = fslock::LockFile::open(&path)?;
    let waited = !lock.try_lock()?;
    if waited {
        lock.lock()?;
    }
    let inherited = std::fs::read(record_path(&path))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<FlightRecord>(&bytes).ok())
        .filter(|record| {
            record.version == FLIGHT_VERSION
                && record.adapter == adapter
                && record.invocation == invocation
        })
        .map(|record| record.payload);
    Ok(Flight {
        lock: Some(lock),
        path,
        adapter,
        invocation: invocation.to_string(),
        waited,
        inherited,
    })
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
/// Crates that fit inside one permit are not worth remembering -- except
/// links, whose measurement is what retires the static floor they are
/// otherwise charged. A compiler killed by SIGKILL -- the Linux OOM killer's
/// signature -- is recorded *heavier* than it measured, so the retry runs
/// with more room instead of repeating the crash.
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
        let mut wake: Option<Option<ReleaseWake>> = None;
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
            // Armed after the first refusal rather than before the first
            // attempt: most admissions succeed immediately and should not pay
            // for a watch. Arming races the release it would have caught,
            // which the timeout below absorbs; from the next iteration on,
            // a release lands in the watch before the scan it should wake.
            let wake = wake.get_or_insert_with(|| ReleaseWake::new(&self.dir.join(LEASES_DIR)));
            match wake {
                Some(wake) => wake.wait(jittered(delay)),
                None => std::thread::sleep(jittered(delay)),
            }
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
            .get(&ledger_key(&demand.name, demand.links))
            .copied();
        let link_bytes = link_floor.map(|weight| weight.saturating_mul(self.bytes_per_permit));
        // History replaces the floor rather than merely raising it. The floor
        // is a guess for links the pool has never seen; once one has been
        // measured, charging it the guess anyway would hold the tail of every
        // build -- the link-heavy stretch -- to half the concurrency its real
        // memory justifies. The ledger never lowers a recorded peak and an
        // OOM kill escalates past it, so trusting the measurement stays safe
        // in the direction that matters.
        //
        // Only a *link's* own history may retire the link floor, which is why
        // the two are separate ledger entries. One crate is compiled both
        // ways in a single build -- `cargo check` emits metadata where `cargo
        // build` links -- and the metadata-only measurement is the smaller of
        // the two by far. Keyed by name alone it would retire the floor for
        // the link beside it, which is over-admission with a plausible number
        // attached to it.
        let predicted = recorded.or(link_bytes);
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
        let capacity = self.capacity - self.reserved();
        let used: u64 = live.iter().sum();
        // A demand too heavy for what the pool will lend it can only ever run
        // by itself, so on an idle pool it does, rather than waiting for room
        // that nothing is going to make. Measured against the capacity left
        // after the reserve, because that is what this build may actually
        // take -- and conditioned on the demand rather than on the pool
        // merely being idle, since idle is the ordinary state between two
        // compilations and granting unconditionally there would let a
        // low-priority build take the machine in exactly the gap the reserve
        // exists to hold open.
        if used == 0 && weight > capacity {
            return self.grant(&leases, weight).map(Some);
        }
        if used.saturating_add(weight) > capacity {
            return Ok(None);
        }
        // The permit arithmetic bounds what history predicted; this bounds it
        // against the machine as it is right now, catching pressure the
        // ledger cannot see -- unmeasured crates already running, or memory
        // that was never the build's to spend.
        //
        // Only while somebody else holds a permit, though. With an idle pool
        // there is no compilation whose finishing would return the memory
        // this one is short of, so deferring would stall the one thing that
        // could proceed and wait out the deadline to reach the same answer.
        if used > 0
            && let Some(needed) = predicted
            && let Some(available) = (self.available_memory)()
            && available < needed
        {
            return Ok(None);
        }
        self.grant(&leases, weight).map(Some)
    }

    /// Create and lock this process's lease. Runs under the registrar lock,
    /// which is what keeps the moment between creation and locking private.
    ///
    /// The name has to be unique against processes this one cannot see. A pid
    /// is only unique within its namespace, and two containers sharing a cache
    /// directory have separate ones -- pid 7 in each is two different
    /// processes. A name collision there would be silent and wrong in both
    /// directions: writing the lease would truncate a live holder's record,
    /// and this compilation would then run unscheduled. So the name carries a
    /// token drawn once per process, and the file is created exclusively so
    /// that a collision is an error to retry rather than an overwrite.
    fn grant(&self, leases: &Path, weight: u64) -> Result<Permit> {
        let lease = serde_json::to_vec(&Lease {
            version: LEASE_VERSION,
            weight,
            priority: self.priority.as_str().into(),
        })?;
        let mut last = None;
        for _ in 0..LEASE_NAME_ATTEMPTS {
            let path = leases.join(format!(
                "{}-{}-{}",
                std::process::id(),
                process_token(),
                LEASE_NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    std::io::Write::write_all(&mut file, &lease)
                        .wrap_err_with(|| format!("failed to write {}", path.display()))?;
                    drop(file);
                    let mut lock = fslock::LockFile::open(&path)?;
                    if !lock.try_lock()? {
                        eyre::bail!("a freshly created lease was already locked");
                    }
                    return Ok(Permit {
                        lock: Some(lock),
                        path,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last = Some(error);
                }
                Err(error) => {
                    return Err(error)
                        .wrap_err_with(|| format!("failed to create {}", path.display()));
                }
            }
        }
        Err(last.expect("a name is only retried after a collision"))
            .wrap_err("failed to name a lease that was not already taken")
    }

    /// Permits this build holds back for somebody who is waiting on them.
    ///
    /// Never the whole pool. A reserve equal to the capacity -- which a
    /// single-permit machine would otherwise get -- stops a low-priority
    /// build admitting through the ordinary path at all, leaving the
    /// oversized-demand escape as the only way it ever runs, and that one
    /// hands over the entire machine at once. Yielding a quarter must not
    /// become yielding everything and then taking everything.
    fn reserved(&self) -> u64 {
        if self.priority == SchedulerPriority::Low && self.priority_wait_is_fresh() {
            (self.capacity / 4).max(1).min(self.capacity - 1)
        } else {
            0
        }
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
        let Some(peak) = ledger_peak(
            measured,
            oom_killed(status),
            demand.links,
            self.bytes_per_permit,
        ) else {
            return;
        };
        if let Err(error) = self.record_peak(&ledger_key(&demand.name, demand.links), peak) {
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

/// What one crate's memory is remembered under.
///
/// A link and a metadata-only compile of the same crate are different
/// workloads with the same name -- `cargo check` and `cargo build` run both
/// against one crate, often at the same moment -- so they are remembered
/// apart. The suffix cannot collide with a crate name, which is a Rust
/// identifier and can hold neither a space nor a bracket.
fn ledger_key(name: &str, links: bool) -> String {
    if links {
        format!("{name} [link]")
    } else {
        name.to_string()
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
/// single permit and carried no static weight worth correcting. A link is
/// always recorded, even light, because an unmeasured link is charged a
/// deliberately heavy floor and the measurement is what retires it. An OOM
/// kill is recorded past what was measured -- the measurement is where the
/// killer stopped it, not what it needed -- and past one permit, so the
/// retry provably weighs more.
fn ledger_peak(measured: u64, oom_killed: bool, links: bool, bytes_per_permit: u64) -> Option<u64> {
    if oom_killed {
        return Some(
            measured
                .saturating_mul(2)
                .max(bytes_per_permit.saturating_add(1)),
        );
    }
    if links {
        return Some(measured);
    }
    (measured > bytes_per_permit).then_some(measured)
}

/// Peak RSS of the compiler this shim ran, from the child rusage counters.
///
/// Exact because a shim process runs exactly one compiler: `RUSAGE_CHILDREN`
/// reports the largest `ru_maxrss` among reaped children, and the identity
/// probes that also ran are orders of magnitude smaller.
///
/// It reaches the linker, which matters here more than anything else it
/// measures. A link is a grandchild -- rustc runs `cc`, which runs `ld` --
/// and the counter still covers it, because a process's own maximum is
/// folded into its parent's children total when it is reaped, all the way up.
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
///
/// Only what rustc itself reports, which is the limit of this signal: a
/// linker killed as a grandchild leaves rustc to exit non-zero with a
/// "linking failed" message, and that is indistinguishable here from an
/// ordinary compile error. What saves the case is that the peak is recorded
/// whether or not the compilation succeeded, so a link that died reaching
/// for memory still teaches the ledger roughly what it was holding; it just
/// does not get the doubling on top.
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

/// Wakes a waiter the moment a permit is released, on hosts that can.
///
/// A permit's release *is* the deletion of its lease file, so on Linux the
/// leases directory is watched for deletions through inotify and a refused
/// waiter sleeps on the watch instead of a timer -- the gap between a
/// compiler exiting and the next admission goes from a poll interval to a
/// wakeup. The timer stays underneath as the backstop: a release can slip
/// past while the watch is first armed, and inotify instances are a bounded
/// per-user resource that a large enough build simply runs out of. Failing
/// to watch therefore just means waiting the way every other platform does.
#[cfg(target_os = "linux")]
struct ReleaseWake {
    fd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl ReleaseWake {
    fn new(leases: &Path) -> Option<Self> {
        use std::os::fd::{AsRawFd as _, FromRawFd as _};
        use std::os::unix::ffi::OsStrExt as _;
        let path = std::ffi::CString::new(leases.as_os_str().as_bytes()).ok()?;
        // SAFETY: inotify_init1 returns a new descriptor that nothing else
        // owns; wrapping it immediately gives it an owner that closes it.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        // SAFETY: the descriptor is a live inotify instance and the path is a
        // valid NUL-terminated string for the duration of the call.
        let watch =
            unsafe { libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), libc::IN_DELETE) };
        (watch >= 0).then_some(Self { fd })
    }

    /// Sleep until a lease is released or the timeout passes.
    fn wait(&self, timeout: Duration) {
        use std::os::fd::AsRawFd as _;
        let mut poll = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = i32::try_from(timeout.as_millis().max(1)).unwrap_or(i32::MAX);
        // SAFETY: one valid pollfd is handed over for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready <= 0 {
            return;
        }
        // Drained so the next wait sleeps on releases that have not been
        // seen rather than returning instantly on ones that have. What the
        // events say does not matter; that they happened is the signal.
        let mut buffer = [0_u8; 4096];
        // SAFETY: reads only ever land inside the local buffer, and the
        // descriptor is nonblocking, so the loop cannot hang.
        while unsafe {
            libc::read(
                self.fd.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        } > 0
        {}
    }
}

/// Hosts without a release watch wait on the timer alone.
#[cfg(not(target_os = "linux"))]
struct ReleaseWake;

#[cfg(not(target_os = "linux"))]
impl ReleaseWake {
    fn new(_leases: &Path) -> Option<Self> {
        None
    }

    fn wait(&self, timeout: Duration) {
        std::thread::sleep(timeout)
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
