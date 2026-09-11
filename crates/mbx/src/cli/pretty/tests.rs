use super::*;
use model::{CacheMix, segments};

#[test]
fn segments_partition_exactly_and_handle_extremes() {
    for mix in [
        CacheMix::default(),
        CacheMix {
            hits: 7,
            misses: 2,
            bypasses: 1,
            saved_ns: 0,
            unconsulted: 0,
        },
        CacheMix {
            hits: u64::MAX,
            misses: u64::MAX,
            bypasses: u64::MAX,
            saved_ns: 0,
            unconsulted: 0,
        },
    ] {
        for width in 0..=80 {
            assert_eq!(segments(&mix, width).iter().sum::<usize>(), width);
        }
    }
    assert_eq!(
        segments(
            &CacheMix {
                hits: 7,
                misses: 2,
                bypasses: 1,
                saved_ns: 0,
                unconsulted: 0
            },
            20
        ),
        [14, 4, 2]
    );
}

#[test]
fn cache_mix_matches_existing_stats_and_excludes_probes() {
    let mut stats = AgentStats::default();
    stats.lookups = 10;
    stats.hits = 6;
    stats.verifications = 1;
    stats.bypasses = BTreeMap::from([("compiler-query".into(), 4), ("unsupported".into(), 2)]);
    stats.avoided_compiler_duration_ns = 42;
    let mix = CacheMix::from(stats);
    assert_eq!(
        (mix.hits, mix.misses, mix.bypasses, mix.saved_ns),
        (6, 3, 2, 42)
    );
}

