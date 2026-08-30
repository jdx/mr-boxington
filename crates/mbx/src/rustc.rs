use crate::materialize::{
    CachedCompilation, CachedOutput, Materialization, StagedOutputs, apply_file_mode,
    denormalize_output_text, executable_mode_matches, exit_code, file_mode, find_blobs,
    normalize_output_text, persist_outputs, read_canonical_blob, read_verified_blob,
    record_action_hit, record_verification, replay_bytes, resolve_executable,
    stage_verified_cached_output, staging_directory, validate_file_mode,
};
use crate::{session, util::workspace_root};
use eyre::{Context, Result, bail};
use mbx_cache_core::{
    ActionPrediction, AgentRequest, AgentResponse, CacheDigest, CacheDirectory, CacheFileNode,
    FileDigestScope, FileIdentity, RecordedFileDigest, RemoteActionResult, RestoreStats,
    RustcMetadata, canonical_json,
};
use mbx_cache_rustc::{
    ActionContext, BypassReason, CompilerIdentity, DiscoveredInputs, LinkerIdentity, ParseOptions,
    PathMapping, RustcAction, RustcDepInfo, RustcInputPrediction, RustcInvocation, RustcOutputs,
    normalize_mapped_path,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime};

#[derive(Clone, Debug, Default)]
struct CompileTiming {
    crate_name: String,
    duration_ns: u64,
}

/// Consecutive misses with changed content before a unit compiles incrementally.
///
/// One changed key is an edit; a run of them is a developer working here. The
/// threshold is what separates the two, and it is small because the cost of
/// guessing wrong is one uncached compilation the unit was going to pay anyway.
const HOT_STREAK_THRESHOLD: u32 = 3;

/// Schema version of the per-checkout churn record.
const CHURN_STATE_VERSION: u8 = 1;

/// How large one unit's incremental state may grow before it is discarded.
const INCREMENTAL_DIR_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// What a churning unit gets: its own incremental state, and no publication.
#[derive(Clone, Debug, Default)]
struct LearnedPlan {
    /// Whether this unit has churned often enough to keep incremental state.
    hot: bool,
    /// Consecutive changed-content misses, including this one.
    streak: u32,
    /// Where that state lives, once somewhere to put it has been resolved.
    directory: Option<PathBuf>,
    /// Where to record what this compilation compiled, and what that was.
    record: Option<(PathBuf, CacheDigest)>,
}

impl LearnedPlan {
    /// Find somewhere to keep the state, for a unit that earned it.
    fn resolved(mut self, invocation: &CacheDigest) -> Self {
        if self.hot {
            self.directory = incremental_directory(invocation);
        }
        self
    }

    /// Record what this crate compiled, now that it has.
    ///
    /// Deliberately after the compiler succeeds rather than before it runs. A
    /// failed compilation leaves nothing behind to compare against, so
    /// recording its sources would make the retry that follows -- with nothing
    /// edited in between -- look like a crate that had settled, and drop it
    /// back to compiling from scratch.
    fn record_compiled(&self) {
        let Some((path, sources)) = &self.record else {
            return;
        };
        if let Err(error) = write_churn_state(path, sources, self.streak) {
            eprintln!("mbx[warning]: churn was not recorded for this crate: {error:#}");
        }
    }

    /// Whether this compilation actually carries incremental state. A hot unit
    /// with nowhere to keep it compiles and publishes like any other miss.
    fn engaged(&self) -> bool {
        self.directory.is_some()
    }
}

/// What one checkout last compiled for one unit, and how long its sources have
/// been moving.
///
/// Kept beside the incremental state it decides, in this checkout's target
/// directory, rather than in the prediction manifest: a manifest is shared by
/// every worktree resolving the same lockfile, so a streak recorded there would
/// let one developer's edit loop mark a crate hot for a sibling worktree that
/// is merely building it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChurnState {
    /// Version of this record's schema.
    version: u8,
    /// Digest of the sources compiled here last time.
    sources: String,
    /// Consecutive compilations whose sources had changed.
    streak: u32,
}

/// What every step of one compilation needs to identify it.
struct Compilation<'a> {
    rustc: &'a OsStr,
    invocation: &'a RustcInvocation,
    working_dir: &'a Path,
    portable: &'a Portable,
    /// Identity of the linker, for an invocation whose key must describe it.
    linker: Option<LinkerIdentity>,
}

