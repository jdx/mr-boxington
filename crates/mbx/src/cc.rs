//! The C and C++ shim: identity, lookup, compilation, and publication.
//!
//! This mirrors [`crate::rustc`] with one structural difference. Cargo leaves a
//! dep-info file in the target directory that the rustc shim can read *before*
//! deciding what to compile, so that adapter has two ways to build a key. A C
//! compile leaves nothing behind -- the dependency list this adapter asks for
//! is its own, and publishing it would add a file the uncached build never
//! produced. So a warm lookup here is always prediction-driven: the invocation
//! fingerprint finds the inputs the last identical compile read, and those are
//! rehashed to rebuild the full key.

use crate::materialize::{
    CachedCompilation, CachedOutput, Materialization, StagedOutputs, denormalize_output_text,
    executable_mode_matches, exit_code, file_mode, find_blobs, normalize_output_text,
    persist_outputs, read_canonical_blob, read_verified_blob, record_action_hit,
    record_verification, replay_bytes, resolve_executable, stage_verified_cached_output,
    staging_directory, validate_file_mode,
};
use crate::session;
use eyre::{Context, Result, bail};
use mbx_cache_cc::{
    CcAction, CcActionContext, CcBypassReason, CcCompilerFamily, CcCompilerIdentity, CcDepfile,
    CcDiscoveredInputs, CcInputPrediction, CcInvocation, CcLanguage, environment_inputs_for,
    is_system_path, manifest_snapshot,
};
use mbx_cache_core::{
    ActionPrediction, AgentRequest, AgentResponse, CacheDigest, CacheDirectory, CacheFileNode,
    CcMetadata, FileDigestScope, FileIdentity, PathMapping, RecordedFileDigest, RemoteActionResult,
    RestoreStats, canonical_json, normalize_mapped_path,
};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};
use std::time::{Instant, SystemTime};

const ADAPTER: &str = "cc";

