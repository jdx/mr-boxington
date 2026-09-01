//! Actionable explanations for conservative cache bypasses.

use crate::config::{CliSettings, Config};
use crate::events::{ActionOutcome, SessionEvent};
use eyre::{Context, Result};
use mbx_cache_core::ActionDiagnostic;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::ExitCode;

/// Run Cargo with a private bypass trace and explain its contents afterwards.
pub fn run(config: &Config, arguments: &[String]) -> Result<ExitCode> {
    let directory = tempfile::Builder::new().prefix("mbx-explain-").tempdir()?;
    let log = directory.path().join("bypasses.tsv");
    let status = crate::cli::cargo_with_bypass_log(config, arguments, Some(&log))?;
    finish(&log, status)
}

pub(crate) fn run_with_settings(
    config: &Config,
    settings: &CliSettings,
    arguments: &[String],
) -> Result<ExitCode> {
    let directory = tempfile::Builder::new().prefix("mbx-explain-").tempdir()?;
    let log = directory.path().join("bypasses.tsv");
    let status =
        crate::cli::cargo_with_settings_and_bypass_log(config, settings, arguments, Some(&log))?;
    finish(&log, status)
}

/// Replay the newest recorded build for this workspace and explain its misses
/// against the most recent earlier hit for each compilation unit.
pub(crate) fn last(config: &Config) -> Result<ExitCode> {
    let workspace = crate::util::workspace_root(&std::env::current_dir()?);
    let sessions = recorded_sessions(&config.store_dir(), &workspace)?;
    let Some((target_index, target)) = sessions.iter().enumerate().next_back() else {
        eyre::bail!(
            "no recorded build was found for {}; run a build with session events enabled first",
            workspace.display()
        );
    };

    let mut previous_hits = PreviousHits::new();
    for session in &sessions[..target_index] {
        for event in &session.events {
            if let SessionEvent::Action {
                outcome: ActionOutcome::Hit,
                crate_name: Some(crate_name),
                diagnostic: Some(diagnostic),
                ..
            } = event
                && let Some(unit) = compilation_unit(diagnostic)
            {
                previous_hits.insert((crate_name.clone(), unit), diagnostic.clone());
            }
        }
    }

    display_last(target, &previous_hits);
    Ok(ExitCode::SUCCESS)
}

struct RecordedSession {
    command: Vec<String>,
    events: Vec<SessionEvent>,
}

type PreviousHits = BTreeMap<(String, String), ActionDiagnostic>;

fn recorded_sessions(store: &Path, workspace: &Path) -> Result<Vec<RecordedSession>> {
    let mut sessions = Vec::new();
    for id in crate::events::session_ids(store) {
        let path = crate::events::session_paths(store, &id).events;
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
            }
        };
        let events = crate::events::parse_events(&contents);
        let Some((recorded_workspace, command)) = events.iter().find_map(|event| match event {
            SessionEvent::SessionStarted {
                workspace_root,
                command,
                ..
            } => Some((workspace_root, command)),
            _ => None,
        }) else {
            continue;
        };
        if recorded_workspace != workspace {
            continue;
        }
        sessions.push(RecordedSession {
            command: command.clone(),
            events,
        });
    }
    Ok(sessions)
}

fn display_last(session: &RecordedSession, previous_hits: &PreviousHits) {
    let command = if session.command.is_empty() {
        "cargo".to_string()
    } else {
        format!("cargo {}", session.command.join(" "))
    };
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for event in &session.events {
        if let SessionEvent::Action { outcome, .. } = event {
            *counts.entry(outcome.label()).or_default() += 1;
        }
    }
    let summary = counts
        .iter()
        .map(|(outcome, count)| format!("{count} {outcome}"))
        .collect::<Vec<_>>()
        .join(", ");
    crate::session::note(&format!(
        "cache explanation: last recorded build\n  command: {command}\n  results: {summary}"
    ));

    let misses = session.events.iter().filter_map(|event| match event {
        SessionEvent::Action {
            outcome: ActionOutcome::Miss,
            crate_name,
            diagnostic,
            ..
        } => Some((
            crate_name.as_deref().unwrap_or("<unknown crate>"),
            diagnostic.as_ref(),
        )),
        _ => None,
    });
    let mut found = false;
    for (crate_name, diagnostic) in misses {
        if !found {
            crate::session::note("\nmissed crates");
            found = true;
        }
        crate::session::note(&format!("\n{crate_name}"));
        let previous = previous_hit(previous_hits, crate_name, diagnostic);
        match (previous, diagnostic) {
            (Some(previous), Some(current)) => display_diff(previous, current),
            (None, _) => crate::session::note(
                "  no earlier recorded hit with key details for this crate; the cache may be cold, this action may use another adapter, or its history may have expired",
            ),
            (Some(_), None) => crate::session::note(
                "  key details were not recorded for this action; run another rustc hit before comparing inputs",
            ),
        }
    }
    if !found {
        crate::session::note("\nno cache misses were recorded");
    }
}