pub(crate) fn compile(rustc: &OsStr, arguments: &[OsString]) -> Result<ExitCode> {
    let working_dir = std::env::current_dir()?;
    // The orchestrated session supplies the target root. A persistent wrapper
    // has no parent session, so first parse just enough of the invocation to
    // learn its output directory and use that as the stable target mapping.
    let options = ParseOptions::caching_native_links(session::cache_links_requested());
    // Appended before anything parses: the debug-map rule inside the parser is
    // exactly what this flag satisfies, so an invocation that would bypass
    // without it has to carry it going in.
    let arguments = with_oso_prefix(arguments, session::cache_links_requested());
    let arguments = arguments.as_ref();
    let initial_invocation = RustcInvocation::parse_with(arguments, options)?;
    let initial_outputs = initial_invocation.outputs(&working_dir)?;
    let portable = Portable::detect(
        &working_dir,
        Some(&initial_outputs.directory),
        initial_invocation.target(),
    );
    let arguments = portable.applied_to(arguments);
    let invocation = RustcInvocation::parse_with(&arguments, options)?;
    let outputs = invocation.outputs(&working_dir)?;

    let verify = session::verify_requested();
    // A shadow compilation compares its result against a cached one, which an
    // incremental artifact would never match, so the two modes are exclusive.
    let learned_enabled = session::learned_incremental_requested() && !verify;
    let mut verification = None;
    let mut action_lookup_attempted = false;
    let mut learned = LearnedPlan::default();
    // Probed once, before anything that would swallow the answer: a host whose
    // linker cannot be described bypasses here, where the reason is recorded,
    // rather than deep inside key construction where it becomes a warning
    // nobody counts.
    let compilation = Compilation {
        rustc,
        invocation: &invocation,
        working_dir: &working_dir,
        portable: &portable,
        linker: linker_for(&invocation)?,
    };
    if outputs.dep_info.is_file()
        && let Ok((candidates, discovered)) =
            action_from_current_dep_info(&compilation, &outputs.dep_info)
    {
        action_lookup_attempted = true;
        match restore_candidates(
            &candidates,
            &outputs,
            &discovered,
            !verify,
            &portable.mappings,
        ) {
            Ok(Some((action, mut cached))) => {
                match refresh_prediction(&compilation, &action, &discovered) {
                    Ok(timing) => {
                        cached.restore.avoided_compiler_duration_ns = timing.duration_ns;
                    }
                    Err(error) => {
                        eprintln!("mbx[warning]: compiler timing was not refreshed: {error:#}");
                    }
                }
                if verify {
                    verification = Some(cached);
                } else {
                    record_action_hit(&action, cached.restore, invocation.crate_name());
                    let _ = replay_bytes(&cached.stdout, &cached.stderr);
                    return Ok(ExitCode::SUCCESS);
                }
            }
            Ok(None) => {
                learned = plan_learned_reuse(&compilation, &discovered, learned_enabled);
            }
            Err(error) => {
                eprintln!("mbx[warning]: result was not restored: {error:#}");
            }
        }
    }
    let mut prediction_missing = false;
    if !action_lookup_attempted {
        match restore_predicted_result(
            &compilation,
            &outputs,
            !verify,
            &mut action_lookup_attempted,
            learned_enabled,
            &mut learned,
        ) {
            Ok(Some(cached)) => {
                if verify {
                    verification = Some(cached);
                } else {
                    let _ = replay_bytes(&cached.stdout, &cached.stderr);
                    return Ok(ExitCode::SUCCESS);
                }
            }
            Ok(None) => {
                prediction_missing = !action_lookup_attempted;
            }
            Err(error) => {
                eprintln!("mbx[warning]: prediction was not restored: {error:#}");
            }
        }
    }

    // Everything past this point would run the real compiler, so this is
    // where machine-wide coordination starts. The flight comes before the
    // permit: if another build is compiling this exact invocation right now,
    // waiting for its result costs less than any amount of capacity -- and
    // waking from that wait, or finding the prediction a finished flight left
    // behind, is one more chance to restore instead of compile. Never in
    // verify mode, whose whole point is running the compiler.
    let flight = if verify {
        None
    } else {
        join_flight(&compilation)
    };
    if let Some(flight) = &flight
        // Only when the payload can say something a lookup this session has
        // not already said: either we waited and it is freshly published, or
        // nothing was ever looked up and it is the first prediction we have.
        && (flight.flight.waited() || !action_lookup_attempted)
        && let Some(payload) = flight.flight.inherited()
    {
        match restore_flight_prediction(
            &compilation,
            &outputs,
            &flight.invocation,
            payload,
            &mut action_lookup_attempted,
            learned_enabled,
            &mut learned,
        ) {
            Ok(Some(cached)) => {
                let _ = replay_bytes(&cached.stdout, &cached.stderr);
                return Ok(ExitCode::SUCCESS);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("mbx[warning]: a flight prediction was not restored: {error:#}");
            }
        }
    }
    // No usable action key, and now no flight prediction either: this
    // compilation runs without an action-result lookup ever being made, which
    // is not a miss and has to be counted as its own thing or the summary
    // reads as though a lookup happened and found nothing. Recorded here
    // rather than where the prediction came up empty, because a flight can
    // still turn such a compilation into a real lookup.
    if prediction_missing && !action_lookup_attempted {
        session::record_unconsulted();
    }
    // The machine-wide permit is taken after every chance to hit the cache,
    // and before anything expensive starts. The timer starts afterwards: time
    // spent waiting for the machine is not time this compilation cost.
    let demand =
        crate::scheduler::Demand::new(invocation.crate_name(), invocation.links_natively());
    let permit = crate::scheduler::pool().and_then(|pool| pool.admit(&demand));
    let compilation_started = SystemTime::now();
    let compiler_timer = Instant::now();
    let mut command = Command::new(rustc);
    command.args(&arguments).current_dir(&working_dir);
    if let Some(directory) = learned.directory.as_deref() {
        // Appended here rather than to the parsed argument vector: the parser
        // treats incremental state as uncacheable and would bypass the whole
        // compilation, which is the opposite of what this is for.
        let mut flag = OsString::from("-Cincremental=");
        flag.push(directory);
        command.arg(flag);
    }
    let output = command.output().wrap_err("failed to execute rustc")?;
    // Released before the outputs are read back and published: hashing and
    // storing cost I/O, not the CPU and memory the permit stands for.
    drop(permit);
    crate::scheduler::record_compiler_memory(&demand, &output.status);
    let timing = CompileTiming {
        crate_name: invocation.crate_name().to_string(),
        duration_ns: compiler_timer
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX),
    };
    session::record_compiler_invocation(
        if verification.is_some() {
            "verification"
        } else if learned.engaged() {
            "incremental"
        } else if action_lookup_attempted {
            "miss"
        } else {
            "unconsulted"
        },
        Some(&timing.crate_name),
        timing.duration_ns,
    );
    let _ = replay_output(&output);
    if let Some(cached) = verification {
        let divergence = verification_divergence(&cached, &output);
        record_verification(divergence.is_none(), cached.restore);
        if let Some(divergence) = divergence {
            eprintln!(
                "mbx[warning]: shadow verification diverged from cached output: {divergence}"
            );
        }
        return Ok(exit_code(output.status));
    }
    if output.status.success() {
        learned.record_compiled();
        let publication: Result<()> = (|| {
            let (candidates, discovered) = action_from_dep_info(&compilation, &outputs.dep_info)?;
            discovered.verify_not_modified_since(compilation_started)?;
            discovered.verify()?;
            // An incremental artifact carries state from this checkout's edit
            // history, so it is recorded as what the unit currently contains --
            // which is how the next build notices the churn ended -- but never
            // published for another checkout to restore. The literal key is
            // enough for that, and it skips reading the outputs back.
            let action = if learned.engaged() {
                &candidates.literal
            } else {
                publish_result(
                    &candidates,
                    &portable,
                    &outputs,
                    &output,
                    &portable.mappings,
                )?
            };
            // The flight prediction is only left behind a *published* result:
            // an incremental artifact was withheld from the store, so a
            // waiter restoring through its key could only miss.
            record_prediction(
                &compilation,
                &action.digest,
                &discovered,
                &timing,
                flight
                    .as_ref()
                    .filter(|_| !learned.engaged())
                    .map(|flight| &flight.flight),
            );
            Ok(())
        })();
        if let Err(error) = publication {
            eprintln!("mbx[warning]: result was not stored: {error:#}");
        }
    }
    Ok(exit_code(output.status))
}

/// Decide whether a missed compilation should carry its own incremental state.
///
/// The comparison is against this crate's own sources, not against its action
/// key: a key also hashes the artifacts the crate links against, so a rebuilt
/// dependency changes it without anybody having touched this crate. Watching
/// the key would drag the whole cone above an edited crate into compiling
/// incrementally and withholding its results, which is the sharing loss this
/// is meant to avoid.
///
/// Unchanged sources are therefore never churn, however the compilation got
/// here. A miss on them means something else lost the result -- a wiped target
/// directory, a first build in this checkout -- and recompiling normally
/// republishes it for everyone.
fn learned_plan(
    recorded: Option<&ChurnState>,
    sources: &CacheDigest,
    enabled: bool,
) -> LearnedPlan {
    let streak = match recorded {
        Some(recorded) if recorded.sources == sources.key() => 0,
        // Capped at the threshold: the streak is a state, not a tally, and
        // letting it climb would only delay noticing that the churn stopped.
        Some(recorded) => recorded.streak.saturating_add(1).min(HOT_STREAK_THRESHOLD),
        None => 0,
    };
    LearnedPlan {
        hot: enabled && streak >= HOT_STREAK_THRESHOLD,
        streak,
        directory: None,
        record: None,
    }
}

/// Where one unit keeps its incremental state.
///
/// It lives under the target directory the session already manages, so a
/// managed target reclaims it with everything else it holds, and `cargo clean`
/// reaches it in a checkout that owns its own. A shim with no session has
/// nowhere of its own to put it, and compiles normally instead.
fn incremental_directory(invocation: &CacheDigest) -> Option<PathBuf> {
    let target_dir = std::env::var_os(session::TARGET_DIR_ENV)?;
    let key = invocation.key();
    let shard = key.get(..16)?;
    let directory = PathBuf::from(target_dir)
        .join("mbx-incremental")
        .join(shard);
    match prepare_incremental_directory(&directory) {
        Ok(()) => Some(directory),
        Err(error) => {
            eprintln!("mbx[warning]: incremental state was not prepared: {error:#}");
            None
        }
    }
}

/// Make sure the unit's state directory exists and is not unbounded.
///
/// Incremental state grows with the edit history rather than the source, so it
/// is discarded wholesale once it is large. Losing it costs one full
/// recompilation, which is what this unit would have paid without it.
fn prepare_incremental_directory(directory: &Path) -> Result<()> {
    if directory_bytes(directory) > INCREMENTAL_DIR_MAX_BYTES {
        std::fs::remove_dir_all(directory)
            .wrap_err("failed to discard oversized incremental state")?;
    }
    std::fs::create_dir_all(directory).wrap_err("failed to create the incremental directory")
}

fn directory_bytes(directory: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(listing) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in listing.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total += metadata.len();
            }
        }
    }
    total
}

