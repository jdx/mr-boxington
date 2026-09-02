//! Configuration, resolved from environment variables over optional files.
//!
//! Precedence is environment, then the workspace policy, then the platform
//! configuration file, then defaults.

use crate::util::parse_duration;
use bytesize::ByteSize;
use eyre::{Context, Result, bail};
use mbx_cache_core::{RemoteCacheMode, S3ConditionalWrites};
use std::path::{Path, PathBuf};
use std::time::Duration;
use usage_config::{EnvLayer, FileLayer, FileScope, Layers};

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HTTP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_HTTP_RETRIES: i64 = 3;
const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long a managed target directory may sit unused before collection.
const DEFAULT_TARGET_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

const GIB: u64 = 1024 * 1024 * 1024;

/// The largest a budget nobody configured will grow to.
///
/// Scaling without a ceiling would hand a 4TB workstation hundreds of
/// gigabytes it never asked to give up.
const MAX_SCALED_BUDGET: u64 = 100 * GIB;

/// Scaled budgets are rounded down to a multiple of this.
///
/// 5% of a disk is a number like 16.65GiB, which reads like a measurement
/// rather than a decision. Rounding down to whole increments makes the budget
/// mbx reports back look like something chosen on purpose, and never rounds a
/// budget up past the share it was allowed.
const BUDGET_INCREMENT: u64 = 5 * GIB;

/// A default budget as a share of the disk, bounded at both ends.
///
/// A budget nobody configured should be generous on a large disk and modest on
/// a small one, but neither unbounded nor uselessly tiny: the floor keeps a
/// small disk from a cache too small to hit in. Sizes are stated in IEC units
/// so they read the way `mbx gc` reports them back.
#[derive(Debug, Clone, Copy)]
struct ScaledBudget {
    /// Percent of the whole disk.
    percent: u64,
    floor: u64,
    /// Used when the disk cannot be measured. Guessing a disk size would be
    /// worse: this is the budget mbx shipped with before it scaled them.
    fallback: u64,
}

const STORE_BUDGET: ScaledBudget = ScaledBudget {
    percent: 5,
    floor: 5 * GIB,
    fallback: 20 * GIB,
};

/// Twice the store's share: target directories hold the linked outputs of every
/// live checkout, and they are what fills a disk in practice.
const TARGET_BUDGET: ScaledBudget = ScaledBudget {
    percent: 10,
    floor: 10 * GIB,
    fallback: 30 * GIB,
};

impl ScaledBudget {
    fn resolve(self, disk_total_bytes: Option<u64>) -> u64 {
        let Some(total) = disk_total_bytes.filter(|total| *total > 0) else {
            return self.fallback;
        };
        let share = (total / 100).saturating_mul(self.percent);
        // Floors and the ceiling are whole increments themselves, so clamping
        // cannot reintroduce a ragged number.
        let whole = share - (share % BUDGET_INCREMENT);
        whole.clamp(self.floor, MAX_SCALED_BUDGET)
    }
}

/// Share of physical memory the compile scheduler budgets by default.
///
/// Deliberately less than the whole machine: the editor, the browser, and the
/// page cache are spending memory the budget cannot see, and a budget equal to
/// physical RAM would schedule compilations into exactly the pressure the
/// scheduler exists to avoid.
const SCHEDULER_MEMORY_PERCENT: u64 = 85;

/// Memory budget used when physical memory cannot be measured.
const SCHEDULER_MEMORY_FALLBACK: u64 = 16 * GIB;

/// The single declaration used to resolve and document mbx configuration.
#[derive(Debug, usage::Config)]
#[usage(file(
    path = "<config directory>/mbx/config.toml",
    scope = "global",
    format = "toml"
))]
pub(crate) struct RawConfig {
    /// Cache root.
    #[usage(env = "MBX_CACHE_DIR", default_note = "platform cache directory")]
    cache_dir: Option<PathBuf>,
    /// Write a JSON build report to this path.
    #[usage(env = "MBX_STATS_REPORT")]
    stats_report: Option<PathBuf>,
    /// Detail printed after a build: one line, the full breakdown, or nothing.
    #[usage(
        env = "MBX_SUMMARY",
        default = "short",
        choices("off", "short", "full")
    )]
    summary: String,
    /// Compile and consult the cache, then compare outputs.
    #[usage(env = "MBX_VERIFY", default = false, scope = "env")]
    verify: bool,
    /// Let local workspace members compile incrementally.
    #[usage(env = "MBX_INCREMENTAL", default = false)]
    incremental: bool,
    /// Compile crates that keep missing the cache with changed content
    /// incrementally, keeping their outputs out of the shared cache.
    #[usage(
        key = "learned_incremental",
        env = "MBX_LEARNED_INCREMENTAL",
        default = true
    )]
    _learned_incremental: bool,
    /// Share eligible compilations that read `OUT_DIR`.
    #[usage(env = "MBX_SHARE_OUT_DIR", default = true)]
    share_out_dir: bool,
    /// Cache executions of build scripts using Cargo's freshness inputs. This may
    /// also be set in workspace `.mbx.toml`; the environment variable wins.
    #[usage(env = "MBX_BUILD_SCRIPT_EXECUTION", default = true)]
    build_script_execution: bool,
    /// Record a per-compilation event stream for `mbx tui` to watch.
    #[usage(env = "MBX_EVENTS", default = true)]
    events: bool,
    /// Cache C and C++ compilations run by build scripts.
    #[usage(env = "MBX_CC", default = true)]
    cc: bool,
    /// How the savings line after a build reads.
    #[usage(
        env = "MBX_SAVINGS",
        default = "quips",
        choices("quips", "plain", "off")
    )]
    savings: String,
    /// Cache natively linked test binaries, executables, and proc macros. On macOS this
    /// also passes ld64 `-oso_prefix` so a debug-info link's debug map stops
    /// naming this checkout, which is what lets it cache. Supported on Linux,
    /// macOS, and Windows; a link mbx cannot describe exactly still links normally.
    #[usage(
        key = "cache_links",
        env = "MBX_CACHE_LINKS",
        default = true,
        scope = "env"
    )]
    cache_links: bool,
    /// Append the full reason for every bypassed compilation to this path.
    #[usage(key = "bypass_log", env = "MBX_BYPASS_LOG", scope = "env")]
    _bypass_log: Option<PathBuf>,
    /// Log filter for mbx's own diagnostics, such as `debug` or `mbx=trace`.
    #[usage(key = "log", env = "MBX_LOG", default = "info", scope = "env")]
    _log: String,
    #[usage(flatten)]
    remote: RawRemote,
    #[usage(flatten)]
    http: RawHttp,
    #[usage(flatten)]
    gc: RawGc,
    #[usage(flatten)]
    target: RawTarget,
    #[usage(flatten)]
    scheduler: RawScheduler,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "scheduler")]
struct RawScheduler {
    /// Coordinate real compilations machine-wide through a permit pool.
    #[usage(env = "MBX_SCHEDULER", default = true)]
    enabled: bool,
    /// Machine-wide concurrent compile permits.
    #[usage(env = "MBX_SCHEDULER_CPUS", default_note = "logical CPUs")]
    cpus: Option<i64>,
    /// Logical CPUs to leave free for the rest of the machine.
    #[usage(env = "MBX_SCHEDULER_RESERVE_CPUS", default = 0)]
    reserve_cpus: i64,
    /// Memory budget the permits divide, or "none" for plain CPU permits.
    #[usage(env = "MBX_SCHEDULER_MEMORY", default_note = "85% of physical memory")]
    memory: Option<String>,
    /// Permit priority of this build's compilations.
    #[usage(
        env = "MBX_SCHEDULER_PRIORITY",
        default = "normal",
        choices("normal", "low")
    )]
    priority: String,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "remote")]
