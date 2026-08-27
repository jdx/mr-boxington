use super::*;

#[test]
fn prediction_tasks_are_valid_shard_identities() {
    let digest = CacheDigest::blake3(b"invocation");
    let task = prediction_task(&digest);
    assert!(
        mbx_cache_core::is_task_identity(&task),
        "prediction task {task} must satisfy the protocol's identity rules"
    );
}

#[test]
fn prediction_tasks_shard_rather_than_collecting_every_compile() {
    let one = prediction_task(&CacheDigest::blake3(b"one"));
    let two = prediction_task(&CacheDigest::blake3(b"two"));
    let shards = (0..64)
        .map(|index| prediction_task(&CacheDigest::blake3(format!("{index}").as_bytes())))
        .collect::<BTreeSet<_>>();
    assert!(shards.len() > 1, "compiles must not share one manifest");
    assert!(shards.len() <= 256, "shards are bounded by the hash prefix");
    let _ = (one, two);
}

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
