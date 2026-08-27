//! Conservative parsing and action-key construction for C and C++ compiles.
//!
//! Cargo build scripts using the `cc` crate compile C and C++ through a
//! gcc-style driver. This adapter models the narrow shape those build scripts
//! produce -- one source, one object, `-c` -- and rejects everything else. As
//! in the rustc adapter, callers should treat [`CcBypassReason`] as a safe
//! cache bypass: run the real compiler and publish nothing.
//!
//! Two properties separate this adapter from a traditional compiler cache.
//! Preprocessor inputs are discovered from a depfile the adapter injects
//! itself, so the key names the headers the compilation actually read; and the
//! directories those headers were searched from contribute a name manifest, so
//! a header that newly *shadows* one of them changes the key even though every
//! previously-read file is byte-identical.
//!
//! Path mappings are shared with the rustc adapter so both agree on which host
//! roots are checkout-specific.
#![deny(missing_docs)]

use mbx_cache_core::{CacheDigest, canonical_json};
use mbx_cache_rustc::{BypassReason as RustcBypassReason, PathMapping, normalize_mapped_path};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

mod depfile;

pub use depfile::{CcDepfile, CcDiscoveredInputs, INCLUDE_MANIFEST_PREFIX};

/// Schema version embedded in canonical cc action descriptors.
pub const ACTION_SCHEMA_VERSION: u8 = 1;
/// Version of the cc argument and input model used to construct keys.
pub const ADAPTER_VERSION: u8 = 1;

/// Maximum discovered inputs, including include-manifest entries.
pub const MAX_PREDICTED_INPUTS: usize = 16 * 1024;
/// Maximum total bytes digested for one action.
pub const MAX_INPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Maximum file names summarized across all include manifests.
pub const MAX_MANIFEST_ENTRIES: usize = 16 * 1024;

/// Environment variables whose values enter every cc action key.
///
/// These change the compiler's own behavior without appearing in argv. They
/// are recorded even when unset, so setting one is distinguishable from
/// leaving it unset.
pub const KEYED_ENVIRONMENT: &[&str] = &[
    "IPHONEOS_DEPLOYMENT_TARGET",
    "LANG",
    "LC_ALL",
    "LC_MESSAGES",
    "MACOSX_DEPLOYMENT_TARGET",
    "SDKROOT",
    "SOURCE_DATE_EPOCH",
    "TVOS_DEPLOYMENT_TARGET",
    "WATCHOS_DEPLOYMENT_TARGET",
    "XROS_DEPLOYMENT_TARGET",
];

/// Environment variables that force a bypass when set.
///
/// Each one either injects search paths the argv model cannot see, redirects
/// sub-tool resolution beneath the identity probe, or makes the driver write an
/// output the adapter does not model.
pub const BYPASS_ENVIRONMENT: &[&str] = &[
    "CPATH",
    "COMPILER_PATH",
    "CPLUS_INCLUDE_PATH",
    "C_INCLUDE_PATH",
    "DEPENDENCIES_OUTPUT",
    "GCC_EXEC_PREFIX",
    "OBJC_INCLUDE_PATH",
    "SUNPRO_DEPENDENCIES",
];

/// Absolute roots whose contents are keyed verbatim rather than through a
/// placeholder.
///
/// Files beneath these roots are still digested; keying the path verbatim only
/// declares that the path itself is a machine property rather than a
/// checkout-specific one, which is what makes system headers shareable between
/// worktrees on one machine.
pub const SYSTEM_ROOTS: &[&str] = &[
    "/Applications/Xcode.app",
    "/Library/Developer",
    "/nix/store",
    "/usr/include",
    "/usr/lib",
    "/usr/local/include",
];

const SUPPORTED_F_FLAGS: &[&str] = &[
    "PIC",
    "PIE",
    "asynchronous-unwind-tables",
    "color-diagnostics",
    "data-sections",
    "diagnostics-color",
    "exceptions",
    "function-sections",
    "merge-all-constants",
    "no-asynchronous-unwind-tables",
    "no-builtin",
    "no-common",
    "no-exceptions",
    "no-omit-frame-pointer",
    "no-plt",
    "no-rtti",
    "no-strict-aliasing",
    "omit-frame-pointer",
    "pic",
    "pie",
    "rtti",
    "short-enums",
    "signed-char",
    "stack-protector",
    "stack-protector-all",
    "stack-protector-strong",
    "strict-aliasing",
    "unsigned-char",
    "visibility",
    "visibility-inlines-hidden",
    "wrapv",
];

const SUPPORTED_M_FLAGS: &[&str] = &[
    "32",
    "64",
    "arch",
    "arm",
    "avx",
    "avx2",
    "cpu",
    "float-abi",
    "fma",
    "fpu",
    "iphoneos-version-min",
    "macosx-version-min",
    "no-omit-leaf-frame-pointer",
    "omit-leaf-frame-pointer",
    "sse",
    "sse2",
    "sse3",
    "sse4.1",
    "sse4.2",
    "thumb",
    "tune",
];

const SUPPORTED_O_FLAGS: &[&str] = &[
    "-O", "-O0", "-O1", "-O2", "-O3", "-Ofast", "-Og", "-Os", "-Oz",
];

const SUPPORTED_G_FLAGS: &[&str] = &[
    "-g",
    "-g0",
    "-g1",
    "-g2",
    "-g3",
    "-gdwarf-2",
    "-gdwarf-3",
    "-gdwarf-4",
    "-gdwarf-5",
];

