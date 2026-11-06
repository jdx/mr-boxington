//! Resolve memory limits at the process's cgroup and every visible ancestor.
//!
//! Parent usage includes sibling workloads. Subtracting only the leaf's usage
//! from a parent's limit would overestimate how much memory a compiler can use.
use crate::util::{Stamp, parse_cgroup_stat, read_cgroup_text};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Memory {
    pub limit: Option<u64>,
    pub headroom: Option<u64>,
    /// The headroom of every limited cgroup, `headroom` being the smallest.
    pub levels: Vec<crate::util::Limit>,
}

#[derive(Debug)]
struct Domain {
    mount: PathBuf,
    current: PathBuf,
    v2: bool,
}

/// PSI files for every visible v2 ancestor, including unlimited ancestors.
pub(crate) fn pressure_files() -> Vec<PathBuf> {
    visible_domains()
        .into_iter()
        .filter(|domain| domain.v2)
        .flat_map(|domain| {
            domain
                .current
                .ancestors()
                .take_while(|path| path.starts_with(&domain.mount))
                .map(|path| path.join("memory.pressure"))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Every visible memory cgroup directory, v1 or v2, from this process's group
/// up to its mount: the directories whose files the memory probes read.
pub(crate) fn memory_directories() -> Vec<PathBuf> {
    visible_domains()
        .into_iter()
        .flat_map(|domain| {
            domain
                .current
                .ancestors()
                .take_while(|path| path.starts_with(&domain.mount))
                .map(Path::to_path_buf)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The kernel-wide identity of every directory in `memory_directories`.
///
/// A cgroup's device and inode name it whichever namespace or mount shows it,
/// so two containers sharing a pool can tell whether one sits inside a cgroup
/// the other reads. Computed once: a process does not change cgroup while it
/// compiles.
pub(crate) fn identities() -> &'static [String] {
    static IDENTITIES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    IDENTITIES.get_or_init(|| directories().iter().map(|(_, id)| id.clone()).collect())
}

/// `memory_directories` paired with their identities, for those that have one.
fn directories() -> &'static [(PathBuf, String)] {
    static DIRECTORIES: std::sync::OnceLock<Vec<(PathBuf, String)>> = std::sync::OnceLock::new();
    DIRECTORIES.get_or_init(|| {
        memory_directories()
            .into_iter()
            .filter_map(|path| identity(&path).map(|id| (path, id)))
            .collect()
    })
}

pub(crate) fn identity(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

/// Pool subdirectory holding the latest walk of each short cgroup, shared by
/// every process using the pool.
const SUBTREES_DIR: &str = "subtrees";
/// How long one walk of a cgroup's subtree is reused for leases older than it.
const SUBTREE_REFRESH_MS: u64 = 1_000;

/// One walk of a cgroup's subtree, timed so that processes can share it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Walk {
    began: Stamp,
    /// The kernel's boot ID, without which a boot clock means nothing.
    boot: String,
    /// `None` when part of the subtree could not be read.
    members: Option<BTreeSet<String>>,
}

/// This boot's ID, or empty where the kernel does not say.
fn boot_id() -> &'static str {
    static BOOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BOOT.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|id| id.trim().to_owned())
            .unwrap_or_default()
    })
}

