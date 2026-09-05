use super::gc::sweep_store;
use crate::config::{CliSettings, Config, RetentionSettings, SummaryStyle};
use crate::session::{self, CacheSession};
use crate::{policy, target};
use bytesize::ByteSize;
use eyre::{Context, Result};
use mbx_cache_core::AgentStats;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Marks that this machine has been told what mbx set up.
///
/// A stamp of its own rather than a side effect of some other file: whether to
/// explain mbx and whether to count a run's savings are unrelated questions,
/// and answering both from one file forces every change to one to reason about
/// the other.
const NOTICE_STAMP: &str = "notice/v1/explained";

pub(super) fn run(
    config: &Config,
    settings: &CliSettings,
    arguments: &[String],
) -> Result<ExitCode> {
    cargo_with_settings_bypass_log_and_roots(config, settings, arguments, None, None)
}

/// Run Cargo with roots an earlier probe already resolved.
///
/// The persistent Cargo shim probes before loading configuration so an
/// invocation outside a usable workspace can pass through untouched. Reusing
/// that result here keeps the shim from running the same `cargo metadata`
/// command twice.
pub(super) fn run_with_roots(
    config: &Config,
    settings: &CliSettings,
    arguments: &[String],
    roots: Roots,
) -> Result<ExitCode> {
    cargo_with_settings_bypass_log_and_roots(config, settings, arguments, None, Some(roots))
}

pub(crate) fn cargo_with_bypass_log(
    config: &Config,
    arguments: &[String],
    bypass_log: Option<&Path>,
) -> Result<ExitCode> {
    cargo_with_settings_bypass_log_and_roots(
        config,
        &CliSettings::default(),
        arguments,
        bypass_log,
        None,
    )
}

pub(crate) fn cargo_with_settings_and_bypass_log(
    config: &Config,
    settings: &CliSettings,
    arguments: &[String],
    bypass_log: Option<&Path>,
) -> Result<ExitCode> {
    cargo_with_settings_bypass_log_and_roots(config, settings, arguments, bypass_log, None)
}