const SUPPORTED_BARE_FLAGS: &[&str] = &[
    "-ansi",
    "-nostdinc",
    "-nostdinc++",
    "-pedantic",
    "-pedantic-errors",
    "-pipe",
    "-pthread",
    "-w",
];

const SEPARATE_PATH_FLAGS: &[&str] = &[
    "-idirafter",
    "-imacros",
    "-include",
    "-iquote",
    "-isysroot",
    "-isystem",
];

const TOOL_PASSTHROUGH_FLAGS: &[&str] = &["-Xassembler", "-Xclang", "-Xlinker", "-Xpreprocessor"];

const COMPILER_QUERY_FLAGS: &[&str] = &[
    "--help",
    "--version",
    "-###",
    // The `cc` crate probes with `-?` to tell an MSVC-style driver from a
    // gcc-style one; neither answer is a compilation.
    "-?",
    "-dumpmachine",
    "-dumpversion",
    "-v",
];

/// Flags that rewrite a path prefix in the compiler's own output.
///
/// The left side is a real path and normalizes like any other; the right side
/// is the text it is replaced with and enters the key verbatim.
const PREFIX_MAP_FLAGS: &[&str] = &[
    "-fdebug-prefix-map",
    "-ffile-prefix-map",
    "-fmacro-prefix-map",
];

impl CcBypassReason {
    /// A stable, low-cardinality name for this reason.
    ///
    /// Many variants carry a path or a flag, so `Display` text cannot be
    /// aggregated; statistics group by this instead.
    pub fn kind(&self) -> &'static str {
        self.into()
    }
}

/// Reason a C or C++ invocation cannot safely use the action cache.
///
/// A bypass is an expected conservative outcome, not a compiler error. Match on
/// [`CcBypassReason::kind`] for aggregation rather than on the variants.
#[derive(Debug, Clone, PartialEq, Eq, Error, strum::IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum CcBypassReason {
    /// An argument cannot be represented in the canonical UTF-8 key.
    #[error("compiler argument {index} is not valid UTF-8")]
    NonUtf8Argument {
        /// Zero-based index in the argument slice.
        index: usize,
    },
    /// The driver was handed an argument file.
    #[error("compiler response file is not modeled by the cache adapter: {0}")]
    ResponseFile(String),
    /// A compiler flag is not modeled by this adapter version.
    #[error("compiler flag is not modeled by the cache adapter: {0}")]
    UnknownFlag(String),
    /// A recognized flag was given without its value.
    #[error("compiler flag {0} is missing its value")]
    MissingValue(String),
    /// The invocation asks the driver about itself rather than compiling.
    #[error("compiler invocation queries the driver instead of compiling")]
    CompilerQuery,
    /// The invocation is not a single-object compile.
    #[error("compiler invocation does not compile with -c")]
    NotACompile,
    /// The invocation emits preprocessed source or assembly.
    #[error("compiler invocation emits a non-object output: {0}")]
    NonObjectOutput(String),
    /// The source arrives on standard input and cannot be rediscovered.
    #[error("compiler invocation reads its source from standard input")]
    StandardInput,
    /// No source file was given.
    #[error("compiler invocation names no source file")]
    MissingInput,
    /// More than one source file was given.
    #[error("compiler invocation names more than one source file")]
    MultipleInputs,
    /// No `-o` was given, so the object name follows driver defaults.
    #[error("compiler invocation names no output file")]
    MissingOutput,
    /// The source language is outside the modeled set.
    #[error("compiler input language is not modeled by the cache adapter: {0}")]
    UnsupportedLanguage(String),
    /// The caller asked for its own dependency output.
    #[error("compiler invocation requests its own dependency output: {0}")]
    CallerDependencyFlags(String),
    /// Precompiled headers are not byte-hermetic key material.
    #[error("precompiled headers are not modeled by the cache adapter: {0}")]
    PrecompiledHeader(String),
    /// Coverage instrumentation writes outputs beside the object.
    #[error("coverage instrumentation is not modeled by the cache adapter: {0}")]
    CoverageInstrumentation(String),
    /// Split debug info writes a `.dwo` beside the object.
    #[error("split debug output is not modeled by the cache adapter: {0}")]
    SplitDebugOutput(String),
    /// Temporary files are preserved beside the object.
    #[error("preserved temporaries are not modeled by the cache adapter: {0}")]
    SaveTemps(String),
    /// An option is smuggled to a sub-tool the adapter cannot model.
    #[error("compiler flag forwards options to another tool: {0}")]
    ToolPassthrough(String),
    /// A compiler plugin makes the output depend on unmodeled code.
    #[error("compiler plugins are not modeled by the cache adapter: {0}")]
    Plugin(String),
    /// The object depends on the machine's own CPU rather than on named inputs.
    #[error("compiler flag tunes for the local CPU: {0}")]
    LocalCpuTarget(String),
    /// The driver is not a gcc-style or clang-style compiler.
    #[error("compiler driver is not modeled by the cache adapter: {0}")]
    UnsupportedCompilerDriver(String),
    /// The identity probe could not be run or parsed.
    #[error("could not establish compiler identity: {0}")]
    CompilerIdentityUnavailable(String),
    /// An environment variable outside the modeled set is set.
    #[error("environment variable {0} changes the compilation in an unmodeled way")]
    UnsupportedEnvironment(String),
    /// The shim could not be told which real compiler to run.
    #[error("no real compiler was pinned for the cc shim")]
    RealCompilerUnpinned,
    /// A read file expands a timestamp macro, so the object is not a function
    /// of its inputs.
    #[error("input expands a timestamp macro: {0}")]
    EmbeddedTimestampMacro(PathBuf),
    /// The injected depfile could not be parsed exactly.
    #[error("could not model the compiler depfile: {0}")]
    MalformedDepfile(String),
    /// The injected depfile could not be read.
    #[error("could not read the compiler depfile {path}: {message}")]
    DepfileRead {
        /// Depfile that could not be read.
        path: PathBuf,
        /// Underlying error text.
        message: String,
    },
    /// The action exceeds an input, byte, or manifest bound.
    #[error("compilation reads more inputs than the cache adapter models")]
    TooManyInputs,
    /// An absolute path lies outside every mapped and system root.
    #[error("path is outside every modeled root: {0}")]
    UnmappedAbsolutePath(PathBuf),
    /// A path cannot be represented in the canonical UTF-8 key.
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    /// The compiler working directory is not absolute.
    #[error("compiler working directory is not absolute: {0}")]
    RelativeWorkingDirectory(PathBuf),
    /// A configured path mapping root is not absolute.
    #[error("path mapping root is not absolute: {0}")]
    RelativePathMapping(PathBuf),
    /// A configured placeholder is empty, duplicated, or not a bare name.
    #[error("invalid path mapping placeholder: {0}")]
    InvalidPathPlaceholder(String),
    /// A required input never appeared among the discovered inputs.
    #[error("required input is missing from the discovered inputs: {0}")]
    MissingRequiredInput(String),
    /// An input digest is malformed.
    #[error("invalid digest for input: {0}")]
    InvalidInputDigest(String),
    /// One normalized path carries two different digests.
    #[error("conflicting digests for input: {0}")]
    ConflictingInput(String),
    /// An input could not be read.
    #[error("could not read input {path}: {message}")]
    InputRead {
        /// Input that could not be read.
        path: PathBuf,
        /// Underlying error text.
        message: String,
    },
    /// An input changed between discovery and publication.
    #[error("input changed during the compilation: {0}")]
    InputChanged(PathBuf),
    /// An input was written while the compiler ran.
    #[error("input was modified during the compilation: {0}")]
    InputModifiedDuringCompilation(PathBuf),
    /// Discovery and the action disagree about the working directory.
    #[error("discovered inputs use a different working directory")]
    DiscoveryWorkingDirectory,
    /// A prediction uses a schema this adapter version does not model.
    #[error("action prediction is not modeled by this adapter version")]
    UnsupportedPrediction,
    /// A predicted input name cannot be resolved back to a host path.
    #[error("invalid predicted input: {0}")]
    InvalidPredictedInput(String),
    /// Canonical serialization failed.
    #[error("could not serialize the action descriptor: {0}")]
    Serialization(String),
}

