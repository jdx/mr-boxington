//! Identity of the toolchain that links a native program.
//!
//! rustc hands a native link to a driver -- `cc` on the platforms this tier
//! admits -- which chooses the real linker, the startup objects, and on macOS
//! the SDK. None of that appears in rustc dep-info, so a link is only
//! cacheable once it does appear in the key.
//!
//! What enters the key is identity rather than content wherever a compiler's
//! own identity is: `cc --version` names a toolchain as precisely as
//! `rustc -vV` names rustc, and far more cheaply than hashing the binary. The
//! startup objects are hashed instead, because nothing else pins the libc a
//! link resolves against.

use crate::session;
use eyre::{Context, Result, bail};
use mbx_cache_core::{AgentRequest, AgentResponse, CacheDigest, canonical_json};
use mbx_cache_rustc::LinkerIdentity;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Environment that selects what the probes below report.
const IDENTITY_ENVIRONMENT: &[&str] = &["SDKROOT", "MACOSX_DEPLOYMENT_TARGET"];

/// What the driver is asked to place, and which of it a key cannot do without.
///
/// The names are platform knowledge; the rule about them is not, so the two are
/// separated and only the names are chosen by `cfg`. A host this build cannot
/// run on is still a host whose logic can be tested here.
struct FileProbes {
    /// The object that starts a program. One of these must resolve, or nothing
    /// pins what the link began with.
    startup: &'static [&'static str],
    /// Names a libc goes by, across the C libraries and linkage modes this
    /// tier admits. One must resolve too: libc is the input a statically
    /// linked program carries inside it, so a key that does not pin it lets
    /// one distribution's binary restore onto another's.
    libc: &'static [&'static str],
    /// Everything else a link pulls in. Individually optional -- a toolchain
    /// that does not use one is not thereby unidentifiable.
    rest: &'static [&'static str],
}

/// GNU-style hosts link against loose objects the driver can place.
#[cfg(target_os = "linux")]
const FILE_PROBES: FileProbes = FileProbes {
    startup: &["Scrt1.o", "crt1.o"],
    libc: &["libc.so.6", "libc.so", "libc.a", "libc.musl-x86_64.so.1"],
    rest: &["crti.o", "crtn.o", "crtbeginS.o", "crtendS.o"],
};

/// macOS links against the SDK rather than loose objects, and the SDK identity
/// below covers what those would have pinned.
#[cfg(not(target_os = "linux"))]
const FILE_PROBES: FileProbes = FileProbes {
    startup: &[],
    libc: &[],
    rest: &[],
};

/// Describe the linker rustc will use for a native link on this host.
///
/// Memoized through the agent for the life of the build, since the answer is
/// the same for every link in it and the probes are several processes.
pub(crate) fn identity() -> Result<LinkerIdentity> {
    let driver = which::which("cc").wrap_err("failed to find the linker driver `cc`")?;
    let environment = IDENTITY_ENVIRONMENT
        .iter()
        .map(|name| ((*name).into(), std::env::var(name).ok()))
        .collect::<BTreeMap<_, _>>();
    if let Some(cached) = find_recorded(&driver, &environment)? {
        return Ok(cached);
    }
    let identity = probe(&driver)?;
    record(&driver, &environment, &identity)?;
    Ok(identity)
}

fn find_recorded(
    driver: &Path,
    environment: &BTreeMap<String, Option<String>>,
) -> Result<Option<LinkerIdentity>> {
    let responses = session::request_agent(&[AgentRequest::FindExecutableIdentity {
        executable: driver.to_path_buf(),
        environment: environment.clone(),
    }])?;
    let Some(AgentResponse::ExecutableIdentity { stdout }) = responses.into_iter().next() else {
        bail!("cache agent did not return the linker identity");
    };
    stdout
        .map(|stdout| serde_json::from_slice(&stdout).wrap_err("invalid recorded linker identity"))
        .transpose()
}

fn record(
    driver: &Path,
    environment: &BTreeMap<String, Option<String>>,
    identity: &LinkerIdentity,
) -> Result<()> {
    let responses = session::request_agent(&[AgentRequest::StoreExecutableIdentity {
        executable: driver.to_path_buf(),
        environment: environment.clone(),
        stdout: canonical_json(identity)?,
    }])?;
    match responses.into_iter().next() {
        Some(AgentResponse::ExecutableIdentity { .. }) => Ok(()),
        Some(AgentResponse::Error { message }) => bail!(message),
        _ => bail!("cache agent returned an unexpected linker identity response"),
    }
}

fn probe(driver: &Path) -> Result<LinkerIdentity> {
    Ok(LinkerIdentity {
        driver: driver
            .to_str()
            .ok_or_else(|| eyre::eyre!("the linker driver path is not valid UTF-8"))?
            .to_owned(),
        driver_version: run(driver, &["--version"])?,
        linker_version: linker_version(driver)?,
        crt_objects: crt_objects(driver)?,
        sdk: sdk_identity()?,
        deployment_target: std::env::var("MACOSX_DEPLOYMENT_TARGET").ok(),
    })
}

