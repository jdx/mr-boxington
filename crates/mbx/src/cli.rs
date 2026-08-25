//! The mbx command line.

use crate::config::{CliSettings, Config, RetentionSettings};
use crate::session::CacheSession;
use crate::util::workspace_root;
use crate::{policy, store, target};
use bytesize::ByteSize;
use eyre::{Context, Result};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(usage::Cli)]
#[usage(
    bin = "mbx",
    version,
    config = crate::config::RawConfig,
    about = "A build cache for Rust projects",
    long_about = "Run any Cargo subcommand with the build cache enabled. The subcommand and all of its arguments are passed through unchanged, including Cargo aliases and installed subcommands. `cache`, `gc`, and `setup` are reserved for mbx's own commands.\n\nExamples:\n  mbx build --release\n  mbx test --workspace\n  mbx clippy --all-targets -- -D warnings\n  mbx setup",
    unknown_flags = "error"
)]
struct Cli {
    #[usage(subcommand)]
    command: Commands,
}

#[derive(usage::Subcommands)]
enum Commands {
    /// Check the local installation, cache, toolchain, and remote connection.
    Doctor(JsonArgs),
    /// Run a Cargo command and explain every compilation mbx cannot cache.
    Explain(ExplainArgs),
    /// Install a persistent rustc wrapper for plain Cargo commands.
    Setup(SetupArgs),
    /// Collect stale managed targets and evict cached objects until the store fits a size budget.
    ///
    /// A missing cached object is rebuilt when it is needed again.
    Gc(GcArgs),
    /// Inspect the local store.
    Cache(CacheArgs),
    /// Download predicted remote artifacts without running Cargo.
    Prefetch(PrefetchArgs),
    #[usage(external_subcommand)]
    Cargo(Vec<String>),
}

#[derive(usage::Args)]
#[usage(unknown_flags = "value", dont_delimit_trailing_values = true)]
struct PrefetchArgs {
    /// Cargo subcommand and arguments whose previous manifest should be warmed.
    #[usage(value_name = "CARGO_ARGS", required = true)]
    cargo_args: Vec<String>,
}

#[derive(usage::Args)]
struct SetupArgs {
    /// Report whether plain Cargo integration is installed and current.
    #[usage(long)]
    status: bool,
    /// Refresh an existing mbx wrapper without installing a missing one.
    #[usage(long)]
    update: bool,
    /// Remove mbx's Cargo configuration and installed wrapper.
    #[usage(long)]
    uninstall: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupAction {
    Install,
    Status,
    Update,
    Uninstall,
}

impl SetupArgs {
    fn action(&self) -> Result<SetupAction> {
        let selected = self.status as u8 + self.update as u8 + self.uninstall as u8;
        if selected > 1 {
            eyre::bail!("--status, --update, and --uninstall are mutually exclusive");
        }
        Ok(if self.status {
            SetupAction::Status
        } else if self.update {
            SetupAction::Update
        } else if self.uninstall {
            SetupAction::Uninstall
        } else {
            SetupAction::Install
        })
    }
}

#[derive(usage::Args)]
struct GcArgs {
    /// Size the store may occupy afterwards, for example 20GiB. Defaults to the
    /// configured budget.
    #[usage(long, value_name = "SIZE")]
    max_size: Option<ByteSize>,
    /// Print a stable machine-readable report.
    #[usage(long)]
    json: bool,
    /// Show what collection would remove without changing any files.
    #[usage(long)]
    dry_run: bool,
}

#[derive(usage::Args)]
struct JsonArgs {
    /// Print a stable machine-readable report.
    #[usage(long)]
    json: bool,
}

#[derive(usage::Args)]
#[usage(unknown_flags = "value")]
struct ExplainArgs {
    /// Cargo subcommand to run under diagnostics.
    #[usage(value_name = "CARGO_COMMAND")]
    cargo_command: String,
    /// Arguments to pass to the Cargo subcommand.
    #[usage(double_dash = "preserve", value_name = "CARGO_ARGS")]
    cargo_args: Vec<String>,
}

impl ExplainArgs {
    fn arguments(self) -> Vec<String> {
        std::iter::once(self.cargo_command)
            .chain(self.cargo_args)
            .collect()
    }
}

#[derive(usage::Args)]
struct CacheArgs {
    #[usage(subcommand)]
    command: CacheCommands,
}

#[derive(usage::Subcommands)]
enum CacheCommands {
    /// Print the store directory.
    Dir(JsonArgs),
    /// Summarize what the store holds.
    Stats(JsonArgs),
    /// Show cache use attributed to recorded workspaces.
    Projects,
    /// List the largest objects and action-result records.
    Largest(LargestArgs),
    /// Verify local objects and action results.
    Verify,
    /// Remove one workspace's managed target and cache claims.
    Remove(RemoveCacheArgs),
}

#[derive(usage::Args)]
struct LargestArgs {
    /// Maximum entries to print.
    #[usage(long, default = "20")]
    limit: usize,
}

#[derive(usage::Args)]
struct RemoveCacheArgs {
    /// Workspace root to forget.
    workspace: PathBuf,
}

/// Parse the command line and run it.
pub fn run() -> Result<ExitCode> {
    let original = std::env::args_os().collect::<Vec<_>>();
    let mut cli = Cli::parse();
    if let Commands::Prefetch(args) = &mut cli.command {
        args.cargo_args = original_prefetch_arguments(&original)?;
    }
    if let Commands::Doctor(args) = &cli.command {
        return crate::doctor::run_loaded(Config::load(), args.json);
    }
    let (config, settings) = Config::load_for_cli()?;
    match cli.command {
        Commands::Doctor(_) => unreachable!("doctor was handled before configuration loading"),
        Commands::Explain(args) => {
            crate::explain::run_with_settings(&config, &settings, &args.arguments())
        }
        Commands::Setup(args) => setup(args.action()?),
        Commands::Gc(args) => gc(
            &config,
            args.max_size
                .map_or(config.gc.max_bytes, |requested| requested.as_u64()),
            args.dry_run,
            args.json,
            &settings.retention,
        )
        .map(|()| ExitCode::SUCCESS),
        Commands::Cache(args) => match args.command {
            CacheCommands::Dir(args) => {
                if args.json {
                    print_json(&CacheDirReport {
                        version: 1,
                        store: config.store_dir().display().to_string(),
                    })?;
                } else {
                    println!("{}", config.store_dir().display());
                }
                Ok(ExitCode::SUCCESS)
            }
            CacheCommands::Stats(args) => {
                cache_stats(&config, args.json).map(|()| ExitCode::SUCCESS)
            }
            CacheCommands::Projects => cache_projects(&config).map(|()| ExitCode::SUCCESS),
            CacheCommands::Largest(args) => {
                cache_largest(&config, args.limit).map(|()| ExitCode::SUCCESS)
            }
            CacheCommands::Verify => cache_verify(&config),
            CacheCommands::Remove(args) => {
                cache_remove(&config, &args.workspace).map(|()| ExitCode::SUCCESS)
            }
        },
        Commands::Prefetch(args) => prefetch(&config, &args.cargo_args),
        Commands::Cargo(arguments) => cargo(&config, &settings, &arguments),
    }
}

fn original_prefetch_arguments(arguments: &[std::ffi::OsString]) -> Result<Vec<String>> {
    let Some(index) = arguments
        .iter()
        .position(|argument| argument == std::ffi::OsStr::new("prefetch"))
    else {
        eyre::bail!("could not recover prefetch arguments");
    };
    arguments[index + 1..]
        .iter()
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| eyre::eyre!("Cargo arguments must be valid UTF-8"))
        })
        .collect()
}

