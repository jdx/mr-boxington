//! Remote-cache write policy.
//!
//! Cache writes are only trusted from a CI context that a pull request cannot
//! influence, so untrusted contexts read the remote cache but never publish to
//! it. Release builds do not participate at all.

use mbx_cache_core::RemoteCacheMode;

/// Resolve the remote-cache mode that is actually permitted here.
///
/// Returns `None` when the configured mode leaves nothing to do.
pub fn effective_remote_cache_mode(configured: RemoteCacheMode) -> Option<RemoteCacheMode> {
    effective_remote_cache_mode_with(configured, |name| std::env::var(name).ok())
}

fn effective_remote_cache_mode_with(
    configured: RemoteCacheMode,
    get_env: impl Fn(&str) -> Option<String>,
) -> Option<RemoteCacheMode> {
    if trusted_cache_writer(&get_env) {
        return Some(configured);
    }
    match configured {
        RemoteCacheMode::ReadWrite | RemoteCacheMode::ReadOnly => Some(RemoteCacheMode::ReadOnly),
        RemoteCacheMode::WriteOnly => None,
    }
}

/// Whether this is a release build, which never uses the cache.
pub fn release_context() -> bool {
    release_context_with(|name| std::env::var(name).ok())
}

fn trusted_cache_writer(get_env: &impl Fn(&str) -> Option<String>) -> bool {
    if env_truthy(get_env("GITHUB_ACTIONS")) {
        return get_env("GITHUB_EVENT_NAME").as_deref() == Some("push")
            && get_env("GITHUB_REF_TYPE").as_deref() == Some("branch")
            && env_truthy(get_env("GITHUB_REF_PROTECTED"));
    }
    if env_truthy(get_env("GITLAB_CI")) {
        return get_env("CI_PIPELINE_SOURCE").as_deref() == Some("push")
            && get_env("CI_COMMIT_TAG").is_none()
            && get_env("CI_MERGE_REQUEST_IID").is_none()
            && env_truthy(get_env("CI_COMMIT_REF_PROTECTED"));
    }
    false
}

fn release_context_with(get_env: impl Fn(&str) -> Option<String>) -> bool {
    (env_truthy(get_env("GITHUB_ACTIONS"))
        && (get_env("GITHUB_REF_TYPE").as_deref() == Some("tag")
            || get_env("GITHUB_EVENT_NAME").as_deref() == Some("release")))
        || (env_truthy(get_env("GITLAB_CI")) && get_env("CI_COMMIT_TAG").is_some())
}

fn env_truthy(value: Option<String>) -> bool {
    value.is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(values: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let values: HashMap<String, String> = values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |name: &str| values.get(name).cloned()
    }

    #[test]
    fn untrusted_contexts_only_read() {
        let local = env(&[]);
        assert_eq!(
            effective_remote_cache_mode_with(RemoteCacheMode::ReadWrite, &local),
            Some(RemoteCacheMode::ReadOnly)
        );
        assert_eq!(
            effective_remote_cache_mode_with(RemoteCacheMode::WriteOnly, &local),
            None
        );
    }

    #[test]
    fn protected_branch_pushes_may_write() {
        let ci = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_REF_TYPE", "branch"),
            ("GITHUB_REF_PROTECTED", "true"),
        ]);
        assert_eq!(
            effective_remote_cache_mode_with(RemoteCacheMode::ReadWrite, &ci),
            Some(RemoteCacheMode::ReadWrite)
        );
    }

    #[test]
    fn pull_requests_may_not_write() {
        let pull_request = env(&[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_EVENT_NAME", "pull_request"),
            ("GITHUB_REF_TYPE", "branch"),
            ("GITHUB_REF_PROTECTED", "true"),
        ]);
        assert_eq!(
            effective_remote_cache_mode_with(RemoteCacheMode::ReadWrite, &pull_request),
            Some(RemoteCacheMode::ReadOnly)
        );
    }

    #[test]
    fn merge_requests_may_not_write() {
        let merge_request = env(&[
            ("GITLAB_CI", "true"),
            ("CI_PIPELINE_SOURCE", "push"),
            ("CI_MERGE_REQUEST_IID", "7"),
            ("CI_COMMIT_REF_PROTECTED", "true"),
        ]);
        assert_eq!(
            effective_remote_cache_mode_with(RemoteCacheMode::ReadWrite, &merge_request),
            Some(RemoteCacheMode::ReadOnly)
        );
    }

    #[test]
    fn tags_and_releases_are_release_contexts() {
        assert!(release_context_with(env(&[
            ("GITHUB_ACTIONS", "1"),
            ("GITHUB_REF_TYPE", "tag"),
        ])));
        assert!(release_context_with(env(&[
            ("GITLAB_CI", "1"),
            ("CI_COMMIT_TAG", "v1.0.0"),
        ])));
        assert!(!release_context_with(env(&[
            ("GITHUB_ACTIONS", "1"),
            ("GITHUB_REF_TYPE", "branch"),
        ])));
    }
}
