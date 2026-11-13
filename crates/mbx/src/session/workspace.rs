//! Cargo workspace membership for invocation deduplication.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MEMBERS_ENV: &str = "MBX_WORKSPACE_MEMBERS";

/// Pass the existing Cargo metadata probe's member manifests to the shims.
/// Directory containment cannot distinguish members from vendored dependencies.
pub(crate) fn record_members(
    members: Option<&[PathBuf]>,
    session: &Path,
    environment: &mut BTreeMap<String, String>,
) {
    // A nested session must not inherit its parent's membership snapshot.
    environment.insert(MEMBERS_ENV.into(), String::new());
    let Some(members) = members else {
        return;
    };
    let recorded = (|| -> eyre::Result<()> {
        let path = session.join("workspace-members.json");
        crate::util::write_atomic(&path, &serde_json::to_vec(members)?)?;
        environment.insert(MEMBERS_ENV.into(), path.to_string_lossy().into_owned());
        Ok(())
    })();
    if let Err(error) = recorded {
        log::debug!("workspace membership was not recorded: {error:#}");
    }
}

pub(crate) fn compiling_member() -> bool {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .map(|root| root.join("Cargo.toml"));
    let members = std::env::var_os(MEMBERS_ENV)
        .filter(|path| !path.is_empty())
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok());
    is_member(
        manifest.as_deref(),
        members.as_deref(),
        std::env::var_os("CARGO_PRIMARY_PACKAGE").is_some(),
    )
}

fn is_member(manifest: Option<&Path>, members: Option<&[PathBuf]>, primary: bool) -> bool {
    // A nested Cargo build may inherit an outer session's snapshot. Cargo's
    // marker still identifies packages explicitly selected by that build.
    if primary {
        return true;
    }
    let Some(members) = members else {
        // A standalone rustc shim cannot infer the *calling build's* workspace
        // from an external dependency's own package directory. Cargo still
        // identifies explicitly selected packages without that snapshot.
        return false;
    };
    let Some(manifest) = manifest else {
        return false;
    };
    let physical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let manifest = physical(manifest);
    members.iter().any(|member| physical(member) == manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_uses_cargos_package_ids_instead_of_directory_containment() {
        let members = vec![PathBuf::from("/project/member/Cargo.toml")];
        assert!(is_member(
            Some(Path::new("/project/member/Cargo.toml")),
            Some(&members),
            false
        ));
        assert!(!is_member(
            Some(Path::new("/project/vendor/dep/Cargo.toml")),
            Some(&members),
            false
        ));
        assert!(!is_member(
            Some(Path::new("/dependency/Cargo.toml")),
            Some(&members),
            false
        ));
    }

    #[test]
    fn standalone_shims_use_cargos_primary_package_marker() {
        let external = Path::new("/dependency/Cargo.toml");
        assert!(!is_member(Some(external), None, false));
        assert!(is_member(Some(external), None, true));
        assert!(is_member(
            Some(external),
            Some(&[PathBuf::from("/outer/Cargo.toml")]),
            true
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_sources_do_not_change_the_packages_membership() {
        let directory = tempfile::tempdir().unwrap();
        let member = directory.path().join("member");
        std::fs::create_dir_all(member.join("src")).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname='member'\nversion='0.1.0'\n",
        )
        .unwrap();
        let source = directory.path().join("external.rs");
        std::fs::write(&source, "pub fn value() {}\n").unwrap();
        std::os::unix::fs::symlink(source, member.join("src/lib.rs")).unwrap();
        assert!(is_member(
            Some(&member.join("Cargo.toml")),
            Some(&[member.join("Cargo.toml")]),
            false
        ));
    }
}
