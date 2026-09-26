//! Identify the build unit a wrapper invocation produced and the units it
//! consumed, so a recorded build can be read as a dependency graph.
//!
//! Cargo gives every unit a hash and passes it to rustc as
//! `-C extra-filename=-<hash>`. The files the unit writes, and the `--extern`
//! paths its dependents receive, carry the same hash. A build script's run has
//! its own directory, `build/<package>-<hash>/out`, which the crates that read
//! it receive as `OUT_DIR`, while the script binary lives in the directory
//! named for its compilation's hash. Those names are all this needs; Cargo's
//! own unit graph is unstable and is not consulted.
//!
//! An identity is a hint for analysis, never a cache input: a name this cannot
//! read yields no identity rather than a guess.

use std::ffi::OsString;
use std::path::Path;

/// The unit a rustc invocation produces and the units whose outputs it reads.
pub(crate) fn rustc_unit(
    arguments: &[OsString],
    out_dir: Option<&Path>,
) -> (Option<String>, Vec<String>) {
    let mut unit = None;
    let mut dependencies = Vec::new();
    let mut arguments = arguments.iter().filter_map(|argument| argument.to_str());
    while let Some(argument) = arguments.next() {
        let codegen = match argument {
            "-C" | "--codegen" => arguments.next(),
            _ => argument
                .strip_prefix("-C")
                .or_else(|| argument.strip_prefix("--codegen="))
                .filter(|option| !option.is_empty()),
        };
        if let Some(hash) = codegen
            .and_then(|option| option.strip_prefix("extra-filename="))
            .map(|value| value.trim_start_matches('-'))
            .filter(|hash| is_hash(hash))
        {
            unit = Some(hash.to_string());
            continue;
        }
        let external = match argument {
            "--extern" => arguments.next(),
            _ => argument.strip_prefix("--extern="),
        };
        if let Some(hash) = external
            .and_then(|value| value.split_once('=').map(|(_, path)| path))
            .and_then(|path| artifact_hash(Path::new(path)))
        {
            dependencies.push(hash);
        }
    }
    dependencies.extend(out_dir.and_then(build_script_run));
    dependencies.sort();
    dependencies.dedup();
    (unit, dependencies)
}

/// The unit a build-script run produces, and the compilation it runs.
pub(crate) fn build_script_run_unit(
    out_dir: Option<&Path>,
    script: &Path,
) -> (Option<String>, Vec<String>) {
    let unit = out_dir.and_then(build_script_run);
    let compilation = script
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .and_then(directory_hash);
    (unit, compilation.into_iter().collect())
}

/// The run a build script's `OUT_DIR` belongs to.
fn build_script_run(out_dir: &Path) -> Option<String> {
    if out_dir.file_name()? != "out" {
        return None;
    }
    let directory = out_dir.parent()?.file_name()?.to_str()?;
    directory_hash(directory).map(|hash| format!("run-{hash}"))
}

/// The hash in a `<package>-<hash>` directory name.
fn directory_hash(name: &str) -> Option<String> {
    let (_, hash) = name.rsplit_once('-')?;
    is_hash(hash).then(|| hash.to_string())
}

/// The hash in a compiler artifact's file name, as Cargo spells one:
/// `lib<crate>-<hash>.rlib`, `.rmeta`, or a dynamic library.
fn artifact_hash(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    if !matches!(extension, "rlib" | "rmeta" | "so" | "dylib" | "dll") {
        return None;
    }
    directory_hash(path.file_stem()?.to_str()?)
}

fn is_hash(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_rustc_unit_is_named_by_its_extra_filename_and_reads_its_externs() {
        let (unit, dependencies) = rustc_unit(
            &arguments(&[
                "--crate-name",
                "app",
                "-C",
                "metadata=0123",
                "-C",
                "extra-filename=-5d4c3b2a",
                "--extern",
                "engine=/t/debug/deps/libengine-aa11.rmeta",
                "--extern=serde_derive=/t/debug/deps/libserde_derive-bb22.so",
                "--extern",
                "noprelude:alloc=/t/debug/deps/liballoc-cc33.rlib",
                // A sysroot crate named without a path has no unit here.
                "--extern",
                "proc_macro",
            ]),
            Some(Path::new("/t/debug/build/app-dd44/out")),
        );

        assert_eq!(unit.as_deref(), Some("5d4c3b2a"));
        assert_eq!(dependencies, ["aa11", "bb22", "cc33", "run-dd44"]);
    }

    #[test]
    fn joined_codegen_spellings_are_read_too() {
        assert_eq!(
            rustc_unit(&arguments(&["-Cextra-filename=-ab12"]), None).0,
            Some("ab12".into())
        );
        assert_eq!(
            rustc_unit(&arguments(&["--codegen=extra-filename=-cd34"]), None).0,
            Some("cd34".into())
        );
    }

    #[test]
    fn a_name_that_is_not_cargo_s_yields_no_identity() {
        let (unit, dependencies) = rustc_unit(
            &arguments(&[
                "-C",
                "extra-filename=custom",
                "--extern",
                "engine=/elsewhere/engine.rlib",
            ]),
            Some(Path::new("/generated")),
        );

        assert_eq!(unit, None);
        assert!(dependencies.is_empty());
    }

    #[test]
    fn a_build_script_run_depends_on_its_compilation() {
        let (unit, dependencies) = build_script_run_unit(
            Some(Path::new("/t/debug/build/ring-77ee/out")),
            Path::new("/t/debug/build/ring-66ff/build-script-build"),
        );

        assert_eq!(unit.as_deref(), Some("run-77ee"));
        assert_eq!(dependencies, ["66ff"]);
    }
}
