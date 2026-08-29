use super::{
    ActionContext, ActionInput, Argument, BypassReason, MAX_NATIVE_INPUT_BYTES,
    MAX_PREDICTED_INPUTS, PathMapping, RustcInvocation, normalize_components,
};
use mbx_cache_core::CacheDigest;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A side-effect-minimized rustc invocation that emits only dependency data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepInfoCommand {
    arguments: Vec<OsString>,
    output: PathBuf,
}

impl DepInfoCommand {
    /// Arguments for the real compiler, excluding the compiler executable.
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// Exact file the compiler must populate with dep-info.
    pub fn output(&self) -> &Path {
        &self.output
    }
}

/// The source and environment inputs reported by rustc's dep-info output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RustcDepInfo {
    /// Source paths listed in the first dep-info dependency rule.
    pub files: Vec<PathBuf>,
    /// Environment inputs recorded by rustc `# env-dep:` lines.
    pub environment: BTreeMap<String, Option<String>>,
}

impl RustcDepInfo {
    /// Read and parse a dep-info file, treating missing or non-UTF-8 output as
    /// an explicit cache bypass.
    pub fn read(path: &Path) -> Result<Self, BypassReason> {
        let contents =
            std::fs::read_to_string(path).map_err(|error| BypassReason::DepInfoRead {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        Self::parse(&contents)
    }

    /// Parse rustc's Makefile-style dep-info format.
    ///
    /// This intentionally follows Cargo's parser contract: the first target
    /// rule contains all source dependencies, spaces are escaped with a
    /// trailing backslash on each token fragment, and `# env-dep:` records
    /// contain the environment observed by `env!` and `option_env!`.
    pub fn parse(contents: &str) -> Result<Self, BypassReason> {
        let mut files = BTreeSet::new();
        let mut environment = BTreeMap::new();
        let mut found_dependencies = false;

        for line in contents.lines() {
            if let Some(record) = line.strip_prefix("# env-dep:") {
                let (name, value) = record
                    .split_once('=')
                    .map_or((record, None), |(name, value)| (name, Some(value)));
                let name = unescape_environment(name)?;
                if name.is_empty() {
                    return Err(BypassReason::MalformedDepInfo(
                        "environment input has an empty name".into(),
                    ));
                }
                let value = value.map(unescape_environment).transpose()?;
                if environment
                    .insert(name.clone(), value.clone())
                    .is_some_and(|previous| previous != value)
                {
                    return Err(BypassReason::ConflictingEnvironment(name));
                }
                continue;
            }

            let Some(separator) = line.find(": ") else {
                continue;
            };
            if found_dependencies {
                continue;
            }
            found_dependencies = true;
            let mut fragments = line[separator + 2..].split_whitespace();
            while let Some(fragment) = fragments.next() {
                let mut file = fragment.to_string();
                while file.ends_with('\\') {
                    file.pop();
                    let continuation = fragments.next().ok_or_else(|| {
                        BypassReason::MalformedDepInfo(
                            "dependency path ends with an unterminated escape".into(),
                        )
                    })?;
                    file.push(' ');
                    file.push_str(continuation);
                }
                if file.is_empty() {
                    return Err(BypassReason::MalformedDepInfo(
                        "dependency path is empty".into(),
                    ));
                }
                files.insert(PathBuf::from(file));
            }
        }

        if !found_dependencies {
            return Err(BypassReason::MalformedDepInfo(
                "dependency rule is missing".into(),
            ));
        }
        if files.is_empty() {
            return Err(BypassReason::MalformedDepInfo(
                "dependency rule contains no inputs".into(),
            ));
        }
        Ok(Self {
            files: files.into_iter().collect(),
            environment,
        })
    }
}

/// A complete, content-addressed compiler input manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredInputs {
    working_dir: PathBuf,
    /// Content-addressed compiler input files.
    pub inputs: Vec<ActionInput>,
    /// Environment inputs captured from dep-info.
    pub environment: BTreeMap<String, Option<String>>,
}

impl DiscoveredInputs {
    pub(crate) fn from_paths(
        working_dir: &Path,
        paths: BTreeSet<PathBuf>,
        environment: BTreeMap<String, Option<String>>,
    ) -> Result<Self, BypassReason> {
        if !working_dir.is_absolute() {
            return Err(BypassReason::RelativeWorkingDirectory(
                working_dir.to_path_buf(),
            ));
        }
        let working_dir = normalize_components(working_dir);
        let mut inputs = Vec::with_capacity(paths.len());
        for path in paths {
            let metadata = std::fs::metadata(&path).map_err(|error| BypassReason::InputRead {
                path: path.clone(),
                message: error.to_string(),
            })?;
            if !metadata.is_file() {
                return Err(BypassReason::InputRead {
                    path,
                    message: "input is not a regular file".into(),
                });
            }
            let digest =
                CacheDigest::blake3_file(&path).map_err(|error| BypassReason::InputRead {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
            inputs.push(ActionInput { path, digest });
        }
        Ok(Self {
            working_dir,
            inputs,
            environment,
        })
    }

    /// Reject inputs whose modification time overlaps the compiler invocation.
    ///
    /// Input contents are first hashed after rustc reports their paths. This
    /// timestamp barrier prevents a post-compile write from being mistaken for
    /// the contents that produced the artifact. `verify` closes the remaining
    /// race after hashing.
    pub fn verify_not_modified_since(&self, started_at: SystemTime) -> Result<(), BypassReason> {
        for input in &self.inputs {
            let modified = std::fs::metadata(&input.path)
                .and_then(|metadata| metadata.modified())
                .map_err(|error| BypassReason::InputRead {
                    path: input.path.clone(),
                    message: error.to_string(),
                })?;
            if modified >= started_at {
                return Err(BypassReason::InputModifiedDuringCompilation(
                    input.path.clone(),
                ));
            }
        }
        Ok(())
    }

    /// Rehash every discovered file after compilation and before publication.
    /// This closes the discovery/compile race by degrading changed inputs to a
    /// cache miss rather than storing outputs beneath a stale action key.
    pub fn verify(&self) -> Result<(), BypassReason> {
        for input in &self.inputs {
            let matches = input.digest.matches_file(&input.path).map_err(|error| {
                BypassReason::InputRead {
                    path: input.path.clone(),
                    message: error.to_string(),
                }
            })?;
            if !matches {
                return Err(BypassReason::InputChanged(input.path.clone()));
            }
        }
        Ok(())
    }

    /// Merge the manifest into an action context after verifying that both use
    /// the same compiler working directory.
    pub fn apply_to(self, context: &mut ActionContext) -> Result<(), BypassReason> {
        if normalize_components(&context.working_dir) != self.working_dir {
            return Err(BypassReason::DiscoveryWorkingDirectory);
        }
        for (name, value) in &self.environment {
            if context
                .environment
                .get(name)
                .is_some_and(|previous| previous != value)
            {
                return Err(BypassReason::ConflictingEnvironment(name.clone()));
            }
        }
        context.environment.extend(self.environment);
        context.inputs.extend(self.inputs);
        Ok(())
    }
}

impl RustcInvocation {
    /// Replace the original output flags with a single explicit dep-info file.
    pub fn dep_info_command(&self, output: &Path) -> Result<DepInfoCommand, BypassReason> {
        if !output.is_absolute() {
            return Err(BypassReason::RelativeDepInfoPath(output.to_path_buf()));
        }
        let output_text = output
            .to_str()
            .ok_or_else(|| BypassReason::NonUtf8Path(output.to_path_buf()))?;
        if output_text.contains(',') {
            return Err(BypassReason::UnsafeDepInfoPath(output.to_path_buf()));
        }

        let mut arguments = Vec::new();
        for argument in &self.arguments {
            match argument {
                Argument::Emit(_) => {}
                Argument::Path { flag, .. } if flag == "--out-dir" || flag == "-o" => {}
                argument => arguments.push(render_argument(argument)?),
            }
        }
        arguments.push(format!("--emit=dep-info={output_text}").into());
        arguments.push(self.source.clone().into_os_string());
        Ok(DepInfoCommand {
            arguments,
            output: output.to_path_buf(),
        })
    }

    /// Hash dep-info sources plus every direct compiler input already modeled
    /// by the invocation (`--extern` artifacts and custom target specs).
    pub fn discover_inputs(
        &self,
        dep_info: &RustcDepInfo,
        working_dir: &Path,
    ) -> Result<DiscoveredInputs, BypassReason> {
        self.discover_inputs_with_mappings(dep_info, working_dir, &[])
    }

    /// Hash dep-info sources plus modeled compiler inputs, allowing native
    /// search directories beneath the working directory or a mapped root.
    pub fn discover_inputs_with_mappings(
        &self,
        dep_info: &RustcDepInfo,
        working_dir: &Path,
        path_mappings: &[PathMapping],
    ) -> Result<DiscoveredInputs, BypassReason> {
        if !working_dir.is_absolute() {
            return Err(BypassReason::RelativeWorkingDirectory(
                working_dir.to_path_buf(),
            ));
        }
        let working_dir = normalize_components(working_dir);
        let mut paths = dep_info
            .files
            .iter()
            .chain(&self.required_inputs)
            .map(|path| {
                let absolute = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    working_dir.join(path)
                };
                normalize_components(&absolute)
            })
            .collect::<BTreeSet<_>>();
        let admitted_roots = native_input_roots(&working_dir, path_mappings);
        let mut native_bytes = 0_u64;
        for argument in &self.arguments {
            if let Argument::SearchPath { kind, path } = argument
                && kind == "native"
            {
                let directory = if path.is_absolute() {
                    path.clone()
                } else {
                    working_dir.join(path)
                };
                // An inert directory outside every mapped root enters the key
                // by its literal path in the arguments, not by its contents --
                // predictions skip it under the same rule, so both discovery
                // paths agree on the action key.
                if self.native_search_is_inert()
                    && matches!(
                        super::normalize_mapped_path(&directory, &working_dir, path_mappings),
                        Err(BypassReason::UnmappedAbsolutePath(_))
                    )
                {
                    continue;
                }
                collect_native_directory(
                    &directory,
                    &admitted_roots,
                    &mut paths,
                    &mut native_bytes,
                )?;
            }
        }
        DiscoveredInputs::from_paths(&working_dir, paths, dep_info.environment.clone())
    }
}

/// Return normalized roots whose native search directories can be tracked.
pub(super) fn native_input_roots(
    working_dir: &Path,
    path_mappings: &[PathMapping],
) -> Vec<PathBuf> {
    std::iter::once(working_dir)
        .chain(path_mappings.iter().map(|mapping| mapping.root.as_path()))
        .map(normalize_components)
        .collect()
}

/// Add regular files beneath an admitted native search directory, enforcing
/// the prediction input count and the caller's cumulative native byte budget.
pub(super) fn collect_native_directory(
    directory: &Path,
    admitted_roots: &[PathBuf],
    paths: &mut BTreeSet<PathBuf>,
    native_bytes: &mut u64,
) -> Result<(), BypassReason> {
    let directory = normalize_components(directory);
    if !admitted_roots
        .iter()
        .any(|root| directory.starts_with(root))
    {
        return Err(BypassReason::UnsupportedSearchPath("native".into()));
    }

    let mut pending = vec![directory];
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(BypassReason::InputRead {
                    path: directory,
                    message: error.to_string(),
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| BypassReason::InputRead {
                path: directory.clone(),
                message: error.to_string(),
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| BypassReason::InputRead {
                path: path.clone(),
                message: error.to_string(),
            })?;
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                *native_bytes = native_bytes
                    .checked_add(
                        entry
                            .metadata()
                            .map_err(|error| BypassReason::InputRead {
                                path: path.clone(),
                                message: error.to_string(),
                            })?
                            .len(),
                    )
                    .ok_or_else(|| BypassReason::UnsupportedSearchPath("native".into()))?;
                paths.insert(path);
            } else {
                return Err(BypassReason::UnsupportedSearchPath("native".into()));
            }
            if paths.len() > MAX_PREDICTED_INPUTS || *native_bytes > MAX_NATIVE_INPUT_BYTES {
                return Err(BypassReason::UnsupportedSearchPath("native".into()));
            }
        }
    }
    Ok(())
}

fn render_argument(argument: &Argument) -> Result<OsString, BypassReason> {
    let rendered = match argument {
        Argument::Plain(value) => value.clone(),
        Argument::Path { flag, path } => format!(
            "{flag}={}",
            path.to_str()
                .ok_or_else(|| BypassReason::NonUtf8Path(path.clone()))?
        ),
        Argument::SearchPath { kind, path } => format!(
            "-L{kind}={}",
            path.to_str()
                .ok_or_else(|| BypassReason::NonUtf8Path(path.clone()))?
        ),
        Argument::Extern { name, path } => match path {
            Some(path) => format!(
                "--extern={name}={}",
                path.to_str()
                    .ok_or_else(|| BypassReason::NonUtf8Path(path.clone()))?
            ),
            None => format!("--extern={name}"),
        },
        Argument::Emit(_) => unreachable!("emit arguments are removed before rendering"),
        Argument::RemapPath { from, to } => format!(
            "--remap-path-prefix={}={to}",
            from.to_str()
                .ok_or_else(|| BypassReason::NonUtf8Path(from.clone()))?
        ),
        // Nothing links while emitting dep-info, so the prefix is inert here;
        // it is replayed verbatim to keep the command faithful.
        Argument::OsoPrefix {
            path,
            trailing_slash,
        } => format!(
            "--codegen=link-arg=-Wl,-oso_prefix,{}{}",
            path.to_str()
                .ok_or_else(|| BypassReason::NonUtf8Path(path.clone()))?,
            if *trailing_slash { "/" } else { "" }
        ),
    };
    Ok(rendered.into())
}

fn unescape_environment(value: &str) -> Result<String, BypassReason> {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match characters.next() {
            Some('\\') => output.push('\\'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(character) => {
                return Err(BypassReason::MalformedDepInfo(format!(
                    "unknown environment escape \\{character}"
                )));
            }
            None => {
                return Err(BypassReason::MalformedDepInfo(
                    "environment input ends with an unterminated escape".into(),
                ));
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_files_spaces_and_environment_records() {
        let parsed = RustcDepInfo::parse(
            "target/lib.rlib: src/lib.rs src/a\\ file.rs generated.rs\n\
             src/lib.rs:\n\
             # env-dep:SET=value\\nnext\n\
             # env-dep:UNSET\n\
             # env-dep:SLASH=a\\\\b\n",
        )
        .unwrap();
        assert_eq!(
            parsed.files,
            vec![
                PathBuf::from("generated.rs"),
                PathBuf::from("src/a file.rs"),
                PathBuf::from("src/lib.rs"),
            ]
        );
        assert_eq!(parsed.environment["SET"], Some("value\nnext".into()));
        assert_eq!(parsed.environment["UNSET"], None);
        assert_eq!(parsed.environment["SLASH"], Some(r"a\b".into()));
    }

    #[test]
    fn malformed_dep_info_bypasses_caching() {
        for contents in [
            "",
            "target: ",
            "target: src/trailing\\\n",
            "target: src/lib.rs\n# env-dep:NAME=bad\\q\n",
        ] {
            assert!(RustcDepInfo::parse(contents).is_err(), "{contents:?}");
        }
    }

    #[test]
    fn native_directory_byte_limit_is_cumulative() {
        let directory = tempfile::tempdir().unwrap();
        let native = directory.path().join("native");
        std::fs::create_dir_all(&native).unwrap();
        std::fs::write(native.join("input.lib"), b"xx").unwrap();
        let roots = native_input_roots(directory.path(), &[]);
        let mut paths = BTreeSet::new();
        let mut native_bytes = MAX_NATIVE_INPUT_BYTES - 1;

        assert_eq!(
            collect_native_directory(&native, &roots, &mut paths, &mut native_bytes),
            Err(BypassReason::UnsupportedSearchPath("native".into()))
        );
    }

    #[test]
    fn discovery_command_removes_real_outputs() {
        let invocation = RustcInvocation::parse(&args(&[
            "--crate-name=widget",
            "--crate-type=lib",
            "--emit=dep-info,metadata,link",
            "--out-dir=target/debug/deps",
            "-o",
            "target/debug/libwidget.rlib",
            "src/lib.rs",
        ]))
        .unwrap();
        let output = if cfg!(windows) {
            PathBuf::from(r"C:\tmp\mbx cache\inputs.d")
        } else {
            PathBuf::from("/tmp/mbx cache/inputs.d")
        };
        let command = invocation.dep_info_command(&output).unwrap();
        let arguments = command
            .arguments()
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            vec![
                "--crate-name=widget",
                "--crate-type=lib",
                &format!("--emit=dep-info={}", output.display()),
                "src/lib.rs",
            ]
        );
    }

    #[test]
    fn discovery_hashes_externs_and_custom_targets() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let source = root.join("src/lib.rs");
        let external = root.join("target/libdependency.rlib");
        let target = root.join("targets/custom.json");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::create_dir_all(external.parent().unwrap()).unwrap();
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&source, "pub fn library() {}\n").unwrap();
        std::fs::write(&external, "dependency artifact\n").unwrap();
        std::fs::write(&target, "{}\n").unwrap();

        let invocation = RustcInvocation::parse(&[
            "--crate-name=widget".into(),
            "--crate-type=lib".into(),
            "--emit=metadata".into(),
            format!("--extern=dependency={}", external.display()).into(),
            format!("--target={}", target.display()).into(),
            source.clone().into_os_string(),
        ])
        .unwrap();
        let dep_info = RustcDepInfo::parse(&format!("output: {}\n", source.display())).unwrap();
        let discovered = invocation.discover_inputs(&dep_info, root).unwrap();
        assert_eq!(discovered.inputs.len(), 3);

        std::fs::remove_file(&external).unwrap();
        assert!(matches!(
            invocation.discover_inputs(&dep_info, root),
            Err(BypassReason::InputRead { path, .. }) if path == external
        ));
    }

    #[test]
    fn discovery_rejects_inputs_modified_during_compilation() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("lib.rs");
        std::fs::write(&source, "pub fn library() {}\n").unwrap();
        let invocation = RustcInvocation::parse(&[
            "--crate-name=widget".into(),
            "--crate-type=lib".into(),
            "--emit=dep-info,metadata".into(),
            source.clone().into_os_string(),
        ])
        .unwrap();
        let dep_info = RustcDepInfo::parse(&format!("output: {}\n", source.display())).unwrap();
        let discovered = invocation
            .discover_inputs(&dep_info, directory.path())
            .unwrap();
        let modified = std::fs::metadata(&source).unwrap().modified().unwrap();

        assert_eq!(
            discovered.verify_not_modified_since(modified),
            Err(BypassReason::InputModifiedDuringCompilation(source))
        );
    }

