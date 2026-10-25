use super::cargo::{CARGO_TARGET_DIR_ENV, cargo_roots, exit_code};
use crate::config::Config;
use eyre::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;

const CARGO_SHIM_STEM: &str = "cargo";
const CARGO_SHIM_MODE_ENV: &str = "MBX_CARGO_SHIM_MODE";
const CARGO_SHIM_PATH_ENV: &str = "MBX_CARGO_SHIM_PATH";
pub(super) const CARGO_SHIM_DEPTH_ENV: &str = "MBX_CARGO_SHIM_DEPTH";
pub(super) const CARGO_SHIM_CHAIN_ENV: &str = "MBX_CARGO_SHIM_CHAIN";
/// Build scripts and Cargo subcommands legitimately reach the shim again, so a
/// repeat is not proof of a cycle. A real wrapper loop never stops, so a depth
/// far beyond any plausible nesting is.
const MAX_CARGO_SHIM_DEPTH: usize = 12;
const CARGO_SHIM_NOTE_ENV: &str = "MBX_CARGO_SHIM_NOTE";
/// Whether this process chose the loop note path, and so removes the note.
static OWNS_LOOP_NOTE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PATH_BEFORE_SHIM_EXCLUSION: OnceLock<OsString> = OnceLock::new();

/// Whether this process was installed under Cargo's name by `mbx setup`.
pub fn is_cargo_shim() -> bool {
    match std::env::var_os(CARGO_SHIM_MODE_ENV).as_deref() {
        // Internal commands must remain mbx commands even when current_exe()
        // points at an installation hardlinked or copied under Cargo's name.
        Some(value) if value == "0" => false,
        Some(value) if value == "1" => true,
        _ => invoked_as_cargo(),
    }
}

fn invoked_as_cargo() -> bool {
    std::env::args_os()
        .next()
        .as_deref()
        .is_some_and(is_cargo_shim_path)
}

fn is_cargo_shim_path(path: &OsStr) -> bool {
    Path::new(path).file_stem().is_some_and(|stem| {
        if cfg!(windows) {
            stem.to_string_lossy().eq_ignore_ascii_case(CARGO_SHIM_STEM)
        } else {
            stem == OsStr::new(CARGO_SHIM_STEM)
        }
    })
}