impl From<RustcBypassReason> for CcBypassReason {
    /// Translate the shared path-normalization errors into this adapter's own
    /// reasons, so a cc bypass never reports a rustc kind.
    fn from(reason: RustcBypassReason) -> Self {
        match reason {
            RustcBypassReason::UnmappedAbsolutePath(path) => Self::UnmappedAbsolutePath(path),
            RustcBypassReason::NonUtf8Path(path) => Self::NonUtf8Path(path),
            other => Self::UnknownFlag(other.kind().into()),
        }
    }
}

/// Source language a driver invocation compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CcLanguage {
    /// C, driven through `CC`.
    C,
    /// C++, driven through `CXX`.
    Cxx,
}

impl CcLanguage {
    /// Shim file stem that selects this language.
    pub fn shim_stem(self) -> &'static str {
        match self {
            Self::C => "mbx-cc",
            Self::Cxx => "mbx-cxx",
        }
    }

    /// Default driver name to fall back to when no real compiler is pinned.
    pub fn default_driver(self) -> &'static str {
        match self {
            Self::C => "cc",
            Self::Cxx => "c++",
        }
    }
}

/// Compiler family, which decides how the identity is assembled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CcCompilerFamily {
    /// GCC, which compiles objects through an external assembler.
    Gcc,
    /// Upstream LLVM clang.
    Clang,
    /// Apple's clang distribution.
    AppleClang,
}

impl CcCompilerFamily {
    /// Stable name recorded in the action key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gcc => "gcc",
            Self::Clang => "clang",
            Self::AppleClang => "apple-clang",
        }
    }

    /// Whether objects are produced through a separate assembler binary whose
    /// version therefore belongs in the identity.
    pub fn uses_external_assembler(self) -> bool {
        matches!(self, Self::Gcc)
    }

    /// Classify a driver from its verbose probe output.
    pub fn classify(probe: &str) -> Result<Self, CcBypassReason> {
        if probe.contains("Apple clang version") {
            Ok(Self::AppleClang)
        } else if probe.contains("clang version") {
            Ok(Self::Clang)
        } else if probe.contains("gcc version") {
            Ok(Self::Gcc)
        } else {
            Err(CcBypassReason::UnsupportedCompilerDriver(
                probe.lines().next().unwrap_or_default().into(),
            ))
        }
    }
}

/// Compiler properties that distinguish incompatible objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcCompilerIdentity {
    /// Driver family.
    pub family: CcCompilerFamily,
    /// Complete verbose probe output, verbatim.
    pub version_text: String,
    /// Target triple the driver reports.
    pub target: String,
    /// Resolved assembler and its version, for families that use one.
    ///
    /// GCC assembles through binutils, whose version changes object bytes
    /// without changing anything `gcc -v` prints. Clang assembles internally,
    /// so this is empty there.
    pub assembler: String,
}