fn cargo_with_settings_bypass_log_and_roots(
    config: &Config,
    settings: &CliSettings,
    arguments: &[String],
    bypass_log: Option<&Path>,
    roots: Option<Roots>,
) -> Result<ExitCode> {
    let retention = &settings.retention;
    let summary = if cargo_is_quiet(arguments) {
        SummaryStyle::Off
    } else {
        settings.summary
    };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    // Only where mbx can identify the linker precisely enough to key what it
    // produced. Said out loud only to somebody who asked for it: this is on
    // by default now, and a platform that cannot do it would otherwise warn
    // every build about a setting nobody chose.
    let cache_links = settings.cache_links && session::cache_links_supported();
    if settings.cache_links && !cache_links && std::env::var_os(session::CACHE_LINKS_ENV).is_some()
    {
        log::warn!("caching native links is not supported on this platform");
    }
    let working_dir = std::env::current_dir()?;
    let roots = roots.unwrap_or_else(|| resolve_roots(&cargo, arguments, &working_dir));
    let mut config = config.clone();
    config.apply_workspace_policy(&roots.workspace_root)?;
    let managed_linker = if cargo_help_requested(arguments) {
        None
    } else {
        crate::managed_linker::resolve(&config.linker, &config.cache_dir, &config.http, arguments)?
    };
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
    // Cargo-managed incremental already covers the inner loop, and a shadow
    // compilation has nothing to compare against if its inputs carry
    // incremental state, so learned reuse yields to both.
    let learned_incremental = policy::learned_incremental_allowed(settings.learned_incremental)
        && !incremental
        && !config.verify;
    let config = &config;

    let migrate_existing = prompt_to_manage_existing_target(config, &roots, arguments)?;
    // Placed before the session starts, because the target directory is what
    // the shim maps out of its cache keys and it has to be the one cargo will
    // actually write to.
    let (placement, removed_target_bytes) = if migrate_existing {
        let outcome = target::migrate_existing(
            config,
            &roots.workspace_root,
            &roots.target_dir,
            roots.target_dir_requested,
        )?;
        (
            TargetViewPlacement {
                directory: outcome.managed,
                touch_path: roots.target_dir.clone(),
            },
            outcome.removed_bytes,
        )
    } else {
        (place_target_view(config, &roots), None)
    };
    if let Some(bytes) = removed_target_bytes {
        crate::session::note(&format!(
            "mbx[gc]: freed {} by removing the existing target/ directory",
            ByteSize::b(bytes).display().iec()
        ));
    }
    if placement.directory.is_none() {
        // Placement declined, but an earlier one may have left a link this
        // build is about to write through. Keep that directory's record fresh
        // so collection does not treat it as idle.
        target::touch_managed(config, &roots.workspace_root, &placement.touch_path);
    }

    // Probed rather than assumed, and only on the one run that will say
    // something about it. Placement is best-effort, so wait until it has
    // finished and probe the directory cargo will actually use rather than
    // predicting that a managed target will win.
    if !policy::is_ci() && !cargo_help_requested(arguments) && !was_explained(&config.store_dir()) {
        let target_dir = placement.directory.as_deref().unwrap_or(&roots.target_dir);
        let reflinks = crate::util::reflinks_work(&config.cache_dir, target_dir);
        crate::session::note(&first_run_notice(config, retention, reflinks));
        mark_explained(&config.store_dir());
    }

    let session_dir = tempfile::Builder::new().prefix("mbx-session-").tempdir()?;
    let cargo_jobs = cargo_job_limit(arguments);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let session_outcome = runtime.block_on(async {
        let session = match CacheSession::start_with_jobs(session_dir.path(), config, cargo_jobs)
            .await
        {
            Ok(session) => session,
            Err(error) if session::listener_unavailable(&error) => {
                crate::session::note(&format!(
                    "mbx[warning]: cache session unavailable; running Cargo without mbx caching: {error:#}"
                ));
                // Protect nested Cargo calls from re-entering mbx, and ensure
                // compiler wrappers cannot inherit an enclosing session that
                // this build did not start. An empty socket is deliberately
                // equivalent to an absent one to every mbx shim.
                let environment = BTreeMap::from([
                    ("MBX_DISABLE".into(), "1".into()),
                    (session::SOCKET_ENV.into(), String::new()),
                ]);
                return Ok((run_cargo(&cargo, arguments, environment), None));
            }
            Err(error) => return Err(error),
        };
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
        if placement.directory.is_some() {
            // Keep Cargo's public artifact paths anchored in the checkout.
            // The target link still puts the bytes in the managed view, while
            // debugger launch configurations survive collection and rebuilds
            // instead of remembering mbx's private, disposable path.
            environment.insert(
                CARGO_TARGET_DIR_ENV.into(),
                roots.target_dir.to_string_lossy().into_owned(),
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
        // Clear inherited selections, including when this build uses the system linker.
        environment.insert(session::MANAGED_LINKER_ENV.into(), String::new());
        environment.insert(session::MANAGED_TARGET_LINKERS_ENV.into(), String::new());
        if let Some(selection) = &managed_linker {
            match selection {
                crate::managed_linker::Selection::Single(executable) => {
                    environment.insert(session::MANAGED_LINKER_ENV.into(), executable.to_string_lossy().into_owned());
                }
                crate::managed_linker::Selection::Targets(executables) => {
                    environment.insert(session::MANAGED_TARGET_LINKERS_ENV.into(), serde_json::to_string(executables)?);
                }
            }
        }
        // Stated explicitly for the same reason as the session's own keys: an
        // unset value would let the shim inherit one from the parent, with no
        // way to turn it off here.
        environment.insert(
            session::LEARNED_INCREMENTAL_ENV.into(),
            if learned_incremental { "1" } else { "0" }.into(),
        );
        environment.insert(
            session::LEARNED_INCREMENTAL_MAX_SIZE_ENV.into(),
            settings
                .learned_incremental_max_size
                .map_or_else(|| "none".to_string(), |bytes| bytes.to_string()),
        );
        environment.insert(
            session::CACHE_LINKS_ENV.into(),
            if cache_links { "1" } else { "0" }.into(),
        );

        let status = run_cargo(&cargo, arguments, environment);
        // The shim records a prediction only after a compilation has either
        // been restored or published successfully. Preserve that completed
        // portion even when a later compilation makes cargo fail: it is still
        // useful to the retry, and the collector must know it is reachable.
        if let Some(run) = run
            && let Err(error) = run.commit().await
        {
            log::warn!("the completed build was not fully recorded: {error}");
        }

        let stats = match session.finish().await {
            Ok(stats) => {
                crate::session::display_stats(&stats, config, summary);
                Some(stats)
            }
            Err(error) => {
                log::warn!("the cache session did not shut down cleanly: {error}");
                None
            }
        };
        Ok((status, stats))
    });
    account_session(config, settings, session_outcome, removed_target_bytes)
}

pub(super) struct TargetViewPlacement {
    pub(super) directory: Option<PathBuf>,
    pub(super) touch_path: PathBuf,
}

/// Place the editor's explicit target inside the checkout's managed view.
///
/// Cargo's `--target-dir` normally opts out of placement. The exact path that
/// `mbx setup` writes is different: placing its `target` parent first prevents
/// an editor-first checkout from creating a real directory that would block
/// managed targets later. Cargo still writes into the requested child, so its
/// directory lock remains independent from terminal builds.
pub(super) fn place_target_view(config: &Config, roots: &Roots) -> TargetViewPlacement {
    let default = roots.workspace_root.join("target");
    if roots.target_dir_requested
        && roots.target_dir == roots.workspace_root.join(super::RUST_ANALYZER_TARGET_DIR)
    {
        let directory = target::place(config, &roots.workspace_root, &default, false)
            .map(|_| roots.target_dir.clone());
        return TargetViewPlacement {
            directory,
            touch_path: default,
        };
    }
    TargetViewPlacement {
        directory: target::place(
            config,
            &roots.workspace_root,
            &roots.target_dir,
            roots.target_dir_requested,
        ),
        touch_path: roots.target_dir.clone(),
    }
}

/// Sweep the store and record the session's savings after a build session.
///
/// Shared by every session-running command, and placed after its runtime has
/// been dropped, so a walk of the whole store cannot occupy a worker thread
/// and adds nothing to the build the user is waiting on.
pub(super) fn account_session(
    config: &Config,
    settings: &CliSettings,
    session_outcome: Result<(Result<ExitCode>, Option<AgentStats>)>,
    removed_target_bytes: Option<u64>,
) -> Result<ExitCode> {
    let retention = &settings.retention;
    // A session that never started still leaves collection to do, so the error
    // travels as the build's result rather than short-circuiting past the sweep.
    let (status, stats) = match session_outcome {
        Ok((status, stats)) => (status, stats),
        Err(error) => (Err(error), None),
    };
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

/// Run a build command outside cargo with the C and C++ shims first on PATH.
pub(super) fn was_explained(store: &Path) -> bool {
    store.join(NOTICE_STAMP).exists()
}

/// Written immediately after the notice prints, so a build that fails later
/// does not explain itself again.
pub(super) fn mark_explained(store: &Path) {
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
pub(super) fn first_run_notice(
    config: &Config,
    retention: &RetentionSettings,
    reflinks: bool,
) -> String {
    let mut lines = vec!["mbx[setup]: first build on this machine".to_string()];
    // Collection is what the rest of this describes, so a machine that turned
    // it off is told what it has instead of what it does not.
    if config.gc.auto {
        lines.push(format!(
            "mbx[setup]:   cache is {}, shared by every checkout and worktree, pruned to {}",
            config.cache_dir.display(),
            ByteSize::b(config.gc.max_bytes).display().iec(),
        ));
    } else {
        lines.push(format!(
            "mbx[setup]:   cache is {}, shared by every checkout and worktree; automatic collection is off, so only `mbx gc` reclaims it",
            config.cache_dir.display(),
        ));
    }
    // Only when a probe just proved it: this is a promise about what the
    // user's disk will do, and a machine on ext4 must not be promised
    // sharing that every restore will quietly turn into a copy.
    if reflinks {
        lines.push(
            "mbx[setup]:   this filesystem supports reflinks, so target/ shares disk with the cache instead of copying"
                .to_string(),
        );
    }
    if config.target.views && config.gc.auto {
        let mut reasons = vec!["its checkout is gone".to_string()];
        if let Some(age) = retention.target_max_age {
            reasons.push(format!("unused for {}", crate::util::format_span(age)));
        }
        if let Some(bytes) = retention.target_max_bytes {
            reasons.push(format!("over {} total", ByteSize::b(bytes).display().iec()));
        }
        lines.push(format!(
            "mbx[setup]:   target/ is managed: deleted when {}",
            join_clauses(&reasons),
        ));
    }
    lines.push(
        "mbx[setup]:   `mbx gc --dry-run` previews cleanup; every limit is configurable"
            .to_string(),
    );

    lines.join("\n")
}

/// Join reasons as prose: "a", "a or b", "a, b, or c".
pub(super) fn join_clauses(clauses: &[String]) -> String {
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
pub(super) fn prompt_to_manage_existing_target(
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

pub(super) fn cargo_help_requested(arguments: &[String]) -> bool {
    arguments.first().is_some_and(|argument| argument == "help")
        || arguments
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
}

pub(super) fn prompt_to_manage_existing_target_with(
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

pub(super) fn run_cargo(
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

/// Cargo's quiet flag applies to mbx's build summary as well as Cargo's own
/// progress. Arguments after `--` belong to rustc or the program being run.
pub(super) fn cargo_is_quiet(arguments: &[String]) -> bool {
    arguments
        .iter()
        .take_while(|argument| argument.as_str() != "--")
        .any(|argument| matches!(argument.as_str(), "-q" | "--quiet"))
}

#[cfg(unix)]
pub(super) fn exit_code(status: std::process::ExitStatus) -> ExitCode {
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
pub(super) fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    match status.code() {
        Some(code) => ExitCode::from(code as u8),
        None => ExitCode::FAILURE,
    }
}

/// Settings the shim maps out of its cache keys.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Roots {
    pub(super) workspace_root: PathBuf,
    pub(super) target_dir: PathBuf,
    /// Whether a flag, environment variable, or Cargo configuration named the
    /// target directory outright.
    ///
    /// Cargo prefers `--target-dir` over `CARGO_TARGET_DIR`, so a build carrying
    /// that flag cannot be moved by setting the environment: cargo would write
    /// where the flag says while the shim had been told the managed path, and
    /// the whole build would stop keying against anything it could reuse. The
    /// value can equal the default location and still have been asked for, so
    /// comparing paths cannot answer this.
    pub(super) target_dir_requested: bool,
}

/// Carry rustc wrappers the caller already configured into the session.
///
/// The session records them so the shim can defer to them; without this a
/// workspace wrapper is mistaken for rustc because Cargo nests it inside
/// `RUSTC_WRAPPER`.
pub(super) fn inherited_environment(
    get_env: impl Fn(&str) -> Option<String>,
    working_dir: &Path,
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    if let Some(wrapper) = get_env("RUSTC_WRAPPER").filter(|value| !value.is_empty()) {
        environment.insert("RUSTC_WRAPPER".into(), wrapper);
    }
    if let Some(wrapper) = get_env("RUSTC_WORKSPACE_WRAPPER").filter(|value| !value.is_empty()) {
        environment.insert("RUSTC_WORKSPACE_WRAPPER".into(), wrapper);
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
pub(super) fn resolve_roots(
    cargo: &std::ffi::OsStr,
    arguments: &[String],
    working_dir: &Path,
) -> Roots {
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
pub(super) const CARGO_TARGET_DIR_ENV: &str = "CARGO_TARGET_DIR";

/// [`resolve_roots`] with the ambient `CARGO_TARGET_DIR` passed in rather than
/// read here, so a caller can resolve as if the variable were unset.
pub(super) fn resolve_roots_with(
    cargo: &std::ffi::OsStr,
    arguments: &[String],
    working_dir: &Path,
    target_dir_env: Option<std::ffi::OsString>,
) -> Roots {
    let resolved = mbx_cache_cargo::resolve(cargo, arguments, working_dir, target_dir_env);
    Roots {
        workspace_root: resolved.workspace_root,
        target_dir: resolved.target_dir,
        target_dir_requested: resolved.target_dir_requested,
    }
}

/// Cargo resolves a relative directory against the invocation directory.
pub(super) fn absolute(working_dir: &Path, value: &str) -> PathBuf {
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
#[cfg(test)]
pub(super) const PROBE_GLOBAL_FLAGS: [&str; 3] = ["-C", "--config", "-Z"];

/// Collect the occurrences of `flags` from `arguments`, preserving order and
/// repeats. Cargo allows `--flag value`, `--flag=value`, and, for the short
/// forms, `-Zvalue`.
#[cfg(test)]
pub(super) fn forwarded_flags(arguments: &[String], flags: &[&str]) -> Vec<String> {
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

pub(super) fn cargo_roots(
    cargo: &std::ffi::OsStr,
    arguments: &[String],
    target_dir_env: Option<&std::ffi::OsStr>,
) -> Option<Roots> {
    let working_dir = std::env::current_dir().ok()?;
    let resolved = mbx_cache_cargo::resolve_reported(
        cargo,
        arguments,
        &working_dir,
        target_dir_env.map(std::ffi::OsStr::to_os_string),
    )?;
    Some(Roots {
        workspace_root: resolved.workspace_root,
        target_dir: resolved.target_dir,
        target_dir_requested: resolved.target_dir_requested,
    })
}

#[cfg(test)]
pub(super) fn parse_cargo_roots(metadata: &[u8]) -> Option<Roots> {
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
/// Cargo's own compiler-process limit, when it narrows the scheduler pool.
///
/// The CLI wins over `CARGO_BUILD_JOBS`, including `default`, just as it does
/// in Cargo. Invalid values are left for Cargo to diagnose and do not make mbx
/// invent a second interpretation.
fn cargo_job_limit(arguments: &[String]) -> Option<u64> {
    cargo_job_limit_with(
        arguments,
        std::env::var("CARGO_BUILD_JOBS").ok().as_deref(),
        std::thread::available_parallelism().map_or(1, |cpus| cpus.get() as u64),
    )
}

pub(super) fn cargo_job_limit_with(
    arguments: &[String],
    environment: Option<&str>,
    logical_cpus: u64,
) -> Option<u64> {
    let mut cli_value = None;
    let mut cli_seen = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--" {
            break;
        }
        let value = if argument == "-j" || argument == "--jobs" {
            index += 1;
            arguments.get(index).map(String::as_str)
        } else if let Some(value) = argument.strip_prefix("--jobs=") {
            Some(value)
        } else if let Some(value) = argument.strip_prefix("-j=") {
            Some(value)
        } else {
            argument
                .strip_prefix("-j")
                .filter(|value| !value.is_empty())
        };
        if let Some(value) = value {
            cli_seen = true;
            cli_value = resolve_cargo_jobs(value, logical_cpus);
        }
        index += 1;
    }
    if cli_seen {
        cli_value
    } else {
        environment.and_then(|value| resolve_cargo_jobs(value, logical_cpus))
    }
}

fn resolve_cargo_jobs(value: &str, logical_cpus: u64) -> Option<u64> {
    if value == "default" {
        return None;
    }
    let jobs = value.parse::<i64>().ok()?;
    if jobs > 0 {
        return Some(jobs as u64);
    }
    if jobs < 0 {
        return i128::from(logical_cpus)
            .checked_add(i128::from(jobs))
            .filter(|jobs| *jobs > 0)
            .and_then(|jobs| u64::try_from(jobs).ok());
    }
    None
}
