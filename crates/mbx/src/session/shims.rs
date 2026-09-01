#[cfg(unix)]
use super::SHIM_STAGING_NONCE;
use super::{
    PATH_SHIM_NAMES, REAL_CC_ENV, REAL_CXX_ENV, RUSTC_SHIM_STEM, RUSTDOC_SHIM_STEM, is_same_binary,
};
use eyre::{Context, Result};
use log::debug;
use mbx_cache_cc::CcLanguage;
use mbx_cache_core::CacheDigest;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::Ordering;

pub(super) struct CcShims {
    /// Installed C shim and the compiler it stands in for, when there is one.
    pub(super) cc: Option<(PathBuf, PathBuf)>,
    /// The same for C++.
    pub(super) cxx: Option<(PathBuf, PathBuf)>,
    /// Target-specific compilers the build named, each behind its own shim.
    pub(super) targeted: Vec<TargetedCompiler>,
}

/// A compiler a cross build asked for by name, and the shim standing in for it.
pub(super) struct TargetedCompiler {
    /// The `cc` crate variable that named it, such as `CC_aarch64-linux-musl`.
    pub(super) variable: String,
    /// File name of the shim installed for it, which is also its pin key.
    pub(super) shim_name: String,
    /// Absolute path to the shim.
    pub(super) shim: PathBuf,
    /// The compiler the build chose, which the shim execs.
    pub(super) real: PathBuf,
}

impl CcShims {
    /// Point build scripts at whichever shims were installed.
    ///
    /// A language with no compiler on the machine contributes nothing, so a C
    /// build on an image without a C++ compiler still gets its caching.
    pub(super) fn apply_host(&self, environment: &mut BTreeMap<String, String>) {
        for (installed, host, real) in [
            (&self.cc, "HOST_CC", REAL_CC_ENV),
            (&self.cxx, "HOST_CXX", REAL_CXX_ENV),
        ] {
            if let Some((shim, compiler)) = installed {
                environment.insert(host.into(), shim.to_string_lossy().into_owned());
                environment.insert(real.into(), compiler.to_string_lossy().into_owned());
            }
        }
    }

    /// Point each variable that named a cross compiler at its own shim.
    pub(super) fn apply_targeted(&self, environment: &mut BTreeMap<String, String>) {
        for targeted in &self.targeted {
            environment.insert(
                targeted.variable.clone(),
                targeted.shim.to_string_lossy().into_owned(),
            );
        }
    }

    /// Pins for the shims that stand in for a named cross compiler.
    ///
    /// They share the map the standalone shims use, which is what lets one
    /// shim binary serve several compilers: it looks itself up by the name it
    /// was invoked under.
    pub(super) fn pins(&self) -> BTreeMap<String, PathBuf> {
        self.targeted
            .iter()
            .map(|targeted| (targeted.shim_name.clone(), targeted.real.clone()))
            .collect()
    }
}

/// Whether an environment variable is how the `cc` crate names a compiler for
/// a particular target.
///
/// `TARGET_CC` and `TARGET_CXX` apply to whatever the build is cross-compiling
/// for; `CC_<target>` and `CXX_<target>` name one triple outright.
pub(super) fn targeted_compiler_language(variable: &str) -> Option<CcLanguage> {
    match variable {
        "TARGET_CC" => return Some(CcLanguage::C),
        "TARGET_CXX" => return Some(CcLanguage::Cxx),
        _ => {}
    }
    // `CXX_` first: `CC_` is not a prefix of it, but reading it the other way
    // round invites the mistake.
    for (prefix, language) in [("CXX_", CcLanguage::Cxx), ("CC_", CcLanguage::C)] {
        if let Some(target) = variable.strip_prefix(prefix)
            && is_target_triple(target)
        {
            return Some(language);
        }
    }
    None
}

