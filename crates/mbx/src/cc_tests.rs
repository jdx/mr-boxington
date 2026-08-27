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
    // Spelled per platform: `/toolchain/bin/as` is not an absolute path on
    // Windows, and a test that quietly took the fallback branch there would
    // assert nothing.
    let absolute = if cfg!(windows) {
        r"C:\toolchain\bin\as.exe"
    } else {
        "/toolchain/bin/as"
    };
    assert_eq!(
        named_assembler(&format!("{absolute}\n")),
        Some(PathBuf::from(absolute))
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

/// Pins the flag against a real driver: whatever it prints has to be one of the
/// two shapes the caller handles.
///
/// Not every installation resolves the tool -- a macOS box with only the
/// command line tools can echo the bare name back -- so the absolute answer is
/// asserted where it appears rather than demanded.
#[cfg(unix)]
#[test]
fn a_real_driver_names_its_assembler_or_defers_to_the_path() {
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
    match named_assembler(&printed) {
        // The driver knows where its assembler is, and the identity records
        // that path rather than whatever `PATH` happens to resolve.
        Some(assembler) => assert_eq!(assembler, PathBuf::from(printed.trim())),
        // It does not, and searching `PATH` is what the driver would do too.
        None => assert!(
            !printed.trim().contains('/'),
            "an unresolved answer should be a bare name, got {printed:?}"
        ),
    }
}