/// One file input paired with the digest used in the action key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcActionInput {
    /// Absolute host path used to read and verify the input, or an
    /// include-manifest pseudo-path.
    pub path: PathBuf,
    /// Digest of the input contents, or of the directory's name manifest.
    pub digest: CacheDigest,
}

/// External information needed to construct a canonical cc action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcActionContext {
    /// Identity of the compiler that produces the object.
    pub compiler: CcCompilerIdentity,
    /// Absolute directory in which the compiler runs.
    pub working_dir: PathBuf,
    /// Host roots replaced with stable placeholders in the key.
    pub path_mappings: Vec<PathMapping>,
    /// Environment inputs and their observed values.
    pub environment: BTreeMap<String, Option<String>>,
    /// Complete set of direct and discovered file inputs.
    pub inputs: Vec<CcActionInput>,
}

/// Canonical action descriptor and its content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcAction {
    /// Digest of `bytes`, used as the action-cache key.
    pub digest: CacheDigest,
    /// Canonical serialized action descriptor.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct CcCompilerDescriptor {
    assembler: String,
    family: String,
    target: String,
    version_text: String,
}

#[derive(Debug, Serialize)]
struct CcInputDescriptor {
    digest: CacheDigest,
    path: String,
}

#[derive(Debug, Serialize)]
struct CcActionDescriptor {
    version: u8,
    kind: &'static str,
    adapter_version: u8,
    compiler: CcCompilerDescriptor,
    arguments: Vec<String>,
    environment: BTreeMap<String, Option<String>>,
    inputs: Vec<CcInputDescriptor>,
}

#[derive(Debug, Serialize)]
struct CcInvocationDescriptor {
    version: u8,
    kind: &'static str,
    adapter_version: u8,
    compiler: CcCompilerDescriptor,
    arguments: Vec<String>,
    required_inputs: Vec<String>,
}

/// Normalized input names from the last successful execution of one modeled
/// compile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CcInputPrediction {
    /// Prediction schema version.
    pub version: u8,
    /// Normalized input paths, including include-manifest entries.
    pub inputs: Vec<String>,
    /// Names of environment variables that entered the key.
    pub environment: Vec<String>,
    /// Compiler wall time from the successful invocation that produced this
    /// prediction. Zero means no timing hint was recorded.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub compiler_duration_ns: u64,
    /// Source file name associated with the timing hint.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_name: String,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// One parsed and admitted argument.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Argument {
    /// Keyed verbatim.
    Plain(String),
    /// Keyed with its path normalized.
    Path { flag: String, path: PathBuf },
    /// A prefix rewrite: the source path normalizes, the replacement does not.
    PrefixMap {
        flag: String,
        from: PathBuf,
        to: String,
    },
    /// The source file.
    Source(PathBuf),
}

/// A parsed, admitted C or C++ compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcInvocation {
    arguments: Vec<Argument>,
    source: PathBuf,
    output: PathBuf,
    include_dirs: Vec<PathBuf>,
    required_inputs: Vec<PathBuf>,
    language: CcLanguage,
    sysroot: Option<PathBuf>,
}

impl CcInvocation {
    /// Parse a driver command line, admitting only modeled single-object
    /// compiles.
    pub fn parse(arguments: &[OsString]) -> Result<Self, CcBypassReason> {
        Parser::new(arguments).parse()
    }

    /// Source file this invocation compiles.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Object file this invocation produces.
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Include search directories named on the command line, in order.
    pub fn include_dirs(&self) -> &[PathBuf] {
        &self.include_dirs
    }

    /// Files that must appear among the discovered inputs.
    pub fn required_inputs(&self) -> &[PathBuf] {
        &self.required_inputs
    }

    /// Language the driver compiles.
    pub fn language(&self) -> CcLanguage {
        self.language
    }

    /// Sysroot named on the command line, if any.
    pub fn sysroot(&self) -> Option<&Path> {
        self.sysroot.as_deref()
    }