/// Run an invocation received through the persistent Cargo shim.
pub fn run_cargo_shim() -> Result<ExitCode> {
    let result = run_cargo_shim_inner();
    // Passthrough and early exits never read the note a loop may have left.
    if OWNS_LOOP_NOTE.load(std::sync::atomic::Ordering::Relaxed)
        && let Some(path) = std::env::var_os(CARGO_SHIM_NOTE_ENV)
    {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn run_cargo_shim_inner() -> Result<ExitCode> {
    let reported_shim = std::env::var_os(CARGO_SHIM_PATH_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            invoked_as_cargo()
                .then(|| std::env::current_exe().ok())
                .flatten()
        });
    // mise removes its wrapper directory before launching mbx, so there is no
    // shim path to exclude. Treating the first remaining Cargo as a shim would
    // instead remove the real tool from PATH.
    let invoked_via_mise_wrapper = !invoked_as_cargo() && reported_shim.is_none();
    // The Windows launcher uses this only to select dispatch in the target mbx.
    // Do not leak it into Cargo, build scripts, or nested mbx commands.
    unsafe { std::env::remove_var(CARGO_SHIM_MODE_ENV) };
    unsafe { std::env::remove_var(CARGO_SHIM_PATH_ENV) };
    #[cfg(windows)]
    if invoked_as_cargo()
        && let Some(code) = forward_to_current_mbx(reported_shim.as_deref())?
    {
        return Ok(code);
    }
    let shim_dir = super::setup_install_dir()
        .ok_or_else(|| eyre::eyre!("the platform data directory could not be located"))?;
    let configured_shim = shim_dir.join(if cfg!(windows) { "cargo.exe" } else { "cargo" });
    let configured_cargo = std::env::var_os("CARGO");
    // What blaze is given as its Cargo, so that the builds it hands to Cargo
    // come back here. mise's wrapper has no path of its own to give, so only
    // an installed shim can stand in for it.
    let blaze_shim;
    let real_cargo = if invoked_via_mise_wrapper {
        blaze_shim = super::blaze::is_executable(&configured_shim).then(|| configured_shim.clone());
        selected_cargo(&configured_shim)?
    } else {
        let shim = resolve_active_cargo_shim(
            &configured_shim,
            reported_shim.as_deref().map(Path::as_os_str),
            std::env::var_os("PATH").as_deref(),
        );
        exclude_shim_from_path(&shim)?;
        let real_cargo = selected_cargo(&shim)?;
        blaze_shim = Some(shim);
        real_cargo
    };
    record_cargo_selection(&real_cargo, configured_cargo.as_deref())?;
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if mbx_disabled() {
        return run_real_cargo(&real_cargo, &arguments);
    }
    if enclosing_session(std::env::var_os(crate::session::SOCKET_ENV).as_deref())
        || cargo_proxy_passthrough(&arguments)
    {
        if super::launch::cargo_subcommand(&arguments) == Some("clean") {
            drop_dangling_managed_link(&real_cargo, &arguments);
        }
        return run_real_cargo(&real_cargo, &arguments);
    }
    let Ok(string_arguments) = super::strings(&arguments) else {
        return run_real_cargo(&real_cargo, &arguments);
    };
    // Read the settings early only for a command blaze could take, so that
    // every other one still reaches its passthrough without them.
    let mut loaded = None;
    if super::blaze::wanted(&string_arguments) {
        let (config, settings) = Config::load_for_cli()?;
        if let Some(result) = super::blaze::route(
            settings.blaze,
            &string_arguments,
            blaze_shim.as_deref(),
            &real_cargo,
        ) {
            return result;
        }
        loaded = Some((config, settings));
    }
    // The roots probe can persist metadata under the cache root. Reject its
    // placement before probing, but only after passthrough decisions above.
    if let Some(root) = mbx_cache_cargo::cache_root() {
        crate::storage::require_local(&root, "mbx cache directory", "MBX_CACHE_DIR")?;
        crate::storage::require_local(
            &root.join("cargo-roots"),
            "mbx cache directory",
            "MBX_CACHE_DIR",
        )?;
    }
    let target_dir_env = std::env::var_os(CARGO_TARGET_DIR_ENV);
    let roots = match cargo_roots(&real_cargo, &string_arguments, target_dir_env.as_deref()) {
        Some(roots) => roots,
        None => match failed_probe(&real_cargo, &arguments, target_dir_env.as_deref()) {
            FailedProbe::Roots(roots) => *roots,
            FailedProbe::Passthrough => return run_real_cargo(&real_cargo, &arguments),
            FailedProbe::Reject => {
                let hint = take_loop_note()
                    .map(|hint| format!(" ({hint})"))
                    .unwrap_or_default();
                eyre::bail!(
                    "could not verify Cargo build storage: metadata probing failed{hint}; run cargo metadata --no-deps --format-version 1 with the same manifest and configuration options to diagnose it"
                )
            }
        },
    };
    // Every probe and the final child must name Cargo directly.
    unsafe { std::env::set_var("CARGO", &real_cargo) };
    let (config, settings) = match loaded {
        Some(loaded) => loaded,
        None => Config::load_for_cli()?,
    };
    super::cargo::run_with_roots(&config, &settings, &string_arguments, roots)
}

/// Fail promptly when the "real" Cargo this shim selected leads back into an
/// mbx shim, as a custom wrapper script that execs mbx does. Each shim entry
/// exports its depth and the Cargo it selected so the next one can see them.
fn record_cargo_selection(real_cargo: &OsStr, configured: Option<&OsStr>) -> Result<()> {
    let depth = std::env::var(CARGO_SHIM_DEPTH_ENV).ok();
    let chain = std::env::var_os(CARGO_SHIM_CHAIN_ENV);
    let (depth, chain) =
        match next_cargo_selection(depth.as_deref(), chain.as_deref(), real_cargo, configured) {
            Ok(selection) => selection,
            Err(error) => {
                leave_loop_note(&format!("{error}"));
                return Err(error);
            }
        };
    // SAFETY: CLI dispatch is single-threaded here, before any child process
    // or cache session has started.
    unsafe { std::env::set_var(CARGO_SHIM_DEPTH_ENV, depth.to_string()) };
    if let Some(chain) = chain {
        unsafe { std::env::set_var(CARGO_SHIM_CHAIN_ENV, chain) };
    }
    if std::env::var_os(CARGO_SHIM_NOTE_ENV).is_none() {
        unsafe { std::env::set_var(CARGO_SHIM_NOTE_ENV, loop_note_path()) };
        OWNS_LOOP_NOTE.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
}

/// Where a re-entered shim leaves its diagnosis for the outermost one. The
/// roots probe discards its child's stderr, and asking a rejected Cargo to run
/// again just to read the answer would launch it when nothing was verified.
fn loop_note_path() -> PathBuf {
    mbx_cache_cargo::cache_root()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("cargo-shim-loop-{}", std::process::id()))
}

fn leave_loop_note(message: &str) {
    let Some(path) = std::env::var_os(CARGO_SHIM_NOTE_ENV) else {
        return;
    };
    if let Some(parent) = Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // `create_new` neither follows a planted link nor replaces a note.
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        let _ = std::io::Write::write_all(&mut file, message.as_bytes());
    }
}

fn take_loop_note() -> Option<String> {
    let path = PathBuf::from(std::env::var_os(CARGO_SHIM_NOTE_ENV)?);
    let note = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(&path);
    note.and_then(|note| {
        note.lines()
            .find(|line| line.contains("re-entered its Cargo shim"))
            .map(|line| line.trim().to_string())
    })
}

