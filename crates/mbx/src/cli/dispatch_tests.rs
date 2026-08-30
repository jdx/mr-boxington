use super::*;

#[test]
fn all_non_reserved_subcommands_are_forwarded_directly() {
    for argv in [
        ["mbx", "new", "--vcs", "none"],
        ["mbx", "init", "--lib", "fixture"],
        ["mbx", "command-added-later", "--future-flag", "value"],
    ] {
        let argv = argv.map(std::ffi::OsStr::new);
        let cli = Cli::try_parse_from(&argv).unwrap();
        let Commands::Cargo(arguments) = cli.command else {
            panic!(
                "{} should be treated as a cargo subcommand",
                argv[1].to_string_lossy()
            );
        };
        let expected = argv[1..]
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(arguments, expected);
    }
}

#[test]
fn a_toolchain_in_front_reaches_cargo_as_it_was_typed() {
    let argv = ["mbx", "+1.91", "check", "--locked"].map(std::ffi::OsStr::new);

    let cli = Cli::try_parse_from(&argv).unwrap();

    assert_eq!(cli.toolchain.as_deref(), Some("1.91"));
    let Commands::Cargo(arguments) = cli.command else {
        panic!("check should still be a cargo subcommand");
    };
    // Read by mbx, and then handed back: cargo is the rustup shim, and the
    // command line is how it is told which toolchain to run.
    assert_eq!(arguments, ["check", "--locked"]);
    assert_eq!(
        with_toolchain(Some("1.91"), arguments),
        ["+1.91", "check", "--locked"]
    );
}

#[test]
fn a_command_that_names_no_toolchain_is_forwarded_untouched() {
    let argv = ["mbx", "check", "--locked"].map(std::ffi::OsStr::new);

    let cli = Cli::try_parse_from(&argv).unwrap();

    assert_eq!(cli.toolchain, None);
    let Commands::Cargo(arguments) = cli.command else {
        panic!("check should still be a cargo subcommand");
    };
    assert_eq!(with_toolchain(None, arguments), ["check", "--locked"]);
}

#[test]
fn a_plus_word_inside_a_cargo_command_belongs_to_cargo() {
    // The sigil classifies the word in front of the command. Past that point the
    // line is the cargo command's own, and a `+` in it means whatever cargo says
    // it means — including one held by a flag, or handed to the built program.
    for argv in [
        ["mbx", "test", "+weird", "--lib"],
        ["mbx", "build", "--features", "+simd"],
        ["mbx", "run", "--", "+5"],
    ] {
        let owned = argv.map(std::ffi::OsStr::new);
        let cli = Cli::try_parse_from(&owned).unwrap();
        assert_eq!(cli.toolchain, None, "{argv:?} names no toolchain to mbx");
        let Commands::Cargo(arguments) = cli.command else {
            panic!("{argv:?} should be a cargo subcommand");
        };
        assert_eq!(arguments, argv[1..]);
    }
}

#[test]
fn a_toolchain_selects_one_for_every_command_that_reaches_a_compiler() {
    let explain = ["mbx", "+nightly", "explain", "clippy"].map(std::ffi::OsStr::new);

    for argv in [
        vec!["mbx", "+nightly", "build"],
        vec!["mbx", "+nightly", "explain", "clippy"],
        vec!["mbx", "+nightly", "doctor"],
    ] {
        let owned = argv.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>();
        let cli = Cli::try_parse_from(&owned).unwrap();
        assert_eq!(cli.toolchain.as_deref(), Some("nightly"), "{argv:?}");
        assert_eq!(compiles_nothing(&cli.command), None, "{argv:?}");
    }

    let cli = Cli::try_parse_from(&explain).unwrap();
    let Commands::Explain(args) = cli.command else {
        panic!("explain should be reserved by mbx");
    };
    assert_eq!(
        with_toolchain(Some("nightly"), args.arguments()),
        ["+nightly", "clippy"]
    );
}

