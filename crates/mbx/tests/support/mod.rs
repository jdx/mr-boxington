//! Keep the developer's own mbx setup out of the integration tests.

use std::process::Command;

/// Give the child an isolated home and XDG directories, as
/// `test/test_helper/common_setup.bash` does for the Bats suites, so a global
/// mbx `config.toml` on the developer's machine cannot change what a test
/// sees. Cargo and rustup keep their real homes so the toolchain still
/// resolves.
///
/// Windows finds these directories through known-folder APIs rather than the
/// environment, so there is nothing to redirect there.
pub fn isolate_host(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        let real_home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        for (name, default) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
            let value = std::env::var_os(name)
                .map(std::path::PathBuf::from)
                .or_else(|| real_home.as_ref().map(|home| home.join(default)));
            if let Some(value) = value {
                command.env(name, value);
            }
        }
        let home = isolated_home();
        command
            .env("HOME", home)
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"));
    }
    command
}

/// Shared by every test in the run, as the real home was. No test writes mbx
/// configuration, so a global configuration file never exists here.
#[cfg(unix)]
fn isolated_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("isolated-home");
        std::fs::create_dir_all(&home).unwrap();
        home
    })
}