/// Work out the plan for a compilation that is about to miss.
///
/// Reads what this checkout last compiled for the unit, decides, and records
/// what it is about to compile now. A checkout with nowhere to keep that -- a
/// shim running without a session -- compiles normally.
fn plan_learned_reuse(
    compilation: &Compilation<'_>,
    discovered: &DiscoveredInputs,
    enabled: bool,
) -> LearnedPlan {
    let planned = (|| {
        let sources = compilation.invocation.source_fingerprint(discovered);
        let context = base_action_context(
            compilation.rustc,
            compilation.working_dir,
            compilation.portable,
        )?;
        let unit = compilation.invocation.invocation_digest(&context)?;
        let Some(state_path) = churn_state_path(&unit) else {
            return Result::<LearnedPlan>::Ok(LearnedPlan::default());
        };
        let plan = learned_plan(read_churn_state(&state_path).as_ref(), &sources, enabled);
        Ok(LearnedPlan {
            record: Some((state_path, sources)),
            ..plan
        }
        .resolved(&unit))
    })();
    match planned {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("mbx[warning]: churn was not tracked for this crate: {error:#}");
            LearnedPlan::default()
        }
    }
}

/// Where this checkout records what it last compiled for one unit.
///
/// A sibling of the unit's incremental directory, so both are reclaimed with
/// the target directory that holds them.
fn churn_state_path(unit: &CacheDigest) -> Option<PathBuf> {
    let target_dir = std::env::var_os(session::TARGET_DIR_ENV)?;
    let key = unit.key();
    let shard = key.get(..16)?;
    Some(
        PathBuf::from(target_dir)
            .join("mbx-incremental")
            .join(format!("{shard}.json")),
    )
}

/// A record this version cannot read is treated as no record: the cost is one
/// compilation that does not count toward a streak.
fn read_churn_state(path: &Path) -> Option<ChurnState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<ChurnState>(&bytes)
        .ok()
        .filter(|state| state.version == CHURN_STATE_VERSION)
}

fn write_churn_state(path: &Path, sources: &CacheDigest, streak: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err("failed to create the incremental state directory")?;
    }
    let state = ChurnState {
        version: CHURN_STATE_VERSION,
        sources: sources.key(),
        streak,
    };
    crate::util::write_atomic(path, &serde_json::to_vec(&state)?)
        .wrap_err("failed to record what this crate last compiled")
}

fn restore_predicted_result(
    compilation: &Compilation<'_>,
    outputs: &RustcOutputs,
    restore_outputs: bool,
    action_lookup_attempted: &mut bool,
    learned_enabled: bool,
    learned: &mut LearnedPlan,
) -> Result<Option<CachedCompilation>> {
    let Compilation {
        invocation,
        working_dir,
        portable,
        ..
    } = compilation;
    let context = base_action_context(compilation.rustc, working_dir, portable)?;
    let invocation_digest = invocation.invocation_digest(&context)?;
    let task = prediction_task(&invocation_digest);
    let responses = session::request_agent(&[AgentRequest::FindActionPrediction {
        task,
        invocation: invocation_digest.clone(),
    }])?;
    let Some(response) = responses.into_iter().next() else {
        bail!("cache agent did not return an action prediction response");
    };
    let prediction = match response {
        AgentResponse::ActionPrediction {
            prediction: Some(prediction),
        } => prediction,
        AgentResponse::ActionPrediction { prediction: None } => {
            // No usable action key: either no dep-info from an earlier build or
            // dep-info that did not yield one, and now no prediction either.
            // Whether that leaves the compilation unconsulted is decided by
            // the caller, after the flight has had its chance to look it up.
            return Ok(None);
        }
        AgentResponse::Error { message } => bail!(message),
        _ => bail!("cache agent returned an unexpected action prediction response"),
    };
    if prediction.adapter != "rustc" || prediction.invocation != invocation_digest {
        bail!("cache agent returned an incompatible rustc action prediction");
    }
    restore_prediction_payload(
        compilation,
        outputs,
        context,
        &invocation_digest,
        &prediction.payload,
        Some(&prediction.action),
        restore_outputs,
        action_lookup_attempted,
        learned_enabled,
        learned,
    )
}

/// Rebuild the action key from one prediction payload and restore through it.
///
/// Shared by the manifest path and the flight path: both hold a payload of
/// predicted inputs, and everything after that -- rehashing them, building
/// the candidates, the lookup itself -- has to be identical, or the two
/// paths would drift in what they accept.
#[allow(clippy::too_many_arguments)]
fn restore_prediction_payload(
    compilation: &Compilation<'_>,
    outputs: &RustcOutputs,
    mut context: ActionContext,
    invocation_digest: &CacheDigest,
    payload: &str,
    recorded_action: Option<&CacheDigest>,
    restore_outputs: bool,
    action_lookup_attempted: &mut bool,
    learned_enabled: bool,
    learned: &mut LearnedPlan,
) -> Result<Option<CachedCompilation>> {
    let Compilation {
        invocation,
        working_dir,
        portable,
        ..
    } = compilation;
    let input_prediction: RustcInputPrediction = serde_json::from_str(payload)?;
    if String::from_utf8(canonical_json(&input_prediction)?)? != payload {
        bail!("the rustc action prediction is not canonical");
    }
    let discovered = input_prediction.discover(
        working_dir,
        &context.path_mappings,
        session::file_digest_cache(),
    )?;
    discovered.clone().apply_to(&mut context)?;
    let candidates = ActionCandidates::build(invocation, context, compilation.linker.clone())?;
    // From this point onward, every return follows at least one action-result
    // request, including error responses from a corrupt local record.
    *action_lookup_attempted = true;
    let restored = restore_candidates(
        &candidates,
        outputs,
        &discovered,
        restore_outputs,
        &portable.mappings,
    )?;
    match restored {
        Some((action, mut cached)) => {
            cached.restore.avoided_compiler_duration_ns = input_prediction.compiler_duration_ns;
            if restore_outputs {
                record_action_hit(&action, cached.restore, invocation.crate_name());
            }
            // The payload being recorded is the one just used, so the only
            // news a rewrite could carry is a different action digest -- the
            // other candidate key hit, or the record came from outside this
            // manifest entirely. An identical record is inherited by the
            // committed manifest without being re-sent.
            if recorded_action != Some(&action) {
                record_prediction_value(
                    invocation_digest.clone(),
                    action,
                    payload.to_string(),
                    invocation.crate_name(),
                );
            }
            Ok(Some(cached))
        }
        None => {
            // A plan an earlier lookup already made carries this checkout's
            // churn record; replacing it would only repeat the work.
            if learned.record.is_none() {
                *learned = plan_learned_reuse(compilation, &discovered, learned_enabled);
            }
            Ok(None)
        }
    }
}

/// The keys one compilation may be published under, most portable first.
///
/// A compilation whose environment holds nothing portable has exactly one key,
/// the literal one, which is what every action looked like before
/// [`Portable`] existed.
struct ActionCandidates {
    /// Normalizes the portable environment values, so two checkouts agree.
    portable: Option<RustcAction>,
    /// What the compilation falls back to when an output carries one of those
    /// values anyway.
    literal: RustcAction,
}

impl ActionCandidates {
    fn build(
        invocation: &RustcInvocation,
        context: ActionContext,
        linker: Option<LinkerIdentity>,
    ) -> Result<Self> {
        // Only worth a second key if a portable name is actually an input here.
        // Crates that never read one keep the key they always had.
        let applies = context
            .portable_environment
            .iter()
            .any(|name| context.environment.contains_key(name));
        let literal_context = ActionContext {
            portable_environment: BTreeSet::new(),
            ..context.clone()
        };
        Ok(Self {
            portable: applies
                .then(|| invocation.action_linked_by(context, linker.clone()))
                .transpose()?,
            literal: invocation.action_linked_by(literal_context, linker)?,
        })
    }

