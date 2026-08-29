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

/// A qualification run reports a count; a count with no reason attached is not
/// something anybody can act on. The C adapter said only that something
/// diverged, which is how 150 identical-looking divergences in one run stayed
/// undiagnosed -- they were every object of one crate whose generated headers
/// live under `OUT_DIR`, and the message could not say so.
#[test]
fn a_divergence_says_which_of_the_things_it_compares_differed() {
    let directory = tempfile::tempdir().unwrap();
    let object = directory.path().join("hello.o");
    std::fs::write(&object, b"compiled").unwrap();

    let cached = |stdout: &[u8], stderr: &[u8], digest: CacheDigest| CachedCompilation {
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
        outputs: vec![CachedOutput {
            path: object.clone(),
            digest,
            executable: false,
            mode: file_mode(&std::fs::metadata(&object).unwrap()),
        }],
        restore: RestoreStats::default(),
    };
    let compiled = |stdout: &[u8], stderr: &[u8]| Output {
        status: success_status(),
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    };
    let matching = CacheDigest::blake3(b"compiled");

    assert_eq!(
        verification_divergence(&cached(b"", b"", matching.clone()), &compiled(b"", b"")),
        None,
        "a compilation that reproduced itself is not a divergence"
    );
    assert_eq!(
        verification_divergence(&cached(b"out", b"", matching.clone()), &compiled(b"", b"")),
        Some("standard output differs".into())
    );
    assert_eq!(
        verification_divergence(&cached(b"", b"warn", matching), &compiled(b"", b"")),
        Some("standard error differs".into())
    );

    // The one that matters: the object itself. This is what every divergence
    // in a real qualification run turned out to be.
    let divergence = verification_divergence(
        &cached(b"", b"", CacheDigest::blake3(b"compiled elsewhere")),
        &compiled(b"", b""),
    )
    .expect("differing contents diverge");
    assert!(
        divergence.ends_with("has different contents"),
        "unexpected reason: {divergence}"
    );
    assert!(
        divergence.contains("hello.o"),
        "the reason names the file: {divergence}"
    );
}

#[cfg(unix)]
fn success_status() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    std::process::ExitStatus::from_raw(0)
}

#[cfg(windows)]
fn success_status() -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt as _;
    std::process::ExitStatus::from_raw(0)
}

/// The flags are the half that makes the compiler record a placeholder; the
/// output check is the half that refuses when something recorded the path
/// anyway. Either alone is wrong -- one silently, one unsoundly.
#[test]
fn a_portable_compilation_remaps_its_paths_and_refuses_an_output_that_kept_one() {
    let directory = tempfile::tempdir().unwrap();
    let out_dir = "/checkout/target/debug/build/widget-1234/out";
    let portable = Portable {
        arguments: vec![OsString::from(format!(
            "-fdebug-prefix-map={out_dir}=${{target}}/debug/build/widget-1234/out"
        ))],
        values: vec![out_dir.to_string()],
    };

    // The flag is appended, so it lands in the key like any other argument.
    let arguments = vec![OsString::from("-c"), OsString::from("src/hello.c")];
    let applied = portable.applied_to(&arguments);
    assert_eq!(applied.len(), 3);
    assert!(
        applied[2]
            .to_string_lossy()
            .starts_with("-fdebug-prefix-map="),
        "the remap is appended: {applied:?}"
    );

    let clean = directory.path().join("clean.o");
    std::fs::write(&clean, b"nothing here names a checkout").unwrap();
    assert!(portable.outputs_are_clean(&clean));

    // A path the source kept as a string survives the remap, and an object
    // carrying one must not be published under a key that normalized it.
    let dirty = directory.path().join("dirty.o");
    std::fs::write(&dirty, format!("built in {out_dir} and says so").as_bytes()).unwrap();
    assert!(!portable.outputs_are_clean(&dirty));

    // An unreadable output is not evidence of cleanliness.
    assert!(!portable.outputs_are_clean(&directory.path().join("absent.o")));

    // With nothing remapped there is nothing to promise, and every output
    // passes -- which is what a build with OUT_DIR sharing off looks like.
    let inert = Portable {
        arguments: Vec::new(),
        values: Vec::new(),
    };
    assert!(inert.outputs_are_clean(&dirty));
    assert!(matches!(inert.applied_to(&arguments), Cow::Borrowed(_)));
}