    /// Short label used for timing statistics.
    pub fn source_name(&self) -> String {
        self.source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Arguments to append so the driver writes a dependency list beside the
    /// object.
    ///
    /// `-MD` rather than `-MMD`: system headers are exactly the inputs most
    /// likely to change without any other key component noticing, because the
    /// compiler identity does not cover the C library or the platform SDK.
    pub fn dependency_arguments(&self, depfile: &Path) -> Vec<OsString> {
        vec!["-MD".into(), "-MF".into(), depfile.into()]
    }

    /// Digest of the pre-input fingerprint, used to look up a prediction.
    pub fn invocation_digest(
        &self,
        context: &CcActionContext,
    ) -> Result<CacheDigest, CcBypassReason> {
        let builder = ActionBuilder::new(self, context.clone());
        let descriptor = builder.invocation_descriptor()?;
        let bytes = canonical_json(&descriptor)
            .map_err(|error| CcBypassReason::Serialization(error.to_string()))?;
        Ok(CacheDigest::blake3(&bytes))
    }

    /// Build the canonical action for this invocation and its discovered
    /// inputs.
    pub fn action(&self, context: CcActionContext) -> Result<CcAction, CcBypassReason> {
        ActionBuilder::new(self, context).build()
    }

    /// Record the normalized inputs of a successful compile so the next cold
    /// invocation can rebuild the same key before compiling.
    pub fn prediction(
        &self,
        context: &CcActionContext,
        compiler_duration_ns: u64,
    ) -> Result<CcInputPrediction, CcBypassReason> {
        let builder = ActionBuilder::new(self, context.clone());
        let mut inputs = context
            .inputs
            .iter()
            .map(|input| builder.normalize_input_path(&input.path))
            .collect::<Result<Vec<_>, _>>()?;
        inputs.sort();
        inputs.dedup();
        Ok(CcInputPrediction {
            version: 1,
            inputs,
            environment: context.environment.keys().cloned().collect(),
            compiler_duration_ns,
            source_name: self.source_name(),
        })
    }
}

impl CcInputPrediction {
    /// Rehash the predicted paths and recompute include manifests. The caller
    /// still recomputes the full action digest, so changed inputs are misses.
    pub fn discover(
        &self,
        working_dir: &Path,
        path_mappings: &[PathMapping],
    ) -> Result<CcDiscoveredInputs, CcBypassReason> {
        if self.version != 1 {
            return Err(CcBypassReason::UnsupportedPrediction);
        }
        if self.inputs.len() > MAX_PREDICTED_INPUTS || self.environment.len() > 4 * 1024 {
            return Err(CcBypassReason::UnsupportedPrediction);
        }
        let mappings = PathMapping::ordered(path_mappings);
        let mut files = BTreeSet::new();
        let mut directories = BTreeSet::new();
        for entry in &self.inputs {
            match entry.strip_prefix(INCLUDE_MANIFEST_PREFIX) {
                Some(directory) => {
                    directories.insert(denormalize_path(directory, &mappings)?);
                }
                None => {
                    files.insert(denormalize_path(entry, &mappings)?);
                }
            }
        }
        CcDiscoveredInputs::collect(working_dir, files, directories)
    }
}

/// Resolve a normalized key path back to a host path.
///
/// Placeholder entries expand through their mapping; a verbatim entry is
/// accepted only when it still lies beneath an admitted system root, so a
/// prediction cannot name an arbitrary absolute path.
fn denormalize_path(value: &str, mappings: &[PathMapping]) -> Result<PathBuf, CcBypassReason> {
    for mapping in mappings {
        let prefix = format!("${{{}}}", mapping.placeholder);
        let suffix = if value == prefix {
            ""
        } else if let Some(suffix) = value.strip_prefix(&format!("{prefix}/")) {
            suffix
        } else {
            continue;
        };
        if !mapping.root.is_absolute() || !safe_suffix(suffix) {
            return Err(CcBypassReason::InvalidPredictedInput(value.into()));
        }
        let mut path = normalize_components(&mapping.root);
        path.extend(suffix.split('/').filter(|component| !component.is_empty()));
        return Ok(path);
    }
    // A verbatim entry names a machine path rather than a placeholder. It is
    // admitted only beneath a system root, and only spelled literally: a
    // traversal component would let a prediction reach outside that root.
    let path = PathBuf::from(value);
    if path.is_absolute() && is_system_path(&path) && normalize_components(&path) == path {
        return Ok(path);
    }
    Err(CcBypassReason::InvalidPredictedInput(value.into()))
}

fn safe_suffix(suffix: &str) -> bool {
    suffix.is_empty()
        || !suffix.split('/').any(|component| {
            component.is_empty() || matches!(component, "." | "..") || component.contains('\\')
        })
}

/// Whether a path lies beneath a root whose location is a machine property.
pub fn is_system_path(path: &Path) -> bool {
    SYSTEM_ROOTS
        .iter()
        .any(|root| path.starts_with(Path::new(root)))
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

/// Read the modeled environment, rejecting variables that change the compile in
/// a way the argv model cannot see.
pub fn environment_inputs<F>(
    lookup: F,
    sysroot: Option<&Path>,
) -> Result<BTreeMap<String, Option<String>>, CcBypassReason>
where
    F: Fn(&str) -> Option<String>,
{
    for name in BYPASS_ENVIRONMENT {
        if lookup(name).is_some() {
            return Err(CcBypassReason::UnsupportedEnvironment((*name).into()));
        }
    }
    let mut environment = BTreeMap::new();
    for name in KEYED_ENVIRONMENT {
        // An explicit `-isysroot` on the command line already pins the SDK, and
        // it is what the driver honors, so the variable stops being an input.
        if *name == "SDKROOT" && sysroot.is_some() {
            continue;
        }
        environment.insert((*name).to_string(), lookup(name));
    }
    Ok(environment)
}

struct ActionBuilder<'a> {
    invocation: &'a CcInvocation,
    context: CcActionContext,
    mappings: Vec<PathMapping>,
}

impl<'a> ActionBuilder<'a> {
    fn new(invocation: &'a CcInvocation, mut context: CcActionContext) -> Self {
        context.path_mappings = PathMapping::ordered(&context.path_mappings);
        let mappings = context.path_mappings.clone();
        Self {
            invocation,
            context,
            mappings,
        }
    }