    /// The key this compilation is published under.
    ///
    /// The portable key is only honest if no output carries the value it
    /// normalized away. `--remap-path-prefix` covers the paths rustc records
    /// itself, but not one a crate reads through `env!` and keeps as a string,
    /// and nothing in the inputs distinguishes the two shapes -- so the outputs
    /// are read.
    fn publishable(&self, outputs_are_clean: bool) -> &RustcAction {
        match &self.portable {
            Some(action) if outputs_are_clean => action,
            _ => &self.literal,
        }
    }

    /// Every key to look up, most portable first.
    fn ordered(&self) -> impl Iterator<Item = &RustcAction> {
        self.portable.iter().chain(std::iter::once(&self.literal))
    }
}

/// Try each candidate key, returning the digest that hit alongside its result.
///
/// Both keys are tried because either shape may be on the other side of the
/// lookup: a crate that keeps `OUT_DIR` in a string was published literally,
/// and without the second lookup it would never hit, not even in the checkout
/// that compiled it.
fn restore_candidates(
    candidates: &ActionCandidates,
    outputs: &RustcOutputs,
    discovered: &DiscoveredInputs,
    restore_outputs: bool,
    mappings: &[PathMapping],
) -> Result<Option<(CacheDigest, CachedCompilation)>> {
    for action in candidates.ordered() {
        if let Some(cached) =
            restore_result(action, outputs, discovered, restore_outputs, mappings)?
        {
            return Ok(Some((action.digest.clone(), cached)));
        }
    }
    Ok(None)
}

fn action_from_dep_info(
    compilation: &Compilation<'_>,
    dep_info: &Path,
) -> Result<(ActionCandidates, DiscoveredInputs)> {
    let dep_info = RustcDepInfo::read(dep_info)?;
    action_from_parsed_dep_info(compilation, &dep_info)
}

fn action_from_current_dep_info(
    compilation: &Compilation<'_>,
    dep_info: &Path,
) -> Result<(ActionCandidates, DiscoveredInputs)> {
    let dep_info = RustcDepInfo::read(dep_info)?;
    verify_environment(&dep_info.environment)?;
    action_from_parsed_dep_info(compilation, &dep_info)
}

fn action_from_parsed_dep_info(
    compilation: &Compilation<'_>,
    dep_info: &RustcDepInfo,
) -> Result<(ActionCandidates, DiscoveredInputs)> {
    let Compilation {
        invocation,
        working_dir,
        portable,
        ..
    } = compilation;
    let discovered = invocation.discover_inputs_with_mappings(
        dep_info,
        working_dir,
        &portable.mappings,
        session::file_digest_cache(),
    )?;
    let mut context = base_action_context(compilation.rustc, working_dir, portable)?;
    discovered.clone().apply_to(&mut context)?;
    let candidates = ActionCandidates::build(invocation, context, compilation.linker.clone())?;
    Ok((candidates, discovered))
}

fn base_action_context(
    rustc: &OsStr,
    working_dir: &Path,
    portable: &Portable,
) -> Result<ActionContext> {
    Ok(ActionContext {
        compiler: compiler_identity(rustc)?,
        working_dir: working_dir.to_path_buf(),
        path_mappings: portable.mappings.clone(),
        environment: BTreeMap::new(),
        portable_environment: portable.names.clone(),
        inputs: Vec::new(),
    })
}

/// Identify the linker, for the invocations whose key has to describe it.
///
/// Only a native link needs one, and probing costs several processes on the
/// first one, so nothing else pays for it.
///
/// A host that cannot be described is reported as a bypass rather than as a
/// failure: not knowing the linker is a reason this compilation cannot be
/// cached, which is what a bypass is, and reporting it as anything else leaves
/// it out of the summary and out of `mbx explain`.
/// Ask ld64 to strip this build's output directory from the debug map.
///
/// On macOS a linked binary records the absolute path and timestamp of every
/// object behind it, which is what makes a debug-info link unportable across
/// checkouts. The output directory is where all of them live, and only the
/// shim sees its managed, hashed spelling on every invocation -- so when
/// native links are being cached on this platform, the shim appends the
/// prefix itself rather than asking anyone to discover it. An invocation that
/// already carries one keeps what its caller chose, and an invocation without
/// an output directory has no linker to hand this to.
///
/// Read straight off the argument list, because this runs before anything
/// parses: the parser's own debug-map rule is exactly what the flag
/// satisfies, so it has to be present going in.
fn with_oso_prefix(arguments: &[OsString], cache_links: bool) -> Cow<'_, [OsString]> {
    if !cfg!(target_os = "macos") || !cache_links {
        return Cow::Borrowed(arguments);
    }
    let mut out_dir = None;
    for (index, argument) in arguments.iter().enumerate() {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        if argument.contains("-oso_prefix,") {
            return Cow::Borrowed(arguments);
        }
        // Only a host link is ld64's to read. An explicit `--target` never
        // classifies as one -- wasm links in particular stay cacheable -- and
        // handing those invocations this flag would turn it into the
        // unmodeled link argument that bypasses them.
        if argument == "--target" || argument.starts_with("--target=") {
            return Cow::Borrowed(arguments);
        }
        if argument == "--out-dir" {
            out_dir = arguments.get(index + 1).map(PathBuf::from);
        } else if let Some(value) = argument.strip_prefix("--out-dir=") {
            out_dir = Some(PathBuf::from(value));
        }
    }
    let Some(out_dir) = out_dir.filter(|out_dir| out_dir.is_absolute()) else {
        return Cow::Borrowed(arguments);
    };
    let mut extended = arguments.to_vec();
    extended.push(format!("-Clink-arg=-Wl,-oso_prefix,{}/", out_dir.display()).into());
    Cow::Owned(extended)
}

fn linker_for(invocation: &RustcInvocation) -> Result<Option<LinkerIdentity>> {
    if !invocation.links_natively() {
        return Ok(None);
    }
    match crate::linker::identity_for(invocation.linker_override()) {
        Ok(identity) => Ok(Some(identity)),
        Err(error) => Err(BypassReason::UnportableNativeLink(format!(
            "the linker could not be identified: {error:#}"
        ))
        .into()),
    }
}

fn record_prediction(
    compilation: &Compilation<'_>,
    action: &CacheDigest,
    discovered: &DiscoveredInputs,
    timing: &CompileTiming,
    flight: Option<&crate::scheduler::Flight>,
) {
    let result = (|| {
        let invocation = compilation.invocation;
        let context = base_action_context(
            compilation.rustc,
            compilation.working_dir,
            compilation.portable,
        )?;
        let invocation_digest = invocation.invocation_digest(&context)?;
        let mut prediction = invocation.prediction(&context, discovered)?;
        prediction.version = prediction.version.max(2);
        prediction.compiler_duration_ns = timing.duration_ns;
        prediction.crate_name.clone_from(&timing.crate_name);
        let payload = String::from_utf8(canonical_json(&prediction)?)?;
        // Anyone waiting on this flight -- and any later build of the same
        // invocation -- restores through this instead of compiling.
        if let Some(flight) = flight {
            flight.leave(&payload);
        }
        record_prediction_value(
            invocation_digest,
            action.clone(),
            payload,
            invocation.crate_name(),
        );
        Result::<()>::Ok(())
    })();
    if let Err(error) = result {
        warn_prediction_not_recorded(compilation.invocation.crate_name(), &error);
    }
}

/// The machine-wide flight this compilation occupies, and the invocation
/// digest that keys it.
struct InvocationFlight {
    flight: crate::scheduler::Flight,
    invocation: CacheDigest,
}

/// Join the flight for this invocation, when one can be keyed.
///
/// `None` -- an invocation whose digest cannot be built, or a machine with
/// scheduling off -- just compiles, the way everything here degrades.
fn join_flight(compilation: &Compilation<'_>) -> Option<InvocationFlight> {
    let invocation = (|| -> Result<CacheDigest> {
        let context = base_action_context(
            compilation.rustc,
            compilation.working_dir,
            compilation.portable,
        )?;
        Ok(compilation.invocation.invocation_digest(&context)?)
    })()
    .ok()?;
    let flight = crate::scheduler::flight("rustc", &invocation.hash)?;
    Some(InvocationFlight { flight, invocation })
}

