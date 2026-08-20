//! The mbx command line.

use crate::config::Config;
use crate::session::CacheSession;
use crate::{policy, store};
use bytesize::ByteSize;
use clap::{Args, Parser, Subcommand};
use eyre::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Parser)]
#[command(name = "mbx", version, about = "A build cache for Rust projects")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run cargo with the build cache enabled.
    Build(BuildArgs),
    /// Evict cached objects until the store fits a size budget.
    Gc(GcArgs),
    /// Inspect the local store.
    Cache(CacheArgs),
}

#[derive(Args)]
struct BuildArgs {
    /// Arguments passed to cargo, starting with the subcommand.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    arguments: Vec<String>,
}

#[derive(Args)]
struct GcArgs {
    /// Size the store may occupy afterwards, for example 20GB.
    #[arg(long, value_name = "SIZE")]
    max_size: ByteSize,
}

#[derive(Args)]
struct CacheArgs {
    #[command(subcommand)]
    command: CacheCommands,
}

#[derive(Subcommand)]
enum CacheCommands {
    /// Print the store directory.
    Dir,
    /// Summarize what the store holds.
    Stats,
}

/// Parse the command line and run it.
pub fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let config = Config::load()?;
    match cli.command {
        Commands::Build(args) => build(&config, &args.arguments),
        Commands::Gc(args) => gc(&config, args.max_size.as_u64()).map(|()| ExitCode::SUCCESS),
        Commands::Cache(args) => match args.command {
            CacheCommands::Dir => {
                println!("{}", config.store_dir().display());
                Ok(ExitCode::SUCCESS)
            }
            CacheCommands::Stats => cache_stats(&config).map(|()| ExitCode::SUCCESS),
        },
    }
}

fn build(config: &Config, arguments: &[String]) -> Result<ExitCode> {
    validate_build_arguments(arguments)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    // Release builds publish artifacts that must not depend on a cache, and a
    // tag build has no later build to share with anyway.
    if policy::release_context() {
        log::warn!("the build cache is disabled for release builds");
        return run_cargo(&cargo, arguments, BTreeMap::new());
    }

    let working_dir = std::env::current_dir()?;
    let roots = resolve_roots(&cargo, arguments, &working_dir);
    let session_dir = tempfile::Builder::new().prefix("mbx-session-").tempdir()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let session = CacheSession::start(session_dir.path(), config).await?;
        let mut environment = inherited_environment(|name| std::env::var(name).ok());
        let run = session
            .begin(
                &roots.workspace_root,
                &roots.target_dir,
                arguments,
                &mut environment,
            )
            .await;

        let status = run_cargo(&cargo, arguments, environment);

        // A failed build has nothing to publish, and its manifest would record
        // actions that never completed.
        match (&status, run) {
            (Ok(code), Some(run)) if *code == ExitCode::SUCCESS => {
                if let Err(error) = run.commit().await {
                    log::warn!("the build manifest was not committed: {error}");
                }
            }
            _ => {}
        }

        match session.finish().await {
            Ok(stats) => crate::session::display_stats(&stats, config),
            Err(error) => log::warn!("the cache session did not shut down cleanly: {error}"),
        }
        status
    })
}

fn run_cargo(
    cargo: &std::ffi::OsStr,
    arguments: &[String],
    environment: BTreeMap<String, String>,
) -> Result<ExitCode> {
    let mut command = Command::new(cargo);
    command.args(arguments);
    command.envs(environment);
    let status = command
        .status()
        .wrap_err_with(|| format!("failed to run {}", cargo.to_string_lossy()))?;
    Ok(exit_code(status))
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    use std::os::unix::process::ExitStatusExt as _;
    // A signalled cargo has no exit code of its own; report the conventional
    // 128 + signal so callers can tell it apart from a clean failure.
    match (status.code(), status.signal()) {
        (Some(code), _) => ExitCode::from(code as u8),
        (None, Some(signal)) => ExitCode::from(128u8.saturating_add(signal as u8)),
        (None, None) => ExitCode::FAILURE,
    }
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    match status.code() {
        Some(code) => ExitCode::from(code as u8),
        None => ExitCode::FAILURE,
    }
}

