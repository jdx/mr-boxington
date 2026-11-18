use super::*;

#[test]
fn reserve_keeps_its_command_arguments_after_a_separator() {
    let argv = [
        "mbx", "reserve", "--memory", "6GiB", "--cpus", "4", "--", "sh", "-c", "echo hi",
    ]
    .map(std::ffi::OsStr::new);

    let cli = Cli::try_parse_from(&argv).unwrap();
    let Commands::Reserve(args) = cli.command else {
        panic!("reserve should be reserved by mbx");
    };
    assert_eq!(args.command, ["sh", "-c", "echo hi"]);
}

#[test]
fn a_toolchain_is_refused_for_reserve() {
    let argv = ["mbx", "+nightly", "reserve", "--cpus", "1", "true"].map(std::ffi::OsStr::new);
    let cli = Cli::try_parse_from(&argv).unwrap();
    assert_eq!(compiles_nothing(&cli.command), Some("reserve"));
}

#[test]
fn external_work_skips_nested_admission_without_leaking_the_worker_marker() {
    let command = reserve::external_command("echo", &[], None);
    let environment = command
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy(),
                value.map(|value| value.to_string_lossy()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_eq!(environment.get("MBX_SCHED_DIR"), Some(&Some("".into())));
    assert_eq!(
        environment.get("MBX_SCHED_DISABLE"),
        Some(&Some("1".into()))
    );
    assert_eq!(environment.get("MBX_RESERVE_WORKER"), Some(&None));
}

fn reserved_cpus(cpus: Option<u64>) -> Option<Option<String>> {
    reserve::external_command("echo", &[], cpus)
        .get_envs()
        .find(|(name, _)| *name == "MBX_RESERVED_CPUS")
        .map(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
}

#[test]
fn external_work_is_told_how_many_cores_it_holds() {
    assert_eq!(reserved_cpus(Some(4)), Some(Some("4".into())));
}

#[test]
fn a_reservation_without_cores_clears_the_inherited_width() {
    assert_eq!(reserved_cpus(None), Some(None));
    assert_eq!(reserved_cpus(Some(0)), Some(None));
}