/// Restore through the prediction a flight left behind.
///
/// The payload gets exactly the treatment a manifest prediction does --
/// canonical-form check, every input rehashed, the store consulted -- so the
/// worst a stale or foreign record can do is miss.
fn restore_flight_prediction(
    compilation: &Compilation<'_>,
    outputs: &RustcOutputs,
    invocation_digest: &CacheDigest,
    payload: &str,
    action_lookup_attempted: &mut bool,
    learned_enabled: bool,
    learned: &mut LearnedPlan,
) -> Result<Option<CachedCompilation>> {
    let context = base_action_context(
        compilation.rustc,
        compilation.working_dir,
        compilation.portable,
    )?;
    restore_prediction_payload(
        compilation,
        outputs,
        context,
        invocation_digest,
        payload,
        None,
        true,
        action_lookup_attempted,
        learned_enabled,
        learned,
    )
}

/// Refresh the stored prediction behind a hit and recover the timing it holds.
///
/// One fetch serves both needs: the stored payload carries the compiler time
/// this hit avoided, and comparing it against the freshly built payload says
/// whether anything needs rewriting. On a warm hit nothing does -- the inputs
/// that produced the key are the inputs the prediction already names -- so the
/// rewrite, the largest request a hit sends, is skipped.
fn refresh_prediction(
    compilation: &Compilation<'_>,
    action: &CacheDigest,
    discovered: &DiscoveredInputs,
) -> Result<CompileTiming> {
    let context = base_action_context(
        compilation.rustc,
        compilation.working_dir,
        compilation.portable,
    )?;
    let invocation_digest = compilation.invocation.invocation_digest(&context)?;
    let task = prediction_task(&invocation_digest);
    let responses = session::request_agent(&[AgentRequest::FindActionPrediction {
        task,
        invocation: invocation_digest.clone(),
    }])?;
    let Some(response) = responses.into_iter().next() else {
        bail!("cache agent did not return an action prediction response");
    };
    let stored = match response {
        AgentResponse::ActionPrediction { prediction } => prediction,
        AgentResponse::Error { message } => bail!(message),
        _ => bail!("cache agent returned an unexpected action prediction response"),
    };
    let timing = match &stored {
        Some(stored) => decode_prediction_timing(stored, &invocation_digest)?,
        None => CompileTiming::default(),
    };
    // Recording stays best-effort and separate from the timing, which the
    // caller credits to this hit either way. A prediction that cannot be
    // rebuilt costs the next build a lookup; it does not make the time this
    // build just saved any less real.
    let recorded = (|| {
        let mut prediction = compilation.invocation.prediction(&context, discovered)?;
        prediction.version = prediction.version.max(2);
        prediction.compiler_duration_ns = timing.duration_ns;
        prediction.crate_name.clone_from(&timing.crate_name);
        let payload = String::from_utf8(canonical_json(&prediction)?)?;
        let unchanged = stored
            .as_ref()
            .is_some_and(|stored| stored.action == *action && stored.payload == payload);
        if !unchanged {
            record_prediction_value(
                invocation_digest,
                action.clone(),
                payload,
                compilation.invocation.crate_name(),
            );
        }
        Result::<()>::Ok(())
    })();
    if let Err(error) = recorded {
        warn_prediction_not_recorded(compilation.invocation.crate_name(), &error);
    }
    Ok(timing)
}

fn decode_prediction_timing(
    prediction: &ActionPrediction,
    invocation: &CacheDigest,
) -> Result<CompileTiming> {
    if prediction.adapter != "rustc" || prediction.invocation != *invocation {
        bail!("cache agent returned an incompatible rustc timing prediction");
    }
    let timing: RustcInputPrediction = serde_json::from_str(&prediction.payload)?;
    if !matches!(timing.version, 1..=4)
        || timing.crate_name.len() > 256
        || timing.crate_name.contains(['\0', '\n', '\r'])
        || String::from_utf8(canonical_json(&timing)?)? != prediction.payload
    {
        bail!("cache agent returned an invalid rustc timing prediction");
    }
    Ok(CompileTiming {
        crate_name: timing.crate_name,
        duration_ns: timing.compiler_duration_ns,
    })
}

fn record_prediction_value(
    invocation: CacheDigest,
    action: CacheDigest,
    payload: String,
    crate_name: &str,
) {
    let result = (|| {
        let task = prediction_task(&invocation);
        let responses = session::request_agent(&[AgentRequest::RecordActionPrediction {
            task,
            prediction: ActionPrediction {
                invocation,
                action,
                adapter: "rustc".into(),
                payload,
            },
        }])?;
        match responses.into_iter().next() {
            Some(AgentResponse::ActionPredictionRecorded) => Ok(()),
            Some(AgentResponse::Error { message }) => bail!(message),
            _ => bail!("cache agent returned an unexpected prediction response"),
        }
    })();
    if let Err(error) = result {
        warn_prediction_not_recorded(crate_name, &error);
    }
}

/// Name the crate whose prediction was lost. Without it the warning says only
/// that one compilation out of thousands failed to record, which is not enough
/// to reproduce or to tell whether the same crate fails on every build.
fn warn_prediction_not_recorded(crate_name: &str, error: &eyre::Report) {
    eprintln!("mbx[warning]: action prediction for {crate_name} was not recorded: {error:#}");
}

/// Select the session run, or a bounded persistent-manifest shard when this
/// shim was installed directly in Cargo configuration.
fn prediction_task(invocation: &CacheDigest) -> String {
    std::env::var(session::BUILD_ENV).unwrap_or_else(|_| {
        // A global manifest would eventually hit the prediction count limit.
        // Sharding by the invocation digest keeps related reads and writes
        // together while bounding each manifest independently.
        let shard = invocation.hash.get(..2).unwrap_or(&invocation.hash);
        CacheDigest::blake3(format!("standalone-predictions-v1\0{shard}").as_bytes()).hash
    })
}

fn verify_environment(environment: &BTreeMap<String, Option<String>>) -> Result<()> {
    for (name, expected) in environment {
        let actual = std::env::var_os(name)
            .map(|value| {
                value.into_string().map_err(|_| {
                    eyre::eyre!("compiler environment input is not valid UTF-8: {name}")
                })
            })
            .transpose()?;
        if &actual != expected {
            bail!("compiler environment input changed: {name}");
        }
    }
    Ok(())
}

