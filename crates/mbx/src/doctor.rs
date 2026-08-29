//! Installation and connectivity diagnostics.

use crate::config::Config;
use crate::policy;
use eyre::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct Check {
    severity: Severity,
    name: &'static str,
    detail: String,
}

impl Check {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            severity: Severity::Pass,
            name,
            detail: detail.into(),
        }
    }

    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warn,
            name,
            detail: detail.into(),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            severity: Severity::Fail,
            name,
            detail: detail.into(),
        }
    }
}

/// Run all diagnostics and fail only when mbx cannot operate as configured.
pub fn run(config: &Config) -> Result<ExitCode> {
    run_formatted(config, false, None)
}

pub(crate) fn run_formatted(
    config: &Config,
    json: bool,
    toolchain: Option<&str>,
) -> Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let checks = runtime.block_on(check(config, toolchain));
    render(&checks, json)
}

fn render(checks: &[Check], json: bool) -> Result<ExitCode> {
    let failures = checks
        .iter()
        .filter(|check| check.severity == Severity::Fail)
        .count();
    let warnings = checks
        .iter()
        .filter(|check| check.severity == Severity::Warn)
        .count();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&DoctorReport {
                version: 1,
                checks,
                failures,
                warnings,
            })?
        );
    } else {
        for check in checks {
            let marker = match check.severity {
                Severity::Pass => "ok",
                Severity::Warn => "warn",
                Severity::Fail => "FAIL",
            };
            println!("{marker:>4}  {:<12} {}", check.name, check.detail);
        }
        println!("\n{failures} failures, {warnings} warnings");
    }
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub(crate) fn run_loaded(
    config: Result<Config>,
    json: bool,
    toolchain: Option<&str>,
) -> Result<ExitCode> {
    match config {
        Ok(config) => run_formatted(&config, json, toolchain),
        Err(error) => render(&[Check::fail("config", format!("{error:#}"))], json),
    }
}

#[derive(serde::Serialize)]
struct DoctorReport<'a> {
    version: u8,
    checks: &'a [Check],
    failures: usize,
    warnings: usize,
}

async fn check(config: &Config, toolchain: Option<&str>) -> Vec<Check> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let mut checks = vec![
        command_check("cargo", &cargo, toolchain),
        command_check("rustc", &rustc, toolchain),
    ];
    checks.push(cache_check(&config.cache_dir));
    checks.push(Check::pass(
        "config",
        format!(
            "{} budget, automatic gc {}, managed targets {} at {}",
            bytesize::ByteSize::b(config.gc.max_bytes).display().iec(),
            enabled(config.gc.auto),
            enabled(config.target.views),
            config.target.root.display(),
        ),
    ));
    checks.push(reflink_check(&config.cache_dir));
    checks.push(setup_check());
    checks.extend(remote_checks(config).await);
    checks
}

fn enabled(value: bool) -> &'static str {
    if value { "enabled" } else { "disabled" }
}

/// The arguments that ask a command for its version, under a toolchain when one
/// was named.
///
/// `mbx +1.91 doctor` is a question about 1.91, so the answer has to come from
/// 1.91's compiler rather than from whichever one the shim resolves by default.
/// A `CARGO` or `RUSTC` that is not a rustup shim rejects the `+`, and the
/// check reports that failure rather than quietly describing another toolchain.
fn version_arguments(toolchain: Option<&str>) -> Vec<String> {
    match toolchain {
        Some(toolchain) => vec![format!("+{toolchain}"), "--version".into()],
        None => vec!["--version".into()],
    }
}

fn command_check(name: &'static str, command: &OsStr, toolchain: Option<&str>) -> Check {
    let arguments = version_arguments(toolchain);
    match Command::new(command).args(&arguments).output() {
        Ok(output) if output.status.success() => Check::pass(
            name,
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ),
        Ok(output) => Check::fail(
            name,
            failure_detail(&arguments, output.status, &output.stderr),
        ),
        Err(error) => Check::fail(
            name,
            format!("could not execute {}: {error}", command.to_string_lossy()),
        ),
    }
}