/// Whether a variable suffix spells a target triple rather than something else
/// that happens to start with `CC_`.
///
/// The `cc` crate hangs its own controls off that prefix -- `CC_FORCE_DISABLE`,
/// `CC_KNOWN_WRAPPER_CUSTOM`, `CC_ENABLE_DEBUG_OUTPUT` -- and autotools adds
/// `CC_FOR_BUILD`. Redirecting one of those would not miss a cache hit, it
/// would answer a question the build asked with a compiler path.
///
/// Case is what separates them: a target triple is lowercase and those knobs
/// are not. A triple also always has at least two components, which a bare word
/// does not.
pub(super) fn is_target_triple(suffix: &str) -> bool {
    !suffix.is_empty()
        && suffix.contains(['-', '_'])
        && suffix.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

/// Variables the `cc` crate consults before falling back to the platform
/// default. A build that sets any of them has chosen its own compiler, and mbx
/// stands aside rather than redirecting it.
pub(super) const CC_CRATE_ENV: &[&str] = &["CC", "CXX", "HOST_CC", "HOST_CXX"];

/// Install the C and C++ shims, resolving the compilers they will run.
///
/// Resolution happens here rather than in the shim so the whole build agrees on
/// one compiler, and so a machine with no C compiler simply gets no shims
/// instead of a build script that fails differently than it would have.
pub(super) fn install_cc_shims(shims_dir: &Path) -> Result<Option<CcShims>> {
    if !cfg!(any(unix, windows)) {
        return Ok(None);
    }
    let executable = std::env::current_exe().wrap_err("failed to locate the running mbx binary")?;
    // Each language stands alone. An image with a C compiler and no C++ one is
    // ordinary, and it must not cost a C-only sys-crate its caching.
    let real_cc = resolve_on_path(CcLanguage::C.default_driver());
    let real_cxx = resolve_on_path(CcLanguage::Cxx.default_driver());
    // Wrapped first, because a cross image is entitled to ship the driver it
    // cross-compiles with and no host `cc` at all. Deciding there is nothing to
    // do before looking would leave exactly that build uncached.
    std::fs::create_dir_all(shims_dir)?;
    let targeted = wrap_targeted_compilers(&executable, shims_dir)?;
    if real_cc.is_none() && real_cxx.is_none() && targeted.is_empty() {
        debug!("no C or C++ compiler was found on PATH; build script compiles are not cached");
        return Ok(None);
    }
    let shim = |language: CcLanguage| -> Result<PathBuf> {
        let destination = shims_dir.join(shim_file_name(language.shim_stem()));
        link_path_shim(&executable, &destination)?;
        Ok(destination)
    };
    let mut installed = CcShims {
        cc: None,
        cxx: None,
        targeted,
    };
    if let Some(real) = real_cc {
        installed.cc = Some((shim(CcLanguage::C)?, real));
    }
    if let Some(real) = real_cxx {
        installed.cxx = Some((shim(CcLanguage::Cxx)?, real));
    }
    Ok(Some(installed))
}

/// Put a shim in front of every cross compiler the build named for itself.
///
/// Deriving one is not an option: which compiler a target implies lives in the
/// `cc` crate's own tables, and guessing wrong would not cost a cache hit, it
/// would build the object with the wrong compiler. So only a compiler the build
/// asked for by name is wrapped, and only when the name resolves to a single
/// executable -- a value like `ccache gcc` is a command, not a path, and is
/// left alone.
fn wrap_targeted_compilers(executable: &Path, shims_dir: &Path) -> Result<Vec<TargetedCompiler>> {
    let mut wrapped = Vec::new();
    for (variable, value) in std::env::vars() {
        let Some(language) = targeted_compiler_language(&variable) else {
            continue;
        };
        let Some(real) = resolve_named_compiler(&value, executable, shims_dir) else {
            debug!("{variable} does not name a single executable; it is left as it is");
            continue;
        };
        // Named for the variable so one shim binary can serve several
        // compilers, telling them apart by the name it was invoked under.
        let shim_name = format!(
            "{}-{}",
            language.shim_stem(),
            variable.to_ascii_lowercase().replace(['.', '/'], "_")
        );
        let shim = shims_dir.join(shim_file_name(&shim_name));
        link_path_shim(executable, &shim)?;
        wrapped.push(TargetedCompiler {
            variable,
            shim_name,
            shim,
            real,
        });
    }
    wrapped.sort_by(|left, right| left.variable.cmp(&right.variable));
    Ok(wrapped)
}

/// Resolve a compiler a build named, rejecting anything that is not one path.
pub(super) fn resolve_named_compiler(
    value: &str,
    executable: &Path,
    shims: &Path,
) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() || value.split_whitespace().count() != 1 {
        return None;
    }
    let candidate = Path::new(value);
    let resolved = if candidate.is_absolute() {
        candidate.is_file().then(|| candidate.to_path_buf())
    } else {
        resolve_on_path_excluding(value, executable, shims)
    }?;
    // A build already pointed at a shim -- this session's, or one an outer
    // session left in the environment -- would otherwise be wrapped a second
    // time, and the inner shim would exec itself. Comparing the directory as
    // well as the binary catches the link that does not compare equal.
    let inside_shims = resolved
        .parent()
        .is_some_and(|parent| canonical(parent) == canonical(shims));
    (!inside_shims && !is_same_binary(&resolved, Some(executable))).then_some(resolved)
}

