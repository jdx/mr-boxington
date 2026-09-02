use super::*;
use crate::manifest_snapshot;
use mbx_cache_core::NoFileDigestCache;
use std::collections::BTreeSet;

#[test]
fn parses_gnu_depfiles_with_continuations_escaped_spaces_and_dollar_signs() {
    let depfile = "entropy.o: lib/entropy.c \\\n  lib/entropy.h \\\n  /usr/include/stdio.h\n";
    let parsed = CcDepfile::parse(depfile).expect("depfile should parse");
    assert_eq!(
        parsed.files,
        [
            PathBuf::from("lib/entropy.c"),
            PathBuf::from("lib/entropy.h"),
            PathBuf::from("/usr/include/stdio.h"),
        ]
    );

    let escaped = "a.o: some\\ dir/a.c price$$.h\n";
    let parsed = CcDepfile::parse(escaped).expect("escapes should parse");
    assert_eq!(
        parsed.files,
        [PathBuf::from("some dir/a.c"), PathBuf::from("price$.h")]
    );
}

#[test]
fn only_the_first_rule_contributes_prerequisites() {
    let depfile = "a.o: a.c a.h\n\na.h:\n";
    let parsed = CcDepfile::parse(depfile).expect("depfile should parse");
    assert_eq!(parsed.files, [PathBuf::from("a.c"), PathBuf::from("a.h")]);
}

#[test]
fn malformed_depfiles_bypass_caching() {
    for contents in [
        "no rule here\n",
        "a.o: a.c\\",
        "a.o: we\\?ird.c\n",
        "a.o: $(VAR).c\n",
    ] {
        assert_eq!(
            CcDepfile::parse(contents).unwrap_err().kind(),
            "malformed-depfile",
            "{contents:?} should bypass"
        );
    }
}

fn write(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(&path, contents).expect("write file");
    path
}

#[test]
fn recursive_manifests_drop_directories_an_ancestor_already_covers() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let root = workspace.path().join("include");
    let generated = workspace.path().join("generated");
    std::fs::create_dir_all(root.join("crypto/fipsmodule")).expect("nested include directory");
    std::fs::create_dir_all(&generated).expect("separate include directory");
    let directories = BTreeSet::from([
        root.join("crypto/fipsmodule"),
        root.clone(),
        root.join("crypto"),
        generated.clone(),
    ]);

    assert_eq!(minimal_manifest_directories(directories), [generated, root]);
}

#[test]
fn a_parent_path_does_not_cover_a_directory_that_escapes_through_dot_dot() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let include = workspace.path().join("include");
    let sibling = workspace.path().join("sibling");
    std::fs::create_dir_all(&include).expect("include directory");
    std::fs::create_dir_all(&sibling).expect("sibling directory");
    let escaped = include.join("../sibling");

    assert_eq!(
        minimal_manifest_directories(BTreeSet::from([include.clone(), escaped.clone()])),
        [include, escaped]
    );
}

#[cfg(unix)]
#[test]
fn a_parent_manifest_does_not_claim_a_directory_reached_through_a_symlink() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().expect("tempdir");
    let include = workspace.path().join("include");
    let generated = workspace.path().join("generated");
    std::fs::create_dir_all(&include).expect("include directory");
    std::fs::create_dir_all(generated.join("nested")).expect("generated directory");
    symlink(&generated, include.join("linked")).expect("directory symlink");
    let linked = include.join("linked/nested");

    assert_eq!(
        minimal_manifest_directories(BTreeSet::from([include.clone(), linked.clone()])),
        [include, linked]
    );
}

#[test]
fn timestamp_macro_tokens_in_any_read_file_bypass() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let source = write(root, "a.c", "int main(void) { return 0; }\n");
    let header = write(root, "a.h", "/* built on __DATE__ */\n");

    let clean = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source.clone()]),
        BTreeSet::new(),
        &NoFileDigestCache,
    )
    .expect("clean inputs should be discovered");
    assert_eq!(clean.inputs.len(), 1);

    let reason = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source, header]),
        BTreeSet::new(),
        &NoFileDigestCache,
    )
    .unwrap_err();
    assert_eq!(reason.kind(), "embedded-timestamp-macro");
}

#[test]
fn file_macro_alone_does_not_bypass() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let source = write(root, "a.c", "#define A() assert(__FILE__ != 0)\n");
    let discovered = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source]),
        BTreeSet::new(),
        &NoFileDigestCache,
    )
    .expect("__FILE__ is not a timestamp macro");
    assert_eq!(discovered.inputs.len(), 1);
}