/// Whether a process sitting in `cgroups` is charged to the cgroup `scope`,
/// one of this process's own memory cgroups. `written` is when the process
/// recorded its cgroups, if known. Called under the registrar lock of
/// `pool`, which also guards the walks shared there.
///
/// Finding `scope` in the list is not enough. A process sees only the cgroups
/// its namespace shows, so a compilation in a container nested below `scope`
/// lists none of the ancestors above its container. So this looks for any of
/// its cgroups in `scope`'s subtree instead, which this process can see
/// whenever it can see `scope`.
///
/// Only a complete walk that began after the cgroups were recorded, by both
/// the wall clock and the boot clock, can show they are outside `scope`: the
/// cgroups existed by then, so the walk saw them. Anything less -- a scope
/// this process cannot find, a subtree it cannot fully read, a record of
/// unknown age, a walk the clocks cannot order after it -- counts as inside.
///
/// Every waiter asks this on each poll, so one walk is shared through the
/// pool rather than repeated in each process: a cgroup is walked at most
/// once a second, and again only for a lease newer than the last walk.
pub(crate) fn contains(
    pool: &Path,
    scope: &str,
    cgroups: &[String],
    written: Option<Stamp>,
) -> bool {
    if cgroups.iter().any(|id| id == scope) {
        return true;
    }
    let Some(((path, _), written)) = directories()
        .iter()
        .find(|(_, id)| id == scope)
        .zip(written)
    else {
        return true;
    };
    let found = |walk: &Walk| {
        walk.members
            .as_ref()
            .is_none_or(|members| cgroups.iter().any(|id| members.contains(id)))
    };
    let directory = pool.join(SUBTREES_DIR);
    let file = directory.join(format!("{scope}.json"));
    let now = Stamp::now();
    let cached = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Walk>(&bytes).ok());
    if let Some(walk) = &cached {
        let fresh = walk.boot == boot_id()
            && walk
                .began
                .until(&now)
                .is_some_and(|age| age < SUBTREE_REFRESH_MS);
        if fresh && (found(walk) || written.before(&walk.began)) {
            return found(walk);
        }
    } else {
        crate::pressure::prune(&directory, SystemTime::now());
    }
    let walk = Walk {
        began: now,
        boot: boot_id().to_owned(),
        members: subtree(path),
    };
    if let Ok(bytes) = serde_json::to_vec(&walk) {
        let _ = crate::util::write_advisory(&file, &bytes);
    }
    // A record the clocks cannot place before the walk -- from the same
    // millisecond, or after a clock change -- is not ruled out by it.
    found(&walk) || !written.before(&walk.began)
}

/// The identities of every cgroup below `path`, or `None` when some part of
/// it could not be read.
fn subtree(path: &Path) -> Option<BTreeSet<String>> {
    let mut members = BTreeSet::new();
    let mut pending = vec![path.to_path_buf()];
    // A cgroup removed during the walk takes its descendants with it, so it
    // hides nothing; any other failure might.
    let gone = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if gone(&error) => continue,
            Err(_) => return None,
        };
        for entry in entries {
            let entry = entry.ok()?;
            // Every cgroup holds dozens of control files. The entry's own
            // type, read with the directory, skips them without a `stat`
            // each, so the walk costs one call per cgroup.
            if !entry.file_type().ok()?.is_dir() {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if gone(&error) => continue,
                Err(_) => return None,
            };
            members.insert(format!("{}:{}", metadata.dev(), metadata.ino()));
            pending.push(entry.path());
        }
    }
    Some(members)
}

pub(crate) fn memory() -> Memory {
    snapshot(&visible_domains())
}

fn visible_domains() -> Vec<Domain> {
    let domains = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .zip(std::fs::read_to_string("/proc/self/mountinfo").ok())
        .map(|(groups, mounts)| domains(&groups, &mounts))
        .unwrap_or_default();
    if domains.is_empty() {
        // Preserve detection in containers that hide proc membership or mounts.
        return vec![
            Domain {
                mount: "/sys/fs/cgroup".into(),
                current: "/sys/fs/cgroup".into(),
                v2: true,
            },
            Domain {
                mount: "/sys/fs/cgroup/memory".into(),
                current: "/sys/fs/cgroup/memory".into(),
                v2: false,
            },
        ];
    }
    domains
}

fn domains(groups: &str, mounts: &str) -> Vec<Domain> {
    let memberships: Vec<_> = groups
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, ':');
            fields.next()?;
            let controllers = fields.next()?;
            let path = Path::new(fields.next()?);
            let v2 = controllers.is_empty();
            (v2 || controllers.split(',').any(|name| name == "memory")).then_some((v2, path))
        })
        .collect();
    mounts
        .lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let fields: Vec<_> = left.split_whitespace().collect();
            let mut options = right.split_whitespace();
            let fs = options.next()?;
            options.next()?;
            let super_options = options.next()?;
            let v2 = match fs {
                "cgroup2" => true,
                "cgroup" if super_options.split(',').any(|name| name == "memory") => false,
                _ => return None,
            };
            let root = mount_path(fields.get(3)?)?;
            let mount = mount_path(fields.get(4)?)?;
            let (_, member) = memberships
                .iter()
                .find(|(version, path)| *version == v2 && path.starts_with(&root))?;
            // Membership is relative to the cgroup namespace. Mount roots use
            // that same namespace; component checks reject prefix lookalikes and
            // proc paths that would escape the visible mount through '..'.
            if !safe_absolute(member) || !safe_absolute(&root) || !safe_absolute(&mount) {
                return None;
            }
            let current = mount.join(member.strip_prefix(&root).ok()?);
            Some(Domain { mount, current, v2 })
        })
        .collect()
}

