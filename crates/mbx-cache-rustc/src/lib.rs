use mbx_cache_core::{CacheDigest, canonical_json};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

mod dep_info;

pub use dep_info::{DepInfoCommand, DiscoveredInputs, RustcDepInfo};

pub const ACTION_SCHEMA_VERSION: u8 = 1;
pub const ADAPTER_VERSION: u8 = 1;

impl BypassReason {
    /// A stable, low-cardinality name for this reason.
    ///
    /// Many variants carry a path or a flag, so `Display` text cannot be
    /// aggregated; statistics group by this instead.
    pub fn kind(&self) -> &'static str {
        self.into()
    }
}

const SUPPORTED_CODEGEN_OPTIONS: &[&str] = &[
    "codegen-units",
    "control-flow-guard",
    "debug-assertions",
    "debuginfo",
    "default-linker-libraries",
    "embed-bitcode",
    "extra-filename",
    "force-frame-pointers",
    "force-unwind-tables",
    "instrument-coverage",
    "link-dead-code",
    "link-self-contained",
    "lto",
    "metadata",
    "no-prepopulate-passes",
    "opt-level",
    "overflow-checks",
    "panic",
    "prefer-dynamic",
    "relocation-model",
    "rpath",
    "save-temps",
    "soft-float",
    "split-debuginfo",
    "split-dwarf-kind",
    "strip",
    "symbol-mangling-version",
    "target-cpu",
    "target-feature",
    "tls-model",
];