#[test]
fn a_timestamp_macro_split_across_a_read_boundary_is_still_found() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let mut contents = "a".repeat(SCAN_CHUNK_BYTES - 4);
    contents.push_str("__TIMESTAMP__");
    let source = write(root, "a.c", &contents);
    let reason = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source]),
        BTreeSet::new(),
        &NoFileDigestCache,
    )
    .unwrap_err();
    assert_eq!(reason.kind(), "embedded-timestamp-macro");
}

#[test]
fn include_directory_manifests_change_the_key_when_a_shadowing_header_appears() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let source = write(root, "src/a.c", "int a(void) { return 0; }\n");
    let include = root.join("include");
    std::fs::create_dir_all(&include).expect("create include dir");
    write(&include, "used.h", "int a(void);\n");

    let before = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source.clone()]),
        BTreeSet::from([include.clone()]),
        &NoFileDigestCache,
    )
    .expect("discovery");

    // A header that nothing read yet, but that would now shadow a later
    // lookup, must still change the manifest digest.
    write(&include, "shadowing.h", "int a(void);\n");
    let after = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source]),
        BTreeSet::from([include.clone()]),
        &NoFileDigestCache,
    )
    .expect("discovery");

    let manifest = |inputs: &CcDiscoveredInputs| {
        inputs
            .inputs
            .iter()
            .find(|input| is_manifest_input(&input.path))
            .expect("manifest input")
            .digest
            .clone()
    };
    assert_ne!(manifest(&before), manifest(&after));
    // The file inputs themselves are untouched, so only the manifest moved.
    assert_eq!(
        before.files().next().expect("file").digest,
        after.files().next().expect("file").digest,
    );
}

#[test]
fn a_missing_include_directory_has_an_empty_manifest_rather_than_an_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let source = write(root, "a.c", "int a(void) { return 0; }\n");
    let discovered = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source]),
        BTreeSet::from([root.join("absent")]),
        &NoFileDigestCache,
    )
    .expect("a directory that does not exist yet is not an error");
    assert_eq!(discovered.inputs.len(), 2);
}

#[test]
fn discovery_rejects_inputs_modified_during_compilation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let source = write(root, "a.c", "int a(void) { return 0; }\n");
    let discovered = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source.clone()]),
        BTreeSet::new(),
        &NoFileDigestCache,
    )
    .expect("discovery");

    // A file written before the compile started is what produced the object.
    assert!(
        discovered
            .verify_not_modified_since(SystemTime::now() + std::time::Duration::from_secs(60))
            .is_ok()
    );
    // A file whose timestamp lands at or after the compile start could have
    // been written by something racing the compiler, so it cannot be trusted.
    assert_eq!(
        discovered
            .verify_not_modified_since(SystemTime::UNIX_EPOCH)
            .unwrap_err()
            .kind(),
        "input-modified-during-compilation"
    );

    std::fs::write(&source, "int a(void) { return 1; }\n").expect("rewrite");
    assert_eq!(discovered.verify().unwrap_err().kind(), "input-changed");
}

#[test]
fn discovery_requires_an_absolute_working_directory() {
    assert_eq!(
        CcDiscoveredInputs::collect(
            Path::new("relative"),
            BTreeSet::new(),
            BTreeSet::new(),
            &NoFileDigestCache,
        )
        .unwrap_err()
        .kind(),
        "relative-working-directory"
    );
}

/// A build writes objects, dependency files, and archives into the same
/// directories a generated header lives in. None of those can shadow an
/// include, and counting them would make the key depend on how many sibling
/// compilations had finished by the time this one was discovered.
#[test]
fn manifests_ignore_files_that_could_never_shadow_an_include() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let include = root.join("include");
    std::fs::create_dir_all(&include).expect("create include dir");
    write(&include, "config.h", "#define V 1\n");

    let manifest = |directory: &Path| {
        manifest_snapshot(&BTreeSet::from([directory.to_path_buf()]))
            .expect("snapshot")
            .remove(directory)
            .expect("directory manifest")
    };
    let before = manifest(&include);

    // A sibling object landing beside the generated header must not move the
    // key.
    write(&include, "a.o", "\0object\0");
    write(&include, "a.d", "a.o: a.c\n");
    write(&include, "libfoo.a", "!<arch>\n");
    assert_eq!(before, manifest(&include));

    // A header appearing there still does.
    write(&include, "shadowing.h", "#define V 2\n");
    assert_ne!(before, manifest(&include));
}

