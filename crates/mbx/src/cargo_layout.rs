//! Where Cargo puts a unit's outputs.
//!
//! Before 1.100, Cargo writes compilations to `<target>/<profile>/deps` (or
//! the same shape below a target-triple directory), examples to
//! `<profile>/examples` and build scripts to `<profile>/build/<unit>`. From
//! 1.100 every unit writes to `<profile>/build/<package>/<hash>/out`. The
//! rustc shim maps these directories to placeholders, and the build-script
//! shim pins its launcher binary beside the profile, so both read the layout
//! from here.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The profile directory Cargo writes this output directory below.
///
/// Anything outside the layout above -- rustdoc's `doc`, a directory a caller
/// chose, a pre-1.100 build script's `OUT_DIR` -- is no profile.
pub(crate) fn profile_dir(output: &Path) -> Option<&Path> {
    if matches!(
        output.file_name().and_then(OsStr::to_str),
        Some("deps" | "examples")
    ) {
        output.parent()
    } else if output.parent().and_then(Path::file_name) == Some(OsStr::new("build")) {
        output.parent().and_then(Path::parent)
    } else if output.file_name() == Some(OsStr::new("out"))
        && output.ancestors().nth(3).and_then(Path::file_name) == Some(OsStr::new("build"))
    {
        output.ancestors().nth(4)
    } else {
        None
    }
}

/// Infer the target root holding the profile subtree shared by rustc outputs
/// and build-script output.
///
/// Mapping the parent of the profile directory, rather than only the output
/// directory, also covers the other units' outputs and generated inputs below
/// `build/`, and the host profile beside a target-triple directory.
pub(crate) fn standalone_target_root(output: &Path, target: Option<&str>) -> PathBuf {
    if let Some(profile_root) = profile_dir(output).and_then(Path::parent) {
        let target_component = target.and_then(|target| Path::new(target).file_stem());
        if target_component.is_some_and(|target| profile_root.file_name() == Some(target))
            && let Some(root) = profile_root.parent()
        {
            return root.to_path_buf();
        }
        return profile_root.to_path_buf();
    }
    output.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_target_mapping_covers_the_profile_tree() {
        assert_eq!(
            standalone_target_root(Path::new("/tmp/target/debug/deps"), None),
            Path::new("/tmp/target")
        );
        assert_eq!(
            standalone_target_root(
                Path::new("/tmp/target/x86_64-unknown-linux-gnu/release/deps"),
                Some("x86_64-unknown-linux-gnu"),
            ),
            Path::new("/tmp/target")
        );
        assert_eq!(
            standalone_target_root(
                Path::new("/tmp/target/custom/release/deps"),
                Some("/tmp/targets/custom.json"),
            ),
            Path::new("/tmp/target")
        );
        // Cargo 1.100 gives every unit its own directory below `build/`.
        assert_eq!(
            standalone_target_root(
                Path::new("/tmp/target/debug/build/widget/0123456789abcdef/out"),
                None,
            ),
            Path::new("/tmp/target")
        );
        assert_eq!(
            standalone_target_root(
                Path::new(
                    "/tmp/target/x86_64-unknown-linux-gnu/debug/build/widget/0123456789abcdef/out"
                ),
                Some("x86_64-unknown-linux-gnu"),
            ),
            Path::new("/tmp/target")
        );
        // A pre-1.100 build script's OUT_DIR is not a unit output directory.
        assert_eq!(
            standalone_target_root(
                Path::new("/tmp/target/debug/build/widget-0123456789abcdef/out"),
                None,
            ),
            Path::new("/tmp/target/debug/build/widget-0123456789abcdef/out")
        );
    }

    #[test]
    fn profile_dir_follows_cargo_output_layouts() {
        let cases = [
            ("/tmp/target/debug/deps", Some("/tmp/target/debug")),
            ("/tmp/target/alt/examples", Some("/tmp/target/alt")),
            (
                "/tmp/target/debug/build/widget-0123456789abcdef",
                Some("/tmp/target/debug"),
            ),
            (
                "/tmp/target/x86_64-unknown-linux-gnu/release/deps",
                Some("/tmp/target/x86_64-unknown-linux-gnu/release"),
            ),
            (
                "/tmp/target/custom/alt/deps",
                Some("/tmp/target/custom/alt"),
            ),
            (
                "/tmp/target/alt/build/widget/0123456789abcdef/out",
                Some("/tmp/target/alt"),
            ),
            // Neither rustdoc's output nor a build script's pre-1.100 OUT_DIR is
            // a compilation's output directory.
            ("/tmp/target/doc", None),
            ("/tmp/target/debug/build/widget-0123456789abcdef/out", None),
            ("/tmp/out", None),
        ];
        for (output, profile) in cases {
            assert_eq!(
                profile_dir(Path::new(output)),
                profile.map(Path::new),
                "{output}"
            );
        }
    }
}