#[test]
fn the_words_before_the_subcommand_are_mbx_s_with_a_toolchain_or_without() {
    // mbx reads what comes before the subcommand, so `mbx -q build` has always
    // been an unknown flag rather than a quiet cargo build. A toolchain used to
    // switch that off by accident — `+stable` matched no command mbx knew, so
    // the rest of the line went to cargo unexamined — and now it does not.
    // Cargo's own globals still work where cargo takes them after the
    // subcommand, as `mbx build -q` does.
    for argv in [
        vec!["mbx", "-q", "build"],
        vec!["mbx", "+stable", "-q", "build"],
        vec!["mbx", "--offline", "build"],
        vec!["mbx", "+stable", "--offline", "build"],
    ] {
        let owned = argv.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>();
        assert!(
            Cli::try_parse_from(&owned).is_err(),
            "{argv:?} should be read by mbx, not handed over whole"
        );
    }

    let owned = ["mbx", "build", "-q"].map(std::ffi::OsStr::new);
    let cli = Cli::try_parse_from(&owned).unwrap();
    let Commands::Cargo(arguments) = cli.command else {
        panic!("build should be a cargo subcommand");
    };
    assert_eq!(arguments, ["build", "-q"]);
}

#[test]
fn a_toolchain_is_refused_where_no_compiler_would_see_it() {
    for (argv, command) in [
        (vec!["mbx", "+1.91", "gc"], "gc"),
        (vec!["mbx", "+1.91", "cache", "dir"], "cache"),
        (vec!["mbx", "+1.91", "tui"], "tui"),
        (vec!["mbx", "+1.91", "exec", "make"], "exec"),
        (vec!["mbx", "+1.91", "setup"], "setup"),
    ] {
        let owned = argv.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>();
        let cli = Cli::try_parse_from(&owned).unwrap();
        // Refused rather than ignored: naming a toolchain for a command that
        // compiles nothing is a misunderstanding worth reporting.
        assert_eq!(compiles_nothing(&cli.command), Some(command), "{argv:?}");
    }
}

#[test]
fn mbx_commands_still_take_precedence() {
    let argv = ["mbx", "gc", "--max-size", "1GiB"].map(std::ffi::OsStr::new);
    let cli = Cli::try_parse_from(&argv).unwrap();
    assert!(matches!(cli.command, Commands::Gc(_)));
}

#[test]
fn explain_forwards_cargo_flags_and_the_rustc_separator() {
    let argv = [
        "mbx",
        "explain",
        "clippy",
        "--workspace",
        "--",
        "-D",
        "warnings",
    ]
    .map(std::ffi::OsStr::new);
    let cli = Cli::try_parse_from(&argv).unwrap();
    let Commands::Explain(arguments) = cli.command else {
        panic!("explain should be reserved by mbx");
    };
    assert_eq!(
        arguments.arguments(),
        ["clippy", "--workspace", "--", "-D", "warnings"]
    );
}

#[test]
fn prefetch_preserves_cargo_flags_and_the_rustc_separator() {
    let argv = [
        "mbx",
        "prefetch",
        "test",
        "--workspace",
        "--",
        "--nocapture",
    ]
    .map(std::ffi::OsStr::new);

    let cli = Cli::try_parse_from(&argv).unwrap();

    let Commands::Prefetch(args) = cli.command else {
        panic!("prefetch should remain an mbx command");
    };
    assert_eq!(args.cargo_args, ["test", "--workspace", "--nocapture"]);
    let original = argv
        .iter()
        .map(|argument| argument.to_os_string())
        .collect::<Vec<_>>();
    assert_eq!(
        original_prefetch_arguments(&original).unwrap(),
        ["test", "--workspace", "--", "--nocapture"]
    );
}

#[test]
fn cli_exposes_its_usage_spec() {
    let spec = Cli::to_kdl();
    assert!(spec.contains("external_subcommand #true"));
    // Still declared, so it keeps working and stays out of help and
    // completions rather than being removed from under anyone using it.
    assert!(spec.contains("cmd setup"));
    assert!(
        spec.contains("hide=#true"),
        "setup should be hidden from help: {spec}"
    );
    assert!(
        spec.contains("sigil=+"),
        "the toolchain argument should be classified by its sigil: {spec}"
    );
    assert!(spec.contains("cmd explain"));
    assert!(spec.contains("cmd doctor"));
    assert!(spec.contains("cmd gc"));
    assert!(spec.contains("cmd cache"));
    assert!(spec.contains("config {"));
    assert!(spec.contains(r#"prop "gc.max_size""#));
    assert!(spec.contains(r#"env "MBX_GC_MAX_SIZE""#));
}