struct RawRemote {
    /// Remote cache URL.
    #[usage(env = "MBX_REMOTE_URL", ty = "url")]
    url: Option<String>,
    /// Remote namespace; required when a URL is configured.
    #[usage(env = "MBX_REMOTE_NAMESPACE")]
    namespace: Option<String>,
    /// Bearer token for the remote cache.
    #[usage(env = "MBX_REMOTE_TOKEN")]
    token: Option<String>,
    /// File containing a bearer token.
    #[usage(env = "MBX_REMOTE_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    /// CI OIDC audience.
    #[usage(env = "MBX_REMOTE_OIDC_AUDIENCE")]
    oidc_audience: Option<String>,
    /// Remote access mode.
    #[usage(
        env = "MBX_REMOTE_MODE",
        default = "read-write",
        choices("read-write", "read-only", "write-only")
    )]
    mode: String,
    /// S3 endpoint for a store that is not AWS, such as MinIO or R2.
    #[usage(env = "MBX_REMOTE_S3_ENDPOINT", ty = "url")]
    s3_endpoint: Option<String>,
    /// S3 region; Cloudflare R2 uses "auto".
    #[usage(env = "MBX_REMOTE_S3_REGION")]
    s3_region: Option<String>,
    /// Address S3 buckets in the path rather than the host.
    #[usage(env = "MBX_REMOTE_S3_FORCE_PATH_STYLE")]
    s3_force_path_style: Option<bool>,
    /// How to treat an S3 store that does not implement conditional writes.
    #[usage(
        env = "MBX_REMOTE_S3_CONDITIONAL_WRITES",
        default = "auto",
        choices("auto", "required", "off")
    )]
    s3_conditional_writes: String,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "target")]
struct RawTarget {
    /// Let mbx place eligible target directories under the managed root.
    #[usage(env = "MBX_TARGET_VIEWS", default = true)]
    views: bool,
    /// Managed target root.
    #[usage(env = "MBX_TARGET_ROOT", default_note = "<cache_dir>/targets")]
    root: Option<PathBuf>,
    /// Managed-target budget, or "none". Live views are collected oldest-first.
    #[usage(
        env = "MBX_TARGET_MAX_SIZE",
        default_note = "10% of the cache disk, from 10GiB to 100GiB"
    )]
    max_size: Option<String>,
    /// Collect live managed targets unused this long, or "none".
    #[usage(env = "MBX_TARGET_MAX_AGE", default = "30d", ty = "duration")]
    max_age: String,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "gc")]
struct RawGc {
    /// Sweep after a build when collection is due.
    #[usage(env = "MBX_GC_AUTO", default = true)]
    auto: bool,
    /// Action-store and per-session remote-download budget.
    #[usage(
        env = "MBX_GC_MAX_SIZE",
        default_note = "5% of the cache disk, from 5GiB to 100GiB"
    )]
    max_size: Option<String>,
    /// Combined action-store and managed-target budget, or "none".
    #[usage(env = "MBX_GC_MAX_TOTAL_SIZE")]
    max_total_size: Option<String>,
    /// Minimum interval between automatic sweeps.
    #[usage(env = "MBX_GC_INTERVAL", default = "1h", ty = "duration")]
    interval: String,
}

#[derive(Debug, usage::Config)]
#[usage(prefix = "http")]
struct RawHttp {
    /// Connect and request timeout.
    #[usage(env = "MBX_HTTP_TIMEOUT", default = "30s", ty = "duration")]
    timeout: String,
    /// Deadline for one blob download, retries and backoff included.
    #[usage(env = "MBX_HTTP_DOWNLOAD_TIMEOUT", default = "10m", ty = "duration")]
    download_timeout: String,
    /// Request retries.
    #[usage(env = "MBX_HTTP_RETRIES", default = 3)]
    retries: i64,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub cache_dir: PathBuf,
    pub stats_report: Option<PathBuf>,
    pub verify: bool,
    /// Let cargo compile workspace members incrementally, rather than forcing
    /// `CARGO_INCREMENTAL=0` for the whole build.
    pub incremental: bool,
    /// Let a compilation that reads `OUT_DIR` be shared between checkouts.
    ///
    /// On by default: the compilation remaps generated sources to a stable
    /// placeholder, and mbx reads the outputs before publishing to fall back
    /// to a checkout-specific key when a crate embeds the literal path.
    pub share_out_dir: bool,
    /// Cache build-script execution when the script declares rerun inputs.
    pub build_script_execution: bool,
    /// Append a per-compilation event stream to the store, for `mbx tui`.
    ///
    /// On by default: one small buffered append per accounted compilation, with
    /// no flush of its own, against a compile or restore measured in
    /// milliseconds. Turn it off to keep the store free of build history.
    pub events: bool,
    /// Point build scripts at a caching `CC` and `CXX`.
    ///
    /// On by default: the shim never changes the compilation, and anything it
    /// cannot model exactly bypasses to the real compiler.
    pub cc: bool,
    pub remote: RemoteSettings,
    pub http: HttpSettings,
    pub gc: GcSettings,
    pub target: TargetSettings,
    pub scheduler: SchedulerSettings,
}

impl Config {
    /// A configuration with everything defaulted, for tests that care about one
    /// setting and should not have to spell out the rest.
    #[cfg(test)]
    pub fn for_test(cache_dir: &std::path::Path) -> Self {
        Self {
            cache_dir: cache_dir.to_path_buf(),
            stats_report: None,
            verify: false,
            incremental: false,
            share_out_dir: false,
            build_script_execution: false,
            events: false,
            // Off like the rest: a test that says nothing about C compilation
            // should not have compiler shims installed underneath it.
            cc: false,
            remote: Default::default(),
            http: Default::default(),
            gc: Default::default(),
            target: TargetSettings {
                views: true,
                root: cache_dir.join("targets"),
            },
            scheduler: Default::default(),
        }
    }
}

/// How real compilations draw from the machine-wide permit pool.
#[derive(Debug, Clone)]
pub struct SchedulerSettings {
    /// Whether compilations take permits at all.
    pub enabled: bool,
    /// Machine-wide concurrent compile permits.
    pub cpus: u64,
    /// Permits withheld from the configured CPU count.
    pub reserve_cpus: u64,
    /// Memory the permits divide; `None` leaves plain CPU permits.
    pub memory_bytes: Option<u64>,
    /// Priority of this build's compilations against other builds.
    pub priority: SchedulerPriority,
}

impl SchedulerSettings {
    /// Compile permits left after preserving capacity for interactive work.
    pub(crate) fn permits(&self) -> u64 {
        self.cpus.saturating_sub(self.reserve_cpus).max(1)
    }
}

/// The scheduling a hand-built `Config` gets: none.
///
/// Deliberately not the declared configuration default, which is enabled. A
/// `Config` assembled in code rather than resolved from the environment has
/// not been told where a machine-wide pool should live or how large the
/// machine is, and guessing would have tests and embedders contending over a
/// real pool they never asked for. Resolution always states every field.
impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            cpus: 1,
            reserve_cpus: 0,
            memory_bytes: None,
            priority: SchedulerPriority::Normal,
        }
    }
}