/// Version of the linker the driver selects, as opposed to the driver itself.
///
/// ld reports through stderr on some platforms and stdout on others, so both
/// are read; only the first line is kept, since later ones list supported
/// emulations that say nothing about the version.
fn linker_version(driver: &Path) -> Result<String> {
    // Asked of the driver rather than resolved from PATH: the `ld` a shell
    // would find is not necessarily the one this driver invokes, and a key
    // naming the wrong linker is worse than no key at all.
    let linker = PathBuf::from(
        run(driver, &["-print-prog-name=ld"]).wrap_err("the linker driver named no linker")?,
    );
    let output = Command::new(&linker)
        .arg("-v")
        .output()
        .wrap_err_with(|| format!("failed to query the linker {}", linker.display()))?;
    let combined = if output.stdout.is_empty() {
        output.stderr
    } else {
        output.stdout
    };
    let text = String::from_utf8_lossy(&combined);
    let version = text.lines().next().unwrap_or_default().trim();
    if version.is_empty() {
        bail!("{} reported no version", linker.display());
    }
    Ok(version.to_owned())
}

/// Hash the startup objects and libc the driver resolves.
///
/// A probe that does not resolve is left out of the map, so a host that
/// resolves a different set keys differently. That alone is not enough: two
/// hosts failing the *same* probe would agree on a key without ever pinning
/// what that probe stood for. So the inputs a link cannot be described without
/// -- a startup object and a libc -- have to resolve, and a host where neither
/// does gets no identity and no cached link.
fn crt_objects(driver: &Path) -> Result<BTreeMap<String, CacheDigest>> {
    probe_files(&FILE_PROBES, |name| {
        let resolved = run(driver, &[&format!("-print-file-name={name}")]).ok()?;
        let path = PathBuf::from(resolved.trim());
        // The driver echoes the name back when it cannot place it.
        path.is_absolute().then_some(path)
    })
}

/// Resolve each probe, hash what came back, and insist on the ones a key
/// cannot describe a link without.
fn probe_files(
    probes: &FileProbes,
    place: impl Fn(&str) -> Option<PathBuf>,
) -> Result<BTreeMap<String, CacheDigest>> {
    let resolved = probes
        .startup
        .iter()
        .chain(probes.libc)
        .chain(probes.rest)
        .filter_map(|name| {
            let digest = CacheDigest::blake3_file(&place(name)?).ok()?;
            Some(((*name).to_owned(), digest))
        })
        .collect::<BTreeMap<_, _>>();
    for (required, what) in [(probes.startup, "startup object"), (probes.libc, "libc")] {
        if !required.is_empty() && !required.iter().any(|name| resolved.contains_key(*name)) {
            bail!("the linker driver resolved no {what}, so its links cannot be identified");
        }
    }
    Ok(resolved)
}

/// Identity of the SDK a link builds against, where the platform has one.
///
/// The build version is what changes when Apple ships new libraries under an
/// unchanged SDK version, so it is the half that matters most here. A host that
/// cannot report it gets no identity rather than one that omits the SDK: two
/// such hosts would otherwise agree on a key while linking against different
/// system libraries.
#[cfg(target_os = "macos")]
fn sdk_identity() -> Result<Option<String>> {
    let describe = || {
        let version = xcrun(&["--sdk", "macosx", "--show-sdk-version"])?;
        let build = xcrun(&["--sdk", "macosx", "--show-sdk-build-version"])?;
        let path = std::env::var("SDKROOT")
            .ok()
            .or_else(|| xcrun(&["--sdk", "macosx", "--show-sdk-path"]))?;
        Some(format!("{path} {version} ({build})"))
    };
    describe().map(Some).ok_or_else(|| {
        eyre::eyre!("the macOS SDK could not be identified, so its links cannot be either")
    })
}

#[cfg(target_os = "macos")]
fn xcrun(arguments: &[&str]) -> Option<String> {
    let output = Command::new("xcrun").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn sdk_identity() -> Result<Option<String>> {
    Ok(None)
}

fn run(program: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .wrap_err_with(|| format!("failed to run {}", program.display()))?;
    if !output.status.success() {
        bail!(
            "{} {arguments:?} failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let reported = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    // A probe that answers with nothing describes nothing. Letting it through
    // would put an empty string in the key, which every host that failed the
    // same way would agree on.
    if reported.is_empty() {
        bail!("{} {arguments:?} reported nothing", program.display());
    }
    Ok(reported)
}

#[cfg(test)]
#[path = "linker_tests.rs"]
mod tests;
