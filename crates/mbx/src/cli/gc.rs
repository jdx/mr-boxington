use super::cache::{
    GcActionStoreReport, GcIncrementalReport, GcReport, GcTargetReport, print_json,
};
use crate::config::{Config, RetentionSettings};
use crate::{store, target};
use bytesize::ByteSize;
use eyre::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the collector a build started leaves what it freed, for the next
/// build to say. Claimed by rename, so two builds finishing together cannot
/// both report it.
pub(super) const SWEEP_REPORT: &str = "gc/v1/last-sweep-report";

/// The detached collector's stderr: warnings a sweep logs have no terminal
/// to reach, and a sweep that never finished is diagnosed from here.
const SWEEP_LOG: &str = "gc/v1/sweep.log";

#[derive(usage::Args)]
pub(super) struct GcArgs {
    /// Size the store may occupy afterwards, for example 20GiB. Defaults to the
    /// configured budget.
    #[usage(long, value_name = "SIZE")]
    pub(super) max_size: Option<ByteSize>,
    /// Print a stable machine-readable report.
    #[usage(long)]
    pub(super) json: bool,
    /// Show what collection would remove without changing any files.
    #[usage(long)]
    pub(super) dry_run: bool,
    /// Run the throttled sweep a build schedules, if one is due. Builds start
    /// this in the background; it is not meant to be typed.
    #[usage(long, hide = true)]
    pub(super) automatic: bool,
}

pub(super) fn run(
    config: &Config,
    max_bytes: u64,
    dry_run: bool,
    json: bool,
    retention: &RetentionSettings,
) -> Result<()> {
    let store = config.store_dir();
    // The collector below remains the authority for store errors. Estimating
    // a combined budget must not prevent independent target collection when
    // the action store is damaged.
    let incremental = match crate::incremental::collect(
        &config.cache_dir.join("incremental"),
        incremental_budget(retention, max_bytes),
        retention.incremental_max_age,
        dry_run,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            log::warn!("learned incremental state was not collected: {error}");
            let stats = crate::incremental::stats(&config.cache_dir.join("incremental"))
                .unwrap_or_default();
            crate::incremental::PruneOutcome {
                remaining_directories: stats.directories,
                remaining_bytes: stats.bytes,
                untracked_directories: stats.untracked_directories,
                ..crate::incremental::PruneOutcome::default()
            }
        }
    };
    let target_budget = target_budget(retention, max_bytes, incremental.remaining_bytes);
    let pruned = target::collect(
        &config.target.root,
        target_budget,
        retention.target_max_age,
        dry_run,
    );
    let projected_target_bytes = match &pruned {
        Ok(outcome) => outcome.remaining_bytes,
        Err(_) if retention.max_total_bytes.is_some() => target::stats(&config.target.root)
            .map(|stats| stats.bytes)
            .unwrap_or_default(),
        Err(_) => 0,
    };
    let store_budget = store_budget(
        retention,
        max_bytes,
        projected_target_bytes.saturating_add(incremental.remaining_bytes),
    );
    // Small and never load-bearing: a swept flight costs at most one
    // compilation that would have been a hit, so it is not part of the
    // budget arithmetic or the dry run's accounting.
    if !dry_run {
        crate::scheduler::prune_flights(&config.cache_dir);
    }
    let outcome = if dry_run {
        store::gc_dry_run(&store, store_budget)
    } else {
        store::gc(&store, store_budget)
    };
    // Independent collections: a broken action store must not prevent the
    // command from freeing the usually much larger target directories.
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            let mut freed_bytes = incremental.removed_bytes;
            match pruned {
                Ok(pruned) => {
                    // Credit what the targets gave back even though the store
                    // sweep failed: those bytes are gone from the disk either way.
                    freed_bytes = freed_bytes.saturating_add(pruned.removed_bytes);
                    if !json && pruned.removed_views > 0 {
                        println!("{}", target_removals(&pruned, dry_run));
                    }
                }
                Err(prune_error) => {
                    log::warn!("target directories were not collected: {prune_error}");
                }
            }
            record_collection(&store, 0, freed_bytes, dry_run);
            if !json {
                print_incremental_removals(&incremental, dry_run);
            }
            return Err(error);
        }
    };
    record_collection(
        &store,
        outcome.removed_bytes,
        pruned.as_ref().map_or(0, |pruned| pruned.removed_bytes) + incremental.removed_bytes,
        dry_run,
    );
    if json {
        let pruned = pruned?;
        print_json(&GcReport {
            version: 1,
            byte_accounting: "logical",
            max_bytes,
            max_total_bytes: retention.max_total_bytes,
            target_max_bytes: retention.target_max_bytes,
            incremental_max_bytes: retention.incremental_max_bytes,
            dry_run,
            action_store: GcActionStoreReport {
                removed_objects: outcome.removed_objects,
                removed_action_results: outcome.removed_action_results,
                removed_checkout_records: outcome.removed_checkout_records,
                removed_session_streams: outcome.removed_session_streams,
                removed_bytes: outcome.removed_bytes,
                remaining_bytes: outcome.remaining_bytes,
            },
            targets: GcTargetReport {
                removed_directories: pruned.removed_views,
                removed_bytes: pruned.removed_bytes,
                remaining_directories: pruned.remaining_views,
                remaining_bytes: pruned.remaining_bytes,
            },
            incremental: GcIncrementalReport {
                removed_directories: incremental.removed_directories,
                removed_bytes: incremental.removed_bytes,
                remaining_directories: incremental.remaining_directories,
                remaining_bytes: incremental.remaining_bytes,
                skipped_active_directories: incremental.skipped_active_directories,
                untracked_directories: incremental.untracked_directories,
            },
        })?;
    } else {
        print_gc_store_outcome(&outcome, dry_run);
        // This collection is independent of the managed-target walk below,
        // so report it even if that walk failed.
        print_incremental_removals(&incremental, dry_run);
        let pruned = pruned?;
        if pruned.removed_views > 0 {
            println!("{}", target_removals(&pruned, dry_run));
        }
    }
    Ok(())
}