#[test]
fn fresh_artifacts_are_not_cache_hits_and_missing_timings_stay_unknown() {
    let mut model = Model::new(&["build".into()]);
    assert!(model.cargo(r##"{"reason":"compiler-artifact","package_id":"registry+url#demo@1.0.0","target":{"name":"demo"},"fresh":true}"##));
    assert_eq!(model.fresh, 1);
    assert_eq!(model.mix.hits, 0);
    assert_eq!(model.done[0].duration, None);
    assert_eq!(model.units_total, None);
    assert!(model.status("    Building [======> ] 2/10: demo"));
    assert_eq!((model.units_done, model.units_total), (2, Some(10)));
}

#[test]
fn native_test_output_tracks_suites_and_failures_without_fabricated_times() {
    let mut model = Model::new(&["test".into()]);
    assert!(model.test_line("running 2 tests"));
    assert!(model.test_line("test good ... ok"));
    assert!(model.test_line("test bad ... FAILED"));
    model.test_line("failures:");
    assert!(!model.test_line("---- bad stdout ----"));
    model.test_line("panic at src/lib.rs:3:4");
    model.test_line("failures:");
    assert_eq!((model.tests_passed, model.tests_failed), (1, 1));
    assert!(model.failures[0].1.contains("panic at"));
    assert_eq!(model.done[0].duration, None);
    assert!(model.test_line("running 1 test"));
    assert_eq!((model.suite_done, model.suite_total), (0, 1));
    assert!(!model.test_line("custom harness output"));
}

#[test]
fn argument_injection_preserves_command_and_program_arguments() {
    let args = vec![
        "run".into(),
        "--bin=server".into(),
        "--".into(),
        "--message-format=custom".into(),
    ];
    assert_eq!(
        cargo_arguments(&args),
        [
            "run",
            "--bin=server",
            "--message-format=json,json-diagnostic-rendered-ansi",
            "--",
            "--message-format=custom"
        ]
    );
    assert!(eligible(&args));
    let prefixed = vec!["+stable".into(), "test".into(), "--".into(), "works".into()];
    assert!(eligible(&prefixed));
    assert_eq!(cargo_verb(&prefixed), Some("test"));
    assert_eq!(
        cargo_arguments(&prefixed),
        [
            "+stable",
            "test",
            "--message-format=json,json-diagnostic-rendered-ansi",
            "--",
            "works"
        ]
    );
    for args in [
        vec!["test", "--message-format=json"],
        vec!["build", "-q"],
        vec!["check", "--config=foo"],
        vec!["metadata"],
    ] {
        assert!(!eligible(
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        ));
    }
}

#[cfg(unix)]
#[test]
fn preserves_signalled_exit_status() {
    use std::os::unix::process::ExitStatusExt;
    let status = portable_pty::ExitStatus::from(std::process::ExitStatus::from_raw(libc::SIGTERM));
    assert_eq!(status_code(&status), 143);
}

#[test]
fn presentation_keeps_reserved_rows_and_clips_diagnostics() {
    let mut model = Model::new(&["build".into()]);
    let empty = view::render(&mut model, None, 80, 27);
    assert_eq!(empty.size().1, 27);
    assert!(strip_ansi(&empty.to_string()).contains("total unknown"));
    assert!(model.status("   Compiling a-crate v0.1.0 (/tmp/a-crate)"));
    assert!(model.cargo(r##"{"reason":"compiler-artifact","package_id":"path+file:///tmp/a-crate#0.1.0","target":{"name":"a_crate"},"fresh":false}"##));
    assert!(model.live.is_empty());
    assert!(model.done[0].duration.is_some());
    let populated = view::render(&mut model, None, 80, 27);
    assert_eq!(populated.size().1, 27);
    model.warnings.push(model::Warning {
        message: "warning".into(),
        rendered: "wide 文本\n".repeat(100),
        location: None,
        help: None,
    });
    let mut browser = view::Browser {
        inspecting: true,
        scroll: usize::MAX,
        selected: 0,
    };
    let block = view::render(&mut model, Some(&mut browser), 20, 12);
    assert!(block.size().0 <= 20);
    assert_eq!(block.size().1, 12);
    assert!(browser.scroll < 200);
}

#[test]
fn terse_and_ignored_test_summaries_remain_accurate() {
    let mut model = Model::new(&["test".into()]);
    model.test_line("running 3 tests");
    model.test_line("..i");
    model.test_line("test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s");
    assert_eq!(
        (
            model.tests_passed,
            model.tests_failed,
            model.tests_ignored,
            model.suite_done
        ),
        (2, 0, 1, 3)
    );
}

#[test]
fn failed_build_does_not_fill_unfinished_units() {
    let mut model = Model::new(&["build".into()]);
    model.units_total = Some(10);
    model.units_done = 3;
    model.build_finished = true;
    model.build_ok = Some(false);
    let text = strip_ansi(&view::render(&mut model, None, 110, 30).to_string());
    assert!(text.contains("3/10 units"));
    assert!(text.contains(&"░".repeat(20)));
    model.units_total = None;
    model.build_ok = Some(true);
    let text = strip_ansi(&view::render(&mut model, None, 110, 30).to_string());
    assert!(text.contains("total unknown"));
}

#[test]
fn status_words_in_child_output_are_preserved() {
    let mut model = Model::new(&["build".into()]);
    for line in [
        "Compiling assets",
        "Checking configuration",
        "   Compiling assets",
        "    Checking configuration",
        "Finished generating assets",
        "Building [assets] 1/2: images",
    ] {
        assert!(!model.status(line), "consumed child output: {line}");
    }
    assert!(model.live.is_empty());
    assert!(model.status("   Compiling demo v1.2.3 (/tmp/demo)"));
    assert!(model.status("    Checking demo v1.2.3"));
    assert!(model.status("    Finished `dev` profile [unoptimized] target(s) in 0.2s"));
}

#[test]
fn final_summary_keeps_only_its_content_rows() {
    let mut model = Model::new(&["build".into()]);
    model.finished = Some((true, Duration::from_secs(2)));
    let block = view::render(&mut model, None, 80, 27);
    assert_eq!(block.size().1, view::summary(&model).size().1);
    assert!(block.size().1 < 10);
}

#[test]
fn passing_test_stdout_does_not_open_failure_browser() {
    let mut model = Model::new(&["test".into()]);
    for line in [
        "running 1 test",
        "test works ... ok",
        "successes:",
        "---- works stdout ----",
        "hello",
        "test result: ok. 1 passed; 0 failed; 0 ignored;",
    ] {
        model.test_line(line);
    }
    assert!(model.failures.is_empty());
    for line in [
        "running 1 test",
        "test fails ... FAILED",
        "failures:",
        "---- fails stdout ----",
        "panic detail",
    ] {
        model.test_line(line);
    }
    assert_eq!(model.failures.len(), 1);
    assert!(model.failures[0].1.contains("panic detail"));
}

#[test]
fn active_rows_keep_slots_and_cache_outcomes_remain_explicit() {
    let mut model = Model::new(&["build".into()]);
    model.status("   Compiling zeta v1.0.0");
    model.status("   Compiling alpha v1.0.0");
    let original = model.slots.clone();
    assert!(model.cargo(r#"{"reason":"compiler-artifact","package_id":"path+file:///zeta#zeta@1.0.0","target":{"name":"zeta"},"filenames":["/tmp/libzeta-1111111111111111.rmeta"],"fresh":false}"#));
    assert_eq!(model.slots[1], original[1]);
    let mut stats = AgentStats::default();
    stats
        .unit_outcomes
        .insert("zeta:1111111111111111".into(), ["hit".into()].into());
    model.update_stats(stats.clone());
    assert!(strip_ansi(&view::render(&mut model, None, 110, 27).to_string()).contains("hit"));
    stats
        .unit_outcomes
        .get_mut("zeta:1111111111111111")
        .unwrap()
        .insert("miss".into());
    model.update_stats(stats);
    assert!(strip_ansi(&view::render(&mut model, None, 110, 27).to_string()).contains("mixed"));
}

#[test]
fn malformed_versions_and_large_exit_codes_stay_failures_or_passthrough() {
    let mut model = Model::new(&["build".into()]);
    assert!(!model.status("   Compiling assets v1.2.not-a-version"));
    assert_eq!(
        status_code(&portable_pty::ExitStatus::with_exit_code(256)),
        1
    );
}

#[test]
fn small_viewports_promote_waiting_crates_without_moving_visible_crates() {
    let mut model = Model::new(&["build".into()]);
    for name in ["first", "second", "third", "fourth", "fifth", "sixth"] {
        model.status(&format!("   Compiling {name} v1.0.0"));
    }
    view::render(&mut model, None, 80, 14);
    assert_eq!(model.slots.len(), 1);
    assert!(model.cargo(r#"{"reason":"compiler-artifact","package_id":"path+file:///first#first@1.0.0","target":{"name":"first"},"fresh":false}"#));
    assert_eq!(model.slots[0].as_deref(), Some("second v1.0.0"));
    let text = strip_ansi(&view::render(&mut model, None, 80, 14).to_string());
    assert!(text.contains("second"));
    view::render(&mut model, None, 80, 24);
    assert_eq!(model.slots[0].as_deref(), Some("second v1.0.0"));
    assert_eq!(model.slots.len(), 6);
}

#[test]
fn summary_uses_the_cargo_verb_and_build_scripts_normalize_like_rustc() {
    for (arguments, expected) in [
        (vec!["c".into()], "Checked"),
        (vec!["+stable".into(), "check".into()], "Checked"),
        (
            vec!["build".into(), "--features".into(), "unchecked".into()],
            "Built",
        ),
    ] {
        let model = Model::new(&arguments);
        assert!(strip_ansi(&view::summary(&model).to_string()).contains(expected));
    }
    let mut model = Model::new(&["build".into()]);
    model.cargo(r#"{"reason":"compiler-artifact","package_id":"path+file:///demo#demo@1.0.0","target":{"name":"build-script-build","kind":["custom-build"]},"filenames":["/tmp/demo-1111111111111111/build-script-build"],"fresh":false}"#);
    assert_eq!(
        model.done[0].cache_target.as_deref(),
        Some("build_script_build:1111111111111111")
    );
}

#[test]
fn cargo_fingerprints_isolate_versions_and_match_compiler_arguments() {
    let mut model = Model::new(&["build".into()]);
    for (version, hash) in [("1.0.0", "1111111111111111"), ("2.0.0", "2222222222222222")] {
        model.cargo(&serde_json::json!({"reason":"compiler-artifact", "package_id":format!("serde@{version}"), "target":{"name":"serde"}, "filenames":[format!("/tmp/libserde-{hash}.rmeta")], "fresh":false}).to_string());
    }
    assert_ne!(model.done[0].cache_target, model.done[1].cache_target);
    assert_eq!(
        model.done[0].cache_target,
        crate::session::compiler_unit_key(
            "serde",
            ["-C".into(), "extra-filename=-1111111111111111".into()]
        )
    );
    assert!(
        crate::session::compiler_unit_key("serde", ["--crate-name".into(), "serde".into()])
            .is_none()
    );
}
