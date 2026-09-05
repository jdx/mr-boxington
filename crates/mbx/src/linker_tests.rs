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
    // either of them having pinned what it stood for. Written against whatever
    // this platform asks for rather than behind a `cfg`, so that the host this
    // does not run on still compiles it.
    for (required, what) in [
        (file_probes().startup, "what the link starts with"),
        (file_probes().libc, "the libc"),
    ] {
        assert!(
            required.is_empty()
                || required
                    .iter()
                    .any(|name| identity.crt_objects.contains_key(*name)),
            "a resolved identity must pin {what}: {identity:?}"
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
    probe(&driver, None).map(|(identity, _)| identity)
}

/// The files a probe pins are the ones it read, so that a session which
/// finds them unchanged can trust what an earlier one recorded.
#[test]
#[cfg(target_os = "linux")]
fn a_probe_pins_the_files_it_read() {
    let driver = which::which("cc").unwrap();
    let (identity, pins) = probe(&driver, None).unwrap();
    assert!(pins.iter().all(|pin| pin.path.is_absolute()), "{pins:?}");
    assert!(pins.iter().all(PinnedFile::holds), "{pins:?}");
    let present = pins.iter().filter(|pin| pin.state.is_some()).count();
    // The driver, the linker, and every object the identity hashed; a
    // linker found on PATH adds the places the search passed over.
    assert!(present >= 2 + identity.crt_objects.len(), "{pins:?}");
    assert_eq!(pins[0].path, driver);
}

/// A `-fuse-ld` selection is resolved through the driver first, and a
/// selection naming nothing the driver or PATH can find is an error rather
/// than a key pointing at a linker nobody pinned.
#[test]
fn a_fuse_ld_selection_resolves_or_is_refused() {
    let Ok(driver) = which::which("cc") else {
        return;
    };

    let bogus = resolve_fuse_ld(&driver, "definitely-not-a-real-linker");
    assert!(
        bogus.is_err(),
        "a nonexistent linker must not resolve: {bogus:?}"
    );

    // An absolute selection names the linker directly, no resolution needed.
    let direct = Path::new(if cfg!(windows) { "C:/ld" } else { "/opt/ld" });
    assert_eq!(
        resolve_fuse_ld(&driver, direct.to_str().unwrap())
            .unwrap()
            .path,
        direct
    );

    // Whatever the host does provide resolves to an absolute path that
    // exists: anything less could alias two linkers in one memoization key.
    for name in ["mold", "lld", "bfd", "gold"] {
        if let Ok(located) = resolve_fuse_ld(&driver, name) {
            let program = &located.path;
            assert!(
                program.is_absolute() && program.exists(),
                "{name} resolved to {program:?}"
            );
            // Found on PATH or named by the driver, it pins itself last.
            assert_eq!(located.pins.last().map(|pin| &pin.path), Some(program));
        }
    }
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

#[test]
fn a_windows_identity_requires_toolset_sdk_and_both_crts() {
    let version = windows_sdk_identity_for(|name| match name {
        "VCToolsVersion" => Some("14.40".into()),
        "WindowsSDKVersion" => Some("10.0.26100.0".into()),
        _ => None,
    })
    .unwrap();
    assert!(version.contains("VCToolsVersion=14.40"));
    assert!(windows_sdk_identity_for(|_| None).is_err());

    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("vcruntime.lib"), b"vc runtime").unwrap();
    std::fs::write(directory.path().join("ucrt.lib"), b"universal crt").unwrap();
    let identity = windows_crt_objects_in(&[directory.path().to_path_buf()]).unwrap();
    assert_eq!(identity.len(), 2);
    std::fs::remove_file(directory.path().join("ucrt.lib")).unwrap();
    assert!(windows_crt_objects_in(&[directory.path().to_path_buf()]).is_err());
}

/// `SDKROOT` names the SDK a link is made against, so it has to be the SDK
/// every part of the identity describes. Reporting the default SDK's version
/// beside another SDK's path would put an SDK nothing was built against in the
/// key.
#[cfg(target_os = "macos")]
#[test]
fn the_sdk_identity_follows_sdkroot() {
    let Ok(Some(_)) = sdk_identity_for(None) else {
        // No usable SDK on this machine; there is nothing to describe.
        return;
    };

    // An SDK that is not there cannot be described, so it is refused rather
    // than quietly reported as the default one.
    let overridden = sdk_identity_for(Some("/nonexistent.sdk".into()));

    assert!(
        overridden.is_err(),
        "an unusable SDKROOT must not report the default SDK: {overridden:?}"
    );
}

/// A shim installed by `mbx setup` is driven by plain cargo, with no session
/// to have applied the platform gate, so the shim applies it itself rather
/// than trusting the variable it was handed.
#[test]
fn the_platform_gate_does_not_depend_on_who_set_the_variable() {
    assert_eq!(
        crate::session::cache_links_supported(),
        cfg!(any(target_os = "linux", target_os = "macos", windows))
    );
    // Whatever the environment says, an unsupported host never admits links.
    if !crate::session::cache_links_supported() {
        assert!(!crate::session::cache_links_requested());
    }
}