fn restore_result(
    action: &RustcAction,
    outputs: &RustcOutputs,
    discovered: &DiscoveredInputs,
    restore_outputs: bool,
    mappings: &[PathMapping],
) -> Result<Option<CachedCompilation>> {
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
        bail!("cached rustc action result has an invalid identity");
    }
    let metadata_digest = result
        .metadata
        .ok_or_else(|| eyre::eyre!("cached rustc action result has no metadata"))?;
    let output_root_digest = result
        .output_root
        .ok_or_else(|| eyre::eyre!("cached rustc action result has no output root"))?;
    let roots = find_blobs(&[
        action.digest.clone(),
        metadata_digest.clone(),
        output_root_digest.clone(),
    ])?;
    let cached_action = read_verified_blob(&roots[0], &action.digest, "action descriptor")?;
    if cached_action != action.bytes {
        bail!("cached rustc action descriptor does not match the invocation");
    }
    let metadata: RustcMetadata =
        read_canonical_blob(&roots[1], &metadata_digest, "rustc metadata")?;
    if metadata.version != 1 || metadata.kind != "rustc" {
        bail!("cached rustc metadata is unsupported");
    }
    let directory: CacheDirectory =
        read_canonical_blob(&roots[2], &output_root_digest, "output directory")?;
    let files = validated_outputs(directory, outputs)?;
    let restored_output_files = files.len().try_into().unwrap_or(u64::MAX);
    let restored_output_bytes = files.iter().fold(0_u64, |total, (node, _)| {
        total.saturating_add(node.digest.size)
    });

    let mut digests = vec![metadata.stdout.clone(), metadata.stderr.clone()];
    digests.extend(files.iter().map(|(node, _)| node.digest.clone()));
    let blobs = find_blobs(&digests)?;
    let stdout = denormalize_output_text(
        &read_verified_blob(&blobs[0], &metadata.stdout, "stdout")?,
        mappings,
    );
    let stderr = denormalize_output_text(
        &read_verified_blob(&blobs[1], &metadata.stderr, "stderr")?,
        mappings,
    );

    let materialization_started = Instant::now();
    std::fs::create_dir_all(&outputs.directory)?;
    let staging = tempfile::tempdir_in(&outputs.directory)?;
    let mut staged = Vec::with_capacity(files.len());
    let mut restore = RestoreStats {
        output_files: restored_output_files,
        output_bytes: restored_output_bytes,
        ..RestoreStats::default()
    };
    let mut cached_outputs = Vec::with_capacity(files.len());
    for (index, ((node, destination), source)) in files.into_iter().zip(&blobs[2..]).enumerate() {
        // The dep-info was stored in placeholder form, so it is written out
        // rather than cloned: what belongs on disk is this checkout's spelling
        // of it, and that is what the verification below must compare against.
        if destination == outputs.dep_info {
            let bytes = denormalize_output_text(
                &read_verified_blob(source, &node.digest, "dep-info")?,
                mappings,
            );
            let digest = CacheDigest::blake3(&bytes);
            cached_outputs.push(CachedOutput {
                path: destination.clone(),
                digest: digest.clone(),
                executable: node.executable,
                mode: node.mode,
            });
            // A dep-info already spelling exactly this stays in place for the
            // same reason a matching artifact does below.
            if restore_outputs
                && std::fs::read(&destination).is_ok_and(|existing| existing == bytes)
            {
                restore.reused_output_files = restore.reused_output_files.saturating_add(1);
                restore.reused_output_bytes = restore
                    .reused_output_bytes
                    .saturating_add(bytes.len().try_into().unwrap_or(u64::MAX));
                continue;
            }
            let temporary = staging.path().join(format!("output-{index}"));
            std::fs::write(&temporary, &bytes)?;
            let temporary = tempfile::TempPath::try_from_path(temporary)?;
            apply_file_mode(&temporary, node.mode, node.executable)?;
            restore.copied_output_files = restore.copied_output_files.saturating_add(1);
            restore.copied_output_bytes = restore
                .copied_output_bytes
                .saturating_add(bytes.len().try_into().unwrap_or(u64::MAX));
            staged.push((temporary, destination));
            continue;
        }
        cached_outputs.push(CachedOutput {
            path: destination.clone(),
            digest: node.digest.clone(),
            executable: node.executable,
            mode: node.mode,
        });
        // An output that already holds these bytes is kept, not rewritten.
        // What the rewrite would change is only the modification time, and
        // that change is what makes cargo's next freshness pass re-dirty
        // every dependent of this unit -- a rebuild loop that never settles.
        // Keeping the file settles it: once nothing rewrites, nothing is
        // newer than what depends on it, and the next build is a no-op.
        if restore_outputs
            && output_already_in_place(&node, &destination, session::file_digest_cache())
        {
            restore.reused_output_files = restore.reused_output_files.saturating_add(1);
            restore.reused_output_bytes =
                restore.reused_output_bytes.saturating_add(node.digest.size);
            continue;
        }
        let (temporary, materialization) =
            stage_verified_cached_output(staging.path(), index, source, &node)?;
        match materialization {
            Materialization::Reflink => {
                restore.reflinked_output_files = restore.reflinked_output_files.saturating_add(1);
                restore.reflinked_output_bytes = restore
                    .reflinked_output_bytes
                    .saturating_add(node.digest.size);
            }
            Materialization::Copy => {
                restore.copied_output_files = restore.copied_output_files.saturating_add(1);
                restore.copied_output_bytes =
                    restore.copied_output_bytes.saturating_add(node.digest.size);
            }
        }
        staged.push((temporary, destination));
    }
    let staged = StagedOutputs {
        directory: staging,
        files: staged,
    };

    // The inputs were hashed moments ago in this same process to build the
    // action key, and nothing is published here, so rehashing them
    // (`discovered.verify()`) guards nothing: an input changing mid-restore is
    // the same width of race a real compile has with cargo's own freshness
    // check, and the next build degrades it to a miss. On a warm build that
    // second pass re-reads every source and upstream rlib per hit, which is
    // most of the restore.
    verify_environment(&discovered.environment)?;
    finalize_restored_outputs(staged, restore_outputs)?;
    if restore_outputs {
        record_output_digests(&cached_outputs);
    }
    restore.duration_ns = materialization_started
        .elapsed()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX);
    Ok(Some(CachedCompilation {
        stdout,
        stderr,
        outputs: cached_outputs,
        restore,
    }))
}

/// Whether the destination already holds exactly the bytes this hit would
/// place there.
///
/// The session ledger answers without a read when it can; otherwise the file
/// is read once and hashed, which costs what the copy it replaces would have
/// cost and buys an unchanged modification time. A ledger entry whose digest
/// disagrees is a content difference already proven, so it refuses without
/// reading either.
pub(crate) fn output_already_in_place(
    node: &CacheFileNode,
    destination: &Path,
    digests: &dyn mbx_cache_core::FileDigestCache,
) -> bool {
    let Ok(metadata) = std::fs::metadata(destination) else {
        return false;
    };
    if !metadata.is_file()
        || metadata.len() != node.digest.size
        || !executable_mode_matches(&metadata, node.executable)
    {
        return false;
    }
    if let Some(identity) = FileIdentity::describe(destination, &metadata)
        && let Some(recorded) = digests
            .find(FileDigestScope::Content, &[identity])
            .pop()
            .flatten()
    {
        return recorded == node.digest;
    }
    CacheDigest::blake3_file(destination).is_ok_and(|digest| digest == node.digest)
}

/// Enter restored outputs into the session file-digest ledger.
///
/// The digests were verified when the blobs entered the store, and the rename
/// that placed each file fixed the identity being recorded; a crate that links
/// one of these artifacts can then key it without reading it back. Best-effort
/// throughout: a file that cannot be described is simply not recorded.
fn record_output_digests(outputs: &[CachedOutput]) {
    let entries = outputs
        .iter()
        .filter_map(|output| {
            let metadata = std::fs::metadata(&output.path).ok()?;
            if metadata.len() != output.digest.size {
                return None;
            }
            Some(RecordedFileDigest {
                file: FileIdentity::describe(&output.path, &metadata)?,
                digest: output.digest.clone(),
            })
        })
        .collect::<Vec<_>>();
    session::record_file_digests(FileDigestScope::Content, entries);
}

fn finalize_restored_outputs(staged: StagedOutputs, restore_outputs: bool) -> Result<()> {
    if restore_outputs {
        persist_outputs(staged)?;
    }
    Ok(())
}

/// Describe how a restore differs from the compilation it was checked against.
///
/// `None` means they agree. A divergence names what disagreed, because that is
/// the whole output of a qualification run: knowing that something differed is
/// not actionable, and the answer is usually one specific file.
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