/// The depth and chain to export, given those this shim inherited.
fn next_cargo_selection(
    depth: Option<&str>,
    chain: Option<&OsStr>,
    real_cargo: &OsStr,
    configured: Option<&OsStr>,
) -> Result<(usize, Option<OsString>)> {
    let depth = depth
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    // Diagnostic history, not a search path: a Cargo path may contain the PATH
    // separator, so entries are newline-separated and kept as display text.
    let mut selected = chain
        .map(|chain| {
            chain
                .to_string_lossy()
                .split('\n')
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    selected.push(PathBuf::from(real_cargo));
    if depth >= MAX_CARGO_SHIM_DEPTH {
        let source = if configured == Some(real_cargo) {
            "the CARGO environment variable"
        } else {
            "PATH"
        };
        let chain = selected
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" -> ");
        eyre::bail!(
            "mbx re-entered its Cargo shim {depth} times: the Cargo it selected from {source} ({}) runs mbx again. Selected so far: {chain}. Set CARGO in the outer wrapper to the real Cargo, or remove the wrapper that runs mbx from PATH",
            Path::new(real_cargo).display()
        );
    }
    let chain = selected
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((depth + 1, Some(OsString::from(chain))))
}

/// Collection removes a managed target but leaves the checkout's `target` link
/// dangling until the next build places it again. Cargo cannot create a
/// directory through that link, so `cargo clean -p` would fail on a checkout
/// that has nothing left to clean. Dropping the link first lets Cargo report
/// the clean it would have done. Best effort: any failure leaves Cargo to run
/// against the checkout as it is.
fn drop_dangling_managed_link(real_cargo: &OsStr, arguments: &[OsString]) {
    let Ok(string_arguments) = super::strings(arguments) else {
        return;
    };
    let target_dir_env = std::env::var_os(CARGO_TARGET_DIR_ENV);
    let Some(roots) = cargo_roots(real_cargo, &string_arguments, target_dir_env.as_deref()) else {
        return;
    };
    if roots.target_dir != roots.workspace_root.join("target")
        || !std::fs::symlink_metadata(&roots.target_dir)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return;
    }
    let Ok((config, _)) = Config::load_for_cli() else {
        return;
    };
    if let Err(error) =
        crate::target::remove_dangling_link(&config.target.root, &roots.workspace_root)
    {
        log::warn!("could not remove the dangling managed target link: {error:#}");
    }
}

fn resolve_active_cargo_shim(
    configured: &Path,
    reported: Option<&OsStr>,
    path: Option<&OsStr>,
) -> PathBuf {
    reported
        .map(Path::new)
        .filter(|shim| is_cargo_shim_path(shim.as_os_str()) && shim.is_file())
        .map(Path::to_path_buf)
        .or_else(|| {
            let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
            std::env::split_paths(path?)
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
        })
        .unwrap_or_else(|| configured.to_path_buf())
}

#[cfg(windows)]
fn forward_to_current_mbx(reported_shim: Option<&Path>) -> Result<Option<ExitCode>> {
    let Some(install_dir) = super::setup_install_dir() else {
        return Ok(None);
    };
    let Some(target) = super::setup::cargo_shim_target(&install_dir) else {
        return Ok(None);
    };
    let mut command = Command::new(&target);
    command
        .args(std::env::args_os().skip(1))
        .env(CARGO_SHIM_MODE_ENV, "1");
    if let Some(reported_shim) = reported_shim {
        command.env(CARGO_SHIM_PATH_ENV, reported_shim);
    }
    let status = command
        .status()
        .wrap_err_with(|| format!("failed to run the current mbx at {}", target.display()))?;
    Ok(Some(exit_code(status)))
}

fn mbx_disabled() -> bool {
    mbx_disabled_value(std::env::var_os("MBX_DISABLE").as_deref())
}

