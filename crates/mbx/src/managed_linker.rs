//! Profile-aware linker selection and verified GitHub release installation.

use crate::config::{HttpSettings, LinkerSettings};
use eyre::{Context, Result, bail};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

const LINKER_ENV: &str = "MBX_LINKER";

/// A linker selected for one Cargo invocation.
#[derive(Debug, Clone)]
pub(crate) struct Selection {
    pub(crate) executable: PathBuf,
    pub(crate) starts_worker: bool,
}

/// Resolve and, where necessary, install the linker selected for this build.
pub(crate) fn resolve(
    settings: &LinkerSettings,
    cache_dir: &Path,
    http: &HttpSettings,
    cargo_arguments: &[String],
) -> Result<Option<Selection>> {
    let profile = cargo_profile(cargo_arguments);
    let environment = std::env::var(LINKER_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let target = if environment.is_none() && settings.profiles.contains_key(&profile) {
        cargo_target(cargo_arguments)?.or_else(|| host_target(cargo_arguments))
    } else {
        cargo_target(cargo_arguments)?
    };
    let configured = environment
        .or_else(|| settings.for_build(&profile, target.as_deref()))
        .unwrap_or_else(|| settings.default.clone());
    let spec = Spec::parse(&configured)?;
    match spec {
        Spec::System => Ok(None),
        Spec::Path(path) => Ok(Some(Selection {
            executable: which::which(&path)
                .wrap_err_with(|| format!("failed to find selected linker `{}`", path.display()))?,
            starts_worker: executable_name(&path) == "jdxld",
        })),
        Spec::RustLld => Ok(Some(Selection {
            executable: rust_lld(target.as_deref(), cache_dir, cargo_arguments)?,
            starts_worker: false,
        })),
        Spec::Managed { provider, version } => {
            let installed = install(provider, &version, cache_dir, http)?;
            Ok(Some(Selection {
                executable: installed,
                starts_worker: provider == Provider::Jdxld,
            }))
        }
    }
}

/// Validate a selector supplied by repository-owned workspace policy.
pub(crate) fn validate_workspace_selector(value: &str) -> Result<()> {
    match Spec::parse(value)? {
        Spec::Path(_) => bail!("workspace linker policy may not select an executable path"),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Jdxld,
    Mold,
    Wild,
}

impl Provider {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "jdxld" => Some(Self::Jdxld),
            "mold" => Some(Self::Mold),
            "wild" => Some(Self::Wild),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Jdxld => "jdxld",
            Self::Mold => "mold",
            Self::Wild => "wild",
        }
    }

    fn repository(self) -> &'static str {
        match self {
            Self::Jdxld => "jdx/jdxld",
            Self::Mold => "rui314/mold",
            Self::Wild => "wild-linker/wild",
        }
    }

    fn tag(self, version: &str) -> String {
        match self {
            Self::Wild => version.to_owned(),
            Self::Jdxld | Self::Mold => format!("v{version}"),
        }
    }

    fn asset(self, version: &str) -> Result<String> {
        let arch = std::env::consts::ARCH;
        match self {
            Self::Mold if cfg!(target_os = "linux") => {
                let arch = match arch {
                    "x86_64" | "aarch64" | "riscv64" | "s390x" => arch,
                    "powerpc64" if cfg!(target_endian = "little") => "ppc64le",
                    other => bail!("mold does not publish a managed binary for {other}-linux"),
                };
                Ok(format!("mold-{version}-{arch}-linux.tar.gz"))
            }
            Self::Wild if cfg!(target_os = "linux") => {
                let arch = match arch {
                    "x86_64" | "aarch64" => arch,
                    "riscv64" => "riscv64gc",
                    other => bail!("Wild does not publish a managed binary for {other}-linux"),
                };
                let environment = if cfg!(target_env = "musl") {
                    "musl"
                } else {
                    "gnu"
                };
                Ok(format!(
                    "wild-linker-{version}-{arch}-unknown-linux-{environment}.tar.gz"
                ))
            }
            Self::Jdxld if cfg!(target_os = "macos") && arch == "aarch64" => {
                Ok("jdxld-aarch64-apple-darwin.tar.gz".into())
            }
            _ => bail!(
                "{} does not publish a managed binary for {}-{}",
                self.name(),
                arch,
                std::env::consts::OS
            ),
        }
    }

    fn executable_suffix(self) -> String {
        match self {
            Self::Jdxld => "/jdxld".into(),
            Self::Mold => "/bin/mold".into(),
            Self::Wild => "/wild".into(),
        }
    }
}

enum Spec {
    System,
    RustLld,
    Path(PathBuf),
    Managed { provider: Provider, version: String },
}

