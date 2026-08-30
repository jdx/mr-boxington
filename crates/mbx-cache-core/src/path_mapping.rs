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
    let absolute = if path.is_absolute() {
        normalize_components(path)
    } else {
        normalize_components(&working_dir.join(path))
    };
    let resolved = if absolute.is_absolute() {
        resolve_path_aliases(&absolute)
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

#[cfg(unix)]
fn resolve_path_aliases(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
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

#[cfg(not(unix))]
fn resolve_path_aliases(path: &Path) -> PathBuf {
    path.to_path_buf()
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
