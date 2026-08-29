//! Dependency-list parsing and input discovery for C and C++ compiles.

use crate::{
    CcActionContext, CcActionInput, CcBypassReason, MAX_INPUT_BYTES, MAX_MANIFEST_ENTRIES,
    MAX_PREDICTED_INPUTS, normalize_components,
};
use mbx_cache_core::{
    CacheDigest, FileDigestCache, FileDigestScope, FileIdentity, RecordedFileDigest,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Marker distinguishing an include-directory name manifest from a file input.
pub const INCLUDE_MANIFEST_PREFIX: &str = "@include-manifest:";

/// Macros whose expansion is not a function of the compilation's inputs.
const TIMESTAMP_MACROS: &[&[u8]] = &[b"__DATE__", b"__TIME__", b"__TIMESTAMP__"];

const SCAN_CHUNK_BYTES: usize = 64 * 1024;

/// A parsed GNU-style dependency list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CcDepfile {
    /// Prerequisite files named by the first rule.
    pub files: Vec<PathBuf>,
}

impl CcDepfile {
    /// Read and parse the dependency list the compiler wrote.
    pub fn read(path: &Path) -> Result<Self, CcBypassReason> {
        let contents =
            std::fs::read_to_string(path).map_err(|error| CcBypassReason::DepfileRead {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        Self::parse(&contents)
    }

    /// Parse a GNU-style dependency list.
    ///
    /// Only the first rule is read. The adapter never passes `-MP`, so a
    /// well-formed file the adapter asked for has exactly one rule, and
    /// anything further is ignored rather than guessed at.
    pub fn parse(contents: &str) -> Result<Self, CcBypassReason> {
        let joined = join_continuations(contents)?;
        let (_, prerequisites) = joined
            .lines()
            .find_map(|line| line.split_once(RULE_SEPARATOR))
            .ok_or_else(|| CcBypassReason::MalformedDepfile("no dependency rule".into()))?;
        let files = split_prerequisites(prerequisites)?;
        Ok(Self { files })
    }
}

const RULE_SEPARATOR: &str = ": ";

/// Join physical lines the compiler wrapped with a trailing backslash.
fn join_continuations(contents: &str) -> Result<String, CcBypassReason> {
    let mut joined = String::with_capacity(contents.len());
    let mut continued = false;
    for line in contents.lines() {
        let trimmed = line.strip_suffix('\r').unwrap_or(line);
        let (text, continues) = match trimmed.strip_suffix('\\') {
            Some(text) => (text, true),
            None => (trimmed, false),
        };
        if continued {
            joined.push(' ');
        }
        joined.push_str(text.trim_end_matches(['\t']));
        if !continues {
            joined.push('\n');
        }
        continued = continues;
    }
    if continued {
        return Err(CcBypassReason::MalformedDepfile(
            "unterminated line continuation".into(),
        ));
    }
    Ok(joined)
}

/// Split a prerequisite list, honoring exactly the escapes make defines.
///
/// Anything else escaped is a spelling this parser does not model, and a
/// mis-parsed prerequisite would silently drop an input from the key.
fn split_prerequisites(value: &str) -> Result<Vec<PathBuf>, CcBypassReason> {
    let mut files = Vec::new();
    let mut current = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            ' ' | '\t' => {
                if !current.is_empty() {
                    files.push(PathBuf::from(std::mem::take(&mut current)));
                }
            }
            '\\' => match characters.next() {
                Some(' ') => current.push(' '),
                Some('#') => current.push('#'),
                Some(other) => {
                    return Err(CcBypassReason::MalformedDepfile(format!(
                        "unmodeled escape \\{other}"
                    )));
                }
                None => {
                    return Err(CcBypassReason::MalformedDepfile(
                        "trailing escape character".into(),
                    ));
                }
            },
            '$' => match characters.next() {
                Some('$') => current.push('$'),
                Some(other) => {
                    return Err(CcBypassReason::MalformedDepfile(format!(
                        "unmodeled variable reference ${other}"
                    )));
                }
                None => {
                    return Err(CcBypassReason::MalformedDepfile(
                        "trailing variable reference".into(),
                    ));
                }
            },
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        files.push(PathBuf::from(current));
    }
    Ok(files)
}

/// A complete, content-addressed compiler input manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcDiscoveredInputs {
    working_dir: PathBuf,
    /// Content-addressed inputs, including include-directory manifests.
    pub inputs: Vec<CcActionInput>,
}