    fn build(self) -> Result<CcAction, CcBypassReason> {
        self.validate_mappings()?;
        let invocation = self.invocation_descriptor()?;

        let mut inputs = BTreeMap::<String, CacheDigest>::new();
        for input in &self.context.inputs {
            input.digest.validate().map_err(|_| {
                CcBypassReason::InvalidInputDigest(input.path.display().to_string())
            })?;
            let path = self.normalize_input_path(&input.path)?;
            if inputs
                .insert(path.clone(), input.digest.clone())
                .is_some_and(|existing| existing != input.digest)
            {
                return Err(CcBypassReason::ConflictingInput(path));
            }
        }
        let required = self
            .invocation
            .required_inputs
            .iter()
            .map(|path| self.normalize_path(path))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if let Some(missing) = required.iter().find(|path| !inputs.contains_key(*path)) {
            return Err(CcBypassReason::MissingRequiredInput(missing.clone()));
        }
        let inputs = inputs
            .into_iter()
            .map(|(path, digest)| CcInputDescriptor { path, digest })
            .collect();
        let descriptor = CcActionDescriptor {
            version: ACTION_SCHEMA_VERSION,
            kind: "cc",
            adapter_version: ADAPTER_VERSION,
            compiler: invocation.compiler,
            arguments: invocation.arguments,
            environment: self.context.environment.clone(),
            inputs,
        };
        let bytes = canonical_json(&descriptor)
            .map_err(|error| CcBypassReason::Serialization(error.to_string()))?;
        let digest = CacheDigest::blake3(&bytes);
        Ok(CcAction { digest, bytes })
    }

    fn invocation_descriptor(&self) -> Result<CcInvocationDescriptor, CcBypassReason> {
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
        Ok(CcInvocationDescriptor {
            version: ACTION_SCHEMA_VERSION,
            kind: "cc",
            adapter_version: ADAPTER_VERSION,
            compiler: self.compiler_descriptor(),
            arguments,
            required_inputs,
        })
    }

    fn compiler_descriptor(&self) -> CcCompilerDescriptor {
        CcCompilerDescriptor {
            assembler: self.context.compiler.assembler.clone(),
            family: self.context.compiler.family.as_str().into(),
            target: self.context.compiler.target.clone(),
            version_text: self.context.compiler.version_text.clone(),
        }
    }

    fn validate_mappings(&self) -> Result<(), CcBypassReason> {
        if !self.context.working_dir.is_absolute() {
            return Err(CcBypassReason::RelativeWorkingDirectory(
                self.context.working_dir.clone(),
            ));
        }
        let mut roots = BTreeSet::new();
        let mut placeholders = BTreeSet::new();
        for mapping in &self.mappings {
            if !mapping.root.is_absolute() {
                return Err(CcBypassReason::RelativePathMapping(mapping.root.clone()));
            }
            if mapping.placeholder.is_empty()
                || !mapping
                    .placeholder
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || !roots.insert(normalize_components(&mapping.root))
                || !placeholders.insert(&mapping.placeholder)
            {
                return Err(CcBypassReason::InvalidPathPlaceholder(
                    mapping.placeholder.clone(),
                ));
            }
        }
        Ok(())
    }

    fn normalize_argument(&self, argument: &Argument) -> Result<String, CcBypassReason> {
        match argument {
            Argument::Plain(value) => Ok(value.clone()),
            Argument::Path { flag, path } => Ok(format!("{flag}={}", self.normalize_path(path)?)),
            Argument::PrefixMap { flag, from, to } => {
                Ok(format!("{flag}={}={to}", self.normalize_path(from)?))
            }
            Argument::Source(path) => Ok(self.normalize_path(path)?),
        }
    }

    /// Normalize a path that names a compilation input or search root.
    ///
    /// A path beneath a mapped root becomes a placeholder so equivalent
    /// checkouts agree. A path beneath a system root stays verbatim: its
    /// location is a property of the machine, and its contents are digested
    /// like any other input.
    fn normalize_path(&self, path: &Path) -> Result<String, CcBypassReason> {
        match normalize_mapped_path(path, &self.context.working_dir, &self.mappings) {
            Ok(normalized) => Ok(normalized),
            Err(reason) => {
                let absolute = absolute_path(path, &self.context.working_dir);
                if is_system_path(&absolute) {
                    return absolute
                        .to_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| CcBypassReason::NonUtf8Path(absolute.clone()));
                }
                Err(reason.into())
            }
        }
    }

    fn normalize_input_path(&self, path: &Path) -> Result<String, CcBypassReason> {
        match path.to_str().and_then(|path| {
            path.strip_prefix(INCLUDE_MANIFEST_PREFIX)
                .map(ToOwned::to_owned)
        }) {
            Some(directory) => Ok(format!(
                "{INCLUDE_MANIFEST_PREFIX}{}",
                self.normalize_path(Path::new(&directory))?
            )),
            None => self.normalize_path(path),
        }
    }
}

fn absolute_path(path: &Path, working_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_components(path)
    } else {
        normalize_components(&working_dir.join(path))
    }
}

struct Parser<'a> {
    arguments: &'a [OsString],
    index: usize,
    parsed: Vec<Argument>,
    source: Option<PathBuf>,
    output: Option<PathBuf>,
    include_dirs: Vec<PathBuf>,
    required_inputs: Vec<PathBuf>,
    sysroot: Option<PathBuf>,
    explicit_language: Option<CcLanguage>,
    compiling: bool,
}

impl<'a> Parser<'a> {
    fn new(arguments: &'a [OsString]) -> Self {
        Self {
            arguments,
            index: 0,
            parsed: Vec::new(),
            source: None,
            output: None,
            include_dirs: Vec::new(),
            required_inputs: Vec::new(),
            sysroot: None,
            explicit_language: None,
            compiling: false,
        }
    }

