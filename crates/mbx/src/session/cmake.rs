//! Keep compiler identity stable when cmake-rs inherits mbx's CC shims.
//!
//! CMake discards configuration options when CMAKE_<LANG>_COMPILER changes.
//! Translate our compiler paths to their real drivers before configuration,
//! and cache C/C++ through launchers instead. ASM keeps its real driver too,
//! but CMake does not support an ASM compiler launcher.

use super::shims::{CcShims, is_target_triple, link_path_shim, resolve_on_path};
use super::{record_cc_bypass, reserve_stderr_for_compiler, run_transparent_cc, session_socket};
use eyre::Result;
use mbx_cache_cc::CcLanguage;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const PROGRAMS: &str = "MBX_CMAKE_PROGRAMS";
const COMPILERS: &str = "MBX_CMAKE_COMPILERS";
const SHIM: &str = "mbx-cmake";
const C_LAUNCHER: &str = "mbx-cmake-launch-c";
const CXX_LAUNCHER: &str = "mbx-cmake-launch-cxx";

pub(super) fn environment(
    directory: &Path,
    compilers: &CcShims,
    build_environment: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let executable = std::env::current_exe()?;
    let mut environment = BTreeMap::new();
    let mut programs = BTreeMap::new();
    let mut choices: BTreeMap<_, _> = std::env::vars()
        .chain(
            build_environment
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        )
        .filter(|(name, _)| {
            matches!(name.as_str(), "CMAKE" | "HOST_CMAKE" | "TARGET_CMAKE")
                || name.strip_prefix("CMAKE_").is_some_and(is_target_triple)
        })
        .collect();
    if !choices.contains_key("CMAKE")
        && let Some(program) = resolve_on_path(&super::shim_file_name("cmake"))
    {
        choices.insert("CMAKE".into(), program.to_string_lossy().into_owned());
    }
    for (variable, program) in choices {
        let name = format!("{SHIM}-{}", variable.to_ascii_lowercase().replace('.', "_"));
        let shim = directory.join(super::shim_file_name(&name));
        // Nested sessions must keep the outer shim's original program.
        let program = build_environment
            .get(PROGRAMS)
            .and_then(|encoded| serde_json::from_str::<BTreeMap<String, PathBuf>>(encoded).ok())
            .unwrap_or_else(|| read_map(PROGRAMS))
            .remove(
                Path::new(&program)
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .unwrap_or_default(),
            )
            .unwrap_or_else(|| PathBuf::from(program));
        link_path_shim(&executable, &shim)?;
        programs.insert(name, program);
        environment.insert(variable, shim.to_string_lossy().into_owned());
    }
    if programs.is_empty() {
        return Ok(environment);
    }
    let mut pins = BTreeMap::new();
    for (shim, real) in compilers.cc.iter().chain(compilers.cxx.iter()) {
        pins.insert(cmake_path(shim), real.clone());
    }
    for compiler in &compilers.targeted {
        pins.insert(cmake_path(&compiler.shim), compiler.real.clone());
    }
    for launcher in [C_LAUNCHER, CXX_LAUNCHER] {
        link_path_shim(
            &executable,
            &directory.join(super::shim_file_name(launcher)),
        )?;
    }
    environment.insert(PROGRAMS.into(), serde_json::to_string(&programs)?);
    environment.insert(COMPILERS.into(), serde_json::to_string(&pins)?);
    Ok(environment)
}

fn read_map(name: &str) -> BTreeMap<String, PathBuf> {
    std::env::var(name)
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
}

fn cmake_path(path: &Path) -> String {
    let path = path.to_string_lossy().into_owned();
    // cmake-rs writes forward slashes even when cc-rs returned a Windows path.
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path
    }
}

