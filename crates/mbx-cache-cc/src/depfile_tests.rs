use super::*;
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
fn timestamp_macro_tokens_in_any_read_file_bypass() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let source = write(root, "a.c", "int main(void) { return 0; }\n");
    let header = write(root, "a.h", "/* built on __DATE__ */\n");

    let clean =
        CcDiscoveredInputs::collect(root, BTreeSet::from([source.clone()]), BTreeSet::new())
            .expect("clean inputs should be discovered");
    assert_eq!(clean.inputs.len(), 1);

    let reason =
        CcDiscoveredInputs::collect(root, BTreeSet::from([source, header]), BTreeSet::new())
            .unwrap_err();
    assert_eq!(reason.kind(), "embedded-timestamp-macro");
}

#[test]
fn file_macro_alone_does_not_bypass() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let source = write(root, "a.c", "#define A() assert(__FILE__ != 0)\n");
    let discovered = CcDiscoveredInputs::collect(root, BTreeSet::from([source]), BTreeSet::new())
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
    let reason =
        CcDiscoveredInputs::collect(root, BTreeSet::from([source]), BTreeSet::new()).unwrap_err();
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
    )
    .expect("discovery");

    // A header that nothing read yet, but that would now shadow a later
    // lookup, must still change the manifest digest.
    write(&include, "shadowing.h", "int a(void);\n");
    let after = CcDiscoveredInputs::collect(
        root,
        BTreeSet::from([source]),
        BTreeSet::from([include.clone()]),
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
    )
    .expect("a directory that does not exist yet is not an error");
    assert_eq!(discovered.inputs.len(), 2);
}

#[test]
fn discovery_rejects_inputs_modified_during_compilation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let source = write(root, "a.c", "int a(void) { return 0; }\n");
    let discovered =
        CcDiscoveredInputs::collect(root, BTreeSet::from([source.clone()]), BTreeSet::new())
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
        CcDiscoveredInputs::collect(Path::new("relative"), BTreeSet::new(), BTreeSet::new())
            .unwrap_err()
            .kind(),
        "relative-working-directory"
    );
}