pub(super) fn resolve_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// A directory of compiler-named shims for builds that find their compiler on
/// `PATH`, and the compilers those shims stand in for.
pub struct PathShims {
    pub directory: PathBuf,
    pub compilers: BTreeMap<String, PathBuf>,
}

/// Install a shim under every plain compiler name that resolves on `PATH`.
///
/// Resolution happens here rather than in the shim so the whole build agrees
/// on one compiler per name, and skips the running binary so a shim directory
/// already on `PATH` cannot become its own compiler. A machine with none of
/// the names simply gets no shim directory.
///
/// `directory` outlives the session on purpose. A configure step records the
/// compiler it found by absolute path -- CMake writes it into `CMakeCache.txt`,
/// autoconf into the generated makefiles -- and a session-local directory would
/// leave that build permanently naming a path that no longer exists. A stable
/// one keeps the recorded path resolvable, and keeps a later `cmake --build`
/// running through the cache rather than around it. Nothing is added to any
/// `PATH` but the one handed to a single `mbx exec` command.
pub fn install_path_shims(directory: &Path) -> Result<Option<PathShims>> {
    if !cfg!(any(unix, windows)) {
        return Ok(None);
    }
    let executable = std::env::current_exe().wrap_err("failed to locate the running mbx binary")?;
    std::fs::create_dir_all(directory)?;
    let mut compilers = BTreeMap::new();
    for (name, _) in PATH_SHIM_NAMES {
        let destination = directory.join(name);
        let Some(real) = resolve_on_path_excluding(name, &executable, directory) else {
            continue;
        };
        // Belt and braces for the recursion the exclusion above prevents: a
        // shim that stood in for itself would exec itself forever, so leave
        // the name uncached rather than install that.
        if canonical(&real) == canonical(&destination) {
            debug!("{name} resolved to its own shim; leaving it uncached");
            continue;
        }
        link_path_shim(&executable, &destination)?;
        compilers.insert((*name).to_string(), real);
    }
    if compilers.is_empty() {
        debug!("no C or C++ compilers were found on PATH; nothing to shim");
        return Ok(None);
    }
    Ok(Some(PathShims {
        directory: directory.to_path_buf(),
        compilers,
    }))
}