/// Compile one C or C++ translation unit, consulting the cache around it.
///
/// An `Err` is a bypass: the caller runs the real compiler transparently. Only
/// a successful compile is ever published, so a compiler error always reaches
/// the build exactly as it would have without mbx.
pub fn compile(compiler: &OsStr, arguments: &[OsString], language: CcLanguage) -> Result<ExitCode> {
    let working_dir = std::env::current_dir()?;
    let mappings = path_mappings(&working_dir);
    let identity = compiler_identity(compiler, language)?;
    // Appended before anything parses, so the flag is part of the key the same
    // way the caller's own prefix maps are.
    let portable = Portable::detect(&mappings, identity.family);
    let arguments = portable.applied_to(arguments);
    let arguments = arguments.as_ref();
    let invocation = CcInvocation::parse_for(arguments, identity.family)?;
    let environment = environment_inputs_for(
        |name| std::env::var(name).ok(),
        invocation.sysroot(),
        identity.family,
    )?;
    let mut context = CcActionContext {
        compiler: identity,
        working_dir: working_dir.clone(),
        path_mappings: mappings,
        environment,
        inputs: Vec::new(),
    };

    let verify = session::verify_requested();
    let invocation_digest = invocation.invocation_digest(&context)?;
    let task = prediction_task(&invocation_digest);
    let mut verification = None;
    // A prediction that no longer describes the tree -- a header it named has
    // been deleted, say -- is not an error, it is simply no longer usable. It
    // must not abort the cache path: this adapter has no second way to build a
    // key, so bypassing here would leave the compilation permanently uncached,
    // with the same stale prediction failing the same way on every later build.
    // Falling through compiles and republishes, which replaces it.
    let usable = find_prediction(&task, &invocation_digest)?.and_then(|prediction| {
        prediction
            .discover(
                &working_dir,
                &context.path_mappings,
                session::file_digest_cache(),
            )
            .ok()
    });
    // Whether an action lookup actually ran. A cold compilation has no
    // prediction to build a key from, so nothing was ever asked of the cache --
    // which the summary and the TUI report separately from a lookup that missed.
    let mut looked_up = false;
    if let Some(discovered) = usable {
        let mut candidate = context.clone();
        discovered.clone().apply_to(&mut candidate)?;
        let action = invocation.action(candidate)?;
        looked_up = true;
        // A restore that fails is a miss, not a bypass. Bypassing would leave
        // the compilation uncached and publish nothing, so a partial or corrupt
        // entry would fail the same way on every later build; compiling and
        // republishing is what repairs it.
        let restored = match restore_result(
            &action,
            &invocation,
            &discovered,
            !verify,
            &context.path_mappings,
        ) {
            Ok(restored) => restored,
            Err(error) => {
                session::report_shim_warning(&format!("cc result was not restored: {error:#}"));
                None
            }
        };
        if let Some(cached) = restored {
            if !verify {
                replay_bytes(&cached.stdout, &cached.stderr)?;
                record_action_hit(
                    &action.digest,
                    cached.restore,
                    &compilation_name(&invocation),
                );
                return Ok(ExitCode::SUCCESS);
            }
            verification = Some(cached);
        }
    }

    // Everything past this point would run the real compiler, so this is
    // where machine-wide coordination starts. The flight comes before the
    // permit: another build compiling this exact invocation right now is
    // waited for rather than duplicated, and the prediction a finished
    // flight left behind is one more chance to restore. Never in verify
    // mode, whose whole point is running the compiler.
    let flight = if verify {
        None
    } else {
        crate::scheduler::flight(ADAPTER, &invocation_digest.hash)
    };
    if let Some(flight) = &flight
        // Only when the payload can say something this session's own lookup
        // has not already said: either we waited and it is freshly
        // published, or nothing was ever looked up and it is the first
        // prediction we have.
        && (flight.waited() || !looked_up)
        && let Some(payload) = flight.inherited()
    {
        match restore_flight_prediction(
            payload,
            &invocation,
            &context,
            &task,
            &invocation_digest,
            &mut looked_up,
        ) {
            Ok(Some((action, cached))) => {
                replay_bytes(&cached.stdout, &cached.stderr)?;
                record_action_hit(&action, cached.restore, &compilation_name(&invocation));
                return Ok(ExitCode::SUCCESS);
            }
            Ok(None) => {}
            Err(error) => {
                session::report_shim_warning(&format!(
                    "a flight prediction was not restored: {error:#}"
                ));
            }
        }
    }
    // Recorded here rather than where the prediction came up empty, because
    // a flight can still turn such a compilation into a real lookup.
    if !looked_up {
        session::record_unconsulted();
    }

    let depfile_staging = staging_directory()?;
    let depfile = depfile_staging.path().join("compile.d");
    // Taken before the compiler runs: the manifest that lands in the key is
    // computed afterwards, and comparing the two is what stops a header that
    // appeared mid-compile from being recorded as one the compiler had seen.
    let searchable = searchable_directories(&invocation, &working_dir);
    let before = manifest_snapshot(&searchable).ok();
    // The machine-wide permit is taken after every chance to hit the cache,
    // and the timer starts afterwards: time spent waiting for the machine is
    // not time this compilation cost.
    let demand = crate::scheduler::Demand::new(&compilation_name(&invocation), false);
    let permit = crate::scheduler::pool().and_then(|pool| pool.admit(&demand));
    let started = Instant::now();
    let compilation_started = SystemTime::now();
    let mut command = Command::new(compiler);
    command.args(arguments);
    command.args(invocation.dependency_arguments(&depfile, context.compiler.family));
    let output = command
        .output()
        .wrap_err_with(|| format!("failed to run {}", Path::new(compiler).display()))?;
    drop(permit);
    crate::scheduler::record_compiler_memory(&demand, &output.status);
    let duration_ns = duration_ns(started.elapsed());
    session::record_compiler_invocation(
        if verification.is_some() {
            "verification"
        } else if looked_up {
            "miss"
        } else {
            "unconsulted"
        },
        Some(&compilation_name(&invocation)),
        duration_ns,
    );

    if let Some(cached) = verification {
        let divergence = verification_divergence(&cached, &output);
        record_verification(divergence.is_none(), cached.restore);
        if let Some(divergence) = divergence {
            session::report_shim_warning(&format!(
                "shadow verification diverged from cached output: {divergence}"
            ));
        }
    }

    replay_bytes(&output.stdout, &output.stderr)?;
    if !output.status.success() {
        return Ok(exit_code(output.status));
    }

    // A failure to publish must not fail a compilation that already succeeded.
    if let Err(error) = publish(
        &invocation,
        &mut context,
        &depfile,
        &output,
        compilation_started,
        &task,
        &invocation_digest,
        duration_ns,
        &searchable,
        before,
        flight.as_ref(),
        &portable,
    ) {
        #[cfg(debug_assertions)]
        session::report_shim_warning(&format!("cc result was not published: {error:#}"));
        #[cfg(not(debug_assertions))]
        let _ = error;
    }
    Ok(exit_code(output.status))
}

/// Digest the inputs the compiler reported, build the key, and store the
/// object under it.
#[allow(clippy::too_many_arguments)]
fn publish(
    invocation: &CcInvocation,
    context: &mut CcActionContext,
    depfile: &Path,
    output: &Output,
    compilation_started: SystemTime,
    task: &str,
    invocation_digest: &CacheDigest,
    duration_ns: u64,
    searchable: &BTreeSet<PathBuf>,
    before: Option<BTreeMap<PathBuf, CacheDigest>>,
    flight: Option<&crate::scheduler::Flight>,
    portable: &Portable,
) -> Result<()> {
    let discovered = discover(invocation, context, depfile)?;
    discovered.verify_not_modified_since(compilation_started)?;
    verify_search_path_unchanged(searchable, before)?;
    discovered.verify()?;
    discovered.apply_to(context)?;
    // The key says this object does not depend on where the build script put
    // its headers. Publishing one that names the directory anyway would make
    // that claim false for every checkout that restored it, so a compilation
    // whose output kept the value is left uncached rather than shared wrong.
    if !portable.outputs_are_clean(invocation.output()) {
        return Err(CcBypassReason::UnportableOutput(invocation.output().to_path_buf()).into());
    }
    let action = invocation.action(context.clone())?;
    publish_result(&action, invocation, output, &context.path_mappings)?;
    let prediction = invocation.prediction(context, duration_ns)?;
    record_prediction(task, invocation_digest, &action.digest, &prediction, flight);
    Ok(())
}

