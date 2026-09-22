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
    for (variable, launcher) in [
        ("CMAKE_C_COMPILER_LAUNCHER", C_LAUNCHER),
        ("CMAKE_CXX_COMPILER_LAUNCHER", CXX_LAUNCHER),
    ] {
        let installed = directory.join(super::shim_file_name(launcher));
        link_path_shim(&executable, &installed)?;
        write_launcher_script(directory, variable, launcher, &installed)?;
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
        let mut scripts = Vec::new();
        for (variable, launcher) in rewrite_compilers(&mut arguments, &read_map(COMPILERS)) {
            // A launcher chosen on the command line is left to the caller.
            // One exported in the environment still runs the script, which
            // installs it in place of a stale mbx launcher: CMake itself would
            // only have read it into a fresh cache.
            if defines(&arguments, variable) {
                continue;
            }
            let directory = invoked.parent().unwrap();
            let script = directory.join(launcher_script_name(launcher));
            if script.is_file() {
                scripts.extend([OsString::from("-C"), script.into_os_string()]);
            } else if std::env::var_os(variable).is_none() {
                command.env(variable, directory.join(super::shim_file_name(launcher)));
            }
        }
        arguments.splice(0..0, scripts);
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

/// File name of the initial-cache script that installs `launcher`.
fn launcher_script_name(launcher: &str) -> String {
    format!("{launcher}.cmake")
}

/// Write the `-C` script that points a CMake cache at this binary's launcher.
///
/// The launcher cannot simply be offered through the environment: CMake reads
/// `CMAKE_<LANG>_COMPILER_LAUNCHER` from there only for a fresh cache. Shims
/// live per mbx binary, so a build tree configured before an upgrade would
/// otherwise keep the previous binary's launcher, and fail every compile once
/// that binary was removed. The script runs against whichever cache CMake
/// loads -- `-B`, the working directory, or a preset's `binaryDir` alike --
/// and replaces only an empty entry or another mbx launcher, never one the
/// build chose for itself. A launcher the caller exports takes the place of
/// ours, just as CMake would have seeded a fresh cache with it.
fn write_launcher_script(
    directory: &Path,
    variable: &str,
    launcher: &str,
    installed: &Path,
) -> Result<()> {
    let quoted = cmake_path(installed)
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$");
    // Either separator: a launcher CMake seeded from the environment, or one
    // an older mbx recorded, keeps Windows backslashes.
    let separator = "[/\\\\]";
    let suffix = if cfg!(windows) {
        "\\\\.[Ee][Xx][Ee]"
    } else {
        ""
    };
    let script = format!(
        "# Written by mbx: point this build at the running mbx's compiler launcher.\n\
         get_property(mbx_launcher CACHE {variable} PROPERTY VALUE)\n\
         if(NOT mbx_launcher OR mbx_launcher MATCHES \"{separator}{launcher}{suffix}$\")\n  \
         if(NOT \"$ENV{{{variable}}}\" STREQUAL \"\")\n    \
         set({variable} \"$ENV{{{variable}}}\" CACHE STRING \"Compiler launcher\" FORCE)\n  \
         else()\n    \
         set({variable} \"{quoted}\" CACHE STRING \"Compiler launcher installed by mbx\" FORCE)\n  \
         endif()\n\
         endif()\n\
         unset(mbx_launcher)\n"
    );
    let destination = directory.join(launcher_script_name(launcher));
    if std::fs::read_to_string(&destination).is_ok_and(|existing| existing == script) {
        return Ok(());
    }
    // Staged and renamed: a concurrent CMake run may be reading it.
    let staging = directory.join(format!(
        ".{}.{}",
        launcher_script_name(launcher),
        std::process::id()
    ));
    std::fs::write(&staging, script)?;
    std::fs::rename(&staging, &destination)?;
    Ok(())
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

    /// Run the generated script against a real cache holding `cached`, and
    /// return what the cache holds afterwards.
    fn launcher_after_script(cached: &str) -> Option<String> {
        let cmake = resolve_on_path(&super::super::shim_file_name("cmake"))?;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let build = directory.path().join("build");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.10)\nproject(probe NONE)\n",
        )
        .unwrap();
        let installed = directory
            .path()
            .join(super::super::shim_file_name(C_LAUNCHER));
        write_launcher_script(
            directory.path(),
            "CMAKE_C_COMPILER_LAUNCHER",
            C_LAUNCHER,
            &installed,
        )
        .unwrap();
        let configure = |arguments: &[OsString]| {
            let status = Command::new(&cmake)
                .args(arguments)
                .arg("-S")
                .arg(&source)
                .arg("-B")
                .arg(&build)
                .env_remove("CMAKE_C_COMPILER_LAUNCHER")
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
        };
        configure(&[format!("-DCMAKE_C_COMPILER_LAUNCHER:STRING={cached}").into()]);
        configure(&[
            "-C".into(),
            directory
                .path()
                .join(launcher_script_name(C_LAUNCHER))
                .into(),
        ]);
        let cache = std::fs::read_to_string(build.join("CMakeCache.txt")).unwrap();
        let value = cache
            .lines()
            .find_map(|line| line.strip_prefix("CMAKE_C_COMPILER_LAUNCHER:STRING="))
            .map(ToOwned::to_owned);
        assert!(value.is_some(), "{cache}");
        let ours = cmake_path(&installed);
        value.map(|value| if value == ours { "ours".into() } else { value })
    }

    #[test]
    fn a_stale_mbx_launcher_is_replaced_whichever_separator_it_uses() {
        let name = super::super::shim_file_name(C_LAUNCHER);
        for cached in [
            format!("/old/shims/native/id/{name}"),
            format!("C:\\old\\shims\\native\\id\\{name}"),
        ] {
            let Some(after) = launcher_after_script(&cached) else {
                return;
            };
            assert_eq!(after, "ours", "{cached} should be replaced");
        }
        let Some(after) = launcher_after_script("/usr/bin/ccache") else {
            return;
        };
        assert_eq!(after, "/usr/bin/ccache", "a user's launcher must stay");
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