fn gc(config: &Config, max_bytes: u64) -> Result<()> {
    let store = config.store_dir();
    let outcome = store::gc(&store, max_bytes)?;
    println!(
        "evicted {} objects and {} action results ({}); {} remain",
        outcome.removed_objects,
        outcome.removed_action_results,
        ByteSize::b(outcome.removed_bytes).display().iec(),
        ByteSize::b(outcome.remaining_bytes).display().iec(),
    );
    Ok(())
}

fn cache_stats(config: &Config) -> Result<()> {
    let store = config.store_dir();
    let stats = store::stats(&store)?;
    println!("store: {}", store.display());
    println!(
        "objects: {} ({})",
        stats.objects,
        ByteSize::b(stats.object_bytes).display().iec()
    );
    println!(
        "action results: {} ({})",
        stats.action_results,
        ByteSize::b(stats.action_result_bytes).display().iec()
    );
    println!(
        "total: {}",
        ByteSize::b(stats.total_bytes()).display().iec()
    );
    Ok(())
}

/// Find the workspace root above `start`.
///
/// The outermost directory holding a `Cargo.lock` is preferred, since that is
/// the one cargo resolves against; a directory holding only a `Cargo.toml`
/// stands in when there is no lockfile yet.
fn workspace_root(start: &Path) -> PathBuf {
    let mut lockfile = None;
    let mut manifest = None;
    for directory in start.ancestors() {
        if directory.join("Cargo.lock").is_file() {
            lockfile = Some(directory.to_path_buf());
        }
        if directory.join("Cargo.toml").is_file() {
            manifest = Some(directory.to_path_buf());
        }
    }
    lockfile.or(manifest).unwrap_or_else(|| start.to_path_buf())
}

/// Settings the shim maps out of its cache keys.
#[derive(Debug, PartialEq, Eq)]
struct Roots {
    workspace_root: PathBuf,
    target_dir: PathBuf,
}

/// Carry a `RUSTC_WRAPPER` the caller already configured into the session.
///
/// The session records it so the shim can defer to it; without this it would be
/// dropped silently and whatever it does would stop happening.
fn inherited_environment(get_env: impl Fn(&str) -> Option<String>) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    if let Some(wrapper) = get_env("RUSTC_WRAPPER").filter(|value| !value.is_empty()) {
        environment.insert("RUSTC_WRAPPER".into(), wrapper);
    }
    environment
}

/// Resolve the workspace root and target directory cargo will actually use.
///
/// Cargo is the only authority: the target directory can come from the
/// environment, a flag, or cargo's own configuration, and `--manifest-path` can
/// move the whole build elsewhere. Inference is kept as a fallback, and costs
/// only cache hits when it is wrong, since an unmapped path bypasses.
fn resolve_roots(cargo: &std::ffi::OsStr, arguments: &[String], working_dir: &Path) -> Roots {
    let reported = cargo_roots(cargo, arguments);
    let workspace_root = reported
        .as_ref()
        .map(|roots| roots.workspace_root.clone())
        .unwrap_or_else(|| workspace_root(working_dir));
    // An explicit flag outranks anything cargo reports from configuration.
    let target_dir = target_dir_argument(arguments)
        .map(|value| absolute(working_dir, value))
        .or_else(|| reported.map(|roots| roots.target_dir))
        .or_else(|| {
            std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(|dir| absolute(working_dir, &dir.to_string_lossy()))
        })
        .unwrap_or_else(|| workspace_root.join("target"));
    Roots {
        workspace_root,
        target_dir,
    }
}

/// Cargo resolves a relative directory against the invocation directory.
fn absolute(working_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        working_dir.join(path)
    }
}

