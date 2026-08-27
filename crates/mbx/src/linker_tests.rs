use super::*;

/// The probe has to describe the host it actually ran on, because a key that
/// cannot tell two hosts apart is what would let one restore the other's
/// binary.
#[test]
fn the_probe_describes_this_host() {
    let Ok(identity) = identity_without_agent() else {
        // No `cc` on this machine; there is nothing a native link could be
        // keyed on, which is the bypass the adapter already handles.
        return;
    };

    assert!(Path::new(&identity.driver).is_absolute());
    // Nothing in the key may be blank: every host whose probe answered with
    // nothing would otherwise agree on the same empty value.
    assert!(!identity.driver_version.is_empty());
    assert!(!identity.linker_version.is_empty());
    assert!(
        identity.crt_objects.values().all(|d| !d.key().is_empty()),
        "{identity:?}"
    );
    // One line, not the driver's whole banner: the rest lists emulations and
    // search paths that say nothing about which linker this is.
    assert!(!identity.driver_version.contains('\n') || identity.driver_version.lines().count() > 1);

    // Whatever the probes could not place is absent from the key, so the
    // inputs a link cannot be described without have to be present -- two
    // hosts failing the same probe would otherwise agree on a key without
    // either of them having pinned what it stood for.
    #[cfg(target_os = "linux")]
    {
        assert!(
            STARTUP_PROBES
                .iter()
                .any(|name| identity.crt_objects.contains_key(*name)),
            "a resolved identity must pin what the link starts with: {identity:?}"
        );
        assert!(
            LIBC_PROBES
                .iter()
                .any(|name| identity.crt_objects.contains_key(*name)),
            "a resolved identity must pin the libc: {identity:?}"
        );
    }
    if cfg!(target_os = "macos") {
        assert!(
            identity.sdk.is_some(),
            "a macOS host should identify its SDK: {identity:?}"
        );
    }
}

/// The identity is recorded as canonical JSON, and read back the same way.
#[test]
fn the_identity_round_trips_through_its_recorded_form() {
    let identity = LinkerIdentity {
        driver: "/usr/bin/cc".into(),
        driver_version: "cc version 14".into(),
        linker_version: "GNU ld 2.42".into(),
        crt_objects: BTreeMap::from([("crt1.o".into(), CacheDigest::blake3(b"crt"))]),
        sdk: None,
        deployment_target: Some("11.0".into()),
    };

    let recorded = canonical_json(&identity).unwrap();

    assert_eq!(
        serde_json::from_slice::<LinkerIdentity>(&recorded).unwrap(),
        identity
    );
}

/// Probe this host without the agent round-trip the real path memoizes through.
fn identity_without_agent() -> Result<LinkerIdentity> {
    let driver = which::which("cc")?;
    probe(&driver)
}

/// The probe names are platform knowledge, but the rule about them is not:
/// what a key cannot describe a link without has to be present, whatever host
/// this test runs on.
#[test]
fn a_link_is_only_identified_once_its_essentials_resolve() {
    let directory = tempfile::tempdir().unwrap();
    let placed = |name: &str| {
        let path = directory.path().join(name);
        std::fs::write(&path, name).unwrap();
        path
    };
    let probes = FileProbes {
        startup: &["Scrt1.o", "crt1.o"],
        libc: &["libc.so.6", "libc.a"],
        rest: &["crti.o"],
    };

    // One of each essential is enough, and the optional one is not required.
    let resolved = probe_files(&probes, |name| {
        matches!(name, "crt1.o" | "libc.a").then(|| placed(name))
    })
    .unwrap();
    assert_eq!(
        resolved.keys().collect::<Vec<_>>(),
        ["crt1.o", "libc.a"],
        "only what resolved belongs in the key"
    );

    // A host whose libc the driver cannot place is a host whose links cannot
    // be told apart from another's.
    assert!(
        probe_files(&probes, |name| (name == "crt1.o").then(|| placed(name))).is_err(),
        "no libc should refuse"
    );
    assert!(
        probe_files(&probes, |name| (name == "libc.a").then(|| placed(name))).is_err(),
        "no startup object should refuse"
    );
    assert!(
        probe_files(&probes, |_| None).is_err(),
        "nothing should refuse"
    );

    // Two hosts placing different libcs cannot agree on a key.
    let glibc = probe_files(&probes, |name| {
        matches!(name, "crt1.o" | "libc.so.6").then(|| placed(name))
    })
    .unwrap();
    assert_ne!(glibc, resolved);
}

/// A platform with no loose objects to place -- macOS, whose SDK identity
/// covers what they would have pinned -- is not thereby unidentifiable.
#[test]
fn a_platform_that_places_nothing_is_still_identified() {
    let probes = FileProbes {
        startup: &[],
        libc: &[],
        rest: &[],
    };

    assert!(probe_files(&probes, |_| None).unwrap().is_empty());
}