/// Whose compilations wait when the machine is contended.
///
/// `Low` is for builds nobody is sitting at -- CI on a shared box, an editor's
/// background check -- which leave a share of the permit pool free whenever a
/// normal-priority build is waiting for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchedulerPriority {
    #[default]
    Normal,
    Low,
}

impl SchedulerPriority {
    /// The spelling the configuration and the session environment use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
}

impl std::str::FromStr for SchedulerPriority {
    type Err = eyre::Report;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "normal" => Ok(Self::Normal),
            "low" => Ok(Self::Low),
            other => Err(eyre::eyre!("priority must be normal or low, not {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RemoteSettings {
    pub url: Option<String>,
    pub namespace: Option<String>,
    pub token: Option<String>,
    pub token_file: Option<PathBuf>,
    pub oidc_audience: Option<String>,
    pub mode: RemoteCacheMode,
    pub s3_endpoint: Option<String>,
    pub s3_region: Option<String>,
    pub s3_force_path_style: Option<bool>,
    pub s3_conditional_writes: S3ConditionalWrites,
}

#[derive(Debug, Clone)]
pub struct HttpSettings {
    pub timeout: Duration,
    pub download_timeout: Duration,
    pub retries: i64,
}

/// Where build outputs are written, and whether mbx places them.
#[derive(Debug, Clone)]
pub struct TargetSettings {
    pub views: bool,
    pub root: PathBuf,
}

/// How the store is kept inside its budget.
#[derive(Debug, Clone)]
pub struct GcSettings {
    pub auto: bool,
    pub max_bytes: u64,
    pub interval: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct RetentionSettings {
    pub target_max_bytes: Option<u64>,
    pub target_max_age: Option<Duration>,
    pub max_total_bytes: Option<u64>,
}

/// Settings only the command line consumes.
///
/// The library target is semver-checked, and `Config` is a public struct
/// callers can construct, so a knob the binary alone reads does not belong on
/// it: adding a field there is a breaking change to an API this crate does not
/// mean to offer.
#[derive(Debug, Clone)]
pub(crate) struct CliSettings {
    pub retention: RetentionSettings,
    pub savings: SavingsStyle,
    pub summary: SummaryStyle,
    /// Whether a churning crate may compile with its own incremental state.
    pub learned_incremental: bool,
    /// Whether natively linked programs may be cached.
    pub cache_links: bool,
}

/// Matches the declared defaults: a derived `Default` would silence the savings
/// line, which is the opposite of what an unconfigured machine should get.
impl Default for CliSettings {
    fn default() -> Self {
        Self {
            retention: RetentionSettings::default(),
            savings: SavingsStyle::default(),
            summary: SummaryStyle::default(),
            learned_incremental: true,
            cache_links: true,
        }
    }
}

/// How much of the per-build cache summary is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SummaryStyle {
    Off,
    #[default]
    Short,
    Full,
}

impl std::str::FromStr for SummaryStyle {
    type Err = eyre::Report;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "off" => Ok(Self::Off),
            "short" => Ok(Self::Short),
            "full" => Ok(Self::Full),
            other => Err(eyre::eyre!(
                "summary must be off, short, or full, not {other:?}"
            )),
        }
    }
}

/// How the line about accumulated savings reads.
///
/// The quips are the product's voice and the default; `plain` is the same
/// facts in the register of the `mbx[cache]:` and `mbx[gc]:` lines beside them, for
/// people who want their build logs to keep a straight face; `off` keeps the
/// totals without printing anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SavingsStyle {
    #[default]
    Quips,
    Plain,
    Off,
}

impl std::str::FromStr for SavingsStyle {
    type Err = eyre::Report;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "quips" => Ok(Self::Quips),
            "plain" => Ok(Self::Plain),
            "off" => Ok(Self::Off),
            other => Err(eyre::eyre!(
                "savings must be quips, plain, or off, not {other:?}"
            )),
        }
    }
}

/// The retention a run gets when nobody resolved configuration for it.
///
/// These are the unmeasured fallbacks rather than the disk-scaled budgets: a
/// `Default` cannot probe a disk it has not been told about. Collection still
/// happens, which is the property that matters -- a default that pruned nothing
/// would make every unconfigured path a leak.
impl Default for RetentionSettings {
    fn default() -> Self {
        Self {
            target_max_bytes: Some(TARGET_BUDGET.fallback),
            target_max_age: Some(DEFAULT_TARGET_MAX_AGE),
            max_total_bytes: None,
        }
    }
}

impl Default for GcSettings {
    fn default() -> Self {
        Self {
            auto: true,
            max_bytes: STORE_BUDGET.fallback,
            interval: DEFAULT_GC_INTERVAL,
        }
    }
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_HTTP_TIMEOUT,
            download_timeout: DEFAULT_HTTP_DOWNLOAD_TIMEOUT,
            retries: DEFAULT_HTTP_RETRIES,
        }
    }
}

impl Config {
    /// Load configuration for this machine.
    pub fn load() -> Result<Self> {
        Self::load_for_cli().map(|(config, _)| config)
    }

    pub(crate) fn load_for_cli() -> Result<(Self, CliSettings)> {
        let env = EnvLayer::from_process();
        let file = config_file_path().map(|path| FileLayer::at(path, FileScope::Global));
        Self::from_layers_for_cli(&env, file.as_ref())
    }

    fn from_layers_for_cli(
        env: &EnvLayer,
        file: Option<&FileLayer>,
    ) -> Result<(Self, CliSettings)> {
        Self::from_layers_measuring(
            env,
            file,
            crate::util::disk_total_bytes,
            crate::util::memory_total_bytes,
        )
    }

