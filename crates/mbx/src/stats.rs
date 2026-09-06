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
        Ok(Self::from_projects(
            &store::projects(store)?,
            shared_store_bytes,
        ))
    }

    fn from_projects(projects: &[store::ProjectUsage], shared_store_bytes: u64) -> Self {
        let live = projects
            .iter()
            .filter(|project| project.live)
            .collect::<Vec<_>>();
        let independent_cache_bytes = live.iter().fold(0u64, |total, project| {
            total.saturating_add(project.action_bytes)
        });
        Self {
            live_workspaces: live.len() as u64,
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
    let stats = store::stats(store)?;
    let targets = target::stats(target_root)?;
    Ok(Report {
        version: 1,
        store: store.display().to_string(),
        savings: Lifetime::from(&tally),
        cache: Cache {
            objects: stats.objects,
            bytes: stats.total_bytes(),
            managed_targets: targets.views,
            managed_target_bytes: targets.bytes,
        },
        sharing: SharingEstimate::read(store, stats.total_bytes())?,
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
        }
    }
}

fn size(bytes: u64) -> String {
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
                format!("{} reflinked (cumulative)", size(self.reflinked_bytes)),
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
        lines.push("Compiler time and reflinked bytes are cumulative, not wall time or current disk savings.".into());
        if cheeky && let Some(quip) = self.savings.quip() {
            lines.extend([String::new(), quip]);
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
    fn sharing_is_a_lower_bound_and_excludes_stale_workspaces_and_targets() {
        let projects = vec![
            store::ProjectUsage {
                workspace_root: "first".into(),
                identities: 1,
                action_bytes: 100,
                target_bytes: 500,
                live: true,
            },
            store::ProjectUsage {
                workspace_root: "second".into(),
                identities: 1,
                action_bytes: 100,
                target_bytes: 500,
                live: true,
            },
            store::ProjectUsage {
                workspace_root: "stale".into(),
                identities: 1,
                action_bytes: 1000,
                target_bytes: 500,
                live: false,
            },
        ];
        let sharing = SharingEstimate::from_projects(&projects, 120);
        assert_eq!(sharing.live_workspaces, 2);
        assert_eq!(sharing.duplicate_cache_bytes_avoided_lower_bound, 80);
        assert_eq!(
            SharingEstimate::from_projects(&projects, 300)
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
    fn an_empty_stats_report_is_read_only_and_versioned() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        let report = collect(&store, &root.path().join("targets")).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["version"], 1);
        assert!(json["savings"]["since_unix_secs"].is_null());
        assert_eq!(json["cache"]["bytes"], 0);
        assert!(report.text(true).contains("no savings recorded yet"));
        assert!(!store.join(savings::TALLY_FILE).exists());
    }
}