fn validated_outputs(
    directory: CacheDirectory,
    outputs: &RustcOutputs,
) -> Result<Vec<(CacheFileNode, PathBuf)>> {
    if directory.version != 1 || !directory.directories.is_empty() || !directory.symlinks.is_empty()
    {
        bail!("cached rustc output directory has unsupported entries");
    }
    let mut expected = outputs
        .files
        .iter()
        .chain(std::iter::once(&outputs.dep_info))
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| eyre::eyre!("expected rustc output name is not UTF-8"))?;
            if path.parent() != Some(outputs.directory.as_path()) {
                bail!("expected rustc output escapes its output directory");
            }
            Ok((
                name.to_string(),
                (path.clone(), outputs.is_executable(path)),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    if directory.files.len() != expected.len() {
        bail!("cached rustc output set does not match the invocation");
    }
    let mut files = Vec::with_capacity(directory.files.len());
    for node in directory.files {
        let (destination, executable) = expected
            .remove(&node.name)
            .ok_or_else(|| eyre::eyre!("cached rustc output is unexpected: {}", node.name))?;
        validate_file_mode(&node, executable)?;
        files.push((node, destination));
    }
    if !expected.is_empty() {
        bail!("cached rustc output set is incomplete");
    }
    Ok(files)
}

fn compiler_identity(rustc: &OsStr) -> Result<CompilerIdentity> {
    // One shim process serves one compiler, but several steps of one
    // compilation each build an action context. Asking the agent every time
    // turns one identity into several round trips per compilation.
    static IDENTITY: OnceLock<(OsString, CompilerIdentity)> = OnceLock::new();
    if let Some((known, identity)) = IDENTITY.get()
        && known.as_os_str() == rustc
    {
        return Ok(identity.clone());
    }
    let identity = query_compiler_identity(rustc)?;
    let _ = IDENTITY.set((rustc.to_os_string(), identity.clone()));
    Ok(identity)
}

fn query_compiler_identity(rustc: &OsStr) -> Result<CompilerIdentity> {
    let executable = resolve_executable(rustc)?;
    let environment = ["RUSTUP_HOME", "RUSTUP_TOOLCHAIN"]
        .into_iter()
        .map(|name| (name.into(), std::env::var(name).ok()))
        .collect::<BTreeMap<_, _>>();
    let responses = session::request_agent(&[AgentRequest::FindExecutableIdentity {
        executable: executable.clone(),
        environment: environment.clone(),
    }])?;
    let Some(AgentResponse::ExecutableIdentity { stdout }) = responses.into_iter().next() else {
        bail!("cache agent did not return the rustc identity");
    };
    let stdout = if let Some(stdout) = stdout {
        stdout
    } else {
        let mut command = Command::new(&executable);
        command.arg("-vV");
        for (name, value) in &environment {
            if let Some(value) = value {
                command.env(name, value);
            } else {
                command.env_remove(name);
            }
        }
        let output = command
            .output()
            .wrap_err("failed to query the rustc identity")?;
        if !output.status.success() {
            bail!(
                "rustc identity command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let responses = session::request_agent(&[AgentRequest::StoreExecutableIdentity {
            executable,
            environment,
            stdout: output.stdout,
        }])?;
        let Some(AgentResponse::ExecutableIdentity {
            stdout: Some(stdout),
        }) = responses.into_iter().next()
        else {
            bail!("cache agent did not store the rustc identity");
        };
        stdout
    };
    let verbose = std::str::from_utf8(&stdout).wrap_err("rustc identity is not UTF-8")?;
    let release = identity_field(verbose, "release")?;
    let host = identity_field(verbose, "host")?;
    let rustc_version = verbose
        .lines()
        .filter(|line| {
            line.starts_with("rustc ")
                || line.starts_with("commit-hash:")
                || line.starts_with("commit-date:")
                || line.starts_with("LLVM version:")
        })
        .collect::<Vec<_>>()
        .join("; ");
    if rustc_version.is_empty() {
        bail!("rustc identity is missing its version");
    }
    let toolchain = std::env::var("RUSTUP_TOOLCHAIN")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| release.to_string());
    Ok(CompilerIdentity {
        toolchain,
        rustc_version,
        host: host.to_string(),
    })
}

fn identity_field<'a>(verbose: &'a str, field: &str) -> Result<&'a str> {
    verbose
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{field}: ")))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| eyre::eyre!("rustc identity is missing {field}"))
}

/// Environment inputs eligible for remapping.
///
/// Deliberately just the one. `OUT_DIR` lives under the target directory, so
/// remapping it confines the change to generated sources, and it is the value
/// the plan identifies as the cross-checkout shortfall. Widening this list
/// widens which paths disappear from debug info, which is its own decision.
const PORTABLE_ENVIRONMENT: &[&str] = &["OUT_DIR"];

/// The environment values whose absolute paths this compilation was made
/// independent of.
///
/// `OUT_DIR` is the one that matters: every crate that includes build-script
/// output reads it, its value differs per checkout, and keeping it in the key
/// verbatim is what stops those compilations sharing between checkouts.
///
/// Two things must hold before a key may normalize such a value, and this type
/// is responsible for both. `--remap-path-prefix` makes rustc record the
/// placeholder instead of the real path, which covers debug info, spans, and
/// diagnostics -- everything rustc writes itself. It does not cover a value the
/// crate reads through `env!` and keeps as a string, so the outputs are read
/// before publishing and the portable key is used only if none carries it.
struct Portable {
    /// Path mappings for this compilation, ordered as keys need them.
    mappings: Vec<PathMapping>,
    /// Flags appended to the real rustc invocation, one per remapped value.
    arguments: Vec<OsString>,
    /// Names whose values an action key may normalize.
    names: BTreeSet<String>,
    /// The literal values, for the check before publishing.
    values: Vec<String>,
}

impl Portable {
    fn detect(working_dir: &Path, target_output: Option<&Path>, target: Option<&str>) -> Self {
        let mut portable = Self {
            mappings: PathMapping::ordered(&path_mappings(working_dir, target_output, target)),
            arguments: Vec::new(),
            names: BTreeSet::new(),
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
            // A value under no known root is one no key could agree on anyway,
            // so there is nothing to remap and nothing to promise.
            let Ok(placeholder) =
                normalize_mapped_path(Path::new(&value), working_dir, &portable.mappings)
            else {
                continue;
            };
            let mut flag = OsString::from("--remap-path-prefix=");
            flag.push(&value);
            flag.push("=");
            flag.push(&placeholder);
            portable.arguments.push(flag);
            portable.names.insert((*name).to_string());
            portable.values.push(value);
        }
        portable
    }

    /// The compiler arguments, with the remapping flags appended.
    fn applied_to(&self, arguments: &[OsString]) -> Vec<OsString> {
        let mut applied = arguments.to_vec();
        applied.extend(self.arguments.iter().cloned());
        applied
    }

    /// Whether the outputs are free of every value a portable key normalized.
    ///
    /// The dep-info file is not one of them: it records absolute input paths by
    /// construction, and is restored as written for every action that already
    /// shares across checkouts today. Judging the artifact by it would reject
    /// every compilation.
    fn contents_are_clean(&self, contents: &[u8]) -> bool {
        !self.values.is_empty() && !self.values.iter().any(|value| carries(contents, value))
    }
}

/// Whether `contents` holds `value` anywhere, in either separator spelling.
///
/// rustc writes paths with the platform separator in some places and forward
/// slashes in others, and a value missed here becomes a wrong answer rather
/// than a slow one, so both spellings are searched.
fn carries(contents: &[u8], value: &str) -> bool {
    if contains(contents, value.as_bytes()) {
        return true;
    }
    value.contains('\\') && contains(contents, value.replace('\\', "/").as_bytes())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && memchr::memmem::find(haystack, needle).is_some()
}

fn path_mappings(
    working_dir: &Path,
    target_output: Option<&Path>,
    target: Option<&str>,
) -> Vec<PathMapping> {
    path_mappings_with_env(working_dir, target_output, target, |name| {
        std::env::var_os(name)
    })
}