fn safe_absolute(path: &Path) -> bool {
    path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir))
}

/// mountinfo escapes spaces, tabs, newlines and backslashes with octal bytes.
fn mount_path(value: &str) -> Option<PathBuf> {
    let mut bytes = Vec::new();
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte == b'\\' {
            let mut decoded = 0u16;
            for _ in 0..3 {
                let digit = input.next()?;
                if !(b'0'..=b'7').contains(&digit) {
                    return None;
                }
                decoded = decoded * 8 + u16::from(digit - b'0');
            }
            bytes.push(u8::try_from(decoded).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    Some(OsString::from_vec(bytes).into())
}

fn snapshot(domains: &[Domain]) -> Memory {
    let mut result = Memory::default();
    for domain in domains {
        for directory in domain.current.ancestors() {
            if !directory.starts_with(&domain.mount) {
                break;
            }
            let (limit_name, usage_name, reclaim_fields) = if domain.v2 {
                (
                    "memory.max",
                    "memory.current",
                    ["active_file", "inactive_file"],
                )
            } else {
                (
                    "memory.limit_in_bytes",
                    "memory.usage_in_bytes",
                    ["total_active_file", "total_inactive_file"],
                )
            };
            let Some(limit) = std::fs::read_to_string(directory.join(limit_name))
                .ok()
                .and_then(|s| {
                    if s.trim() == "0" {
                        Some(0)
                    } else {
                        read_cgroup_text(&s)
                    }
                })
            else {
                continue;
            };
            let usage = std::fs::read_to_string(directory.join(usage_name))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok());
            // Usage counts page cache, which the kernel reclaims when a process
            // needs the memory. Both file lists count, not just the inactive
            // one: a cache read over and over sits on the active list, and
            // treating it as used would hold back compilations in a container
            // that is only full of cache. Whether reclaiming it hurts shows up
            // in the stall figures, which the pressure gate reads as well.
            // Shared memory and tmpfs sit on the anonymous lists, so neither
            // field includes memory that cannot be reclaimed without swap.
            let stat = std::fs::read_to_string(directory.join("memory.stat")).ok();
            let reclaimable = reclaim_fields
                .iter()
                .filter_map(|field| parse_cgroup_stat(stat.as_deref()?, field))
                .fold(0, u64::saturating_add);
            let headroom = usage.map_or(limit, |used| {
                limit.saturating_sub(used.saturating_sub(reclaimable))
            });
            result.limit = Some(result.limit.map_or(limit, |old| old.min(limit)));
            result.headroom = Some(result.headroom.map_or(headroom, |old| old.min(headroom)));
            result.levels.push(crate::util::Limit {
                cgroup: identity(directory),
                bytes: headroom,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(path: &Path, bytes: u64) -> crate::util::Limit {
        crate::util::Limit {
            cgroup: identity(path),
            bytes,
        }
    }

    fn write_memory(path: &Path, limit: &str, usage: u64, inactive: u64, v2: bool) {
        write_cache(path, limit, usage, 0, inactive, v2);
    }

    fn write_cache(path: &Path, limit: &str, usage: u64, active: u64, inactive: u64, v2: bool) {
        std::fs::create_dir_all(path).unwrap();
        let (limit_name, usage_name, prefix) = if v2 {
            ("memory.max", "memory.current", "")
        } else {
            ("memory.limit_in_bytes", "memory.usage_in_bytes", "total_")
        };
        std::fs::write(path.join(limit_name), limit).unwrap();
        std::fs::write(path.join(usage_name), usage.to_string()).unwrap();
        // The shape of a real `memory.stat`, including the `file` total that
        // also counts shared memory and must not be read as reclaimable.
        let stat = format!(
            "{prefix}anon 9\n{prefix}file {}\n{prefix}active_file {active}\n{prefix}inactive_file {inactive}\n{prefix}shmem 7\n",
            active + inactive + 7
        );
        std::fs::write(path.join("memory.stat"), stat).unwrap();
    }

    #[test]
    fn nested_membership_uses_parent_headroom_including_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path();
        write_memory(mount, "max", 0, 0, true);
        write_memory(&mount.join("slice"), "1000", 950, 100, true);
        write_memory(&mount.join("slice/build"), "800", 200, 50, true);
        let mounts = format!("1 0 0:1 / {} rw - cgroup2 cgroup rw", mount.display());
        let domains = domains("0::/slice/build\n", &mounts);
        assert_eq!(
            snapshot(&domains),
            Memory {
                limit: Some(800),
                headroom: Some(150),
                levels: vec![
                    level(&mount.join("slice/build"), 650),
                    level(&mount.join("slice"), 150),
                ],
            }
        );
        write_memory(&mount.join("slice"), "1000", 1100, 0, true);
        assert_eq!(snapshot(&domains).headroom, Some(0));
    }

    #[test]
    fn v1_memory_controller_and_subtree_mount() {
        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path().join("memory mount");
        write_memory(&mount, "1000", 950, 300, false);
        write_memory(&mount.join("build"), "9223372036854771712", 100, 0, false);
        let escaped = mount.to_string_lossy().replace(' ', "\\040");
        let mounts = format!("1 0 0:1 /slice {escaped} rw - cgroup cgroup rw,memory\n");
        let found = domains("2:cpu,cpuacct:/other\n3:memory:/slice/build\n", &mounts);
        assert_eq!(found[0].current, mount.join("build"));
        assert_eq!(
            snapshot(&found),
            Memory {
                limit: Some(1000),
                headroom: Some(350),
                levels: vec![level(&mount, 350)],
            }
        );
        assert!(domains("3:memory:/slice-other/build", &mounts).is_empty());
    }

    #[test]
    fn bind_mounts_do_not_hide_visible_ancestor_limits() {
        let temp = tempfile::tempdir().unwrap();
        let full = temp.path().join("full");
        let bind = temp.path().join("bind");
        write_memory(&full, "1000", 900, 0, true);
        write_memory(&full.join("slice"), "max", 0, 0, true);
        write_memory(&bind, "max", 0, 0, true);
        let mounts = format!(
            "1 0 0:1 /slice {} rw - cgroup2 cgroup rw\n2 0 0:1 / {} rw - cgroup2 cgroup rw",
            bind.display(),
            full.display()
        );
        assert_eq!(snapshot(&domains("0::/slice", &mounts)).headroom, Some(100));
    }

    #[test]
    fn missing_stats_count_all_usage_and_zero_limit_is_real() {
        let temp = tempfile::tempdir().unwrap();
        write_memory(temp.path(), "1000", 700, 600, true);
        let domains = [Domain {
            mount: temp.path().into(),
            current: temp.path().into(),
            v2: true,
        }];
        std::fs::remove_file(temp.path().join("memory.stat")).unwrap();
        assert_eq!(snapshot(&domains).headroom, Some(300));
        std::fs::write(temp.path().join("memory.max"), "0").unwrap();
        assert_eq!(
            snapshot(&domains),
            Memory {
                limit: Some(0),
                headroom: Some(0),
                levels: vec![level(temp.path(), 0)],
            }
        );
    }

    #[test]
    fn active_and_inactive_file_cache_both_count_as_headroom() {
        for v2 in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            // A container at its limit, nearly all of it page cache read
            // often enough to sit on the active list.
            write_cache(temp.path(), "1000", 1000, 800, 100, v2);
            let domains = [Domain {
                mount: temp.path().into(),
                current: temp.path().into(),
                v2,
            }];
            assert_eq!(snapshot(&domains).headroom, Some(900), "v2: {v2}");
        }
    }

    #[test]
    fn a_subtree_holds_nested_cgroups_whose_own_lists_stop_below_it() {
        let temp = tempfile::tempdir().unwrap();
        let scope = temp.path().join("scope");
        let nested = scope.join("container/inner");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let members = subtree(&scope).unwrap();
        // A container below the scope lists only what its namespace shows.
        assert!(members.contains(&identity(&nested).unwrap()));
        assert!(members.contains(&identity(&scope.join("container")).unwrap()));
        assert!(!members.contains(&identity(&outside).unwrap()));
        assert!(!members.contains(&identity(&scope).unwrap()));
    }

    #[test]
    fn an_unreadable_subtree_proves_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let hidden = temp.path().join("delegated");
        std::fs::create_dir_all(hidden.join("container")).unwrap();
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&hidden).is_ok();
        let members = subtree(temp.path());
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root reads through any mode, so there is nothing to show there.
        if !readable {
            assert_eq!(members, None);
        }
    }

    fn stamp(wall_ms: u64, boot_ms: u64) -> Stamp {
        Stamp { wall_ms, boot_ms }
    }

    /// This process's own cgroup, when its subtree can be read.
    fn own_scope() -> Option<&'static str> {
        let (path, scope) = directories().first()?;
        subtree(path).is_some().then_some(scope.as_str())
    }

    #[test]
    fn a_scope_this_process_cannot_find_contains_everything() {
        let pool = tempfile::tempdir().unwrap();
        let old = Some(stamp(0, 0));
        assert!(contains(pool.path(), "0:0", &["1:1".into()], old));
        assert!(contains(pool.path(), "1:1", &["1:1".into()], old));
    }

    #[test]
    fn only_a_walk_both_clocks_place_after_the_record_rules_a_cgroup_out() {
        // This process's own cgroup holds nothing that "0:0" names.
        let Some(scope) = own_scope() else {
            return;
        };
        let pool = tempfile::tempdir().unwrap();
        let outside = ["0:0".to_string()];
        let written = Stamp::now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(!contains(pool.path(), scope, &outside, Some(written)));
        assert!(
            contains(pool.path(), scope, &outside, None),
            "a record of unknown age"
        );
        // One clock alone placing the record first is not enough: the wall
        // clock may have stepped back, or the boot clock may be offset.
        let now = Stamp::now();
        let stepped = stamp(now.wall_ms.saturating_sub(60_000), now.boot_ms + 60_000);
        assert!(contains(pool.path(), scope, &outside, Some(stepped)));
        let offset = stamp(now.wall_ms + 60_000, now.boot_ms.saturating_sub(60_000));
        assert!(contains(pool.path(), scope, &outside, Some(offset)));
    }

    #[test]
    fn processes_share_one_walk_through_the_pool() {
        let Some(scope) = own_scope() else {
            return;
        };
        let pool = tempfile::tempdir().unwrap();
        let old = Some(stamp(0, 0));
        assert!(!contains(pool.path(), scope, &["0:0".into()], old));
        // Another process's fresh walk is believed as it is, both in what
        // it found and, for leases older than it, in what it did not.
        let file = pool.path().join(SUBTREES_DIR).join(format!("{scope}.json"));
        let now = Stamp::now();
        let shared = Walk {
            began: stamp(now.wall_ms - 500, now.boot_ms - 500),
            boot: boot_id().to_owned(),
            members: Some(BTreeSet::from(["0:5".to_string()])),
        };
        let write = |walk: &Walk| std::fs::write(&file, serde_json::to_vec(walk).unwrap()).unwrap();
        write(&shared);
        assert!(contains(pool.path(), scope, &["0:5".into()], old));
        assert!(!contains(pool.path(), scope, &["0:0".into()], old));
        let read = || serde_json::from_slice::<Walk>(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(read().members, shared.members, "no walk was needed");
        // A walk from another boot is not believed at all.
        write(&Walk {
            boot: "another boot".into(),
            ..read()
        });
        assert!(!contains(pool.path(), scope, &["0:5".into()], old));
        assert_ne!(read().members, shared.members, "walked again");
        // A lease newer than the walk gets a walk of its own, which other
        // waiters then reuse for it.
        write(&shared);
        let newer = Some(stamp(shared.began.wall_ms + 1, shared.began.boot_ms + 1));
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(!contains(pool.path(), scope, &["0:0".into()], newer));
        let again = read().began;
        assert_ne!(again, shared.began, "walked again");
        assert!(!contains(pool.path(), scope, &["0:0".into()], newer));
        assert_eq!(read().began, again, "not walked a third time");
        // A lease from the walk's own millisecond gets a walk of its own.
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(!contains(pool.path(), scope, &["0:1".into()], Some(again)));
        assert_ne!(read().began, again, "walked for the same-instant lease");
    }

    #[test]
    fn malformed_mounts_and_escaping_memberships_are_ignored() {
        let mounts = "broken\n1 0 0:1 / /sys/fs/cgroup rw - cgroup2 cgroup rw";
        assert!(domains("0::/../escape", mounts).is_empty());
        assert!(domains("0::relative", mounts).is_empty());
        assert_eq!(mount_path("/a\\040b\\134c"), Some("/a b\\c".into()));
        assert!(mount_path("/bad\\999").is_none());
    }
}
