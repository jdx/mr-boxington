//! Machine-wide scheduling of real compiler processes.
//!
//! Cargo plans one build at a time: three simultaneous worktree builds each
//! believe they own the machine and multiply `-j`. The shims sit in front of
//! every expensive process across all of those builds at once, so this module
//! coordinates what Cargo cannot -- a permit pool under the cache directory
//! that real compilations draw from. Cache hits never touch it, and Cargo
//! keeps its own dependency scheduling; only the processes that actually cost
//! CPU and memory wait their turn. With `scheduler.tests`, the test binaries
//! `cargo test` and `cargo nextest` run draw from the same pool (see
//! `cli::test_runner`).
//!
//! The pool is a directory of *lease* files, one per admitted compilation,
//! each held under an OS file lock for the life of the compile. The kernel
//! releases those locks when their holder dies, so a killed shim can never
//! leak capacity; a lease whose lock can be taken is stale and is reclaimed
//! by the next admission scan. Admission itself serializes on one registrar
//! lock, held only long enough to scan and grant.
//!
//! CPU cores and predicted memory bytes are independent budgets. A request
//! must fit both, with an empty-scope exception on each dimension so oversize
//! work can make progress. Peak-RSS history supplies the memory prediction;
//! unmeasured work keeps the original permit-based memory guess. The legacy
//! weight and permit size remain in records for older wrappers.
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

#[path = "scheduler_accuracy.rs"]
mod accuracy;
#[path = "scheduler_decisions.rs"]
mod decisions;
#[path = "scheduler_memory.rs"]
mod memory;

use crate::config::{Config, SchedulerPriority};
use eyre::{Context, Result};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Directory under the cache root holding the pool.
const SCHEDULER_DIR: &str = "scheduler";
/// Subdirectory holding one locked lease file per admitted compilation.
const LEASES_DIR: &str = "leases";
const WAITERS_DIR: &str = "waiters";
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
/// Per-test history, kept apart from the shared suite ledger.
const TEST_HISTORY_DIR: &str = "tests";
/// Magic and version for the compact per-test history format.
const TEST_HISTORY_MAGIC: &[u8; 8] = b"mbxtest\x01";
/// Bytes in a per-test history header.
const TEST_HISTORY_HEADER_LEN: u64 = 32;
/// Bytes in one per-test history record.
const TEST_HISTORY_RECORD_LEN: u64 = 24;
/// Lock serializing per-test history updates, for every binary at once.
///
/// One lock rather than one per history file, because a lock file can never
/// be removed safely: a writer already waiting on the removed file would lock
/// it while a newcomer locks its replacement. Writes are rare -- a test's
/// history changes only when a measurement exceeds it -- so they can share.
const TEST_HISTORY_LOCK: &str = "tests.lock";

/// Where the pool lives; empty means the session turned scheduling off.
pub(crate) const SCHED_DIR_ENV: &str = "MBX_SCHED_DIR";
/// Keep scheduler shims off for work already covered by an outer reservation.
///
/// A nested `mbx` command otherwise constructs a new session and re-resolves
/// configuration, which would replace an inherited empty `MBX_SCHED_DIR`.
pub(crate) const SCHED_DISABLE_ENV: &str = "MBX_SCHED_DISABLE";
/// Machine-wide CPU budget after reserving cores for other work.
pub(crate) const SCHED_SLOTS_ENV: &str = "MBX_SCHED_SLOTS";
/// Configured CPU count before reserve_cpus, for the local CPU ceiling.
const SCHED_CPUS_ENV: &str = "MBX_SCHED_CPUS";
/// Exact memory budget, independent of legacy permit rounding.
const SCHED_MEMORY_BYTES_ENV: &str = "MBX_SCHED_MEMORY_BYTES";
/// Legacy memory share; zero disables memory predictions.
pub(crate) const SCHED_SLOT_BYTES_ENV: &str = "MBX_SCHED_SLOT_BYTES";
/// Permit priority of this build's compilations.
pub(crate) const SCHED_PRIORITY_ENV: &str = "MBX_SCHED_PRIORITY";
/// Identity shared by every compiler shim in one Cargo build.
const SCHED_PRESSURE_ENV: &str = "MBX_SCHED_PRESSURE";
/// The session's resolved events gate, including workspace overrides.
const SCHED_EVENTS_ENV: &str = "MBX_SCHED_EVENTS";
const SCHED_BUILD_ID_ENV: &str = "MBX_SCHED_BUILD_ID";
/// Names the build whose permit an ancestor compiler process already holds.
/// A wrapper such as `cargo-llvm-cov` can run another `mbx-rustc` beneath the
/// one that holds a permit; asking for a second permit there deadlocks once
/// every slot is held by a parent waiting on its own child.
const SCHED_PERMIT_HELD_ENV: &str = "MBX_SCHED_PERMIT_HELD";
/// Most CPU cores one Cargo build may hold at once.
const SCHED_BUILD_SLOTS_ENV: &str = "MBX_SCHED_BUILD_SLOTS";

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
/// Link measurements the window keeps, newest last.
///
/// Small on purpose: it stands for what *this* machine's links cost lately,
/// and a window long enough to remember a toolchain ago would keep quoting a
/// number nothing here produces any more.
const MAX_LINK_SAMPLES: usize = 16;
/// Trust threshold used only by the independent old-reader test reference.
#[cfg(test)]
const MIN_LINK_SAMPLES: usize = 3;
/// Permits a native link costs before it has ever been measured.
///
/// Links are the out-of-memory class -- rust-lang/cargo#12912 is about little
/// else -- so they start heavy rather than waiting for history to say so.
const LINK_WEIGHT: u64 = 2;
/// Permits an unmeasured test starts with when memory scheduling is on.
///
/// Tests can allocate most of their working set at runtime, so allowing many
/// cold tests to start together can exhaust memory before history is learned.
const UNMEASURED_TEST_WEIGHT: u64 = 2;
/// How old a history file's timestamp may get before a reader refreshes it.
///
/// History is written only when a measurement rises, so a binary whose tests
/// run every day at the same cost would otherwise look unused to `mbx gc`.
/// Refreshing at most once a day keeps that one metadata write per binary
/// per day, rather than one per test.
const TEST_HISTORY_REFRESH: Duration = Duration::from_secs(24 * 60 * 60);
/// Most test names retained for one nextest binary.
const MAX_TEST_HISTORY_ENTRIES: usize = 16_384;
/// First delay after a refused admission.
const POLL_INITIAL: Duration = Duration::from_millis(2);
/// Longest delay between admission attempts.
///
/// A permit is released the instant its compiler exits and nothing wakes the
/// waiters, so this bounds how long a core sits idle with work queued for
/// it. Kept short for that reason: one attempt is a readdir and a handful of
/// `try_lock`s, which even a full pool of waiters can afford at this rate.
///
/// Waking waiters on the release itself -- watching the leases directory, so
/// the wait ends when a lease disappears -- was built and measured and did
/// not pay, so it is deliberately not here. The reason is not that the win
/// was small enough to lose in the noise: this interval is not what a waiter
/// is waiting on. Waiters poll at independent jittered phases, so a released
/// permit is found by whichever of them wakes first, and the more waiters
/// there are the sooner that is.
///
/// Measured on hk, which is the shape that makes this look worst -- one
/// build, permits deliberately scarce so the pool really is the constraint:
///
/// - at the default permit count, the whole build spends 0.2 s waiting for
///   permits across ten admissions, so nothing here is worth reaching for;
/// - at two permits, where admissions wait 1100 s in aggregate, cutting this
///   interval by twelve and a half times -- to 2 ms, past what any wake could
///   save -- moved the build from 62.5 s to 63.0 s, and the aggregate wait
///   from 1105 s to 1073 s. Both are inside the run-to-run spread.
///
/// What that aggregate wait is, then, is capacity that genuinely was not
/// free. No mechanism for noticing sooner can return it.
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

fn next_lease_nonce() -> u64 {
    LEASE_NONCE.fetch_add(1, Ordering::Relaxed)
}

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

/// What one compilation, test suite, or nextest test asks the pool for.
pub(crate) struct Demand {
    /// Demand name, also keying compiler and suite history.
    name: String,
    /// Whether the invocation links a native program.
    links: bool,
    /// Permits the process's threads need whatever its memory, or `None` for
    /// half the pool: a test harness sized to the machine that has not said
    /// how many threads it will really use.
    threads: Option<u64>,
    /// Whether the ledger's record of cores this process kept busy replaces
    /// `threads`. Only test binaries are measured; a compiler is one thread.
    learns_cpu: bool,
    /// The code-generation settings of a link, which key its window of recent
    /// measurements: a release link's peaks must not stand in for a dev
    /// link that has none of its own yet.
    profile: Option<String>,
    /// A nextest test whose measurements belong in its separate history file.
    per_test: Option<TestIdentity>,
    compile_mode: Option<String>,
    /// Another wrapper may have returned a cache hit without running rustc.
    count_compile_samples: bool,
    observation: std::cell::Cell<Option<accuracy::Admission>>,
    /// Refreshed while waiting, then retained for completion calibration.
    memory_features: std::cell::Cell<Option<memory::Features>>,
    /// Frozen only for sibling-sourced admissions; never own-window features.
    sibling_features: std::cell::Cell<Option<memory::Features>>,
}

#[derive(Debug, Clone)]
struct TestIdentity {
    binary: String,
    test: String,
    /// What history said about this test when it was planned, so a finished
    /// test whose measurements raise nothing records without taking the
    /// history lock or reading the file again. `None` until planned.
    known: std::cell::Cell<Option<TestMeasurement>>,
}

impl Demand {
    pub(crate) fn new(name: &str, links: bool) -> Self {
        Self {
            name: name.to_string(),
            links,
            threads: Some(1),
            learns_cpu: false,
            profile: None,
            compile_mode: None,
            count_compile_samples: true,
            observation: std::cell::Cell::new(None),
            memory_features: std::cell::Cell::new(None),
            sibling_features: std::cell::Cell::new(None),
            per_test: None,
        }
    }

    /// Remember links by code-generation profile and non-links by compile mode.
    ///
    /// A release build with fat LTO and one codegen unit peaks several times
    /// higher than a dev link of the same binary, and the ledger never lowers
    /// a recorded peak, so without this one release build would charge every
    /// later dev link its memory. Read from the rustc flags rather than the
    /// profile name, since a custom profile inherits from `release`.
    pub(crate) fn with_link_profile(mut self, arguments: &[std::ffi::OsString]) -> Self {
        if self.links {
            let profile = link_profile(arguments);
            self.name = format!("{} [{profile}]", self.name);
            self.profile = Some(profile);
        } else {
            self.compile_mode = Some(format!(
                "{} [compile {}]",
                self.name,
                compile_profile(arguments)
            ));
        }
        self
    }

    /// Build the mode from the effective compiler flags, including a private
    /// flag appended by mbx after argument parsing. Wrapper RSS can raise a
    /// peak, but cannot establish a successful real-compiler sample.
    pub(crate) fn with_compiler_execution(
        mut self,
        arguments: &[std::ffi::OsString],
        appended_flag: Option<&std::ffi::OsStr>,
        wrapped: bool,
        clippy: bool,
    ) -> Self {
        let mut effective = arguments.to_vec();
        effective.extend(appended_flag.map(std::ffi::OsStr::to_os_string));
        self.count_compile_samples = !wrapped;
        let mut demand = self.with_link_profile(&effective);
        // Clippy runs lint passes a plain check does not, so it learns its own
        // estimate rather than borrowing one from `cargo check`.
        if clippy && demand.compile_mode.is_some() {
            demand.compile_mode = Some(format!(
                "{} [compile clippy {}]",
                demand.name,
                compile_profile(&effective)
            ));
        }
        demand
    }

