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
const IDENTITY_ENVIRONMENT: &[&str] = &[
    "SDKROOT",
    "MACOSX_DEPLOYMENT_TARGET",
    "LIB",
    "UCRTVersion",
    "UniversalCRTSdkDir",
    "VCToolsInstallDir",
    "VCToolsVersion",
    "WindowsSdkDir",
    "WindowsSDKVersion",
];

/// What the driver is asked to place, and which of it a key cannot do without.
///
/// The names are platform knowledge; the rule about them is not, so the two are
/// separated and only the names are chosen by `cfg`. A host this build cannot
/// run on is still a host whose logic can be tested here.
#[derive(Clone, Copy)]
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
const GNU_PROBES: FileProbes = FileProbes {
    startup: &["Scrt1.o", "crt1.o"],
    libc: &["libc.so.6", "libc.so", "libc.a", "libc.musl-x86_64.so.1"],
    // Both spellings of the constructor objects: GNU drivers place the `S`
    // variants for shared and position-independent links and the plain or `T`
    // ones for static and non-PIE links. Naming only the first pair would
    // leave a static link's own CRT out of the key, so changing it would not
    // change the key.
    rest: &[
        "crti.o",
        "crtn.o",
        "crtbegin.o",
        "crtbeginS.o",
        "crtbeginT.o",
        "crtend.o",
        "crtendS.o",
    ],
};

/// macOS links against the SDK rather than loose objects, and the SDK identity
/// below covers what those would have pinned.
const NO_PROBES: FileProbes = FileProbes {
    startup: &[],
    libc: &[],
    rest: &[],
};

/// What this platform asks the driver to place.
///
/// Chosen with `cfg!` rather than `#[cfg]` so that both tables are compiled
/// wherever this builds. A table only one platform compiles is a table only
/// that platform's CI can find a mistake in.
fn file_probes() -> FileProbes {
    if cfg!(target_os = "linux") {
        GNU_PROBES
    } else {
        NO_PROBES
    }
}

/// Describe the linker rustc will use for a native link on this host.
///
/// Memoized through the agent for the life of the build, since the answer is
/// the same for every link in it and the probes are several processes.
/// Describe the default linker, or a recognized Windows linker override.
pub(crate) fn identity_for(override_linker: Option<&Path>) -> Result<LinkerIdentity> {
    let driver = if let Some(linker) = override_linker {
        if !cfg!(windows)
            || !linker
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    matches!(
                        name.to_ascii_lowercase().as_str(),
                        "link" | "link.exe" | "lld-link" | "lld-link.exe"
                    )
                })
        {
            bail!("the selected linker is not one mbx can identify");
        }
        which::which(linker)
            .wrap_err_with(|| format!("failed to find linker `{}`", linker.display()))?
    } else if cfg!(windows) {
        msvc_tool("link.exe").wrap_err("failed to find the linker `link.exe`")?
    } else {
        which::which("cc").wrap_err("failed to find the linker driver `cc`")?
    };
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
    if cfg!(windows) {
        return probe_windows(driver);
    }
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

/// Bind a Windows link to the MSVC/LLVM linker, toolset, SDK, and CRT import
/// libraries selected by the developer environment.
fn probe_windows(linker: &Path) -> Result<LinkerIdentity> {
    let is_lld = linker
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("lld-link"));
    let linker_report = run_allowing_status(linker, if is_lld { &["--version"] } else { &["/?"] })?;
    if !is_lld && !linker_report.contains("Microsoft (R) Incremental Linker") {
        bail!(
            "{} is not the MSVC linker, so the link cannot be identified",
            linker.display()
        );
    }
    let linker_version = linker_report
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_owned();
    let cl =
        msvc_tool("cl.exe").wrap_err("failed to find `cl.exe` for the MSVC toolchain identity")?;
    let compiler_version = run_allowing_status(&cl, &["/Bv"])?;
    let crt_objects = windows_crt_objects()?;
    let sdk = windows_sdk_identity()?;
    Ok(LinkerIdentity {
        driver: linker
            .to_str()
            .ok_or_else(|| eyre::eyre!("the linker path is not valid UTF-8"))?
            .to_owned(),
        driver_version: compiler_version,
        linker_version,
        crt_objects,
        sdk: Some(sdk),
        deployment_target: None,
    })
}