fn path_mappings_with_env(
    working_dir: &Path,
    target_output: Option<&Path>,
    target: Option<&str>,
    environment: impl Fn(&str) -> Option<OsString>,
) -> Vec<PathMapping> {
    let mut mappings = Vec::new();
    let mut roots = BTreeSet::new();
    let home_roots = ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(|name| environment(name).map(PathBuf::from))
        .filter(|root| root.is_absolute())
        .collect::<Vec<_>>();
    // The target directory comes first, and before the workspace that usually
    // contains it: output paths are the ones that differ between checkouts, and
    // mapping them explicitly also keeps keys stable when the target directory
    // is moved out of the workspace.
    //
    // Cargo compiles a dependency with its working directory inside the
    // registry, not in the workspace, so neither root can be inferred from the
    // working directory -- the session passes both in.
    let configured_target = environment(session::TARGET_DIR_ENV)
        .map(PathBuf::from)
        .filter(|root| root.is_absolute());
    if let Some(root) = configured_target.or_else(|| {
        target_output
            .filter(|root| root.is_absolute())
            .map(|output| standalone_target_root(output, target))
    }) {
        add_mapping(&mut mappings, &mut roots, root, "target");
    }
    for (name, placeholder) in [
        (session::WORKSPACE_ROOT_ENV, "workspace"),
        ("CARGO_HOME", "cargo_home"),
        ("RUSTUP_HOME", "rustup_home"),
    ] {
        if let Some(root) = environment(name).map(PathBuf::from)
            && root.is_absolute()
        {
            add_mapping(&mut mappings, &mut roots, root, placeholder);
        }
    }
    if let Some(home) = home_roots.first() {
        for (directory, placeholder) in [(".cargo", "cargo_home"), (".rustup", "rustup_home")] {
            if !mappings
                .iter()
                .any(|mapping| mapping.placeholder == placeholder)
            {
                add_mapping(&mut mappings, &mut roots, home.join(directory), placeholder);
            }
        }
    }
    // Without a session, recover Cargo's workspace root from the outermost
    // lockfile so member crates use the same placeholder as session mode.
    if !mappings
        .iter()
        .any(|mapping| mapping.placeholder == "workspace")
        && !roots.iter().any(|root| working_dir.starts_with(root))
    {
        add_mapping(
            &mut mappings,
            &mut roots,
            workspace_root(working_dir),
            "workspace",
        );
    }
    // Home is deliberately last. Most real checkouts live under it, but a
    // checkout-specific prefix must be `${workspace}` so equivalent worktrees
    // agree on their source paths. Cargo and rustup roots come first because a
    // registry compilation uses one of those as its working directory.
    for root in home_roots {
        add_mapping(&mut mappings, &mut roots, root, "home");
    }
    mappings
}

/// Infer the profile subtree shared by rustc outputs and build-script output.
///
/// Cargo normally writes compilations to `<target>/<profile>/deps` (or the
/// same shape below a target-triple directory). Mapping the profile parent,
/// rather than only `deps`, also covers generated inputs below `build/`.
fn standalone_target_root(output: &Path, target: Option<&str>) -> PathBuf {
    if output.file_name() == Some(OsStr::new("deps"))
        && let Some(profile_root) = output.parent().and_then(Path::parent)
    {
        let target_component = target.and_then(|target| Path::new(target).file_stem());
        if target_component.is_some_and(|target| profile_root.file_name() == Some(target))
            && let Some(root) = profile_root.parent()
        {
            return root.to_path_buf();
        }
        return profile_root.to_path_buf();
    }
    output.to_path_buf()
}

fn add_mapping(
    mappings: &mut Vec<PathMapping>,
    roots: &mut BTreeSet<PathBuf>,
    root: PathBuf,
    placeholder: &str,
) {
    if roots.insert(root.clone())
        && !mappings
            .iter()
            .any(|mapping| mapping.placeholder == placeholder)
    {
        mappings.push(PathMapping::new(root, placeholder));
    }
}

fn replay_output(output: &Output) -> Result<()> {
    replay_bytes(&output.stdout, &output.stderr)
}

fn publish_result<'a>(
    candidates: &'a ActionCandidates,
    portable: &Portable,
    outputs: &RustcOutputs,
    output: &Output,
    mappings: &[PathMapping],
) -> Result<&'a RustcAction> {
    if outputs.files.is_empty() {
        bail!("rustc produced no cacheable outputs");
    }
    let staging = staging_directory()?;
    let mut blobs = Vec::new();
    let stdout = staged_bytes(
        staging.path(),
        "stdout",
        &normalize_output_text(&output.stdout, mappings),
    )?;
    let stderr = staged_bytes(
        staging.path(),
        "stderr",
        &normalize_output_text(&output.stderr, mappings),
    )?;
    blobs.extend([stdout.clone(), stderr.clone()]);

    let output_paths = outputs
        .files
        .iter()
        .chain(std::iter::once(&outputs.dep_info));
    let mut files = Vec::with_capacity(outputs.files.len() + 1);
    let mut hashed_outputs = Vec::with_capacity(outputs.files.len());
    let mut portable_outputs_are_clean = candidates.portable.is_some();
    for path in output_paths {
        let metadata = std::fs::metadata(path)
            .wrap_err_with(|| format!("failed to inspect rustc output {}", path.display()))?;
        if !metadata.is_file() {
            bail!("rustc output is not a regular file: {}", path.display());
        }
        // The dep-info is stored in its placeholder form, so the checkout that
        // restores it gets rules naming its own target directory rather than
        // the one that published them.
        let digest = if path == &outputs.dep_info {
            let normalized = normalize_output_text(&std::fs::read(path)?, mappings);
            let staged = staged_bytes(staging.path(), "dep-info", &normalized)?;
            blobs.push(staged.clone());
            staged.0
        } else {
            // When a portable key is possible, inspect the same bytes used to
            // hash the artifact. Reading first and then calling `blake3_file`
            // made cold builds read every output twice merely to decide which
            // action key could safely name it.
            let digest = if candidates.portable.is_some() {
                let contents = std::fs::read(path)
                    .wrap_err_with(|| format!("failed to read rustc output {}", path.display()))?;
                portable_outputs_are_clean &= portable.contents_are_clean(&contents);
                CacheDigest::blake3(&contents)
            } else {
                CacheDigest::blake3_file(path)?
            };
            blobs.push((digest.clone(), path.clone()));
            // Freshly compiled artifacts enter the ledger too: on a cold
            // build these are exactly the rlibs every dependent is about to
            // key, and this hash is the read that ledger entries stand in
            // for. The dep-info stays out -- its stored digest describes the
            // placeholder form, not what is on disk.
            if metadata.len() == digest.size
                && let Some(file) = FileIdentity::describe(path, &metadata)
            {
                hashed_outputs.push(RecordedFileDigest {
                    file,
                    digest: digest.clone(),
                });
            }
            digest
        };
        files.push(CacheFileNode {
            digest,
            executable: outputs.is_executable(path),
            mode: file_mode(&metadata),
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| eyre::eyre!("rustc output name is not UTF-8"))?
                .to_string(),
        });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    session::record_file_digests(FileDigestScope::Content, hashed_outputs);

    let action = candidates.publishable(portable_outputs_are_clean);
    blobs.push(staged_bytes(staging.path(), "action.json", &action.bytes)?);

    let metadata = canonical_json(&RustcMetadata {
        version: 1,
        kind: "rustc".into(),
        stdout: stdout.0,
        stderr: stderr.0,
    })?;
    let metadata = staged_bytes(staging.path(), "metadata.json", &metadata)?;
    blobs.push(metadata.clone());
    let directory = canonical_json(&CacheDirectory {
        directories: Vec::new(),
        files,
        symlinks: Vec::new(),
        version: 1,
    })?;
    let directory = staged_bytes(staging.path(), "directory.json", &directory)?;
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
    Ok(action)
}

fn staged_bytes(directory: &Path, name: &str, bytes: &[u8]) -> Result<(CacheDigest, PathBuf)> {
    let path = directory.join(name);
    std::fs::write(&path, bytes)?;
    Ok((CacheDigest::blake3(bytes), path))
}

#[cfg(test)]
#[path = "rustc_tests.rs"]
mod tests;
