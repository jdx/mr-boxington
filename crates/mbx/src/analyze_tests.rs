use super::*;
use crate::events::ActionDetail;
use mbx_cache_core::CacheDigest;

const SECOND: u64 = 1_000_000_000;

fn digest(value: &str) -> CacheDigest {
    CacheDigest::blake3(value.as_bytes())
}

/// Key material for `name`, identified as the same compilation unit in every
/// recording so that a later build finds it as a baseline.
fn diagnostic(
    name: &str,
    components: &[(&str, &str)],
    inputs: &[(&str, &str)],
) -> ActionDiagnostic {
    let mut all = BTreeMap::from([("compilation unit".to_string(), digest(name))]);
    all.extend(
        components
            .iter()
            .map(|(component, value)| (component.to_string(), digest(value))),
    );
    let inputs: BTreeMap<_, _> = inputs
        .iter()
        .map(|(path, value)| (path.to_string(), digest(value)))
        .collect();
    let action = digest(&format!("{name}{all:?}{inputs:?}"));
    ActionDiagnostic {
        action,
        components: all,
        inputs,
    }
}

fn action(
    outcome: ActionOutcome,
    name: &str,
    seconds: u64,
    diagnostic: Option<ActionDiagnostic>,
) -> SessionEvent {
    SessionEvent::Action {
        v: 1,
        ts_ms: 0,
        outcome,
        crate_name: Some(name.into()),
        duration_ns: seconds * SECOND,
        detail: ActionDetail::default(),
        diagnostic,
    }
}

fn session(events: Vec<SessionEvent>) -> RecordedSession {
    RecordedSession {
        workspace: "/work".into(),
        identity: Some("project".into()),
        command: vec!["build".into()],
        events,
    }
}

fn analyze(earlier: Vec<SessionEvent>, events: Vec<SessionEvent>) -> Analysis {
    let target = session(events);
    let baselines = Baselines::collect(&[session(earlier)], &target);
    Analysis::of(&target, &baselines)
}

fn group<'a>(analysis: &'a Analysis, cause: &Cause) -> &'a Group {
    analysis
        .groups
        .get(cause)
        .unwrap_or_else(|| panic!("no {cause:?} group in {:?}", analysis.groups))
}

const ENGINE_BEFORE: &str = "target/debug/deps/libengine-0a1b2c3d.rmeta";
const API_BEFORE: &str = "target/debug/deps/libapi-0f0f0f0f.rmeta";

fn engine(source: &str) -> ActionDiagnostic {
    diagnostic("engine", &[], &[("src/lib.rs", source)])
}

fn api(engine_artifact: &str) -> ActionDiagnostic {
    diagnostic(
        "api",
        &[],
        &[("src/api.rs", "api"), (ENGINE_BEFORE, engine_artifact)],
    )
}

fn cli(api_artifact: &str) -> ActionDiagnostic {
    diagnostic(
        "cli",
        &[],
        &[("src/main.rs", "cli"), (API_BEFORE, api_artifact)],
    )
}

#[test]
fn a_change_is_charged_to_the_crate_it_started_in() {
    let analysis = analyze(
        vec![
            action(ActionOutcome::Hit, "engine", 0, Some(engine("v1"))),
            action(ActionOutcome::Hit, "api", 0, Some(api("engine v1"))),
            action(ActionOutcome::Hit, "cli", 0, Some(cli("api v1"))),
        ],
        vec![
            action(ActionOutcome::Miss, "engine", 4, Some(engine("v2"))),
            action(ActionOutcome::Miss, "api", 3, Some(api("engine v2"))),
            // Two levels down: cli consumes only api, and api only moved
            // because engine did.
            action(ActionOutcome::Miss, "cli", 2, Some(cli("api v2"))),
        ],
    );

    assert_eq!(analysis.groups.len(), 1, "{:?}", analysis.groups);
    let changed = group(&analysis, &Cause::Changed("engine".into()));
    assert_eq!(changed.duration_ns, 9 * SECOND);
    assert_eq!(changed.direct_count, 1);
    assert_eq!(changed.dependent_count, 2);
    let text = analysis.text();
    assert!(text.contains("inputs of engine changed (3)"), "{text}");
    assert!(
        text.contains("then 2 crates that depend on it rebuilt: api 3.00s and cli 2.00s"),
        "{text}"
    );
    assert!(
        text.contains("Every crate that depends on engine recompiled"),
        "{text}"
    );
}

#[test]
fn dependency_hashes_that_follow_a_flag_change_are_charged_to_it() {
    let opt = |level: &str| {
        diagnostic(
            "engine",
            &[
                ("argument --C opt-level", level),
                ("argument --C metadata", level),
            ],
            &[("src/lib.rs", "same")],
        )
    };
    // Cargo moves a dependent's metadata hash and `--extern` path along with
    // its dependency's, so those changes are consequences.
    let dependent = |level: &str| {
        diagnostic(
            "api",
            &[
                ("argument --C metadata", level),
                ("argument --extern", level),
            ],
            &[(ENGINE_BEFORE, level)],
        )
    };
    let analysis = analyze(
        vec![
            action(ActionOutcome::Hit, "engine", 0, Some(opt("0"))),
            action(ActionOutcome::Hit, "api", 0, Some(dependent("0"))),
        ],
        vec![
            action(ActionOutcome::Miss, "engine", 5, Some(opt("3"))),
            action(ActionOutcome::Miss, "api", 1, Some(dependent("3"))),
        ],
    );

    let flags = group(&analysis, &Cause::Settings(Setting::Flags));
    assert_eq!(flags.duration_ns, 6 * SECOND);
    assert_eq!(flags.dependent_count, 1);
    let text = analysis.text();
    assert!(text.contains("compiler arguments changed (2)"), "{text}");
    assert!(text.contains("changed: -C opt-level (1)"), "{text}");
}

