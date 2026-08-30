use super::TASK_ACTION_MANIFEST_VERSION;
use crate::{CacheDigest, TaskActionManifest};
use eyre::{Context, Result, bail};
use log::warn;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Whether `task` is a well-formed task action identity.
///
/// Identities name files and directories in the store, so anything that reads
/// the store back has to be able to tell an identity from whatever else a user
/// left lying there.
pub fn is_task_identity(task: &str) -> bool {
    task.len() == 64
        && task
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn validate_task_identity(task: &str) -> Result<()> {
    if !is_task_identity(task) {
        bail!("invalid task action identity");
    }
    Ok(())
}

/// Where a store keeps its task prediction manifests.
pub(super) fn task_manifest_dir(store: &Path) -> PathBuf {
    store.join("task-manifests").join("v1")
}

/// The action digests a task's prediction manifest recorded.
///
/// Read straight off disk rather than through an agent, because a collector
/// needs the action set of tasks no session is running. A manifest that is
/// missing or no longer parseable yields no actions rather than an error: this
/// is a prediction index, so the worst a thin answer costs is a cold prefetch,
/// or an object collected earlier than it deserved.
pub fn task_manifest_actions(store: &Path, task: &str) -> Result<Vec<CacheDigest>> {
    validate_task_identity(task)?;
    let path = task_manifest_dir(store).join(format!("{task}.json"));
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
        }
    };
    let Ok(manifest) = serde_json::from_slice::<TaskActionManifest>(&bytes) else {
        return Ok(Vec::new());
    };
    if validate_task_manifest(&manifest, task).is_err() {
        return Ok(Vec::new());
    }
    Ok(manifest
        .predictions
        .into_iter()
        .map(|prediction| prediction.action)
        .collect())
}

pub(super) fn validate_task_manifest(manifest: &TaskActionManifest, task: &str) -> Result<()> {
    if manifest.task == task && manifest.validate() {
        Ok(())
    } else {
        bail!("invalid task action manifest")
    }
}

pub(super) fn merge_task_manifests(
    task: &str,
    base: Option<TaskActionManifest>,
    update: TaskActionManifest,
) -> Result<TaskActionManifest> {
    validate_task_manifest(&update, task)?;
    let mut predictions = BTreeMap::new();
    if let Some(base) = base {
        validate_task_manifest(&base, task)?;
        predictions.extend(
            base.predictions
                .into_iter()
                .map(|prediction| (prediction.invocation.clone(), prediction)),
        );
    }
    predictions.extend(
        update
            .predictions
            .into_iter()
            .map(|prediction| (prediction.invocation.clone(), prediction)),
    );
    let manifest = TaskActionManifest {
        version: TASK_ACTION_MANIFEST_VERSION,
        task: task.to_owned(),
        predictions: predictions.into_values().collect(),
    };
    validate_task_manifest(&manifest, task)?;
    Ok(manifest)
}

pub(super) fn merge_remote_task_manifest(
    task: &str,
    remote: TaskActionManifest,
    local: TaskActionManifest,
) -> (TaskActionManifest, bool) {
    match merge_task_manifests(task, Some(remote), local.clone()) {
        Ok(manifest) => (manifest, true),
        Err(error) => {
            warn!("remote task action manifest merge failed for {task}: {error}");
            (local, false)
        }
    }
}
