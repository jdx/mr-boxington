//! Actionable explanations for conservative cache bypasses.

use crate::config::{CliSettings, Config};
use eyre::{Context, Result};
use std::collections::BTreeMap;
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

fn finish(log: &Path, status: ExitCode) -> Result<ExitCode> {
    let records = read_records(log)?;
    display(&records);
    Ok(status)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Records(BTreeMap<String, BTreeMap<String, u64>>);

impl Records {
    fn add(&mut self, kind: &str, detail: &str) {
        *self
            .0
            .entry(kind.to_string())
            .or_default()
            .entry(detail.to_string())
            .or_default() += 1;
    }

    fn total(&self) -> u64 {
        self.0.values().flat_map(BTreeMap::values).copied().sum()
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
        let (kind, detail) = line
            .split_once('\t')
            .ok_or_else(|| eyre::eyre!("invalid bypass record on line {}", index + 1))?;
        if kind.is_empty() || detail.is_empty() {
            eyre::bail!("invalid bypass record on line {}", index + 1);
        }
        records.add(kind, detail);
    }
    Ok(records)
}

fn display(records: &Records) {
    if records.0.is_empty() {
        crate::session::note("\ncache explanation: no compilations bypassed the cache");
        return;
    }
    crate::session::note(&format!(
        "\ncache explanation: {} compilations bypassed the cache",
        records.total()
    ));
    for (kind, details) in &records.0 {
        let count: u64 = details.values().sum();
        crate::session::note(&format!("\n{kind} ({count})"));
        crate::session::note(guidance(kind));
        for (detail, occurrences) in details {
            let suffix = if *occurrences > 1 {
                format!(" ({occurrences} times)")
            } else {
                String::new()
            };
            crate::session::note(&format!("  - {detail}{suffix}"));
        }
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
        assert_eq!(records.0["unsupported-crate-type"].values().sum::<u64>(), 2);
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
}