/// Locate one MSVC tool in the developer environment rustc itself uses.
///
/// GitHub's Windows runners do not consistently put `cl.exe` on the Git Bash
/// `PATH`, and that path may contain an unrelated GNU `link.exe`. Visual
/// Studio's environment variables name the selected toolset unambiguously.
fn msvc_tool(name: &str) -> Result<PathBuf> {
    let tools = std::env::var_os("VCToolsInstallDir")
        .map(PathBuf::from)
        .or_else(visual_studio_tools_dir);
    if let Some(root) = tools {
        let host = std::env::var("VSCMD_ARG_HOST_ARCH")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(native_msvc_arch);
        let target = selected_msvc_arch();
        let candidate = root
            .join("bin")
            .join(format!("Host{host}"))
            .join(target)
            .join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Ok(which::which(name)?)
}

fn selected_msvc_arch() -> String {
    std::env::var("VSCMD_ARG_TGT_ARCH")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(native_msvc_arch)
}

/// Ask Visual Studio Installer which MSVC toolset rustc will use when no
/// developer-shell environment has been exported.
fn visual_studio_tools_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let program_files =
        std::env::var_os("ProgramFiles(x86)").or_else(|| std::env::var_os("ProgramFiles"))?;
    let vswhere = PathBuf::from(program_files)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    let output = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let installation = String::from_utf8(output.stdout).ok()?;
    let installation = PathBuf::from(installation.trim());
    let version = std::fs::read_to_string(
        installation.join("VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt"),
    )
    .ok()?;
    Some(installation.join("VC/Tools/MSVC").join(version.trim()))
}

fn native_msvc_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "x86",
        "aarch64" => "arm64",
        other => other,
    }
    .to_owned()
}

fn run_allowing_status(program: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .wrap_err_with(|| format!("failed to run {}", program.display()))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let reported = text.trim();
    if reported.is_empty() {
        bail!("{} {arguments:?} reported nothing", program.display());
    }
    Ok(reported.to_owned())
}

fn windows_sdk_identity() -> Result<String> {
    let mut values = ["VCToolsVersion", "WindowsSDKVersion", "UCRTVersion"]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| format!("{name}={value}"))
        })
        .collect::<Vec<_>>();
    if !values
        .iter()
        .any(|value| value.starts_with("VCToolsVersion="))
        && let Some(version) = std::env::var_os("VCToolsInstallDir")
            .map(PathBuf::from)
            .or_else(visual_studio_tools_dir)
            .and_then(|path| path.file_name().map(|name| name.to_owned()))
    {
        values.push(format!("VCToolsVersion={}", version.to_string_lossy()));
    }
    if !values
        .iter()
        .any(|value| value.starts_with("WindowsSDKVersion="))
        && let Some((_, version)) = windows_sdk_root_and_version()
    {
        values.push(format!("WindowsSDKVersion={version}"));
    }
    if values.len() < 2 {
        bail!("the MSVC toolset and Windows SDK versions could not be identified");
    }
    Ok(values.join("; "))
}

#[cfg(test)]
fn windows_sdk_identity_for(lookup: impl Fn(&str) -> Option<String>) -> Result<String> {
    let values = ["VCToolsVersion", "WindowsSDKVersion", "UCRTVersion"]
        .into_iter()
        .filter_map(|name| {
            lookup(name)
                .filter(|value| !value.is_empty())
                .map(|value| format!("{name}={value}"))
        })
        .collect::<Vec<_>>();
    if values.len() < 2 {
        bail!("the MSVC toolset and Windows SDK versions could not be identified");
    }
    Ok(values.join("; "))
}