/// A source can satisfy an `#include` too -- `#include "generated.c"` is
/// unusual but legal -- so one appearing in a search directory has to move the
/// key. Counting them is safe because a build writes its generated sources
/// before compiling, unlike the objects it emits during.
#[test]
fn a_source_appearing_in_a_search_directory_changes_the_manifest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let include = directory.path().join("include");
    std::fs::create_dir_all(&include).expect("create include dir");
    write(&include, "used.h", "int a(void);\n");

    let manifest = |directory: &Path| {
        manifest_snapshot(&BTreeSet::from([directory.to_path_buf()]))
            .expect("snapshot")
            .remove(directory)
            .expect("directory manifest")
    };
    let before = manifest(&include);
    for source in ["shadow.c", "shadow.cpp", "shadow.cc", "shadow.cxx"] {
        write(&include, source, "int a(void) { return 0; }\n");
        assert_ne!(
            before,
            manifest(&include),
            "{source} can satisfy an #include and must move the key"
        );
        std::fs::remove_file(include.join(source)).expect("remove");
    }
}

/// Extensionless names are how C++ standard headers are spelled, and projects
/// ship their own, so they stay in the manifest.
#[test]
fn extensionless_names_count_as_includable() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let include = root.join("include");
    std::fs::create_dir_all(&include).expect("create include dir");
    let manifest = |directory: &Path| {
        manifest_snapshot(&BTreeSet::from([directory.to_path_buf()]))
            .expect("snapshot")
            .remove(directory)
            .expect("directory manifest")
    };
    let empty = manifest(&include);
    write(&include, "vector", "#pragma once\n");
    assert_ne!(empty, manifest(&include));
}

/// A precompiled header answers an `#include` without being named by one:
/// GCC prefers `foo.h.gch` over `foo.h` with nothing on the command line to
/// say so. Nothing else in the adapter can see that substitution, so the
/// manifest has to.
#[test]
fn a_precompiled_header_appearing_beside_its_header_changes_the_manifest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let include = directory.path().join("include");
    std::fs::create_dir_all(&include).expect("create include dir");
    write(&include, "foo.h", "#define V 1\n");

    let manifest = |directory: &Path| {
        manifest_snapshot(&BTreeSet::from([directory.to_path_buf()]))
            .expect("snapshot")
            .remove(directory)
            .expect("directory manifest")
    };
    let before = manifest(&include);
    for precompiled in ["foo.h.gch", "foo.h.pch"] {
        write(&include, precompiled, "\0precompiled\0");
        assert_ne!(
            before,
            manifest(&include),
            "{precompiled} can answer an #include and must move the key"
        );
        std::fs::remove_file(include.join(precompiled)).expect("remove");
    }
}

/// A ledger that vouches for one cc input identity.
struct VouchingLedger {
    known: mbx_cache_core::FileIdentity,
    digest: CacheDigest,
}

impl mbx_cache_core::FileDigestCache for VouchingLedger {
    fn find(
        &self,
        scope: mbx_cache_core::FileDigestScope,
        files: &[mbx_cache_core::FileIdentity],
    ) -> Vec<Option<CacheDigest>> {
        assert_eq!(scope, mbx_cache_core::FileDigestScope::CcInput);
        files
            .iter()
            .map(|file| (*file == self.known).then(|| self.digest.clone()))
            .collect()
    }

    fn record(
        &self,
        _scope: mbx_cache_core::FileDigestScope,
        _entries: Vec<mbx_cache_core::RecordedFileDigest>,
    ) {
    }
}

#[test]
fn a_vouched_cc_input_skips_the_scan_it_already_passed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    // The macro token would bypass on a first encounter; a cc-scope ledger
    // entry says this exact identity already passed the scan, so neither the
    // scan nor the hash reads the file again.
    let source = write(root, "a.c", "/* __DATE__ */ int main(void) { return 0; }\n");
    let metadata = std::fs::metadata(&source).expect("metadata");
    let digest = CacheDigest {
        algorithm: "blake3".into(),
        hash: "d".repeat(64),
        size: metadata.len(),
    };
    let ledger = VouchingLedger {
        known: mbx_cache_core::FileIdentity::describe(&source, &metadata).expect("identity"),
        digest: digest.clone(),
    };

    let discovered = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source.clone()]),
        BTreeSet::new(),
        &ledger,
    )
    .expect("the vouched identity answers without a scan");
    assert_eq!(discovered.inputs.len(), 1);
    assert_eq!(discovered.inputs[0].digest, digest);

    // Rewrite the file to a new length: the identity no longer matches, so
    // the scan runs and the macro token bypasses again.
    std::fs::write(&source, "/* __DATE__ */ int main(void) { return 12345; }\n").expect("rewrite");
    let reason =
        CcDiscoveredInputs::collect(root, BTreeSet::from([source]), BTreeSet::new(), &ledger)
            .unwrap_err();
    assert_eq!(reason.kind(), "embedded-timestamp-macro");
}