fn mbx_disabled_value(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy();
        value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn enclosing_session(session_socket: Option<&OsStr>) -> bool {
    // Cargo subcommands such as watch and nextest can invoke Cargo again. The
    // outer session's compiler wrapper and agent socket are already inherited,
    // so starting another session here would pay the setup cost for every
    // nested invocation and replace the long-lived watch session's context.
    session_socket.is_some_and(|socket| !socket.is_empty())
}

pub(super) fn cargo_proxy_passthrough(arguments: &[OsString]) -> bool {
    let cargo_arguments = arguments
        .iter()
        .take_while(|argument| argument.as_os_str() != OsStr::new("--"))
        .collect::<Vec<_>>();
    let command = super::launch::cargo_subcommand(arguments);
    if cargo_arguments.iter().any(|argument| {
        matches!(argument.to_str(), Some("-V" | "--help" | "-h"))
            // For install, --version selects a package version.
            || (argument.as_os_str() == "--version" && command != Some("install"))
    }) {
        return true;
    }
    if command == Some("install") {
        // Registry and Git installs compile in a scratch directory with no
        // local manifest, and run in a session of their own (see
        // `remote_install_roots`). Only the metadata query compiles nothing.
        return cargo_arguments
            .iter()
            .any(|argument| argument.as_os_str() == "--list");
    }
    command.is_none()
        || matches!(
            command,
            Some(
                "help"
                    | "new"
                    | "init"
                    | "add"
                    | "remove"
                    | "update"
                    | "fetch"
                    | "clean"
                    | "config"
                    | "fmt"
                    | "tree"
                    | "search"
                    | "info"
                    | "login"
                    | "logout"
                    | "owner"
                    | "package"
                    | "publish"
                    | "uninstall"
                    | "yank"
                    | "locate-project"
                    | "metadata"
                    | "generate-lockfile"
                    | "pkgid"
                    | "read-manifest"
                    | "report"
                    | "vendor"
                    | "verify-project"
                    | "version"
                    | "git-checkout"
            )
        )
}

/// What to do with an invocation whose roots probe failed.
pub(super) enum FailedProbe {
    /// Roots recovered from the expanded invocation: manage the build.
    Roots(Box<super::cargo::Roots>),
    /// Hand the invocation to Cargo as it was typed.
    Passthrough,
    /// Refuse it: the build storage cannot be verified.
    Reject,
}

/// Decide what a failed roots probe leaves the caller able to do.
///
/// Resolve once, then use the same arguments for root recovery and the
/// decision to allow a manifest-less invocation.
pub(super) fn failed_probe(
    cargo: &OsStr,
    arguments: &[OsString],
    target_dir_env: Option<&OsStr>,
) -> FailedProbe {
    // An alias can name a manifest that the command line does not, so expand
    // it before reading the invocation: `i = "install --path /project"`
    // compiles a local package from a directory that has none of its own.
    let invocation = super::cargo_invocation::resolve(cargo, arguments);
    // The first probe asked about the alias's own name and got nothing back.
    // Ask again as Cargo will read it: the roots belong to the package the
    // expansion names, though the child still receives the command line as it
    // was typed, since Cargo expands the alias itself.
    if let Some(resolved) = &invocation
        && resolved.arguments != arguments
        && let Ok(expanded) = super::strings(&resolved.arguments)
        && let Some(roots) = super::cargo::cargo_roots(cargo, &expanded, target_dir_env)
    {
        return FailedProbe::Roots(Box::new(roots));
    }
    if metadata_failure_passthrough(invocation.as_ref()) {
        note_uncached_passthrough(invocation.as_ref());
        return FailedProbe::Passthrough;
    }
    FailedProbe::Reject
}

/// Say that a command is running without a cache session. An external
/// subcommand may create the workspace itself, as `cargo chef cook` does in an
/// empty directory, and the builds it starts through `$CARGO` then never reach
/// mbx; without a message that looks like a slow build. Other commands outside
/// a project are routine, so they only log at debug level.
fn note_uncached_passthrough(invocation: Option<&super::cargo_invocation::Invocation>) {
    let Some(invocation) = invocation else {
        return;
    };
    let Some((position, command)) = super::launch::cargo_subcommand_at(&invocation.arguments)
    else {
        return;
    };
    // Cargo's own quiet flag precedes the subcommand; anything after it belongs
    // to the subcommand.
    let quiet = invocation.arguments[..position]
        .iter()
        .any(|argument| argument == "-q" || argument == "--quiet");
    if matches!(invocation.kind, super::cargo_invocation::Kind::External) && !quiet {
        log::info!(
            "no Cargo manifest in scope; running `cargo {command}` without the cache. If it builds a workspace it creates, write the manifest first so the build runs inside an mbx session"
        );
    } else {
        log::debug!("no Cargo manifest in scope; running `cargo {command}` without the cache");
    }
}

/// Preserve invocations outside a project, non-build aliases, and
/// unknown-command diagnostics without allowing failed metadata to launch a
/// build alias or an external Cargo command inside one.
fn metadata_failure_passthrough(invocation: Option<&super::cargo_invocation::Invocation>) -> bool {
    let Some(invocation) = invocation else {
        return false;
    };
    // Only losslessly resolved invocations reach this point. No manifest
    // means Cargo has no local package whose storage needs verification.
    if let Ok(strings) = super::strings(&invocation.arguments)
        && let Ok(working_dir) = std::env::current_dir()
        && !mbx_cache_cargo::manifest_in_scope(
            // External commands own their flags. A missing --manifest-path
            // must not disguise the project the invocation runs inside.
            if matches!(invocation.kind, super::cargo_invocation::Kind::External) {
                &[]
            } else {
                &strings
            },
            &working_dir,
        )
    {
        return true;
    }
    matches!(
        invocation.kind,
        super::cargo_invocation::Kind::Unknown | super::cargo_invocation::Kind::Proxy
    )
}

fn same_path(left: &Path, right: &Path) -> bool {
    let (left, right) = match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => (left, right),
        _ => (left.to_path_buf(), right.to_path_buf()),
    };
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Keep explicit `mbx build`-style commands from rediscovering a persistent
/// Cargo shim or mise command wrapper on PATH.
pub(super) fn prepare_explicit_cargo() -> Result<()> {
    let standalone_shim = super::setup_install_dir()
        .map(|directory| directory.join(if cfg!(windows) { "cargo.exe" } else { "cargo" }))
        .filter(|shim| shim.is_file());
    let Some(path) = std::env::var_os("PATH") else {
        return Ok(());
    };
    let (path, proxy) = path_without_cargo_proxies(&path, standalone_shim.as_deref())?;
    let Some(proxy) = proxy else {
        return Ok(());
    };
    let cargo = resolve_explicit_cargo(
        &proxy,
        standalone_shim.as_deref(),
        std::env::var_os("CARGO").as_deref(),
        &path,
    )?;
    PATH_BEFORE_SHIM_EXCLUSION.get_or_init(|| std::env::var_os("PATH").unwrap_or_default());
    // SAFETY: CLI dispatch is single-threaded here, before the cache session
    // or any child process has started.
    unsafe {
        std::env::set_var("PATH", path);
        std::env::set_var("CARGO", cargo);
    }
    Ok(())
}

fn path_without_cargo_proxies(
    path: &OsStr,
    standalone_shim: Option<&Path>,
) -> Result<(OsString, Option<PathBuf>)> {
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    let mut first_proxy = None;
    let directories = std::env::split_paths(path).filter(|directory| {
        let candidate = directory.join(name);
        let is_standalone = standalone_shim.is_some_and(|shim| same_path(&candidate, shim));
        let is_proxy =
            candidate.is_file() && (is_standalone || is_mise_command_wrapper_path(&candidate));
        if is_proxy {
            first_proxy.get_or_insert(candidate);
        }
        !is_proxy
    });
    Ok((std::env::join_paths(directories)?, first_proxy))
}

/// Resolve Cargo for an explicit command without accepting its standalone shim.
fn resolve_explicit_cargo(
    proxy: &Path,
    standalone_shim: Option<&Path>,
    configured: Option<&OsStr>,
    path: &OsStr,
) -> Result<OsString> {
    if let Some(configured) = configured.map(Path::new).filter(|candidate| {
        candidate.is_absolute()
            && candidate.is_file()
            && !is_mise_command_wrapper_path(candidate)
            && standalone_shim.is_none_or(|shim| !same_path(candidate, shim))
    }) {
        return Ok(configured.as_os_str().to_owned());
    }
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    for directory in std::env::split_paths(path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate.into_os_string());
        }
    }
    eyre::bail!(
        "could not find the real Cargo after excluding {}",
        proxy.display()
    )
}

