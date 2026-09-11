use crate::config::config_file_path;
use eyre::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode, ExitStatus};

pub(super) fn run() -> Result<ExitCode> {
    let path = config_file_path()
        .ok_or_else(|| eyre::eyre!("could not resolve the global configuration path"))?;
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    run_with(
        &path,
        visual.as_deref(),
        editor.as_deref(),
        |program, arguments| Command::new(program).args(arguments).status(),
    )
}

fn run_with<F>(
    path: &Path,
    visual: Option<&str>,
    editor: Option<&str>,
    launch: F,
) -> Result<ExitCode>
where
    F: FnOnce(&OsStr, &[OsString]) -> io::Result<ExitStatus>,
{
    let command = configured_editor(visual, editor);
    let (program, mut arguments) = split_editor_command(command)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("failed to create {}", path.display()));
        }
    }

    arguments.push(path.as_os_str().to_owned());
    let status = launch(OsStr::new(&program), &arguments)
        .wrap_err_with(|| format!("failed to launch editor `{program}` for {}", path.display()))?;
    if !status.success() {
        eyre::bail!("editor `{program}` exited unsuccessfully: {status}");
    }
    Ok(ExitCode::SUCCESS)
}

fn configured_editor<'a>(visual: Option<&'a str>, editor: Option<&'a str>) -> &'a str {
    visual
        .filter(|value| !value.trim().is_empty())
        .or_else(|| editor.filter(|value| !value.trim().is_empty()))
        .unwrap_or(DEFAULT_EDITOR)
}

#[cfg(windows)]
const DEFAULT_EDITOR: &str = "notepad";
#[cfg(not(windows))]
const DEFAULT_EDITOR: &str = "nano";

fn split_editor_command(editor: &str) -> Result<(String, Vec<OsString>)> {
    let mut parts = shell_words::split(editor)
        .wrap_err_with(|| format!("failed to parse editor command {editor:?}"))?
        .into_iter();
    let program = parts
        .next()
        .filter(|program| !program.is_empty())
        .ok_or_else(|| eyre::eyre!("editor command is empty"))?;
    Ok((program, parts.map(Into::into).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_precedes_editor_and_empty_visual_falls_back() {
        assert_eq!(configured_editor(Some("code"), Some("vim")), "code");
        assert_eq!(configured_editor(Some("  "), Some("vim")), "vim");
        assert_eq!(configured_editor(None, Some("")), DEFAULT_EDITOR);
    }

    #[test]
    fn parses_arguments_and_a_quoted_executable_path() {
        let (program, arguments) =
            split_editor_command(r#""/Applications/My Editor.app/editor" --wait"#).unwrap();
        assert_eq!(program, "/Applications/My Editor.app/editor");
        assert_eq!(arguments, [OsString::from("--wait")]);
    }

    #[test]
    fn rejects_an_empty_quoted_editor() {
        assert!(split_editor_command(r#"""#).is_err());
    }

    #[test]
    fn creates_a_missing_file_and_passes_a_spaced_path_as_one_argument() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config root/mbx/config.toml");

        let result = run_with(&path, Some("code --wait"), None, |program, arguments| {
            assert_eq!(program, "code");
            assert_eq!(arguments, [OsString::from("--wait"), path.clone().into()]);
            assert_eq!(std::fs::read(&path).unwrap(), b"");
            Ok(exit_status(0))
        });

        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn leaves_an_existing_file_untouched_before_launch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::write(&path, "not valid toml = [").unwrap();

        run_with(&path, None, Some("vim"), |_, _| {
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "not valid toml = ["
            );
            Ok(exit_status(0))
        })
        .unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "not valid toml = [");
    }

    #[test]
    fn reports_launch_and_unsuccessful_exit_failures() {
        let first = tempfile::tempdir().unwrap();
        let path = first.path().join("config.toml");
        let error = run_with(&path, Some("missing-editor"), None, |_, _| {
            Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("failed to launch editor `missing-editor`"));

        let second = tempfile::tempdir().unwrap();
        let path = second.path().join("config.toml");
        let error = run_with(&path, Some("false"), None, |_, _| Ok(exit_status(7)))
            .unwrap_err()
            .to_string();
        assert!(error.contains("editor `false` exited unsuccessfully"));
    }

    #[cfg(unix)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt as _;
        ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt as _;
        ExitStatus::from_raw(code as u32)
    }
}