#[test]
fn parses_msvc_source_dependencies() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("deps.json");
    std::fs::write(
        &path,
        r#"{"Version":"1.2","Data":{"Source":"src\\a.c","ProvidedModule":"","ImportedModules":[],"Includes":["include\\a.h","C:\\SDK\\stdio.h"]}}"#,
    )
    .expect("write dependency JSON");

    let dependencies = CcDepfile::read_msvc(&path).expect("parse");
    assert_eq!(
        dependencies.files,
        [
            PathBuf::from("include\\a.h"),
            PathBuf::from("C:\\SDK\\stdio.h")
        ]
    );
}

/// A ledger that answers one known identity with a fixed digest.
struct SentinelLedger {
    known: FileIdentity,
    digest: CacheDigest,
}

impl FileDigestCache for SentinelLedger {
    fn find(&self, _scope: FileDigestScope, files: &[FileIdentity]) -> Vec<Option<CacheDigest>> {
        files
            .iter()
            .map(|file| (*file == self.known).then(|| self.digest.clone()))
            .collect()
    }

    fn record(&self, _scope: FileDigestScope, _entries: Vec<RecordedFileDigest>) {}
}

/// Verification confirms an input by the identity `collect` recorded rather
/// than by reading it again, and reads it again once that identity has moved.
/// Only where the identity carries a change time.
#[cfg(unix)]
#[test]
fn verification_trusts_an_unchanged_identity_and_rereads_a_changed_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let header = write(root, "a.h", "int a(void);\n");
    let metadata = std::fs::metadata(&header).expect("metadata");
    // Hashing could never produce the sentinel, so a verify that passes can
    // only have trusted the identity.
    let sentinel = CacheDigest {
        algorithm: "blake3".into(),
        hash: "c".repeat(64),
        size: metadata.len(),
    };
    let ledger = SentinelLedger {
        known: FileIdentity::describe(&header, &metadata).expect("identity"),
        digest: sentinel.clone(),
    };
    let discovered = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([header.clone()]),
        BTreeSet::from([root.to_path_buf()]),
        &ledger,
    )
    .expect("discovery");
    assert_eq!(discovered.files().next().expect("input").digest, sentinel);

    discovered.verify().expect("an unchanged identity verifies");

    // Same length, new bytes: the write moves the identity, so the file is
    // read again and the sentinel no longer describes it.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&header, "int b(void);\n").expect("rewrite");
    assert_eq!(discovered.verify().unwrap_err().kind(), "input-changed");
}

/// The list the shim writes for a caller reads back through the same parser,
/// escapes exactly what the parser unescapes, quotes only the targets that
/// asked for it, and gives every header but the source a phony rule when `-MP`
/// asked.
#[test]
fn a_rendered_caller_depfile_reads_back_and_quotes_like_the_driver() {
    let files = vec![
        PathBuf::from("/src/a.c"),
        PathBuf::from("/src/my dir/a.h"),
        PathBuf::from("/src/#odd$/b.h"),
    ];
    let targets = [
        crate::DepfileTarget {
            name: "$(OBJ)".into(),
            quoted: false,
        },
        crate::DepfileTarget {
            name: "out/a b.o".into(),
            quoted: true,
        },
    ];

    let rendered = CcDepfile::render(&targets, &files, Path::new("/src/a.c"), true);

    assert_eq!(
        rendered,
        "$(OBJ) out/a\\ b.o: \\\n /src/a.c \\\n /src/my\\ dir/a.h \\\n /src/\\#odd$$/b.h\n/src/my\\ dir/a.h:\n/src/\\#odd$$/b.h:\n"
    );
    assert_eq!(CcDepfile::parse(&rendered).unwrap().files, files);

    let plain = CcDepfile::render(&targets[1..], &files[..1], Path::new("/src/a.c"), false);
    assert_eq!(plain, "out/a\\ b.o: \\\n /src/a.c\n");
}