impl CcDiscoveredInputs {
    /// Digest every file the compilation read, and summarize the directories it
    /// searched.
    ///
    /// Digesting the files answers "did any input change". The directory
    /// manifests answer the question a dependency list cannot: whether a header
    /// that was *not* read now exists somewhere that would shadow one that was.
    pub fn collect(
        working_dir: &Path,
        files: BTreeSet<PathBuf>,
        directories: BTreeSet<PathBuf>,
        digests: &dyn FileDigestCache,
    ) -> Result<Self, CcBypassReason> {
        if !working_dir.is_absolute() {
            return Err(CcBypassReason::RelativeWorkingDirectory(
                working_dir.to_path_buf(),
            ));
        }
        let directories = minimal_manifest_directories(directories);
        if files.len() + directories.len() > MAX_PREDICTED_INPUTS {
            return Err(CcBypassReason::TooManyInputs);
        }
        let working_dir = normalize_components(working_dir);
        let mut inputs = Vec::with_capacity(files.len() + directories.len());
        let mut total_bytes = 0_u64;
        // Stat everything first so one batched ledger lookup can stand in for
        // rereading headers the session already scanned and hashed. A ledger
        // entry in the cc scope was recorded after the timestamp-macro scan
        // passed, so a hit skips the scan for the same reason it skips the
        // hash: the identity says the contents have not changed since both
        // were established.
        let mut identified = Vec::with_capacity(files.len());
        for path in files {
            let metadata = std::fs::metadata(&path).map_err(|error| CcBypassReason::InputRead {
                path: path.clone(),
                message: error.to_string(),
            })?;
            if !metadata.is_file() {
                return Err(CcBypassReason::InputRead {
                    path,
                    message: "input is not a regular file".into(),
                });
            }
            total_bytes = total_bytes.saturating_add(metadata.len());
            if total_bytes > MAX_INPUT_BYTES {
                return Err(CcBypassReason::TooManyInputs);
            }
            let identity = FileIdentity::describe(&path, &metadata);
            identified.push((path, identity));
        }
        let queries = identified
            .iter()
            .filter_map(|(_, identity)| identity.clone())
            .collect::<Vec<_>>();
        let mut recorded = digests.find(FileDigestScope::CcInput, &queries).into_iter();
        let mut fresh = Vec::new();
        for (path, identity) in identified {
            let remembered = identity
                .as_ref()
                .and_then(|_| recorded.next().flatten())
                .filter(|digest| {
                    identity
                        .as_ref()
                        .is_some_and(|identity| identity.len == digest.size)
                });
            let digest = match remembered {
                Some(digest) => digest,
                None => {
                    if contains_timestamp_macro(&path)? {
                        return Err(CcBypassReason::EmbeddedTimestampMacro(path));
                    }
                    let digest = CacheDigest::blake3_file(&path).map_err(|error| {
                        CcBypassReason::InputRead {
                            path: path.clone(),
                            message: error.to_string(),
                        }
                    })?;
                    if let Some(identity) = identity
                        && identity.len == digest.size
                    {
                        fresh.push(RecordedFileDigest {
                            file: identity,
                            digest: digest.clone(),
                        });
                    }
                    digest
                }
            };
            inputs.push(CcActionInput { path, digest });
        }
        if !fresh.is_empty() {
            digests.record(FileDigestScope::CcInput, fresh);
        }
        let mut manifest_entries = 0_usize;
        for directory in directories {
            let digest = include_manifest(&directory, &mut manifest_entries)?;
            inputs.push(CcActionInput {
                path: PathBuf::from(format!("{INCLUDE_MANIFEST_PREFIX}{}", directory.display())),
                digest,
            });
        }
        Ok(Self {
            working_dir,
            inputs,
        })
    }

    /// File inputs, excluding include-directory manifests.
    pub fn files(&self) -> impl Iterator<Item = &CcActionInput> {
        self.inputs
            .iter()
            .filter(|input| !is_manifest_input(&input.path))
    }

