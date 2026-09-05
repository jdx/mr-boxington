use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Mapping from a host-specific absolute root to a stable key placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMapping {
    /// Absolute host path to replace.
    pub root: PathBuf,
    /// Placeholder name without the surrounding `${...}` syntax.
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

    /// Order mappings deepest root first, which is what normalization needs.
    pub fn ordered(mappings: &[PathMapping]) -> Vec<PathMapping> {
        let mut ordered = mappings.to_vec();
        ordered.sort_by_key(|mapping| std::cmp::Reverse(mapping.root.components().count()));
        ordered
    }
}

/// Why a path could not be represented with the configured mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathNormalizationError {
    /// An absolute path has no matching stable placeholder.
    UnmappedAbsolutePath(PathBuf),
    /// A path component cannot be represented in the UTF-8 cache key.
    NonUtf8Path(PathBuf),
}

impl fmt::Display for PathNormalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnmappedAbsolutePath(path) => {
                write!(
                    formatter,
                    "absolute path has no stable cache mapping: {}",
                    path.display()
                )
            }
            Self::NonUtf8Path(path) => {
                write!(
                    formatter,
                    "cache key paths must be valid UTF-8: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for PathNormalizationError {}

/// Map an absolute path to its cache-key placeholder form.
///
/// `mappings` must already be ordered by [`PathMapping::ordered`]. Existing
/// path aliases are resolved while a not-yet-created suffix is preserved.
pub fn normalize_mapped_path(
    path: &Path,
    working_dir: &Path,
    mappings: &[PathMapping],
) -> Result<String, PathNormalizationError> {
    let mappings = resolve_path_mappings(mappings);
    normalize_resolved_mapped_path(path, working_dir, &mappings)
}

/// Resolve filesystem aliases in mapping roots once for repeated lookups.
///
/// The returned mappings retain the caller's order and can be passed to
/// [`normalize_resolved_mapped_path`].
pub fn resolve_path_mappings(mappings: &[PathMapping]) -> Vec<PathMapping> {
    mappings
        .iter()
        .map(|mapping| PathMapping {
            root: resolve_mapping_root(&mapping.root),
            placeholder: mapping.placeholder.clone(),
        })
        .collect()
}

/// Normalize a path against mapping roots already resolved by
/// [`resolve_path_mappings`].
pub fn normalize_resolved_mapped_path(
    path: &Path,
    working_dir: &Path,
    mappings: &[PathMapping],
) -> Result<String, PathNormalizationError> {
    normalize_resolved_mapped_path_with(&PathAliases::default(), path, working_dir, mappings)
}

/// Directories whose aliases a caller has already resolved.
///
/// One compilation normalizes hundreds of input paths that share a handful of
/// parent directories, and `realpath` walks every component of every one of
/// them. Resolving a parent once and then only the last component of each
/// path costs one `lstat` per path instead of one per component. The memo
/// belongs to the caller and lives as long as the caller wants it to: one
/// action's worth of paths, in practice, which is no longer than a link is
/// assumed to hold between two `realpath` calls today.
#[derive(Debug, Default)]
pub struct PathAliases {
    #[cfg(unix)]
    directories: std::cell::RefCell<std::collections::HashMap<PathBuf, PathBuf>>,
}

/// [`normalize_resolved_mapped_path`], resolving directory aliases through
/// `aliases` rather than from scratch.
pub fn normalize_resolved_mapped_path_with(
    aliases: &PathAliases,
    path: &Path,
    working_dir: &Path,
    mappings: &[PathMapping],
) -> Result<String, PathNormalizationError> {
    let absolute = if path.is_absolute() {
        normalize_components(path)
    } else {
        normalize_components(&working_dir.join(path))
    };
    let resolved = if absolute.is_absolute() {
        resolve_path_aliases_with(aliases, &absolute)
    } else {
        absolute.clone()
    };
    for mapping in mappings {
        if let Ok(relative) = resolved.strip_prefix(&mapping.root) {
            let suffix = slash_path(relative)?;
            return Ok(if suffix.is_empty() {
                format!("${{{}}}", mapping.placeholder)
            } else {
                format!("${{{}}}/{suffix}", mapping.placeholder)
            });
        }
    }
    Err(PathNormalizationError::UnmappedAbsolutePath(absolute))
}

#[cfg(any(unix, windows))]
fn resolve_path_aliases(path: &Path) -> PathBuf {
    resolve_path_aliases_with(&PathAliases::default(), path)
}

#[cfg(any(unix, windows))]
fn resolve_path_aliases_with(aliases: &PathAliases, path: &Path) -> PathBuf {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        match aliases.canonicalize(existing) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return normalize_components(&resolved);
            }
            Err(_) => {
                let Some(name) = existing.file_name() else {
                    return path.to_path_buf();
                };
                missing.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return path.to_path_buf();
                };
                existing = parent;
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn resolve_path_aliases(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(not(any(unix, windows)))]
fn resolve_path_aliases_with(_aliases: &PathAliases, path: &Path) -> PathBuf {
    path.to_path_buf()
}

impl PathAliases {
    /// `std::fs::canonicalize`, through the directories already resolved.
    #[cfg(unix)]
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return std::fs::canonicalize(path);
        };
        if parent.as_os_str().is_empty() {
            return std::fs::canonicalize(path);
        }
        // Looked up, then released: resolving the parent below borrows the
        // memo again, and a `Ref` that lived through the match would panic.
        let known = self.directories.borrow().get(parent).cloned();
        let resolved_parent = match known {
            Some(resolved) => resolved,
            None => {
                let resolved = self.canonicalize(parent)?;
                self.directories
                    .borrow_mut()
                    .insert(parent.to_path_buf(), resolved.clone());
                resolved
            }
        };
        let candidate = resolved_parent.join(name);
        if std::fs::symlink_metadata(&candidate)?
            .file_type()
            .is_symlink()
        {
            std::fs::canonicalize(&candidate)
        } else {
            Ok(candidate)
        }
    }

    /// Windows keeps the plain call: its canonical paths are verbatim, and a
    /// junction is not what `is_symlink` reports.
    #[cfg(windows)]
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
}