    fn from_layers_measuring(
        env: &EnvLayer,
        file: Option<&FileLayer>,
        measure_disk: impl Fn(&Path) -> Option<u64>,
        measure_memory: impl Fn() -> Option<u64>,
    ) -> Result<(Self, CliSettings)> {
        let mut layers = Layers::new().then(env);
        if let Some(file) = file {
            layers = layers.then(file);
        }
        let resolved = usage_config::resolve(RawConfig::SETTINGS_REGISTRY, layers)?;
        if !resolved.warnings.is_empty() {
            let problems = resolved
                .warnings
                .iter()
                .map(|warning| match &warning.origin {
                    Some(origin) => format!("{} ({})", warning.message, origin.describe()),
                    None => warning.message.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("invalid configuration:\n{problems}");
        }
        let raw = RawConfig::read(&resolved)?;
        Self::from_raw_measuring(raw, measure_disk, measure_memory)
    }

    /// Resolve settings, measuring the machine with the given probes.
    ///
    /// The probes are parameters so tests can state a disk or memory size
    /// rather than asserting against whatever machine happens to run them.
    fn from_raw_measuring(
        raw: RawConfig,
        measure_disk: impl Fn(&Path) -> Option<u64>,
        measure_memory: impl Fn() -> Option<u64>,
    ) -> Result<(Self, CliSettings)> {
        let cache_dir = raw.cache_dir.or_else(default_cache_dir).ok_or_else(|| {
            eyre::eyre!("could not determine a cache directory; set MBX_CACHE_DIR")
        })?;
        let target_root = match raw.target.root {
            Some(root) if root.is_absolute() => root,
            Some(root) => cache_dir.join(root),
            None => cache_dir.join("targets"),
        };
        let store_budget = raw
            .gc
            .max_size
            .as_deref()
            .map(parse_store_budget)
            .transpose()
            .wrap_err("invalid gc.max_size")?;
        let target_budget = raw
            .target
            .max_size
            .as_deref()
            .map(parse_optional_byte_size)
            .transpose()
            .wrap_err("invalid target.max_size")?;
        // Measured only where a budget actually needs scaling, so a fully
        // configured machine pays for no syscall at all. The two budgets are
        // measured separately because `target.root` can be on another volume,
        // and sizing a 4TB scratch disk from a 128GB home directory would prune
        // it to the floor.
        let store_disk = store_budget
            .is_none()
            .then(|| measure_disk(&cache_dir))
            .flatten();
        let target_disk = target_budget
            .is_none()
            .then(|| measure_disk(&target_root))
            .flatten();
        let retention = RetentionSettings {
            target_max_bytes: target_budget
                .unwrap_or_else(|| Some(TARGET_BUDGET.resolve(target_disk))),
            target_max_age: parse_optional_duration(&raw.target.max_age)
                .wrap_err("invalid target.max_age")?,
            max_total_bytes: raw
                .gc
                .max_total_size
                .as_deref()
                .map(parse_optional_byte_size)
                .transpose()
                .wrap_err("invalid gc.max_total_size")?
                .flatten(),
        };
        let mode = raw.remote.mode.parse().wrap_err("invalid remote.mode")?;
        let s3_conditional_writes = raw
            .remote
            .s3_conditional_writes
            .parse()
            .wrap_err("invalid remote.s3_conditional_writes")?;
        let http = HttpSettings {
            timeout: parse_duration(&raw.http.timeout).wrap_err("invalid http.timeout")?,
            download_timeout: parse_duration(&raw.http.download_timeout)
                .wrap_err("invalid http.download_timeout")?,
            retries: raw.http.retries,
        };
        let gc = GcSettings {
            auto: raw.gc.auto,
            max_bytes: store_budget.unwrap_or_else(|| STORE_BUDGET.resolve(store_disk)),
            interval: parse_duration(&raw.gc.interval).wrap_err("invalid gc.interval")?,
        };
        let target = TargetSettings {
            views: raw.target.views,
            root: target_root,
        };
        let scheduler_cpus = match raw.scheduler.cpus {
            Some(cpus) => u64::try_from(cpus)
                .ok()
                .filter(|cpus| *cpus > 0)
                .ok_or_else(|| eyre::eyre!("invalid scheduler.cpus: must be a positive count"))?,
            None => std::thread::available_parallelism().map_or(1, |cpus| cpus.get() as u64),
        };
        let scheduler_reserve_cpus = u64::try_from(raw.scheduler.reserve_cpus).map_err(|_| {
            eyre::eyre!("invalid scheduler.reserve_cpus: must be a non-negative count")
        })?;
        let scheduler_memory = match raw.scheduler.memory.as_deref() {
            Some(value) => parse_optional_byte_size(value).wrap_err("invalid scheduler.memory")?,
            // Measured only when the scheduler would use the answer, like the
            // disk budgets above.
            None => Some(
                raw.scheduler
                    .enabled
                    .then(&measure_memory)
                    .flatten()
                    .map_or(SCHEDULER_MEMORY_FALLBACK, |total| {
                        total / 100 * SCHEDULER_MEMORY_PERCENT
                    }),
            ),
        };
        let scheduler = SchedulerSettings {
            enabled: raw.scheduler.enabled,
            cpus: scheduler_cpus,
            reserve_cpus: scheduler_reserve_cpus,
            memory_bytes: scheduler_memory,
            priority: raw
                .scheduler
                .priority
                .parse()
                .wrap_err("invalid scheduler.priority")?,
        };
        let config = Self {
            cache_dir,
            gc,
            target,
            scheduler,
            stats_report: raw.stats_report,
            verify: raw.verify,
            incremental: raw.incremental,
            share_out_dir: raw.share_out_dir,
            build_script_execution: raw.build_script_execution,
            events: raw.events,
            cc: raw.cc,
            remote: RemoteSettings {
                url: raw.remote.url,
                namespace: raw.remote.namespace,
                token: raw.remote.token,
                token_file: raw.remote.token_file,
                oidc_audience: raw.remote.oidc_audience,
                mode,
                s3_endpoint: raw.remote.s3_endpoint,
                s3_region: raw.remote.s3_region,
                s3_force_path_style: raw.remote.s3_force_path_style,
                s3_conditional_writes,
            },
            http,
        };
        Ok((
            config,
            CliSettings {
                retention,
                savings: raw.savings.parse().wrap_err("invalid savings")?,
                summary: raw.summary.parse().wrap_err("invalid summary")?,
                learned_incremental: raw._learned_incremental,
                cache_links: raw.cache_links,
            },
        ))
    }

    /// Apply the deliberately small, safe policy surface from `.mbx.toml` at
    /// the resolved Cargo workspace root.
    pub fn apply_workspace_policy(&mut self, workspace_root: &Path) -> Result<()> {
        self.apply_workspace_policy_with(workspace_root, |name| std::env::var_os(name).is_some())
    }

    fn apply_workspace_policy_with(
        &mut self,
        workspace_root: &Path,
        environment_contains: impl Fn(&str) -> bool,
    ) -> Result<()> {
        let path = workspace_root.join(".mbx.toml");
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
            }
        };
        let document = contents
            .parse::<toml_edit::DocumentMut>()
            .wrap_err_with(|| format!("failed to parse {}", path.display()))?;

        for (key, value) in document.iter() {
            if key == "scheduler" {
                let table = value
                    .as_table()
                    .ok_or_else(|| eyre::eyre!("{}.scheduler must be a table", path.display()))?;
                for (scheduler_key, value) in table.iter() {
                    let setting = format!("scheduler.{scheduler_key}");
                    match scheduler_key {
                        "enabled" if !environment_contains("MBX_SCHEDULER") => {
                            self.scheduler.enabled = workspace_bool(&path, &setting, value)?;
                        }
                        "cpus" if !environment_contains("MBX_SCHEDULER_CPUS") => {
                            self.scheduler.cpus = workspace_count(&path, &setting, value, false)?;
                        }
                        "reserve_cpus" if !environment_contains("MBX_SCHEDULER_RESERVE_CPUS") => {
                            self.scheduler.reserve_cpus =
                                workspace_count(&path, &setting, value, true)?;
                        }
                        "memory" if !environment_contains("MBX_SCHEDULER_MEMORY") => {
                            let memory = value.as_str().ok_or_else(|| {
                                eyre::eyre!("{}.{} must be a string", path.display(), setting)
                            })?;
                            self.scheduler.memory_bytes = parse_optional_byte_size(memory)
                                .wrap_err_with(|| {
                                    format!("invalid {}.{setting}", path.display())
                                })?;
                        }
                        "priority" if !environment_contains("MBX_SCHEDULER_PRIORITY") => {
                            let priority = value.as_str().ok_or_else(|| {
                                eyre::eyre!("{}.{} must be a string", path.display(), setting)
                            })?;
                            self.scheduler.priority = priority.parse().wrap_err_with(|| {
                                format!("invalid {}.{setting}", path.display())
                            })?;
                        }
                        "enabled" | "cpus" | "reserve_cpus" | "memory" | "priority" => {}
                        _ => bail!(
                            "{} contains unsupported workspace setting {setting:?}; only scheduler.enabled, scheduler.cpus, scheduler.reserve_cpus, scheduler.memory, and scheduler.priority are allowed",
                            path.display()
                        ),
                    }
                }
                continue;
            }
            if !matches!(
                key,
                "incremental" | "share_out_dir" | "build_script_execution" | "cc"
            ) {
                bail!(
                    "{} contains unsupported workspace setting {key:?}; only incremental, share_out_dir, build_script_execution, cc, and scheduler are allowed",
                    path.display()
                );
            }
            let value = value
                .as_bool()
                .ok_or_else(|| eyre::eyre!("{}.{} must be a boolean", path.display(), key))?;
            match key {
                "incremental" if !environment_contains("MBX_INCREMENTAL") => {
                    self.incremental = value;
                }
                "share_out_dir" if !environment_contains("MBX_SHARE_OUT_DIR") => {
                    self.share_out_dir = value;
                }
                "build_script_execution" if !environment_contains("MBX_BUILD_SCRIPT_EXECUTION") => {
                    self.build_script_execution = value;
                }
                "cc" if !environment_contains("MBX_CC") => {
                    self.cc = value;
                }
                "incremental" | "share_out_dir" | "build_script_execution" | "cc" => {}
                _ => unreachable!("workspace policy keys were validated above"),
            }
        }
        Ok(())
    }