fn exclude_shim_from_path(shim: &Path) -> Result<()> {
    let Some(path) = std::env::var_os("PATH") else {
        return Ok(());
    };
    PATH_BEFORE_SHIM_EXCLUSION.get_or_init(|| path.clone());
    let path = path_without_shim(shim, &path)?;
    // SAFETY: CLI dispatch is single-threaded here, before any child process
    // or cache session has started.
    unsafe { std::env::set_var("PATH", path) };
    Ok(())
}

pub(super) fn activation_path() -> Option<OsString> {
    PATH_BEFORE_SHIM_EXCLUSION
        .get()
        .cloned()
        .or_else(|| std::env::var_os("PATH"))
}

fn path_without_shim(shim: &Path, path: &OsStr) -> Result<OsString> {
    let Some(shim_dir) = shim.parent() else {
        return Ok(path.to_owned());
    };
    std::env::join_paths(
        std::env::split_paths(path).filter(|directory| !same_path(directory, shim_dir)),
    )
    .map_err(Into::into)
}

/// The Cargo this shim runs: the one it selected before handing the command to
/// blaze, when this is blaze's call coming back, else the real one.
fn selected_cargo(current: &Path) -> Result<OsString> {
    match super::blaze::returning_cargo(current) {
        Some(cargo) => Ok(cargo),
        None => resolve_real_cargo(current),
    }
}

fn resolve_real_cargo(current: &Path) -> Result<OsString> {
    let configured = std::env::var_os("CARGO");
    if let Some(cargo) = configured_real_cargo(current, configured.as_deref()) {
        return Ok(cargo);
    }
    let path = std::env::var_os("PATH").ok_or_else(|| eyre::eyre!("PATH is not set"))?;
    resolve_real_cargo_from(current, None, &path)
}

fn configured_real_cargo(current: &Path, configured: Option<&OsStr>) -> Option<OsString> {
    let configured = Path::new(configured?);
    let current_dir = current.parent();
    (configured.is_absolute()
        && configured.is_file()
        && !is_mise_cargo_proxy_path(configured)
        && current_dir.is_none_or(|current_dir| {
            configured
                .parent()
                .is_some_and(|parent| !same_path(parent, current_dir))
        }))
    .then(|| configured.as_os_str().to_owned())
}

