//! Registry build units copied into a profile a checkout has not built yet.
//!
//! A new checkout's first build asks the cache for every unit, which means
//! Cargo starts the mbx shim once per unit, and each one keys its inputs and
//! restores its outputs. From Cargo 1.100 a unit is one directory,
//! `<profile>/build/<package>/<hash>/`, holding its outputs, its fingerprint,
//! and a build script's run output. A registry or git package's unit hash and
//! fingerprint are the same in every checkout on a machine, so its directory
//! copied from another checkout's target directory is fresh in Cargo's eyes,
//! and Cargo skips the unit without starting rustc or the shim.
//!
//! The copy keeps what Cargo's freshness check depends on:
//!
//! - Only registry and git packages are copied. A path package's fingerprint
//!   trusts the modification times of its sources, so a unit copied from a
//!   checkout with different sources could pass as fresh.
//! - Modification times are copied, because Cargo rebuilds a unit whose
//!   dependency's outputs are newer than its own.
//! - Files are copied, which is a reflink where the filesystem supports one.
//!   Only read-only files are hard-linked: those are cache objects nothing
//!   writes in place. Fingerprints, build-script output, and `OUT_DIR` contents
//!   are rewritten in place by later builds, and sharing them would let one
//!   checkout's build change another's.
//! - Each unit is copied beside its final name and renamed into place, so an
//!   interrupted copy never leaves a fingerprint without its outputs.
//!
//! A unit copied from the other checkout that this build does not use is
//! removed later by [`crate::target_units`] like any other unused unit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where each unit is copied before it is renamed into place.
const STAGING_SUFFIX: &str = ".mbx-seeding-";
/// How long before a donor's last build a unit may have been read and still
/// count as one that build used. `relatime` refreshes access times daily.
const RECENT_USE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SeedOutcome {
    pub units: u64,
    /// The checkout the units came from, when any did.
    pub donor: Option<PathBuf>,
}

/// A target directory units may be copied from.
#[derive(Debug, Clone)]
pub(crate) struct Donor {
    pub directory: PathBuf,
    pub workspace_root: PathBuf,
    /// When a build last claimed the directory, in seconds since the epoch.
    pub updated_secs: u64,
}

/// The profile directories, relative to the target directory, a Cargo
/// command writes: the host profile, and the same profile below each
/// `--target` it names.
pub(crate) fn profile_directories(arguments: &[String]) -> Vec<PathBuf> {
    let profile = match crate::managed_linker::cargo_profile(arguments).as_str() {
        "dev" | "test" => "debug".to_owned(),
        "bench" => "release".to_owned(),
        other => other.to_owned(),
    };
    let mut directories = vec![PathBuf::from(&profile)];
    for target in crate::managed_linker::cargo_targets(arguments) {
        // A target given as a JSON file builds under the file's stem.
        if let Some(stem) = Path::new(target.trim()).file_stem() {
            directories.push(Path::new(stem).join(&profile));
        }
    }
    directories
}

/// The registry and git packages in a lockfile: those with a `source`, less
/// any name a path package also uses.
pub(crate) fn registry_packages(lockfile: &str) -> BTreeSet<String> {
    let Ok(lock) = lockfile.parse::<toml::Table>() else {
        return BTreeSet::new();
    };
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut registry = BTreeSet::new();
    let mut path = BTreeSet::new();
    for package in packages {
        let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        if package.get("source").is_some() {
            registry.insert(name.to_owned());
        } else {
            path.insert(name.to_owned());
        }
    }
    &registry - &path
}

/// Copy `packages`' units into each of `profiles`, relative to `view`, that
/// has no `build/` directory yet, from the first donor that has built it.
pub(crate) fn seed(
    view: &Path,
    profiles: &[PathBuf],
    packages: &BTreeSet<String>,
    donors: &[Donor],
) -> SeedOutcome {
    let mut outcome = SeedOutcome::default();
    if packages.is_empty() {
        return outcome;
    }
    for profile in profiles {
        let destination = view.join(profile);
        if destination.join("build").exists() {
            continue;
        }
        for donor in donors {
            let source = donor.directory.join(profile);
            if !has_units(&source.join("build")) {
                continue;
            }
            match seed_profile(&source, &destination, packages, donor.updated_secs) {
                Ok(Some(units)) => {
                    if units > 0 {
                        outcome.units += units;
                        outcome.donor = Some(donor.workspace_root.clone());
                    }
                    break;
                }
                // A build holds one of the two profiles; try the next donor.
                Ok(None) => continue,
                Err(error) => {
                    log::debug!(
                        "could not copy build units from {}: {error}",
                        source.display()
                    );
                    continue;
                }
            }
        }
    }
    outcome
}