fn compilation_unit(diagnostic: &ActionDiagnostic) -> Option<String> {
    diagnostic
        .components
        .get("compilation unit")
        .map(mbx_cache_core::CacheDigest::key)
}

fn previous_hit<'a>(
    hits: &'a PreviousHits,
    crate_name: &str,
    diagnostic: Option<&ActionDiagnostic>,
) -> Option<&'a ActionDiagnostic> {
    let unit = diagnostic.and_then(compilation_unit)?;
    hits.get(&(crate_name.to_string(), unit))
}

fn display_diff(previous: &ActionDiagnostic, current: &ActionDiagnostic) {
    if previous.action == current.action {
        crate::session::note(
            "  the action key did not change; its cached result was unavailable, evicted, or absent from the configured remote",
        );
        return;
    }
    crate::session::note("  inputs changed since the last hit:");
    for name in changed_keys(&previous.components, &current.components) {
        crate::session::note(&format!("    - {name}"));
    }
    for path in changed_keys(&previous.inputs, &current.inputs) {
        crate::session::note(&format!("    - input {path}"));
    }
}

fn changed_keys(
    previous: &BTreeMap<String, mbx_cache_core::CacheDigest>,
    current: &BTreeMap<String, mbx_cache_core::CacheDigest>,
) -> Vec<String> {
    previous
        .keys()
        .chain(current.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| previous.get(*key) != current.get(*key))
        .cloned()
        .collect()
}

fn finish(log: &Path, status: ExitCode) -> Result<ExitCode> {
    let records = read_records(log)?;
    display(&records);
    Ok(status)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Records {
    bypasses: BTreeMap<String, BypassGroup>,
    observations: BTreeMap<String, String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct BypassGroup {
    records: BTreeMap<(String, Option<String>), u64>,
}

impl Records {
    fn add(&mut self, kind: &str, detail: &str, remediation: Option<&str>) {
        let group = self.bypasses.entry(kind.to_string()).or_default();
        *group
            .records
            .entry((detail.to_string(), remediation.map(str::to_string)))
            .or_default() += 1;
    }

    fn total(&self) -> u64 {
        self.bypasses
            .values()
            .flat_map(|group| group.records.values())
            .copied()
            .sum()
    }
}

fn read_records(path: &Path) -> Result<Records> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Records::default()),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
        }
    };
    parse_records(&contents)
}

fn parse_records(contents: &str) -> Result<Records> {
    let mut records = Records::default();
    for (index, line) in contents.lines().enumerate() {
        let mut fields = line.splitn(3, '\t');
        let kind = fields.next().unwrap_or_default();
        let detail = fields
            .next()
            .ok_or_else(|| eyre::eyre!("invalid bypass record on line {}", index + 1))?;
        if kind == "@observation" {
            let observation = fields.next().ok_or_else(|| {
                eyre::eyre!("invalid cacheability observation on line {}", index + 1)
            })?;
            records
                .observations
                .insert(detail.to_string(), observation.to_string());
            continue;
        }
        if kind.is_empty() || detail.is_empty() {
            eyre::bail!("invalid bypass record on line {}", index + 1);
        }
        records.add(kind, detail, fields.next());
    }
    Ok(records)
}

fn display(records: &Records) {
    if records.bypasses.is_empty() {
        crate::session::note("\ncache explanation: no compilations bypassed the cache");
    } else {
        crate::session::note(&format!(
            "\ncache explanation: {} compilations bypassed the cache",
            records.total()
        ));
        for (kind, group) in &records.bypasses {
            let count: u64 = group.records.values().sum();
            crate::session::note(&format!("\n{kind} ({count})"));
            let mut sections: BTreeMap<Option<&str>, Vec<(&str, u64)>> = BTreeMap::new();
            for ((detail, remediation), occurrences) in &group.records {
                sections
                    .entry(remediation.as_deref())
                    .or_default()
                    .push((detail, *occurrences));
            }
            for (remediation, details) in sections {
                crate::session::note(remediation.unwrap_or_else(|| guidance(kind)));
                for (detail, occurrences) in details {
                    let suffix = if occurrences > 1 {
                        format!(" ({occurrences} times)")
                    } else {
                        String::new()
                    };
                    crate::session::note(&format!("  - {detail}{suffix}"));
                }
            }
        }
    }
    for observation in records.observations.values() {
        crate::session::note(&format!("\ncacheability warning\n{observation}"));
    }
}