#[test]
fn settings_are_named_by_what_changed() {
    let names = |components: &[&str]| {
        components
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    };

    assert_eq!(
        setting(&names(&["compiler version", "argument --C opt-level"])),
        (Setting::Toolchain, Vec::new())
    );
    assert_eq!(
        setting(&names(&["action model"])),
        (Setting::ActionModel, Vec::new())
    );
    // rustc's own spelling of `-C` is `--codegen`, and a metadata hash moves
    // with any dependency, so it is not what changed here.
    assert_eq!(
        setting(&names(&[
            "argument --cfg #3",
            "argument #12",
            "argument --codegen rpath",
            "argument --codegen metadata",
        ])),
        (
            Setting::Flags,
            names(&["--cfg", "-C rpath", "argument values"])
        )
    );
    assert_eq!(
        setting(&names(&["environment RUSTFLAGS_EXTRA"])),
        (Setting::Environment, names(&["RUSTFLAGS_EXTRA"]))
    );
    assert_eq!(
        setting(&names(&["linker driver"])),
        (Setting::Linker, Vec::new())
    );
}

#[test]
fn causes_are_ranked_by_time_and_expected_bypasses_are_set_apart() {
    let analysis = analyze(
        vec![action(ActionOutcome::Hit, "small", 0, Some(engine("v1")))],
        vec![
            action(
                ActionOutcome::Bypass {
                    reason: "compiler-query".into(),
                },
                "___",
                0,
                None,
            ),
            action(
                ActionOutcome::Bypass {
                    reason: "native-library".into(),
                },
                "ring",
                7,
                None,
            ),
            action(ActionOutcome::Unconsulted, "fresh", 2, None),
            action(ActionOutcome::Miss, "gone", 1, None),
        ],
    );

    let text = analysis.text();
    let position = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle:?} missing from {text}"))
    };
    assert!(
        position("not cacheable: native-library") < position("first build of these compilations")
    );
    assert!(
        position("first build of these compilations")
            < position("misses with no earlier recording")
    );
    assert!(position("misses with no earlier recording") < position("expected, nothing to cache"));
    assert!(text.contains("expected, nothing to cache: compiler-query (1)"));
    assert!(!text.contains("not cacheable: compiler-query"));
    assert!(text.contains("10.00s in 3 uncached compilations"), "{text}");
}

/// New RUSTFLAGS leave no prediction to look up, so these compilations are
/// unconsulted rather than missed. Their keys still say what changed.
#[test]
fn an_unconsulted_compilation_is_compared_with_its_last_recording() {
    let flags = |value: &str| {
        diagnostic(
            "engine",
            &[("argument --cfg", value)],
            &[("src/lib.rs", "same")],
        )
    };
    let analysis = analyze(
        vec![action(ActionOutcome::Hit, "engine", 0, Some(flags("none")))],
        vec![
            action(
                ActionOutcome::Unconsulted,
                "engine",
                2,
                Some(flags("extra")),
            ),
            action(ActionOutcome::Unconsulted, "fresh", 1, None),
        ],
    );

    let flags = group(&analysis, &Cause::Settings(Setting::Flags));
    assert_eq!(flags.direct_count, 1);
    assert_eq!(flags.details, BTreeMap::from([("--cfg".to_string(), 1)]));
    assert_eq!(group(&analysis, &Cause::FirstBuild).direct_count, 1);
}

#[test]
fn probes_are_not_counted_as_uncached_work() {
    let probe = || {
        action(
            ActionOutcome::Bypass {
                reason: "compiler-query".into(),
            },
            "___",
            0,
            None,
        )
    };
    let text = analyze(Vec::new(), vec![probe(), probe()]).text();

    assert!(text.contains("0s in 0 uncached compilations\n"), "{text}");
    assert!(text.contains("Cargo found every unit up to date"), "{text}");
}

#[test]
fn an_unchanged_key_that_missed_is_a_missing_result() {
    let analysis = analyze(
        vec![action(ActionOutcome::Hit, "engine", 0, Some(engine("v1")))],
        vec![action(ActionOutcome::Miss, "engine", 3, Some(engine("v1")))],
    );

    assert_eq!(group(&analysis, &Cause::Unavailable).direct_count, 1);
}

#[test]
fn a_bypass_without_recorded_time_says_so() {
    let analysis = analyze(
        Vec::new(),
        vec![action(
            ActionOutcome::Bypass {
                reason: "rustdoc".into(),
            },
            "docs",
            0,
            None,
        )],
    );

    let text = analysis.text();
    assert!(text.contains("-  not cacheable: rustdoc (1)"), "{text}");
    assert!(text.contains("compiler time was not recorded"), "{text}");
}

#[test]
fn a_build_with_nothing_uncached_says_so() {
    let analysis = analyze(
        Vec::new(),
        vec![action(ActionOutcome::Hit, "engine", 0, Some(engine("v1")))],
    );

    let text = analysis.text();
    assert!(
        text.contains("0s in 0 uncached compilations; 1 hit avoided"),
        "{text}"
    );
    assert!(text.contains("every compilation that could be cached was restored"));
}

#[test]
fn prose_wraps_at_word_boundaries() {
    assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
}