/// Restore through the prediction a flight left behind.
///
/// The payload gets exactly the treatment a manifest prediction does -- its
/// inputs rehashed, the key rebuilt, the store consulted -- so the worst a
/// stale or foreign record can do is miss. A restore that works is recorded
/// into this session's own manifest, which is how the next build finds it
/// without a flight.
fn restore_flight_prediction(
    payload: &str,
    invocation: &CcInvocation,
    context: &CcActionContext,
    task: &str,
    invocation_digest: &CacheDigest,
    looked_up: &mut bool,
) -> Result<Option<(CacheDigest, CachedCompilation)>> {
    let prediction: CcInputPrediction = serde_json::from_str(payload)?;
    let discovered = prediction.discover(
        &context.working_dir,
        &context.path_mappings,
        session::file_digest_cache(),
    )?;
    let mut candidate = context.clone();
    discovered.clone().apply_to(&mut candidate)?;
    let action = invocation.action(candidate)?;
    *looked_up = true;
    let restored = restore_result(
        &action,
        invocation,
        &discovered,
        true,
        &context.path_mappings,
    )?;
    let Some(cached) = restored else {
        return Ok(None);
    };
    record_prediction(task, invocation_digest, &action.digest, &prediction, None);
    Ok(Some((action.digest, cached)))
}

/// The environment values whose absolute paths a compilation is made
/// independent of, and the check that says it really was.
///
/// This is the C and C++ half of what [`crate::rustc::Portable`] does, and it
/// exists for the same reason. `OUT_DIR` differs per checkout and per target
/// directory, a build script that generates headers there passes it as an
/// include directory, and the compiler records that directory in the debug
/// information of every object it produces. The key already normalizes those
/// paths, so without this the cache serves an object naming a directory it
/// was not built in -- equivalent, but not the artifact this compilation
/// would have written, and every qualification run says so once per object.
///
/// Two things must hold, exactly as they must for rustc.
/// `-fdebug-prefix-map` makes the compiler record the placeholder instead of
/// the real path, covering the debug information it writes itself. It does
/// not cover a path the source keeps as a string, so the object is read
/// before publishing and a compilation whose output still carries the value
/// is not published at all.
///
/// `-fdebug-prefix-map` rather than `-ffile-prefix-map`: the latter also
/// rewrites `__FILE__`, which a C program can print or assert on, and
/// changing what a program says at runtime is not this cache's business.
struct Portable {
    /// Flags appended to the real compiler invocation, one per value.
    arguments: Vec<OsString>,
    /// The literal values, for the check before publishing.
    values: Vec<String>,
}

impl Portable {
    fn detect(mappings: &[PathMapping], family: CcCompilerFamily) -> Self {
        let mut portable = Self {
            arguments: Vec::new(),
            values: Vec::new(),
        };
        if !session::share_out_dir_requested() {
            return portable;
        }
        for name in PORTABLE_ENVIRONMENT {
            let Some(value) = std::env::var(name)
                .ok()
                .filter(|value| Path::new(value).is_absolute())
            else {
                continue;
            };
            // A value under no known root is one no key could agree on
            // anyway, so there is nothing to remap and nothing to promise.
            let Ok(placeholder) =
                normalize_mapped_path(Path::new(&value), Path::new("/"), mappings)
            else {
                continue;
            };
            let mut flag = OsString::from(if family == CcCompilerFamily::Msvc {
                "/pathmap:"
            } else {
                "-fdebug-prefix-map="
            });
            flag.push(&value);
            flag.push("=");
            flag.push(&placeholder);
            portable.arguments.push(flag);
            portable.values.push(value);
        }
        portable
    }

    fn applied_to<'a>(&self, arguments: &'a [OsString]) -> Cow<'a, [OsString]> {
        if self.arguments.is_empty() {
            return Cow::Borrowed(arguments);
        }
        let mut applied = arguments.to_vec();
        applied.extend(self.arguments.iter().cloned());
        Cow::Owned(applied)
    }

    /// Whether an output is free of every value the flags normalized away.
    fn outputs_are_clean(&self, path: &Path) -> bool {
        if self.values.is_empty() {
            return true;
        }
        let Ok(contents) = std::fs::read(path) else {
            // Unreadable is not evidence of cleanliness.
            return false;
        };
        !self.values.iter().any(|value| {
            memchr::memmem::find(&contents, value.as_bytes()).is_some()
                || (value.contains('\\')
                    && memchr::memmem::find(&contents, value.replace('\\', "/").as_bytes())
                        .is_some())
        })
    }
}