/// Why a version query failed, in one line.
///
/// The arguments are named because a named toolchain is among them, and the
/// reason a toolchain query fails — most often that the toolchain is not
/// installed — is on stderr. Reporting the exit status alone leaves the reader
/// with a diagnostic that diagnoses nothing.
///
/// The status is taken as anything printable rather than as an `ExitStatus`,
/// which has no portable constructor: a test would otherwise have to spawn a
/// process that fails on every platform this runs on to reach one line of
/// formatting.
fn failure_detail(arguments: &[String], status: impl std::fmt::Display, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let arguments = arguments.join(" ");
    if reason.is_empty() {
        format!("{arguments} exited with {status}")
    } else {
        format!("{arguments} exited with {status}: {reason}")
    }
}

fn cache_check(cache_dir: &Path) -> Check {
    if let Err(error) = std::fs::create_dir_all(cache_dir) {
        return Check::fail("cache", format!("{}: {error}", cache_dir.display()));
    }
    match tempfile::NamedTempFile::new_in(cache_dir) {
        Ok(_) => Check::pass("cache", format!("{} is writable", cache_dir.display())),
        Err(error) => Check::fail(
            "cache",
            format!("{} is not writable: {error}", cache_dir.display()),
        ),
    }
}

fn reflink_check(cache_dir: &Path) -> Check {
    let Ok(directory) = tempfile::tempdir_in(cache_dir) else {
        return Check::warn("reflink", "not tested because the cache is not writable");
    };
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    if let Err(error) = std::fs::write(&source, b"mbx reflink probe") {
        return Check::warn("reflink", format!("probe could not be written: {error}"));
    }
    match reflink_copy::reflink(&source, &destination) {
        Ok(()) => Check::pass("reflink", "supported by the cache filesystem"),
        Err(error) => Check::warn(
            "reflink",
            format!("unavailable ({error}); restored outputs will be copied"),
        ),
    }
}

/// Report the optional plain-cargo wrapper.
///
/// Its absence is not a problem to fix: `mbx <cargo command>` is how mbx is
/// meant to be used, and it does strictly more. Only a half-installed or
/// displaced wrapper warns, because that is a state somebody meant to be in
/// and is not.
fn setup_check() -> Check {
    let Some((config_path, expected_shim)) = setup_paths() else {
        return Check::warn("setup", "platform Cargo paths could not be determined");
    };
    let contents = match std::fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Check::pass("setup", NO_PLAIN_CARGO_WRAPPER);
        }
        Err(error) => {
            return Check::warn(
                "setup",
                format!("{} could not be read: {error}", config_path.display()),
            );
        }
    };
    let document = match contents.parse::<toml_edit::DocumentMut>() {
        Ok(document) => document,
        Err(error) => {
            return Check::warn(
                "setup",
                format!("{} is invalid TOML: {error}", config_path.display()),
            );
        }
    };
    let wrapper = document
        .get("build")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|build| build.get("rustc-wrapper"))
        .and_then(toml_edit::Item::as_str);
    match wrapper {
        Some(wrapper) if Path::new(wrapper) == expected_shim && expected_shim.is_file() => {
            Check::pass("setup", format!("{} is installed", expected_shim.display()))
        }
        Some(wrapper) if Path::new(wrapper) == expected_shim => Check::warn(
            "setup",
            format!("configured wrapper is missing: {}", expected_shim.display()),
        ),
        Some(wrapper) => Check::warn("setup", format!("Cargo uses another wrapper: {wrapper}")),
        None => Check::pass("setup", NO_PLAIN_CARGO_WRAPPER),
    }
}

/// Said when nothing is installed, which is the ordinary case.
const NO_PLAIN_CARGO_WRAPPER: &str = "no plain-cargo wrapper installed; mbx wraps cargo directly";

fn setup_paths() -> Option<(PathBuf, PathBuf)> {
    let data = dirs::data_local_dir()?;
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))?;
    let config = if cargo_home.join("config.toml").exists() || !cargo_home.join("config").exists() {
        cargo_home.join("config.toml")
    } else {
        cargo_home.join("config")
    };
    let shim = data.join("mbx").join("bin").join(if cfg!(windows) {
        format!("{}.exe", crate::session::RUSTC_SHIM_STEM)
    } else {
        crate::session::RUSTC_SHIM_STEM.into()
    });
    Some((config, shim))
}

async fn remote_checks(config: &Config) -> Vec<Check> {
    let release_context = policy::release_context();
    let effective_mode = if release_context {
        None
    } else {
        policy::effective_remote_cache_mode(config.remote.mode)
    };
    remote_checks_with_policy(config, release_context, effective_mode).await
}