    /// Where cached actions and blobs live.
    pub fn store_dir(&self) -> PathBuf {
        self.cache_dir.join("actions")
    }
}

fn workspace_bool(path: &Path, setting: &str, value: &toml_edit::Item) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| eyre::eyre!("{}.{} must be a boolean", path.display(), setting))
}

fn workspace_count(
    path: &Path,
    setting: &str,
    value: &toml_edit::Item,
    zero_allowed: bool,
) -> Result<u64> {
    let count = value
        .as_integer()
        .and_then(|count| u64::try_from(count).ok())
        .filter(|count| zero_allowed || *count > 0);
    count.ok_or_else(|| {
        let requirement = if zero_allowed {
            "non-negative"
        } else {
            "positive"
        };
        eyre::eyre!(
            "{}.{} must be a {requirement} count",
            path.display(),
            setting
        )
    })
}

/// Resolve a byte size, written either plainly or with a unit.
///
/// Both IEC and SI spellings parse -- `20GiB` and `20GB` are different numbers,
/// and sizes are reported back in IEC, so the two are not interchangeable.
fn parse_byte_size(value: &str) -> Result<u64> {
    // `ByteSize`'s parse error is a bare `String`, so it cannot be a source.
    value
        .trim()
        .parse::<ByteSize>()
        .map(|size| size.as_u64())
        .map_err(|error| eyre::eyre!(error))
}

/// The spelling that turns a limit off outright.
///
/// A limit needs an off switch that is not "unset", because unset now means the
/// scaled default. Only this exact word does it: anything else that fails to
/// parse is still an error, so a typo cannot quietly disable collection.
const NO_LIMIT: &str = "none";

fn is_no_limit(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case(NO_LIMIT)
}

/// Parse the store budget, which alone has no off switch.
///
/// An unbounded action store is the problem collection exists to prevent, so
/// `"none"` is refused -- but it has to be refused in words, since the sibling
/// size settings accept it and `bytesize` would otherwise blame a float.
fn parse_store_budget(value: &str) -> Result<u64> {
    if is_no_limit(value) {
        eyre::bail!("the action store budget cannot be disabled; set a size such as 20GiB");
    }
    parse_byte_size(value)
}

fn parse_optional_byte_size(value: &str) -> Result<Option<u64>> {
    if is_no_limit(value) {
        return Ok(None);
    }
    parse_byte_size(value).map(Some)
}

fn parse_optional_duration(value: &str) -> Result<Option<Duration>> {
    if is_no_limit(value) {
        return Ok(None);
    }
    parse_duration(value).map(Some)
}

fn config_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("mbx").join("config.toml"))
}