/// The environment values a C or C++ compilation may be made independent of.
///
/// `OUT_DIR` alone, for the same reason the rustc adapter names only that one:
/// it is what a build script hands its own compilations, and what differs
/// between two checkouts and two target directories.
const PORTABLE_ENVIRONMENT: &[&str] = &["OUT_DIR"];

/// Read the dependency list and turn it into digested inputs.
fn discover(
    invocation: &CcInvocation,
    context: &CcActionContext,
    depfile: &Path,
) -> Result<CcDiscoveredInputs> {
    let dependencies = CcDepfile::read_for(depfile, context.compiler.family)?;
    let mut files = BTreeSet::new();
    for path in dependencies
        .files
        .into_iter()
        .chain(invocation.required_inputs().iter().map(ToOwned::to_owned))
    {
        files.insert(absolute(&path, &context.working_dir));
    }
    Ok(CcDiscoveredInputs::collect(
        &context.working_dir,
        files.clone(),
        manifest_directories(invocation, context, &files),
        session::file_digest_cache(),
    )?)
}

/// Directories a shadowing header could appear in that are known before the
/// compiler runs.
///
/// The include search path and the source's own directory are named on the
/// command line, so they can be snapshotted up front. A directory reached only
/// through a header that includes its neighbour is not known until the
/// dependency list names it, and is covered by the digests of the files that
/// were actually read.
fn searchable_directories(invocation: &CcInvocation, working_dir: &Path) -> BTreeSet<PathBuf> {
    invocation
        .include_dirs()
        .iter()
        .map(|directory| absolute(directory, working_dir))
        .chain(
            invocation
                .source()
                .parent()
                .map(|parent| absolute(parent, working_dir)),
        )
        .filter(|directory| !is_system_include_path(directory))
        .collect()
}

/// Reject a compilation whose include search path shifted underneath it.
fn verify_search_path_unchanged(
    searchable: &BTreeSet<PathBuf>,
    before: Option<BTreeMap<PathBuf, CacheDigest>>,
) -> Result<()> {
    // A snapshot that could not be taken is not evidence of a change; the
    // directory walk failing is already reported when discovery repeats it.
    let Some(before) = before else {
        return Ok(());
    };
    let after = manifest_snapshot(searchable)?;
    for (directory, digest) in &before {
        if after.get(directory) != Some(digest) {
            return Err(
                CcBypassReason::SearchPathModifiedDuringCompilation(directory.clone()).into(),
            );
        }
    }
    Ok(())
}

/// Directories whose contents could change which file a future include
/// resolves to.
///
/// Every search directory named on the command line qualifies, and so does the
/// directory each discovered header actually came from -- a quoted include
/// searches the includer's own directory, which never appears in argv. System
/// roots are left out: enumerating an SDK per compile costs more than the
/// residual risk, and anything actually read from one is digested anyway.
fn manifest_directories(
    invocation: &CcInvocation,
    context: &CcActionContext,
    files: &BTreeSet<PathBuf>,
) -> BTreeSet<PathBuf> {
    invocation
        .include_dirs()
        .iter()
        .map(|directory| absolute(directory, &context.working_dir))
        .chain(
            files
                .iter()
                .filter_map(|file| file.parent().map(Path::to_path_buf)),
        )
        .filter(|directory| !is_system_include_path(directory))
        .collect()
}

fn absolute(path: &Path, working_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    }
}

/// Roots the key replaces with placeholders.
///
/// Deliberately the same roots the rustc shim maps, and read from the same
/// session variables, so both adapters agree on which paths belong to a
/// checkout rather than to the machine.
fn path_mappings(working_dir: &Path) -> Vec<PathMapping> {
    let mut mappings = Vec::new();
    let mut add = |root: Option<PathBuf>, placeholder: &str| {
        if let Some(root) = root.filter(|root| root.is_absolute())
            && !mappings
                .iter()
                .any(|existing: &PathMapping| existing.root == root)
        {
            mappings.push(PathMapping::new(root, placeholder));
        }
    };
    add(session_path(session::TARGET_DIR_ENV), "target");
    add(session_path(session::WORKSPACE_ROOT_ENV), "workspace");
    add(
        std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".cargo"))),
        "cargo_home",
    );
    add(
        std::env::var_os("RUSTUP_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".rustup"))),
        "rustup_home",
    );
    add(dirs::home_dir(), "home");
    add(std::env::var_os("VCINSTALLDIR").map(PathBuf::from), "msvc");
    add(
        std::env::var_os("WindowsSdkDir").map(PathBuf::from),
        "windows_sdk",
    );
    add(
        std::env::var_os("UniversalCRTSdkDir").map(PathBuf::from),
        "ucrt_sdk",
    );
    let _ = working_dir;
    mappings
}

