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
    assert!(!identity.driver_version.is_empty());
    // One line, not the driver's whole banner: the rest lists emulations and
    // search paths that say nothing about which linker this is.
    assert!(!identity.driver_version.contains('\n') || identity.driver_version.lines().count() > 1);

    if cfg!(target_os = "linux") {
        assert!(
            identity.crt_objects.contains_key("crt1.o")
                || identity.crt_objects.contains_key("Scrt1.o"),
            "a GNU-style host should resolve its startup objects: {identity:?}"
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