    /// Reject inputs whose modification time overlaps the compiler invocation.
    ///
    /// Contents are hashed after the compiler reports the paths it read. This
    /// timestamp barrier prevents a write that landed during the compile from
    /// being mistaken for the contents that produced the object; `verify`
    /// closes the remaining race after hashing.
    pub fn verify_not_modified_since(&self, started_at: SystemTime) -> Result<(), CcBypassReason> {
        for input in self.files() {
            let modified = std::fs::metadata(&input.path)
                .and_then(|metadata| metadata.modified())
                .map_err(|error| CcBypassReason::InputRead {
                    path: input.path.clone(),
                    message: error.to_string(),
                })?;
            if modified >= started_at {
                return Err(CcBypassReason::InputModifiedDuringCompilation(
                    input.path.clone(),
                ));
            }
        }
        Ok(())
    }

    /// Rehash every discovered file before publication, degrading a changed
    /// input to a miss rather than storing an object under a stale key.
    pub fn verify(&self) -> Result<(), CcBypassReason> {
        for input in self.files() {
            let matches = input.digest.matches_file(&input.path).map_err(|error| {
                CcBypassReason::InputRead {
                    path: input.path.clone(),
                    message: error.to_string(),
                }
            })?;
            if !matches {
                return Err(CcBypassReason::InputChanged(input.path.clone()));
            }
        }
        Ok(())
    }

    /// Merge the manifest into an action context after confirming both use the
    /// same compiler working directory.
    pub fn apply_to(self, context: &mut CcActionContext) -> Result<(), CcBypassReason> {
        if normalize_components(&context.working_dir) != self.working_dir {
            return Err(CcBypassReason::DiscoveryWorkingDirectory);
        }
        context.inputs.extend(self.inputs);
        Ok(())
    }
}

/// Drop include directories already covered by an ancestor's recursive manifest.
///
/// Discovered headers often contribute hundreds of nested parent directories,
/// especially for amalgamated C sources. Keeping both an ancestor and its
/// descendants walks and hashes the same subtree repeatedly, and can exhaust
/// the manifest-entry budget even though the ancestor already names every
/// includable file below it.
fn minimal_manifest_directories(directories: BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let mut directories = directories
        .into_iter()
        .map(|directory| {
            let normalized = normalize_components(&directory);
            (directory, normalized)
        })
        .collect::<Vec<_>>();
    directories.sort_by(|(left, left_normalized), (right, right_normalized)| {
        left_normalized
            .components()
            .count()
            .cmp(&right_normalized.components().count())
            .then_with(|| left_normalized.cmp(right_normalized))
            .then_with(|| left.cmp(right))
    });

    let mut minimal = Vec::<(PathBuf, PathBuf)>::new();
    for (directory, normalized) in directories {
        if !minimal
            .iter()
            .any(|(_, ancestor)| manifest_covers(ancestor, &normalized))
        {
            minimal.push((directory, normalized));
        }
    }
    minimal
        .into_iter()
        .map(|(directory, _)| directory)
        .collect()
}

/// Whether walking `ancestor` recursively is guaranteed to visit `descendant`.
///
/// Component-aware normalization rejects a lexical prefix that escapes through
/// `..`. Directory symlinks need an explicit check because `read_dir` follows
/// the directory it starts at but the recursive walk deliberately does not
/// follow symlink entries beneath it.
fn manifest_covers(ancestor: &Path, descendant: &Path) -> bool {
    let Ok(relative) = descendant.strip_prefix(ancestor) else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return false;
    }
    let mut current = ancestor.to_path_buf();
    for component in relative.components() {
        current.push(component);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            return false;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return false;
        }
    }
    true
}

fn is_manifest_input(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|path| path.starts_with(INCLUDE_MANIFEST_PREFIX))
}

/// Digest the includable names in each directory, reading no file contents.
///
/// Taken once before the compiler runs and again before publishing, this is
/// what detects a header that appeared in a search directory *while* the
/// compilation was in flight. The manifest recorded in the key is the one from
/// after the compile, and without this check that later state would be claimed
/// as the state the compiler saw.
pub fn manifest_snapshot(
    directories: &BTreeSet<PathBuf>,
) -> Result<BTreeMap<PathBuf, CacheDigest>, CcBypassReason> {
    let mut budget = 0_usize;
    minimal_manifest_directories(directories.iter().cloned().collect())
        .into_iter()
        .map(|directory| {
            include_manifest(&directory, &mut budget).map(|digest| (directory, digest))
        })
        .collect()
}