fn is_system_include_path(path: &Path) -> bool {
    is_system_path(path)
        || ["VCINSTALLDIR", "WindowsSdkDir", "UniversalCRTSdkDir"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .any(|root| path.starts_with(root))
}

fn rustc_path_mappings(mappings: &[PathMapping]) -> Vec<mbx_cache_rustc::PathMapping> {
    mappings
        .iter()
        .map(|mapping| mbx_cache_rustc::PathMapping::new(&mapping.root, &mapping.placeholder))
        .collect()
}

fn session_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Fingerprint the driver, and for gcc the assembler it hands objects to.
///
/// The probe output is memoized through the agent, because a build script may
/// compile hundreds of translation units and each one would otherwise re-run
/// the compiler just to ask its version.
fn compiler_identity(compiler: &OsStr, language: CcLanguage) -> Result<CcCompilerIdentity> {
    let executable = resolve_executable(compiler)?;
    let is_cl = executable
        .file_stem()
        .and_then(OsStr::to_str)
        .is_some_and(|stem| stem.eq_ignore_ascii_case("cl"));
    let probe = if is_cl {
        probe_msvc(&executable)?
    } else {
        probe_executable(&executable, &["-v"], &executable)?
    };
    let family = CcCompilerFamily::classify(&probe)?;
    let target = if family == CcCompilerFamily::Msvc {
        std::env::var("VSCMD_ARG_TGT_ARCH").unwrap_or_default()
    } else {
        probe
            .lines()
            .find_map(|line| line.strip_prefix("Target: "))
            .unwrap_or_default()
            .to_string()
    };
    let assembler = if family.uses_external_assembler() {
        assembler_identity(&executable)
    } else {
        String::new()
    };
    let _ = language;
    Ok(CcCompilerIdentity {
        family,
        version_text: probe,
        target,
        assembler,
    })
}

/// MSVC prints its detailed `/Bv` identity before complaining that no source
/// was supplied. That non-zero exit is a property of the query shape, not a
/// failed identity probe, so accept it only when the expected banner is there.
fn probe_msvc(executable: &Path) -> Result<String> {
    let environment = BTreeMap::new();
    if let Ok(responses) = session::request_agent(&[AgentRequest::FindExecutableIdentity {
        executable: executable.to_path_buf(),
        environment: environment.clone(),
    }]) && let Some(AgentResponse::ExecutableIdentity { stdout: Some(text) }) =
        responses.into_iter().next()
    {
        return Ok(String::from_utf8_lossy(&text).into_owned());
    }
    let output = Command::new(executable)
        .arg("/Bv")
        .output()
        .map_err(|error| CcBypassReason::CompilerIdentityUnavailable(error.to_string()))?;
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    if !text.contains("Microsoft (R) C/C++ Optimizing Compiler") {
        return Err(CcBypassReason::CompilerIdentityUnavailable(format!(
            "{} did not report an MSVC compiler identity",
            executable.display()
        ))
        .into());
    }
    let _ = session::request_agent(&[AgentRequest::StoreExecutableIdentity {
        executable: executable.to_path_buf(),
        environment,
        stdout: text.clone().into_bytes(),
    }]);
    Ok(text)
}

/// The assembler path a driver named, if it named one at all.
///
/// `-print-prog-name` echoes the bare name back when the driver cannot resolve
/// the tool, which is not a path and means the driver would search `PATH` for
/// it -- so the caller does the same.
fn named_assembler(printed: &str) -> Option<PathBuf> {
    let trimmed = printed.trim();
    Path::new(trimmed)
        .is_absolute()
        .then(|| PathBuf::from(trimmed))
}

/// The assembler gcc will hand objects to.
///
/// Its version changes object bytes without changing anything `gcc -v` prints,
/// so it belongs in the identity.
///
/// The driver is asked which assembler it will run rather than the first `as`
/// on `PATH`: gcc resolves the tool through its own exec prefix and whatever it
/// was configured with, so a toolchain shipping its own binutils runs one
/// assembler while `PATH` names another. Keying the wrong one would let two
/// toolchains share an entry whose object bytes they do not agree on.
///
/// An assembler that cannot be resolved yields a marker rather than a bypass:
/// the compile still happens, and the key is simply less specific than it could
/// have been -- which every machine in that state shares.
fn assembler_identity(compiler: &Path) -> String {
    // A driver that cannot resolve the tool echoes the bare name back, and then
    // searching PATH is exactly what it does itself.
    let named = probe_executable(compiler, &["-print-prog-name=as"], &compiler.join("as"))
        .unwrap_or_default();
    let assembler = match named_assembler(&named) {
        Some(assembler) => assembler,
        None => match resolve_executable(OsStr::new("as")) {
            Ok(found) => found,
            Err(_) => return "unresolved".into(),
        },
    };
    let version = probe_executable(&assembler, &["--version"], &assembler)
        .ok()
        .and_then(|probe| probe.lines().next().map(ToOwned::to_owned))
        .unwrap_or_default();
    format!("{}; {version}", assembler.display())
}

/// Run a probe once per session, memoized by the agent.
///
/// `memo` is the key the answer is stored under. The agent keys an executable
/// identity by path alone, so two probes of the same binary would otherwise
/// return each other's output; a caller asking a compiler something other than
/// its version passes a key that cannot collide with a real path.
fn probe_executable(executable: &Path, arguments: &[&str], memo: &Path) -> Result<String> {
    let key = memo.to_path_buf();
    // No environment variable changes what a C driver prints for these, so the
    // memo needs nothing beyond the key.
    let environment = BTreeMap::new();
    let responses = session::request_agent(&[AgentRequest::FindExecutableIdentity {
        executable: key.clone(),
        environment: environment.clone(),
    }]);
    if let Ok(responses) = responses
        && let Some(AgentResponse::ExecutableIdentity { stdout: Some(text) }) =
            responses.into_iter().next()
    {
        return Ok(String::from_utf8_lossy(&text).into_owned());
    }
    let output = Command::new(executable)
        .args(arguments)
        .output()
        .map_err(|error| CcBypassReason::CompilerIdentityUnavailable(error.to_string()))?;
    if !output.status.success() {
        return Err(CcBypassReason::CompilerIdentityUnavailable(format!(
            "{} exited with {}",
            executable.display(),
            output.status
        ))
        .into());
    }
    // Drivers print their banner on stderr; clang and gcc both do.
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stdout).into_owned();
    }
    let _ = session::request_agent(&[AgentRequest::StoreExecutableIdentity {
        executable: key,
        environment,
        stdout: text.clone().into_bytes(),
    }]);
    Ok(text)
}

