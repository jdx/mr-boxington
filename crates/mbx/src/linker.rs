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

/// Startup objects and libc a GNU-style link resolves against.
#[cfg(target_os = "linux")]
const CRT_PROBES: &[&str] = &[
    "Scrt1.o",
    "crt1.o",
    "crti.o",
    "crtn.o",
    "crtbeginS.o",
    "crtendS.o",
    "libc.so.6",
];

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
        crt_objects: crt_objects(driver),
        sdk: sdk_identity(),
        deployment_target: std::env::var("MACOSX_DEPLOYMENT_TARGET").ok(),
    })
}

/// Version of the linker the driver selects, as opposed to the driver itself.
///
/// ld reports through stderr on some platforms and stdout on others, so both
/// are read; only the first line is kept, since later ones list supported
/// emulations that say nothing about the version.
fn linker_version(driver: &Path) -> Result<String> {
    let linker = run(driver, &["-print-prog-name=ld"])
        .map(|path| PathBuf::from(path.trim()))
        .unwrap_or_else(|_| PathBuf::from("ld"));
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
    Ok(text.lines().next().unwrap_or_default().trim().to_owned())
}

/// Hash the startup objects and libc the driver resolves.
///
/// A probe that does not resolve to a readable file is left out rather than
/// guessed at: the ones that do resolve already distinguish two hosts, and a
/// missing entry cannot make two different hosts look alike.
#[cfg(target_os = "linux")]
fn crt_objects(driver: &Path) -> BTreeMap<String, CacheDigest> {
    CRT_PROBES
        .iter()
        .filter_map(|name| {
            let resolved = run(driver, &[&format!("-print-file-name={name}")]).ok()?;
            let path = PathBuf::from(resolved.trim());
            // The driver echoes the name back when it cannot place it.
            let digest = path
                .is_absolute()
                .then(|| CacheDigest::blake3_file(&path))?;
            Some(((*name).to_owned(), digest.ok()?))
        })
        .collect()
}

/// macOS links against the SDK rather than loose startup objects, and the SDK
/// identity below covers it.
#[cfg(not(target_os = "linux"))]
fn crt_objects(_driver: &Path) -> BTreeMap<String, CacheDigest> {
    BTreeMap::new()
}

/// Identity of the SDK a link builds against, where the platform has one.
///
/// The build version is what changes when Apple ships new libraries under an
/// unchanged SDK version, so it is the half that matters most here.
#[cfg(target_os = "macos")]
fn sdk_identity() -> Option<String> {
    let version = xcrun(&["--sdk", "macosx", "--show-sdk-version"])?;
    let build = xcrun(&["--sdk", "macosx", "--show-sdk-build-version"])?;
    let path = std::env::var("SDKROOT")
        .ok()
        .or_else(|| xcrun(&["--sdk", "macosx", "--show-sdk-path"]))?;
    Some(format!("{path} {version} ({build})"))
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
fn sdk_identity() -> Option<String> {
    None
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
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
#[path = "linker_tests.rs"]
mod tests;