/// Dispatch before mbx's CLI: CMake launchers also survive outside a session.
pub fn dispatch() -> Option<ExitCode> {
    let invoked = PathBuf::from(std::env::args_os().next()?);
    let name = invoked.file_stem()?.to_str()?;
    if matches!(name, C_LAUNCHER | CXX_LAUNCHER) {
        let language = if name == C_LAUNCHER {
            CcLanguage::C
        } else {
            CcLanguage::Cxx
        };
        reserve_stderr_for_compiler();
        let mut arguments = std::env::args_os().skip(1);
        let Some(compiler) = arguments.next() else {
            eprintln!("mbx[error]: CMake launcher requires a compiler");
            return Some(ExitCode::FAILURE);
        };
        let arguments: Vec<_> = arguments.collect();
        if session_socket().is_some() {
            match crate::cc::compile(&compiler, &arguments, language) {
                Ok(code) => return Some(code),
                Err(error) => record_cc_bypass(&error),
            }
        }
        return Some(run_transparent_cc(compiler, arguments));
    }
    if !name.starts_with(&format!("{SHIM}-")) {
        return None;
    }
    let program = read_map(PROGRAMS)
        .remove(name)
        .unwrap_or_else(|| "cmake".into());
    let mut arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let mut command = Command::new(program);
    // --build, --install, -E, -P, and probes must pass through verbatim.
    if !arguments.iter().any(|arg| {
        matches!(
            arg.to_str(),
            Some("--build" | "--install" | "-E" | "-P" | "--version" | "--help")
        )
    }) {
        let mut replacements = Vec::new();
        for (variable, launcher) in rewrite_compilers(&mut arguments, &read_map(COMPILERS)) {
            // Environment defaults preserve launchers chosen on the command
            // line, in an existing cache, or by the caller's environment.
            if std::env::var_os(variable).is_some() {
                continue;
            }
            let ours = invoked
                .parent()
                .unwrap()
                .join(super::shim_file_name(launcher));
            // Except a cache that recorded another mbx binary's launcher.
            // The environment only seeds a fresh cache, and shims live per
            // binary, so after an upgrade the old path would be used until
            // its binary went away and then fail every compile.
            if !defines(&arguments, variable)
                && cached_launcher(&build_directory(&arguments), variable)
                    .is_some_and(|cached| replaces_launcher(&cached, launcher, &cmake_path(&ours)))
            {
                replacements.push(OsString::from(format!(
                    "-D{variable}:STRING={}",
                    cmake_path(&ours)
                )));
            } else {
                command.env(variable, ours);
            }
        }
        arguments.extend(replacements);
    }
    let status = command.args(arguments).status();
    Some(match status {
        Ok(status) => crate::materialize::exit_code(status),
        Err(error) => {
            eprintln!("mbx[error]: failed to execute CMake: {error}");
            ExitCode::FAILURE
        }
    })
}

/// The build tree a configure invocation writes, as CMake chooses it.
fn build_directory(arguments: &[OsString]) -> PathBuf {
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "-B" {
            if let Some(directory) = arguments.next() {
                return PathBuf::from(directory);
            }
        } else if let Some(directory) = argument.to_str().and_then(|text| text.strip_prefix("-B")) {
            return PathBuf::from(directory);
        }
    }
    PathBuf::from(".")
}

/// Whether the command line sets `variable` itself.
fn defines(arguments: &[OsString], variable: &str) -> bool {
    let mut after_define = false;
    arguments.iter().any(|argument| {
        let text = argument.to_str().unwrap_or_default();
        let definition = text.strip_prefix("-D").or(after_define.then_some(text));
        after_define = text == "-D";
        definition.is_some_and(|definition| {
            definition
                .split(['=', ':'])
                .next()
                .is_some_and(|name| name == variable)
        })
    })
}

/// The value an existing cache in `build` holds for `variable`.
fn cached_launcher(build: &Path, variable: &str) -> Option<String> {
    let cache = std::fs::read_to_string(build.join("CMakeCache.txt")).ok()?;
    cache.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.split(':').next()? == variable).then(|| value.to_string())
    })
}

/// Whether a cached launcher is an mbx launcher other than this binary's.
///
/// Only one named like ours is replaced: a launcher the build chose for
/// itself is the build's business.
fn replaces_launcher(cached: &str, launcher: &str, ours: &str) -> bool {
    cached != ours
        && Path::new(cached)
            .file_stem()
            .is_some_and(|stem| stem == launcher)
}