fn print_incremental_removals(outcome: &crate::incremental::PruneOutcome, dry_run: bool) {
    if outcome.removed_directories == 0
        && outcome.skipped_active_directories == 0
        && outcome.untracked_directories == 0
    {
        return;
    }
    let verb = if dry_run { "would remove" } else { "removed" };
    println!(
        "{verb} {} learned incremental directories ({} logical); {} logical remain",
        outcome.removed_directories,
        ByteSize::b(outcome.removed_bytes).display().iec(),
        ByteSize::b(outcome.remaining_bytes).display().iec(),
    );
    if outcome.skipped_active_directories > 0 {
        println!(
            "kept {} learned incremental directories used by active builds",
            outcome.skipped_active_directories
        );
    }
    if outcome.untracked_directories > 0 {
        println!(
            "kept {} learned incremental directories with unreadable checkout records",
            outcome.untracked_directories
        );
    }
}

/// Add what a collection reclaimed to this machine's lifetime totals.
///
/// A dry run reclaimed nothing, so it contributes nothing.
pub(super) fn record_collection(store: &Path, store_bytes: u64, target_bytes: u64, dry_run: bool) {
    if dry_run || (store_bytes == 0 && target_bytes == 0) {
        return;
    }
    crate::savings::record_quietly(
        store,
        &crate::savings::Delta {
            freed_store_bytes: store_bytes,
            freed_target_bytes: target_bytes,
            ..crate::savings::Delta::default()
        },
    );
}

pub(super) fn print_gc_store_outcome(outcome: &store::GcOutcome, dry_run: bool) {
    let prefix = if dry_run { "would have " } else { "" };
    println!("{prefix}{}", evictions(outcome));
    if outcome.removed_checkout_records > 0 {
        println!(
            "{prefix}dropped {} stale checkout records",
            outcome.removed_checkout_records
        );
    }
    if outcome.removed_session_streams > 0 {
        println!(
            "{prefix}dropped {} session event streams",
            outcome.removed_session_streams
        );
    }
}

/// One line describing the target directories a sweep freed.
pub(super) fn target_removals(outcome: &target::CollectionOutcome, dry_run: bool) -> String {
    let verb = if dry_run { "would remove" } else { "removed" };
    format!(
        "{verb} {} target directories ({} logical, {} abandoned and {} live); {} logical remain",
        outcome.removed_views,
        ByteSize::b(outcome.removed_bytes).display().iec(),
        outcome.removed_stale_views,
        outcome.removed_live_views,
        ByteSize::b(outcome.remaining_bytes).display().iec(),
    )
}

/// One line describing what a sweep evicted.
///
/// Shared so the explicit command and the automatic sweep cannot drift into
/// describing the same outcome two different ways.
pub(super) fn evictions(outcome: &store::GcOutcome) -> String {
    format!(
        "evicted {} objects and {} action results ({} logical); {} logical remain",
        outcome.removed_objects,
        outcome.removed_action_results,
        ByteSize::b(outcome.removed_bytes).display().iec(),
        ByteSize::b(outcome.remaining_bytes).display().iec(),
    )
}

/// What an automatic sweep freed, and the lines that say so.
#[derive(Debug, Default)]
pub(super) struct Sweep {
    pub(super) delta: crate::savings::Delta,
    /// One line per collection that removed something, without the `mbx[gc]`
    /// prefix: a sweep that evicted nothing says nothing.
    pub(super) lines: Vec<String>,
}