    #[cfg(test)]
    pub(crate) fn compile_mode_key(&self) -> Option<&str> {
        self.compile_mode.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn counts_compile_samples(&self) -> bool {
        self.count_compile_samples
    }

    /// A test binary, remembered apart from the crate that compiled it.
    ///
    /// Half the pool by default rather than all of it. libtest starts a
    /// thread per CPU, but most suites spend much of their time waiting, and
    /// a demand for the whole pool is admitted only once the pool is idle --
    /// which a stream of other builds' compilations can postpone
    /// indefinitely. Half still keeps two suites from each running at full
    /// width beside a third build's compiles.
    ///
    /// A binary run with a stated thread count is remembered apart from the
    /// same binary at its default width: the two keep different numbers of
    /// cores busy.
    pub(crate) fn test(name: &str, threads: Option<u64>) -> Self {
        Self {
            // The suffix cannot collide with a crate name, for the reason
            // `ledger_key` gives.
            name: match threads {
                Some(threads) => format!("{name} [test, {threads} threads]"),
                None => format!("{name} [test]"),
            },
            links: false,
            threads,
            learns_cpu: true,
            profile: None,
            compile_mode: None,
            count_compile_samples: true,
            observation: std::cell::Cell::new(None),
            memory_features: std::cell::Cell::new(None),
            sibling_features: std::cell::Cell::new(None),
            per_test: None,
        }
    }

    /// One nextest test, remembered apart from its binary's other tests.
    pub(crate) fn nextest(binary: &str, test: &str) -> Self {
        Self {
            name: format!("{binary} {test} [nextest]"),
            links: false,
            threads: Some(1),
            learns_cpu: true,
            profile: None,
            compile_mode: None,
            count_compile_samples: true,
            observation: std::cell::Cell::new(None),
            memory_features: std::cell::Cell::new(None),
            sibling_features: std::cell::Cell::new(None),
            per_test: Some(TestIdentity {
                binary: binary.to_owned(),
                test: test.to_owned(),
                known: std::cell::Cell::new(None),
            }),
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
        // Unlocking and closing makes the lease stale to anyone scanning,
        // which is exactly what it is: its holder has finished. The worst a
        // concurrent scan does is remove it first.
        drop(self.lock.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One lease record, stored beside the lock that proves its holder is alive.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Lease {
    version: u8,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    mode_key: Option<String>,
    weight: u64,
    #[serde(default)]
    cpu: Option<u64>,
    #[serde(default)]
    memory_bytes: Option<u64>,
    priority: String,
    #[serde(default)]
    build_id: Option<String>,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    bytes_per_permit: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Waiting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_arrival_ms: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    mode_key: Option<String>,
    ticket: u64,
    weight: u64,
    #[serde(default)]
    cpu: Option<u64>,
    #[serde(default)]
    memory_bytes: Option<u64>,
    priority: String,
    domain: String,
    domain_capacity: u64,
    #[serde(default)]
    global_capacity: Option<u64>,
    #[serde(default)]
    cpu_cap: Option<u64>,
    #[serde(default)]
    mem_cap: Option<u64>,
    #[serde(default)]
    domain_memory_cap: Option<u64>,
    #[serde(default)]
    domain_cpu_cap: Option<u64>,
    bytes_per_permit: u64,
    build: Option<(String, u64)>,
}

struct QueueEntry {
    record: Option<Waiting>,
    ticket: u64,
    serialized_record: Vec<u8>,
    _guard: Permit,
}

impl QueueEntry {
    fn mark_released(&self) {
        // fslock unlocks before truncating. Empty the record while we still
        // hold its lock so a graceful release cannot look like a dead holder.
        // On Windows fslock locks a range far beyond the record's bytes, so
        // truncating through another handle should work there too, but that is
        // untested. If truncation fails, the old classification race remains.
        if let Ok(file) = std::fs::OpenOptions::new()
            .write(true)
            .open(&self._guard.path)
        {
            let _ = file.set_len(0);
        }
    }
}

impl Drop for QueueEntry {
    fn drop(&mut self) {
        self.mark_released();
        #[cfg(test)]
        decisions::after_queue_release(self);
    }
}

struct AdmissionState {
    queued: Option<QueueEntry>,
    first_arrival_ms: u64,
    last_gate: Option<&'static str>,
}

impl AdmissionState {
    fn new(first_arrival_ms: u64) -> Self {
        Self {
            queued: None,
            first_arrival_ms,
            last_gate: None,
        }
    }
}

fn fits(used: u64, weight: u64, capacity: u64) -> bool {
    used == 0
        || used
            .checked_add(weight)
            .is_some_and(|total| total <= capacity)
}

/// Physical resources. A zero memory ceiling means unlimited memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Resources {
    cpu: u64,
    memory_bytes: u64,
}

impl Resources {
    fn fits(self, used: Self, request: Self) -> bool {
        // Each dimension grants its own empty-scope exception. An oversize
        // CPU request still needs memory room, and vice versa.
        fits(used.cpu, request.cpu, self.cpu)
            && (self.memory_bytes == 0
                || fits(used.memory_bytes, request.memory_bytes, self.memory_bytes))
    }
}

/// The only legacy conversion: old records reserve their full weight on both
/// axes. Keep zero as a real value for new memory-only/CPU-only reservations.
fn record_resources(cpu: Option<u64>, memory: Option<u64>, weight: u64, unit: u64) -> Resources {
    Resources {
        cpu: cpu.unwrap_or(weight),
        memory_bytes: memory.unwrap_or_else(|| weight.saturating_mul(unit)),
    }
}

impl Lease {
    fn resources(&self) -> Resources {
        record_resources(
            self.cpu,
            self.memory_bytes,
            self.weight,
            self.bytes_per_permit,
        )
    }
}

impl Waiting {
    fn resources(&self) -> Resources {
        record_resources(
            self.cpu,
            self.memory_bytes,
            self.weight,
            self.bytes_per_permit,
        )
    }
    fn global_budget(&self, fallback: Resources) -> Resources {
        Resources {
            cpu: self
                .cpu_cap
                .or(self.global_capacity)
                .unwrap_or(fallback.cpu),
            memory_bytes: self.mem_cap.unwrap_or_else(|| {
                self.global_capacity.map_or(fallback.memory_bytes, |cap| {
                    cap.saturating_mul(self.bytes_per_permit)
                })
            }),
        }
    }
    fn domain_budget(&self) -> Resources {
        Resources {
            cpu: self.domain_cpu_cap.unwrap_or(self.domain_capacity),
            memory_bytes: self
                .domain_memory_cap
                .unwrap_or_else(|| self.domain_capacity.saturating_mul(self.bytes_per_permit)),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Plan {
    resources: Resources,
    weight: u64,
    predicted: Option<u64>,
    source: &'static str,
}

const MEMORY_QUANTUM: u64 = 16 * 1024 * 1024;

// Match the scalar charge an old reader computes after a permit-unit change.
fn weight_in_units(weight: u64, original_unit: u64, unit: u64) -> u64 {
    if unit == 0 {
        weight
    } else {
        weight.max(weight.saturating_mul(original_unit).div_ceil(unit))
    }
}

#[cfg(test)]
fn can_pass_waiters(capacity: u64, live: &[u64], older: &[u64], weight: u64) -> bool {
    can_pass_quantized(capacity, live, older, weight, 1)
}

/// Protect every older prefix against every possible completion subset.
#[cfg(test)]
fn can_pass_quantized(capacity: u64, live: &[u64], older: &[u64], charge: u64, unit: u64) -> bool {
    pass_quantized(capacity, live, older, charge, unit, |_, _| {})
}

fn pass_quantized(
    capacity: u64,
    live: &[u64],
    older: &[u64],
    charge: u64,
    unit: u64,
    mut record: impl FnMut(usize, u64),
) -> bool {
    if older.is_empty() {
        return true;
    }
    const MAX_BITS: u64 = 1 << 16;
    let Some(total) = live.iter().chain(older).try_fold(0_u64, |sum, w| {
        sum.checked_add(w.div_ceil(unit))
            .filter(|sum| *sum < MAX_BITS)
    }) else {
        return false;
    };
    let Some(used) = live.iter().try_fold(0_u64, |sum, w| sum.checked_add(*w)) else {
        return false;
    };
    let Some(free) = capacity.checked_sub(used) else {
        return false;
    };
    if charge > free {
        return false;
    }
    let mut sums = vec![0_u64; (total as usize + 1).div_ceil(64)];
    sums[0] = 1;
    for w in live {
        add_subset_weight(&mut sums, w.div_ceil(unit) as usize);
    }
    let mut prefix = 0_u64;
    for (index, h) in older.iter().enumerate() {
        let Some(next) = prefix.checked_add(*h) else {
            return false;
        };
        prefix = next;
        let slack = if prefix <= free {
            free - prefix
        } else {
            let threshold = prefix - free;
            let quantized_threshold = threshold.div_ceil(unit) as usize;
            let first_word = quantized_threshold / 64;
            let Some(sum) = (first_word..sums.len()).find_map(|index| {
                let bits = if index == first_word {
                    sums[index] & (u64::MAX << (quantized_threshold % 64))
                } else {
                    sums[index]
                };
                (bits != 0).then(|| index * 64 + bits.trailing_zeros() as usize)
            }) else {
                return false;
            };
            // Every eligible exact subset S >= threshold has rounded-up sum
            // Q(S) >= ceil(threshold/unit). Thus the minimum DP sum is no
            // larger than Q(S). Each item adds at most unit-1 rounding bytes;
            // subtracting n*(unit-1) makes this a LOWER bound on every eligible
            // exact sum, hence on slack. Extra ineligible subsets only reduce
            // the bound. Saturation must refuse, not manufacture slack.
            let Some(error) = (live.len() + index)
                .try_into()
                .ok()
                .and_then(|count: u64| count.checked_mul(unit - 1))
            else {
                return false;
            };
            let Some(lower) = (sum as u64)
                .checked_mul(unit)
                .and_then(|sum| sum.checked_sub(error))
            else {
                return false;
            };
            let Some(slack) = lower.checked_sub(threshold) else {
                return false;
            };
            slack
        };
        record(index, slack);
        if charge > slack {
            return false;
        }
        add_subset_weight(&mut sums, h.div_ceil(unit) as usize);
    }
    true
}

fn add_subset_weight(sums: &mut [u64], weight: usize) {
    let words = weight / 64;
    let shift = weight % 64;
    for index in (words..sums.len()).rev() {
        let mut shifted = sums[index - words] << shift;
        if shift != 0 && index > words {
            shifted |= sums[index - words - 1] >> (64 - shift);
        }
        sums[index] |= shifted;
    }
}

fn scope_used<'a>(leases: impl Iterator<Item = &'a Lease>) -> Resources {
    leases.fold(Resources::default(), |used, lease| {
        let charge = lease.resources();
        Resources {
            cpu: used.cpu.saturating_add(charge.cpu),
            memory_bytes: used.memory_bytes.saturating_add(charge.memory_bytes),
        }
    })
}

fn domain_used(live: &[Lease], domain: &str) -> Resources {
    scope_used(
        live.iter()
            .filter(|lease| lease.domain.as_deref().is_none_or(|id| id == domain)),
    )
}

fn build_fits(live: &[Lease], build: &Option<(String, u64)>, cpu: u64) -> bool {
    build.as_ref().is_none_or(|(id, capacity)| {
        fits(
            scope_used(
                live.iter()
                    .filter(|lease| lease.build_id.as_ref() == Some(id)),
            )
            .cpu,
            cpu,
            *capacity,
        )
    })
}

#[derive(Debug, PartialEq, Eq)]
enum AdmissionDecision {
    Ordered,
    Passing,
    Waiting,
}

fn priority_exception(older: &Waiting, request: &Waiting) -> bool {
    older.priority == "low" && request.priority == "normal"
}

fn older_exception(live: &[Lease], older: &Waiting, request: &Waiting) -> Option<&'static str> {
    if priority_exception(older, request) {
        return Some("exception:priority");
    }
    if !build_fits(live, &older.build, older.resources().cpu) {
        return (older.build.as_ref().map(|(id, _)| id)
            != request.build.as_ref().map(|(id, _)| id))
        .then_some("exception:build");
    }
    if !older
        .domain_budget()
        .fits(domain_used(live, &older.domain), older.resources())
    {
        return (older.domain != request.domain).then_some("exception:domain");
    }
    None
}

/// Both physical axes must fit before queue order or safe passing is checked.
#[cfg(test)]
fn admission_decision<'a>(
    capacity: u64,
    live: &[Lease],
    waiting: impl Iterator<Item = &'a Waiting>,
    request: &Waiting,
) -> AdmissionDecision {
    admission_decision_traced(capacity, live, waiting, request, None)
}

/// Tracing observes the very same predicates and slack bounds as admission.
fn admission_decision_traced<'a>(
    capacity: u64,
    live: &[Lease],
    waiting: impl Iterator<Item = &'a Waiting>,
    request: &Waiting,
    trace: Option<&std::cell::RefCell<decisions::Trace>>,
) -> AdmissionDecision {
    let fallback = Resources {
        cpu: capacity,
        memory_bytes: capacity.saturating_mul(request.bytes_per_permit),
    };
    let budget = request.global_budget(fallback);
    let charge = request.resources();
    if !build_fits(live, &request.build, charge.cpu)
        || !request
            .domain_budget()
            .fits(domain_used(live, &request.domain), charge)
        || !budget.fits(scope_used(live.iter()), charge)
    {
        return AdmissionDecision::Waiting;
    }
    let mut older: Vec<_> = if let Some(trace) = trace {
        let all_older: Vec<_> = waiting
            .filter(|older| older.ticket < request.ticket)
            .collect();
        trace.borrow_mut().initialize(&all_older, live, request);
        all_older
            .into_iter()
            .filter(|older| !priority_exception(older, request))
            .collect()
    } else {
        waiting
            .filter(|older| older.ticket < request.ticket && !priority_exception(older, request))
            .collect()
    };
    older.sort_unstable_by_key(|older| older.ticket);
    let ordered_blocked = older
        .iter()
        .any(|older| older_exception(live, older, request).is_none());
    if !ordered_blocked {
        return AdmissionDecision::Ordered;
    }
    let scope_allows = |budget: Resources, build: Option<&str>, domain: Option<&str>| {
        let live: Vec<_> = live
            .iter()
            .filter(|lease| {
                build.is_none_or(|id| lease.build_id.as_deref() == Some(id))
                    && domain.is_none_or(|id| {
                        lease
                            .domain
                            .as_deref()
                            .is_none_or(|lease_id| lease_id == id)
                    })
            })
            .map(Lease::resources)
            .collect();
        let heads: Vec<_> = older
            .iter()
            .filter(|waiter| {
                build.is_none_or(|id| waiter.build.as_ref().map(|(id, _)| id.as_str()) == Some(id))
                    && domain.is_none_or(|id| waiter.domain == id)
            })
            .map(|waiter| waiter.resources())
            .collect();
        let tickets = trace.map(|_| {
            older
                .iter()
                .filter(|waiter| {
                    build.is_none_or(|id| {
                        waiter.build.as_ref().map(|(id, _)| id.as_str()) == Some(id)
                    }) && domain.is_none_or(|id| waiter.domain == id)
                })
                .map(|w| w.ticket)
                .collect::<Vec<_>>()
        });
        let scope = if build.is_some() {
            "build"
        } else if domain.is_some() {
            "domain"
        } else {
            "global"
        };
        let cpus: Vec<_> = live.iter().map(|r| r.cpu).collect();
        let head_cpus: Vec<_> = heads.iter().map(|r| r.cpu).collect();
        let memory: Vec<_> = live.iter().map(|r| r.memory_bytes).collect();
        let head_memory: Vec<_> = heads.iter().map(|r| r.memory_bytes).collect();
        // Subsets sufficient on both axes are a subset of those sufficient
        // on either one. Independent minima are therefore no larger than
        // the joint minimum; requiring both independent slack bounds is safe.
        pass_quantized(budget.cpu, &cpus, &head_cpus, charge.cpu, 1, |i, slack| {
            if let (Some(trace), Some(tickets)) = (trace, tickets.as_ref()) {
                trace
                    .borrow_mut()
                    .slack(tickets[i], scope, budget, "cpu", slack);
            }
        }) && (budget.memory_bytes == 0
            || pass_quantized(
                budget.memory_bytes,
                &memory,
                &head_memory,
                charge.memory_bytes,
                MEMORY_QUANTUM,
                |i, slack| {
                    if let (Some(trace), Some(tickets)) = (trace, tickets.as_ref()) {
                        trace
                            .borrow_mut()
                            .slack(tickets[i], scope, budget, "memory_bytes", slack);
                    }
                },
            ))
    };
    if !scope_allows(budget, None, None) {
        return AdmissionDecision::Waiting;
    }
    // Queue entries commonly share budgets. Evaluate each physical scope's
    // distinct ceilings once, as in the scalar policy.
    let mut checked_global = std::collections::BTreeSet::from([(budget.cpu, budget.memory_bytes)]);
    let mut checked = std::collections::BTreeSet::new();
    for waiter in &older {
        let global = waiter.global_budget(budget);
        if checked_global.insert((global.cpu, global.memory_bytes))
            && !scope_allows(global, None, None)
        {
            return AdmissionDecision::Waiting;
        }
        let domain = waiter.domain_budget();
        if waiter.domain == request.domain
            && checked.insert((false, domain.cpu, domain.memory_bytes))
            && !scope_allows(domain, None, Some(&waiter.domain))
        {
            return AdmissionDecision::Waiting;
        }
        if let (Some((id, ceiling)), Some((request_id, _))) = (&waiter.build, &request.build)
            && id == request_id
            && checked.insert((true, *ceiling, 0))
            && !scope_allows(
                Resources {
                    cpu: *ceiling,
                    memory_bytes: 0,
                },
                Some(id),
                None,
            )
        {
            return AdmissionDecision::Waiting;
        }
    }
    // Old writers still decide their own admission using scalar weights.
    // Vector slack cannot protect them: max(cpu, memory permits) is not
    // additive across leases with different dominant resources.
    let scalar_scope_allows = |capacity, unit, build: Option<&str>, domain: Option<&str>| {
        let weights: Vec<_> = live
            .iter()
            .filter(|lease| {
                build.is_none_or(|id| lease.build_id.as_deref() == Some(id))
                    && domain
                        .is_none_or(|id| lease.domain.as_deref().is_none_or(|other| other == id))
            })
            .map(|lease| weight_in_units(lease.weight, lease.bytes_per_permit, unit))
            .collect();
        let heads: Vec<_> = older
            .iter()
            .filter(|waiter| {
                build.is_none_or(|id| waiter.build.as_ref().map(|(id, _)| id.as_str()) == Some(id))
                    && domain.is_none_or(|id| waiter.domain == id)
            })
            .map(|waiter| weight_in_units(waiter.weight, waiter.bytes_per_permit, unit))
            .collect();
        let tickets = trace.map(|_| {
            older
                .iter()
                .filter(|waiter| {
                    build.is_none_or(|id| {
                        waiter.build.as_ref().map(|(id, _)| id.as_str()) == Some(id)
                    }) && domain.is_none_or(|id| waiter.domain == id)
                })
                .map(|w| w.ticket)
                .collect::<Vec<_>>()
        });
        let scope = if build.is_some() {
            "legacy:build"
        } else if domain.is_some() {
            "legacy:domain"
        } else {
            "legacy:global"
        };
        pass_quantized(
            capacity,
            &weights,
            &heads,
            weight_in_units(request.weight, request.bytes_per_permit, unit),
            1,
            |i, slack| {
                if let (Some(trace), Some(tickets)) = (trace, tickets.as_ref()) {
                    trace.borrow_mut().slack(
                        tickets[i],
                        scope,
                        Resources {
                            cpu: capacity,
                            memory_bytes: unit,
                        },
                        "weight",
                        slack,
                    );
                }
            },
        )
    };
    let mut scalar_checked = std::collections::BTreeSet::new();
    for waiter in &older {
        // New waiters need only the vector checks. An old scalar passer
        // reading their legacy weights is conservative: weight >= cpu and
        // weight * bytes_per_permit >= memory (subject to the existing
        // legacy capacity clamp for oversized demands).
        if waiter.cpu.is_some() && waiter.memory_bytes.is_some() {
            continue;
        }
        let unit = waiter.bytes_per_permit;
        let global = waiter.global_capacity.unwrap_or(capacity);
        if scalar_checked.insert((0, global, unit))
            && !scalar_scope_allows(global, unit, None, None)
        {
            return AdmissionDecision::Waiting;
        }
        if waiter.domain == request.domain
            && scalar_checked.insert((1, waiter.domain_capacity, unit))
            && !scalar_scope_allows(waiter.domain_capacity, unit, None, Some(&waiter.domain))
        {
            return AdmissionDecision::Waiting;
        }
        if let (Some((id, ceiling)), Some((request_id, _))) = (&waiter.build, &request.build)
            && id == request_id
            && scalar_checked.insert((2, *ceiling, unit))
            && !scalar_scope_allows(*ceiling, unit, Some(id), None)
        {
            return AdmissionDecision::Waiting;
        }
    }
    AdmissionDecision::Passing
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempt: Option<String>,
}

/// Exclusive occupancy of one invocation identity, machine-wide.
///
/// Concurrent builds duplicate work the permit pool can only pace: four CI
/// jobs compiling the same dependency graph run the same `rustc` invocation
/// four times, each burning a permit on a result the store is about to hold
/// anyway. A flight is a lock keyed by the invocation digest, taken just
/// before a real compilation starts. Whoever holds it compiles; anyone else
/// arriving at the same invocation blocks on the lock instead of the
/// compiler, then tries to restore the published result. Invocations can share
/// arguments while their input contents differ, so a restore can miss.
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
/// first of them compiles while holding the lock, giving later waiters a
/// chance to reuse its result. If the previous holder published a prediction
/// before releasing its lock but it does not restore, we release the lock before
/// compiling: different inputs must not serialize a queue of builds. Waiting
/// is bounded, and a timed-out build compiles without a flight. Predictions
/// are written atomically, including by compilers that released their lock.
pub(crate) struct Flight {
    lock: Option<fslock::LockFile>,
    path: PathBuf,
    adapter: &'static str,
    invocation: String,
    /// Whether another process held this invocation when we arrived.
    waited: bool,
    /// The prediction found under the lock, when it describes this invocation.
    inherited: Option<String>,
    published_while_waiting: bool,
    attempt: String,
    wait_duration: Duration,
}

impl Flight {
    /// Whether this process blocked behind another compiling the same thing.
    pub(crate) fn waited(&self) -> bool {
        self.waited
    }

    /// Whether the previous holder published the prediction found under the lock.
    pub(crate) fn published_while_waiting(&self) -> bool {
        self.published_while_waiting
    }

    /// Release occupancy while retaining the path for a later atomic `leave`.
    pub(crate) fn release(&mut self) {
        drop(self.lock.take());
    }

    /// Whether this flight gave up its lock after an unusable handoff.
    pub(crate) fn released(&self) -> bool {
        self.lock.is_none()
    }

    /// Finish the wrapper's restore attempt before returning or compiling.
    pub(crate) fn finish_restore(&mut self, hit: bool) {
        if !self.waited {
            return;
        }
        let outcome = if hit {
            "restore hit"
        } else if self.published_while_waiting() {
            self.release();
            "restore missed, flight released"
        } else {
            "restore missed, flight kept"
        };
        crate::session::report_shim_debug(&format!(
            "flight {}-{} waited {:.3}s: {outcome}",
            self.adapter,
            self.invocation,
            self.wait_duration.as_secs_f64()
        ));
    }

    /// The prediction the previous holder left, if any.
    pub(crate) fn inherited(&self) -> Option<&str> {
        self.inherited.as_deref()
    }

    /// Leave this compilation's prediction for whoever comes next.
    ///
    /// Written beside the lock file rather than into it, because the lock
    /// file's contents belong to the lock: releasing one truncates it. The
    /// write is atomic, so readers cannot see half a record even when a
    /// compiler has released its flight before publishing. It is not
    /// fsynced: a record lost to a crash costs the next build of this
    /// invocation a cold key, and a torn one fails to parse and is ignored.
    /// The disk round trip was paid once per compilation, on the shim's
    /// critical path, and build scripts that compile many C files at once
    /// queued behind each other on it.
    pub(crate) fn leave(&self, payload: &str) {
        if payload.len() > MAX_FLIGHT_PAYLOAD {
            return;
        }
        let record = FlightRecord {
            version: FLIGHT_VERSION,
            adapter: self.adapter.into(),
            invocation: self.invocation.clone(),
            payload: payload.into(),
            attempt: Some(self.attempt.clone()),
        };
        let written = serde_json::to_vec(&record)
            .map_err(eyre::Report::from)
            .and_then(|bytes| crate::util::write_advisory(&record_path(&self.path), &bytes));
        if let Err(error) = written {
            debug!("a flight prediction was not left behind: {error:#}");
        }
    }
}

/// Where the current holder's attempt identity lives, beside its lock.
fn attempt_path(lock_path: &Path) -> PathBuf {
    let mut path = lock_path.as_os_str().to_owned();
    path.push(".attempt");
    PathBuf::from(path)
}

/// Where one flight's prediction record lives, beside its lock.
fn record_path(lock_path: &Path) -> PathBuf {
    let mut path = lock_path.as_os_str().to_owned();
    path.push(".prediction");
    PathBuf::from(path)
}

/// How long an untouched flight is kept for.
///
/// A flight's files get a fresh timestamp every time its invocation
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
            .is_some_and(|extension| extension == "prediction" || extension == "attempt")
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
            let _ = std::fs::remove_file(attempt_path(&path));
            // Closed before the removal, because Windows will not delete a
            // file that is still open anywhere.
            drop(lock);
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Drop nextest history no test binary has updated for a month.
///
/// Every history file is written under the one [`TEST_HISTORY_LOCK`], which
/// is never removed, so a sweep holding it cannot race a writer: age is read
/// under the lock, and a file a test appended to a moment ago is no longer
/// stale by then. A lock somebody holds means tests are recording right now;
/// the sweep leaves everything for the next `mbx gc` rather than wait.
pub(crate) fn prune_test_history(cache_dir: &Path) {
    let scheduler = cache_dir.join(SCHEDULER_DIR);
    let Ok(entries) = std::fs::read_dir(scheduler.join(TEST_HISTORY_DIR)) else {
        return;
    };
    let Ok(mut lock) = fslock::LockFile::open(&scheduler.join(TEST_HISTORY_LOCK)) else {
        return;
    };
    if !lock.try_lock().is_ok_and(|taken| taken) {
        return;
    }
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let stale = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > FLIGHT_MAX_AGE);
        if stale {
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
/// runs. `None` means flights could not be used or the wait reached its
/// limit; the compilation just runs.
pub(crate) fn flight(adapter: &'static str, invocation: &str, label: &str) -> Option<Flight> {
    let pool = pool()?;
    match flight_at_inner(
        &pool.dir,
        adapter,
        invocation,
        label,
        FlightWait::default(),
        || {},
        crate::session::report_shim_warning,
    ) {
        Ok(flight) => Some(flight),
        Err(error) => {
            debug!("compiling outside a flight: {error:#}");
            None
        }
    }
}

const FLIGHT_POLL_MAX: Duration = Duration::from_millis(100);
const FLIGHT_WAIT_MIN: Duration = Duration::from_secs(10 * 60);

struct FlightWait {
    notice: Duration,
    limit: Option<Duration>,
}

impl Default for FlightWait {
    fn default() -> Self {
        Self {
            notice: SLOW_WAIT_NOTICE,
            limit: None,
        }
    }
}

fn flight_wait_limit(record: Option<&[u8]>) -> Duration {
    record
        .and_then(|bytes| serde_json::from_slice::<FlightRecord>(bytes).ok())
        .and_then(|record| serde_json::from_str::<serde_json::Value>(&record.payload).ok())
        .and_then(|payload| {
            payload
                .get("compiler_duration_ns")
                .and_then(|value| value.as_u64())
        })
        .map(|ns| {
            Duration::from_nanos(ns)
                .saturating_mul(4)
                .max(FLIGHT_WAIT_MIN)
        })
        .unwrap_or(FLIGHT_WAIT_MIN)
}

#[cfg(test)]
fn flight_at(pool_dir: &Path, adapter: &'static str, invocation: &str) -> Result<Flight> {
    flight_at_inner(
        pool_dir,
        adapter,
        invocation,
        invocation,
        FlightWait::default(),
        || {},
        |_| {},
    )
}

#[allow(clippy::too_many_arguments)]
fn flight_at_inner(
    pool_dir: &Path,
    adapter: &'static str,
    invocation: &str,
    label: &str,
    policy: FlightWait,
    on_wait: impl FnOnce(),
    mut warning: impl FnMut(&str),
) -> Result<Flight> {
    let _phase = crate::phase_timing::phase("flight_wait");
    let dir = pool_dir.join(INFLIGHT_DIR);
    std::fs::create_dir_all(&dir)
        .wrap_err_with(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{adapter}-{invocation}"));
    let mut lock = fslock::LockFile::open(&path)?;
    let waited = !lock.try_lock()?;
    let started = Instant::now();
    if waited {
        let previous = std::fs::read(record_path(&path)).ok();
        let limit = policy
            .limit
            .unwrap_or_else(|| flight_wait_limit(previous.as_deref()));
        crate::session::report_shim_debug(&format!("waiting for flight {adapter}-{invocation}"));
        on_wait();
        let mut delay = POLL_INITIAL;
        let mut notified = false;
        loop {
            let elapsed = started.elapsed();
            if elapsed >= limit {
                warning(&format!(
                    "stopped waiting after {:.1} minutes for another build compiling {label} with the same arguments; this build is compiling it itself",
                    elapsed.as_secs_f64() / 60.0
                ));
                crate::session::report_shim_debug(&format!(
                    "flight {adapter}-{invocation} waited {:.3}s: wait limit reached, compiling without a flight",
                    elapsed.as_secs_f64()
                ));
                eyre::bail!("flight {adapter}-{invocation} wait limit reached");
            }
            if !notified && elapsed >= policy.notice {
                warning(&format!(
                    "waited 1 minute for another build compiling {label} with the same arguments; this build will reuse its result if it matches, or compile when it finishes"
                ));
                notified = true;
            }
            if lock.try_lock()? {
                break;
            }
            std::thread::sleep(
                jittered(delay)
                    .min(FLIGHT_POLL_MAX)
                    .min(limit.saturating_sub(started.elapsed())),
            );
            delay = (delay * 2).min(FLIGHT_POLL_MAX);
        }
    }
    // Both reads happen under the lock. A holder may publish before we even
    // arrive, or spend time storing artifacts after publishing its prediction.
    let previous_attempt = std::fs::read_to_string(attempt_path(&path)).ok();
    let record = std::fs::read(record_path(&path))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<FlightRecord>(&bytes).ok())
        .filter(|record| {
            record.version == FLIGHT_VERSION
                && record.adapter == adapter
                && record.invocation == invocation
        });
    let published_while_waiting = waited
        && previous_attempt.as_ref().is_some_and(|attempt| {
            record.as_ref().and_then(|record| record.attempt.as_ref()) == Some(attempt)
        });
    let inherited = record.map(|record| record.payload);
    // Establish this attempt before returning to either compiler wrapper.
    // A failed write abandons coordination instead of compiling with a stale id.
    // A process killed before this write has not begun its compilation; the
    // last established attempt remains the one whose publication we inspected.
    let attempt = crate::util::random_string(32);
    crate::util::write_advisory(&attempt_path(&path), attempt.as_bytes())?;
    Ok(Flight {
        lock: Some(lock),
        path,
        adapter,
        invocation: invocation.to_string(),
        waited,
        inherited,
        published_while_waiting,
        attempt,
        wait_duration: started.elapsed(),
    })
}

/// Peak compiler RSS by crate name, machine-wide.
///
/// Read the way the savings tally is: anything unreadable is an empty ledger,
/// because refusing to schedule over a corrupt file would help nobody.
#[derive(Debug, Serialize, Deserialize)]
struct MemoryLedger {
    version: u8,
    crates: BTreeMap<String, u64>,
    /// Recent link measurements, newest last, whatever crate they came from.
    link_peaks: Vec<u64>,
    /// Average cores each test binary kept busy, by demand name.
    #[serde(default)]
    cpus: BTreeMap<String, u64>,
    /// Recent link measurements by code-generation profile, newest last.
    #[serde(default)]
    profile_link_peaks: BTreeMap<String, Vec<u64>>,
    /// Successful non-link samples, capped at the trust threshold. Peaks stay
    /// in `crates` so old shims preserve them, but dropping counts is safe.
    #[serde(default)]
    compile_samples: BTreeMap<String, u8>,
    /// Recent successful unwrapped peaks (plus doubled OOM peaks), newest last.
    #[serde(default)]
    memory_windows: BTreeMap<String, Vec<u64>>,
    /// Log-ratio scores. The distinct field ignores old standardized scores.
    #[serde(default, rename = "memory_log_ratios")]
    memory_residuals: BTreeMap<String, Vec<f64>>,
    /// Independent causal calibration for previously unsampled compile modes.
    #[serde(default)]
    sibling_residuals: Vec<f64>,
    /// Between-mode spread, recomputed from the surviving windows on read.
    #[serde(default = "memory::default_sibling_spread")]
    sibling_spread: f64,
    /// Derived once per read, never persisted alongside the raw observations.
    #[serde(skip)]
    pooled_spreads: BTreeMap<String, f64>,
}

impl Default for MemoryLedger {
    fn default() -> Self {
        Self {
            version: 0,
            crates: BTreeMap::new(),
            link_peaks: Vec::new(),
            cpus: BTreeMap::new(),
            profile_link_peaks: BTreeMap::new(),
            compile_samples: BTreeMap::new(),
            memory_windows: BTreeMap::new(),
            memory_residuals: BTreeMap::new(),
            sibling_residuals: Vec::new(),
            sibling_spread: memory::default_sibling_spread(),
            pooled_spreads: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TestMeasurement {
    peak: Option<u64>,
    cores: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default)]
struct TestHistoryHeader {
    count: u64,
    maximum: TestMeasurement,
}

#[derive(Debug, Clone, Copy, Default)]
struct TestHistoryLookup {
    test: Option<TestMeasurement>,
    header: TestHistoryHeader,
}

#[derive(Debug, Clone, Copy, Default)]
struct TestHistoryRecord {
    hash: u64,
    measurement: TestMeasurement,
}

/// The machine-wide permit pool one process draws from.
pub(crate) struct Pool {
    pub(crate) dir: PathBuf,
    capacity: u64,
    domain_capacity: u64,
    /// The original combined CPU/memory ceiling, written for old readers.
    legacy_domain_capacity: u64,
    memory_capacity: u64,
    domain_memory_capacity: u64,
    /// A per-Cargo-build ceiling, separate from the machine-wide capacity.
    build: Option<(String, u64)>,
    /// Original memory share, retained for fallback guesses and old readers.
    /// Zero disables memory predictions and the available-memory gate.
    bytes_per_permit: u64,
    events: bool,
    priority: SchedulerPriority,
    /// Probe for currently available memory, injectable for tests.
    available_memory: fn() -> Option<u64>,
    pressure: bool,
    pressure_probe: fn() -> crate::pressure::Reading,
    /// The memory domain `pressure_probe` reads, injectable for tests. Pools
    /// in different containers share leases but not pressure state.
    pressure_domain: &'static str,
    clock: fn() -> u64,
    /// Source of the counter in lease names, injectable for tests.
    lease_nonce: fn() -> u64,
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

/// Construct the pool used by `mbx reserve`.
///
/// Reservations are explicit user requests, unlike compiler shims where a
/// broken scheduler must fail open. Keep their priority separate from the
/// configured default: an unattended external job may deliberately yield to
/// normal builds without changing the priority of every later Cargo command.
pub(crate) fn reservation_pool(config: &Config, priority: SchedulerPriority) -> Option<Pool> {
    let scheduler = &config.scheduler;
    scheduler.enabled.then(|| {
        let permits = scheduler.permits();
        let mut pool = Pool::new(
            config.cache_dir.join(SCHEDULER_DIR),
            permits,
            bytes_per_permit(scheduler.memory_bytes, permits),
            priority,
        );
        pool.domain_capacity = scheduler.cpus.min(default_capacity()).max(1);
        pool.set_memory_capacity(scheduler.memory_bytes.unwrap_or(0));
        pool.events = config.events;
        pool.pressure = scheduler.pressure;
        pool
    })
}

fn resolve_pool() -> Option<Pool> {
    match std::env::var_os(SCHED_DIR_ENV) {
        Some(dir) if dir.is_empty() => None,
        Some(dir) => {
            let capacity = env_number(SCHED_SLOTS_ENV).unwrap_or_else(default_capacity);
            let mut pool = Pool::new(
                PathBuf::from(dir),
                capacity,
                env_number(SCHED_SLOT_BYTES_ENV).unwrap_or(0),
                std::env::var(SCHED_PRIORITY_ENV)
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default(),
            );
            pool.domain_capacity = env_number(SCHED_CPUS_ENV)
                .unwrap_or(capacity)
                .min(default_capacity())
                .max(1);
            pool.events = env_number(SCHED_EVENTS_ENV) != Some(0);
            pool.set_memory_capacity(
                env_number(SCHED_MEMORY_BYTES_ENV)
                    .unwrap_or_else(|| capacity.saturating_mul(pool.bytes_per_permit)),
            );
            pool.pressure = env_number(SCHED_PRESSURE_ENV) == Some(1);
            pool.build = std::env::var(SCHED_BUILD_ID_ENV)
                .ok()
                .filter(|id| !id.is_empty())
                .map(|id| {
                    let slots = env_number(SCHED_BUILD_SLOTS_ENV)
                        .unwrap_or(capacity)
                        .clamp(1, capacity.max(1));
                    (id, slots)
                });
            Some(pool)
        }
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
/// The environment for a Cargo build whose own jobserver may be narrower.
pub(crate) fn session_environment_with_jobs(
    config: &Config,
    cargo_jobs: Option<u64>,
) -> Vec<(String, String)> {
    let scheduler = &config.scheduler;
    if !scheduler.enabled || std::env::var_os(SCHED_DISABLE_ENV).is_some() {
        return vec![(SCHED_DIR_ENV.into(), String::new())];
    }
    let permits = scheduler.permits();
    let build_permits = cargo_jobs.map_or(permits, |jobs| permits.min(jobs.max(1)));
    vec![
        (
            SCHED_DIR_ENV.into(),
            config
                .cache_dir
                .join(SCHEDULER_DIR)
                .to_string_lossy()
                .into_owned(),
        ),
        (SCHED_EVENTS_ENV.into(), u8::from(config.events).to_string()),
        (SCHED_SLOTS_ENV.into(), permits.to_string()),
        (SCHED_CPUS_ENV.into(), scheduler.cpus.to_string()),
        (
            SCHED_MEMORY_BYTES_ENV.into(),
            scheduler.memory_bytes.unwrap_or(0).to_string(),
        ),
        (
            SCHED_PRESSURE_ENV.into(),
            u8::from(scheduler.pressure).to_string(),
        ),
        (
            SCHED_SLOT_BYTES_ENV.into(),
            bytes_per_permit(scheduler.memory_bytes, permits).to_string(),
        ),
        (
            SCHED_PRIORITY_ENV.into(),
            scheduler.priority.as_str().into(),
        ),
        (
            "MBX_SCHED_SUSPEND".into(),
            u8::from(scheduler.suspend && scheduler.pressure && scheduler.memory_bytes.is_some())
                .to_string(),
        ),
        (
            "MBX_SCHED_CGROUP_ROOT".into(),
            scheduler
                .cgroup_root
                .as_ref()
                .map_or_else(String::new, |path| path.to_string_lossy().into_owned()),
        ),
        (SCHED_BUILD_ID_ENV.into(), crate::util::random_string(12)),
        (SCHED_BUILD_SLOTS_ENV.into(), build_permits.to_string()),
    ]
}

fn bytes_per_permit(memory_bytes: Option<u64>, cpus: u64) -> u64 {
    memory_bytes.map_or(0, |memory| memory / cpus.max(1))
}

/// Record what the compiler that just ran cost, for the next admission.
///
/// Crates that fit inside one permit are not worth remembering -- except
/// links that succeeded, whose measurement is what retires the static floor
/// they are otherwise charged. A compiler killed by SIGKILL -- the Linux OOM
/// killer's signature -- is recorded *heavier* than it measured, so the retry
/// runs with more room instead of repeating the crash.
pub(crate) fn record_compiler_memory(demand: &Demand, status: &ExitStatus) {
    let Some(pool) = pool() else {
        return;
    };
    pool.note_compilation(demand, status);
}

/// Record what a finished test binary or nextest test cost, for its next
/// admission: its peak memory, and the average cores it kept busy when
/// `cores` says it ran long enough and completely enough to tell.
pub(crate) fn record_test_run(demand: &Demand, status: &ExitStatus, cores: Option<u64>) {
    let Some(pool) = pool() else {
        return;
    };
    let Some(identity) = &demand.per_test else {
        pool.note_compilation(demand, status);
        if let Some(cores) = cores
            && let Err(error) = pool.record_cpu(&demand.name, cores)
        {
            debug!("test CPU use was not recorded: {error:#}");
        }
        return;
    };
    let measured = child_peak_rss_bytes();
    pool.observe(demand, status, measured);
    // A successful test is a true reading of what it costs, so it is recorded
    // whatever it measured, the way a successful link is: that is what lets a
    // light test retire the unmeasured default. A failure can only show that
    // a test is heavy.
    let peak = measured.and_then(|measured| {
        ledger_peak(
            measured,
            oom_killed(status),
            true,
            status.success(),
            pool.bytes_per_permit,
        )
    });
    if let Err(error) = pool.record_test_measurement(identity, peak, cores) {
        debug!("test measurements were not recorded: {error:#}");
    }
}

/// Cores kept busy on average: CPU time over the wall time it took, to the
/// nearest whole core and never less than one. The nearest rather than the
/// next, because a suite that averaged just over one core is a one-core suite
/// with a helper thread, not a two-core one.
pub(crate) fn average_cores(cpu: Duration, wall: Duration) -> u64 {
    if wall.is_zero() {
        return 1;
    }
    (cpu.as_secs_f64() / wall.as_secs_f64()).round().max(1.0) as u64
}

// Explicit shared-pool budgets describe the host. Keep the process's own
// CPU/cgroup budget as a second ceiling across every build in its domain.
// A memory budget that fits in local memory is a local budget: it applies as
// configured, so only a larger one is held to 85% of the local memory.
fn local_memory_ceiling(budget: u64, local: Option<u64>) -> u64 {
    local.map_or(budget, |bytes| {
        if budget > bytes {
            bytes / 100 * 85
        } else {
            budget
        }
    })
}

// Preserve fe73826's combined scalar calculation, including permit rounding
// and its one-permit minimum. Old readers cannot see the independent ceilings.
fn legacy_local_ceiling(
    capacity: u64,
    bytes_per_permit: u64,
    cpus: u64,
    memory: Option<u64>,
) -> u64 {
    let memory = if bytes_per_permit == 0 {
        capacity
    } else {
        memory.map_or(capacity, |bytes| {
            // The budget admission enforces. Permit rounding puts it fewer
            // than `capacity` bytes below the configured value.
            let budget = capacity.saturating_mul(bytes_per_permit);
            if budget > bytes {
                (bytes / 100 * 85) / bytes_per_permit
            } else {
                capacity
            }
        })
    };
    capacity.min(cpus).min(memory).max(1)
}

/// Pure local ceilings, independently applied to each physical axis.
fn local_ceiling(cpu: u64, memory: u64, local_cpus: u64, local_memory: Option<u64>) -> Resources {
    Resources {
        cpu: cpu.min(local_cpus).max(1),
        memory_bytes: local_memory_ceiling(memory, local_memory),
    }
}

impl Pool {
    fn set_memory_capacity(&mut self, memory: u64) {
        self.memory_capacity = memory;
        self.domain_memory_capacity =
            local_memory_ceiling(memory, crate::util::memory_total_bytes());
    }

    fn effective_budget(&self) -> Resources {
        let yielding = self.priority == SchedulerPriority::Low && self.priority_wait_is_fresh();
        let memory_headroom = if yielding && self.memory_capacity > 0 {
            (self.memory_capacity / 4)
                .max(1)
                .min(self.memory_capacity - 1)
        } else {
            0
        };
        Resources {
            cpu: self.capacity - self.reserved(),
            memory_bytes: self.memory_capacity - memory_headroom,
        }
    }

    fn new(
        dir: PathBuf,
        capacity: u64,
        bytes_per_permit: u64,
        priority: SchedulerPriority,
    ) -> Self {
        let local_cpus = default_capacity();
        let local_memory = crate::util::memory_total_bytes();
        Self {
            dir,
            capacity: capacity.max(1),
            domain_capacity: local_ceiling(capacity, 0, local_cpus, None).cpu,
            legacy_domain_capacity: legacy_local_ceiling(
                capacity,
                bytes_per_permit,
                local_cpus,
                local_memory,
            ),
            memory_capacity: capacity.saturating_mul(bytes_per_permit),
            domain_memory_capacity: local_memory_ceiling(
                capacity.saturating_mul(bytes_per_permit),
                local_memory,
            ),
            build: None,
            bytes_per_permit,
            events: true,
            priority,
            available_memory: crate::util::memory_available_bytes,
            pressure: true,
            pressure_probe: crate::pressure::probe,
            pressure_domain: crate::pressure::domain(),
            clock: crate::pressure::now_ms,
            lease_nonce: next_lease_nonce,
        }
    }

    fn from_config(config: &Config) -> Option<Self> {
        let scheduler = &config.scheduler;
        let permits = scheduler.permits();
        scheduler.enabled.then(|| {
            let mut pool = Self::new(
                config.cache_dir.join(SCHEDULER_DIR),
                permits,
                bytes_per_permit(scheduler.memory_bytes, permits),
                scheduler.priority,
            );
            pool.domain_capacity = scheduler.cpus.min(default_capacity()).max(1);
            pool.set_memory_capacity(scheduler.memory_bytes.unwrap_or(0));
            pool.events = config.events;
            pool.pressure = scheduler.pressure;
            pool
        })
    }

    /// Wait for capacity to run one real compilation.
    ///
    /// `None` means the pool could not be used and the compilation should run
    /// unscheduled; it never means the compilation should not run.
    pub(crate) fn admit(&self, demand: &Demand) -> Option<Permit> {
        let permit = self.admit_unmarked(demand)?;
        if let Some((build, _)) = &self.build
            && pool().is_some_and(|global| std::ptr::eq(global, self))
        {
            // SAFETY: called on the compile path before this process spawns
            // the compiler that inherits it, and nothing else here mutates
            // the environment.
            unsafe { std::env::set_var(SCHED_PERMIT_HELD_ENV, build) };
        }
        Some(permit)
    }

    fn admit_unmarked(&self, demand: &Demand) -> Option<Permit> {
        if let Some((build, _)) = &self.build
            && std::env::var(SCHED_PERMIT_HELD_ENV).is_ok_and(|held| &held == build)
        {
            debug!(
                "an enclosing compiler already holds this build's permit; not asking for another ({})",
                demand.name
            );
            return None;
        }
        let _phase = crate::phase_timing::phase("permit_wait");
        demand.observation.set(None);
        let started = Instant::now();
        let mut state = AdmissionState::new(if self.events { (self.clock)() } else { 0 });
        let mut delay = POLL_INITIAL;
        let mut stamped: Option<Instant> = None;
        let mut complained = false;
        let mut ledger_cache = None;
        loop {
            // Polls run every 2 to 25 ms. Reuse the plan until history reloads;
            // its calibration features remain paired with the cached charge.
            let Plan {
                resources,
                weight,
                predicted,
                source,
            } = self.admission_plan(demand, &mut ledger_cache);
            // Features are refreshed with the plan and retained on admission.
            // The gate stops applying at its deadline so a machine that stays
            // short of memory delays this compilation rather than starving it.
            let gate = predicted.filter(|_| started.elapsed() < GATE_DEADLINE);
            match self.try_admit_state(resources, weight, gate, &mut state, Some(demand)) {
                Ok(Some(permit)) => {
                    demand.observation.set(Some(accuracy::Admission {
                        weight,
                        resources,
                        predicted,
                        source,
                        wait_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                        started: Instant::now(),
                    }));
                    return Some(permit);
                }
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
                if self.pressure && self.bytes_per_permit > 0 {
                    debug!(
                        "compiler admission may be waiting for memory pressure to recover ({})",
                        demand.name
                    );
                }
                debug!(
                    "waiting for {} cores and {} memory bytes ({})",
                    resources.cpu, resources.memory_bytes, demand.name
                );
            }
            std::thread::sleep(jittered(delay));
            delay = (delay * 2).min(POLL_MAX);
        }
    }

    /// Wait for and hold capacity for work mbx does not launch itself.
    ///
    /// Unlike compiler admission this propagates scheduler errors: silently
    /// running an explicitly reserved command without its reservation would
    /// defeat the command's purpose.
    /// The permits a reservation of `cpus` and `memory_bytes` holds.
    pub(crate) fn reservation_weight(
        &self,
        cpus: Option<u64>,
        memory_bytes: Option<u64>,
    ) -> Result<u64> {
        let cpu_weight = cpus.unwrap_or(0);
        let memory_weight = match memory_bytes {
            Some(_) if self.bytes_per_permit == 0 => {
                eyre::bail!("--memory needs scheduler.memory to be configured")
            }
            Some(memory) => memory.div_ceil(self.bytes_per_permit),
            None => 0,
        };
        let weight = cpu_weight.max(memory_weight);
        if weight == 0 {
            eyre::bail!("reserve needs --cpus, --memory, or both")
        }
        Ok(weight)
    }

    pub(crate) fn reserve(&self, cpus: Option<u64>, memory_bytes: Option<u64>) -> Result<Permit> {
        let weight = self.reservation_weight(cpus, memory_bytes)?;

        let started = Instant::now();
        let mut state = AdmissionState::new(if self.events { (self.clock)() } else { 0 });
        let mut delay = POLL_INITIAL;
        let mut stamped: Option<Instant> = None;
        loop {
            // Like compiler admission, a persistently low available-memory
            // reading must not turn an advisory gate into permanent starvation.
            let gate = memory_bytes.filter(|_| started.elapsed() < GATE_DEADLINE);
            if let Some(permit) = self.try_admit_state(
                Resources {
                    cpu: cpus.unwrap_or(0),
                    memory_bytes: memory_bytes.unwrap_or(0),
                },
                weight,
                gate,
                &mut state,
                None,
            )? {
                return Ok(permit);
            }
            if self.priority == SchedulerPriority::Normal
                && stamped.is_none_or(|last| last.elapsed() >= PRIORITY_STAMP_INTERVAL)
            {
                let _ = std::fs::write(self.dir.join(PRIORITY_WAIT_STAMP), b"");
                stamped = Some(Instant::now());
            }
            std::thread::sleep(jittered(delay));
            delay = (delay * 2).min(POLL_MAX);
        }
    }

    /// The permits this demand costs, and its predicted memory when known.
    #[cfg(test)]
    fn plan(&self, demand: &Demand) -> (u64, Option<u64>) {
        let (weight, predicted, _) = self.prediction(demand);
        (weight, predicted)
    }

    #[cfg(test)]
    fn prediction(&self, demand: &Demand) -> (u64, Option<u64>, &'static str) {
        let plan = self.demand_vector(demand);
        (plan.weight, plan.predicted, plan.source)
    }

    #[cfg(test)]
    fn demand_vector(&self, demand: &Demand) -> Plan {
        self.admission_plan(demand, &mut None)
    }

    fn admission_plan(&self, demand: &Demand, cache: &mut Option<LedgerCache>) -> Plan {
        // Nextest uses separate per-test history. CPU-only compilers have no
        // ledger prediction, while test binaries may still need learned cores.
        if demand.per_test.is_some() || (self.bytes_per_permit == 0 && !demand.learns_cpu) {
            return self.demand_vector_with_ledger(demand, &MemoryLedger::default());
        }
        cache
            .get_or_insert_with(LedgerCache::default)
            .prediction(&self.ledger_path(), |ledger| {
                self.demand_vector_with_ledger(demand, ledger)
            })
    }

    fn demand_vector_with_ledger(&self, demand: &Demand, ledger: &MemoryLedger) -> Plan {
        if let Some(identity) = &demand.per_test {
            let path = test_history_path(&self.dir.join(TEST_HISTORY_DIR), &identity.binary);
            let history = lookup_test_history(&path, &identity.binary, &identity.test);
            if history.header.count > 0 {
                refresh_test_history(&path);
            }
            identity.known.set(Some(history.test.unwrap_or_default()));
            let measurement = history
                .test
                .or_else(|| (history.header.count > 0).then_some(history.header.maximum));
            let cores = measurement
                .and_then(|measurement| measurement.cores)
                .unwrap_or(1)
                .max(1);
            if self.bytes_per_permit == 0 {
                return Plan {
                    resources: Resources {
                        cpu: cores,
                        memory_bytes: 0,
                    },
                    weight: cores.min(self.capacity),
                    predicted: None,
                    source: "unmeasured",
                };
            }
            let known_peak = measurement.and_then(|measurement| measurement.peak);
            let predicted = known_peak
                .unwrap_or_else(|| UNMEASURED_TEST_WEIGHT.saturating_mul(self.bytes_per_permit));
            let weight = predicted
                .div_ceil(self.bytes_per_permit)
                .max(cores)
                .clamp(1, self.capacity);
            return Plan {
                resources: Resources {
                    cpu: cores,
                    memory_bytes: if known_peak.is_some() {
                        predicted
                    } else {
                        predicted.max(weight.saturating_mul(self.bytes_per_permit))
                    },
                },
                weight,
                predicted: Some(predicted),
                source: if known_peak.is_some() {
                    "test-history"
                } else {
                    "unmeasured"
                },
            };
        }
        // What a test binary was measured using beats what it said or what
        // its harness would default to: libtest starts a thread per CPU
        // whether or not the tests keep them busy.
        let learned = demand
            .learns_cpu
            .then(|| ledger.cpus.get(&demand.name).copied())
            .flatten();
        let threads = learned
            .or(demand.threads)
            .unwrap_or_else(|| self.capacity.div_ceil(2))
            .max(1);
        let link_floor = demand.links.then_some(LINK_WEIGHT);
        if self.bytes_per_permit == 0 {
            return Plan {
                resources: Resources {
                    cpu: link_floor.unwrap_or(1).max(threads),
                    memory_bytes: 0,
                },
                weight: link_floor.unwrap_or(1).max(threads).min(self.capacity),
                predicted: None,
                source: "unmeasured",
            };
        }
        let key = ledger_key(&demand.name, demand.links);
        let (predicted, source) = if let Some(mode) = &demand.compile_mode {
            demand.memory_features.set(Some(
                memory::estimate(ledger, mode, "compile")
                    .map_or(memory::Features::COLD, |e| e.features),
            ));
            // One permit is what an unmeasured compile is charged.
            let prediction =
                memory::compile_prediction(ledger, &demand.name, mode, self.bytes_per_permit);
            demand.sibling_features.set(if prediction.1 == "sibling" {
                memory::sibling_estimate(ledger, &demand.name, self.bytes_per_permit)
                    .map(|e| e.features)
            } else {
                None
            });
            prediction
        } else if demand.learns_cpu {
            let peak = ledger.crates.get(&key).copied();
            (
                peak,
                if peak.is_some() {
                    "test-history"
                } else {
                    "unmeasured"
                },
            )
        } else if demand.links {
            let estimate = memory::estimate(ledger, &key, "link");
            demand.memory_features.set(Some(
                estimate.map_or(memory::Features::COLD, |e| e.features),
            ));
            memory::link_prediction(
                ledger,
                &key,
                demand.profile.as_deref(),
                LINK_WEIGHT.saturating_mul(self.bytes_per_permit),
            )
        } else {
            // cc:<filename> aliases different libraries' same-named sources.
            // All non-mode/non-link keys retain the never-lowered legacy rule.
            (ledger.crates.get(&key).copied(), "legacy")
        };
        // Project the current prediction into the legacy scalar weight.
        // Admission uses independent resources without this capacity clamp.
        // Old readers retain today's oversize under-charge across different
        // capacities; new readers use exact `cpu` and `memory_bytes` fields.
        let weight = predicted
            .map_or(1, |bytes| bytes.div_ceil(self.bytes_per_permit))
            .max(threads)
            .clamp(1, self.capacity);
        let memory_bytes =
            if predicted.is_none() || matches!(source, "unmeasured" | "default" | "link-floor") {
                predicted
                    .unwrap_or(0)
                    .max(weight.saturating_mul(self.bytes_per_permit))
            } else {
                predicted.unwrap_or(0)
            };
        Plan {
            resources: Resources {
                cpu: threads,
                memory_bytes,
            },
            weight,
            predicted,
            source,
        }
    }

    /// One admission attempt under the registrar lock.
    #[cfg(test)]
    fn try_admit(&self, weight: u64, predicted: Option<u64>) -> Result<Option<Permit>> {
        self.try_admit_queued(weight, predicted, &mut None)
    }

    #[cfg(test)]
    fn try_admit_queued(
        &self,
        weight: u64,
        predicted: Option<u64>,
        queued: &mut Option<QueueEntry>,
    ) -> Result<Option<Permit>> {
        self.try_admit_named(
            record_resources(None, None, weight, self.bytes_per_permit),
            weight,
            predicted,
            queued,
            None,
        )
    }

    #[cfg(test)]
    fn try_admit_named(
        &self,
        resources: Resources,
        weight: u64,
        predicted: Option<u64>,
        queued: &mut Option<QueueEntry>,
        demand: Option<&Demand>,
    ) -> Result<Option<Permit>> {
        let first_arrival_ms = queued
            .as_ref()
            .and_then(|entry| entry.record.as_ref()?.first_arrival_ms)
            .unwrap_or_else(|| (self.clock)());
        let mut state = AdmissionState {
            queued: queued.take(),
            first_arrival_ms,
            last_gate: None,
        };
        let result = self.try_admit_state(resources, weight, predicted, &mut state, demand);
        *queued = state.queued;
        result
    }

    fn try_admit_state(
        &self,
        resources: Resources,
        weight: u64,
        predicted: Option<u64>,
        state: &mut AdmissionState,
        demand: Option<&Demand>,
    ) -> Result<Option<Permit>> {
        let mut events = Vec::new();
        let result = self.try_admit_inner(resources, weight, predicted, state, demand, &mut events);
        // The registrar guard belongs to try_admit_inner and has been dropped,
        // including on errors. No log encoding, append or rotation holds it.
        if result.is_err() {
            self.dequeue(&mut state.queued, "error", &mut events);
        }
        self.write_decisions(events);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn try_admit_inner(
        &self,
        resources: Resources,
        weight: u64,
        predicted: Option<u64>,
        state: &mut AdmissionState,
        demand: Option<&Demand>,
        events: &mut Vec<decisions::Event>,
    ) -> Result<Option<Permit>> {
        let leases = self.dir.join(LEASES_DIR);
        std::fs::create_dir_all(&leases)
            .wrap_err_with(|| format!("failed to create {}", leases.display()))?;
        let registrar_path = self.dir.join(POOL_LOCK);
        let mut registrar = fslock::LockFile::open(&registrar_path)?;
        registrar.lock()?;
        let live = scan_leases(&leases)?;
        let now = (self.clock)();
        let first_arrival_ms = state.first_arrival_ms;
        // Broken sensing disables only this advisory gate, never CPU scheduling.
        let pressure_state = (self.pressure && self.bytes_per_permit > 0)
            .then(|| crate::pressure::domain_state(&self.dir, self.pressure_domain));
        let mut pressure = pressure_state
            .as_ref()
            .and_then(|path| crate::pressure::sample(path, now, self.pressure_probe).ok());
        if self.pressure
            && self.bytes_per_permit > 0
            && !live.is_empty()
            && crate::supervision::resumes_pending(&self.dir, now)
        {
            self.gated(state, "resumes-pending", events, || {
                self.waiting_record(resources, weight, demand, first_arrival_ms, u64::MAX)
            });
            return Ok(None);
        }
        if !live.is_empty()
            && let Some(pressure) = &pressure
            && pressure.valid
            && (pressure.pressured
                || (pressure.recovered_ms.is_some()
                    && pressure.last_admission_ms.is_some_and(|last| {
                        now.saturating_sub(last) < crate::pressure::INTERVAL_MS
                    })))
        {
            self.gated(state, "pressure", events, || {
                self.waiting_record(resources, weight, demand, first_arrival_ms, u64::MAX)
            });
            return Ok(None);
        }
        let budget = self.effective_budget();
        let capacity = budget.cpu;
        // The permit arithmetic bounds what history predicted; this bounds it
        // against the machine as it is right now, catching pressure the
        // ledger cannot see -- unmeasured crates already running, or memory
        // that was never the build's to spend.
        //
        // Only while somebody else holds a permit, though. With an idle pool
        // there is no compilation whose finishing would return the memory
        // this one is short of, so deferring would stall the one thing that
        // could proceed and wait out the deadline to reach the same answer.
        if !live.is_empty()
            && let Some(needed) = predicted
            && let Some(available) = (self.available_memory)()
            && available < needed
        {
            self.gated(state, "available-memory", events, || {
                self.waiting_record(resources, weight, demand, first_arrival_ms, u64::MAX)
            });
            return Ok(None);
        }
        let waiters = self.dir.join(WAITERS_DIR);
        std::fs::create_dir_all(&waiters)?;
        let waiting = scan_waiters_logged(&waiters, Some(self), events)?;
        let ticket = state.queued.as_ref().map_or(u64::MAX, |entry| entry.ticket);
        let mut request = self.waiting_record(resources, weight, demand, first_arrival_ms, ticket);
        request.global_capacity = Some(capacity);
        request.cpu_cap = Some(budget.cpu);
        request.mem_cap = Some(budget.memory_bytes);
        let trace = self
            .events
            .then(|| std::cell::RefCell::new(decisions::Trace::default()));
        let decision =
            admission_decision_traced(capacity, &live, waiting.iter(), &request, trace.as_ref());
        if decision == AdmissionDecision::Waiting {
            if let Some(entry) = &state.queued {
                let serialized_record = serde_json::to_vec(&request)?;
                if entry.serialized_record != serialized_record {
                    let guard = self.lock_record(&waiters, &serialized_record)?;
                    state.queued = Some(QueueEntry {
                        ticket,
                        _guard: guard,
                        serialized_record,
                        record: self.events.then(|| request.clone()),
                    });
                }
            } else {
                let ticket = waiting
                    .iter()
                    .map(|entry| entry.ticket)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| eyre::eyre!("scheduler queue exhausted"))?;
                let record = Waiting { ticket, ..request };
                let serialized_record = serde_json::to_vec(&record)?;
                state.queued = Some(QueueEntry {
                    ticket,
                    _guard: self.lock_record(&waiters, &serialized_record)?,
                    serialized_record,
                    record: self.events.then(|| record.clone()),
                });
                self.decision_event("enqueued", &record, None, events);
                state.last_gate = None;
            }
            return Ok(None);
        }
        let permit = self.grant(&leases, resources, weight, demand)?;
        if let Some(trace) = trace {
            self.admitted_event(&request, &live, decision, trace.into_inner(), events);
        }
        state.queued = None;
        state.last_gate = None;
        if let Some((path, state)) = pressure_state.as_ref().zip(pressure.as_mut()) {
            state.last_admission_ms = Some(now);
            let _ = crate::pressure::save(path, state);
        }
        Ok(Some(permit))
    }

    fn waiting_record(
        &self,
        resources: Resources,
        weight: u64,
        demand: Option<&Demand>,
        first_arrival_ms: u64,
        ticket: u64,
    ) -> Waiting {
        Waiting {
            first_arrival_ms: self.events.then_some(first_arrival_ms),
            pid: self.events.then(std::process::id),
            name: demand.map(|d| d.name.clone()),
            mode_key: demand.map(|d| {
                d.compile_mode
                    .clone()
                    .unwrap_or_else(|| ledger_key(&d.name, d.links))
            }),
            ticket,
            weight,
            priority: self.priority.as_str().into(),
            domain: self.pressure_domain.into(),
            domain_capacity: self.legacy_domain_capacity,
            domain_cpu_cap: Some(self.domain_capacity),
            global_capacity: Some(self.capacity),
            cpu_cap: Some(self.capacity),
            mem_cap: Some(self.memory_capacity),
            domain_memory_cap: Some(self.domain_memory_capacity),
            cpu: Some(resources.cpu),
            memory_bytes: Some(resources.memory_bytes),
            bytes_per_permit: self.bytes_per_permit,
            build: self.build.clone(),
        }
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
    fn grant(
        &self,
        leases: &Path,
        resources: Resources,
        weight: u64,
        demand: Option<&Demand>,
    ) -> Result<Permit> {
        let lease = serde_json::to_vec(&Lease {
            name: demand.map(|d| d.name.clone()),
            mode_key: demand.map(|d| {
                d.compile_mode
                    .clone()
                    .unwrap_or_else(|| ledger_key(&d.name, d.links))
            }),
            version: LEASE_VERSION,
            cpu: Some(resources.cpu),
            memory_bytes: Some(resources.memory_bytes),
            weight,
            priority: self.priority.as_str().into(),
            build_id: self.build.as_ref().map(|(id, _)| id.clone()),
            domain: Some(self.pressure_domain.into()),
            bytes_per_permit: self.bytes_per_permit,
        })?;
        self.lock_record(leases, &lease)
    }

    fn lock_record(&self, leases: &Path, record: &[u8]) -> Result<Permit> {
        let mut last = None;
        for _ in 0..LEASE_NAME_ATTEMPTS {
            let path = leases.join(format!(
                "{}-{}-{}",
                std::process::id(),
                process_token(),
                (self.lease_nonce)()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    std::io::Write::write_all(&mut file, record)
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

    /// CPU headroom for waiting normal-priority work. `effective_budget`
    /// withholds memory headroom too.
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
        let measured = child_peak_rss_bytes();
        self.observe(demand, status, measured);
        if self.bytes_per_permit == 0 {
            return;
        }
        let Some(measured) = measured else {
            return;
        };
        if demand.compile_mode.is_some() {
            if let Err(error) =
                self.record_compile(demand, measured, oom_killed(status), status.success())
            {
                debug!("compiler memory was not recorded: {error:#}");
            }
            return;
        }
        let oom = oom_killed(status);
        let peak = ledger_peak(
            measured,
            oom,
            demand.links,
            status.success(),
            self.bytes_per_permit,
        );
        let eligible =
            demand.links && !demand.learns_cpu && status.success() && demand.count_compile_samples;
        let sample_peak = if demand.links && !demand.learns_cpu && oom {
            peak
        } else {
            eligible.then_some(measured).filter(|p| *p > 0)
        };
        if peak.is_none() && sample_peak.is_none() {
            return;
        }
        let sample = sample_peak.map(|peak| memory::Sample {
            peak,
            features: demand.memory_features.take(),
            calibrate: eligible && !oom,
        });
        if let Err(error) = self.record_link_measurement(
            &ledger_key(&demand.name, demand.links),
            peak,
            demand.links,
            demand.profile.as_deref(),
            sample,
        ) {
            debug!("compiler memory was not recorded: {error:#}");
        }
    }

    fn record_compile(
        &self,
        demand: &Demand,
        measured: u64,
        oom: bool,
        succeeded: bool,
    ) -> Result<()> {
        let Some(key) = demand.compile_mode.as_ref() else {
            return Ok(());
        };
        if self.bytes_per_permit == 0 {
            return Ok(());
        }
        let succeeded = succeeded && demand.count_compile_samples;
        let peak = ledger_peak(measured, oom, false, succeeded, self.bytes_per_permit);
        let sample = if oom {
            peak
        } else {
            succeeded.then_some(measured).filter(|p| *p > 0)
        };
        if sample.is_none()
            && peak.is_none()
            && !read_ledger(&self.ledger_path())
                .crates
                .contains_key(&demand.name)
        {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut lock = fslock::LockFile::open(&self.dir.join(LEDGER_LOCK))?;
        lock.lock()?;
        let mut ledger = read_ledger(&self.ledger_path());
        if let Some(sample) = sample {
            memory::record_sibling(
                &mut ledger,
                sample,
                demand.sibling_features.take(),
                succeeded && !oom,
            );
            let features = demand
                .memory_features
                .take()
                .or_else(|| memory::estimate(&ledger, key, "compile").map(|e| e.features));
            memory::record(
                &mut ledger,
                key,
                "compile",
                sample,
                features,
                succeeded && !oom,
            );
        }
        let legacy_changed =
            update_legacy_compile(&mut ledger, demand, measured, peak, succeeded, oom);
        if sample.is_none() && !legacy_changed {
            return Ok(());
        }
        ledger.version = LEDGER_VERSION;
        trim_compile_ledger(&mut ledger);
        let mut contents = serde_json::to_vec(&ledger)?;
        contents.push(b'\n');
        crate::util::write_advisory(&self.ledger_path(), &contents)
    }

    fn ledger_path(&self) -> PathBuf {
        self.dir.join(LEDGER_FILE)
    }

    /// Raise a planned nextest test's history, skipping the lock and the read
    /// entirely when nothing measured exceeds what planning found.
    ///
    /// Most runs of most tests raise nothing, and every test in a run would
    /// otherwise queue on the one history lock to read its binary's file.
    /// A value another process raised since planning is caught under the
    /// lock, which is where the comparison is repeated.
    fn record_test_measurement(
        &self,
        identity: &TestIdentity,
        peak: Option<u64>,
        cores: Option<u64>,
    ) -> Result<()> {
        let raises = |value: Option<u64>, known: Option<u64>| {
            value.is_some_and(|value| known.is_none_or(|known| value > known))
        };
        if let Some(known) = identity.known.get()
            && !raises(peak, known.peak)
            && !raises(cores, known.cores)
        {
            return Ok(());
        }
        self.record_test_history(&identity.binary, &identity.test, peak, cores)
    }

    /// Raise a nextest test's memory or CPU history without changing suite
    /// records in the shared ledger.
    fn record_test_history(
        &self,
        binary: &str,
        test: &str,
        peak: Option<u64>,
        cores: Option<u64>,
    ) -> Result<()> {
        let dir = self.dir.join(TEST_HISTORY_DIR);
        std::fs::create_dir_all(&dir)
            .wrap_err_with(|| format!("failed to create {}", dir.display()))?;
        let path = test_history_path(&dir, binary);
        let mut lock = fslock::LockFile::open(&self.dir.join(TEST_HISTORY_LOCK))?;
        lock.lock()?;

        let hash = test_history_hash(test);
        let mut records = read_test_history_records(&path, binary);
        let position = records.binary_search_by_key(&hash, |record| record.hash);
        let current = position
            .as_ref()
            .ok()
            .map(|index| records[*index].measurement)
            .unwrap_or_default();
        let updated = raise_measurement(
            current,
            TestMeasurement {
                peak: peak.filter(|value| *value > 0),
                cores: cores.filter(|value| *value > 0),
            },
        );
        if updated == current {
            return Ok(());
        }

        match position {
            Ok(index) => records[index].measurement = updated,
            Err(index) => records.insert(
                index,
                TestHistoryRecord {
                    hash,
                    measurement: updated,
                },
            ),
        }
        enforce_test_history_cap(&mut records, MAX_TEST_HISTORY_ENTRIES);
        let contents = encode_test_history(binary, &records);
        crate::util::write_advisory(&path, &contents)
            .wrap_err_with(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Raise the recorded peak for one crate, never lowering it, and add a
    /// link's measurement to the machine-wide window whatever the entry does.
    #[cfg(test)]
    fn record_peak(&self, name: &str, peak: u64, links: bool) -> Result<()> {
        self.record_link_peak(name, peak, links, None)
    }

    #[cfg(test)]
    fn record_link_peak(
        &self,
        name: &str,
        peak: u64,
        links: bool,
        profile: Option<&str>,
    ) -> Result<()> {
        self.record_link_measurement(
            name,
            Some(peak),
            links,
            profile,
            links.then_some(memory::Sample {
                peak,
                features: None,
                calibrate: true,
            }),
        )
    }

    fn record_link_measurement(
        &self,
        name: &str,
        peak: Option<u64>,
        links: bool,
        profile: Option<&str>,
        sample: Option<memory::Sample>,
    ) -> Result<()> {
        // Plain native keys collide across libraries and never teach windows.
        let sample = sample.filter(|_| links);
        std::fs::create_dir_all(&self.dir)?;
        let mut lock = fslock::LockFile::open(&self.dir.join(LEDGER_LOCK))?;
        lock.lock()?;
        let path = self.ledger_path();
        let mut ledger = read_ledger(&path);
        if let Some(sample) = sample {
            let kind = if links { "link" } else { "compile" };
            let features = sample
                .features
                .or_else(|| memory::estimate(&ledger, name, kind).map(|e| e.features));
            memory::record(
                &mut ledger,
                name,
                kind,
                sample.peak,
                features,
                sample.calibrate,
            );
        }
        let legacy_changed =
            peak.is_some_and(|peak| update_legacy_link(&mut ledger, name, peak, links, profile));
        if sample.is_none() && !legacy_changed {
            return Ok(());
        }
        ledger.version = LEDGER_VERSION;
        trim_compile_ledger(&mut ledger);
        let mut contents = serde_json::to_vec(&ledger)?;
        contents.push(b'\n');
        crate::util::write_advisory(&path, &contents)
    }
}

impl Pool {
    /// Raise the recorded core count for one test binary, never lowering it.
    ///
    /// Never lowered for the reason memory is not, and one more: a suite
    /// measured while the machine was busy got fewer cores than it would
    /// have used, so the lower of two readings is the less trustworthy.
    fn record_cpu(&self, name: &str, cores: u64) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .wrap_err_with(|| format!("failed to create {}", self.dir.display()))?;
        let mut lock = fslock::LockFile::open(&self.dir.join(LEDGER_LOCK))?;
        lock.lock()?;
        let path = self.ledger_path();
        let mut ledger = read_ledger(&path);
        if ledger.cpus.get(name).is_some_and(|known| *known >= cores) {
            return Ok(());
        }
        ledger.version = LEDGER_VERSION;
        ledger.cpus.insert(name.to_string(), cores);
        while ledger.cpus.len() > MAX_LEDGER_ENTRIES {
            let smallest = ledger
                .cpus
                .iter()
                .min_by_key(|(_, cores)| **cores)
                .map(|(name, _)| name.clone())
                .expect("an overfull ledger has entries");
            ledger.cpus.remove(&smallest);
        }
        let mut contents = serde_json::to_vec(&ledger)?;
        contents.push(b'\n');
        crate::util::write_advisory(&path, &contents)
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

/// The rustc flags that decide how much memory a link takes: optimization,
/// LTO, codegen units and debug info. The last occurrence of each wins, as it
/// does in rustc.
fn link_profile(arguments: &[std::ffi::OsString]) -> String {
    let (mut opt, mut lto, mut units, mut debug) = (None, None, None, None);
    let mut arguments = arguments.iter().filter_map(|argument| argument.to_str());
    while let Some(argument) = arguments.next() {
        let setting = match argument {
            "-C" | "--codegen" => arguments.next(),
            "-O" => {
                opt = Some("3");
                continue;
            }
            "-g" => {
                debug = Some("2");
                continue;
            }
            _ => argument
                .strip_prefix("--codegen=")
                .or_else(|| argument.strip_prefix("-C")),
        };
        let Some(setting) = setting else { continue };
        let (key, value) = setting.split_once('=').unwrap_or((setting, ""));
        match key {
            "opt-level" => opt = Some(value),
            // A bare `-C lto` is fat LTO.
            "lto" => {
                lto = Some(match value {
                    "" | "yes" | "y" | "on" | "true" | "fat" => "fat",
                    "no" | "n" | "off" | "false" => "off",
                    other => other,
                })
            }
            "codegen-units" => units = Some(value),
            "debuginfo" => debug = Some(value),
            _ => {}
        }
    }
    format!(
        "opt{} lto-{} cgu{} debug{}",
        opt.unwrap_or("0"),
        lto.unwrap_or("default"),
        units.unwrap_or("-"),
        debug.unwrap_or("0"),
    )
}

/// Compile suffixes start with `[compile `, unlike link/profile and test
/// suffixes. Rust crate identifiers contain neither spaces nor brackets.
fn compile_profile(arguments: &[std::ffi::OsString]) -> String {
    let mut emits = std::collections::BTreeSet::new();
    let mut saw_emit = false;
    let mut crate_types = std::collections::BTreeSet::new();
    let mut targets = Vec::new();
    let mut test = false;
    let mut incremental = false;
    let mut threads = None;
    let mut args = arguments.iter().filter_map(|a| a.to_str());
    while let Some(arg) = args.next() {
        if arg == "--test" {
            test = true;
            continue;
        }
        let emit = if arg == "--emit" {
            args.next()
        } else {
            arg.strip_prefix("--emit=")
        };
        if let Some(value) = emit {
            saw_emit = true;
            emits.extend(value.split(',').filter_map(|kind| {
                let kind = kind.split('=').next()?;
                (kind != "dep-info").then_some(kind)
            }));
            continue;
        }
        let types = if arg == "--crate-type" {
            args.next()
        } else {
            arg.strip_prefix("--crate-type=")
        };
        if let Some(value) = types {
            crate_types.extend(value.split(','));
            continue;
        }
        let triple = if arg == "--target" {
            args.next()
        } else {
            arg.strip_prefix("--target=")
        };
        if let Some(value) = triple {
            targets.push(value);
            continue;
        }
        let setting = match arg {
            "-C" | "--codegen" => args.next(),
            _ => arg
                .strip_prefix("-C")
                .or_else(|| arg.strip_prefix("--codegen=")),
        };
        if let Some(value) = setting.and_then(|s| s.strip_prefix("incremental=")) {
            incremental = !value.is_empty();
        }
        let unstable = match arg {
            "-Z" => args.next(),
            _ => arg.strip_prefix("-Z"),
        };
        if let Some(value) = unstable.and_then(|s| s.strip_prefix("threads=")) {
            threads = Some(value);
        }
    }
    if !saw_emit {
        emits.insert("link");
    }
    format!(
        "emit={} crate-type={}{}{} {} incremental-{}{}",
        emits.into_iter().collect::<Vec<_>>().join(","),
        crate_types.into_iter().collect::<Vec<_>>().join(","),
        // Every target, in order, when there is more than one: such a compile
        // must not share a key with any single-target compile, whichever
        // target a rustc would honor.
        match targets.as_slice() {
            [] => String::new(),
            [target] => format!(" target={target:?}"),
            targets => format!(" targets={targets:?}"),
        },
        if test { " test" } else { "" },
        link_profile(arguments),
        if incremental { "on" } else { "off" },
        threads.map(|n| format!(" threads={n}")).unwrap_or_default()
    )
}

fn update_legacy_link(
    ledger: &mut MemoryLedger,
    name: &str,
    peak: u64,
    links: bool,
    profile: Option<&str>,
) -> bool {
    // The window records every link, including one whose per-crate entry
    // is already higher: it says what links cost around here lately, which
    // a measurement no worse than the last one still answers.
    if links {
        let window = match profile {
            Some(profile) => ledger
                .profile_link_peaks
                .entry(profile.to_string())
                .or_default(),
            None => &mut ledger.link_peaks,
        };
        window.push(peak);
        let surplus = window.len().saturating_sub(MAX_LINK_SAMPLES);
        window.drain(..surplus);
        while ledger.profile_link_peaks.len() > MAX_LEDGER_ENTRIES {
            ledger.profile_link_peaks.pop_first();
        }
    }
    let known = ledger.crates.get(name).is_some_and(|known| *known >= peak);
    if known && !links {
        // Nothing moved: the entry already stands higher and there is no
        // window entry to add. Rewriting the file identically would cost
        // every repeat compilation a write under this lock.
        return false;
    }
    if !known {
        ledger.crates.insert(name.to_string(), peak);
    }
    true
}

fn update_legacy_compile(
    ledger: &mut MemoryLedger,
    demand: &Demand,
    measured: u64,
    peak: Option<u64>,
    succeeded: bool,
    oom: bool,
) -> bool {
    let key = demand.compile_mode.as_ref().expect("compile mode");
    if peak.is_none() && !ledger.crates.contains_key(&demand.name) {
        return false;
    }
    let mode_peak = peak.unwrap_or(measured);
    let bare_known = ledger.crates.get(&demand.name).copied();
    let mode_known = ledger.crates.get(key).copied();
    let count = ledger.compile_samples.get(key).copied().unwrap_or(0);
    if bare_known.is_some_and(|bare| peak.is_none_or(|peak| peak <= bare))
        && mode_known.is_some_and(|known| known >= mode_peak)
        && (!succeeded || oom || count >= 3)
    {
        return false;
    }
    if let Some(peak) = peak {
        // A mode may survive without its bare entry (e.g. an old writer's
        // eviction). Recreating the fallback must cover every retained
        // mode, rather than cap a known mode below its measured maximum.
        let retained = if bare_known.is_none() {
            let prefix = format!("{} [compile ", demand.name);
            ledger
                .crates
                .iter()
                .filter(|(name, _)| name.starts_with(&prefix))
                .map(|(_, peak)| *peak)
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        let bare = ledger.crates.entry(demand.name.clone()).or_default();
        *bare = (*bare).max(peak).max(retained);
    }
    if !ledger.crates.contains_key(&demand.name) {
        return false;
    }
    // Even a light successful mode must learn when its crate is heavy.
    let mode = ledger.crates.entry(key.clone()).or_default();
    *mode = (*mode).max(mode_peak);
    if succeeded && !oom {
        let count = ledger.compile_samples.entry(key.clone()).or_default();
        *count = count.saturating_add(1).min(3);
    }
    true
}

#[cfg(test)]
// Independent old-reader prediction for compatibility assertions only.
fn legacy_compile_prediction(ledger: &MemoryLedger, name: &str, key: &str) -> (Option<u64>, bool) {
    let bare = ledger.crates.get(name).copied();
    if ledger.compile_samples.get(key).copied().unwrap_or(0) >= 3
        && let Some(peak) = ledger.crates.get(key).copied()
    {
        let padded = peak.saturating_add(peak.div_ceil(2));
        return (Some(bare.map_or(padded, |bare| padded.min(bare))), true);
    }
    (bare, false)
}

fn trim_compile_ledger(ledger: &mut MemoryLedger) {
    while ledger.crates.len() > MAX_LEDGER_ENTRIES {
        let smallest = ledger
            .crates
            .iter()
            .min_by_key(|(_, peak)| **peak)
            .map(|(name, _)| name.clone())
            .expect("overfull ledger");
        ledger.crates.remove(&smallest);
        ledger.compile_samples.remove(&smallest);
        // If a crate-wide fallback is evicted, remove its modes too. Counts
        // must never survive an evicted peak and authorize a new low reading.
        let prefix = format!("{smallest} [compile ");
        ledger.crates.retain(|key, _| !key.starts_with(&prefix));
    }
    ledger
        .compile_samples
        .retain(|key, _| ledger.crates.contains_key(key));
}

/// A read-only snapshot: files may include abandoned records until the next
/// admission reclaims them. Inspecting statistics must never acquire a lease.
#[derive(Debug, Serialize)]
pub(crate) struct SchedulerReport {
    enabled: bool,
    capacity: u64,
    memory_capacity: u64,
    bytes_per_permit: u64,
    entries: Vec<SchedulerEntry>,
    memory_calibration: Vec<MemoryCalibration>,
    activity: Vec<SchedulerActivity>,
    accuracy: accuracy::Summary,
    decisions: decisions::Summary,
}

#[derive(Debug, Serialize)]
struct SchedulerEntry {
    key: String,
    recorded_peak_bytes: u64,
    successful_samples: u8,
    n: usize,
    window_max_bytes: Option<u64>,
    bound_bytes: Option<u64>,
    q: Option<f64>,
    charge_bytes: Option<u64>,
    charge_permits: Option<u64>,
    charge_cpu: Option<u64>,
    source: &'static str,
}

#[derive(Debug, Serialize)]
struct MemoryCalibration {
    kind: &'static str,
    target_p: f64,
    sigma_pool: f64,
    residual_count: usize,
    /// A cold-key t multiplier (df=4) until calibrated; each key shows its q.
    q: f64,
    calibrated: bool,
}

#[derive(Debug, Serialize)]
struct SchedulerActivity {
    kind: &'static str,
    name: String,
    mode_key: String,
    build_id: String,
    pressure_domain: String,
    charged_permits: u64,
    charged_cpu: u64,
    charged_memory_bytes: u64,
    charged_bytes: u64,
    age_secs: Option<u64>,
}

/// Charges under the global scheduler settings. A workspace `.mbx.toml` can
/// change them for its own builds, and only `mbx cargo` resolves which
/// workspace that is; lease and waiter records keep what was really charged.
pub(crate) fn scheduler_report(config: &Config) -> SchedulerReport {
    let capacity = config.scheduler.permits();
    let mut report = scheduler_snapshot(
        &config.cache_dir.join(SCHEDULER_DIR),
        capacity,
        bytes_per_permit(config.scheduler.memory_bytes, capacity),
        config.scheduler.enabled,
    );
    report.memory_capacity = config.scheduler.memory_bytes.unwrap_or(0);
    report
}

fn scheduler_snapshot(
    dir: &Path,
    capacity: u64,
    bytes_per_permit: u64,
    enabled: bool,
) -> SchedulerReport {
    let ledger = read_ledger(&dir.join(LEDGER_FILE));
    let memory_calibration = memory::KINDS
        .into_iter()
        .map(|kind| {
            let residual_count = ledger.memory_residuals.get(kind).map_or(0, Vec::len);
            MemoryCalibration {
                kind,
                target_p: memory::TARGET_P,
                sigma_pool: memory::sigma_pool(&ledger, kind),
                residual_count,
                q: memory::quantile(&ledger, kind, 1),
                calibrated: residual_count >= 50,
            }
        })
        .chain(std::iter::once(MemoryCalibration {
            kind: "compile-sibling",
            target_p: memory::TARGET_P,
            sigma_pool: ledger.sibling_spread,
            residual_count: ledger.sibling_residuals.len(),
            q: memory::sibling_quantile(&ledger, 1),
            calibrated: ledger.sibling_residuals.len() >= 50,
        }))
        .collect();
    let mut activity = Vec::new();
    for (subdir, kind) in [(LEASES_DIR, "lease"), (WAITERS_DIR, "waiter")] {
        let Ok(files) = std::fs::read_dir(dir.join(subdir)) else {
            continue;
        };
        let mut paths: Vec<_> = files
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect();
        paths.sort();
        for path in paths {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let age_secs = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs());
            let record = if kind == "lease" {
                serde_json::from_slice::<Lease>(&bytes).ok().map(|l| {
                    (
                        l.resources(),
                        l.name,
                        l.mode_key,
                        l.build_id,
                        l.domain,
                        l.weight,
                        l.bytes_per_permit,
                    )
                })
            } else {
                serde_json::from_slice::<Waiting>(&bytes).ok().map(|w| {
                    (
                        w.resources(),
                        w.name,
                        w.mode_key,
                        w.build.map(|(id, _)| id),
                        Some(w.domain),
                        w.weight,
                        w.bytes_per_permit,
                    )
                })
            };
            if let Some((resources, name, mode, build, domain, weight, unit)) = record {
                let unknown = || "unknown".to_owned();
                activity.push(SchedulerActivity {
                    kind,
                    name: name.unwrap_or_else(unknown),
                    mode_key: mode.unwrap_or_else(unknown),
                    build_id: build.unwrap_or_else(unknown),
                    pressure_domain: domain.unwrap_or_else(unknown),
                    charged_permits: weight,
                    charged_cpu: resources.cpu,
                    charged_memory_bytes: resources.memory_bytes,
                    charged_bytes: weight.saturating_mul(unit),
                    age_secs,
                });
            }
        }
    }
    let keys: std::collections::BTreeSet<_> = ledger
        .crates
        .keys()
        .chain(ledger.memory_windows.keys())
        .chain(
            activity
                .iter()
                .filter(|a| a.mode_key.contains(" [compile "))
                .map(|a| &a.mode_key),
        )
        .collect();
    let entries = keys
        .into_iter()
        .map(|key| {
            let mode = key.split_once(" [compile ");
            let kind = memory::kind(key);
            let predictable = mode.is_some() || !key.contains(" [") || kind == "link";
            let estimate = (mode.is_some() || kind == "link")
                .then(|| memory::estimate(&ledger, key, kind))
                .flatten();
            let (charge, source) = if kind == "link" {
                let profile = key
                    .strip_suffix(" [link]")
                    .and_then(|name| name.split_once(" ["))
                    .map(|(_, profile)| profile.trim_end_matches(']'));
                memory::link_prediction(
                    &ledger,
                    key,
                    profile,
                    LINK_WEIGHT.saturating_mul(bytes_per_permit),
                )
            } else if let Some((name, _)) = mode {
                memory::compile_prediction(&ledger, name, key, bytes_per_permit)
            } else if predictable {
                (ledger.crates.get(key).copied(), "legacy")
            } else {
                (None, "see admission")
            };
            let estimate = estimate.or_else(|| {
                if source == "sibling" {
                    mode.and_then(|(name, _)| {
                        memory::sibling_estimate(&ledger, name, bytes_per_permit)
                    })
                } else {
                    None
                }
            });
            let charge_bytes = (bytes_per_permit > 0).then_some(charge).flatten();
            SchedulerEntry {
                key: key.clone(),
                recorded_peak_bytes: ledger.crates.get(key).copied().unwrap_or(0),
                successful_samples: ledger.compile_samples.get(key).copied().unwrap_or(0),
                n: estimate.map_or(0, |e| e.n),
                window_max_bytes: estimate.map(|e| e.window_max),
                bound_bytes: estimate.map(|e| e.bound),
                q: estimate.map(|e| e.q),
                charge_bytes,
                charge_cpu: predictable.then_some(1),
                charge_permits: charge_bytes
                    .map(|bytes| bytes.div_ceil(bytes_per_permit).clamp(1, capacity)),
                source: if bytes_per_permit == 0 && predictable {
                    "CPU-only"
                } else {
                    source
                },
            }
        })
        .collect();
    SchedulerReport {
        enabled,
        capacity,
        memory_capacity: capacity.saturating_mul(bytes_per_permit),
        bytes_per_permit,
        entries,
        memory_calibration,
        activity,
        accuracy: accuracy::Summary::read(dir),
        decisions: decisions::Summary::read(dir),
    }
}

impl SchedulerReport {
    pub(crate) fn text(&self) -> String {
        let mut lines = vec![format!(
            "Scheduler: {} ({} cores, {} memory budget, from the global settings)",
            if self.enabled { "enabled" } else { "disabled" },
            self.capacity,
            if self.bytes_per_permit == 0 {
                "unlimited".into()
            } else {
                crate::stats::size(self.memory_capacity)
            }
        )];
        for e in &self.entries {
            let charge = e.charge_bytes.zip(e.charge_cpu).map_or_else(
                || "see admission".to_owned(),
                |(bytes, cpu)| format!("{cpu} cores + {}", crate::stats::size(bytes)),
            );
            let source = if e.source == "see admission" {
                String::new()
            } else {
                format!(" ({})", e.source)
            };
            lines.push(format!(
                "  {}: peak={}, samples={}, n={}, window max={}, bound={}, charge={}{}",
                e.key,
                crate::stats::size(e.recorded_peak_bytes),
                e.successful_samples,
                e.n,
                e.window_max_bytes
                    .map_or_else(|| "unknown".into(), crate::stats::size),
                e.bound_bytes
                    .map_or_else(|| "unknown".into(), crate::stats::size),
                charge,
                source
            ));
        }
        if self.entries.is_empty() {
            lines.push("  No recorded memory peaks.".into());
        }
        for calibration in &self.memory_calibration {
            let spread_label = if calibration.kind == "compile-sibling" {
                "between_mode_spread"
            } else {
                "sigma_pool"
            };
            lines.push(format!(
                "  {}: {spread_label}={:.6}, residuals={}, p={}, q={:.6}{}",
                calibration.kind,
                calibration.sigma_pool,
                calibration.residual_count,
                calibration.target_p,
                calibration.q,
                if calibration.calibrated {
                    " (empirical)"
                } else {
                    " (t fallback at n=1; per-key df=4+n-1)"
                }
            ));
        }
        lines.push(
            "  Activity snapshot (may include abandoned records; no lease locks taken):".into(),
        );
        for a in &self.activity {
            lines.push(format!(
                "  {} {}: mode={}, build={}, domain={}, charge={} cores + {}, age={}",
                a.kind,
                a.name,
                a.mode_key,
                a.build_id,
                a.pressure_domain,
                a.charged_cpu,
                crate::stats::size(a.charged_memory_bytes),
                a.age_secs
                    .map_or_else(|| "unknown".into(), |s| format!("{s}s"))
            ));
        }
        if self.activity.is_empty() {
            lines.push("  No lease or waiter records.".into());
        }
        lines.push(self.accuracy.text());
        lines.push(self.decisions.text());
        lines.join("\n")
    }
}

// Atomic ledger replacements have new file identities even if timestamp and
// size match. A bounded lifetime also handles in-place writes and platforms
// without a portable file identity.
const LEDGER_CACHE_MAX_AGE: Duration = Duration::from_secs(1);

#[derive(PartialEq, Eq)]
struct LedgerStamp {
    modified: std::time::SystemTime,
    len: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl LedgerStamp {
    fn read(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok()?,
            len: metadata.len(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}

#[derive(Default)]
struct LedgerCache {
    stamp: Option<LedgerStamp>,
    loaded_at: Option<Instant>,
    ledger: MemoryLedger,
    plan: Option<Plan>,
}

impl LedgerCache {
    fn read(&mut self, path: &Path) -> &MemoryLedger {
        let stamp = LedgerStamp::read(path);
        if stamp != self.stamp
            || self
                .loaded_at
                .is_none_or(|loaded| loaded.elapsed() >= LEDGER_CACHE_MAX_AGE)
        {
            self.ledger = read_ledger(path);
            self.stamp = stamp;
            self.loaded_at = Some(Instant::now());
            self.plan = None;
        }
        &self.ledger
    }

    fn prediction(&mut self, path: &Path, predict: impl FnOnce(&MemoryLedger) -> Plan) -> Plan {
        self.read(path);
        *self.plan.get_or_insert_with(|| predict(&self.ledger))
    }
}

#[cfg(test)]
thread_local! {
    static LEDGER_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn read_ledger(path: &Path) -> MemoryLedger {
    #[cfg(test)]
    LEDGER_READS.with(|reads| reads.set(reads.get() + 1));
    let mut ledger: MemoryLedger = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    memory::prepare(&mut ledger);
    ledger
}

fn test_history_path(dir: &Path, binary: &str) -> PathBuf {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(binary.as_bytes());
    dir.join(format!("{}.hist", hex::encode(&digest[..8])))
}

fn test_history_hash(test: &str) -> u64 {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(test.as_bytes());
    u64::from_le_bytes(digest[..8].try_into().expect("SHA-256 has eight bytes"))
}

fn test_history_binary_tag(binary: &str) -> u64 {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(binary.as_bytes());
    u64::from_le_bytes(digest[8..16].try_into().expect("SHA-256 has sixteen bytes"))
}

/// Mark a history file as still in use, at most once per
/// [`TEST_HISTORY_REFRESH`].
///
/// Done without the history lock. A sweep that judged the file stale just
/// before this refresh can still delete it, which costs that binary one run
/// at the unmeasured estimate; skipping the refresh would cost it that every
/// month.
fn refresh_test_history(path: &Path) {
    let old = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > TEST_HISTORY_REFRESH);
    if old && let Ok(file) = std::fs::OpenOptions::new().append(true).open(path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

fn parse_test_history_header(
    bytes: &[u8],
    file_len: u64,
    binary: &str,
) -> Option<TestHistoryHeader> {
    if bytes.len() != TEST_HISTORY_HEADER_LEN as usize
        || file_len < TEST_HISTORY_HEADER_LEN
        || &bytes[..8] != TEST_HISTORY_MAGIC
        || read_u64(&bytes[8..16]) != test_history_binary_tag(binary)
    {
        return None;
    }
    Some(TestHistoryHeader {
        count: (file_len - TEST_HISTORY_HEADER_LEN) / TEST_HISTORY_RECORD_LEN,
        maximum: TestMeasurement {
            peak: nonzero(read_u64(&bytes[16..24])),
            cores: nonzero(read_u64(&bytes[24..32])),
        },
    })
}

fn read_test_history_header(file: &File, binary: &str) -> Option<TestHistoryHeader> {
    let file_len = file.metadata().ok()?.len();
    if file_len < TEST_HISTORY_HEADER_LEN {
        return None;
    }
    let mut bytes = [0; TEST_HISTORY_HEADER_LEN as usize];
    read_exact_at(file, &mut bytes, 0).ok()?;
    parse_test_history_header(&bytes, file_len, binary)
}

fn lookup_test_history(path: &Path, binary: &str, test: &str) -> TestHistoryLookup {
    let Ok(file) = File::open(path) else {
        return TestHistoryLookup::default();
    };
    let Some(header) = read_test_history_header(&file, binary) else {
        return TestHistoryLookup::default();
    };

    let hash = test_history_hash(test);
    let mut low = 0;
    let mut high = header.count;
    let mut found = None;
    while low < high {
        let middle = low + (high - low) / 2;
        let offset = TEST_HISTORY_HEADER_LEN + middle * TEST_HISTORY_RECORD_LEN;
        let mut bytes = [0; TEST_HISTORY_RECORD_LEN as usize];
        if read_exact_at(&file, &mut bytes, offset).is_err() {
            break;
        }
        let record = decode_test_history_record(&bytes);
        match record.hash.cmp(&hash) {
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
            std::cmp::Ordering::Equal => {
                found = Some(record.measurement);
                break;
            }
        }
    }
    TestHistoryLookup {
        test: found,
        header,
    }
}

fn read_test_history_records(path: &Path, binary: &str) -> Vec<TestHistoryRecord> {
    let Ok(contents) = std::fs::read(path) else {
        return Vec::new();
    };
    let Some(header) = contents
        .get(..TEST_HISTORY_HEADER_LEN as usize)
        .and_then(|bytes| parse_test_history_header(bytes, contents.len() as u64, binary))
    else {
        return Vec::new();
    };

    let (record_bytes, _) = contents[TEST_HISTORY_HEADER_LEN as usize..]
        .as_chunks::<{ TEST_HISTORY_RECORD_LEN as usize }>();
    let mut records: Vec<_> = record_bytes
        .iter()
        .take(header.count as usize)
        .map(|bytes| decode_test_history_record(bytes))
        .collect();
    records.sort_unstable_by_key(|record| record.hash);
    let mut unique: Vec<TestHistoryRecord> = Vec::with_capacity(records.len());
    for record in records {
        if let Some(previous) = unique
            .last_mut()
            .filter(|previous| previous.hash == record.hash)
        {
            previous.measurement = raise_measurement(previous.measurement, record.measurement);
        } else {
            unique.push(record);
        }
    }
    unique
}

fn decode_test_history_record(bytes: &[u8]) -> TestHistoryRecord {
    TestHistoryRecord {
        hash: read_u64(&bytes[..8]),
        measurement: TestMeasurement {
            peak: nonzero(read_u64(&bytes[8..16])),
            cores: nonzero(read_u64(&bytes[16..24])),
        },
    }
}

fn encode_test_history(binary: &str, records: &[TestHistoryRecord]) -> Vec<u8> {
    let maximum = records
        .iter()
        .fold(TestMeasurement::default(), |maximum, record| {
            raise_measurement(maximum, record.measurement)
        });
    let mut bytes = Vec::with_capacity(
        TEST_HISTORY_HEADER_LEN as usize + records.len() * TEST_HISTORY_RECORD_LEN as usize,
    );
    bytes.extend_from_slice(TEST_HISTORY_MAGIC);
    bytes.extend_from_slice(&test_history_binary_tag(binary).to_le_bytes());
    bytes.extend_from_slice(&maximum.peak.unwrap_or(0).to_le_bytes());
    bytes.extend_from_slice(&maximum.cores.unwrap_or(0).to_le_bytes());
    for record in records {
        bytes.extend_from_slice(&record.hash.to_le_bytes());
        bytes.extend_from_slice(&record.measurement.peak.unwrap_or(0).to_le_bytes());
        bytes.extend_from_slice(&record.measurement.cores.unwrap_or(0).to_le_bytes());
    }
    bytes
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("history field is eight bytes"))
}

fn nonzero(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt as _;
    file.read_at(buffer, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt as _;
    file.seek_read(buffer, offset)
}

#[cfg(not(any(unix, windows)))]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read(buffer)
}

fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !buffer.is_empty() {
        match read_at(file, buffer, offset) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => {
                offset = offset
                    .checked_add(read as u64)
                    .ok_or(std::io::ErrorKind::InvalidInput)?;
                buffer = &mut buffer[read..];
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn enforce_test_history_cap(records: &mut Vec<TestHistoryRecord>, limit: usize) -> bool {
    let mut capped = false;
    while records.len() > limit {
        let smallest = records
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                left.measurement
                    .peak
                    .unwrap_or(0)
                    .cmp(&right.measurement.peak.unwrap_or(0))
                    .then_with(|| {
                        left.measurement
                            .cores
                            .unwrap_or(0)
                            .cmp(&right.measurement.cores.unwrap_or(0))
                    })
                    .then_with(|| left.hash.cmp(&right.hash))
            })
            .map(|(index, _)| index)
            .expect("an overfull test history has entries");
        records.remove(smallest);
        capped = true;
    }
    capped
}

fn raise_measurement(current: TestMeasurement, incoming: TestMeasurement) -> TestMeasurement {
    TestMeasurement {
        peak: match (current.peak, incoming.peak) {
            (Some(current), Some(incoming)) => Some(current.max(incoming)),
            (current, incoming) => current.or(incoming),
        },
        cores: match (current.cores, incoming.cores) {
            (Some(current), Some(incoming)) => Some(current.max(incoming)),
            (current, incoming) => current.or(incoming),
        },
    }
}

/// Queue entries have the same kernel-owned lifetime as leases. A crashed or
/// cancelled waiter cannot leave a permanent head-of-line blocker.
#[cfg(test)]
fn scan_waiters(directory: &Path) -> Result<Vec<Waiting>> {
    scan_waiters_logged(directory, None, &mut Vec::new())
}

fn scan_waiters_logged(
    directory: &Path,
    pool: Option<&Pool>,
    events: &mut Vec<decisions::Event>,
) -> Result<Vec<Waiting>> {
    let mut waiting = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let mut lock = fslock::LockFile::open(&path)?;
        if lock.try_lock()? {
            if let Some(pool) = pool
                && pool.events
                && let Ok(Some(record)) = read_waiter(&path)
            {
                let age = waiter_mtime_ms(&path).map(|time| (pool.clock)().saturating_sub(time));
                pool.decision_event("abandoned", &record, age, events);
            }
            drop(lock);
            let _ = std::fs::remove_file(&path);
        } else if let Some(mut record) = read_waiter(&path)? {
            if pool.is_some_and(|pool| pool.events) && record.first_arrival_ms.is_none() {
                record.first_arrival_ms = waiter_mtime_ms(&path);
            }
            waiting.push(record);
        }
    }
    Ok(waiting)
}

fn waiter_mtime_ms(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

/// A live waiter's record, or `None` when it is empty or has vanished.
///
/// Records are completely written before their holder locks them. QueueEntry
/// empties its file while still locked; fslock then unlocks and truncates it,
/// and Permit removes it. A scan can see the lock held and then an empty file
/// or no file; both mean that waiter left the queue. Propagate non-empty
/// unreadable records through the scheduler's usual fail-open path.
fn read_waiter(path: &Path) -> Result<Option<Waiting>> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(None),
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The live lease weights, reclaiming any lease whose holder has died.
fn scan_leases(leases: &Path) -> Result<Vec<Lease>> {
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
        let lease = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Lease>(&bytes).ok())
            .map_or_else(
                || Lease {
                    name: None,
                    mode_key: None,
                    version: LEASE_VERSION,
                    weight: 1,
                    cpu: None,
                    memory_bytes: None,
                    priority: SchedulerPriority::Normal.as_str().into(),
                    build_id: None,
                    domain: None,
                    bytes_per_permit: 0,
                },
                |mut lease| {
                    lease.weight = lease.weight.max(1);
                    lease
                },
            );
        live.push(lease);
    }
    Ok(live)
}

/// What the ledger should remember about one finished compilation.
///
/// `None` means nothing worth writing: the compile fit comfortably inside a
/// single permit and carried no static weight worth correcting. An OOM kill
/// is recorded past what was measured -- the measurement is where the killer
/// stopped it, not what it needed -- and past one permit, so the retry
/// provably weighs more.
///
/// The link case is the reason this takes an outcome at all, and the rule is
/// deliberately asymmetric: a *successful* link is recorded whatever it
/// measured, because that is a true reading of what linking that crate costs
/// and it is what retires the static floor. A failed one is not. Whether an
/// invocation links is read off its crate type, so a binary that dies in type
/// checking never reached the linker at all -- and recording its small peak
/// would retire the floor using a number from a compilation that never did
/// the expensive thing, machine-wide, for every later build sharing the
/// cache. A failure is therefore held to the ordinary bar, where it can still
/// teach the ledger that something is *heavy* (a link that died reaching for
/// memory) but can never argue that anything is light.
fn ledger_peak(
    measured: u64,
    oom_killed: bool,
    links: bool,
    succeeded: bool,
    bytes_per_permit: u64,
) -> Option<u64> {
    if oom_killed {
        return Some(
            measured
                .saturating_mul(2)
                .max(bytes_per_permit.saturating_add(1)),
        );
    }
    if links && succeeded {
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
    let usage = children_usage()?;
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

/// CPU time, user and system, of every child this process has reaped.
///
/// A test runner shim reaps one test binary, and the counter folds in
/// whatever that binary ran in turn -- the builds a test starts are charged
/// to it, as its permit is.
#[cfg(unix)]
pub(crate) fn child_cpu_time() -> Option<Duration> {
    let usage = children_usage()?;
    let time = |value: libc::timeval| {
        Some(Duration::new(
            u64::try_from(value.tv_sec).ok()?,
            u32::try_from(value.tv_usec).ok()?.saturating_mul(1000),
        ))
    };
    Some(time(usage.ru_utime)? + time(usage.ru_stime)?)
}

#[cfg(not(unix))]
pub(crate) fn child_cpu_time() -> Option<Duration> {
    None
}

#[cfg(unix)]
fn children_usage() -> Option<libc::rusage> {
    // SAFETY: `getrusage` only writes into the zeroed struct it is given.
    unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        (libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage) == 0).then_some(usage)
    }
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

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