    fn parse(mut self) -> Result<CcInvocation, CcBypassReason> {
        while self.index < self.arguments.len() {
            let value = self.current()?.to_string();
            self.index += 1;
            if value == "-" {
                return Err(CcBypassReason::StandardInput);
            }
            if let Some(argfile) = value.strip_prefix('@') {
                return Err(CcBypassReason::ResponseFile(argfile.into()));
            }
            if value.starts_with('-') {
                self.parse_flag(&value)?;
            } else {
                self.parse_input(&value)?;
            }
        }

        if !self.compiling {
            return Err(CcBypassReason::NotACompile);
        }
        let source = self.source.clone().ok_or(CcBypassReason::MissingInput)?;
        let output = self.output.clone().ok_or(CcBypassReason::MissingOutput)?;
        let language = self.language(&source)?;
        self.required_inputs.push(source.clone());
        Ok(CcInvocation {
            arguments: self.parsed,
            source,
            output,
            include_dirs: self.include_dirs,
            required_inputs: self.required_inputs,
            language,
            sysroot: self.sysroot,
        })
    }

    fn language(&self, source: &Path) -> Result<CcLanguage, CcBypassReason> {
        if let Some(language) = self.explicit_language {
            return Ok(language);
        }
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        match extension {
            "c" => Ok(CcLanguage::C),
            "cc" | "cpp" | "cxx" | "c++" => Ok(CcLanguage::Cxx),
            _ => Err(CcBypassReason::UnsupportedLanguage(
                source.display().to_string(),
            )),
        }
    }

    fn current(&self) -> Result<&str, CcBypassReason> {
        self.arguments[self.index]
            .to_str()
            .ok_or(CcBypassReason::NonUtf8Argument { index: self.index })
    }

    fn take_value(&mut self, flag: &str, inline: Option<&str>) -> Result<String, CcBypassReason> {
        if let Some(value) = inline
            && !value.is_empty()
        {
            return Ok(value.into());
        }
        if self.index >= self.arguments.len() {
            return Err(CcBypassReason::MissingValue(flag.into()));
        }
        let value = self.current()?.to_string();
        self.index += 1;
        Ok(value)
    }

    fn parse_input(&mut self, value: &str) -> Result<(), CcBypassReason> {
        if self.source.is_some() {
            return Err(CcBypassReason::MultipleInputs);
        }
        let path = PathBuf::from(value);
        // With an explicit `-x`, the driver ignores the extension entirely;
        // without one, the extension is the only thing that decides the
        // language, so an unmodeled extension has to bypass here.
        if self.explicit_language.is_none() {
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default();
            if !matches!(extension, "c" | "cc" | "cpp" | "cxx" | "c++") {
                return Err(CcBypassReason::UnsupportedLanguage(value.into()));
            }
        }
        self.source = Some(path.clone());
        self.parsed.push(Argument::Source(path));
        Ok(())
    }