/// Point one compiler name at this binary, replacing a stale link in place.
///
/// A symlink rather than a hard link so an upgraded mbx is picked up without
/// reinstalling, and because on macOS a hard link taken while the binary is
/// replaced can be killed at exec.
///
/// The replacement goes through a temporary name and a rename because this
/// directory is shared: another build may be executing the very name being
/// replaced, and `rename` leaves it resolvable at every instant where removing
/// and recreating would not.
#[cfg(unix)]
fn link_path_shim(executable: &Path, destination: &Path) -> Result<()> {
    // Absolutized for the same reason [`symlink_shim`] does it: a symlink is
    // resolved from the directory holding it, which here is the cache's shim
    // directory and never the caller's. `current_exe` has been absolute on
    // every platform mbx runs on, but its contract does not promise one, and a
    // relative target would point inside the shim directory itself.
    let target = std::path::absolute(executable)?;
    if std::fs::read_link(destination).is_ok_and(|existing| existing == target) {
        return Ok(());
    }
    let staging = destination.with_file_name(format!(
        ".{}.{}.{}",
        destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        std::process::id(),
        SHIM_STAGING_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&staging);
    std::os::unix::fs::symlink(&target, &staging)?;
    std::fs::rename(&staging, destination)
        .wrap_err_with(|| format!("failed to install the shim {}", destination.display()))?;
    Ok(())
}

#[cfg(windows)]
fn link_path_shim(executable: &Path, destination: &Path) -> Result<()> {
    // Windows cannot replace an executable while another process has it open,
    // so a stable existing shim is preferable to a racy in-place upgrade. A
    // missing shim is pinned to this binary by hard link, with copy fallback.
    if destination.is_file() {
        return Ok(());
    }
    if let Err(link_error) = std::fs::hard_link(executable, destination) {
        std::fs::copy(executable, destination).wrap_err_with(|| {
            format!(
                "failed to install the shim {} by hard link ({link_error}) or copy",
                destination.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn link_path_shim(_executable: &Path, _destination: &Path) -> Result<()> {
    eyre::bail!("PATH shims are not supported on this platform")
}

/// Find the real compiler `name` refers to, never a shim.
///
/// The shim directory is skipped by location, not by identity. Identity alone
/// is not enough: a nested `mbx exec` sees that directory first on `PATH`, and
/// a shim there may point at a *different* mbx binary -- an upgrade, or a build
/// from another checkout -- which no inode comparison against the running one
/// can recognize. Choosing it would pin a shim as the compiler and then relink
/// it to the running binary, leaving it its own compiler and recursing until
/// the process table fills. Nothing mbx puts in that directory is ever a real
/// compiler, so the directory itself is the honest thing to exclude.
///
/// The identity check stays for the rest of `PATH`, where a copy of mbx under a
/// compiler's name can still turn up outside any directory mbx owns.
fn resolve_on_path_excluding(name: &str, this_binary: &Path, shims: &Path) -> Option<PathBuf> {
    resolve_in_path(&std::env::var_os("PATH")?, name, this_binary, shims)
}

/// The search itself, over a `PATH` the caller supplies.
///
/// Separated from the environment so a test can hand it one: `PATH` is process
/// global, and cargo runs these tests on a thread pool, so setting it here
/// would race every other test that reads one -- `which` in the linker tests
/// among them.
pub(super) fn resolve_in_path(
    path: &OsStr,
    name: &str,
    this_binary: &Path,
    shims: &Path,
) -> Option<PathBuf> {
    let shims = canonical(shims);
    std::env::split_paths(path)
        .filter(|directory| canonical(directory) != shims)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file() && !is_same_binary(candidate, Some(this_binary)))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn install_session_shims(
    session_dir: &Path,
    persistent_shims: &Path,
) -> Result<(PathBuf, PathBuf)> {
    let executable = std::env::current_exe().wrap_err("failed to locate the running mbx binary")?;
    let binary_shims = binary_shims_dir(persistent_shims, &executable)?;
    std::fs::create_dir_all(&binary_shims)?;
    let rustc = binary_shims.join(shim_file_name(RUSTC_SHIM_STEM));
    link_path_shim(&executable, &rustc)?;
    let rustdoc = install_shim_named(
        &executable,
        session_dir,
        RUSTDOC_SHIM_STEM,
        ShimLink::Tracking,
    )?;
    Ok((rustc, rustdoc))
}

/// A stable shim directory for exactly one installed mbx executable.
///
/// Cargo keys its cached rustc probes by the wrapper path. A session-local
/// wrapper makes every invocation look like a different compiler, while a
/// single machine-wide name would conceal upgrades. The executable's path and
/// file identity give repeated runs of one binary the same name and a replaced
/// binary a new one without reading the whole executable at startup.
fn binary_shims_dir(shims: &Path, executable: &Path) -> Result<PathBuf> {
    let executable = std::path::absolute(executable)?;
    let metadata = std::fs::metadata(&executable)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    let mut identity = executable.as_os_str().as_encoded_bytes().to_vec();
    identity.push(0);
    identity.extend_from_slice(&metadata.len().to_le_bytes());
    identity.extend_from_slice(
        &modified
            .map_or(0, |duration| duration.as_secs())
            .to_le_bytes(),
    );
    identity.extend_from_slice(
        &modified
            .map_or(0, |duration| duration.subsec_nanos())
            .to_le_bytes(),
    );
    let digest = CacheDigest::blake3(&identity);
    Ok(shims.join("rust").join(digest.hash))
}

/// How an installed shim refers to the mbx binary behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShimLink {
    /// A symlink: the shim is whatever binary that path holds when it runs.
    Tracking,
    /// A hard link, or a copy where the filesystem cannot link: the bytes that
    /// were there on the day the shim was installed.
    Pinned,
}

/// File name a shim with this stem is installed under.
pub fn shim_file_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.into()
    }
}

/// Install a rustc shim into `directory` as a link to `executable`.
///
/// The shim must be the same binary as the agent -- the handshake requires an
/// exact version match -- so it is a link to the running binary rather than an
/// independently built copy.
///
/// Which kind of link matters more than it looks. On macOS, exec of a path that
/// `link(2)` created moments ago is intermittently killed outright: SIGKILL, no
/// output, no crash report, nothing in the system log. Measured on macOS 26.6
/// (Apple silicon) at about one exec in eight hundred under heavy parallel
/// load, against a binary nothing was writing, and far more readily for a large
/// image -- the 50 MB debug build of mbx died where a 500 KB one never did,
/// which is the shape a race in per-page signature validation would have. The
/// window closes about half a second after the link appears, and the same path
/// then runs fine forever. Exec of the original path never fails, and neither
/// does exec of a symlink to it -- the kernel resolves that to a file whose
/// signature it validated long ago. Reading the new link through first,
/// fsyncing it and its directory, and taking the first exec ourselves to spend
/// the race were all tried, and none of them helped.
///
/// [`ShimLink::Tracking`] is used for shims that should follow a replaced mbx
/// binary, including the session shim and the persistent Cargo shim on Unix.
/// [`ShimLink::Pinned`] remains available for callers that need the installed
/// bytes to survive deletion of the original path.
///
/// The one thing tracking gives up: replace the mbx binary underneath a running
/// build and the session shim follows it, so cargo either execs nothing (the
/// path is gone, and the build stops with a plain error) or execs a version the
/// agent will not shake hands with, which bypasses the cache for the rest of the
/// build. Both are loud and self-inflicted, unlike the kill they replace.
pub fn install_shim(executable: &Path, directory: &Path, link: ShimLink) -> Result<PathBuf> {
    install_shim_named(executable, directory, RUSTC_SHIM_STEM, link)
}

/// Install a shim for one compiler into `directory` as a link to `executable`.
///
/// Every shim is the same binary under a different name; the name is what tells
/// it which compiler it stands in for. Everything [`install_shim`] documents
/// about which kind of link is used applies here too.
pub fn install_shim_named(
    executable: &Path,
    directory: &Path,
    stem: &str,
    link: ShimLink,
) -> Result<PathBuf> {
    let shim = directory.join(shim_file_name(stem));
    let _ = std::fs::remove_file(&shim);
    if link == ShimLink::Tracking && symlink_shim(executable, &shim) {
        return Ok(shim);
    }
    if let Err(link_error) = std::fs::hard_link(executable, &shim) {
        std::fs::copy(executable, &shim).wrap_err_with(|| {
            format!("failed to install the {stem} shim by hard link ({link_error}) or copy")
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        // Some filesystems do not retain executable permissions when the
        // cross-device fallback copies the running binary. Cargo must be able
        // to invoke the installed wrapper directly.
        let mut permissions = std::fs::metadata(&shim)?.permissions();
        permissions.set_mode(permissions.mode() | 0o100);
        std::fs::set_permissions(&shim, permissions)?;
    }
    Ok(shim)
}

/// Point `shim` at `executable` by symlink, reporting whether that worked.
///
/// A failure is not an error: the hard link the caller falls back to is what
/// every shim was before, so the only thing lost is the race described on
/// [`install_shim`].
///
/// The target is absolutized first, because the two kinds of link do not read a
/// relative one the same way: `link(2)` and `copy` resolve it from the caller's
/// working directory, while a symlink resolves it from the shim's own directory
/// -- a temporary one that shares nothing with the caller's. Resolving it here
/// gives the argument one meaning. Absolutizing rather than declining, because a
/// relative target must still get a symlink: `current_exe()` has been absolute
/// on every platform mbx runs on, but nothing in its contract promises that, and
/// a shim that quietly stopped tracking would put the kill back.
///
/// A target that is not there gets no symlink at all. A hard link and a copy
/// both refuse one, and a symlink would instead name it and leave cargo to
/// discover the dangling wrapper mid-build.
#[cfg(unix)]
pub(super) fn symlink_shim(executable: &Path, shim: &Path) -> bool {
    let Ok(target) = std::path::absolute(executable) else {
        return false;
    };
    target.exists() && std::os::unix::fs::symlink(&target, shim).is_ok()
}

/// Windows has no shim symlinks: creating one needs a privilege ordinary
/// accounts lack, and the code-signature race they exist to avoid is a macOS
/// kernel behaviour with no Windows counterpart.
#[cfg(windows)]
pub(super) fn symlink_shim(_executable: &Path, _shim: &Path) -> bool {
    false
}
