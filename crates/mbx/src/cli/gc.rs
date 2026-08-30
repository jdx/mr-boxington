use super::cache::{GcActionStoreReport, GcReport, GcTargetReport, print_json};
use crate::config::{Config, RetentionSettings};
use crate::{store, target};
use bytesize::ByteSize;
use eyre::Result;
use std::path::Path;

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
    let target_budget = target_budget(retention, max_bytes);
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
    let store_budget = retention.max_total_bytes.map_or(max_bytes, |total| {
        max_bytes.min(total.saturating_sub(projected_target_bytes))
    });
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
            match pruned {
                Ok(pruned) => {
                    // Credit what the targets gave back even though the store
                    // sweep failed: those bytes are gone from the disk either way.
                    record_collection(&store, 0, pruned.removed_bytes, dry_run);
                    if !json && pruned.removed_views > 0 {
                        println!("{}", target_removals(&pruned, dry_run));
                    }
                }
                Err(prune_error) => {
                    log::warn!("target directories were not collected: {prune_error}");
                }
            }
            return Err(error);
        }
    };
    record_collection(
        &store,
        outcome.removed_bytes,
        pruned.as_ref().map_or(0, |pruned| pruned.removed_bytes),
        dry_run,
    );
    if json {
        let pruned = pruned?;
        print_json(&GcReport {
            version: 1,
            max_bytes,
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
            },
        })?;
    } else {
        print_gc_store_outcome(&outcome, dry_run);
        let pruned = pruned?;
        if pruned.removed_views > 0 {
            println!("{}", target_removals(&pruned, dry_run));
        }
    }
    Ok(())
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
    let verb = if dry_run { "would free" } else { "freed" };
    format!(
        "{verb} {} target directories ({}, {} abandoned and {} live); {} remain",
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
        "evicted {} objects and {} action results ({}); {} remain",
        outcome.removed_objects,
        outcome.removed_action_results,
        ByteSize::b(outcome.removed_bytes).display().iec(),
        ByteSize::b(outcome.remaining_bytes).display().iec(),
    )
}

/// Keep the store inside its budget, at most once per configured interval.
///
/// Reported like the cache summary beside it: a sweep that evicted nothing says
/// nothing. A sweep that fails is logged and forgotten -- the build is already
/// over, and its exit status is the build's answer, not the collector's. What it
/// freed is returned so the lifetime totals can count it.
pub(super) fn sweep_store(config: &Config, retention: &RetentionSettings) -> crate::savings::Delta {
    let mut delta = crate::savings::Delta::default();
    if !config.gc.auto {
        return delta;
    }
    match store::claim_sweep(&config.store_dir(), config.gc.interval) {
        Ok(false) => {}
        Ok(true) => {
            let pruned = prune_targets(config, retention, config.gc.max_bytes);
            delta.freed_target_bytes = pruned.freed_bytes;
            let store_budget = retention
                .max_total_bytes
                .map_or(config.gc.max_bytes, |total| {
                    let target_bytes = pruned.remaining_bytes.unwrap_or_else(|| {
                        target::stats(&config.target.root)
                            .map(|stats| stats.bytes)
                            .unwrap_or_default()
                    });
                    config.gc.max_bytes.min(total.saturating_sub(target_bytes))
                });
            crate::scheduler::prune_flights(&config.cache_dir);
            let outcome = match store::gc(&config.store_dir(), store_budget) {
                Ok(outcome) => outcome,
                Err(error) => {
                    log::warn!("the store was not swept: {error}");
                    return delta;
                }
            };
            delta.freed_store_bytes = outcome.removed_bytes;
            if outcome.removed_bytes > 0 {
                crate::session::note(&format!("mbx[gc]: {}", evictions(&outcome)));
            }
        }
        Err(error) => {
            log::warn!("the store was not swept: {error}");
            delta.freed_target_bytes =
                prune_targets(config, retention, config.gc.max_bytes).freed_bytes;
        }
    }
    delta
}

/// What one target collection left behind, and what it reclaimed.
pub(super) struct PruneReport {
    /// `None` when collection failed, so a caller sizing a combined budget
    /// knows to measure rather than assume.
    remaining_bytes: Option<u64>,
    freed_bytes: u64,
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
    let target_budget = target_budget(retention, store_reserve);
    match target::collect(
        &config.target.root,
        target_budget,
        retention.target_max_age,
        false,
    ) {
        Ok(pruned) => {
            if pruned.removed_views > 0 {
                crate::session::note(&format!("mbx[gc]: {}", target_removals(&pruned, false)));
            }
            PruneReport {
                remaining_bytes: Some(pruned.remaining_bytes),
                freed_bytes: pruned.removed_bytes,
            }
        }
        Err(error) => {
            log::warn!("target directories were not collected: {error}");
            PruneReport {
                remaining_bytes: None,
                freed_bytes: 0,
            }
        }
    }
}

pub(super) fn target_budget(retention: &RetentionSettings, store_reserve: u64) -> Option<u64> {
    retention
        .max_total_bytes
        .map_or(retention.target_max_bytes, |total| {
            let combined = total.saturating_sub(store_reserve);
            Some(
                retention
                    .target_max_bytes
                    .map_or(combined, |target| target.min(combined)),
            )
        })
}
