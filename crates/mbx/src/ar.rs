//! Native archive timestamp determinism.
//!
//! Apple's `ar` and `ranlib` stamp the current time into an archive's symbol
//! table and member headers. Re-running them over unchanged objects therefore
//! produces different bytes for the same content, and a build script that
//! hands Cargo such an archive hands mbx a changed input: the archive's digest
//! moves, so every action downstream of it misses even though nothing was
//! recompiled. CMake-based native dependencies hit this routinely because they
//! invoke `/usr/bin/ar` directly rather than going through the `cc` crate,
//! which already sets `ZERO_AR_DATE` for the implementations it knows about.
//!
//! `ZERO_AR_DATE=1` makes those tools write zeros instead, which is the
//! toolchain's own supported way to ask for a reproducible archive. mbx sets it
//! for the build scripts it runs, subject to [`ArDeterminism`].
//!
//! This only normalizes the timestamp fields. An archive whose *member
//! payloads* differ between builds is a different problem with a different
//! cause, and nothing here addresses it -- see the `archive-stability`
//! benchmark for telling the two apart.

/// The environment variable Apple's archive tools read.
pub(crate) const ZERO_AR_DATE: &str = "ZERO_AR_DATE";

/// Cargo sets this for every build script it runs.
pub(crate) const PROFILE: &str = "PROFILE";

/// When mbx normalizes native archive timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ArDeterminism {
    /// Normalize for every profile except `release`.
    ///
    /// The cache misses this prevents are worst in the edit-build loop, where
    /// fresh targets are common and a moved archive digest costs a wave of
    /// downstream recompilation. A release build is also the one whose exact
    /// output bytes someone may be signing, publishing, or comparing against a
    /// previous artifact, so the default leaves those exactly as the host
    /// toolchain produced them rather than quietly changing them.
    #[default]
    Auto,
    /// Normalize for every profile, including `release`.
    Always,
    /// Never normalize; leave the toolchain's own behavior alone.
    Off,
}

impl ArDeterminism {
    /// Parse a configured value, falling back to the default when unrecognized.
    ///
    /// Configuration already constrains this to the documented choices; an
    /// unknown value here means some other caller, and the default is the safe
    /// reading of one.
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "always" => Self::Always,
            "off" => Self::Off,
            _ => Self::Auto,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Off => "off",
        }
    }
}

/// Whether mbx should set `ZERO_AR_DATE` for a build script.
///
/// `profile` is Cargo's `PROFILE` for the build script, and `already_set` is
/// whether the environment already carries `ZERO_AR_DATE`. An explicit setting
/// from the user wins outright: they have said what they want the archive tools
/// to do, and mbx overriding that would make their own configuration
/// unpredictable.
pub(crate) fn normalizes(mode: ArDeterminism, profile: Option<&str>, already_set: bool) -> bool {
    if already_set {
        return false;
    }
    match mode {
        ArDeterminism::Off => false,
        ArDeterminism::Always => true,
        ArDeterminism::Auto => profile != Some("release"),
    }
}

/// Whether this host's archive tools stamp a timestamp mbx would normalize.
///
/// Answered by running the tools rather than by assuming from the platform:
/// which `ar` and `ranlib` are on `PATH` is a property of the installation, and
/// a toolchain that is already deterministic needs no help. Both archives are
/// built from the same bytes in the same directory and differ only in whether
/// `ZERO_AR_DATE` is set, so a difference between them is the timestamp and
/// nothing else. No delay is needed: the comparison is zeroed-versus-stamped,
/// not one clock reading against a later one.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Probe {
    /// The tools stamp a timestamp, and `ZERO_AR_DATE` suppresses it.
    Stamps,
    /// The tools already produce identical bytes for identical input.
    Deterministic,
    /// The probe could not run; the reason is for a human, not for a decision.
    Unavailable(String),
}

pub(crate) fn probe() -> Probe {
    match probe_inner() {
        Ok(true) => Probe::Stamps,
        Ok(false) => Probe::Deterministic,
        Err(error) => Probe::Unavailable(error.to_string()),
    }
}

fn probe_inner() -> std::io::Result<bool> {
    use std::process::{Command, Stdio};

    let directory = tempfile::Builder::new().prefix("mbx-ar-probe-").tempdir()?;
    let member = directory.path().join("member");
    // Any file serves. `ranlib` warns that it is not an object and still
    // rewrites the header this probe is reading, so the archive stays valid
    // for the comparison without needing a compiler on the host.
    std::fs::write(&member, b"mbx archive probe")?;

    let run = |program: &str, arguments: &[&std::ffi::OsStr], zero: bool| -> std::io::Result<()> {
        let mut command = Command::new(program);
        command
            .args(arguments)
            .current_dir(directory.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if zero {
            command.env(ZERO_AR_DATE, "1");
        } else {
            command.env_remove(ZERO_AR_DATE);
        }
        let status = command.status()?;
        if status.success() {
            return Ok(());
        }
        Err(std::io::Error::other(format!(
            "{program} exited with {status}"
        )))
    };

    let mut digests = Vec::new();
    for (name, zero) in [("zeroed.a", true), ("stamped.a", false)] {
        let archive = directory.path().join(name);
        run(
            "ar",
            &["rc".as_ref(), archive.as_os_str(), member.as_os_str()],
            zero,
        )?;
        run("ranlib", &[archive.as_os_str()], zero)?;
        digests.push(std::fs::read(&archive)?);
    }
    Ok(digests[0] != digests[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_setting_from_the_user_is_never_overridden() {
        for mode in [
            ArDeterminism::Auto,
            ArDeterminism::Always,
            ArDeterminism::Off,
        ] {
            assert!(!normalizes(mode, Some("debug"), true));
            assert!(!normalizes(mode, Some("release"), true));
        }
    }

    #[test]
    fn auto_leaves_release_archives_as_the_toolchain_made_them() {
        assert!(!normalizes(ArDeterminism::Auto, Some("release"), false));
        assert!(normalizes(ArDeterminism::Auto, Some("debug"), false));
        assert!(normalizes(ArDeterminism::Auto, Some("bench"), false));
    }

    #[test]
    fn a_missing_profile_is_treated_as_a_non_release_build() {
        // Cargo always sets PROFILE, so this is a build script reached some
        // other way. Normalizing is the behavior that keeps the cache honest.
        assert!(normalizes(ArDeterminism::Auto, None, false));
    }

    #[test]
    fn always_and_off_ignore_the_profile() {
        assert!(normalizes(ArDeterminism::Always, Some("release"), false));
        assert!(normalizes(ArDeterminism::Always, Some("debug"), false));
        assert!(!normalizes(ArDeterminism::Off, Some("release"), false));
        assert!(!normalizes(ArDeterminism::Off, Some("debug"), false));
    }

    #[test]
    fn unrecognized_settings_parse_to_the_default() {
        assert_eq!(ArDeterminism::parse("always"), ArDeterminism::Always);
        assert_eq!(ArDeterminism::parse("off"), ArDeterminism::Off);
        assert_eq!(ArDeterminism::parse("auto"), ArDeterminism::Auto);
        assert_eq!(ArDeterminism::parse("nonsense"), ArDeterminism::Auto);
        assert_eq!(ArDeterminism::default(), ArDeterminism::Auto);
    }

    #[test]
    fn every_mode_round_trips_through_its_configured_spelling() {
        for mode in [
            ArDeterminism::Auto,
            ArDeterminism::Always,
            ArDeterminism::Off,
        ] {
            assert_eq!(ArDeterminism::parse(mode.as_str()), mode);
        }
    }
}
