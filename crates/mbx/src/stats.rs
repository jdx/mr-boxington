//! The machine-wide report shared by `mbx stats` and the dashboard.

use crate::{savings, store, target};
use bytesize::ByteSize;
use eyre::Result;
use serde::Serialize;
use std::path::Path;

/// A conservative estimate from logical cache sizes, not filesystem block use.
#[derive(Debug, Default, Serialize)]
pub(crate) struct SharingEstimate {
    pub live_workspaces: u64,
    pub independent_cache_bytes: u64,
    pub shared_store_bytes: u64,
    pub duplicate_cache_bytes_avoided_lower_bound: u64,
}

impl SharingEstimate {
    pub(crate) fn read(store: &Path, shared_store_bytes: u64) -> Result<Self> {
        Ok(Self::from_live_projects(
            &store::live_project_cache_bytes(store)?,
            shared_store_bytes,
        ))
    }

    fn from_live_projects(live_cache_bytes: &[u64], shared_store_bytes: u64) -> Self {
        let independent_cache_bytes = live_cache_bytes
            .iter()
            .fold(0u64, |total, bytes| total.saturating_add(*bytes));
        Self {
            live_workspaces: live_cache_bytes.len() as u64,
            independent_cache_bytes,
            shared_store_bytes,
            // The store also contains unclaimed objects. Subtracting all of it
            // deliberately understates sharing rather than claiming those bytes.
            duplicate_cache_bytes_avoided_lower_bound: independent_cache_bytes
                .saturating_sub(shared_store_bytes),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Report {
    version: u8,
    store: String,
    pub savings: Lifetime,
    pub cache: Cache,
    pub sharing: SharingEstimate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<crate::scheduler::SchedulerReport>,
}

#[derive(Serialize)]
pub(crate) struct Lifetime {
    byte_accounting: &'static str,
    since_unix_secs: Option<u64>,
    builds: u64,
    cached_compilations: u64,
    estimated_compiler_ns_avoided: u64,
    reflinked_bytes: u64,
    pruned_bytes: u64,
    pruned_target_bytes: u64,
    pruned_store_bytes: u64,
    automatically_pruned_bytes: u64,
    automatically_pruned_since_unix_secs: Option<u64>,
    requested_removal_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    compilations: Option<Compilations>,
}

/// Compilations by what the cache did with them, from when mbx started
/// counting them. Raw totals: which of them belong in a hit rate's denominator
/// is the reader's choice.
#[derive(Serialize)]
pub(crate) struct Compilations {
    since_unix_secs: u64,
    hits: u64,
    misses: u64,
    unconsulted: u64,
    bypasses: u64,
    verifications: u64,
    /// Also counted under their outcome.
    incremental: u64,
    compiler_ns: CompilerTime,
}

#[derive(Serialize)]
struct CompilerTime {
    miss: u64,
    unconsulted: u64,
    bypass: u64,
    verification: u64,
}

#[derive(Serialize)]
pub(crate) struct Cache {
    objects: u64,
    bytes: u64,
    managed_targets: u64,
    managed_target_bytes: u64,
}

pub(crate) fn collect(store: &Path, target_root: &Path) -> Result<Report> {
    let tally = savings::read_tally(store);
    // Sizing the target directories and reading the store are independent and
    // both bound by the filesystem, so they overlap.
    let (stats, targets, sharing) = std::thread::scope(|scope| {
        let targets = scope.spawn(|| target::stats(target_root));
        let sharing = scope.spawn(|| store::live_project_cache_bytes(store));
        let stats = store::stats(store);
        (stats, targets.join(), sharing.join())
    });
    let stats = stats?;
    let targets = targets.map_err(|_| eyre::eyre!("sizing target directories panicked"))??;
    let live_cache_bytes =
        sharing.map_err(|_| eyre::eyre!("sizing the cache store panicked"))??;
    Ok(Report {
        scheduler: None,
        version: 1,
        store: store.display().to_string(),
        savings: Lifetime::from(&tally),
        cache: Cache {
            objects: stats.objects,
            bytes: stats.total_bytes(),
            managed_targets: targets.views,
            managed_target_bytes: targets.bytes,
        },
        sharing: SharingEstimate::from_live_projects(&live_cache_bytes, stats.total_bytes()),
    })
}

impl From<&savings::Tally> for Lifetime {
    fn from(tally: &savings::Tally) -> Self {
        Self {
            byte_accounting: "logical",
            since_unix_secs: (tally.since_secs > 0).then_some(tally.since_secs),
            builds: tally.builds,
            cached_compilations: tally.cached_compilations,
            estimated_compiler_ns_avoided: tally.avoided_compiler_ns,
            reflinked_bytes: tally.reflinked_bytes,
            pruned_bytes: tally
                .freed_target_bytes
                .saturating_add(tally.freed_store_bytes),
            pruned_target_bytes: tally.freed_target_bytes,
            pruned_store_bytes: tally.freed_store_bytes,
            automatically_pruned_bytes: tally.auto_pruned_bytes,
            automatically_pruned_since_unix_secs: (tally.auto_pruned_since_secs > 0)
                .then_some(tally.auto_pruned_since_secs),
            requested_removal_bytes: tally.freed_requested_bytes,
            compilations: (tally.outcomes.since_secs > 0).then(|| {
                let outcomes = &tally.outcomes;
                Compilations {
                    since_unix_secs: outcomes.since_secs,
                    hits: outcomes.hits,
                    misses: outcomes.misses,
                    unconsulted: outcomes.unconsulted,
                    bypasses: outcomes.bypasses,
                    verifications: outcomes.verifications,
                    incremental: outcomes.incremental,
                    compiler_ns: CompilerTime {
                        miss: outcomes.miss_compiler_ns,
                        unconsulted: outcomes.unconsulted_compiler_ns,
                        bypass: outcomes.bypass_compiler_ns,
                        verification: outcomes.verification_compiler_ns,
                    },
                }
            }),
        }
    }
}

pub(crate) fn size(bytes: u64) -> String {
    ByteSize::b(bytes).display().iec().to_string()
}

fn logical_size(bytes: u64) -> String {
    format!("{} logical", size(bytes))
}

impl Lifetime {
    pub(crate) fn since(&self) -> String {
        self.since_unix_secs
            .map_or_else(|| "no savings recorded yet".into(), savings::since)
    }

    pub(crate) fn rows(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                "compiler time avoided",
                format!(
                    "{} (estimated)",
                    savings::nanos(self.estimated_compiler_ns_avoided)
                ),
            ),
            ("builds", self.builds.to_string()),
            ("cache hits", self.cached_compilations.to_string()),
            (
                "compilations",
                self.compilations.as_ref().map_or_else(
                    || "not tracked yet".into(),
                    |counted| {
                        // Verifications only appear where sampling is on.
                        let verified = if counted.verifications > 0 {
                            format!(", {} verified", counted.verifications)
                        } else {
                            String::new()
                        };
                        format!(
                            "{} hits, {} misses, {} not looked up, {} bypassed{verified} {}",
                            counted.hits,
                            counted.misses,
                            counted.unconsulted,
                            counted.bypasses,
                            savings::since(counted.since_unix_secs)
                        )
                    },
                ),
            ),
            ("pruned by mbx", logical_size(self.pruned_bytes)),
            (
                "  targets / cache",
                format!(
                    "{} / {}",
                    logical_size(self.pruned_target_bytes),
                    logical_size(self.pruned_store_bytes)
                ),
            ),
            (
                "automatically pruned",
                self.automatically_pruned_since_unix_secs.map_or_else(
                    || "not tracked yet".into(),
                    |since| {
                        format!(
                            "{} {}",
                            logical_size(self.automatically_pruned_bytes),
                            savings::since(since)
                        )
                    },
                ),
            ),
            (
                "requested removals",
                logical_size(self.requested_removal_bytes),
            ),
            (
                "copying avoided",
                format!("{} (cumulative)", size(self.reflinked_bytes)),
            ),
        ]
    }

    /// Stable while a dashboard is open; no randomly changing joke on every tick.
    pub(crate) fn quip(&self) -> Option<String> {
        if self.automatically_pruned_bytes > 0 {
            Some(format!(
                "{} taken out with the trash. You did not lift a finger.",
                logical_size(self.automatically_pruned_bytes)
            ))
        } else if self.cached_compilations > 0 {
            Some(format!(
                "{} compilations served reheated. rustc can finish its coffee.",
                self.cached_compilations
            ))
        } else {
            None
        }
    }
}

impl Report {
    pub(crate) fn text(&self, cheeky: bool) -> String {
        let mut lines = vec![
            format!("mbx · {}", self.savings.since()),
            format!("store: {}", self.store),
            String::new(),
        ];
        lines.extend(
            self.savings
                .rows()
                .into_iter()
                .map(|(label, value)| format!("{label:<24} {value}")),
        );
        lines.push(String::new());
        lines.push(format!(
            "shared cache             {} in {} objects",
            size(self.cache.bytes),
            self.cache.objects
        ));
        lines.push(format!(
            "managed targets          {} ({})",
            self.cache.managed_targets,
            size(self.cache.managed_target_bytes)
        ));
        lines.extend(
            self.sharing
                .rows()
                .into_iter()
                .map(|(label, value)| format!("{label:<24} {value}")),
        );
        lines.push(String::new());
        lines.push(
            "Sharing is a lower-bound estimate of logical cache bytes, excluding targets.".into(),
        );
        lines.push(
            "Pruned totals are logical file sizes, not physical space reclaimed; requested removals are separate."
                .into(),
        );
        lines.push("Compiler time and copying avoided are cumulative, not wall time or current disk savings.".into());
        if cheeky && let Some(quip) = self.savings.quip() {
            lines.extend([String::new(), quip]);
        }
        if let Some(scheduler) = &self.scheduler {
            lines.extend([String::new(), scheduler.text()]);
        }
        lines.join("\n")
    }
}

impl SharingEstimate {
    pub(crate) fn rows(&self) -> Vec<(&'static str, String)> {
        vec![
            ("live workspaces", self.live_workspaces.to_string()),
            ("with separate caches", size(self.independent_cache_bytes)),
            (
                "duplication avoided",
                format!(
                    "at least {} (estimated)",
                    size(self.duplicate_cache_bytes_avoided_lower_bound)
                ),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharing_is_a_lower_bound() {
        let sharing = SharingEstimate::from_live_projects(&[100, 100], 120);
        assert_eq!(sharing.live_workspaces, 2);
        assert_eq!(sharing.independent_cache_bytes, 200);
        assert_eq!(sharing.duplicate_cache_bytes_avoided_lower_bound, 80);
        assert_eq!(
            SharingEstimate::from_live_projects(&[100, 100], 300)
                .duplicate_cache_bytes_avoided_lower_bound,
            0
        );
    }

    #[test]
    fn old_tallies_do_not_claim_historical_automatic_pruning() {
        let tally = serde_json::from_value::<savings::Tally>(serde_json::json!({"version":1, "since_secs": 1_700_000_000, "freed_target_bytes":1024, "freed_store_bytes":2048, "freed_requested_bytes":4096})).unwrap();
        let report = Lifetime::from(&tally);
        assert_eq!(report.pruned_bytes, 3072);
        assert_eq!(report.automatically_pruned_since_unix_secs, None);
        assert_eq!(
            report
                .rows()
                .iter()
                .find(|(label, _)| *label == "automatically pruned")
                .unwrap()
                .1,
            "not tracked yet"
        );
        assert_eq!(report.since(), "since 2023-11-14");
    }

    #[test]
    fn compilations_by_outcome_appear_once_counting_starts() {
        let old = serde_json::from_value::<savings::Tally>(
            serde_json::json!({"version": 1, "since_secs": 1_700_000_000, "cached_compilations": 9}),
        )
        .unwrap();
        let report = Lifetime::from(&old);
        assert!(
            serde_json::to_value(&report)
                .unwrap()
                .get("compilations")
                .is_none()
        );
        let row = |report: &Lifetime| {
            report
                .rows()
                .into_iter()
                .find(|(label, _)| *label == "compilations")
                .unwrap()
                .1
        };
        assert_eq!(row(&report), "not tracked yet");

        let counted = serde_json::from_value::<savings::Tally>(serde_json::json!({
            "version": 1,
            "since_secs": 1_700_000_000,
            "outcomes": {
                "since_secs": 1_800_000_000,
                "hits": 7,
                "misses": 2,
                "unconsulted": 3,
                "bypasses": 1,
                "verifications": 4,
                "incremental": 2,
                "unconsulted_compiler_ns": 5,
            },
        }))
        .unwrap();
        let report = Lifetime::from(&counted);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["compilations"]["since_unix_secs"], 1_800_000_000);
        assert_eq!(json["compilations"]["hits"], 7);
        assert_eq!(json["compilations"]["unconsulted"], 3);
        assert_eq!(json["compilations"]["incremental"], 2);
        assert_eq!(json["compilations"]["compiler_ns"]["unconsulted"], 5);
        assert_eq!(
            row(&report),
            "7 hits, 2 misses, 3 not looked up, 1 bypassed, 4 verified since 2027-01-15"
        );
    }

    #[test]
    fn scheduler_section_is_in_text_and_json_without_creating_files() {
        let root = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::for_test(root.path());
        config.scheduler.enabled = true;
        config.scheduler.cpus = 4;
        config.scheduler.reserve_cpus = 0;
        config.scheduler.memory_bytes = Some(4000);
        let mut report = collect(&config.store_dir(), &config.target.root).unwrap();
        report.scheduler = Some(crate::scheduler::scheduler_report(&config));
        let text = report.text(false);
        assert!(
            text.contains(
                "Scheduler: enabled (4 cores, 3.9 KiB memory budget, from the global settings)"
            ),
            "{text}"
        );
        assert!(text.contains("No recorded memory peaks."));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["scheduler"]["bytes_per_permit"], 1000);
        assert_eq!(json["scheduler"]["capacity"], 4);
        assert_eq!(json["scheduler"]["entries"], serde_json::json!([]));
        assert!(!root.path().join("scheduler").exists());
    }

    #[test]
    fn an_empty_stats_report_is_read_only_and_versioned() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        let report = collect(&store, &root.path().join("targets")).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["version"], 1);
        assert!(json.get("scheduler").is_none());
        assert!(!report.text(false).contains("Scheduler:"));
        assert!(json["savings"]["since_unix_secs"].is_null());
        assert_eq!(json["cache"]["bytes"], 0);
        assert!(report.text(true).contains("no savings recorded yet"));
        assert!(!store.join(savings::TALLY_FILE).exists());
    }
}