fn find_prediction(task: &str, invocation: &CacheDigest) -> Result<Option<CcInputPrediction>> {
    let responses = session::request_agent(&[AgentRequest::FindActionPrediction {
        task: task.to_string(),
        invocation: invocation.clone(),
    }])?;
    let Some(AgentResponse::ActionPrediction { prediction }) = responses.into_iter().next() else {
        return Ok(None);
    };
    let Some(prediction) = prediction else {
        return Ok(None);
    };
    if prediction.adapter != ADAPTER {
        return Ok(None);
    }
    let payload: CcInputPrediction = serde_json::from_str(&prediction.payload)?;
    Ok(Some(payload))
}

fn record_prediction(
    task: &str,
    invocation: &CacheDigest,
    action: &CacheDigest,
    prediction: &CcInputPrediction,
    flight: Option<&crate::scheduler::Flight>,
) {
    let Ok(payload) = serde_json::to_string(prediction) else {
        return;
    };
    // Anyone waiting on this flight -- and any later build of the same
    // invocation -- restores through this instead of compiling.
    if let Some(flight) = flight {
        flight.leave(&payload);
    }
    let _ = session::request_agent(&[AgentRequest::RecordActionPrediction {
        task: task.to_string(),
        prediction: ActionPrediction {
            invocation: invocation.clone(),
            action: action.clone(),
            adapter: ADAPTER.into(),
            payload,
        },
    }]);
}

/// Task identity for a compilation's predictions.
///
/// Inside a build this is the session's own run, the same manifest the rustc
/// shim records into: it is the one the session loads before the build and
/// commits after it, so a prediction written by one checkout is there to be
/// found by the next. A shim running outside a session falls back to sharding
/// by the invocation fingerprint, which keeps each manifest bounded.
fn prediction_task(invocation: &CacheDigest) -> String {
    std::env::var(session::BUILD_ENV).unwrap_or_else(|_| standalone_prediction_task(invocation))
}

/// Shard predictions by invocation fingerprint, which is what a shim outside a
/// session has to fall back to. A single manifest would eventually reach the
/// protocol's prediction limit.
fn standalone_prediction_task(invocation: &CacheDigest) -> String {
    let shard = invocation.hash.get(..2).unwrap_or(&invocation.hash);
    CacheDigest::blake3(format!("cc-standalone-predictions-v1\0{shard}").as_bytes()).hash
}