/// Keep the store inside its budget, at most once per configured interval.
///
/// A sweep that fails is logged and forgotten -- the build that scheduled it
/// is already over, and its exit status is the build's answer, not the
/// collector's. What it freed is returned so the lifetime totals can count it.
pub(super) fn sweep_store(config: &Config, retention: &RetentionSettings) -> Sweep {
    let mut sweep = Sweep::default();
    if !config.gc.auto {
        return sweep;
    }
    match store::claim_sweep(&config.store_dir(), config.gc.interval) {
        Ok(false) => {}
        Ok(true) => {
            let pruned = prune_targets(config, retention, config.gc.max_bytes);
            sweep.delta.freed_target_bytes = pruned.freed_bytes;
            sweep.lines.extend(pruned.removals);
            let non_store_bytes = pruned.remaining_bytes.unwrap_or_else(|| {
                let target_bytes =
                    target::stats(&config.target.root).map_or(0, |stats| stats.bytes);
                let incremental_bytes =
                    crate::incremental::stats(&config.cache_dir.join("incremental"))
                        .map_or(0, |stats| stats.bytes);
                target_bytes.saturating_add(incremental_bytes)
            });
            let store_budget = store_budget(retention, config.gc.max_bytes, non_store_bytes);
            crate::scheduler::prune_flights(&config.cache_dir);
            let outcome = match store::gc(&config.store_dir(), store_budget) {
                Ok(outcome) => outcome,
                Err(error) => {
                    log::warn!("the store was not swept: {error}");
                    return sweep;
                }
            };
            sweep.delta.freed_store_bytes = outcome.removed_bytes;
            if outcome.removed_bytes > 0 {
                sweep.lines.push(evictions(&outcome));
            }
        }
        Err(error) => {
            log::warn!("the store was not swept: {error}");
            let pruned = prune_targets(config, retention, config.gc.max_bytes);
            sweep.delta.freed_target_bytes = pruned.freed_bytes;
            sweep.lines.extend(pruned.removals);
        }
    }
    sweep
}

/// Start the sweep a finished build leaves behind, in a process of its own.
///
/// The walk of every managed target and the whole store is the slowest thing
/// mbx does after a build, and on a machine with many checkouts it takes
/// longer than the edit-loop build that happened to come due: measured at
/// twelve seconds added to a two-second build. Nothing the build printed
/// depends on it, so the build exits and the collector runs on without a
/// terminal. What it frees is written for the next build to report, and the
/// lifetime totals are updated by the collector itself.
///
/// The due check here is unlocked, so two builds finishing together may both
/// start a collector; the claim inside the collector lets exactly one sweep.
/// A collector that cannot be started sweeps in this process instead, as
/// builds always did, so a machine where spawning fails is still collected.
pub(super) fn schedule_sweep(config: &Config, retention: &RetentionSettings) {
    if !config.gc.auto {
        return;
    }
    let store = config.store_dir();
    if !store::sweep_is_due(&store, config.gc.interval) {
        return;
    }
    if let Err(error) = spawn_collector(&store) {
        log::debug!("the automatic sweep runs in the foreground: {error:#}");
        if let Err(error) = run_automatic(config, retention) {
            log::warn!("the store was not swept: {error:#}");
        }
    }
}