#[derive(Debug, Clone, PartialEq, Eq, Error, strum::IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum BypassReason {
    #[error("rustc argument {index} is not valid UTF-8")]
    NonUtf8Argument { index: usize },
    #[error("rustc response files are not supported: {0}")]
    ResponseFile(String),
    #[error("rustc flag is not modeled by the cache adapter: {0}")]
    UnknownFlag(String),
    #[error("rustc codegen option is not modeled by the cache adapter: {0}")]
    UnknownCodegenOption(String),
    #[error("rustc flag requires a value: {0}")]
    MissingValue(String),
    #[error("rustc invocation is a compiler query, not a compilation")]
    CompilerQuery,
    #[error("rustc invocation reads source from standard input")]
    StandardInput,
    #[error("rustc invocation has no source input")]
    MissingInput,
    #[error("rustc invocation has multiple source inputs")]
    MultipleInputs,
    #[error("incremental compilation cannot be combined with action caching")]
    Incremental,
    #[error("rustc crate type is not cacheable yet: {0}")]
    UnsupportedCrateType(String),
    #[error("rustc output type is not cacheable yet: {0}")]
    UnsupportedEmit(String),
    #[error("rustc invocation does not emit an rlib or metadata artifact")]
    NoCacheableOutput,
    #[error("rustc invocation does not emit dependency information")]
    NoDepInfo,
    #[error("rustc output paths do not share one directory")]
    SplitOutputDirectories,
    #[error("rustc output path has no file name: {0}")]
    InvalidOutputPath(PathBuf),
    #[error("rustc -o with an emit that has no explicit path is not modeled: {0}")]
    ImplicitEmitWithOutputFile(PathBuf),
    #[error("native library lookup is not cacheable yet")]
    NativeLibrary,
    #[error("rustc search path kind is not cacheable yet: {0}")]
    UnsupportedSearchPath(String),
    #[error("rustc extern does not identify an input artifact: {0}")]
    UnresolvedExtern(String),
    #[error("absolute path has no stable cache mapping: {0}")]
    UnmappedAbsolutePath(PathBuf),
    #[error("cache key paths must be valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("cache action working directory must be absolute: {0}")]
    RelativeWorkingDirectory(PathBuf),
    #[error("cache path mapping must use an absolute root: {0}")]
    RelativePathMapping(PathBuf),
    #[error("cache path mapping placeholder is invalid: {0}")]
    InvalidPathPlaceholder(String),
    #[error("required compiler input was not provided: {0}")]
    MissingRequiredInput(String),
    #[error("compiler input has an invalid digest: {0}")]
    InvalidInputDigest(String),
    #[error("compiler input appears more than once with different content: {0}")]
    ConflictingInput(String),
    #[error("rustc dep-info is malformed: {0}")]
    MalformedDepInfo(String),
    #[error("failed to read rustc dep-info {path}: {message}")]
    DepInfoRead { path: PathBuf, message: String },
    #[error("rustc dep-info output path must be absolute: {0}")]
    RelativeDepInfoPath(PathBuf),
    #[error("rustc dep-info output path cannot contain a comma: {0}")]
    UnsafeDepInfoPath(PathBuf),
    #[error("failed to read compiler input {path}: {message}")]
    InputRead { path: PathBuf, message: String },
    #[error("compiler input changed after discovery: {0}")]
    InputChanged(PathBuf),
    #[error("compiler input was modified during compilation: {0}")]
    InputModifiedDuringCompilation(PathBuf),
    #[error("discovered inputs were collected from a different working directory")]
    DiscoveryWorkingDirectory,
    #[error("compiler environment input has conflicting values: {0}")]
    ConflictingEnvironment(String),
    #[error("failed to serialize the rustc action: {0}")]
    Serialization(String),
    #[error("rustc action prediction is unsupported")]
    UnsupportedPrediction,
    #[error("rustc action prediction contains an invalid input path: {0}")]
    InvalidPredictedInput(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Argument {
    Plain(String),
    Path { flag: String, path: PathBuf },
    SearchPath { kind: String, path: PathBuf },
    Extern { name: String, path: Option<PathBuf> },
    Emit(Vec<Emit>),
    RemapPath { from: PathBuf, to: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Emit {
    kind: String,
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustcInvocation {
    arguments: Vec<Argument>,
    source: PathBuf,
    required_inputs: Vec<PathBuf>,
    crate_name: String,
    extra_filename: String,
    out_dir: Option<PathBuf>,
    explicit_output: Option<PathBuf>,
    emits: Vec<Emit>,
}

/// The cacheable files and dependency manifest produced by a rustc invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustcOutputs {
    pub directory: PathBuf,
    pub files: Vec<PathBuf>,
    pub dep_info: PathBuf,
}

impl RustcInvocation {
    /// Parse rustc's arguments, excluding the compiler executable supplied as
    /// the first argument to `RUSTC_WRAPPER`.
    ///
    /// Any flag whose cache semantics are not modeled returns a bypass reason
    /// instead of guessing. A successful parse only admits the initial
    /// rlib/rmeta cacheability tier.
    pub fn parse(arguments: &[OsString]) -> Result<Self, BypassReason> {
        Parser::new(arguments).parse()
    }

    /// Return the source input passed to rustc.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Resolve the rlib/rmeta files produced by this invocation.
    ///
    /// The initial cache tier requires one output directory so its artifact can
    /// be represented by one protocol directory and restored atomically later.
    pub fn outputs(&self, working_dir: &Path) -> Result<RustcOutputs, BypassReason> {
        if !working_dir.is_absolute() {
            return Err(BypassReason::RelativeWorkingDirectory(
                working_dir.to_path_buf(),
            ));
        }
        let explicit_output = self
            .explicit_output
            .as_deref()
            .map(|path| absolute_path(path, working_dir));
        let output_directory = explicit_output
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| {
                self.out_dir
                    .as_deref()
                    .map(|path| absolute_path(path, working_dir))
            })
            .unwrap_or_else(|| normalize_components(working_dir));
        // rustc applies `-o` to every emit that has no path of its own, so the
        // file names cannot be derived from the crate name here. Cargo always
        // uses --out-dir instead, so refusing to model this costs nothing.
        if let Some(output) = &explicit_output
            && self.emits.iter().any(|emit| {
                emit.path.is_none()
                    && matches!(emit.kind.as_str(), "dep-info" | "link" | "metadata")
            })
        {
            return Err(BypassReason::ImplicitEmitWithOutputFile(output.clone()));
        }
        let mut files = BTreeSet::new();
        let mut dep_info = None;
        for emit in &self.emits {
            if emit.kind == "dep-info" {
                let path = emit.path.as_ref().map_or_else(
                    || {
                        explicit_output.clone().map_or_else(
                            || {
                                output_directory
                                    .join(format!("{}{}.d", self.crate_name, self.extra_filename))
                            },
                            |path| path.with_extension("d"),
                        )
                    },
                    |path| absolute_path(path, working_dir),
                );
                if path.file_name().is_none() {
                    return Err(BypassReason::InvalidOutputPath(path));
                }
                dep_info = Some(path);
                continue;
            }
            let extension = match emit.kind.as_str() {
                "link" => "rlib",
                "metadata" => "rmeta",
                _ => continue,
            };
            let path = if let Some(path) = &emit.path {
                absolute_path(path, working_dir)
            } else {
                output_directory.join(format!(
                    "lib{}{}.{}",
                    self.crate_name, self.extra_filename, extension
                ))
            };
            if path.file_name().is_none() {
                return Err(BypassReason::InvalidOutputPath(path));
            }
            if path.parent() != Some(output_directory.as_path()) {
                return Err(BypassReason::SplitOutputDirectories);
            }
            files.insert(path);
        }
        let dep_info = dep_info.ok_or(BypassReason::NoDepInfo)?;
        if dep_info.parent() != Some(output_directory.as_path()) {
            return Err(BypassReason::SplitOutputDirectories);
        }
        Ok(RustcOutputs {
            directory: output_directory,
            files: files.into_iter().collect(),
            dep_info,
        })
    }

    /// Build canonical action bytes after precise input discovery has run.
    ///
    /// `context.inputs` must contain the source, every explicit extern, and
    /// every additional source or environment-generated input discovered from
    /// dep-info.
    pub fn action(&self, context: ActionContext) -> Result<RustcAction, BypassReason> {
        ActionBuilder::new(self, context).build()
    }

    /// Fingerprint the modeled invocation before dependency contents are known.
    pub fn invocation_digest(&self, context: &ActionContext) -> Result<CacheDigest, BypassReason> {
        let descriptor = ActionBuilder::new(self, context.clone()).invocation_descriptor()?;
        let bytes = canonical_json(&descriptor)
            .map_err(|error| BypassReason::Serialization(error.to_string()))?;
        Ok(CacheDigest::blake3(&bytes))
    }

    /// Capture normalized dependency paths for a future invocation that has no
    /// dep-info file yet.
    pub fn prediction(
        &self,
        context: &ActionContext,
        discovered: &DiscoveredInputs,
    ) -> Result<RustcInputPrediction, BypassReason> {
        let builder = ActionBuilder::new(self, context.clone());
        builder.validate_mappings()?;
        let inputs = discovered
            .inputs
            .iter()
            .map(|input| builder.normalize_path(&input.path))
            .collect::<Result<BTreeSet<_>, _>>()?
            .into_iter()
            .collect();
        Ok(RustcInputPrediction {
            version: 1,
            inputs,
            environment: discovered.environment.keys().cloned().collect(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMapping {
    pub root: PathBuf,
    pub placeholder: String,
}

impl PathMapping {
    /// Map an absolute host path to a stable cache-key placeholder.
    pub fn new(root: impl Into<PathBuf>, placeholder: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            placeholder: placeholder.into(),
        }
    }

    /// Order mappings deepest root first, which is what normalization needs:
    /// a target directory inside the workspace has to win over the workspace.
    pub fn ordered(mappings: &[PathMapping]) -> Vec<PathMapping> {
        let mut ordered = mappings.to_vec();
        ordered.sort_by_key(|mapping| std::cmp::Reverse(mapping.root.components().count()));
        ordered
    }
}

/// Map an absolute path to its cache-key placeholder form.
///
/// `mappings` must already be ordered by [`PathMapping::ordered`]. Exposed for
/// callers that need the placeholder text before an action exists -- notably to
/// build the `--remap-path-prefix` flag that makes a compilation independent of
/// a path in its environment.
pub fn normalize_mapped_path(
    path: &Path,
    working_dir: &Path,
    mappings: &[PathMapping],
) -> Result<String, BypassReason> {
    let absolute = if path.is_absolute() {
        normalize_components(path)
    } else {
        normalize_components(&working_dir.join(path))
    };
    for mapping in mappings {
        let root = normalize_components(&mapping.root);
        if let Ok(relative) = absolute.strip_prefix(&root) {
            let suffix = slash_path(relative)?;
            return Ok(if suffix.is_empty() {
                format!("${{{}}}", mapping.placeholder)
            } else {
                format!("${{{}}}/{suffix}", mapping.placeholder)
            });
        }
    }
    Err(BypassReason::UnmappedAbsolutePath(absolute))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerIdentity {
    pub toolchain: String,
    pub rustc_version: String,
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionInput {
    pub path: PathBuf,
    pub digest: CacheDigest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionContext {
    pub compiler: CompilerIdentity,
    pub working_dir: PathBuf,
    pub path_mappings: Vec<PathMapping>,
    pub environment: BTreeMap<String, Option<String>>,
    /// Environment inputs whose absolute values the compilation has been made
    /// independent of, and whose values the key therefore normalizes.
    ///
    /// Naming one here is a claim about the compilation, not a preference: the
    /// caller must both neutralize the value inside it (with
    /// `--remap-path-prefix`) and confirm no output carries the value anyway.
    pub portable_environment: BTreeSet<String>,
    pub inputs: Vec<ActionInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustcAction {
    pub digest: CacheDigest,
    pub bytes: Vec<u8>,
}

/// Normalized input names from the last successful execution of one modeled
/// rustc invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustcInputPrediction {
    pub version: u8,
    pub inputs: Vec<String>,
    pub environment: Vec<String>,
}

impl RustcInputPrediction {
    /// Rehash the predicted paths and read the current environment. The caller
    /// still recomputes the full action digest, so changed inputs are misses.
    pub fn discover(
        &self,
        working_dir: &Path,
        path_mappings: &[PathMapping],
    ) -> Result<DiscoveredInputs, BypassReason> {
        if self.version != 1 {
            return Err(BypassReason::UnsupportedPrediction);
        }
        if self.inputs.len() > 16 * 1024 || self.environment.len() > 4 * 1024 {
            return Err(BypassReason::UnsupportedPrediction);
        }
        let paths = self
            .inputs
            .iter()
            .map(|path| denormalize_path(path, path_mappings))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let environment = self
            .environment
            .iter()
            .map(|name| {
                if name.is_empty() || name.contains(['=', '\0']) {
                    return Err(BypassReason::UnsupportedPrediction);
                }
                let value = std::env::var_os(name)
                    .map(|value| {
                        value
                            .into_string()
                            .map_err(|_| BypassReason::UnsupportedPrediction)
                    })
                    .transpose()?;
                Ok((name.clone(), value))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        DiscoveredInputs::from_paths(working_dir, paths, environment)
    }
}

#[derive(Serialize)]
struct ActionDescriptor {
    version: u8,
    kind: &'static str,
    adapter_version: u8,
    compiler: CompilerDescriptor,
    arguments: Vec<String>,
    environment: BTreeMap<String, Option<String>>,
    inputs: Vec<InputDescriptor>,
}

#[derive(Serialize)]
struct InvocationDescriptor {
    version: u8,
    kind: &'static str,
    adapter_version: u8,
    compiler: CompilerDescriptor,
    arguments: Vec<String>,
    required_inputs: Vec<String>,
}

#[derive(Serialize)]
struct CompilerDescriptor {
    toolchain: String,
    rustc_version: String,
    host: String,
}

#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct InputDescriptor {
    path: String,
    digest: CacheDigest,
}

struct Parser<'a> {
    arguments: &'a [OsString],
    index: usize,
    parsed: Vec<Argument>,
    source: Option<PathBuf>,
    crate_types: Vec<String>,
    emits: Vec<Emit>,
    required_inputs: Vec<PathBuf>,
    test: bool,
    crate_name: Option<String>,
    extra_filename: String,
    out_dir: Option<PathBuf>,
    explicit_output: Option<PathBuf>,
}

impl<'a> Parser<'a> {
    fn new(arguments: &'a [OsString]) -> Self {
        Self {
            arguments,
            index: 0,
            parsed: Vec::new(),
            source: None,
            crate_types: Vec::new(),
            emits: Vec::new(),
            required_inputs: Vec::new(),
            test: false,
            crate_name: None,
            extra_filename: String::new(),
            out_dir: None,
            explicit_output: None,
        }
    }

    fn parse(mut self) -> Result<RustcInvocation, BypassReason> {
        while self.index < self.arguments.len() {
            let value = self.current()?.to_string();
            self.index += 1;
            if value.starts_with('@') {
                return Err(BypassReason::ResponseFile(value));
            }
            if let Some(long) = value.strip_prefix("--") {
                self.parse_long(long)?;
            } else if value.starts_with('-') && value != "-" {
                self.parse_short(&value)?;
            } else {
                self.parse_input(&value)?;
            }
        }

        let source = self.source.clone().ok_or(BypassReason::MissingInput)?;
        self.classify()?;
        let crate_name = self.crate_name.clone().map_or_else(
            || {
                source
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .map(|name| name.replace('-', "_"))
                    .ok_or_else(|| BypassReason::NonUtf8Path(source.clone()))
            },
            Ok,
        )?;
        self.required_inputs.push(source.clone());
        Ok(RustcInvocation {
            arguments: self.parsed,
            source,
            required_inputs: self.required_inputs,
            crate_name,
            extra_filename: self.extra_filename,
            out_dir: self.out_dir,
            explicit_output: self.explicit_output,
            emits: self.emits,
        })
    }

    fn current(&self) -> Result<&str, BypassReason> {
        self.arguments[self.index]
            .to_str()
            .ok_or(BypassReason::NonUtf8Argument { index: self.index })
    }

    fn take_value(&mut self, flag: &str, inline: Option<&str>) -> Result<String, BypassReason> {
        if let Some(value) = inline {
            if value.is_empty() {
                return Err(BypassReason::MissingValue(flag.into()));
            }
            return Ok(value.into());
        }
        if self.index >= self.arguments.len() {
            return Err(BypassReason::MissingValue(flag.into()));
        }
        let value = self.current()?.to_string();
        self.index += 1;
        Ok(value)
    }

    fn parse_long(&mut self, value: &str) -> Result<(), BypassReason> {
        let (flag, inline) = value
            .split_once('=')
            .map_or((value, None), |(flag, value)| (flag, Some(value)));
        let rendered_flag = format!("--{flag}");
        match flag {
            "help" | "version" | "explain" | "print" => Err(BypassReason::CompilerQuery),
            "test" => {
                self.test = true;
                self.parsed.push(Argument::Plain(rendered_flag));
                Ok(())
            }
            "verbose" => {
                self.parsed.push(Argument::Plain(rendered_flag));
                Ok(())
            }
            "crate-name" => {
                let value = self.take_value(&rendered_flag, inline)?;
                self.crate_name = Some(value.clone());
                self.parsed
                    .push(Argument::Plain(format!("{rendered_flag}={value}")));
                Ok(())
            }
            "cfg" | "check-cfg" | "edition" | "error-format" | "json" | "color"
            | "diagnostic-width" | "remap-path-scope" | "allow" | "warn" | "force-warn"
            | "deny" | "forbid" | "cap-lints" => {
                let value = self.take_value(&rendered_flag, inline)?;
                self.parsed
                    .push(Argument::Plain(format!("{rendered_flag}={value}")));
                Ok(())
            }
            "target" => {
                let value = self.take_value(&rendered_flag, inline)?;
                if value.ends_with(".json") || value.contains(['/', '\\']) {
                    let path = PathBuf::from(value);
                    self.required_inputs.push(path.clone());
                    self.parsed.push(Argument::Path {
                        flag: rendered_flag,
                        path,
                    });
                } else {
                    self.parsed
                        .push(Argument::Plain(format!("{rendered_flag}={value}")));
                }
                Ok(())
            }
            "crate-type" => {
                let value = self.take_value(&rendered_flag, inline)?;
                self.crate_types
                    .extend(value.split(',').map(ToOwned::to_owned));
                self.parsed
                    .push(Argument::Plain(format!("{rendered_flag}={value}")));
                Ok(())
            }
            "emit" => {
                let value = self.take_value(&rendered_flag, inline)?;
                let emits = parse_emits(&value);
                self.emits.extend(emits.clone());
                self.parsed.push(Argument::Emit(emits));
                Ok(())
            }
            "out-dir" => {
                let path = PathBuf::from(self.take_value(&rendered_flag, inline)?);
                self.out_dir = Some(path.clone());
                self.parsed.push(Argument::Path {
                    flag: rendered_flag,
                    path,
                });
                Ok(())
            }
            "sysroot" => {
                let path = self.take_value(&rendered_flag, inline)?;
                self.parsed.push(Argument::Path {
                    flag: rendered_flag,
                    path: path.into(),
                });
                Ok(())
            }
            "extern" => {
                let value = self.take_value(&rendered_flag, inline)?;
                let (name, path) = value
                    .split_once('=')
                    .map_or((value.as_str(), None), |(name, path)| {
                        (name, Some(PathBuf::from(path)))
                    });
                if let Some(path) = &path {
                    self.required_inputs.push(path.clone());
                }
                self.parsed.push(Argument::Extern {
                    name: name.into(),
                    path,
                });
                Ok(())
            }
            "remap-path-prefix" => {
                let value = self.take_value(&rendered_flag, inline)?;
                let Some((from, to)) = value.split_once('=') else {
                    return Err(BypassReason::MissingValue(rendered_flag));
                };
                self.parsed.push(Argument::RemapPath {
                    from: from.into(),
                    to: to.into(),
                });
                Ok(())
            }
            "codegen" => {
                let value = self.take_value(&rendered_flag, inline)?;
                self.parse_codegen(&value)
            }
            _ => Err(BypassReason::UnknownFlag(rendered_flag)),
        }
    }

    fn parse_short(&mut self, value: &str) -> Result<(), BypassReason> {
        match value {
            // `-vV` is how cargo and build scripts ask for the verbose
            // version, so it is a query rather than a flag left unmodeled.
            "-h" | "-V" | "-vV" => return Err(BypassReason::CompilerQuery),
            "-g" | "-O" | "-v" => {
                self.parsed.push(Argument::Plain(value.into()));
                return Ok(());
            }
            _ => {}
        }
        for (short, long) in [
            ("-A", "--allow"),
            ("-W", "--warn"),
            ("-D", "--deny"),
            ("-F", "--forbid"),
        ] {
            if let Some(attached) = value.strip_prefix(short) {
                let lint = self.take_value(short, (!attached.is_empty()).then_some(attached))?;
                self.parsed.push(Argument::Plain(format!("{long}={lint}")));
                return Ok(());
            }
        }
        if let Some(attached) = value.strip_prefix("-C") {
            let option = self.take_value("-C", (!attached.is_empty()).then_some(attached))?;
            return self.parse_codegen(&option);
        }
        if let Some(attached) = value.strip_prefix("-L") {
            let search = self.take_value("-L", (!attached.is_empty()).then_some(attached))?;
            let (kind, path) = search
                .split_once('=')
                .map_or(("all", search.as_str()), |(kind, path)| (kind, path));
            if kind != "dependency" {
                return Err(BypassReason::UnsupportedSearchPath(kind.into()));
            }
            self.parsed.push(Argument::SearchPath {
                kind: kind.into(),
                path: path.into(),
            });
            return Ok(());
        }
        if value == "-l" || value.starts_with("-l") {
            return Err(BypassReason::NativeLibrary);
        }
        if let Some(attached) = value.strip_prefix("-o") {
            let path = self.take_value("-o", (!attached.is_empty()).then_some(attached))?;
            self.explicit_output = Some(path.clone().into());
            self.parsed.push(Argument::Path {
                flag: "-o".into(),
                path: path.into(),
            });
            return Ok(());
        }
        Err(BypassReason::UnknownFlag(value.into()))
    }

    fn parse_codegen(&mut self, value: &str) -> Result<(), BypassReason> {
        let name = value.split_once('=').map_or(value, |(name, _)| name);
        if name == "incremental" {
            return Err(BypassReason::Incremental);
        }
        if SUPPORTED_CODEGEN_OPTIONS.binary_search(&name).is_err() {
            return Err(BypassReason::UnknownCodegenOption(name.into()));
        }
        self.parsed
            .push(Argument::Plain(format!("--codegen={value}")));
        if name == "extra-filename" {
            self.extra_filename = value
                .split_once('=')
                .map_or(String::new(), |(_, value)| value.to_string());
        }
        Ok(())
    }

    fn parse_input(&mut self, value: &str) -> Result<(), BypassReason> {
        if value == "-" {
            return Err(BypassReason::StandardInput);
        }
        if self.source.replace(value.into()).is_some() {
            return Err(BypassReason::MultipleInputs);
        }
        Ok(())
    }

    fn classify(&self) -> Result<(), BypassReason> {
        if self.crate_types.is_empty() {
            return Err(BypassReason::UnsupportedCrateType("bin".into()));
        }
        if let Some(crate_type) = self
            .crate_types
            .iter()
            .find(|crate_type| !matches!(crate_type.as_str(), "lib" | "rlib"))
        {
            return Err(BypassReason::UnsupportedCrateType(crate_type.clone()));
        }
        if self.test {
            return Err(BypassReason::UnsupportedCrateType("test".into()));
        }
        if let Some(name) = self.parsed.iter().find_map(|argument| match argument {
            Argument::Extern { name, path: None } if name != "proc_macro" => Some(name),
            _ => None,
        }) {
            return Err(BypassReason::UnresolvedExtern(name.clone()));
        }
        if let Some(emit) = self
            .emits
            .iter()
            .find(|emit| !matches!(emit.kind.as_str(), "dep-info" | "link" | "metadata"))
        {
            return Err(BypassReason::UnsupportedEmit(emit.kind.clone()));
        }
        if !self
            .emits
            .iter()
            .any(|emit| matches!(emit.kind.as_str(), "link" | "metadata"))
        {
            return Err(BypassReason::NoCacheableOutput);
        }
        Ok(())
    }
}

fn parse_emits(value: &str) -> Vec<Emit> {
    value
        .split(',')
        .map(|emit| {
            let (kind, path) = emit
                .split_once('=')
                .map_or((emit, None), |(kind, path)| (kind, Some(path.into())));
            Emit {
                kind: kind.into(),
                path,
            }
        })
        .collect()
}

struct ActionBuilder<'a> {
    invocation: &'a RustcInvocation,
    context: ActionContext,
    mappings: Vec<PathMapping>,
}

impl<'a> ActionBuilder<'a> {
    fn new(invocation: &'a RustcInvocation, mut context: ActionContext) -> Self {
        context.path_mappings = PathMapping::ordered(&context.path_mappings);
        Self {
            invocation,
            mappings: context.path_mappings.clone(),
            context,
        }
    }

    fn build(self) -> Result<RustcAction, BypassReason> {
        self.validate_mappings()?;
        let invocation = self.invocation_descriptor()?;
        let environment = self.environment_descriptor()?;

        let mut inputs = BTreeMap::<String, CacheDigest>::new();
        for input in &self.context.inputs {
            input
                .digest
                .validate()
                .map_err(|_| BypassReason::InvalidInputDigest(input.path.display().to_string()))?;
            let path = self.normalize_path(&input.path)?;
            if inputs
                .insert(path.clone(), input.digest.clone())
                .is_some_and(|existing| existing != input.digest)
            {
                return Err(BypassReason::ConflictingInput(path));
            }
        }
        let required = self
            .invocation
            .required_inputs
            .iter()
            .map(|path| self.normalize_path(path))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if let Some(missing) = required.iter().find(|path| !inputs.contains_key(*path)) {
            return Err(BypassReason::MissingRequiredInput(missing.clone()));
        }
        let inputs = inputs
            .into_iter()
            .map(|(path, digest)| InputDescriptor { path, digest })
            .collect();
        let descriptor = ActionDescriptor {
            version: ACTION_SCHEMA_VERSION,
            kind: "rustc",
            adapter_version: ADAPTER_VERSION,
            compiler: invocation.compiler,
            arguments: invocation.arguments,
            environment,
            inputs,
        };
        let bytes = canonical_json(&descriptor)
            .map_err(|error| BypassReason::Serialization(error.to_string()))?;
        let digest = CacheDigest::blake3(&bytes);
        Ok(RustcAction { digest, bytes })
    }

    fn invocation_descriptor(&self) -> Result<InvocationDescriptor, BypassReason> {
        self.validate_mappings()?;
        let arguments = self
            .invocation
            .arguments
            .iter()
            .map(|argument| self.normalize_argument(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let required_inputs = self
            .invocation
            .required_inputs
            .iter()
            .map(|path| self.normalize_path(path))
            .collect::<Result<BTreeSet<_>, _>>()?
            .into_iter()
            .collect();
        Ok(InvocationDescriptor {
            version: ACTION_SCHEMA_VERSION,
            kind: "rustc",
            adapter_version: ADAPTER_VERSION,
            compiler: CompilerDescriptor {
                toolchain: self.context.compiler.toolchain.clone(),
                rustc_version: self.context.compiler.rustc_version.clone(),
                host: self.context.compiler.host.clone(),
            },
            arguments,
            required_inputs,
        })
    }

    fn validate_mappings(&self) -> Result<(), BypassReason> {
        if !self.context.working_dir.is_absolute() {
            return Err(BypassReason::RelativeWorkingDirectory(
                self.context.working_dir.clone(),
            ));
        }
        let mut roots = BTreeSet::new();
        let mut placeholders = BTreeSet::new();
        for mapping in &self.mappings {
            if !mapping.root.is_absolute() {
                return Err(BypassReason::RelativePathMapping(mapping.root.clone()));
            }
            if mapping.placeholder.is_empty()
                || !mapping
                    .placeholder
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || !roots.insert(normalize_components(&mapping.root))
                || !placeholders.insert(&mapping.placeholder)
            {
                return Err(BypassReason::InvalidPathPlaceholder(
                    mapping.placeholder.clone(),
                ));
            }
        }
        Ok(())
    }

    fn normalize_argument(&self, argument: &Argument) -> Result<String, BypassReason> {
        match argument {
            Argument::Plain(value) => Ok(value.clone()),
            Argument::Path { flag, path } => Ok(format!("{flag}={}", self.normalize_path(path)?)),
            Argument::SearchPath { kind, path } => {
                Ok(format!("-L{kind}={}", self.normalize_path(path)?))
            }
            Argument::Extern { name, path } => match path {
                Some(path) => Ok(format!("--extern={name}={}", self.normalize_path(path)?)),
                None => Ok(format!("--extern={name}")),
            },
            Argument::Emit(emits) => Ok(format!(
                "--emit={}",
                emits
                    .iter()
                    .map(|emit| match &emit.path {
                        Some(path) => self
                            .normalize_path(path)
                            .map(|path| format!("{}={path}", emit.kind)),
                        None => Ok(emit.kind.clone()),
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .join(",")
            )),
            Argument::RemapPath { from, to } => Ok(format!(
                "--remap-path-prefix={}={}",
                self.normalize_path(from)?,
                to
            )),
        }
    }

    /// Environment values enter the key verbatim, because rustc may embed one
    /// through `env!`: unlike a path used to locate an input, changing the value
    /// changes the artifact.
    ///
    /// A name in `portable_environment` is the exception the caller has earned.
    /// Its value normalizes like any other path, so two checkouts agree on it.
    fn environment_descriptor(&self) -> Result<BTreeMap<String, Option<String>>, BypassReason> {
        self.context
            .environment
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    Some(value) if self.context.portable_environment.contains(name) => {
                        Some(self.normalize_path(Path::new(value))?)
                    }
                    value => value.clone(),
                };
                Ok((name.clone(), value))
            })
            .collect()
    }

    fn normalize_path(&self, path: &Path) -> Result<String, BypassReason> {
        normalize_mapped_path(path, &self.context.working_dir, &self.mappings)
    }
}

fn denormalize_path(value: &str, mappings: &[PathMapping]) -> Result<PathBuf, BypassReason> {
    for mapping in mappings {
        let prefix = format!("${{{}}}", mapping.placeholder);
        let suffix = if value == prefix {
            ""
        } else if let Some(suffix) = value.strip_prefix(&format!("{prefix}/")) {
            suffix
        } else {
            continue;
        };
        if !mapping.root.is_absolute()
            || (!suffix.is_empty()
                && suffix.split('/').any(|component| {
                    component.is_empty()
                        || matches!(component, "." | "..")
                        || component.contains('\\')
                }))
        {
            return Err(BypassReason::InvalidPredictedInput(value.into()));
        }
        let mut path = normalize_components(&mapping.root);
        path.extend(suffix.split('/').filter(|component| !component.is_empty()));
        return Ok(path);
    }
    Err(BypassReason::InvalidPredictedInput(value.into()))
}

fn normalize_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn absolute_path(path: &Path, working_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_components(path)
    } else {
        normalize_components(&working_dir.join(path))
    }
}

fn slash_path(path: &Path) -> Result<String, BypassReason> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(
                value
                    .to_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| BypassReason::NonUtf8Path(path.to_path_buf())),
            ),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))
}

#[cfg(test)]
#[path = "rustc_cache_tests.rs"]
mod tests;