    fn parse_flag(&mut self, value: &str) -> Result<(), CcBypassReason> {
        if COMPILER_QUERY_FLAGS.contains(&value) || value.starts_with("-print") {
            return Err(CcBypassReason::CompilerQuery);
        }
        if matches!(value, "-E" | "-S") {
            return Err(CcBypassReason::NonObjectOutput(value.into()));
        }
        if value.starts_with("-M") {
            return Err(CcBypassReason::CallerDependencyFlags(value.into()));
        }
        if value.starts_with("-save-temps") {
            return Err(CcBypassReason::SaveTemps(value.into()));
        }
        if value == "--coverage" {
            return Err(CcBypassReason::CoverageInstrumentation(value.into()));
        }
        if TOOL_PASSTHROUGH_FLAGS.contains(&value)
            || value.starts_with("-Wp,")
            || value.starts_with("-Wa,")
            || value.starts_with("-Wl,")
        {
            // The forwarded options are the compilation's real inputs and they
            // are not modeled, so consuming the value would not make this safe.
            return Err(CcBypassReason::ToolPassthrough(value.into()));
        }
        if value.starts_with("-include-pch") || value == "-emit-pch" {
            return Err(CcBypassReason::PrecompiledHeader(value.into()));
        }

        if value == "-c" {
            self.compiling = true;
            self.parsed.push(Argument::Plain(value.into()));
            return Ok(());
        }
        if SUPPORTED_BARE_FLAGS.contains(&value)
            || SUPPORTED_O_FLAGS.contains(&value)
            || SUPPORTED_G_FLAGS.contains(&value)
            || value.starts_with("-std=")
        {
            self.parsed.push(Argument::Plain(value.into()));
            return Ok(());
        }
        if let Some(rest) = value.strip_prefix("-o") {
            let path = self.take_value("-o", Some(rest))?;
            // A repeated `-o` follows the driver: the last one names the file
            // that is produced. Every occurrence still enters the key.
            self.output = Some(PathBuf::from(&path));
            self.parsed.push(Argument::Path {
                flag: "-o".into(),
                path: PathBuf::from(path),
            });
            return Ok(());
        }
        if let Some(rest) = value.strip_prefix("-I") {
            let path = PathBuf::from(self.take_value("-I", Some(rest))?);
            self.include_dirs.push(path.clone());
            self.parsed.push(Argument::Path {
                flag: "-I".into(),
                path,
            });
            return Ok(());
        }
        if SEPARATE_PATH_FLAGS.contains(&value) {
            let path = PathBuf::from(self.take_value(value, None)?);
            match value {
                "-isystem" | "-iquote" | "-idirafter" => self.include_dirs.push(path.clone()),
                "-isysroot" => self.sysroot = Some(path.clone()),
                // `-include` and `-imacros` are deliberately not required
                // inputs. The driver resolves the name through the include
                // chain, so the file need not exist relative to the working
                // directory, and the dependency list names it at whatever path
                // it was actually found at.
                _ => {}
            }
            self.parsed.push(Argument::Path {
                flag: value.into(),
                path,
            });
            return Ok(());
        }
        // `--include=<file>` is the long spelling of `-include <file>`; the
        // `cc` crate emits it for prefixed headers.
        if let Some(rest) = value.strip_prefix("--include=") {
            let path = PathBuf::from(rest);
            self.parsed.push(Argument::Path {
                flag: "-include".into(),
                path,
            });
            return Ok(());
        }
        if let Some((flag, rest)) = PREFIX_MAP_FLAGS.iter().find_map(|flag| {
            value
                .strip_prefix(&format!("{flag}="))
                .map(|rest| (*flag, rest))
        }) {
            let (from, to) = rest.split_once('=').unwrap_or((rest, ""));
            self.parsed.push(Argument::PrefixMap {
                flag: flag.into(),
                from: PathBuf::from(from),
                to: to.into(),
            });
            return Ok(());
        }
        // `--param name=value` tunes the optimizer; its text fully describes it.
        if value == "--param" {
            let parameter = self.take_value("--param", None)?;
            self.parsed
                .push(Argument::Plain(format!("--param={parameter}")));
            return Ok(());
        }
        if let Some(parameter) = value.strip_prefix("--param=") {
            self.parsed
                .push(Argument::Plain(format!("--param={parameter}")));
            return Ok(());
        }
        if let Some(rest) = value.strip_prefix("--sysroot=") {
            let path = PathBuf::from(rest);
            self.sysroot = Some(path.clone());
            self.parsed.push(Argument::Path {
                flag: "--sysroot".into(),
                path,
            });
            return Ok(());
        }
        if let Some(rest) = value
            .strip_prefix("-D")
            .or_else(|| value.strip_prefix("-U"))
        {
            let flag = &value[..2];
            let definition = self.take_value(flag, Some(rest))?;
            self.parsed
                .push(Argument::Plain(format!("{flag}{definition}")));
            return Ok(());
        }
        if let Some(rest) = value.strip_prefix("-x") {
            let language = self.take_value("-x", Some(rest))?;
            self.explicit_language = Some(match language.as_str() {
                "c" => CcLanguage::C,
                "c++" => CcLanguage::Cxx,
                other => return Err(CcBypassReason::UnsupportedLanguage(other.into())),
            });
            self.parsed.push(Argument::Plain(format!("-x{language}")));
            return Ok(());
        }
        if let Some(target) = value.strip_prefix("--target=") {
            self.parsed
                .push(Argument::Plain(format!("--target={target}")));
            return Ok(());
        }
        if value == "-target" {
            let target = self.take_value("-target", None)?;
            self.parsed
                .push(Argument::Plain(format!("--target={target}")));
            return Ok(());
        }
        if value == "-arch" {
            let arch = self.take_value("-arch", None)?;
            self.parsed.push(Argument::Plain(format!("-arch={arch}")));
            return Ok(());
        }
        if let Some(option) = value.strip_prefix("-f") {
            return self.parse_f_flag(value, option);
        }
        if let Some(option) = value.strip_prefix("-m") {
            return self.parse_m_flag(value, option);
        }
        if value.starts_with("-g") {
            // `-gsplit-dwarf` writes a `.dwo` beside the object; every other
            // unlisted `-g` spelling is simply unmodeled.
            return Err(if value.starts_with("-gsplit-dwarf") {
                CcBypassReason::SplitDebugOutput(value.into())
            } else {
                CcBypassReason::UnknownFlag(value.into())
            });
        }
        if value.starts_with("-W") {
            // Warning selection changes only diagnostics, which are replayed
            // from the cache, and the exit status, and only successful
            // compiles are ever published.
            self.parsed.push(Argument::Plain(value.into()));
            return Ok(());
        }
        Err(CcBypassReason::UnknownFlag(value.into()))
    }

    fn parse_f_flag(&mut self, value: &str, option: &str) -> Result<(), CcBypassReason> {
        if option.starts_with("plugin") || option.starts_with("pass-plugin") {
            return Err(CcBypassReason::Plugin(value.into()));
        }
        if option.starts_with("profile-") || option == "test-coverage" {
            return Err(CcBypassReason::CoverageInstrumentation(value.into()));
        }
        let name = option.split_once('=').map_or(option, |(name, _)| name);
        if SUPPORTED_F_FLAGS.binary_search(&name).is_err() {
            return Err(CcBypassReason::UnknownFlag(value.into()));
        }
        self.parsed.push(Argument::Plain(value.into()));
        Ok(())
    }

    fn parse_m_flag(&mut self, value: &str, option: &str) -> Result<(), CcBypassReason> {
        if option == "llvm" {
            return Err(CcBypassReason::ToolPassthrough(value.into()));
        }
        // `-march=native` and its relatives resolve against whatever CPU this
        // machine has. The resulting object is not a function of the key, so
        // another machine could otherwise restore code its processor cannot
        // run.
        if let Some((name, selection)) = option.split_once('=')
            && matches!(name, "arch" | "cpu" | "tune")
            && matches!(selection, "native" | "host")
        {
            return Err(CcBypassReason::LocalCpuTarget(value.into()));
        }
        let name = option.split_once('=').map_or(option, |(name, _)| name);
        if SUPPORTED_M_FLAGS.binary_search(&name).is_err() {
            return Err(CcBypassReason::UnknownFlag(value.into()));
        }
        self.parsed.push(Argument::Plain(value.into()));
        Ok(())
    }
}

#[cfg(test)]
#[path = "cc_cache_tests.rs"]
mod tests;