fn spawn_collector(store: &Path) -> Result<()> {
    let executable = std::env::current_exe().wrap_err("failed to locate mbx")?;
    let log_path = store.join(SWEEP_LOG);
    std::fs::create_dir_all(log_path.parent().expect("the sweep log has a parent"))?;
    let log = std::fs::File::create(&log_path)
        .wrap_err_with(|| format!("failed to create {}", log_path.display()))?;
    let mut command = Command::new(executable);
    command
        .args(["gc", "--automatic"])
        // The build may have arrived through the Cargo shim. The collector is
        // an mbx command, and must not be dispatched as Cargo.
        .env_remove("MBX_CARGO_SHIM_MODE")
        .env_remove("MBX_CARGO_SHIM_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    detach(&mut command);
    command.spawn().wrap_err("failed to start the collector")?;
    Ok(())
}

/// Keep the collector out of the terminal's process group, so the interrupt
/// that stops the next build does not stop a sweep halfway through.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

/// The sweep a build scheduled: `mbx gc --automatic`.
///
/// Claims the throttle stamp like the in-process sweep always did, counts what
/// it freed toward the lifetime totals, and leaves the description for the
/// next build to print. Its own stderr is the sweep log.
pub(super) fn run_automatic(config: &Config, retention: &RetentionSettings) -> Result<()> {
    let sweep = sweep_store(config, retention);
    let store = config.store_dir();
    log::debug!(
        "the automatic sweep freed {} target bytes and {} store bytes: {:?}",
        sweep.delta.freed_target_bytes,
        sweep.delta.freed_store_bytes,
        sweep.lines
    );
    let mut delta = sweep.delta;
    delta.auto_pruned_bytes = delta
        .freed_target_bytes
        .saturating_add(delta.freed_store_bytes);
    if delta != crate::savings::Delta::default() {
        crate::savings::record_quietly(&store, &delta);
    }
    if sweep.lines.is_empty() {
        return Ok(());
    }
    let report = sweep.lines.join("\n") + "\n";
    crate::util::write_atomic(&store.join(SWEEP_REPORT), report.as_bytes())
}

/// What the last background sweep freed, said once.
///
/// The report is claimed by renaming it away before it is read, so of two
/// builds finishing at the same moment, the one whose rename succeeds prints
/// it and the other finds nothing.
pub(super) fn take_sweep_report(store: &Path) -> Vec<String> {
    let path = store.join(SWEEP_REPORT);
    let claimed = claimed_report_path(&path);
    if std::fs::rename(&path, &claimed).is_err() {
        return Vec::new();
    }
    let report = std::fs::read_to_string(&claimed).unwrap_or_default();
    let _ = std::fs::remove_file(&claimed);
    report
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

fn claimed_report_path(report: &Path) -> PathBuf {
    report.with_extension(format!("claimed-{}", std::process::id()))
}

/// What one target collection left behind, and what it reclaimed.
pub(super) struct PruneReport {
    /// `None` when collection failed, so a caller sizing a combined budget
    /// knows to measure rather than assume.
    remaining_bytes: Option<u64>,
    freed_bytes: u64,
    /// The line describing removed target directories, when any were.
    removals: Option<String>,
}

/// Collect target views as the other half of a due automatic sweep.
pub(super) fn prune_targets(
    config: &Config,
    retention: &RetentionSettings,
    store_reserve: u64,
) -> PruneReport {
    // A target directory whose checkout is gone is the largest thing
    // collection ever frees, and walking for it on every build would be the
    // slowest, so callers keep this inside the store sweep's throttle.
    let incremental = crate::incremental::collect(
        &config.cache_dir.join("incremental"),
        incremental_budget(retention, store_reserve),
        retention.incremental_max_age,
        false,
    );
    let (incremental_bytes, incremental_remaining) = match incremental {
        Ok(outcome) => (outcome.removed_bytes, outcome.remaining_bytes),
        Err(error) => {
            log::warn!("learned incremental state was not collected: {error}");
            let remaining = crate::incremental::stats(&config.cache_dir.join("incremental"))
                .map_or(0, |stats| stats.bytes);
            (0, remaining)
        }
    };
    let target_budget = target_budget(retention, store_reserve, incremental_remaining);
    match target::collect(
        &config.target.root,
        target_budget,
        retention.target_max_age,
        false,
    ) {
        Ok(pruned) => {
            log::debug!(
                "target collection removed {} directories ({} abandoned) and kept {}",
                pruned.removed_views,
                pruned.removed_stale_views,
                pruned.remaining_views
            );
            PruneReport {
                remaining_bytes: Some(pruned.remaining_bytes.saturating_add(incremental_remaining)),
                freed_bytes: pruned.removed_bytes.saturating_add(incremental_bytes),
                removals: (pruned.removed_views > 0).then(|| target_removals(&pruned, false)),
            }
        }
        Err(error) => {
            log::warn!("target directories were not collected: {error}");
            PruneReport {
                remaining_bytes: None,
                freed_bytes: incremental_bytes,
                removals: None,
            }
        }
    }
}

pub(super) fn target_budget(
    retention: &RetentionSettings,
    store_reserve: u64,
    incremental_reserve: u64,
) -> Option<u64> {
    retention
        .max_total_bytes
        .map_or(retention.target_max_bytes, |total| {
            let combined = total.saturating_sub(store_reserve.saturating_add(incremental_reserve));
            Some(
                retention
                    .target_max_bytes
                    .map_or(combined, |target| target.min(combined)),
            )
        })
}

/// Bound incremental state independently and inside the space left after the
/// action-store reserve in a combined budget.
pub(super) fn incremental_budget(retention: &RetentionSettings, store_reserve: u64) -> Option<u64> {
    retention
        .max_total_bytes
        .map_or(retention.incremental_max_bytes, |total| {
            let combined = total.saturating_sub(store_reserve);
            Some(
                retention
                    .incremental_max_bytes
                    .map_or(combined, |incremental| incremental.min(combined)),
            )
        })
}

pub(super) fn store_budget(
    retention: &RetentionSettings,
    max_bytes: u64,
    non_store_bytes: u64,
) -> u64 {
    retention.max_total_bytes.map_or(max_bytes, |total| {
        max_bytes.min(total.saturating_sub(non_store_bytes))
    })
}