/// Extensions a file must carry to be a plausible `#include` target.
///
/// An extensionless name also qualifies: C++ standard headers are spelled that
/// way and projects ship their own.
///
/// `gch` and `pch` are here because a precompiled header answers an `#include`
/// without being named by one. GCC prefers `foo.h.gch` over `foo.h` on its own,
/// with nothing on the command line to say so, which is precisely the
/// substitution these manifests exist to notice -- and the one case the
/// adapter's explicit precompiled-header bypass cannot see.
const INCLUDABLE_EXTENSIONS: &[&str] = &[
    "c", "c++", "cc", "cpp", "cxx", "def", "gch", "h", "h++", "hh", "hpp", "hxx", "inc", "inl",
    "ipp", "pch", "tcc",
];

/// Whether a file name could be what an `#include` directive names.
///
/// The manifest exists to notice a file appearing where it would shadow a
/// header that was read. A build writes its own objects, dependency files, and
/// archives into these directories -- often the very directory a generated
/// header lives in -- and none of those can shadow an include. Counting them
/// would make the key depend on how many sibling compilations had finished,
/// which is not a property of this compilation at all.
fn is_includable(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => INCLUDABLE_EXTENSIONS
            .binary_search(&extension.to_ascii_lowercase().as_str())
            .is_ok(),
        // No extension, or a leading-dot name like `.keep`.
        _ => !name.starts_with('.'),
    }
}

/// Digest the sorted includable file names beneath a directory.
///
/// Names only: the contents of anything actually read are digested as inputs,
/// so this exists purely to notice a file appearing where it could shadow one
/// of them. A directory that does not exist has an empty manifest, which is
/// what makes "the directory was created" a key change rather than an error.
fn include_manifest(directory: &Path, budget: &mut usize) -> Result<CacheDigest, CcBypassReason> {
    let mut names = Vec::new();
    let mut pending = vec![(directory.to_path_buf(), String::new())];
    while let Some((current, prefix)) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(CcBypassReason::InputRead {
                    path: current,
                    message: error.to_string(),
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| CcBypassReason::InputRead {
                path: current.clone(),
                message: error.to_string(),
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(CcBypassReason::NonUtf8Path(entry.path()));
            };
            let relative = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            let file_type = entry
                .file_type()
                .map_err(|error| CcBypassReason::InputRead {
                    path: entry.path(),
                    message: error.to_string(),
                })?;
            if file_type.is_dir() {
                pending.push((entry.path(), relative));
                continue;
            }
            if !is_includable(name) {
                continue;
            }
            *budget += 1;
            if *budget > MAX_MANIFEST_ENTRIES {
                return Err(CcBypassReason::TooManyInputs);
            }
            names.push(relative);
        }
    }
    names.sort();
    Ok(CacheDigest::blake3(names.join("\n").as_bytes()))
}

/// Whether a file mentions a macro whose expansion is not a function of the
/// compilation's inputs.
///
/// The token is looked for rather than its expansion: a match inside a comment
/// or a string literal bypasses a compilation that would in fact have been
/// cacheable, which is the conservative direction.
fn contains_timestamp_macro(path: &Path) -> Result<bool, CcBypassReason> {
    let file = std::fs::File::open(path).map_err(|error| CcBypassReason::InputRead {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let longest = TIMESTAMP_MACROS
        .iter()
        .map(|macro_name| macro_name.len())
        .max()
        .unwrap_or_default();
    let mut reader = std::io::BufReader::new(file);
    let mut window = Vec::with_capacity(SCAN_CHUNK_BYTES + longest);
    let mut chunk = vec![0_u8; SCAN_CHUNK_BYTES];
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|error| CcBypassReason::InputRead {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        if read == 0 {
            return Ok(false);
        }
        window.extend_from_slice(&chunk[..read]);
        if TIMESTAMP_MACROS
            .iter()
            .any(|macro_name| contains_subslice(&window, macro_name))
        {
            return Ok(true);
        }
        // Keep the tail so a token split across two reads is still found.
        let keep = window.len().saturating_sub(longest.saturating_sub(1));
        window.drain(..keep);
    }
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
#[path = "depfile_tests.rs"]
mod tests;