    /// The MSVC toolset directories `cc`-built dependencies hand to every
    /// downstream compile on Windows: absolute, version-stamped, and outside
    /// every mapped root.
    fn toolchain_native_directory(version: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\Program Files\MSVC\{version}\lib\x64"))
        } else {
            PathBuf::from(format!("/opt/msvc/{version}/lib/x64"))
        }
    }

    fn library_with_native_search(source: &Path, directory: &Path) -> RustcInvocation {
        RustcInvocation::parse(&[
            "--crate-name=widget".into(),
            "--crate-type=lib".into(),
            "--emit=metadata,link".into(),
            format!("-Lnative={}", directory.display()).into(),
            source.to_path_buf().into_os_string(),
        ])
        .unwrap()
    }

    fn library_context(root: &Path, mappings: Vec<PathMapping>) -> ActionContext {
        ActionContext {
            compiler: crate::CompilerIdentity {
                toolchain: "core:rust@test".into(),
                rustc_version: "test".into(),
                host: std::env::consts::ARCH.into(),
            },
            working_dir: root.to_path_buf(),
            path_mappings: mappings,
            environment: BTreeMap::new(),
            portable_environment: BTreeSet::new(),
            inputs: Vec::new(),
        }
    }

    #[test]
    fn unmapped_native_directory_is_keyed_by_path_for_library_emits() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let source = root.join("lib.rs");
        std::fs::write(&source, "pub fn library() {}\n").unwrap();
        let toolchain = toolchain_native_directory("14.51.36231");
        let invocation = library_with_native_search(&source, &toolchain);
        let mappings = vec![PathMapping::new(root, "workspace")];
        let dep_info = RustcDepInfo::parse(&format!("output: {}\n", source.display())).unwrap();

        // The directory does not even exist: its contents are not inputs.
        let discovered = invocation
            .discover_inputs_with_mappings(&dep_info, root, &mappings)
            .unwrap();
        assert_eq!(discovered.inputs.len(), 1);
        assert_eq!(discovered.inputs[0].path, source);

        // The literal path is key material, so a toolset update misses.
        let context = library_context(root, mappings.clone());
        let digest = invocation.invocation_digest(&context).unwrap();
        let updated = library_with_native_search(&source, &toolchain_native_directory("14.52.0"));
        assert_ne!(digest, updated.invocation_digest(&context).unwrap());

        // The prediction skips the directory the same way discovery does, so a
        // build that replays it derives the action key dep-info would have.
        let mut recorded = context.clone();
        discovered.clone().apply_to(&mut recorded).unwrap();
        let action = invocation.action(recorded).unwrap();
        let prediction = invocation.prediction(&context, &discovered).unwrap();
        let replayed = prediction.discover(root, &context.path_mappings).unwrap();
        let mut replay_context = context.clone();
        replayed.apply_to(&mut replay_context).unwrap();
        assert_eq!(
            invocation.action(replay_context).unwrap().digest,
            action.digest
        );
    }

    #[test]
    fn unmapped_native_directory_still_refuses_a_native_link() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let source = root.join("main.rs");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        let toolchain = toolchain_native_directory("14.51.36231");
        let invocation = RustcInvocation::parse_with(
            &[
                "--crate-name=app".into(),
                "--crate-type=bin".into(),
                "--emit=link".into(),
                format!("-Lnative={}", toolchain.display()).into(),
                source.clone().into_os_string(),
            ],
            crate::ParseOptions::caching_native_links(true),
        )
        .unwrap();
        let mappings = vec![PathMapping::new(root, "workspace")];

        // A linker reads those directories, so their contents stay inputs the
        // key must account for, and an unmapped one stays a bypass.
        let context = library_context(root, mappings.clone());
        assert!(matches!(
            invocation.invocation_digest(&context),
            Err(BypassReason::UnmappedAbsolutePath(_))
        ));
        let dep_info = RustcDepInfo::parse(&format!("output: {}\n", source.display())).unwrap();
        assert_eq!(
            invocation.discover_inputs_with_mappings(&dep_info, root, &mappings),
            Err(BypassReason::UnsupportedSearchPath("native".into()))
        );
    }

    #[test]
    fn discovery_resolves_parent_components_against_the_working_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        let shared = directory.path().join("shared.rs");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&shared, "pub fn shared() {}\n").unwrap();

        let invocation = RustcInvocation::parse(&args(&[
            "--crate-name=widget",
            "--crate-type=lib",
            "--emit=metadata",
            "../shared.rs",
        ]))
        .unwrap();
        let dep_info = RustcDepInfo::parse("output: ../shared.rs\n").unwrap();
        let discovered = invocation.discover_inputs(&dep_info, &root).unwrap();

        assert_eq!(discovered.inputs.len(), 1);
        assert_eq!(discovered.inputs[0].path, shared);
    }

    #[test]
    fn rustc_dep_info_round_trip_discovers_real_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("lib.rs"),
            "mod child; const _: &str = include_str!(\"data file.txt\"); \
             const _: &str = env!(\"MBX_DISCOVERY_TEST\"); \
             const _: Option<&str> = option_env!(\"MBX_DISCOVERY_UNSET\");",
        )
        .unwrap();
        std::fs::write(root.join("child.rs"), "pub fn child() {}\n").unwrap();
        std::fs::write(root.join("data file.txt"), "included\n").unwrap();

        let invocation = RustcInvocation::parse(&args(&[
            "--crate-name=mbx_cache_discovery_test",
            "--crate-type=lib",
            "--emit=metadata,link",
            "lib.rs",
        ]))
        .unwrap();
        let dep_info_path = root.join("discovery inputs.d");
        let discovery_command = invocation.dep_info_command(&dep_info_path).unwrap();
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = match Command::new(rustc)
            .args(discovery_command.arguments())
            .current_dir(root)
            .env("MBX_DISCOVERY_TEST", "observed")
            .env_remove("MBX_DISCOVERY_UNSET")
            .output()
        {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => panic!("failed to execute rustc: {error}"),
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let parsed = RustcDepInfo::read(&dep_info_path).unwrap();
        let discovered = invocation.discover_inputs(&parsed, root).unwrap();
        assert_eq!(
            discovered.environment["MBX_DISCOVERY_TEST"],
            Some("observed".into())
        );
        assert_eq!(discovered.environment["MBX_DISCOVERY_UNSET"], None);
        assert_eq!(discovered.inputs.len(), 3);
        assert!(
            discovered
                .inputs
                .iter()
                .all(|input| input.digest.algorithm == "blake3")
        );
        let mut context = ActionContext {
            compiler: crate::CompilerIdentity {
                toolchain: "core:rust@test".into(),
                rustc_version: "test".into(),
                host: std::env::consts::ARCH.into(),
            },
            working_dir: root.to_path_buf(),
            path_mappings: vec![crate::PathMapping::new(root, "workspace")],
            environment: BTreeMap::new(),
            portable_environment: BTreeSet::new(),
            inputs: Vec::new(),
        };
        discovered.clone().apply_to(&mut context).unwrap();
        let action = invocation.action(context).unwrap();
        assert!(
            String::from_utf8(action.bytes)
                .unwrap()
                .contains(r#""MBX_DISCOVERY_TEST":"observed""#)
        );
        discovered.verify().unwrap();
        std::fs::write(root.join("child.rs"), "pub fn changed() {}\n").unwrap();
        assert_eq!(
            discovered.verify(),
            Err(BypassReason::InputChanged(root.join("child.rs")))
        );
    }
}
