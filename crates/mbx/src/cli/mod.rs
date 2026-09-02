//! The mbx command line.

use crate::config::Config;
#[cfg(test)]
use crate::config::RetentionSettings;
#[cfg(test)]
use crate::util::workspace_root;
use eyre::Result;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::process::ExitCode;

mod cache;
mod cargo;
mod doctor;
mod exec;
mod explain;
mod gc;
mod prefetch;
mod setup;
mod shim;
mod tui;

const CARGO_SHIM_TARGET_FILE: &str = "mbx-target";
const RUST_ANALYZER_TARGET_DIR: &str = "target/rust-analyzer";

pub(crate) use cargo::{cargo_with_bypass_log, cargo_with_settings_and_bypass_log};
pub(crate) use setup::{cargo_shim_is_current, setup_install_dir};
pub use shim::{is_cargo_shim, run_cargo_shim};

pub(crate) fn cargo_activation_path() -> Option<std::ffi::OsString> {
    shim::activation_path()
}

pub(crate) use shim::is_mise_command_wrapper_path;

#[cfg(test)]
use prefetch::validate_prefetch_config;
#[cfg(all(test, unix))]
pub(crate) use setup::CARGO_SHIM_LAUNCHER as DOCTOR_CARGO_SHIM_LAUNCHER;
#[cfg(test)]
pub(crate) use setup::{
    MiseScope as DoctorMiseScope, SetupAction as DoctorSetupAction,
    setup_at_action as doctor_setup_at_action,
};
#[cfg(test)]
use {cache::*, cargo::*, exec::*, gc::*, setup::*};

#[derive(usage::Cli)]
#[usage(
    bin = "mbx",
    version,
    config = crate::config::RawConfig,
    about = "A build cache for Rust projects",
    long_about = "Run `mbx setup` once, then keep using Cargo normally. Compiled work is shared across every checkout and build storage prunes itself. Use mbx directly for its own commands, such as `tui`, `cache`, `gc`, and `doctor`, or prefix Cargo commands with `mbx` for zero-config use.\n\nExamples:\n  mbx setup\n  cargo build --release\n  cargo test --workspace\n  cargo clippy --all-targets -- -D warnings\n  mbx gc --dry-run",
    unknown_flags = "error"
)]
struct Cli {
    /// Toolchain to run under, named the way rustup names it: `mbx +1.91 check`.
    //
    // Classified by its `+` rather than left to the external subcommand, which
    // is what makes the word mean the same thing in front of an mbx command as
    // it does in front of a Cargo one. Forwarded as it was typed by everything
    // that ends in cargo, so `mbx +1.91 check` still reaches rustup's shim the
    // way it always has. A plain comment rather than a doc comment, because the
    // generated CLI reference renders those and this is a note to the next
    // reader here.
    #[usage(sigil = "+", value_name = "TOOLCHAIN")]
    toolchain: Option<String>,
    #[usage(subcommand)]
    command: Commands,
}

#[derive(usage::Subcommands)]
enum Commands {
    /// Check the local installation, cache, toolchain, and remote connection.
    Doctor(doctor::DoctorArgs),
    /// Run a Cargo command and explain every compilation mbx cannot cache.
    Explain(explain::ExplainArgs),
    /// Make plain Cargo commands run through mbx.
    Setup(setup::SetupArgs),
    /// Collect stale managed targets and evict cached objects until the store fits a size budget.
    ///
    /// A missing cached object is rebuilt when it is needed again.
    Gc(gc::GcArgs),
    /// Inspect the local store.
    Cache(cache::CacheArgs),
    /// Watch cache activity across every build on this machine.
    Tui(tui::TuiArgs),
    /// Download predicted remote artifacts without running Cargo.
    Prefetch(prefetch::PrefetchArgs),
    /// Run a build command outside cargo with its C and C++ compiles cached.
    Exec(exec::ExecArgs),
    #[usage(external_subcommand)]
    Cargo(Vec<String>),
}

/// Hand a named toolchain back to the Cargo command line it was typed on.
///
/// The word is mbx's to parse and rustup's to answer: `cargo` on `PATH` is the
/// rustup shim, and `+1.91` in front of the subcommand is how it is told which
/// toolchain to run. Every path that ends in cargo puts it back exactly where
/// it was typed, so a build that names a toolchain runs the compiler it named,
/// as it did when mbx forwarded the word without reading it.
fn with_toolchain(toolchain: Option<&str>, arguments: Vec<String>) -> Vec<String> {
    let Some(toolchain) = toolchain else {
        return arguments;
    };
    std::iter::once(format!("+{toolchain}"))
        .chain(arguments)
        .collect()
}