pub(crate) fn is_mise_command_wrapper_path(path: &Path) -> bool {
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    path.ends_with(Path::new("command-wrappers").join("bin").join(name))
}

fn is_mise_cargo_proxy_path(path: &Path) -> bool {
    if is_mise_command_wrapper_path(path) {
        return true;
    }
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    path.file_name() == Some(OsStr::new(name)) && is_mise_shim_path(path)
}

/// Whether `path` is in mise's shims directory. A mise shim runs its tool only
/// where mise config enables it and fails everywhere else.
pub(crate) fn is_mise_shim_path(path: &Path) -> bool {
    is_mise_shim_path_in(
        path,
        std::env::var_os("MISE_SHIMS_DIR").as_deref(),
        std::env::var_os("MISE_DATA_DIR").as_deref(),
    )
}

/// `shims_dir` and `data_dir` are mise's `MISE_SHIMS_DIR` and `MISE_DATA_DIR`.
/// A `shims_dir` set only in mise's global config is not visible here.
fn is_mise_shim_path_in(path: &Path, shims_dir: Option<&OsStr>, data_dir: Option<&OsStr>) -> bool {
    let Some(directory) = path.parent() else {
        return false;
    };
    if shims_dir.is_some_and(|configured| same_path(directory, Path::new(configured))) {
        return true;
    }
    if !directory
        .file_name()
        .is_some_and(|name| file_name_is(name, "shims"))
    {
        return false;
    }
    let Some(parent) = directory.parent() else {
        return false;
    };
    parent
        .file_name()
        .is_some_and(|name| file_name_is(name, "mise"))
        || data_dir.is_some_and(|configured| same_path(parent, Path::new(configured)))
}

/// Windows compares file names without regard to case.
fn file_name_is(name: &OsStr, expected: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(expected)
    } else {
        name == expected
    }
}

fn resolve_real_cargo_from(
    current: &Path,
    configured: Option<&OsStr>,
    path: &OsStr,
) -> Result<OsString> {
    let current_dir = current.parent();
    if let Some(cargo) = configured_real_cargo(current, configured) {
        return Ok(cargo);
    }
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    for directory in std::env::split_paths(path) {
        if current_dir.is_some_and(|current_dir| same_path(&directory, current_dir)) {
            continue;
        }
        let candidate = directory.join(name);
        if candidate.is_file() && !is_mise_cargo_proxy_path(&candidate) {
            return Ok(candidate.into_os_string());
        }
    }
    eyre::bail!(
        "could not find the real Cargo after excluding {}",
        current.display()
    )
}

