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

mod adopt;
mod analyze;
mod cache;
mod cargo;
mod cargo_invocation;
mod clean;
mod doctor;
mod exec;
mod explain;
mod gc;
pub(crate) use gc::{schedule_low_disk_sweep, spawn_collector_from};
pub mod launch;
mod mascot;
mod plain_progress;
mod prefetch;
mod pretty;
mod settings;
mod setup;
mod shim;
mod stats;
pub mod test_runner;
mod tui;

const CARGO_SHIM_TARGET_FILE: &str = "mbx-target";
const RUST_ANALYZER_TARGET_DIR: &str = "target/rust-analyzer";
const CHECK_LANE_TARGET_DIR: &str = "target/check";

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
use {cache::*, cargo::*, exec::*, gc::*, settings::*, setup::*};

#[derive(usage::Cli)]
#[usage(completion = true)]
#[usage(
    bin = "mbx",
    version,
    config = crate::config::RawConfig,
    about = "A build cache for Rust projects",
    long_about = "Enable mise's `mr_boxington` Rust option, or run `mbx setup` once, then keep using Cargo normally. Compiled work is shared across every checkout, and build storage prunes itself. Use mbx directly for its own commands, such as `tui`, `stats`, `cache`, `gc`, and `doctor`, or prefix Cargo commands with `mbx` for zero-config use.\n\nExamples:\n  mbx setup\n  cargo build --release\n  cargo test --workspace\n  cargo clippy --all-targets -- -D warnings\n  mbx gc --dry-run",
    unknown_flags = "error"
)]
struct Cli {
    /// Toolchain to run under, named the way rustup names it: `mbx +1.91 check`.
    ///
    /// Cargo commands, `mbx doctor`, `mbx explain`, and `mbx prefetch` take a
    /// toolchain. Other mbx commands reject one, and so does `mbx explain --last`.
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
    /// Generate a self-contained shell completion script.
    Completion {
        /// Shell: bash, zsh, fish, powershell (or pwsh), nu (or nushell), or elvish.
        #[usage(arg)]
        shell: String,
    },
    /// Check the installation, cache, toolchain, and remote cache connection.
    Doctor(doctor::DoctorArgs),
    /// Explain cache bypasses, or replay the last build and diagnose its misses.
    ///
    /// With a Cargo command, mbx runs it and explains every compilation that
    /// bypassed the cache, grouped by cause, as in `mbx explain build --workspace`.
    /// With `--last`, mbx runs nothing: it reads this workspace's most recent
    /// recorded build and explains each miss against an earlier recording of
    /// that compilation unit, preferring this workspace's own.
    Explain(explain::ExplainArgs),
    /// Rank the uncached compiler time of this workspace's last build by cause.
    ///
    /// Each cause comes with what would remove it. mbx reads the newest recorded
    /// build of the Cargo workspace that contains the current directory, so it
    /// runs nothing. A crate that rebuilt only because a dependency changed is
    /// counted under the change that started it. The report ends with the
    /// critical path (the chain of units the build waited on) and the time only
    /// one unit was running.
    Analyze(analyze::AnalyzeArgs),
    /// Make plain Cargo commands run through mbx.
    ///
    /// mbx installs a stable Cargo shim and prints its path. When you pick a
    /// mise scope, with a flag or at the prompt, mbx also adds a
    /// `[wrappers.cargo]` entry to that scope's mise configuration, if it has
    /// none, and runs `mise reshim`; this needs mise 2026.8.16 or newer. Without
    /// a mise scope, put the shim's directory first on `PATH` yourself; mbx
    /// never edits shell startup files.
    ///
    /// mbx also writes `check.overrideCommand` to rust-analyzer's user
    /// `rust-analyzer.toml` so editor checks run through the shim, unless that
    /// file already has check settings of its own.
    Setup(setup::SetupArgs),
    /// Read and change settings in the global configuration file.
    Settings(settings::SettingsArgs),
    /// Collect build state and evict store objects to fit the size budgets.
    ///
    /// mbx collects learned incremental state and managed targets first, then
    /// evicts store objects until the action store fits its budget
    /// (`--max-size`, or `gc.max_size` by default, lowered when
    /// `gc.max_total_size` leaves less room). When a disk has less free space
    /// than `gc.min_free_size`, mbx also collects past those budgets to free
    /// the shortfall: private state and managed targets go first, then shared
    /// store objects, even below `gc.max_size`. Active and most recently used
    /// state stays, so a disk filled by something else can remain short.
    ///
    /// An evicted object is rebuilt when a build needs it again.
    Gc(gc::GcArgs),
    /// Inspect and manage the local cache.
    ///
    /// Three subcommands change files. `export` writes a cache export. `import`
    /// adds one to the store, consumes a directory export, and can restore
    /// Cargo workspace state into an absent or empty target directory. `remove`
    /// deletes a workspace's managed target, learned incremental state, and
    /// cache claims. The rest only read.
    Cache(cache::CacheArgs),
    /// Remove a workspace's managed target and learned incremental state.
    ///
    /// mbx also removes the checkout's `target` link when it points at that
    /// managed target. Shared store objects and the workspace's cache claims
    /// stay, so a later build can restore matching outputs; `mbx cache remove`
    /// also forgets the claims. A managed target or learned incremental state
    /// that a running command is using is kept, with a warning.
    Clean(clean::CleanArgs),
    /// Move existing target directories under mbx management, contents intact.
    ///
    /// mbx moves each directory under the managed target root and leaves a
    /// `target` link in its place. The adopted directory then follows the same
    /// collection policy as any other managed target.
    Adopt(adopt::AdoptArgs),
    /// Watch cache activity across every build that uses this cache directory.
    ///
    /// Builds that use a different cache directory, or that run with the
    /// `events` setting off, do not appear.
    Tui(tui::TuiArgs),
    /// Show lifetime savings, pruning totals, and estimated workspace sharing.
    Stats(stats::StatsArgs),
    /// Download predicted actions from the remote cache without running Cargo.
    ///
    /// The predictions are the actions that earlier builds on this platform
    /// recorded for the same `Cargo.lock`; when that lockfile has none, mbx
    /// borrows an earlier lockfile's. `mbx prefetch` needs `remote.url` and a
    /// `remote.mode` that reads (`read-only` or `read-write`).
    Prefetch(prefetch::PrefetchArgs),
    /// Run a build command outside Cargo with its C and C++ compilations cached.
    ///
    /// Options for exec, such as `--project-root`, go before the command.
    /// Everything after the command name belongs to the command, including a
    /// later `--`, as in `mbx exec cmake --build build -- -j8`.
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
        Commands::Completion { .. } => Some("completion"),
        Commands::Setup(_) => Some("setup"),
        Commands::Settings(_) => Some("settings"),
        Commands::Gc(_) => Some("gc"),
        Commands::Cache(_) => Some("cache"),
        Commands::Clean(_) => Some("clean"),
        Commands::Adopt(_) => Some("adopt"),
        Commands::Tui(_) => Some("tui"),
        Commands::Stats(_) => Some("stats"),
        Commands::Analyze(_) => Some("analyze"),
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
    if let Some(answer) = Cli::completion_request(&original[1..]) {
        print!("{answer}");
        return Ok(ExitCode::SUCCESS);
    }
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
    if let Commands::Completion { shell } = &cli.command {
        let shell = usage::complete::Shell::from_name(shell)
            .ok_or_else(|| eyre::eyre!("unsupported shell: {shell}"))?;
        print!("{}", Cli::completion_script(shell));
        return Ok(ExitCode::SUCCESS);
    }
    // Settings resolve the configuration themselves, so a broken file can
    // still be read and repaired.
    if let Commands::Settings(args) = cli.command {
        return settings::run(args);
    }
    let (config, settings) = Config::load_for_cli()?;
    match cli.command {
        Commands::Completion { .. } => {
            unreachable!("completion was handled before configuration loading")
        }
        Commands::Doctor(_) => unreachable!("doctor was handled before configuration loading"),
        Commands::Settings(_) => {
            unreachable!("settings were handled before configuration loading")
        }
        Commands::Explain(args) => {
            shim::prepare_explicit_cargo()?;
            explain::run(&config, &settings, args, toolchain)
        }
        Commands::Analyze(args) => analyze::run(&config, args),
        Commands::Setup(args) => setup::run(&args, args.action()?),
        Commands::Gc(args) if args.automatic => {
            if args.json || args.dry_run || args.max_size.is_some() {
                eyre::bail!("--automatic runs the configured sweep and takes no other option");
            }
            gc::run_automatic(&config, &settings.retention).map(|()| ExitCode::SUCCESS)
        }
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
        Commands::Clean(args) => clean::run(&config, &args),
        Commands::Adopt(args) => adopt::run(&config, &args),
        Commands::Tui(args) => tui::run(
            &config,
            args,
            settings.savings == crate::config::SavingsStyle::Quips,
        ),
        Commands::Stats(args) => stats::run(&config, args, settings.savings),
        Commands::Prefetch(args) => {
            shim::prepare_explicit_cargo()?;
            prefetch::run(&config, settings.events_max_size, &args.cargo_args)
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
mod adopt_tests;
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
mod settings_tests;
#[cfg(test)]
mod setup_tests;