fn restore_result(
    action: &CcAction,
    invocation: &CcInvocation,
    discovered: &CcDiscoveredInputs,
    restore_outputs: bool,
    mappings: &[PathMapping],
) -> Result<Option<CachedCompilation>> {
    let text_mappings = rustc_path_mappings(mappings);
    let responses = session::request_agent(&[AgentRequest::FindActionResult {
        action: action.digest.clone(),
    }])?;
    let Some(response) = responses.into_iter().next() else {
        bail!("cache agent did not return an action lookup response");
    };
    let result = match response {
        AgentResponse::ActionResult { result } => match result {
            Some(result) => result,
            None => return Ok(None),
        },
        AgentResponse::Error { message } => bail!(message),
        _ => bail!("cache agent returned an unexpected action lookup response"),
    };
    if result.version != 1 || result.action != action.digest {
        bail!("cached cc action result has an invalid identity");
    }
    let metadata_digest = result
        .metadata
        .ok_or_else(|| eyre::eyre!("cached cc action result has no metadata"))?;
    let output_root_digest = result
        .output_root
        .ok_or_else(|| eyre::eyre!("cached cc action result has no output root"))?;
    let roots = find_blobs(&[
        action.digest.clone(),
        metadata_digest.clone(),
        output_root_digest.clone(),
    ])?;
    let cached_action = read_verified_blob(&roots[0], &action.digest, "action descriptor")?;
    if cached_action != action.bytes {
        bail!("cached cc action descriptor does not match the invocation");
    }
    let metadata: CcMetadata = read_canonical_blob(&roots[1], &metadata_digest, "cc metadata")?;
    if !metadata.validate() {
        bail!("cached cc metadata is unsupported");
    }
    let directory: CacheDirectory =
        read_canonical_blob(&roots[2], &output_root_digest, "output directory")?;
    let node = validated_object(directory, invocation)?;
    let destination = invocation.output().to_path_buf();

    let blobs = find_blobs(&[
        metadata.stdout.clone(),
        metadata.stderr.clone(),
        node.digest.clone(),
    ])?;
    // A compiler diagnostic names the file it is about, so a warning stored by
    // one checkout would otherwise be replayed in another pointing at paths
    // that belong to the checkout that published it.
    let stdout = denormalize_output_text(
        &read_verified_blob(&blobs[0], &metadata.stdout, "stdout")?,
        &text_mappings,
    );
    let stderr = denormalize_output_text(
        &read_verified_blob(&blobs[1], &metadata.stderr, "stderr")?,
        &text_mappings,
    );

    let materialization_started = Instant::now();
    let parent = destination
        .parent()
        .ok_or_else(|| eyre::eyre!("cc output has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let mut restore = RestoreStats {
        output_files: 1,
        output_bytes: node.digest.size,
        ..RestoreStats::default()
    };
    // An object already holding these bytes is kept, not rewritten: the
    // rewrite would only refresh its modification time, which is what keeps
    // downstream freshness checks re-dirtying whatever reads it.
    let mut staged_files = Vec::new();
    if restore_outputs
        && crate::rustc::output_already_in_place(&node, &destination, session::file_digest_cache())
    {
        restore.reused_output_files = 1;
        restore.reused_output_bytes = node.digest.size;
    } else {
        let (temporary, materialization) =
            stage_verified_cached_output(staging.path(), 0, &blobs[2], &node)?;
        match materialization {
            Materialization::Reflink => {
                restore.reflinked_output_files = 1;
                restore.reflinked_output_bytes = node.digest.size;
            }
            Materialization::Copy => {
                restore.copied_output_files = 1;
                restore.copied_output_bytes = node.digest.size;
            }
        }
        staged_files.push((temporary, destination.clone()));
    }
    let staged = StagedOutputs {
        directory: staging,
        files: staged_files,
    };

    // Re-check the inputs after staging: a header rewritten while the lookup
    // was in flight must not be answered from the key it no longer matches.
    discovered.verify()?;
    if restore_outputs {
        persist_outputs(staged)?;
        // The restored object is what a later native link hashes as an input,
        // so its digest enters the content ledger the moment its identity is
        // fixed by the rename.
        if let Ok(metadata) = std::fs::metadata(&destination)
            && metadata.len() == node.digest.size
            && let Some(file) = FileIdentity::describe(&destination, &metadata)
        {
            session::record_file_digests(
                FileDigestScope::Content,
                vec![RecordedFileDigest {
                    file,
                    digest: node.digest.clone(),
                }],
            );
        }
    }
    restore.duration_ns = duration_ns(materialization_started.elapsed());
    Ok(Some(CachedCompilation {
        stdout,
        stderr,
        outputs: vec![CachedOutput {
            path: destination,
            digest: node.digest,
            executable: node.executable,
            mode: node.mode,
        }],
        restore,
    }))
}

/// Confirm the cached directory holds exactly the one object this compile
/// produces.
fn validated_object(directory: CacheDirectory, invocation: &CcInvocation) -> Result<CacheFileNode> {
    if directory.version != 1 || !directory.directories.is_empty() || !directory.symlinks.is_empty()
    {
        bail!("cached cc output directory has unsupported entries");
    }
    let name = invocation
        .output()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| eyre::eyre!("cc output name is not UTF-8"))?;
    let [node] = <[CacheFileNode; 1]>::try_from(directory.files)
        .map_err(|_| eyre::eyre!("cached cc output set does not match the invocation"))?;
    if node.name != name {
        bail!("cached cc output is unexpected: {}", node.name);
    }
    validate_file_mode(&node, false)?;
    Ok(node)
}