fn default_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("mbx"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The disk size the tests below resolve scaled budgets against, so an
    /// assertion describes the scaling rule rather than the disk running it.
    const TEST_DISK: u64 = 400 * GIB;

    fn configured(file: Option<&str>, values: &[(&str, &str)]) -> Result<Config> {
        configured_for_cli(file, values).map(|(config, _)| config)
    }

    fn configured_for_cli(
        file: Option<&str>,
        values: &[(&str, &str)],
    ) -> Result<(Config, CliSettings)> {
        configured_on_disk(file, values, Some(TEST_DISK))
    }

    fn configured_retention(
        file: Option<&str>,
        values: &[(&str, &str)],
    ) -> Result<(Config, RetentionSettings)> {
        configured_for_cli(file, values).map(|(config, settings)| (config, settings.retention))
    }

    fn configured_on_disk(
        file: Option<&str>,
        values: &[(&str, &str)],
        disk_total_bytes: Option<u64>,
    ) -> Result<(Config, CliSettings)> {
        configured_measuring(file, values, move |_| disk_total_bytes)
    }

    /// Resolve with a stated size per path, for the budgets that are measured
    /// against different disks.
    fn configured_measuring(
        file: Option<&str>,
        values: &[(&str, &str)],
        measure_disk: impl Fn(&Path) -> Option<u64>,
    ) -> Result<(Config, CliSettings)> {
        let env = EnvLayer::new(
            values
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
        );
        let directory = tempfile::tempdir().unwrap();
        let file = file.map(|contents| {
            let path = directory.path().join("config.toml");
            std::fs::write(&path, contents).unwrap();
            FileLayer::at(path, FileScope::Global)
        });
        Config::from_layers_measuring(&env, file.as_ref(), measure_disk, || Some(32 * GIB))
    }

    #[test]
    fn environment_overrides_the_file() {
        let file = r#"
            cache_dir = "/from/file"
            [remote]
            url = "https://file.example"
            namespace = "file"
            mode = "read-only"
            s3_conditional_writes = "required"
            [http]
            timeout = "5s"
            retries = 9
            [gc]
            auto = false
            max_size = "1GiB"
            interval = "6h"
            [target]
            views = true
            root = "/from/file/targets"
            "#;
        let config = configured(
            Some(file),
            &[
                ("MBX_CACHE_DIR", "/from/env"),
                ("MBX_REMOTE_URL", "https://env.example"),
                ("MBX_REMOTE_MODE", "write-only"),
                ("MBX_HTTP_TIMEOUT", "250ms"),
                ("MBX_GC_MAX_SIZE", "2GiB"),
                ("MBX_TARGET_ROOT", "/from/env/targets"),
            ],
        )
        .unwrap();

        assert_eq!(config.cache_dir, PathBuf::from("/from/env"));
        assert_eq!(config.remote.url.unwrap(), "https://env.example");
        assert_eq!(config.remote.mode, RemoteCacheMode::WriteOnly);
        assert_eq!(config.http.timeout, Duration::from_millis(250));
        assert_eq!(config.gc.max_bytes, 2 * 1024 * 1024 * 1024);
        // Values absent from the environment still come from the file.
        assert_eq!(config.remote.namespace.unwrap(), "file");
        assert_eq!(
            config.remote.s3_conditional_writes,
            S3ConditionalWrites::Required
        );
        assert_eq!(config.http.retries, 9);
        assert!(!config.gc.auto);
        assert_eq!(config.gc.interval, Duration::from_secs(6 * 60 * 60));
        assert_eq!(config.target.root, PathBuf::from("/from/env/targets"));
        assert!(config.target.views);
    }

    #[test]
    fn defaults_apply_without_configuration() {
        let (config, retention) = configured_retention(None, &[]).unwrap();
        assert_eq!(
            config.remote.s3_conditional_writes,
            S3ConditionalWrites::Auto
        );
        assert_eq!(config.http.timeout, DEFAULT_HTTP_TIMEOUT);
        assert_eq!(config.http.download_timeout, DEFAULT_HTTP_DOWNLOAD_TIMEOUT);
        assert_eq!(config.http.retries, DEFAULT_HTTP_RETRIES);
        assert_eq!(config.remote.mode, RemoteCacheMode::ReadWrite);
        assert!(config.remote.url.is_none());
        assert!(!config.verify);
        assert!(!config.incremental);
        assert!(config.share_out_dir);
        assert!(config.build_script_execution);
        assert!(config.store_dir().ends_with("actions"));
        assert!(config.gc.auto, "collection runs until it is turned off");
        assert_eq!(config.gc.max_bytes, 20 * GIB, "5% of a 400GiB disk");
        assert_eq!(config.gc.interval, DEFAULT_GC_INTERVAL);
        assert!(
            config.target.views,
            "eligible target directories are managed"
        );
        assert_eq!(config.target.root, config.cache_dir.join("targets"));
        // Collection of live target directories is on by default; leaving these
        // unset is what used to let a disk fill up indefinitely.
        assert_eq!(
            retention.target_max_bytes,
            Some(40 * GIB),
            "10% of a 400GiB disk"
        );
        assert_eq!(retention.target_max_age, Some(DEFAULT_TARGET_MAX_AGE));
        assert_eq!(
            retention.max_total_bytes, None,
            "a combined budget stays opt-in"
        );
    }

    #[test]
    fn budgets_scale_with_the_disk_within_bounds() {
        let cases = [
            // A small disk lands on the floors rather than a cache too small
            // to ever hit in.
            (Some(32 * GIB), STORE_BUDGET.floor, TARGET_BUDGET.floor),
            (Some(400 * GIB), 20 * GIB, 40 * GIB),
            // 5% and 10% of 1TiB are 51.2 and 102.4 GiB: the first rounds down
            // to a whole increment, the second meets the ceiling.
            (Some(1_024 * GIB), 50 * GIB, MAX_SCALED_BUDGET),
            // A large disk stops at the ceiling on both counts.
            (Some(8_000 * GIB), MAX_SCALED_BUDGET, MAX_SCALED_BUDGET),
            // An unmeasurable disk uses the fixed fallbacks.
            (None, STORE_BUDGET.fallback, TARGET_BUDGET.fallback),
        ];
        for (disk, store, target) in cases {
            let (config, settings) = configured_on_disk(None, &[], disk).unwrap();
            let retention = settings.retention;
            assert_eq!(config.gc.max_bytes, store, "store budget for {disk:?}");
            assert_eq!(
                retention.target_max_bytes,
                Some(target),
                "target budget for {disk:?}"
            );
        }
    }

    #[test]
    fn a_scaled_budget_is_always_a_whole_increment() {
        // Awkward disk sizes are the point: 5% of 333GiB is 16.65GiB, which
        // should be reported as a number somebody could have chosen.
        for disk_gib in [17_u64, 63, 333, 500, 999, 1_500, 4_000] {
            for budget in [STORE_BUDGET, TARGET_BUDGET] {
                let resolved = budget.resolve(Some(disk_gib * GIB));
                assert_eq!(
                    resolved % BUDGET_INCREMENT,
                    0,
                    "{resolved} is not a whole increment for a {disk_gib}GiB disk"
                );
                assert!(resolved >= budget.floor);
                assert!(resolved <= MAX_SCALED_BUDGET);
            }
        }
    }

    #[test]
    fn rounding_never_hands_out_more_than_the_share() {
        // Rounded down, never to nearest: a budget must not exceed the share of
        // the disk it was allowed, floors aside.
        for disk_gib in [200_u64, 333, 617, 1_000] {
            let total = disk_gib * GIB;
            let resolved = STORE_BUDGET.resolve(Some(total));
            let share = (total / 100) * STORE_BUDGET.percent;
            assert!(
                resolved <= share.max(STORE_BUDGET.floor),
                "{resolved} exceeds the {share} share of a {disk_gib}GiB disk"
            );
        }
    }

    #[test]
    fn configured_budgets_outrank_the_disk() {
        let (config, settings) = configured_on_disk(
            None,
            &[("MBX_GC_MAX_SIZE", "3GiB"), ("MBX_TARGET_MAX_SIZE", "7GiB")],
            Some(8_000 * GIB),
        )
        .unwrap();
        let retention = settings.retention;

        assert_eq!(config.gc.max_bytes, 3 * GIB);
        assert_eq!(retention.target_max_bytes, Some(7 * GIB));
    }

    #[test]
    fn each_budget_is_sized_from_the_disk_that_holds_it() {
        // A scratch volume for targets is exactly why these are measured apart:
        // sizing a 4TiB disk from a 64GiB home directory would prune it to the
        // floor.
        let (config, settings) = configured_measuring(
            None,
            &[("MBX_CACHE_DIR", "/cache"), ("MBX_TARGET_ROOT", "/scratch")],
            |path| {
                Some(if path.starts_with("/scratch") {
                    4_000 * GIB
                } else {
                    64 * GIB
                })
            },
        )
        .unwrap();

        assert_eq!(config.gc.max_bytes, STORE_BUDGET.floor, "5% of 64GiB");
        assert_eq!(
            settings.retention.target_max_bytes,
            Some(MAX_SCALED_BUDGET),
            "10% of 4TiB, at the ceiling"
        );
    }

    #[test]
    fn the_store_budget_cannot_be_disabled() {
        let error = configured(None, &[("MBX_GC_MAX_SIZE", "none")])
            .expect_err("an unbounded store is what collection exists to prevent");
        let message = format!("{error:#}");
        assert!(
            message.contains("cannot be disabled"),
            "the error should say why rather than blame a float: {message}"
        );
    }

    #[test]
    fn none_turns_a_retention_limit_off() {
        let (_, retention) = configured_retention(
            None,
            &[
                ("MBX_TARGET_MAX_SIZE", "none"),
                ("MBX_TARGET_MAX_AGE", "NONE"),
                ("MBX_GC_MAX_TOTAL_SIZE", "None"),
            ],
        )
        .unwrap();

        assert_eq!(retention.target_max_bytes, None);
        assert_eq!(retention.target_max_age, None);
        assert_eq!(retention.max_total_bytes, None);
    }

    #[test]
    fn the_unconfigured_retention_default_still_collects() {
        // Paths that resolve no configuration must not silently stop pruning.
        let retention = RetentionSettings::default();
        assert_eq!(retention.target_max_bytes, Some(TARGET_BUDGET.fallback));
        assert_eq!(retention.target_max_age, Some(DEFAULT_TARGET_MAX_AGE));
    }

    #[test]
    fn reads_target_and_whole_cache_retention_limits() {
        let (_, retention) = configured_retention(
            None,
            &[
                ("MBX_TARGET_MAX_SIZE", "8GiB"),
                ("MBX_TARGET_MAX_AGE", "14d"),
                ("MBX_GC_MAX_TOTAL_SIZE", "12GiB"),
            ],
        )
        .unwrap();

        assert_eq!(retention.target_max_bytes, Some(8 * 1024 * 1024 * 1024));
        assert_eq!(
            retention.target_max_age,
            Some(Duration::from_secs(14 * 86_400))
        );
        assert_eq!(retention.max_total_bytes, Some(12 * 1024 * 1024 * 1024));
    }

    #[test]
    fn managed_targets_can_be_turned_off() {
        for value in ["0", "false", "no", "off", ""] {
            let config = configured(None, &[("MBX_TARGET_VIEWS", value)]).unwrap();
            assert!(
                !config.target.views,
                "MBX_TARGET_VIEWS={value:?} should disable managed targets"
            );
        }
    }

    #[test]
    fn relative_target_roots_are_anchored_to_the_cache_directory() {
        let config = configured(
            None,
            &[
                ("MBX_CACHE_DIR", "/cache"),
                ("MBX_TARGET_ROOT", "target-views"),
            ],
        )
        .unwrap();

        assert_eq!(config.target.root, PathBuf::from("/cache/target-views"));
    }

    #[test]
    fn rejects_unparseable_values() {
        assert!(configured(None, &[("MBX_REMOTE_MODE", "sideways")]).is_err());
        assert!(configured(None, &[("MBX_HTTP_TIMEOUT", "later")]).is_err());
        assert!(configured(None, &[("MBX_HTTP_RETRIES", "many")]).is_err());
        assert!(configured(None, &[("MBX_GC_MAX_SIZE", "lots")]).is_err());
        assert!(configured(None, &[("MBX_GC_MAX_TOTAL_SIZE", "lots")]).is_err());
        assert!(configured(None, &[("MBX_GC_INTERVAL", "later")]).is_err());
        assert!(configured(None, &[("MBX_TARGET_MAX_SIZE", "lots")]).is_err());
        assert!(configured(None, &[("MBX_TARGET_MAX_AGE", "later")]).is_err());
        // A budget that cannot be read must not be guessed at, and neither must
        // a switch: silently collecting to the wrong number, or not at all, is
        // worse than saying so.
        assert!(configured(None, &[("MBX_GC_AUTO", "maybe")]).is_err());
        assert!(configured(None, &[("MBX_TARGET_VIEWS", "sometimes")]).is_err());
    }

    #[test]
    fn collection_runs_until_it_is_turned_off() {
        for value in ["0", "false", "no", "off", ""] {
            let config = configured(None, &[("MBX_GC_AUTO", value)]).unwrap();
            assert!(!config.gc.auto, "MBX_GC_AUTO={value:?} should disable");
        }
        for value in ["1", "true", "yes", "on", "ON"] {
            let config = configured(None, &[("MBX_GC_AUTO", value)]).unwrap();
            assert!(config.gc.auto, "MBX_GC_AUTO={value:?} should enable");
        }
    }

    #[test]
    fn reads_a_budget_in_either_unit_convention() {
        // `20GB` and `20GiB` are different numbers and sizes are reported back
        // in IEC, so both spellings have to mean exactly what they say.
        for (value, expected) in [
            ("20GiB", 20 * 1024 * 1024 * 1024),
            ("20GB", 20_000_000_000),
            ("1024", 1024),
        ] {
            let config = configured(None, &[("MBX_GC_MAX_SIZE", value)]).unwrap();
            assert_eq!(config.gc.max_bytes, expected, "{value} should parse");
        }
    }

    #[test]
    fn the_environment_can_turn_off_what_the_file_turned_on() {
        let file = "incremental = true";
        let config = configured(Some(file), &[]).unwrap();
        assert!(config.incremental);
        let config = configured(Some(file), &[("MBX_INCREMENTAL", "0")]).unwrap();
        assert!(!config.incremental);
    }

    /// On by default: a crate nobody is editing never reaches the threshold, so
    /// the setting only matters to the one the developer is working in.
    #[test]
    fn learned_incremental_is_on_until_it_is_turned_off() {
        let (_, settings) = configured_for_cli(None, &[]).unwrap();
        assert!(settings.learned_incremental);

        let (_, settings) = configured_for_cli(None, &[("MBX_LEARNED_INCREMENTAL", "0")]).unwrap();
        assert!(!settings.learned_incremental);
    }

    #[test]
    fn workspace_policy_overrides_global_safe_settings() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(".mbx.toml"),
            "incremental = true\nshare_out_dir = true\nbuild_script_execution = true\n",
        )
        .unwrap();
        let mut config = configured(
            Some("incremental = false\nshare_out_dir = false\nbuild_script_execution = false"),
            &[],
        )
        .unwrap();

        config
            .apply_workspace_policy_with(directory.path(), |_| false)
            .unwrap();

        assert!(config.incremental);
        assert!(config.share_out_dir);
        assert!(config.build_script_execution);
    }

    #[test]
    fn workspace_policy_accepts_scheduler_settings() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(".mbx.toml"),
            "[scheduler]\nenabled = true\ncpus = 8\nreserve_cpus = 2\nmemory = \"6GiB\"\npriority = \"low\"\n",
        )
        .unwrap();
        let mut config = configured(None, &[("MBX_SCHEDULER", "false")]).unwrap();

        config
            .apply_workspace_policy_with(directory.path(), |name| name == "MBX_SCHEDULER")
            .unwrap();

        assert!(!config.scheduler.enabled, "the environment still wins");
        assert_eq!(config.scheduler.cpus, 8);
        assert_eq!(config.scheduler.reserve_cpus, 2);
        assert_eq!(config.scheduler.permits(), 6);
        assert_eq!(config.scheduler.memory_bytes, Some(6 * GIB));
        assert_eq!(config.scheduler.priority, SchedulerPriority::Low);
    }

    #[test]
    fn environment_overrides_workspace_scheduler_policy() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(".mbx.toml"),
            "[scheduler]\ncpus = 8\nreserve_cpus = 2\nmemory = \"6GiB\"\npriority = \"low\"\n",
        )
        .unwrap();
        let environment = [
            ("MBX_SCHEDULER_CPUS", "5"),
            ("MBX_SCHEDULER_RESERVE_CPUS", "1"),
            ("MBX_SCHEDULER_MEMORY", "4GiB"),
            ("MBX_SCHEDULER_PRIORITY", "normal"),
        ];
        let mut config = configured(None, &environment).unwrap();

        config
            .apply_workspace_policy_with(directory.path(), |name| {
                environment.iter().any(|(key, _)| *key == name)
            })
            .unwrap();

        assert_eq!(config.scheduler.cpus, 5);
        assert_eq!(config.scheduler.reserve_cpus, 1);
        assert_eq!(config.scheduler.memory_bytes, Some(4 * GIB));
        assert_eq!(config.scheduler.priority, SchedulerPriority::Normal);
    }

    #[test]
    fn workspace_policy_validates_scheduler_settings() {
        for setting in [
            "[scheduler]\ncpus = 0",
            "[scheduler]\nreserve_cpus = -1",
            "[scheduler]\nmemory = \"plenty\"",
            "[scheduler]\npriority = \"urgent\"",
            "[scheduler]\nunknown = true",
        ] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join(".mbx.toml"), setting).unwrap();
            let mut config = configured(None, &[]).unwrap();
            assert!(
                config
                    .apply_workspace_policy_with(directory.path(), |_| false)
                    .is_err(),
                "{setting:?} should be rejected"
            );
        }
    }

    #[test]
    fn environment_overrides_workspace_policy() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(".mbx.toml"),
            "incremental = true\nshare_out_dir = true\nbuild_script_execution = true\n",
        )
        .unwrap();
        let mut config = configured(
            None,
            &[
                ("MBX_SHARE_OUT_DIR", "0"),
                ("MBX_BUILD_SCRIPT_EXECUTION", "0"),
            ],
        )
        .unwrap();

        config
            .apply_workspace_policy_with(directory.path(), |_| true)
            .unwrap();

        assert!(!config.incremental);
        assert!(!config.share_out_dir);
        assert!(!config.build_script_execution);
    }

    #[test]
    fn workspace_policy_rejects_every_setting_outside_the_allowlist() {
        for setting in [
            "cache_dir = \"elsewhere\"",
            "verify = true",
            "[remote]\nurl = \"https://example.com\"",
            // Credentials and the store they authenticate to are a machine's
            // business, never a repository's.
            "[remote]\ns3_endpoint = \"https://store.example.com\"",
            "[remote]\ns3_region = \"us-west-2\"",
            "[gc]\nauto = false",
            "[target]\nviews = false",
        ] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join(".mbx.toml"), setting).unwrap();
            let mut config = configured(None, &[]).unwrap();
            let error = config
                .apply_workspace_policy_with(directory.path(), |_| false)
                .unwrap_err();
            assert!(
                error.to_string().contains("unsupported workspace setting"),
                "{error}"
            );
        }
    }

    #[test]
    fn missing_workspace_policy_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = configured(None, &[]).unwrap();
        config
            .apply_workspace_policy_with(directory.path(), |_| false)
            .unwrap();
    }

    #[test]
    fn unknown_file_keys_are_rejected() {
        let error = configured(Some("not_a_setting = true"), &[]).unwrap_err();
        assert!(error.to_string().contains("not_a_setting"), "{error}");
    }

    #[test]
    fn verify_is_off_for_empty_and_zero() {
        for value in ["", "0"] {
            let config = configured(None, &[("MBX_VERIFY", value)]).unwrap();
            assert!(!config.verify, "MBX_VERIFY={value:?} should not enable");
        }
        let config = configured(None, &[("MBX_VERIFY", "1")]).unwrap();
        assert!(config.verify);
    }

    #[test]
    fn savings_default_to_quips_with_plain_and_off_as_choices() {
        let (_, settings) = configured_for_cli(None, &[]).unwrap();
        assert_eq!(
            settings.savings,
            SavingsStyle::Quips,
            "an unconfigured machine gets the voice"
        );
        let (_, settings) = configured_for_cli(None, &[("MBX_SAVINGS", "plain")]).unwrap();
        assert_eq!(settings.savings, SavingsStyle::Plain);
        let (_, settings) = configured_for_cli(None, &[("MBX_SAVINGS", "off")]).unwrap();
        assert_eq!(settings.savings, SavingsStyle::Off);
        assert!(
            configured_for_cli(None, &[("MBX_SAVINGS", "sarcastic")]).is_err(),
            "an unknown style is an error, not a silent default"
        );
    }

    #[test]
    fn summaries_default_to_short_with_off_and_full_as_choices() {
        let (_, settings) = configured_for_cli(None, &[]).unwrap();
        assert_eq!(settings.summary, SummaryStyle::Short);
        let (_, settings) = configured_for_cli(None, &[("MBX_SUMMARY", "off")]).unwrap();
        assert_eq!(settings.summary, SummaryStyle::Off);
        let (_, settings) = configured_for_cli(None, &[("MBX_SUMMARY", "full")]).unwrap();
        assert_eq!(settings.summary, SummaryStyle::Full);
        assert!(
            configured_for_cli(None, &[("MBX_SUMMARY", "verbose")]).is_err(),
            "an unknown style is an error, not a silent default"
        );
    }

    #[test]
    fn the_scheduler_defaults_on_with_most_of_the_measured_memory() {
        let config = configured(None, &[]).unwrap();
        assert!(config.scheduler.enabled);
        assert_eq!(
            config.scheduler.cpus,
            std::thread::available_parallelism().unwrap().get() as u64
        );
        // 85% of the 32GiB the test harness reports.
        assert_eq!(
            config.scheduler.memory_bytes,
            Some(32 * GIB / 100 * 85),
            "the budget leaves headroom for what it cannot see"
        );
        assert_eq!(config.scheduler.priority, SchedulerPriority::Normal);
        assert_eq!(config.scheduler.reserve_cpus, 0);
    }

    #[test]
    fn the_scheduler_is_configurable_and_refuses_nonsense() {
        let config = configured(
            None,
            &[
                ("MBX_SCHEDULER_CPUS", "3"),
                ("MBX_SCHEDULER_RESERVE_CPUS", "1"),
                ("MBX_SCHEDULER_MEMORY", "4GiB"),
                ("MBX_SCHEDULER_PRIORITY", "low"),
            ],
        )
        .unwrap();
        assert_eq!(config.scheduler.cpus, 3);
        assert_eq!(config.scheduler.reserve_cpus, 1);
        assert_eq!(config.scheduler.permits(), 2);
        assert_eq!(config.scheduler.memory_bytes, Some(4 * GIB));
        assert_eq!(config.scheduler.priority, SchedulerPriority::Low);

        let config = configured(None, &[("MBX_SCHEDULER", "false")]).unwrap();
        assert!(!config.scheduler.enabled);

        let config = configured(None, &[("MBX_SCHEDULER_MEMORY", "none")]).unwrap();
        assert_eq!(
            config.scheduler.memory_bytes, None,
            "\"none\" keeps plain CPU permits"
        );

        let config = configured(
            None,
            &[
                ("MBX_SCHEDULER_CPUS", "3"),
                ("MBX_SCHEDULER_RESERVE_CPUS", "9"),
            ],
        )
        .unwrap();
        assert_eq!(config.scheduler.permits(), 1, "one permit always remains");

        let error = configured(None, &[("MBX_SCHEDULER_CPUS", "0")]).unwrap_err();
        assert!(error.to_string().contains("scheduler.cpus"), "{error}");
        let error = configured(None, &[("MBX_SCHEDULER_RESERVE_CPUS", "-1")]).unwrap_err();
        assert!(
            error.to_string().contains("scheduler.reserve_cpus"),
            "{error}"
        );
        let error = configured(None, &[("MBX_SCHEDULER_PRIORITY", "loud")]).unwrap_err();
        assert!(error.to_string().contains("scheduler.priority"), "{error}");
    }

    #[test]
    fn a_disabled_scheduler_needs_no_memory_measurement() {
        let env = EnvLayer::new([("MBX_SCHEDULER".to_string(), "false".to_string())]);
        let (config, _) = Config::from_layers_measuring(
            &env,
            None,
            |_| None,
            || panic!("a disabled scheduler must not probe memory"),
        )
        .unwrap();
        assert!(!config.scheduler.enabled);
    }

    #[test]
    fn the_usage_spec_declares_files_environment_and_defaults() {
        let spec = RawConfig::spec_kdl();
        assert!(spec.contains(r#"file "<config directory>/mbx/config.toml""#));
        assert!(spec.contains(r#"env "MBX_GC_MAX_SIZE""#));
        assert!(spec.contains(r#"prop "gc.max_size""#));
        assert!(spec.contains(r#"prop "target.max_size""#));
        assert!(spec.contains(r#"env "MBX_TARGET_MAX_AGE""#));
        assert!(spec.contains(r#"default="30d""#));
        // The scaled budgets have no literal default to declare, so the
        // generated reference has to describe them instead.
        assert!(spec.contains("5% of the cache disk"));
        assert!(spec.contains("10% of the cache disk"));
        assert!(spec.contains(r#"env "MBX_SAVINGS""#));
        assert!(spec.contains(r#"default="quips""#));
    }
}