fn rewrite_compilers(
    arguments: &mut [OsString],
    pins: &BTreeMap<String, PathBuf>,
) -> Vec<(&'static str, &'static str)> {
    let mut launchers = Vec::new();
    // Both -DNAME[:TYPE]=value and -D NAME[:TYPE]=value are accepted by CMake.
    // Only exact paths installed by this session are ours to replace.
    let mut after_define = false;
    for argument in arguments {
        let Some(text) = argument.to_str() else {
            after_define = false;
            continue;
        };
        let definition = if let Some(value) = text.strip_prefix("-D") {
            value
        } else if after_define {
            text
        } else {
            continue;
        };
        after_define = text == "-D";
        let Some((key, value)) = definition.split_once('=') else {
            continue;
        };
        let variable = key.split(':').next().unwrap();
        if !matches!(
            variable,
            "CMAKE_C_COMPILER" | "CMAKE_CXX_COMPILER" | "CMAKE_ASM_COMPILER"
        ) {
            continue;
        }
        let Some(real) = pins.get(&cmake_path(Path::new(value))) else {
            continue;
        };
        match variable {
            "CMAKE_C_COMPILER" => launchers.push(("CMAKE_C_COMPILER_LAUNCHER", C_LAUNCHER)),
            "CMAKE_CXX_COMPILER" => launchers.push(("CMAKE_CXX_COMPILER_LAUNCHER", CXX_LAUNCHER)),
            _ => {}
        }
        let prefix = if text.starts_with("-D") { "-D" } else { "" };
        *argument = format!("{prefix}{key}={}", cmake_path(real)).into();
    }
    launchers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_definitions_keep_types_spaces_and_unrelated_options() {
        let mut arguments: Vec<OsString> = [
            "-S",
            "source tree",
            "-D",
            "CMAKE_C_COMPILER:FILEPATH=/cache dir/mbx-c",
            "-DCMAKE_CXX_COMPILER=/cache dir/mbx-cxx",
            "-DCMAKE_ASM_COMPILER=/cache dir/mbx-c",
            "-DCMAKE_C_COMPILER_LAUNCHER=user-launcher",
            "-DBUILD_TESTING=OFF",
            "-DSOME_COMPILER=/cache dir/mbx-c",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        let pins = BTreeMap::from([
            ("/cache dir/mbx-c".into(), "/tool chain/cc".into()),
            ("/cache dir/mbx-cxx".into(), "/tool chain/c++".into()),
        ]);
        let launchers = rewrite_compilers(&mut arguments, &pins);
        assert_eq!(
            arguments,
            [
                "-S",
                "source tree",
                "-D",
                "CMAKE_C_COMPILER:FILEPATH=/tool chain/cc",
                "-DCMAKE_CXX_COMPILER=/tool chain/c++",
                "-DCMAKE_ASM_COMPILER=/tool chain/cc",
                "-DCMAKE_C_COMPILER_LAUNCHER=user-launcher",
                "-DBUILD_TESTING=OFF",
                "-DSOME_COMPILER=/cache dir/mbx-c",
            ]
            .map(OsString::from)
        );
        assert_eq!(
            launchers,
            vec![
                ("CMAKE_C_COMPILER_LAUNCHER", C_LAUNCHER),
                ("CMAKE_CXX_COMPILER_LAUNCHER", CXX_LAUNCHER),
            ]
        );
    }

    #[test]
    fn another_binarys_cached_launcher_is_replaced_and_a_users_is_kept() {
        let build = tempfile::tempdir().unwrap();
        std::fs::write(
            build.path().join("CMakeCache.txt"),
            "//comment\nCMAKE_C_COMPILER_LAUNCHER:STRING=/shims/native/old/mbx-cmake-launch-c\n\
             CMAKE_CXX_COMPILER_LAUNCHER:STRING=/usr/bin/ccache\n",
        )
        .unwrap();
        let arguments: Vec<OsString> = ["-S", "src", "-B"]
            .into_iter()
            .map(OsString::from)
            .chain([build.path().as_os_str().to_owned()])
            .collect();
        let directory = build_directory(&arguments);
        assert_eq!(directory, build.path());
        let ours = "/shims/native/new/mbx-cmake-launch-c";
        let cached = cached_launcher(&directory, "CMAKE_C_COMPILER_LAUNCHER").unwrap();
        assert!(replaces_launcher(&cached, C_LAUNCHER, ours));
        assert!(!replaces_launcher(ours, C_LAUNCHER, ours));
        let users = cached_launcher(&directory, "CMAKE_CXX_COMPILER_LAUNCHER").unwrap();
        assert!(!replaces_launcher(&users, CXX_LAUNCHER, ours));
        assert_eq!(
            cached_launcher(&directory, "CMAKE_ASM_COMPILER_LAUNCHER"),
            None
        );
    }

    #[test]
    fn launchers_set_on_the_command_line_are_recognized() {
        let arguments: Vec<OsString> = [
            "-D",
            "CMAKE_C_COMPILER_LAUNCHER=user",
            "-DCMAKE_CXX_COMPILER_LAUNCHER:STRING=user",
            "-DCMAKE_C_COMPILER_LAUNCHER_EXTRA=unrelated",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        assert!(defines(&arguments, "CMAKE_C_COMPILER_LAUNCHER"));
        assert!(defines(&arguments, "CMAKE_CXX_COMPILER_LAUNCHER"));
        assert!(!defines(&arguments[3..], "CMAKE_C_COMPILER_LAUNCHER"));
        assert_eq!(build_directory(&arguments), PathBuf::from("."));
        assert_eq!(
            build_directory(&[OsString::from("-Bbuild dir")]),
            PathBuf::from("build dir")
        );
    }

    #[test]
    fn compiler_overrides_are_not_rewritten_or_wrapped() {
        let mut arguments = [OsString::from("-DCMAKE_C_COMPILER=/custom/mbx-c")];
        let original = arguments.clone();
        let pins = BTreeMap::from([("/cache/mbx-c".into(), "/usr/bin/cc".into())]);
        assert!(rewrite_compilers(&mut arguments, &pins).is_empty());
        assert_eq!(arguments, original);
    }
}