fn cargo_roots(cargo: &std::ffi::OsStr, arguments: &[String]) -> Option<Roots> {
    let mut command = Command::new(cargo);
    command.args(["metadata", "--no-deps", "--format-version", "1"]);
    // Describe the project the build will actually operate on.
    if let Some(manifest) = flag_value(arguments, "--manifest-path") {
        command.args(["--manifest-path", manifest]);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_cargo_roots(&output.stdout)
}

fn parse_cargo_roots(metadata: &[u8]) -> Option<Roots> {
    let metadata: serde_json::Value = serde_json::from_slice(metadata).ok()?;
    Some(Roots {
        workspace_root: PathBuf::from(metadata.get("workspace_root")?.as_str()?),
        target_dir: PathBuf::from(metadata.get("target_directory")?.as_str()?),
    })
}

fn target_dir_argument(arguments: &[String]) -> Option<&str> {
    flag_value(arguments, "--target-dir")
}

/// Read `--flag <value>` or `--flag=<value>` out of cargo's arguments.
fn flag_value<'a>(arguments: &'a [String], flag: &str) -> Option<&'a str> {
    let joined = format!("{flag}=");
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if let Some(value) = argument.strip_prefix(&joined) {
            return Some(value);
        }
        if argument == flag {
            return arguments.next().map(String::as_str);
        }
    }
    None
}

/// Reject a build invocation that cargo would not accept anyway.
pub fn validate_build_arguments(arguments: &[String]) -> Result<()> {
    if arguments.is_empty() {
        bail!("mbx build expects a cargo subcommand, for example: mbx build build --release");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_outermost_lockfile_as_the_workspace_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let member = root.join("crates").join("member");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(member.join("Cargo.toml"), "[package]\n").unwrap();

        assert_eq!(workspace_root(&member), root);
    }

    #[test]
    fn falls_back_to_a_manifest_without_a_lockfile() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let nested = root.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();

        assert_eq!(workspace_root(&nested), root);
    }

    #[test]
    fn falls_back_to_the_starting_directory() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(workspace_root(directory.path()), directory.path());
    }

    #[test]
    fn reads_both_flag_spellings() {
        let joined = ["build".to_string(), "--target-dir=/tmp/out".to_string()];
        let split = [
            "build".to_string(),
            "--target-dir".to_string(),
            "/tmp/out".to_string(),
        ];
        let dangling = ["build".to_string(), "--target-dir".to_string()];

        assert_eq!(target_dir_argument(&joined), Some("/tmp/out"));
        assert_eq!(target_dir_argument(&split), Some("/tmp/out"));
        assert_eq!(target_dir_argument(&dangling), None);
        assert_eq!(target_dir_argument(&["build".to_string()]), None);
    }

    #[test]
    fn reads_the_roots_cargo_reports() {
        let metadata = br#"{
            "workspace_root": "/elsewhere/project",
            "target_directory": "/var/cache/shared-target",
            "packages": []
        }"#;

        assert_eq!(
            parse_cargo_roots(metadata).unwrap(),
            Roots {
                workspace_root: PathBuf::from("/elsewhere/project"),
                target_dir: PathBuf::from("/var/cache/shared-target"),
            }
        );
    }

    #[test]
    fn ignores_unusable_cargo_metadata() {
        assert!(parse_cargo_roots(b"not json").is_none());
        assert!(parse_cargo_roots(br#"{"packages": []}"#).is_none());
    }

    #[test]
    fn resolves_a_relative_directory_against_the_working_directory() {
        let cwd = Path::new("/workspace/crates/member");
        assert_eq!(
            absolute(cwd, "out"),
            Path::new("/workspace/crates/member/out")
        );
        assert_eq!(absolute(cwd, "/tmp/out"), Path::new("/tmp/out"));
    }

    #[test]
    fn carries_an_inherited_wrapper_into_the_session() {
        let with = inherited_environment(|name| {
            (name == "RUSTC_WRAPPER").then(|| "/usr/bin/sccache".to_string())
        });
        assert_eq!(with.get("RUSTC_WRAPPER").unwrap(), "/usr/bin/sccache");

        // An empty value is how a shell unsets it in practice.
        let empty = inherited_environment(|name| (name == "RUSTC_WRAPPER").then(String::new));
        assert!(empty.is_empty());
        assert!(inherited_environment(|_| None).is_empty());
    }

    #[test]
    fn build_requires_a_cargo_subcommand() {
        assert!(validate_build_arguments(&[]).is_err());
        assert!(validate_build_arguments(&["build".to_string()]).is_ok());
    }
}