impl Spec {
    fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value == "system" {
            return Ok(Self::System);
        }
        if matches!(value, "lld" | "rust-lld") {
            return Ok(Self::RustLld);
        }
        if let Some(path) = value.strip_prefix("path:") {
            if path.is_empty() {
                bail!("linker selector `path:` requires an executable path");
            }
            return Ok(Self::Path(path.into()));
        }
        let Some((name, version)) = value.split_once('@') else {
            bail!(
                "unknown linker selector `{value}`; use system, rust-lld, path:<executable>, or jdxld|mold|wild@<version>"
            );
        };
        let Some(provider) = Provider::parse(name) else {
            bail!("unknown managed linker `{name}`; supported linkers are jdxld, mold, and wild");
        };
        let version = version.trim_start_matches('v');
        if version.is_empty() || matches!(version, "." | ".." | "latest") {
            bail!("managed linker `{name}` must name an exact version");
        }
        if !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
        {
            bail!("managed linker `{name}` has an invalid version `{version}`");
        }
        Ok(Self::Managed {
            provider,
            version: version.to_owned(),
        })
    }
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

fn install(
    provider: Provider,
    version: &str,
    cache_dir: &Path,
    http: &HttpSettings,
) -> Result<PathBuf> {
    let install_dir = cache_dir
        .join("tools")
        .join(provider.name())
        .join(version)
        .join(host_install_name());
    let executable = install_dir.join(executable_filename(provider.name()));
    if executable.is_file() {
        return Ok(executable);
    }
    fs::create_dir_all(&install_dir)?;
    let lock_path = install_dir.join("install.lock");
    let mut lock = fslock::LockFile::open(&lock_path)?;
    lock.lock()?;
    if executable.is_file() {
        return Ok(executable);
    }

    let client = github_client(http)?;
    let tag = provider.tag(version);
    let release_url = format!(
        "https://api.github.com/repos/{}/releases/tags/{tag}",
        provider.repository()
    );
    let release: Release = client
        .get(&release_url)
        .send()
        .wrap_err_with(|| format!("failed to query {release_url}"))?
        .error_for_status()?
        .json()
        .wrap_err("invalid GitHub release response")?;
    let asset_name = provider.asset(version)?;
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            eyre::eyre!(
                "{} {version} has no release asset `{asset_name}`",
                provider.name()
            )
        })?;
    let expected = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or_else(|| eyre::eyre!("GitHub reported no SHA-256 digest for `{asset_name}`"))?;

    crate::session::note(&format!(
        "mbx[linker]: downloading {} {version} for {}",
        provider.name(),
        host_install_name()
    ));
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .wrap_err_with(|| format!("failed to download `{asset_name}`"))?
        .error_for_status()?;
    let archive_path = install_dir.join(format!(".{asset_name}.download"));
    let mut archive = File::create(&archive_path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        archive.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
    }
    archive.sync_all()?;
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("checksum mismatch for `{asset_name}`: expected {expected}, got {actual}");
    }

    let temporary = install_dir.join(format!(".{}.new", provider.name()));
    extract_executable(&archive_path, &temporary, &provider.executable_suffix())?;
    fs::rename(&temporary, &executable)?;
    let _ = fs::remove_file(&archive_path);
    Ok(executable)
}

fn github_client(http: &HttpSettings) -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("mr-boxington"));
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
    }
    Ok(Client::builder()
        .default_headers(headers)
        .connect_timeout(http.timeout)
        .timeout(http.download_timeout)
        .build()?)
}

fn extract_executable(archive: &Path, destination: &Path, suffix: &str) -> Result<()> {
    let input = BufReader::new(File::open(archive)?);
    let mut archive = tar::Archive::new(GzDecoder::new(input));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let normalized = path.to_string_lossy();
        if normalized == suffix.trim_start_matches('/') || normalized.ends_with(suffix) {
            let mut output = File::create(destination)?;
            std::io::copy(&mut entry, &mut output)?;
            output.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;
            }
            return Ok(());
        }
    }
    bail!("release archive did not contain an executable ending in `{suffix}`")
}

fn rust_lld(target: Option<&str>, cache_dir: &Path, cargo_arguments: &[String]) -> Result<PathBuf> {
    if cfg!(windows) {
        bail!("managed rust-lld selection on Windows is not supported yet");
    }
    let output = rustc_command(cargo_arguments)
        .args(["--print", "sysroot"])
        .output()
        .wrap_err("failed to find the active Rust sysroot")?;
    if !output.status.success() {
        bail!("rustc could not report its sysroot");
    }
    let sysroot = String::from_utf8(output.stdout)?;
    let host = host_target(cargo_arguments)
        .ok_or_else(|| eyre::eyre!("rustc did not report its host target"))?;
    let path = PathBuf::from(sysroot.trim())
        .join("lib/rustlib")
        .join(&host)
        .join("bin")
        .join("rust-lld");
    if !path.is_file() {
        bail!(
            "the active Rust toolchain has no bundled linker at `{}`",
            path.display()
        );
    }
    let link_target = target.unwrap_or(&host);
    let flavor = if link_target.contains("apple") {
        "ld64.lld"
    } else {
        "ld.lld"
    };
    let sysroot_key = hex::encode(Sha256::digest(sysroot.trim().as_bytes()));
    let shim_dir = cache_dir
        .join("tools/rust-lld")
        .join(host)
        .join(sysroot_key);
    fs::create_dir_all(&shim_dir)?;
    let shim = shim_dir.join(flavor);
    #[cfg(unix)]
    {
        let mut lock = fslock::LockFile::open(&shim_dir.join("install.lock"))?;
        lock.lock()?;
        if fs::read_link(&shim).is_ok_and(|target| target == path) {
            return Ok(shim);
        }
        let temporary = shim_dir.join(format!(".{flavor}.new"));
        let _ = fs::remove_file(&temporary);
        std::os::unix::fs::symlink(&path, &temporary)?;
        fs::rename(&temporary, &shim)?;
    }
    Ok(shim)
}