fn windows_crt_objects() -> Result<BTreeMap<String, CacheDigest>> {
    let mut directories = std::env::var_os("LIB")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    let arch = selected_msvc_arch();
    if let Some(tools) = std::env::var_os("VCToolsInstallDir")
        .map(PathBuf::from)
        .or_else(visual_studio_tools_dir)
    {
        directories.push(tools.join("lib").join(&arch));
    }
    if let Some((sdk, version)) = windows_sdk_root_and_version() {
        directories.push(sdk.join("Lib").join(version).join("ucrt").join(&arch));
    }
    windows_crt_objects_in(&directories)
}

fn windows_sdk_root_and_version() -> Option<(PathBuf, String)> {
    let root = std::env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("ProgramFiles(x86)")
                .or_else(|| std::env::var_os("ProgramFiles"))
                .map(PathBuf::from)
                .map(|path| path.join("Windows Kits/10"))
        })?;
    let version = std::env::var("WindowsSDKVersion")
        .ok()
        .map(|version| version.trim_matches(['/', '\\']).to_owned())
        .filter(|version| !version.is_empty())
        .or_else(|| {
            let mut versions = std::fs::read_dir(root.join("Lib"))
                .ok()?
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("ucrt").is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect::<Vec<_>>();
            versions.sort();
            versions.pop()
        })?;
    Some((root, version))
}

fn windows_crt_objects_in(directories: &[PathBuf]) -> Result<BTreeMap<String, CacheDigest>> {
    let names = [
        "libcmt.lib",
        "libcmtd.lib",
        "msvcrt.lib",
        "msvcrtd.lib",
        "vcruntime.lib",
        "vcruntimed.lib",
        "ucrt.lib",
        "ucrtd.lib",
    ];
    let mut resolved = BTreeMap::<String, CacheDigest>::new();
    for name in names {
        if let Some(path) = directories
            .iter()
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
        {
            let digest = CacheDigest::blake3_file(&path)
                .wrap_err_with(|| format!("failed to hash CRT library {}", path.display()))?;
            resolved.insert(name.into(), digest);
        }
    }
    if !resolved.keys().any(|name| name.starts_with("vcruntime"))
        || !resolved.keys().any(|name| name.starts_with("ucrt"))
    {
        bail!("the MSVC and Universal CRT libraries could not both be identified");
    }
    Ok(resolved)
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
    probe_files(&file_probes(), |name| {
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
/// Written for every platform and gated with `cfg!` rather than `#[cfg]`, for
/// the same reason as the probe tables: code only one platform compiles is
/// code only that platform's CI can find a mistake in.
fn sdk_identity() -> Result<Option<String>> {
    sdk_identity_for(std::env::var("SDKROOT").ok())
}

/// Split from the environment it usually reads so that a test can ask about an
/// SDK without setting a variable every other test in the process would see.
fn sdk_identity_for(root: Option<String>) -> Result<Option<String>> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let describe = || {
        // Every question is asked of the SDK the link will actually use, which
        // `SDKROOT` names when it is set. Asking for `macosx` regardless would
        // report the default SDK's version beside another SDK's path, so the
        // key would describe an SDK no link was made against.
        let sdk = root.as_deref().unwrap_or("macosx");
        let version = xcrun(&["--sdk", sdk, "--show-sdk-version"])?;
        let build = xcrun(&["--sdk", sdk, "--show-sdk-build-version"])?;
        let path = match &root {
            Some(root) => root.clone(),
            None => xcrun(&["--sdk", sdk, "--show-sdk-path"])?,
        };
        Some(format!("{path} {version} ({build})"))
    };
    describe().map(Some).ok_or_else(|| {
        eyre::eyre!("the macOS SDK could not be identified, so its links cannot be either")
    })
}

fn xcrun(arguments: &[&str]) -> Option<String> {
    let output = Command::new("xcrun").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
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