fn resolve_mapping_root(root: &Path) -> PathBuf {
    let root = normalize_components(root);
    if root.is_absolute() {
        resolve_path_aliases(&root)
    } else {
        root
    }
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

fn slash_path(path: &Path) -> Result<String, PathNormalizationError> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(
                value
                    .to_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| PathNormalizationError::NonUtf8Path(path.to_path_buf())),
            ),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn maps_verbatim_windows_paths_to_non_verbatim_roots() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("clippy.toml");
        std::fs::write(&input, "disallowed-methods = []").unwrap();
        let verbatim = std::fs::canonicalize(&input).unwrap();
        assert!(verbatim.to_string_lossy().starts_with(r"\\?\"));

        let normalized = normalize_mapped_path(
            &verbatim,
            directory.path(),
            &[PathMapping::new(directory.path(), "workspace")],
        )
        .unwrap();

        assert_eq!(normalized, "${workspace}/clippy.toml");
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;

    #[test]
    fn resolves_aliases_through_remembered_parents() {
        let directory = tempfile::tempdir().unwrap();
        let aliases = PathAliases::default();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let physical = root.join("physical");
        std::fs::create_dir_all(physical.join("src")).unwrap();
        std::fs::write(physical.join("src").join("lib.rs"), "").unwrap();
        std::fs::write(physical.join("real.rs"), "").unwrap();
        std::os::unix::fs::symlink(&physical, root.join("alias")).unwrap();
        std::os::unix::fs::symlink(physical.join("real.rs"), physical.join("link.rs")).unwrap();
        let mappings = [PathMapping::new(&physical, "workspace")];

        // A directory alias, then a sibling that reuses its resolved parent.
        let through_alias = root.join("alias").join("src").join("lib.rs");
        assert_eq!(
            normalize_resolved_mapped_path_with(&aliases, &through_alias, &root, &mappings)
                .unwrap(),
            "${workspace}/src/lib.rs"
        );
        let sibling = root.join("alias").join("src").join("missing.rs");
        assert_eq!(
            normalize_resolved_mapped_path_with(&aliases, &sibling, &root, &mappings).unwrap(),
            "${workspace}/src/missing.rs"
        );
        // A file that is itself a link resolves to what it names.
        assert_eq!(
            normalize_resolved_mapped_path_with(
                &aliases,
                &physical.join("link.rs"),
                &root,
                &mappings
            )
            .unwrap(),
            "${workspace}/real.rs"
        );
        // And a suffix that does not exist yet is kept as written.
        let unborn = root
            .join("alias")
            .join("target")
            .join("deps")
            .join("out.rlib");
        assert_eq!(
            normalize_resolved_mapped_path_with(&aliases, &unborn, &root, &mappings).unwrap(),
            "${workspace}/target/deps/out.rlib"
        );
    }

    #[test]
    fn a_retargeted_alias_is_seen_by_the_next_caller() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        for name in ["one", "two"] {
            std::fs::create_dir_all(root.join(name).join("src")).unwrap();
            std::fs::write(root.join(name).join("src").join("lib.rs"), "").unwrap();
        }
        let alias = root.join("alias");
        std::os::unix::fs::symlink(root.join("one"), &alias).unwrap();
        let mappings = [
            PathMapping::new(root.join("one"), "one"),
            PathMapping::new(root.join("two"), "two"),
        ];
        let input = alias.join("src").join("lib.rs");

        let first = PathAliases::default();
        assert_eq!(
            normalize_resolved_mapped_path_with(&first, &input, &root, &mappings).unwrap(),
            "${one}/src/lib.rs"
        );
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(root.join("two"), &alias).unwrap();
        // The memo that resolved it is the caller's, and a new caller starts
        // from the filesystem.
        assert_eq!(
            normalize_resolved_mapped_path_with(&PathAliases::default(), &input, &root, &mappings)
                .unwrap(),
            "${two}/src/lib.rs"
        );
    }
}