/// Whether `build` holds any Cargo 1.100 unit, `<package>/<hash>/fingerprint`.
fn has_units(build: &Path) -> bool {
    subdirectories(build).iter().any(|package| {
        subdirectories(package)
            .iter()
            .any(|unit| unit.join("fingerprint").is_dir())
    })
}

/// Copy one profile's units, holding both profiles' Cargo locks so neither
/// checkout builds meanwhile. `None` when either is in use.
fn seed_profile(
    source: &Path,
    destination: &Path,
    packages: &BTreeSet<String>,
    donor_updated_secs: u64,
) -> std::io::Result<Option<u64>> {
    let Some(_source_lock) = try_lock(&source.join(".cargo-lock"))? else {
        return Ok(None);
    };
    std::fs::create_dir_all(destination)?;
    let Some(_destination_lock) = try_lock(&destination.join(".cargo-lock"))? else {
        return Ok(None);
    };
    // A build that got the lock first may have started this profile.
    if destination.join("build").exists() {
        return Ok(Some(0));
    }
    // Only the units the donor's latest build read, when access times say
    // which those are. Without them every unit of the package is copied, and
    // the ones this checkout does not use are collected later.
    let used_since = crate::target_units::access_times_tracked(source)
        .then(|| std::time::UNIX_EPOCH + Duration::from_secs(donor_updated_secs) - RECENT_USE);
    let mut units = 0;
    for package in subdirectories(&source.join("build")) {
        let Some(name) = package.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !packages.contains(name) {
            continue;
        }
        let target = destination.join("build").join(name);
        for unit in subdirectories(&package) {
            let fingerprint = unit.join("fingerprint");
            if !fingerprint.is_dir()
                || used_since
                    .is_some_and(|since| crate::target_units::last_use(&fingerprint) < since)
            {
                continue;
            }
            let Some(hash) = unit.file_name() else {
                continue;
            };
            match copy_unit(&unit, &target, hash) {
                Ok(()) => units += 1,
                Err(error) => log::debug!("did not copy {}: {error}", unit.display()),
            }
        }
    }
    // Build-script launchers in the copied units exec the mbx binary pinned
    // beside the profile, so a script Cargo later reruns needs it here too.
    let shims = source.join(".mbx-build-script-shims");
    if units > 0 && shims.is_dir() {
        let staging = destination.join(format!(
            ".mbx-build-script-shims{STAGING_SUFFIX}{}",
            std::process::id()
        ));
        match copy_tree(&shims, &staging)
            .and_then(|()| std::fs::rename(&staging, destination.join(".mbx-build-script-shims")))
        {
            Ok(()) => {}
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                log::debug!("did not copy the build-script shims: {error}");
            }
        }
    }
    Ok(Some(units))
}

fn try_lock(path: &Path) -> std::io::Result<Option<fslock::LockFile>> {
    let mut lock = fslock::LockFile::open(path)?;
    Ok(lock.try_lock()?.then_some(lock))
}

/// Copy `unit` to `package/hash` by way of a staging directory beside it.
fn copy_unit(unit: &Path, package: &Path, hash: &std::ffi::OsStr) -> std::io::Result<()> {
    std::fs::create_dir_all(package)?;
    let mut staging_name = hash.to_os_string();
    staging_name.push(format!("{STAGING_SUFFIX}{}", std::process::id()));
    let staging = package.join(staging_name);
    let copied =
        copy_tree(unit, &staging).and_then(|()| std::fs::rename(&staging, package.join(hash)));
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    copied
}

/// Copy a directory tree, keeping every file's modification time and leaving
/// the source's access times as they were.
fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_file() {
            copy_file(&from, &to, &entry.metadata()?)?;
        } else {
            return Err(std::io::Error::other(format!(
                "{} is neither a file nor a directory",
                from.display()
            )));
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path, metadata: &std::fs::Metadata) -> std::io::Result<()> {
    if metadata.permissions().readonly() && std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    let mut times = std::fs::FileTimes::new().set_modified(metadata.modified()?);
    if let Ok(accessed) = metadata.accessed() {
        // The copy read the source. Putting its access time back keeps this
        // copy from counting as a build using the donor's unit.
        let _ = std::fs::File::open(from)
            .and_then(|file| file.set_times(std::fs::FileTimes::new().set_accessed(accessed)));
        times = times.set_accessed(accessed);
    }
    // `copy` carries the mode, so a copied read-only file needs a handle that
    // does not ask for write access; an owner may set times through one on
    // Unix. Windows needs write access to change a file's times.
    let file = if metadata.permissions().readonly() {
        std::fs::File::open(to)?
    } else {
        std::fs::OpenOptions::new().write(true).open(to)?
    };
    file.set_times(times)
}

fn subdirectories(directory: &Path) -> Vec<PathBuf> {
    let Ok(listing) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    listing
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

#[cfg(test)]
#[path = "target_seed_tests.rs"]
mod tests;