/// The name of a command that never reaches a compiler, if this is one.
///
/// A toolchain named in front of one of these is refused rather than ignored:
/// `mbx +1.91 gc` reads as a request mbx has no way to honour, and collecting
/// the store under whichever toolchain happened to be default is not a smaller
/// version of it. Matched exhaustively, so a command added later has to say
/// which kind it is instead of inheriting an answer.
fn compiles_nothing(command: &Commands) -> Option<&'static str> {
    match command {
        Commands::Setup(_) => Some("setup"),
        Commands::Gc(_) => Some("gc"),
        Commands::Cache(_) => Some("cache"),
        Commands::Tui(_) => Some("tui"),
        // Its whole subject is the C and C++ compiles of a build cargo is not
        // running, so a Rust toolchain has nothing to select here.
        Commands::Exec(_) => Some("exec"),
        Commands::Doctor(_) | Commands::Explain(_) | Commands::Prefetch(_) | Commands::Cargo(_) => {
            None
        }
    }
}

/// Parse the command line and run it.
pub fn run() -> Result<ExitCode> {
    let original = std::env::args_os().collect::<Vec<_>>();
    let mut cli = Cli::parse();
    let toolchain = cli.toolchain.take();
    let toolchain = toolchain.as_deref();
    if let Some(toolchain) = toolchain
        && let Some(command) = compiles_nothing(&cli.command)
    {
        eyre::bail!(
            "+{toolchain} names a toolchain to compile with, and `mbx {command}` compiles nothing"
        );
    }
    if let Commands::Prefetch(args) = &mut cli.command {
        args.cargo_args = with_toolchain(toolchain, original_prefetch_arguments(&original)?);
    }
    if let Commands::Exec(args) = &mut cli.command {
        args.command = original_exec_arguments(&original)?;
    }
    if let Commands::Doctor(args) = &cli.command {
        // Doctor describes the toolchain selected at this invocation site.
        // In particular, a mise Cargo wrapper is what resolves a project-local
        // toolchain; excluding it first would make the version check fall
        // through to an unrelated Cargo later on PATH.
        return doctor::run(args, toolchain);
    }
    let (config, settings) = Config::load_for_cli()?;
    match cli.command {
        Commands::Doctor(_) => unreachable!("doctor was handled before configuration loading"),
        Commands::Explain(args) => {
            shim::prepare_explicit_cargo()?;
            explain::run(&config, &settings, args, toolchain)
        }
        Commands::Setup(args) => setup::run(&args, args.action()?),
        Commands::Gc(args) => gc::run(
            &config,
            args.max_size
                .map_or(config.gc.max_bytes, |requested| requested.as_u64()),
            args.dry_run,
            args.json,
            &settings.retention,
        )
        .map(|()| ExitCode::SUCCESS),
        Commands::Cache(args) => cache::run(&config, args.command),
        Commands::Tui(args) => tui::run(&config, args),
        Commands::Prefetch(args) => {
            shim::prepare_explicit_cargo()?;
            prefetch::run(&config, &args.cargo_args)
        }
        Commands::Exec(args) => exec::run(&config, &settings, &args),
        Commands::Cargo(arguments) => {
            shim::prepare_explicit_cargo()?;
            cargo::run(&config, &settings, &with_toolchain(toolchain, arguments))
        }
    }
}

/// Recover `mbx exec`'s command exactly as it was typed.
///
/// `double_dash = "automatic"` already stops flag interpretation once the
/// command is named, which is what keeps exec from reading a `--project-root`
/// meant for the command. What it does not do is hand over the first `--` that
/// follows: the parser still consumes one, deliberately, so that a `--` can
/// unlock an argument declared as requiring one. exec declares no such
/// argument, and `cmake --build build -- -j8` needs that separator, so it is
/// taken back from the raw argv, past exec's own options.
fn original_exec_arguments(arguments: &[std::ffi::OsString]) -> Result<Vec<String>> {
    let Some(index) = arguments
        .iter()
        .position(|argument| argument == std::ffi::OsStr::new("exec"))
    else {
        eyre::bail!("could not recover exec arguments");
    };
    let mut rest = &arguments[index + 1..];
    while let Some(first) = rest.first().and_then(|argument| argument.to_str()) {
        if first == "--project-root" {
            rest = rest.get(2..).unwrap_or_default();
        } else if first.starts_with("--project-root=") {
            rest = &rest[1..];
        } else {
            break;
        }
    }
    // A separator before the command belongs to exec, and running it would
    // look for a program named `--`.
    if rest.first().is_some_and(|argument| argument == "--") {
        rest = &rest[1..];
    }
    strings(rest)
}

fn original_prefetch_arguments(arguments: &[std::ffi::OsString]) -> Result<Vec<String>> {
    let Some(index) = arguments
        .iter()
        .position(|argument| argument == std::ffi::OsStr::new("prefetch"))
    else {
        eyre::bail!("could not recover prefetch arguments");
    };
    strings(&arguments[index + 1..])
}

fn strings(arguments: &[std::ffi::OsString]) -> Result<Vec<String>> {
    arguments
        .iter()
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| eyre::eyre!("Cargo arguments must be valid UTF-8"))
        })
        .collect()
}

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod cargo_tests;
#[cfg(test)]
mod dispatch_tests;
#[cfg(test)]
mod exec_tests;
#[cfg(test)]
mod gc_tests;
#[cfg(test)]
mod prefetch_tests;
#[cfg(test)]
mod setup_tests;