fn prefetch(config: &Config, arguments: &[String]) -> Result<ExitCode> {
    validate_prefetch_config(config, policy::release_context())?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let working_dir = std::env::current_dir()?;
    let roots = resolve_roots(&cargo, arguments, &working_dir);
    let session_dir = tempfile::Builder::new().prefix("mbx-prefetch-").tempdir()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let stats = runtime.block_on(async {
        let session = CacheSession::start(session_dir.path(), config).await?;
        session.prefetch(&roots.workspace_root, arguments).await?;
        session.finish().await
    })?;
    if stats.prefetch_runs == 0 {
        println!("no recorded actions for this workspace and Cargo command");
    } else {
        println!(
            "prefetched {} actions; {} downloaded and {} stored locally",
            stats.prefetched_actions,
            ByteSize::b(stats.downloaded_bytes).display().iec(),
            ByteSize::b(stats.stored_bytes).display().iec()
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn validate_prefetch_config(config: &Config, release_context: bool) -> Result<()> {
    if config.remote.url.is_none() {
        eyre::bail!("remote prefetch requires remote.url or MBX_REMOTE_URL");
    }
    if !config.remote.mode.reads() {
        eyre::bail!("remote prefetch requires a read-capable remote.mode");
    }
    if release_context {
        eyre::bail!("remote prefetch is disabled in release contexts");
    }
    Ok(())
}

fn setup(action: SetupAction) -> Result<ExitCode> {
    let executable = std::env::current_exe().wrap_err("failed to locate the mbx executable")?;
    let (install_dir, config_path) = setup_paths()?;
    setup_at_action(&executable, &install_dir, &config_path, action)
}

fn setup_paths() -> Result<(PathBuf, PathBuf)> {
    let data = dirs::data_local_dir()
        .ok_or_else(|| eyre::eyre!("the platform data directory could not be located"))?;
    let install_dir = data.join("mbx").join("bin");
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
        .ok_or_else(|| eyre::eyre!("Cargo's configuration directory could not be located"))?;
    let config_path =
        if cargo_home.join("config.toml").exists() || !cargo_home.join("config").exists() {
            cargo_home.join("config.toml")
        } else {
            cargo_home.join("config")
        };
    Ok((install_dir, config_path))
}

#[cfg(test)]
fn setup_at(executable: &Path, install_dir: &Path, config_path: &Path) -> Result<()> {
    setup_at_action(executable, install_dir, config_path, SetupAction::Install)?;
    Ok(())
}

fn setup_at_action(
    executable: &Path,
    install_dir: &Path,
    config_path: &Path,
    action: SetupAction,
) -> Result<ExitCode> {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut document = contents
        .parse::<toml_edit::DocumentMut>()
        .wrap_err_with(|| format!("failed to parse {}", config_path.display()))?;
    let shim_name = if cfg!(windows) {
        format!("{}.exe", crate::session::RUSTC_SHIM_STEM)
    } else {
        crate::session::RUSTC_SHIM_STEM.into()
    };
    let shim = install_dir.join(shim_name);
    let configured = document
        .get("build")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|build| build.get("rustc-wrapper"));
    let owns_configuration = configured
        .and_then(toml_edit::Item::as_str)
        .is_some_and(|configured| Path::new(configured) == shim);

    match action {
        SetupAction::Status => {
            if !owns_configuration {
                if let Some(configured) = configured {
                    println!(
                        "mbx setup is not active: build.rustc-wrapper is {}",
                        configured
                    );
                } else {
                    println!("mbx setup is not installed");
                }
                return Ok(ExitCode::FAILURE);
            }
            if !shim.is_file() {
                println!(
                    "mbx setup is configured but {} is missing; run `mbx setup --update`",
                    shim.display()
                );
                return Ok(ExitCode::FAILURE);
            }
            if same_file_contents(executable, &shim)? {
                println!("mbx setup is installed and current: {}", shim.display());
                return Ok(ExitCode::SUCCESS);
            }
            println!("mbx setup is installed but outdated; run `mbx setup --update`");
            return Ok(ExitCode::FAILURE);
        }
        SetupAction::Update if !owns_configuration => {
            if let Some(configured) = configured {
                eyre::bail!(
                    "mbx setup is not active because build.rustc-wrapper is {configured}; refusing to replace another wrapper"
                );
            }
            eyre::bail!("mbx setup is not installed; run `mbx setup` first");
        }
        SetupAction::Uninstall => {
            if owns_configuration {
                let build = document
                    .get_mut("build")
                    .and_then(toml_edit::Item::as_table_like_mut)
                    .expect("the configuration was inspected above");
                build.remove("rustc-wrapper");
                if build.is_empty() {
                    document.remove("build");
                }
                crate::util::write_atomic(config_path, document.to_string().as_bytes())?;
            }
            match std::fs::remove_file(&shim) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if owns_configuration {
                println!(
                    "removed mbx setup from {} and deleted {}",
                    config_path.display(),
                    shim.display()
                );
            } else {
                println!("mbx setup was not installed");
            }
            return Ok(ExitCode::SUCCESS);
        }
        SetupAction::Install | SetupAction::Update => {}
    }

    if let Some(configured) = configured {
        if owns_configuration {
            std::fs::create_dir_all(install_dir)?;
            crate::session::install_shim(executable, install_dir)?;
            let verb = if action == SetupAction::Update {
                "updated"
            } else {
                "refreshed"
            };
            println!("{verb} {}; Cargo was already configured", shim.display());
            return Ok(ExitCode::SUCCESS);
        }
        println!(
            "left {} unchanged: build.rustc-wrapper is already {}",
            config_path.display(),
            configured
        );
        return Ok(ExitCode::SUCCESS);
    }

    std::fs::create_dir_all(install_dir)?;
    let shim = crate::session::install_shim(executable, install_dir)?;
    document["build"]["rustc-wrapper"] = toml_edit::value(shim.to_string_lossy().into_owned());
    let parent = config_path
        .parent()
        .ok_or_else(|| eyre::eyre!("Cargo configuration path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    crate::util::write_atomic(config_path, document.to_string().as_bytes())?;
    println!(
        "installed {} and configured {}",
        shim.display(),
        config_path.display()
    );
    println!("plain cargo commands now use mbx's local action cache");
    Ok(ExitCode::SUCCESS)
}

fn same_file_contents(left: &Path, right: &Path) -> Result<bool> {
    let left_metadata = std::fs::metadata(left)?;
    let right_metadata = std::fs::metadata(right)?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }
    Ok(mbx_cache_core::CacheDigest::blake3_file(left)?
        == mbx_cache_core::CacheDigest::blake3_file(right)?)
}

fn cargo(config: &Config, settings: &CliSettings, arguments: &[String]) -> Result<ExitCode> {
    cargo_with_settings_and_bypass_log(config, settings, arguments, None)
}

pub(crate) fn cargo_with_bypass_log(
    config: &Config,
    arguments: &[String],
    bypass_log: Option<&Path>,
) -> Result<ExitCode> {
    cargo_with_settings_and_bypass_log(config, &CliSettings::default(), arguments, bypass_log)
}

pub(crate) fn cargo_with_settings_and_bypass_log(
    config: &Config,
    settings: &CliSettings,
    arguments: &[String],
    bypass_log: Option<&Path>,
) -> Result<ExitCode> {
    let retention = &settings.retention;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    // Release builds publish artifacts that must not depend on a cache, and a
    // tag build has no later build to share with anyway.
    if policy::release_context() {
        log::warn!("the build cache is disabled for release builds");
        return run_cargo(&cargo, arguments, BTreeMap::new());
    }

    let working_dir = std::env::current_dir()?;
    let mut roots = resolve_roots(&cargo, arguments, &working_dir);
    let mut config = config.clone();
    config.apply_workspace_policy(&roots.workspace_root)?;
    let incremental = policy::incremental_allowed(config.incremental);
    if config.incremental && !incremental {
        log::warn!(
            "incremental compilation is disabled here; it needs an earlier build to build on"
        );
    }
    // An enabled build stops overriding CARGO_INCREMENTAL rather than setting
    // it, so a 0 already in the environment still wins -- which CI images and
    // rust-cache set as a matter of course. Say so, because the alternative is
    // a setting that silently does nothing.
    if incremental && std::env::var("CARGO_INCREMENTAL").as_deref() == Ok("0") {
        log::warn!(
            "CARGO_INCREMENTAL=0 is set in the environment, so this build is not incremental after all"
        );
    }
    config.incremental = incremental;
    let config = &config;

    // Before the prompt below, so the offer to replace an existing `target/`
    // arrives after the explanation of what a managed one is for. One existence
    // check, and only on the cargo path.
    if !cargo_help_requested(arguments) && !was_explained(&config.store_dir()) {
        // Probed rather than assumed, and probed only on the one run that
        // will say something about it.
        let reflinks = crate::util::reflinks_work(&config.cache_dir);
        crate::session::note(&first_run_notice(config, retention, reflinks));
        mark_explained(&config.store_dir());
    }

    let migrate_existing = prompt_to_manage_existing_target(config, &roots, arguments)?;
    // Placed before the session starts, because the target directory is what
    // the shim maps out of its cache keys and it has to be the one cargo will
    // actually write to.
    let (placed, removed_target_bytes) = if migrate_existing {
        let outcome = target::migrate_existing(
            config,
            &roots.workspace_root,
            &roots.target_dir,
            roots.target_dir_requested,
        )?;
        (outcome.managed, outcome.removed_bytes)
    } else {
        (
            target::place(
                config,
                &roots.workspace_root,
                &roots.target_dir,
                roots.target_dir_requested,
            ),
            None,
        )
    };
    if let Some(bytes) = removed_target_bytes {
        crate::session::note(&format!(
            "freed {} by removing the existing target/ directory",
            ByteSize::b(bytes).display().iec()
        ));
    }
    if let Some(directory) = &placed {
        roots.target_dir = directory.clone();
    } else {
        // Placement declined, but an earlier one may have left a link this
        // build is about to write through. Keep that directory's record fresh
        // so collection does not treat it as idle.
        target::touch_managed(config, &roots.workspace_root, &roots.target_dir);
    }
    let session_dir = tempfile::Builder::new().prefix("mbx-session-").tempdir()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let session_outcome = runtime.block_on(async {
        let session = CacheSession::start(session_dir.path(), config).await?;
        let mut environment = inherited_environment(|name| std::env::var(name).ok(), &working_dir);
        if let Some(path) = bypass_log {
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                working_dir.join(path)
            };
            environment.insert(
                crate::session::BYPASS_LOG_ENV.into(),
                path.display().to_string(),
            );
        }
        if let Some(directory) = &placed {
            environment.insert(
                CARGO_TARGET_DIR_ENV.into(),
                directory.to_string_lossy().into_owned(),
            );
        }
        let run = session
            .begin(
                &roots.workspace_root,
                &roots.target_dir,
                arguments,
                &mut environment,
            )
            .await;

        let status = run_cargo(&cargo, arguments, environment);

        // The shim records a prediction only after a compilation has either
        // been restored or published successfully. Preserve that completed
        // portion even when a later compilation makes cargo fail: it is still
        // useful to the retry, and the collector must know it is reachable.
        if let Some(run) = run
            && let Err(error) = run.commit().await
        {
            log::warn!("the build manifest was not committed: {error}");
        }

        let stats = match session.finish().await {
            Ok(stats) => {
                crate::session::display_stats(&stats, config);
                Some(stats)
            }
            Err(error) => {
                log::warn!("the cache session did not shut down cleanly: {error}");
                None
            }
        };
        Ok((status, stats))
    });
    // A session that never started still leaves collection to do, so the error
    // travels as the build's result rather than short-circuiting past the sweep.
    let (status, stats) = match session_outcome {
        Ok((status, stats)) => (status, stats),
        Err(error) => (Err(error), None),
    };
    // Outside the runtime, so a walk of the whole store cannot occupy a worker
    // thread, and after cargo has exited, so it adds nothing to the build the
    // user is waiting on.
    let mut delta = sweep_store(config, retention);
    // Removing the checkout's own `target/` on the way in freed disk too, and
    // it is the largest single reclaim a first build ever reports -- but the
    // user confirmed it, so it must not feed the counters the collection
    // brags read: that directory had not outlived anything.
    delta.freed_requested_bytes = removed_target_bytes.unwrap_or_default();
    let facts = stats
        .as_ref()
        .map_or_else(crate::savings::SessionFacts::default, |stats| {
            crate::savings::SessionFacts {
                hits: stats.hits,
                avoided_compiler_ns: stats.avoided_compiler_duration_ns,
            }
        });
    if let Some(stats) = &stats {
        // A session that was never consulted did not build anything worth
        // counting as a build; storing its zeroes would dilute every average.
        if crate::session::session_was_active(stats) {
            delta.builds = 1;
        }
        delta.cached_compilations = stats.hits;
        delta.avoided_compiler_ns = stats.avoided_compiler_duration_ns;
        delta.reflinked_bytes = stats.reflinked_output_bytes;
    }
    // Unconditional, including on a run that built nothing: a sweep that came
    // due during `cargo build --help` really did reclaim those bytes, and the
    // lifetime total is wrong if they go unrecorded.
    if let Some(line) =
        crate::savings::record_and_describe(&config.store_dir(), &delta, &facts, settings.savings)
    {
        crate::session::note(&line);
    }
    status
}

/// Marks that this machine has been told what mbx set up.
///
/// A stamp of its own rather than a side effect of some other file: whether to
/// explain mbx and whether to count a run's savings are unrelated questions,
/// and answering both from one file forces every change to one to reason about
/// the other.
const NOTICE_STAMP: &str = "notice/v1/explained";

fn was_explained(store: &Path) -> bool {
    store.join(NOTICE_STAMP).exists()
}

/// Written immediately after the notice prints, so a build that fails later
/// does not explain itself again.
fn mark_explained(store: &Path) {
    if let Err(error) = crate::util::write_atomic(&store.join(NOTICE_STAMP), b"") {
        // Worth one line at debug: the cost is a repeated notice, and a store
        // this cannot write to has larger problems than that.
        log::debug!("the first-run notice was not recorded: {error}");
    }
}

/// What mbx has arranged on this machine, said once.
///
/// A cache that manages its own disk should say so before it starts deleting
/// things, and the numbers it prints are the resolved ones rather than the
/// documented ones: budgets scale with the disk, so a fixed sentence here would
/// be wrong on most machines. A limit somebody turned off is left unmentioned
/// rather than described.
fn first_run_notice(config: &Config, retention: &RetentionSettings, reflinks: bool) -> String {
    let mut lines =
        vec!["mbx: first build on this machine -- here is the arrangement:".to_string()];
    // Collection is what the rest of this describes, so a machine that turned
    // it off is told what it has instead of what it does not.
    if config.gc.auto {
        lines.push(format!(
            "mbx:   compiled work is cached once in {} and shared with every checkout and worktree; the store sweeps itself back to {}",
            config.cache_dir.display(),
            ByteSize::b(config.gc.max_bytes).display().iec(),
        ));
    } else {
        lines.push(format!(
            "mbx:   compiled work is cached once in {} and shared with every checkout and worktree; automatic collection is off, so `mbx gc` is the only thing that reclaims it",
            config.cache_dir.display(),
        ));
    }
    // Only when a probe just proved it: this is a promise about what the
    // user's disk will do, and a machine on ext4 must not be promised
    // sharing that every restore will quietly turn into a copy.
    if reflinks {
        lines.push(
            "mbx:   this filesystem can reflink, so outputs land in target/ without copying -- many checkouts, one copy on disk"
                .to_string(),
        );
    }
    if config.target.views && config.gc.auto {
        let mut reasons = vec!["their checkout disappears".to_string()];
        if let Some(age) = retention.target_max_age {
            reasons.push(format!(
                "they sit unused for {}",
                crate::util::format_span(age)
            ));
        }
        if let Some(bytes) = retention.target_max_bytes {
            reasons.push(format!(
                "they together outgrow {}",
                ByteSize::b(bytes).display().iec()
            ));
        }
        lines.push(format!(
            "mbx:   target/ directories are managed and collected when {}",
            join_clauses(&reasons),
        ));
    }
    lines.push(
        "mbx:   nothing else to run; `mbx gc --dry-run` previews a cleanup and every cap is configurable".to_string(),
    );

    lines.join("\n")
}

/// Join reasons as prose: "a", "a or b", "a, b, or c".
fn join_clauses(clauses: &[String]) -> String {
    match clauses {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} or {second}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

/// Offer to replace an existing default target directory with a managed one.
///
/// A non-interactive run must never wait for input. Refusing or cancelling the
/// prompt leaves cargo's directory alone and the build continues normally.
fn prompt_to_manage_existing_target(
    config: &Config,
    roots: &Roots,
    arguments: &[String],
) -> Result<bool> {
    if cargo_help_requested(arguments)
        || !std::io::stdin().is_terminal()
        || !std::io::stderr().is_terminal()
    {
        return Ok(false);
    }
    prompt_to_manage_existing_target_with(config, roots, |directory| {
        let description = format!(
            "mbx can remove {} and replace it with a managed target that is pruned after this checkout is deleted.",
            directory.display()
        );
        match demand::Confirm::new("Use a managed target directory?")
            .description(&description)
            .affirmative("Remove target/")
            .negative("Keep it")
            .selected(false)
            .run()
        {
            Ok(answer) => Ok(answer),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => Ok(false),
            Err(error) => Err(error.into()),
        }
    })
}

fn cargo_help_requested(arguments: &[String]) -> bool {
    arguments.first().is_some_and(|argument| argument == "help")
        || arguments
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
}

fn prompt_to_manage_existing_target_with(
    config: &Config,
    roots: &Roots,
    prompt: impl FnOnce(&Path) -> Result<bool>,
) -> Result<bool> {
    if !target::can_remove_existing(
        config,
        &roots.workspace_root,
        &roots.target_dir,
        roots.target_dir_requested,
    ) {
        return Ok(false);
    }
    prompt(&roots.target_dir)
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

fn gc(
    config: &Config,
    max_bytes: u64,
    dry_run: bool,
    json: bool,
    retention: &RetentionSettings,
) -> Result<()> {
    let store = config.store_dir();
    // The collector below remains the authority for store errors. Estimating
    // a combined budget must not prevent independent target collection when
    // the action store is damaged.
    let target_budget = target_budget(retention, max_bytes);
    let pruned = target::collect(
        &config.target.root,
        target_budget,
        retention.target_max_age,
        dry_run,
    );
    let projected_target_bytes = match &pruned {
        Ok(outcome) => outcome.remaining_bytes,
        Err(_) if retention.max_total_bytes.is_some() => target::stats(&config.target.root)
            .map(|stats| stats.bytes)
            .unwrap_or_default(),
        Err(_) => 0,
    };
    let store_budget = retention.max_total_bytes.map_or(max_bytes, |total| {
        max_bytes.min(total.saturating_sub(projected_target_bytes))
    });
    let outcome = if dry_run {
        store::gc_dry_run(&store, store_budget)
    } else {
        store::gc(&store, store_budget)
    };
    // Independent collections: a broken action store must not prevent the
    // command from freeing the usually much larger target directories.
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            match pruned {
                Ok(pruned) => {
                    // Credit what the targets gave back even though the store
                    // sweep failed: those bytes are gone from the disk either way.
                    record_collection(&store, 0, pruned.removed_bytes, dry_run);
                    if !json && pruned.removed_views > 0 {
                        println!("{}", target_removals(&pruned, dry_run));
                    }
                }
                Err(prune_error) => {
                    log::warn!("target directories were not collected: {prune_error}");
                }
            }
            return Err(error);
        }
    };
    record_collection(
        &store,
        outcome.removed_bytes,
        pruned.as_ref().map_or(0, |pruned| pruned.removed_bytes),
        dry_run,
    );
    if json {
        let pruned = pruned?;
        print_json(&GcReport {
            version: 1,
            max_bytes,
            dry_run,
            action_store: GcActionStoreReport {
                removed_objects: outcome.removed_objects,
                removed_action_results: outcome.removed_action_results,
                removed_checkout_records: outcome.removed_checkout_records,
                removed_bytes: outcome.removed_bytes,
                remaining_bytes: outcome.remaining_bytes,
            },
            targets: GcTargetReport {
                removed_directories: pruned.removed_views,
                removed_bytes: pruned.removed_bytes,
            },
        })?;
    } else {
        print_gc_store_outcome(&outcome, dry_run);
        let pruned = pruned?;
        if pruned.removed_views > 0 {
            println!("{}", target_removals(&pruned, dry_run));
        }
    }
    Ok(())
}

/// Add what a collection reclaimed to this machine's lifetime totals.
///
/// A dry run reclaimed nothing, so it contributes nothing.
fn record_collection(store: &Path, store_bytes: u64, target_bytes: u64, dry_run: bool) {
    if dry_run || (store_bytes == 0 && target_bytes == 0) {
        return;
    }
    crate::savings::record_quietly(
        store,
        &crate::savings::Delta {
            freed_store_bytes: store_bytes,
            freed_target_bytes: target_bytes,
            ..crate::savings::Delta::default()
        },
    );
}

fn print_gc_store_outcome(outcome: &store::GcOutcome, dry_run: bool) {
    let prefix = if dry_run { "would have " } else { "" };
    println!("{prefix}{}", evictions(outcome));
    if outcome.removed_checkout_records > 0 {
        println!(
            "{prefix}dropped {} stale checkout records",
            outcome.removed_checkout_records
        );
    }
}

/// One line describing the target directories a sweep freed.
fn target_removals(outcome: &target::CollectionOutcome, dry_run: bool) -> String {
    let verb = if dry_run { "would free" } else { "freed" };
    format!(
        "{verb} {} target directories ({}, {} abandoned and {} live); {} remain",
        outcome.removed_views,
        ByteSize::b(outcome.removed_bytes).display().iec(),
        outcome.removed_stale_views,
        outcome.removed_live_views,
        ByteSize::b(outcome.remaining_bytes).display().iec(),
    )
}

/// One line describing what a sweep evicted.
///
/// Shared so the explicit command and the automatic sweep cannot drift into
/// describing the same outcome two different ways.
fn evictions(outcome: &store::GcOutcome) -> String {
    format!(
        "evicted {} objects and {} action results ({}); {} remain",
        outcome.removed_objects,
        outcome.removed_action_results,
        ByteSize::b(outcome.removed_bytes).display().iec(),
        ByteSize::b(outcome.remaining_bytes).display().iec(),
    )
}

/// Keep the store inside its budget, at most once per configured interval.
///
/// Reported like the cache summary beside it: a sweep that evicted nothing says
/// nothing. A sweep that fails is logged and forgotten -- the build is already
/// over, and its exit status is the build's answer, not the collector's. What it
/// freed is returned so the lifetime totals can count it.
fn sweep_store(config: &Config, retention: &RetentionSettings) -> crate::savings::Delta {
    let mut delta = crate::savings::Delta::default();
    if !config.gc.auto {
        return delta;
    }
    match store::claim_sweep(&config.store_dir(), config.gc.interval) {
        Ok(false) => {}
        Ok(true) => {
            let pruned = prune_targets(config, retention, config.gc.max_bytes);
            delta.freed_target_bytes = pruned.freed_bytes;
            let store_budget = retention
                .max_total_bytes
                .map_or(config.gc.max_bytes, |total| {
                    let target_bytes = pruned.remaining_bytes.unwrap_or_else(|| {
                        target::stats(&config.target.root)
                            .map(|stats| stats.bytes)
                            .unwrap_or_default()
                    });
                    config.gc.max_bytes.min(total.saturating_sub(target_bytes))
                });
            let outcome = match store::gc(&config.store_dir(), store_budget) {
                Ok(outcome) => outcome,
                Err(error) => {
                    log::warn!("the store was not swept: {error}");
                    return delta;
                }
            };
            delta.freed_store_bytes = outcome.removed_bytes;
            if outcome.removed_bytes > 0 {
                crate::session::note(&format!("gc: {}", evictions(&outcome)));
            }
        }
        Err(error) => {
            log::warn!("the store was not swept: {error}");
            delta.freed_target_bytes =
                prune_targets(config, retention, config.gc.max_bytes).freed_bytes;
        }
    }
    delta
}

/// What one target collection left behind, and what it reclaimed.
struct PruneReport {
    /// `None` when collection failed, so a caller sizing a combined budget
    /// knows to measure rather than assume.
    remaining_bytes: Option<u64>,
    freed_bytes: u64,
}

/// Collect target views as the other half of a due automatic sweep.
fn prune_targets(
    config: &Config,
    retention: &RetentionSettings,
    store_reserve: u64,
) -> PruneReport {
    // A target directory whose checkout is gone is the largest thing
    // collection ever frees, and walking for it on every build would be the
    // slowest, so callers keep this inside the store sweep's throttle.
    let target_budget = target_budget(retention, store_reserve);
    match target::collect(
        &config.target.root,
        target_budget,
        retention.target_max_age,
        false,
    ) {
        Ok(pruned) => {
            if pruned.removed_views > 0 {
                crate::session::note(&format!("gc: {}", target_removals(&pruned, false)));
            }
            PruneReport {
                remaining_bytes: Some(pruned.remaining_bytes),
                freed_bytes: pruned.removed_bytes,
            }
        }
        Err(error) => {
            log::warn!("target directories were not collected: {error}");
            PruneReport {
                remaining_bytes: None,
                freed_bytes: 0,
            }
        }
    }
}

fn target_budget(retention: &RetentionSettings, store_reserve: u64) -> Option<u64> {
    retention
        .max_total_bytes
        .map_or(retention.target_max_bytes, |total| {
            let combined = total.saturating_sub(store_reserve);
            Some(
                retention
                    .target_max_bytes
                    .map_or(combined, |target| target.min(combined)),
            )
        })
}

fn cache_stats(config: &Config, json: bool) -> Result<()> {
    let store = config.store_dir();
    let stats = store::stats(&store)?;
    if json {
        let views = target::stats(&config.target.root)?;
        return print_json(&CacheStatsReport {
            version: 1,
            store: store.display().to_string(),
            objects: stats.objects,
            object_bytes: stats.object_bytes,
            action_results: stats.action_results,
            action_result_bytes: stats.action_result_bytes,
            total_bytes: stats.total_bytes(),
            live_checkouts: stats.live_checkouts,
            stale_checkouts: stats.stale_checkouts,
            target_directories: views.views,
            target_bytes: views.bytes,
        });
    }
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
    println!(
        "checkouts: {} live, {} stale",
        stats.live_checkouts, stats.stale_checkouts
    );
    let views = target::stats(&config.target.root)?;
    println!(
        "target directories: {} ({})",
        views.views,
        ByteSize::b(views.bytes).display().iec()
    );
    Ok(())
}

#[derive(serde::Serialize)]
struct CacheDirReport {
    version: u8,
    store: String,
}

#[derive(serde::Serialize)]
struct CacheStatsReport {
    version: u8,
    store: String,
    objects: u64,
    object_bytes: u64,
    action_results: u64,
    action_result_bytes: u64,
    total_bytes: u64,
    live_checkouts: u64,
    stale_checkouts: u64,
    target_directories: u64,
    target_bytes: u64,
}

#[derive(serde::Serialize)]
struct GcReport {
    version: u8,
    max_bytes: u64,
    dry_run: bool,
    action_store: GcActionStoreReport,
    targets: GcTargetReport,
}

#[derive(serde::Serialize)]
struct GcActionStoreReport {
    removed_objects: u64,
    removed_action_results: u64,
    removed_checkout_records: u64,
    removed_bytes: u64,
    remaining_bytes: u64,
}

#[derive(serde::Serialize)]
struct GcTargetReport {
    removed_directories: u64,
    removed_bytes: u64,
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn cache_projects(config: &Config) -> Result<()> {
    let projects = store::projects(&config.store_dir())?;
    if projects.is_empty() {
        println!("no recorded workspaces");
        return Ok(());
    }
    for project in projects {
        let state = if project.live { "live" } else { "stale" };
        println!(
            "{}\t{} actions\t{} targets\t{} identities\t{state}",
            project.workspace_root.display(),
            ByteSize::b(project.action_bytes).display().iec(),
            ByteSize::b(project.target_bytes).display().iec(),
            project.identities,
        );
    }
    Ok(())
}

fn cache_largest(config: &Config, limit: usize) -> Result<()> {
    for entry in store::largest(&config.store_dir(), limit)? {
        let path = entry
            .path
            .strip_prefix(config.store_dir())
            .unwrap_or(&entry.path);
        println!(
            "{}\t{}\t{}",
            ByteSize::b(entry.bytes).display().iec(),
            entry.kind,
            path.display()
        );
    }
    Ok(())
}

fn cache_verify(config: &Config) -> Result<ExitCode> {
    let outcome = store::verify(&config.store_dir())?;
    println!(
        "verified {} objects and {} action results",
        outcome.checked_objects, outcome.checked_action_results
    );
    for path in &outcome.problems {
        println!("invalid: {}", path.display());
    }
    Ok(if outcome.problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn cache_remove(config: &Config, workspace: &Path) -> Result<()> {
    let working_dir = std::env::current_dir()?;
    let requested = absolute(&working_dir, &workspace.to_string_lossy());
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let workspace = cache_workspace_root(&cargo, &requested);
    let target_bytes = target::remove_workspace(&config.target.root, &workspace)?;
    let removed = store::remove_project(&config.store_dir(), &workspace)?;
    println!(
        "removed {} checkout records for {}",
        removed.removed_checkout_records,
        workspace.display()
    );
    if let Some(bytes) = target_bytes {
        crate::savings::record_quietly(
            &config.store_dir(),
            &crate::savings::Delta {
                freed_requested_bytes: bytes,
                ..crate::savings::Delta::default()
            },
        );
        println!(
            "freed managed target directory ({})",
            ByteSize::b(bytes).display().iec()
        );
    }
    println!("shared cache objects remain available to other workspaces and normal GC");
    Ok(())
}

/// Resolve the workspace exactly as Cargo does when recording cache ownership.
///
/// Filesystem canonicalization is not interchangeable with Cargo's reported
/// root: on Windows it can introduce a `\\?\` prefix, and symlink spellings can
/// differ too. Both checkout records and managed-target keys use the metadata
/// spelling, so removal must obtain that same identity or it can silently miss
/// the workspace it was asked to forget.
fn cache_workspace_root(cargo: &std::ffi::OsStr, requested: &Path) -> PathBuf {
    let arguments = vec![
        "--manifest-path".to_string(),
        requested.join("Cargo.toml").to_string_lossy().into_owned(),
    ];
    cargo_roots(cargo, &arguments, None)
        .map(|roots| roots.workspace_root)
        .unwrap_or_else(|| requested.to_path_buf())
}

/// Settings the shim maps out of its cache keys.
#[derive(Debug, PartialEq, Eq)]
struct Roots {
    workspace_root: PathBuf,
    target_dir: PathBuf,
    /// Whether a flag, environment variable, or Cargo configuration named the
    /// target directory outright.
    ///
    /// Cargo prefers `--target-dir` over `CARGO_TARGET_DIR`, so a build carrying
    /// that flag cannot be moved by setting the environment: cargo would write
    /// where the flag says while the shim had been told the managed path, and
    /// the whole build would stop keying against anything it could reuse. The
    /// value can equal the default location and still have been asked for, so
    /// comparing paths cannot answer this.
    target_dir_requested: bool,
}

/// Carry a `RUSTC_WRAPPER` the caller already configured into the session.
///
/// The session records it so the shim can defer to it; without this it would be
/// dropped silently and whatever it does would stop happening.
fn inherited_environment(
    get_env: impl Fn(&str) -> Option<String>,
    working_dir: &Path,
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    if let Some(wrapper) = get_env("RUSTC_WRAPPER").filter(|value| !value.is_empty()) {
        environment.insert("RUSTC_WRAPPER".into(), wrapper);
    }
    // Cargo gives every shim its own working directory, so a relative
    // destination would scatter records across crate directories -- or fail
    // outright in a read-only registry checkout. Resolve it here, against the
    // directory the user typed it in, like every other path the shim receives.
    if let Some(log) = get_env(crate::session::BYPASS_LOG_ENV).filter(|value| !value.is_empty()) {
        environment.insert(
            crate::session::BYPASS_LOG_ENV.into(),
            absolute(working_dir, &log).display().to_string(),
        );
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
    resolve_roots_with(
        cargo,
        arguments,
        working_dir,
        std::env::var_os(CARGO_TARGET_DIR_ENV),
    )
}

/// The environment name cargo reads for the target directory. It outranks a
/// config-set `build.target-dir`, so both the probe and the fallback below have
/// to agree on one value for it.
const CARGO_TARGET_DIR_ENV: &str = "CARGO_TARGET_DIR";

/// [`resolve_roots`] with the ambient `CARGO_TARGET_DIR` passed in rather than
/// read here, so a caller can resolve as if the variable were unset.
fn resolve_roots_with(
    cargo: &std::ffi::OsStr,
    arguments: &[String],
    working_dir: &Path,
    target_dir_env: Option<std::ffi::OsString>,
) -> Roots {
    // Everything below inspects cargo's own flags only; the real build still
    // receives the full argument list untouched.
    let arguments = cargo_arguments(arguments);
    let reported = cargo_roots(cargo, arguments, target_dir_env.as_deref());
    // `-C` moves the directory cargo resolves relative paths against, so the
    // fallbacks below have to follow it rather than this process's cwd.
    let invocation_dir = invocation_dir(arguments, working_dir);
    let workspace_root = reported
        .as_ref()
        .map(|roots| roots.workspace_root.clone())
        .unwrap_or_else(|| workspace_root(&invocation_dir));
    let flagged = target_dir_argument(arguments);
    let from_environment = target_dir_env.as_ref().is_some_and(|dir| !dir.is_empty());
    let target_dir_requested = flagged.is_some()
        || from_environment
        || cargo_config_may_set_target_dir(arguments, &invocation_dir);
    // An explicit flag outranks anything cargo reports from configuration.
    let target_dir = flagged
        .map(|value| absolute(&invocation_dir, value))
        .or_else(|| reported.map(|roots| roots.target_dir))
        .or_else(|| {
            target_dir_env
                .map(PathBuf::from)
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(|dir| absolute(&invocation_dir, &dir.to_string_lossy()))
        })
        .unwrap_or_else(|| workspace_root.join("target"));
    Roots {
        workspace_root,
        target_dir,
        target_dir_requested,
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

/// Cargo options that belong before the subcommand and change what `cargo
/// metadata` would report. `-C` moves the whole invocation, and `--config` can
/// set `build.target-dir` outright, so a probe that drops them describes a
/// different tree than the one being built.
const PROBE_GLOBAL_FLAGS: [&str; 3] = ["-C", "--config", "-Z"];
/// Options cargo accepts only after the subcommand. The network flags matter
/// because a probe left to itself may reach the registry the build was told to
/// stay away from.
const PROBE_MANIFEST_TOGGLES: [&str; 3] = ["--offline", "--frozen", "--locked"];

/// Cargo's own arguments, stopping at the `--` that hands the rest to rustc.
///
/// The separator matters more than it looks: past it, `-C` is a codegen option,
/// not a directory for cargo to run in, and `-C opt-level=3` is ordinary. Reading
/// one as the other would point the probe and the path mapping at a directory
/// that does not exist, so every action would bypass.
fn cargo_arguments(arguments: &[String]) -> &[String] {
    let end = arguments
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(arguments.len());
    &arguments[..end]
}

/// Collect the occurrences of `flags` from `arguments`, preserving order and
/// repeats. Cargo allows `--flag value`, `--flag=value`, and, for the short
/// forms, `-Zvalue`.
fn forwarded_flags(arguments: &[String], flags: &[&str]) -> Vec<String> {
    let mut forwarded = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if let Some((flag, value)) = argument
            .split_once('=')
            .filter(|(flag, _)| flags.contains(flag))
        {
            forwarded.push(flag.to_string());
            forwarded.push(value.to_string());
        } else if flags.contains(&argument.as_str()) {
            if let Some(value) = remaining.next() {
                forwarded.push(argument.clone());
                forwarded.push(value.clone());
            }
        } else if flags
            .iter()
            .any(|flag| flag.len() == 2 && argument.len() > 2 && argument.starts_with(flag))
        {
            forwarded.push(argument.clone());
        }
    }
    forwarded
}

/// The directory cargo resolves relative paths against, which `-C` can move.
fn invocation_dir(arguments: &[String], working_dir: &Path) -> PathBuf {
    flag_value(arguments, "-C")
        .map(|value| absolute(working_dir, value))
        .unwrap_or_else(|| working_dir.to_path_buf())
}

fn cargo_roots(
    cargo: &std::ffi::OsStr,
    arguments: &[String],
    target_dir_env: Option<&std::ffi::OsStr>,
) -> Option<Roots> {
    let mut command = Command::new(cargo);
    // Say what the probe should see rather than letting it inherit: cargo lets
    // this variable outrank configuration, so a probe that disagrees with the
    // caller about it describes a different target directory than the build's.
    match target_dir_env {
        Some(dir) => command.env(CARGO_TARGET_DIR_ENV, dir),
        None => command.env_remove(CARGO_TARGET_DIR_ENV),
    };
    // Globals come first; cargo rejects them after the subcommand.
    command.args(forwarded_flags(arguments, &PROBE_GLOBAL_FLAGS));
    command.args(["metadata", "--no-deps", "--format-version", "1"]);
    // Describe the project the build will actually operate on.
    if let Some(manifest) = flag_value(arguments, "--manifest-path") {
        command.args(["--manifest-path", manifest]);
    }
    for toggle in arguments
        .iter()
        .filter(|argument| PROBE_MANIFEST_TOGGLES.contains(&argument.as_str()))
    {
        command.arg(toggle);
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
        // What cargo reports has folded configuration in already, so it cannot
        // say whether anyone asked. The caller's flags, environment, and Cargo
        // configuration answer that, and `resolve_roots_with` reads them itself.
        target_dir_requested: false,
    })
}

fn target_dir_argument(arguments: &[String]) -> Option<&str> {
    flag_value(arguments, "--target-dir")
}

/// Whether Cargo configuration may have named the target directory.
///
/// `cargo metadata` reports only the resolved path, so an explicit
/// `build.target-dir = "target"` is otherwise indistinguishable from Cargo's
/// default. Any explicit config file passed on the command line is treated
/// conservatively because it may include another file.
fn cargo_config_may_set_target_dir(arguments: &[String], invocation_dir: &Path) -> bool {
    if std::env::var_os("CARGO_BUILD_TARGET_DIR").is_some_and(|value| !value.is_empty()) {
        return true;
    }
    if config_arguments(arguments).any(|value| {
        value
            .split_once('=')
            .is_none_or(|(key, _)| matches!(key.trim(), "build.target-dir" | "include"))
    }) {
        return true;
    }

    let project_configs = invocation_dir.ancestors().flat_map(|directory| {
        let cargo = directory.join(".cargo");
        [cargo.join("config.toml"), cargo.join("config")]
    });
    let home_configs = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
        .into_iter()
        .flat_map(|cargo| [cargo.join("config.toml"), cargo.join("config")]);
    project_configs
        .chain(home_configs)
        .any(|path| cargo_config_file_may_set_target_dir(&path))
}

fn config_arguments(arguments: &[String]) -> impl Iterator<Item = &str> {
    let mut values = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if let Some(value) = argument.strip_prefix("--config=") {
            values.push(value);
        } else if argument == "--config"
            && let Some(value) = remaining.next()
        {
            values.push(value.as_str());
        }
    }
    values.into_iter()
}

fn cargo_config_file_may_set_target_dir(path: &Path) -> bool {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let Ok(config) = toml::from_str::<toml::Value>(&contents) else {
        // Cargo will reject it, but mbx must not mutate a target directory on
        // the way to that error.
        return true;
    };
    config
        .get("build")
        .and_then(|build| build.get("target-dir"))
        .is_some()
        // An included file may contain the setting; Cargo owns that merge.
        || config.get("include").is_some()
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

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