async fn remote_checks_with_policy(
    config: &Config,
    release_context: bool,
    effective_mode: Option<mbx_cache_core::RemoteCacheMode>,
) -> Vec<Check> {
    let Some(base_url) = config
        .remote
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return vec![Check::pass(
            "remote",
            "not configured; using the local cache",
        )];
    };
    let effective = if release_context {
        "disabled in this release context".to_string()
    } else {
        effective_mode.map_or_else(
            || "disabled by cache write policy".to_string(),
            |mode| mode.to_string(),
        )
    };
    let policy = Check::pass(
        "policy",
        format!("configured {}, effective {effective}", config.remote.mode),
    );
    if effective_mode.is_none() {
        return vec![
            policy,
            Check::pass("remote", "probe skipped because remote caching is disabled"),
        ];
    }
    let namespace = config
        .remote
        .namespace
        .as_deref()
        .unwrap_or_default()
        .trim();
    // The build builds its client the same way, so a configuration it would
    // refuse is reported here rather than discovered mid-compilation.
    let client = match crate::remote::remote_client(config) {
        Ok(Some(client)) => client,
        Ok(None) => {
            return vec![
                policy,
                Check::pass("remote", "not configured; using the local cache"),
            ];
        }
        Err(error) => return vec![policy, Check::fail("remote", format!("{error:#}"))],
    };
    let remote = match client
        .check_connection()
        .await
        .wrap_err("connection check failed")
    {
        Ok(()) => Check::pass("remote", format!("{base_url} ({namespace}) is compatible")),
        Err(error) => Check::fail("remote", format!("{error:#}")),
    };
    vec![policy, remote]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_cache_passes_and_leaves_no_probe() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(cache_check(directory.path()).severity, Severity::Pass);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_named_toolchain_is_the_one_the_version_checks_ask() {
        assert_eq!(version_arguments(None), ["--version"]);
        assert_eq!(version_arguments(Some("1.91")), ["+1.91", "--version"]);
    }

    #[test]
    fn a_failed_version_query_reports_what_it_asked_and_why_it_failed() {
        let arguments = version_arguments(Some("1.91"));
        let status = "exit status: 1";

        let detail = failure_detail(
            &arguments,
            status,
            b"error: toolchain '1.91' is not installed\nnote: run rustup\n",
        );

        assert_eq!(
            detail,
            "+1.91 --version exited with exit status: 1: error: toolchain '1.91' is not installed"
        );
        // Nothing on stderr leaves the status to end the line, rather than a
        // dangling separator.
        assert_eq!(
            failure_detail(&arguments, status, b"  \n"),
            "+1.91 --version exited with exit status: 1"
        );
    }

    #[test]
    fn missing_remote_namespace_fails() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config {
            cache_dir: directory.path().to_path_buf(),
            stats_report: None,
            verify: false,
            incremental: false,
            share_out_dir: false,
            events: false,
            cc: false,
            remote: Default::default(),
            http: Default::default(),
            gc: Default::default(),
            scheduler: Default::default(),
            target: crate::config::TargetSettings {
                views: true,
                root: directory.path().join("targets"),
            },
        };
        config.remote.url = Some("https://cache.example".into());
        config.remote.mode = mbx_cache_core::RemoteCacheMode::ReadWrite;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let checks = runtime.block_on(remote_checks_with_policy(
            &config,
            false,
            Some(mbx_cache_core::RemoteCacheMode::ReadOnly),
        ));
        let failure = checks
            .iter()
            .find(|check| check.severity == Severity::Fail)
            .expect("the missing namespace should fail");
        assert!(failure.detail.contains("namespace"));
    }

    #[test]
    fn disabled_remote_does_not_require_a_namespace_or_client() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config {
            cache_dir: directory.path().to_path_buf(),
            stats_report: None,
            verify: false,
            incremental: false,
            share_out_dir: false,
            events: false,
            cc: false,
            remote: Default::default(),
            http: Default::default(),
            gc: Default::default(),
            scheduler: Default::default(),
            target: crate::config::TargetSettings {
                views: true,
                root: directory.path().join("targets"),
            },
        };
        config.remote.url = Some("not a URL".into());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let checks = runtime.block_on(remote_checks_with_policy(&config, false, None));

        assert!(checks.iter().all(|check| check.severity == Severity::Pass));
        assert!(checks.iter().any(|check| {
            check.name == "policy" && check.detail.contains("disabled by cache write policy")
        }));
        assert!(
            checks
                .iter()
                .any(|check| { check.name == "remote" && check.detail.contains("probe skipped") })
        );
    }
}