fn guidance(kind: &str) -> &'static str {
    match kind {
        "compiler-query" => {
            "Expected: Cargo asks rustc for toolchain information; there is no compilation to cache."
        }
        "standard-input" => {
            "Expected for Cargo probes: source supplied on standard input cannot be rediscovered later."
        }
        "incremental" => {
            "Cargo compiled this incrementally, which mbx cannot cache. `MBX_INCREMENTAL=0` makes it cacheable again; mbx already gives a crate you are editing its own incremental state without giving up the rest of the cache."
        }
        "response-file" => {
            "The invocation uses an `@response-file`; mbx does not model response-file contents yet."
        }
        "unsupported-crate-type" => {
            "This linked artifact type is outside mbx's current cacheability tier. Compilations that link nothing -- what `cargo check` and clippy run -- are cached whatever their crate type, and native test binaries and executables are cached where the linker can be identified. Dynamic libraries and proc macros still link normally."
        }
        "ambiguous-output-name" => {
            "This output is named like a library but is a program, so mbx cannot tell which permissions to restore it with."
        }
        "unportable-native-link" => {
            "This link would embed a path, a timestamp, or a file mbx does not store, so another checkout could not use its result."
        }
        "unsupported-search-path" | "native-library" => {
            "A native dependency or search path is not a precise compiler input, so mbx cannot safely reuse this action."
        }
        "unknown-flag" | "unknown-codegen-option" => {
            "The toolchain passed an option this mbx adapter does not model. Check for a newer mbx release before reporting it."
        }
        "unmapped-absolute-path" => {
            "The invocation references an absolute path outside the workspace, target, Cargo, toolchain, and home mappings. Move it under a mapped root or keep this action uncached."
        }
        "no-dep-info" | "malformed-dep-info" => {
            "mbx needs valid rustc dep-info to discover every input. Inspect the detail below for the rejected output."
        }
        "input-read" | "input-changed" | "input-modified-during-compilation" => {
            "An input was unavailable or changed while rustc ran. Stabilize generated inputs before retrying."
        }
        "cc-not-a-compile" | "cc-non-object-output" | "cc-compiler-query" => {
            "Expected: a build script asked the C compiler to link, preprocess, or describe itself, which is not a cacheable compilation."
        }
        "cc-unknown-flag" | "cc-tool-passthrough" => {
            "A build script passed a C compiler option this mbx adapter does not model. Check for a newer mbx release before reporting it."
        }
        "cc-unsupported-language" => {
            "mbx caches C and C++ compilations; assembly, Objective-C, and precompiled headers still compile normally."
        }
        "cc-embedded-timestamp-macro" => {
            "A source or header expands `__DATE__`, `__TIME__`, or `__TIMESTAMP__`, so its object is not a function of its inputs and cannot be reused."
        }
        "cc-unsupported-environment" => {
            "An include-path or sub-tool environment variable changes the compilation in a way the argv model cannot see. Unset it to make these compilations cacheable."
        }
        "cc-search-path-modified-during-compilation" => {
            "An include directory gained or lost a header while the compiler was running, so what it read cannot be established. Avoid writing headers into a search directory during a build."
        }
        "cc-local-cpu-target" => {
            "A build script compiles for the machine's own processor (`-march=native` or similar), so the object it produces is not a function of anything the cache key names. Name the architecture explicitly to make these compilations cacheable."
        }
        "cc-unsupported-compiler-driver" => {
            "mbx models gcc-style and clang-style drivers. Other compilers, including MSVC, compile normally without caching."
        }
        "cc-unmapped-absolute-path" => {
            "A build script's compilation reads a path outside the mapped and system roots, so mbx cannot key it portably."
        }
        "other" => {
            "The cache adapter failed outside a recognized conservative bypass. The detail below should be included in a bug report."
        }
        _ => {
            "This action cannot be represented safely by the current cache model. The exact reason is shown below."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_repeated_details_by_stable_kind() {
        let records = parse_records(
            "unsupported-crate-type\trustc crate type is not cacheable yet: bin\n\
             unsupported-crate-type\trustc crate type is not cacheable yet: bin\n\
             compiler-query\trustc invocation is a compiler query, not a compilation\n",
        )
        .unwrap();

        assert_eq!(records.total(), 3);
        assert_eq!(
            records.bypasses["unsupported-crate-type"]
                .records
                .values()
                .sum::<u64>(),
            2
        );
        assert!(guidance("incremental").contains("MBX_INCREMENTAL=0"));
    }

    /// A kind with nothing specific to say falls back to a sentence that
    /// amounts to "look at the reason below", which is what `mbx explain`
    /// exists to save someone from. Every kind the adapter can report should
    /// therefore say something of its own.
    #[test]
    fn every_reported_kind_says_something_of_its_own() {
        let generic = guidance("a kind nobody wrote guidance for");
        for kind in [
            "compiler-query",
            "standard-input",
            "incremental",
            "response-file",
            "unsupported-crate-type",
            "unsupported-search-path",
            "native-library",
            "unportable-native-link",
            "ambiguous-output-name",
            "unknown-flag",
            "unknown-codegen-option",
            "unmapped-absolute-path",
        ] {
            assert_ne!(guidance(kind), generic, "{kind} has no guidance of its own");
        }
    }

    #[test]
    fn rejects_partial_records_instead_of_guessing() {
        let error = parse_records("missing a tab\n").unwrap_err();
        assert!(error.to_string().contains("line 1"));
    }

    #[test]
    fn reads_remediations_and_non_bypass_observations() {
        let records = parse_records(
            "unportable-native-link\tnative link is not reproducible: split-debuginfo=packed\tRemove the reported option.\n\
             @observation\tcc-compiler-override\tCC is already set, so C compiles are invisible.\n",
        )
        .unwrap();

        assert_eq!(records.total(), 1);
        assert!(
            records.bypasses["unportable-native-link"]
                .records
                .keys()
                .any(|(_, remediation)| remediation.as_deref()
                    == Some("Remove the reported option."))
        );
        assert!(records.observations["cc-compiler-override"].contains("invisible"));
    }

    #[test]
    fn one_kind_keeps_distinct_remediations_with_their_details() {
        let records = parse_records(
            "unportable-native-link\tnative link: split-debuginfo=packed\tRemove the option.\n\
             unportable-native-link\tthe linker could not be identified\tConfigure the linker.\n",
        )
        .unwrap();

        let group = &records.bypasses["unportable-native-link"];
        assert_eq!(group.records.len(), 2);
        assert!(group.records.contains_key(&(
            "native link: split-debuginfo=packed".into(),
            Some("Remove the option.".into())
        )));
        assert!(group.records.contains_key(&(
            "the linker could not be identified".into(),
            Some("Configure the linker.".into())
        )));
    }

    #[test]
    fn key_diff_names_changed_added_and_removed_inputs() {
        let digest = |text: &[u8]| mbx_cache_core::CacheDigest::blake3(text);
        let previous = BTreeMap::from([
            ("same".into(), digest(b"same")),
            ("changed".into(), digest(b"old")),
            ("removed".into(), digest(b"removed")),
        ]);
        let current = BTreeMap::from([
            ("same".into(), digest(b"same")),
            ("changed".into(), digest(b"new")),
            ("added".into(), digest(b"added")),
        ]);

        assert_eq!(
            changed_keys(&previous, &current),
            ["added", "changed", "removed"]
        );
    }

    #[test]
    fn matches_history_by_compilation_unit_not_only_crate_name() {
        let diagnostic = |unit: &str, action: &str| ActionDiagnostic {
            action: mbx_cache_core::CacheDigest::blake3(action.as_bytes()),
            components: BTreeMap::from([(
                "compilation unit".into(),
                mbx_cache_core::CacheDigest::blake3(unit.as_bytes()),
            )]),
            inputs: BTreeMap::new(),
        };
        let lib = diagnostic("lib-unit", "lib-hit");
        let test = diagnostic("test-unit", "test-hit");
        let current = diagnostic("lib-unit", "lib-miss");
        let hits = PreviousHits::from([
            (("shared_name".into(), compilation_unit(&lib).unwrap()), lib),
            (
                ("shared_name".into(), compilation_unit(&test).unwrap()),
                test,
            ),
        ]);

        let matched = previous_hit(&hits, "shared_name", Some(&current)).unwrap();
        assert_eq!(
            matched.action,
            mbx_cache_core::CacheDigest::blake3(b"lib-hit")
        );
    }
}