fn run_real_cargo(cargo: &OsStr, arguments: &[OsString]) -> Result<ExitCode> {
    let status = Command::new(cargo)
        .args(arguments)
        .status()
        .wrap_err_with(|| format!("failed to run {}", cargo.to_string_lossy()))?;
    Ok(exit_code(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_cargo_shim_entry_fails_with_the_selected_cargo() {
        let cargo = OsStr::new("/usr/local/bin/cargo");
        let error = next_cargo_selection(
            Some(&MAX_CARGO_SHIM_DEPTH.to_string()),
            Some(cargo),
            cargo,
            Some(cargo),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("CARGO environment variable"), "{error}");
        assert!(error.contains("/usr/local/bin/cargo"), "{error}");

        let (depth, chain) =
            next_cargo_selection(Some("2"), None, OsStr::new("/usr/bin/cargo"), None).unwrap();
        assert_eq!(depth, 3);
        assert_eq!(chain.as_deref(), Some(OsStr::new("/usr/bin/cargo")));
    }

    #[test]
    fn cargo_shim_chain_keeps_paths_containing_the_path_separator() {
        let first = OsStr::new("/tmp/rust:nightly/cargo");
        let (_, chain) = next_cargo_selection(None, None, first, None).unwrap();
        let error = next_cargo_selection(
            Some(&MAX_CARGO_SHIM_DEPTH.to_string()),
            chain.as_deref(),
            OsStr::new("/usr/bin/cargo"),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("/tmp/rust:nightly/cargo -> /usr/bin/cargo"),
            "{error}"
        );
    }

    #[test]
    fn first_cargo_shim_entry_starts_the_chain_without_an_empty_entry() {
        let error = next_cargo_selection(
            Some(&MAX_CARGO_SHIM_DEPTH.to_string()),
            None,
            OsStr::new("/usr/bin/cargo"),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("Selected so far: /usr/bin/cargo."),
            "{error}"
        );
    }

    #[test]
    fn cargo_shim_passthrough_preserves_cargo_namespace_and_toolchains() {
        assert!(cargo_proxy_passthrough(&[]));
        assert!(cargo_proxy_passthrough(&["--verbose".into()]));
        assert!(cargo_proxy_passthrough(&[
            "--color".into(),
            "always".into()
        ]));
        assert!(!cargo_proxy_passthrough(&[
            "--color".into(),
            "always".into(),
            "build".into()
        ]));
        assert!(cargo_proxy_passthrough(&[
            "-C".into(),
            "project".into(),
            "clean".into()
        ]));
        assert!(!cargo_proxy_passthrough(&["cache".into(), "stats".into()]));
        assert!(!cargo_proxy_passthrough(&[
            "install".into(),
            "ripgrep".into()
        ]));
        assert!(cargo_proxy_passthrough(&[
            "+nightly".into(),
            "--version".into()
        ]));
        assert!(!cargo_proxy_passthrough(&[
            "+nightly".into(),
            "build".into()
        ]));
        assert!(!cargo_proxy_passthrough(&[
            "test".into(),
            "--workspace".into()
        ]));
        assert!(!cargo_proxy_passthrough(&[
            "run".into(),
            "--".into(),
            "--help".into()
        ]));
        for command in [
            "clean",
            "config",
            "fmt",
            "pkgid",
            "read-manifest",
            "report",
            "vendor",
            "verify-project",
        ] {
            assert!(
                cargo_proxy_passthrough(&[command.into()]),
                "{command} should not start an mbx session"
            );
        }
        for command in [
            "build", "check", "test", "bench", "run", "rustc", "rustdoc", "doc", "clippy", "fix",
        ] {
            assert!(
                !cargo_proxy_passthrough(&[command.into()]),
                "{command} should start an mbx session"
            );
        }
    }

    #[test]
    fn cargo_shim_wraps_every_install_but_not_queries() {
        for args in [
            vec!["install", "--path", "."],
            vec!["install", "--path", ".", "--version", "0.1.0"],
            vec![
                "install",
                "--locked",
                "--path=foo",
                "--target-dir",
                "target",
            ],
            vec!["+stable", "--color", "always", "install", "--path", "foo"],
            vec!["install", "ripgrep"],
            vec!["install", "helloworld-rs", "--root", "out", "--force"],
            vec!["install", "--git", "https://example.com/repo"],
            vec!["install", "--version", "1.0.0", "ripgrep"],
            vec!["install", "--", "--path", "."],
        ] {
            let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
            assert!(!cargo_proxy_passthrough(&args), "{args:?}");
        }
        for args in [
            vec!["install", "--list"],
            vec!["install", "--path", ".", "--help"],
            vec!["install", "--list", "--path", "."],
        ] {
            let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
            assert!(cargo_proxy_passthrough(&args), "{args:?}");
        }
    }

    #[test]
    fn cargo_shim_reuses_an_enclosing_session() {
        assert!(enclosing_session(Some(OsStr::new("session-socket"))));
        assert!(!enclosing_session(Some(OsStr::new(""))));
        assert!(!enclosing_session(None));
    }

    #[test]
    fn cargo_shim_identification_and_disable_values_are_explicit() {
        assert!(is_cargo_shim_path(OsStr::new("/tmp/cargo")));
        assert!(is_cargo_shim_path(OsStr::new("cargo.exe")));
        assert!(!is_cargo_shim_path(OsStr::new("/tmp/mbx")));
        assert!(mbx_disabled_value(Some(OsStr::new("1"))));
        assert!(!mbx_disabled_value(Some(OsStr::new("0"))));
        assert!(!mbx_disabled_value(Some(OsStr::new("false"))));
        assert!(!mbx_disabled_value(None));
    }

    #[cfg(windows)]
    #[test]
    fn cargo_shim_paths_are_case_insensitive_on_windows() {
        assert!(is_cargo_shim_path(OsStr::new(r"C:\Tools\CARGO.EXE")));
        assert!(same_path(
            Path::new(r"C:\Users\Example\mbx\bin"),
            Path::new(r"c:\users\example\MBX\BIN")
        ));
    }

    #[test]
    fn cargo_resolution_excludes_the_shim_directory_and_prevents_recursion() {
        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&shim_dir, &real_dir]).unwrap();

        assert_eq!(
            resolve_real_cargo_from(&shim, Some(shim.as_os_str()), &path).unwrap(),
            real.into_os_string()
        );
    }

    #[test]
    fn cargo_resolution_uses_the_active_shim_when_the_configured_home_changed() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("new-home/bin/cargo");
        let shim_dir = directory.path().join("original-home/bin");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&shim_dir, &real_dir]).unwrap();

        let active = resolve_active_cargo_shim(&configured, None, Some(&path));
        assert_eq!(active, shim);
        let path = path_without_shim(&active, &path).unwrap();
        assert_eq!(
            resolve_real_cargo_from(&active, None, &path).unwrap(),
            real.into_os_string()
        );
    }

    #[cfg(windows)]
    #[test]
    fn reported_windows_shim_wins_when_real_cargo_precedes_it_on_path() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("new-home/bin/cargo.exe");
        let shim_dir = directory.path().join("shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let shim = shim_dir.join("cargo.exe");
        let real = real_dir.join("cargo.exe");
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&real_dir, &shim_dir]).unwrap();

        let active = resolve_active_cargo_shim(&configured, Some(shim.as_os_str()), Some(&path));
        assert_eq!(active, shim);
        let path = path_without_shim(&active, &path).unwrap();
        assert_eq!(
            resolve_real_cargo_from(&active, None, &path).unwrap(),
            real.into_os_string()
        );
    }

    #[test]
    fn configured_real_cargo_does_not_need_a_path_search() {
        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();

        assert_eq!(
            configured_real_cargo(&shim, Some(real.as_os_str())),
            Some(real.into_os_string())
        );
    }

    #[test]
    fn cargo_resolution_rejects_mise_proxies_as_the_real_cargo() {
        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let wrapper_dir = directory.path().join("command-wrappers/bin");
        let mise_shims_dir = directory.path().join("mise/shims");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        std::fs::create_dir_all(&mise_shims_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let wrapper = wrapper_dir.join(name);
        let mise_shim = mise_shims_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&wrapper, b"wrapper").unwrap();
        std::fs::write(&mise_shim, b"mise shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&wrapper_dir, &mise_shims_dir, &real_dir]).unwrap();

        assert_eq!(
            configured_real_cargo(&shim, Some(wrapper.as_os_str())),
            None
        );
        assert_eq!(
            configured_real_cargo(&shim, Some(mise_shim.as_os_str())),
            None
        );
        assert_eq!(
            resolve_real_cargo_from(&shim, Some(mise_shim.as_os_str()), &path).unwrap(),
            real.into_os_string()
        );
    }

    #[test]
    fn cargo_resolution_removes_the_shim_directory_from_child_path() {
        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let shim = shim_dir.join(name);
        let path = std::env::join_paths([&shim_dir, &real_dir]).unwrap();

        assert_eq!(
            std::env::split_paths(&path_without_shim(&shim, &path).unwrap()).collect::<Vec<_>>(),
            vec![real_dir]
        );
    }

    #[test]
    fn explicit_cargo_excludes_mise_wrapper_from_child_path() {
        let directory = tempfile::tempdir().unwrap();
        let wrapper_dir = directory.path().join("command-wrappers/bin");
        let shim_dir = directory.path().join("mbx/bin");
        let mise_shims_dir = directory.path().join("mise/shims");
        let tool_dir = directory.path().join("tool/bin");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&mise_shims_dir).unwrap();
        std::fs::create_dir_all(&tool_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
        let wrapper = wrapper_dir.join(name);
        let shim = shim_dir.join(name);
        let mise_shim = mise_shims_dir.join(name);
        let real = real_dir.join(name);
        std::fs::write(&wrapper, b"mise wrapper").unwrap();
        std::fs::write(&shim, b"mbx shim").unwrap();
        std::fs::write(&mise_shim, b"mise shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([
            &wrapper_dir,
            &shim_dir,
            &mise_shims_dir,
            &tool_dir,
            &real_dir,
        ])
        .unwrap();

        let (path, proxy) = path_without_cargo_proxies(&path, Some(&shim)).unwrap();

        assert_eq!(proxy, Some(wrapper.clone()));
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            vec![mise_shims_dir, tool_dir, real_dir]
        );
        assert_eq!(
            resolve_explicit_cargo(&wrapper, Some(&shim), Some(shim.as_os_str()), &path).unwrap(),
            mise_shim.into_os_string()
        );
    }

    #[cfg(unix)]
    #[test]
    fn cargo_resolution_excludes_a_symlink_to_the_shim_directory() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let shim_dir = directory.path().join("shim");
        let shim_link = directory.path().join("linked-shim");
        let real_dir = directory.path().join("real");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::create_dir_all(&real_dir).unwrap();
        symlink(&shim_dir, &shim_link).unwrap();
        let shim = shim_dir.join("cargo");
        let real = real_dir.join("cargo");
        std::fs::write(&shim, b"shim").unwrap();
        std::fs::write(&real, b"real").unwrap();
        let path = std::env::join_paths([&shim_link, &real_dir]).unwrap();

        assert_eq!(
            resolve_real_cargo_from(&shim, None, &path).unwrap(),
            real.into_os_string()
        );
    }

    #[test]
    fn mise_shims_are_recognized_in_configured_and_default_directories() {
        let directory = tempfile::tempdir().unwrap();
        let custom = directory.path().join("tools/bin");
        std::fs::create_dir_all(&custom).unwrap();
        let shim = custom.join("mbx");
        assert!(!is_mise_shim_path_in(&shim, None, None));
        assert!(is_mise_shim_path_in(&shim, Some(custom.as_os_str()), None));

        let data = directory.path().join("data");
        assert!(is_mise_shim_path_in(
            &data.join("shims/mbx"),
            None,
            Some(data.as_os_str())
        ));
        assert!(is_mise_shim_path_in(
            &directory.path().join("mise/shims/mbx"),
            None,
            None
        ));
        assert!(!is_mise_shim_path_in(
            &directory.path().join("other/shims/mbx"),
            None,
            None
        ));
    }
}
