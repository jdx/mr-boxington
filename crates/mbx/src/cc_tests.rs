use super::*;

/// Inside a build the task is the session's run id, so predictions land in the
/// manifest the session commits. Only the standalone fallback is derived here,
/// and it must still be a legal task identity.
#[test]
fn standalone_prediction_tasks_are_valid_shard_identities() {
    let digest = CacheDigest::blake3(b"invocation");
    let task = standalone_prediction_task(&digest);
    assert!(
        mbx_cache_core::is_task_identity(&task),
        "prediction task {task} must satisfy the protocol's identity rules"
    );
}

#[test]
fn standalone_prediction_tasks_shard_rather_than_collecting_every_compile() {
    let shards = (0..64)
        .map(|index| {
            standalone_prediction_task(&CacheDigest::blake3(format!("{index}").as_bytes()))
        })
        .collect::<BTreeSet<_>>();
    assert!(shards.len() > 1, "compiles must not share one manifest");
    assert!(shards.len() <= 256, "shards are bounded by the hash prefix");
}

/// The exempt roots are POSIX paths and the shims are only installed on unix,
/// so this models the platform the feature runs on.
#[cfg(unix)]
#[test]
fn manifest_directories_skip_system_roots_and_include_header_parents() {
    let arguments = ["-I", "include", "-c", "-o", "a.o", "a.c"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    let invocation = CcInvocation::parse(&arguments).expect("admitted");
    let context = CcActionContext {
        compiler: CcCompilerIdentity {
            family: CcCompilerFamily::Clang,
            version_text: "clang version 17.0.6".into(),
            target: "aarch64-apple-darwin".into(),
            assembler: String::new(),
        },
        working_dir: PathBuf::from("/work/pkg"),
        path_mappings: Vec::new(),
        environment: BTreeMap::new(),
        inputs: Vec::new(),
    };
    let files = BTreeSet::from([
        PathBuf::from("/work/pkg/a.c"),
        PathBuf::from("/work/pkg/vendor/deep.h"),
        PathBuf::from("/usr/include/stdio.h"),
    ]);
    let directories = manifest_directories(&invocation, &context, &files);
    assert!(directories.contains(Path::new("/work/pkg/include")));
    assert!(directories.contains(Path::new("/work/pkg/vendor")));
    assert!(directories.contains(Path::new("/work/pkg")));
    assert!(
        !directories.contains(Path::new("/usr/include")),
        "system roots are covered by digests, not by enumerating an SDK"
    );
}

/// GCC resolves its assembler through its own exec prefix, so the first `as` on
/// `PATH` is often not the one it runs. Keying the wrong one would let two
/// toolchains share an entry whose object bytes they do not agree on.
#[test]
fn only_an_absolute_answer_from_the_driver_names_the_assembler() {
    assert_eq!(
        named_assembler("/toolchain/bin/as\n"),
        Some(PathBuf::from("/toolchain/bin/as"))
    );
    // A driver that cannot resolve the tool echoes the bare name back, which is
    // its way of saying "search PATH" -- so the caller must fall back, not
    // record a relative name.
    for unresolved in ["as", "as\n", "", "   "] {
        assert_eq!(
            named_assembler(unresolved),
            None,
            "{unresolved:?} does not name an assembler"
        );
    }
}

/// Pins the flag itself: a real driver answers `-print-prog-name=as` with a
/// path, which is what makes asking it better than reading `PATH`.
#[cfg(unix)]
#[test]
fn a_real_driver_names_its_assembler_by_absolute_path() {
    let Ok(output) = std::process::Command::new("cc")
        .arg("-print-prog-name=as")
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        named_assembler(&printed).is_some(),
        "the driver should name its assembler by path, said {printed:?}"
    );
}
