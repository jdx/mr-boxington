//! Where Cargo writes a unit's outputs, shared by the rustc and build-script
//! shims.

use std::ffi::OsStr;
use std::path::Path;

/// The profile directory (`target/debug`, `target/<triple>/release`) that
/// Cargo writes the compilation output directory `output` beneath.
///
/// Before 1.100 compilations go to `<profile>/deps` or `<profile>/examples`
/// and build scripts to `<profile>/build/<unit>`. From 1.100 every unit writes
/// to `<profile>/build/<package>/<hash>/out`. Anything else, such as rustdoc's
/// `doc` or a directory the caller chose, names no profile.
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

/// The profile directory of a build script's `OUT_DIR`.
///
/// From 1.100 `OUT_DIR` is the unit's own output directory. Before it,
/// `OUT_DIR` is `<unit>/out`, one level below the unit directory.
pub(crate) fn out_dir_profile(out_dir: &Path) -> Option<&Path> {
    profile_dir(out_dir).or_else(|| out_dir.parent().and_then(profile_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_dir_follows_both_cargo_layouts() {
        let cases = [
            ("/t/target/debug/deps", Some("/t/target/debug")),
            ("/t/target/alt/examples", Some("/t/target/alt")),
            ("/t/target/debug/build/w-0123", Some("/t/target/debug")),
            (
                "/t/target/x86_64-unknown-linux-gnu/release/deps",
                Some("/t/target/x86_64-unknown-linux-gnu/release"),
            ),
            (
                "/t/target/alt/build/w/0123456789abcdef/out",
                Some("/t/target/alt"),
            ),
            ("/t/target/doc", None),
            ("/t/target/debug/build/w-0123/out", None),
            ("/t/out", None),
        ];
        for (output, profile) in cases {
            assert_eq!(profile_dir(Path::new(output)), profile.map(Path::new));
        }
    }

    #[test]
    fn out_dir_profile_follows_both_build_script_layouts() {
        for out_dir in [
            "/t/target/alt/build/w-0123456789abcdef/out",
            "/t/target/alt/build/w/0123456789abcdef/out",
        ] {
            assert_eq!(
                out_dir_profile(Path::new(out_dir)),
                Some(Path::new("/t/target/alt")),
                "{out_dir}"
            );
        }
        assert_eq!(out_dir_profile(Path::new("/t/out")), None);
    }
}