fn publish_result(
    action: &CcAction,
    invocation: &CcInvocation,
    output: &Output,
    mappings: &[PathMapping],
) -> Result<()> {
    let text_mappings = rustc_path_mappings(mappings);
    let object = invocation.output();
    let metadata = std::fs::metadata(object)
        .wrap_err_with(|| format!("failed to inspect cc output {}", object.display()))?;
    if !metadata.is_file() {
        bail!("cc output is not a regular file: {}", object.display());
    }
    let staging = staging_directory()?;
    let mut blobs = vec![staged_bytes(staging.path(), "action.json", &action.bytes)?];
    let stdout = staged_bytes(
        staging.path(),
        "stdout",
        &normalize_output_text(&output.stdout, &text_mappings),
    )?;
    let stderr = staged_bytes(
        staging.path(),
        "stderr",
        &normalize_output_text(&output.stderr, &text_mappings),
    )?;
    blobs.extend([stdout.clone(), stderr.clone()]);

    let digest = CacheDigest::blake3_file(object)?;
    blobs.push((digest.clone(), object.to_path_buf()));
    // A freshly compiled object is a future native-link input; the hash just
    // taken is the read a ledger entry stands in for.
    if metadata.len() == digest.size
        && let Some(file) = FileIdentity::describe(object, &metadata)
    {
        session::record_file_digests(
            FileDigestScope::Content,
            vec![RecordedFileDigest {
                file,
                digest: digest.clone(),
            }],
        );
    }
    let files = vec![CacheFileNode {
        digest,
        // An object file is never executable, which is what makes the mode
        // model here trivial compared with the rustc adapter's.
        executable: false,
        mode: file_mode(&metadata),
        name: object
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| eyre::eyre!("cc output name is not UTF-8"))?
            .to_string(),
    }];

    let metadata_bytes = canonical_json(&CcMetadata {
        version: 1,
        kind: ADAPTER.into(),
        stdout: stdout.0,
        stderr: stderr.0,
    })?;
    let metadata = staged_bytes(staging.path(), "metadata.json", &metadata_bytes)?;
    blobs.push(metadata.clone());
    let directory_bytes = canonical_json(&CacheDirectory {
        directories: Vec::new(),
        files,
        symlinks: Vec::new(),
        version: 1,
    })?;
    let directory = staged_bytes(staging.path(), "directory.json", &directory_bytes)?;
    blobs.push(directory.clone());

    let mut requests = Vec::new();
    let mut published = BTreeSet::new();
    for (digest, source) in blobs {
        if published.insert(digest.clone()) {
            requests.push(AgentRequest::StoreBlob { digest, source });
        }
    }
    requests.push(AgentRequest::StoreActionResult {
        result: RemoteActionResult {
            action: action.digest.clone(),
            metadata: Some(metadata.0),
            output_root: Some(directory.0),
            version: 1,
        },
    });
    for response in session::request_agent(&requests)? {
        match response {
            AgentResponse::Stored { .. } | AgentResponse::ActionStored { .. } => {}
            AgentResponse::Error { message } => bail!(message),
            _ => bail!("cache agent returned an unexpected publish response"),
        }
    }
    Ok(())
}

fn staged_bytes(directory: &Path, name: &str, bytes: &[u8]) -> Result<(CacheDigest, PathBuf)> {
    let path = directory.join(name);
    std::fs::write(&path, bytes)?;
    Ok((CacheDigest::blake3(bytes), path))
}

/// Why a shadow compilation disagreed with the cached result, if it did.
///
/// Said in words rather than as a bare `false`, the way the rustc adapter
/// says it. A qualification run reports a count, and a count with no reason
/// attached is not something anybody can act on: every divergence looks
/// alike in the log, so a systematic one and a real modeling bug read the
/// same.
fn verification_divergence(cached: &CachedCompilation, output: &Output) -> Option<String> {
    if !output.status.success() {
        return Some("the shadow compilation failed".into());
    }
    if cached.stdout != output.stdout {
        return Some("standard output differs".into());
    }
    if cached.stderr != output.stderr {
        return Some("standard error differs".into());
    }
    for expected in &cached.outputs {
        let name = expected.path.display();
        let Ok(metadata) = std::fs::metadata(&expected.path) else {
            return Some(format!("{name} is missing"));
        };
        if file_mode(&metadata) != expected.mode {
            return Some(format!("{name} has a different file mode"));
        }
        if !executable_mode_matches(&metadata, expected.executable) {
            return Some(format!("{name} has a different executable bit"));
        }
        if !expected
            .digest
            .matches_file(&expected.path)
            .unwrap_or(false)
        {
            return Some(format!("{name} has different contents"));
        }
    }
    None
}

/// Name this compilation goes by in build statistics.
///
/// Prefixed so a C source cannot be mistaken for a crate in the same table.
fn compilation_name(invocation: &CcInvocation) -> String {
    format!("{ADAPTER}:{}", invocation.source_name())
}

fn duration_ns(duration: std::time::Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "cc_tests.rs"]
mod tests;