fn cargo_profile(arguments: &[String]) -> String {
    let mut arguments = arguments
        .iter()
        .take_while(|argument| argument.as_str() != "--");
    let mut subcommand = None;
    let mut profile = None;
    while let Some(argument) = arguments.next() {
        if argument == "--release" {
            profile = Some("release".to_owned());
        } else if argument == "--profile" {
            profile = arguments.next().cloned();
        } else if let Some(value) = argument.strip_prefix("--profile=") {
            profile = Some(value.to_owned());
        } else if matches!(argument.as_str(), "--config" | "--color" | "-Z") {
            let _ = arguments.next();
        } else if !argument.starts_with('-') && !argument.starts_with('+') && subcommand.is_none() {
            subcommand = Some(argument.as_str());
        }
    }
    profile.unwrap_or_else(|| {
        if subcommand == Some("bench") {
            "bench"
        } else {
            "dev"
        }
        .into()
    })
}

fn cargo_target(arguments: &[String]) -> Result<Option<String>> {
    let mut target = None;
    let mut arguments = arguments
        .iter()
        .take_while(|argument| argument.as_str() != "--");
    while let Some(argument) = arguments.next() {
        let value = if argument == "--target" {
            arguments.next().cloned()
        } else {
            argument.strip_prefix("--target=").map(str::to_owned)
        };
        if let Some(value) = value
            && target.replace(value).is_some()
        {
            bail!("managed linker selection supports one Cargo --target per invocation");
        }
    }
    Ok(target)
}

fn rustc_command(cargo_arguments: &[String]) -> Command {
    let mut command = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
    if let Some(toolchain) = cargo_arguments
        .first()
        .filter(|argument| argument.starts_with('+'))
    {
        command.arg(toolchain);
    }
    command
}

fn host_target(cargo_arguments: &[String]) -> Option<String> {
    let output = rustc_command(cargo_arguments).arg("-vV").output().ok()?;
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
}

fn host_install_name() -> String {
    host_target(&[])
        .unwrap_or_else(|| format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS))
}

fn executable_filename(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    }
}

fn executable_name(path: &Path) -> &str {
    path.file_stem().and_then(OsStr::to_str).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profiles_without_reading_program_arguments() {
        assert_eq!(cargo_profile(&["build".into()]), "dev");
        assert_eq!(
            cargo_profile(&["build".into(), "--release".into()]),
            "release"
        );
        assert_eq!(cargo_profile(&["bench".into()]), "bench");
        assert_eq!(
            cargo_profile(&["--config".into(), "build.jobs=1".into(), "bench".into(),]),
            "bench"
        );
        assert_eq!(
            cargo_profile(&["build".into(), "--profile=fast".into()]),
            "fast"
        );
        assert_eq!(
            cargo_profile(&["run".into(), "--".into(), "--profile=ignored".into()]),
            "dev"
        );
    }

    #[test]
    fn parses_supported_selectors() {
        assert!(matches!(Spec::parse("system").unwrap(), Spec::System));
        assert!(matches!(Spec::parse("lld").unwrap(), Spec::RustLld));
        assert!(matches!(
            Spec::parse("path:/tmp/ld").unwrap(),
            Spec::Path(_)
        ));
        assert!(matches!(
            Spec::parse("mold@2.42.0").unwrap(),
            Spec::Managed { provider: Provider::Mold, version } if version == "2.42.0"
        ));
        assert!(Spec::parse("wild@latest").is_err());
        assert!(Spec::parse("mold@../../escape").is_err());
        assert!(Spec::parse("mold@..").is_err());
        assert!(Spec::parse("unknown@1").is_err());
    }

    #[test]
    fn target_flag_is_unambiguous() {
        assert_eq!(cargo_target(&["build".into()]).unwrap(), None);
        assert_eq!(
            cargo_target(&["build".into(), "--target=aarch64-apple-darwin".into()]).unwrap(),
            Some("aarch64-apple-darwin".into())
        );
        assert!(
            cargo_target(&[
                "build".into(),
                "--target".into(),
                "one".into(),
                "--target=two".into(),
            ])
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rust_lld_repairs_a_dangling_cached_shim() {
        let cache = tempfile::tempdir().unwrap();
        let shim = rust_lld(None, cache.path(), &[]).unwrap();
        fs::remove_file(&shim).unwrap();
        std::os::unix::fs::symlink("/missing/rust-lld", &shim).unwrap();

        assert_eq!(rust_lld(None, cache.path(), &[]).unwrap(), shim);
        assert!(fs::read_link(shim).unwrap().is_file());
    }
}
