//! Resolve memory limits at the process's cgroup and every visible ancestor.
//!
//! Parent usage includes sibling workloads. Subtracting only the leaf's usage
//! from a parent's limit would overestimate how much memory a compiler can use.
use crate::util::{Stamp, parse_cgroup_stat, read_cgroup_text};
use std::collections::{BTreeMap, BTreeSet};
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
    IDENTITIES.get_or_init(|| {
        identify_directories(memory_directories())
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    })
}

/// Identities of readable probe directories. Unlike lease placement, pressure
/// state must still distinguish the cgroups whose probes can be read when
/// another directory is missing (for example, the v1 fallback on a v2 host).
pub(crate) fn pressure_identities() -> Vec<String> {
    directories().iter().map(|(_, id)| id.clone()).collect()
}

/// Every directory that can be identified, for local probes and scope lookup.
fn directories() -> &'static [(PathBuf, String)] {
    static DIRECTORIES: std::sync::OnceLock<Vec<(PathBuf, String)>> = std::sync::OnceLock::new();
    DIRECTORIES.get_or_init(|| readable_directories(memory_directories()))
}

fn readable_directories(paths: Vec<PathBuf>) -> Vec<(PathBuf, String)> {
    paths
        .into_iter()
        .filter_map(|path| identity(&path).map(|id| (path, id)))
        .collect()
}

fn identify_directories(paths: Vec<PathBuf>) -> Vec<(PathBuf, String)> {
    // A partial list could name only ancestors outside a short subtree,
    // hiding a compiler inside it. Empty identities become an unplaced lease.
    paths
        .into_iter()
        .map(|path| identity(&path).map(|id| (path, id)))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

pub(crate) fn identity(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

/// Pool subdirectory holding the latest walk of each short cgroup, shared by
/// every process using the pool.
const SUBTREES_DIR: &str = "subtrees";
/// How long one walk of a cgroup's subtree is reused for already checked IDs.
const SUBTREE_REFRESH_MS: u64 = 1_000;

/// One walk of a cgroup's subtree, timed so that processes can share it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Walk {
    began: Stamp,
    /// The kernel's boot ID, without which a boot clock means nothing.
    boot: String,
    /// `None` when part of the subtree could not be read.
    members: Option<BTreeSet<String>>,
    /// Cgroups of leases read before a walk began, with when that walk
    /// began. They existed by then, so this walk, begun no earlier, would
    /// have found any inside the subtree, whatever their leases' clocks
    /// say. Kept only for a refresh interval, so the list stays as short as
    /// the leases checked in that time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    vouched: BTreeMap<String, Stamp>,
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
/// Only a complete walk known to have begun after the cgroups existed can
/// show they are outside `scope`. A walk made here for this lease is: the
/// lease was read before it began. A shared walk can rule the lease out only
/// if every ID was already checked: clock offsets in time namespaces and a
/// wall-clock step can make a newer lease appear older by both clocks.
/// Anything less --
/// a scope this process cannot find, a subtree it cannot fully read, a
/// record of unknown age -- counts as inside.
///
/// Every waiter asks this on each poll, so one walk is shared through the
/// pool rather than repeated in each process: a cgroup is walked at most
/// once a second, and again only for IDs not yet checked by a recent walk.
pub(crate) fn contains(
    pool: &Path,
    scope: &str,
    cgroups: &[String],
    written: Option<Stamp>,
) -> bool {
    if cgroups.iter().any(|id| id == scope) {
        return true;
    }
    let Some((path, _)) = directories()
        .iter()
        .find(|(_, id)| id == scope)
        .filter(|_| written.is_some())
    else {
        return true;
    };
    contains_at(pool, scope, path, cgroups, Stamp::now())
}

fn contains_at(pool: &Path, scope: &str, path: &Path, cgroups: &[String], now: Stamp) -> bool {
    let found = |walk: &Walk| {
        walk.members
            .as_ref()
            .is_none_or(|members| cgroups.iter().any(|id| members.contains(id)))
    };
    let directory = pool.join(SUBTREES_DIR);
    let file = directory.join(format!("{scope}.json"));
    let cached = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Walk>(&bytes).ok());
    if let Some(walk) = &cached {
        let fresh = walk.boot == boot_id()
            && walk
                .began
                .until(&now)
                .is_some_and(|age| age < SUBTREE_REFRESH_MS);
        // One shared ancestor (or a shared v2 group on a hybrid host)
        // cannot vouch for other groups created since the walk.
        let vouched = cgroups.iter().all(|id| walk.vouched.contains_key(id));
        if fresh && (found(walk) || vouched) {
            return found(walk);
        }
    } else {
        crate::pressure::prune(&directory, SystemTime::now());
    }
    // Earlier walks' cgroups stay vouched for while recent, since this walk
    // begins later still; the clocks of a lease they cannot order then cost
    // one walk per refresh interval rather than one per poll.
    let mut vouched: BTreeMap<String, Stamp> = cached
        .filter(|walk| walk.boot == boot_id())
        .map(|walk| walk.vouched)
        .unwrap_or_default();
    vouched.retain(|_, began| {
        began
            .until(&now)
            .is_some_and(|age| age < SUBTREE_REFRESH_MS)
    });
    vouched.extend(cgroups.iter().map(|id| (id.clone(), now)));
    let walk = Walk {
        began: now,
        boot: boot_id().to_owned(),
        members: subtree(path),
        vouched,
    };
    if let Ok(bytes) = serde_json::to_vec(&walk) {
        let _ = crate::util::write_advisory(&file, &bytes);
    }
    found(&walk)
}

/// The identities of every cgroup below `path`, or `None` when some part of
/// it could not be read.
fn subtree(path: &Path) -> Option<BTreeSet<String>> {
    let mut members = BTreeSet::new();
    let mut pending = vec![path.to_path_buf()];
    // A vanished path may have been renamed with live descendants (v1),
    // rather than deleted. Any failed lookup leaves the walk incomplete.
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            // Every cgroup holds dozens of control files. The entry's own
            // type, read with the directory, skips them without a `stat`
            // each, so the walk costs one call per cgroup.
            if !entry.file_type().ok()?.is_dir() {
                continue;
            }
            let metadata = entry.metadata().ok()?;
            members.insert(format!("{}:{}", metadata.dev(), metadata.ino()));
            pending.push(entry.path());
        }
    }
    Some(members)
}

pub(crate) fn memory() -> Memory {
    snapshot(&visible_domains())
}

/// The lowest memory limit on a visible ancestor of this process's cgroup,
/// not counting the process's own cgroup.
///
/// Sibling cgroups can share an ancestor's limit, so a scheduler pool they
/// share must stay within it. A limit on the process's own cgroup, as a
/// container runtime sets one, belongs to that cgroup alone.
pub(crate) fn inherited_memory_limit() -> Option<u64> {
    inherited_limit(&visible_domains())
}

fn inherited_limit(domains: &[Domain]) -> Option<u64> {
    domains
        .iter()
        .flat_map(|domain| {
            domain
                .current
                .ancestors()
                .skip(1)
                .take_while(|path| path.starts_with(&domain.mount))
                .filter_map(|directory| read_limit(directory, domain.v2))
        })
        .min()
}

/// The lowest CPU quota or cpuset size on a visible ancestor of this process's
/// cgroup, in whole CPUs, not counting the process's own cgroup.
///
/// For the reason [`inherited_memory_limit`] gives: siblings share a parent's
/// quota or cpuset, while a container's own restriction belongs to it alone.
pub(crate) fn inherited_cpu_limit() -> Option<u64> {
    let (Ok(groups), Ok(mounts)) = (
        std::fs::read_to_string("/proc/self/cgroup"),
        std::fs::read_to_string("/proc/self/mountinfo"),
    ) else {
        return None;
    };
    inherited_cpu_limits(&groups, &mounts)
}

fn inherited_cpu_limits(groups: &str, mounts: &str) -> Option<u64> {
    let quotas = inherited_cpus(&domains_for(groups, mounts, "cpu"));
    let cpusets = inherited_cpuset_cpus(&domains_for(groups, mounts, "cpuset"));
    quotas.into_iter().chain(cpusets).min()
}

fn inherited_cpuset_cpus(domains: &[Domain]) -> Option<u64> {
    domains.iter().filter_map(inherited_cpuset).min()
}

fn inherited_cpuset(domain: &Domain) -> Option<u64> {
    let read_set = |directory: &Path, name: &str| {
        std::fs::read_to_string(directory.join(name))
            .ok()
            .and_then(|text| cpuset_size(&text))
    };
    if domain.v2
        && std::fs::read_to_string(domain.current.join("cpuset.cpus.partition"))
            .is_ok_and(|text| matches!(text.trim(), "root" | "isolated"))
    {
        // A valid leaf partition's CPUs have been removed from its parents'
        // effective sets. Its own restriction stays with its local ceiling.
        return None;
    }
    let mut configured: Option<u64> = None;
    for directory in domain
        .current
        .ancestors()
        .skip(1)
        .take_while(|path| path.starts_with(&domain.mount))
    {
        let effective = if domain.v2 {
            "cpuset.cpus.effective"
        } else {
            "cpuset.effective_cpus"
        };
        if let Some(cpus) = read_set(directory, effective) {
            // The nearest effective set already includes inherited limits.
            // Higher ancestors can exclude CPUs delegated to this partition.
            return Some(configured.map_or(cpus, |limit| limit.min(cpus)));
        }
        // Older v1 mounts only expose configured sets. Empty sets inherit
        // from the next ancestor; keep every non-empty configured restriction.
        if let Some(cpus) = read_set(directory, "cpuset.cpus") {
            configured = Some(configured.map_or(cpus, |limit| limit.min(cpus)));
        }
    }
    configured
}

/// Kernel CPU lists contain ordered, disjoint CPU numbers or inclusive ranges.
fn cpuset_size(text: &str) -> Option<u64> {
    let mut count = 0_u64;
    let mut previous = None;
    for entry in text.trim().split(',') {
        let (start, end) = entry.split_once('-').unwrap_or((entry, entry));
        let start = start.parse::<u32>().ok()?;
        let end = end.parse::<u32>().ok()?;
        if end < start || previous.is_some_and(|previous| start <= previous) {
            return None;
        }
        count += u64::from(end) - u64::from(start) + 1;
        previous = Some(end);
    }
    Some(count)
}

fn inherited_cpus(domains: &[Domain]) -> Option<u64> {
    domains
        .iter()
        .flat_map(|domain| {
            domain
                .current
                .ancestors()
                .skip(1)
                .take_while(|path| path.starts_with(&domain.mount))
                .filter_map(|directory| read_cpu_quota(directory, domain.v2))
        })
        .min()
}

/// A CPU quota as whole CPUs, rounded down as the standard library rounds a
/// quota, and at least one: v2's `cpu.max`, or v1's `cpu.cfs_quota_us` over
/// `cpu.cfs_period_us`. `max` and v1's `-1` are no quota.
fn read_cpu_quota(directory: &Path, v2: bool) -> Option<u64> {
    let read = |name: &str| std::fs::read_to_string(directory.join(name)).ok();
    let (quota, period) = if v2 {
        let text = read("cpu.max")?;
        let mut fields = text.split_whitespace();
        (
            fields.next()?.parse::<u64>().ok()?,
            fields.next()?.parse::<u64>().ok()?,
        )
    } else {
        (
            read("cpu.cfs_quota_us")?.trim().parse::<u64>().ok()?,
            read("cpu.cfs_period_us")?.trim().parse::<u64>().ok()?,
        )
    };
    (period > 0).then(|| (quota / period).max(1))
}

fn read_limit(directory: &Path, v2: bool) -> Option<u64> {
    let name = if v2 {
        "memory.max"
    } else {
        "memory.limit_in_bytes"
    };
    std::fs::read_to_string(directory.join(name))
        .ok()
        .and_then(|s| {
            if s.trim() == "0" {
                Some(0)
            } else {
                read_cgroup_text(&s)
            }
        })
}

fn visible_domains() -> Vec<Domain> {
    let domains = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .zip(std::fs::read_to_string("/proc/self/mountinfo").ok())
        .map(|(groups, mounts)| domains(&groups, &mounts))
        .unwrap_or_default();
    if domains.is_empty() {
        // Preserve detection in containers that hide proc membership or mounts.
        return fallback_domains(Path::new("/sys/fs/cgroup"));
    }
    domains
}

fn fallback_domains(root: &Path) -> Vec<Domain> {
    [(root.to_path_buf(), true), (root.join("memory"), false)]
        .into_iter()
        // These are optional conventional mount locations, not paths learned
        // from proc. An absent hierarchy is not incomplete lease placement.
        // Other failures remain candidates so permission errors stay unknown.
        .filter(|(path, _)| {
            !std::fs::metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        })
        .map(|(path, v2)| Domain {
            mount: path.clone(),
            current: path,
            v2,
        })
        .collect()
}

fn domains(groups: &str, mounts: &str) -> Vec<Domain> {
    domains_for(groups, mounts, "memory")
}

/// The cgroup v2 hierarchy and the v1 hierarchy carrying `controller`.
fn domains_for(groups: &str, mounts: &str, controller: &str) -> Vec<Domain> {
    let memberships: Vec<_> = groups
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, ':');
            fields.next()?;
            let controllers = fields.next()?;
            let path = Path::new(fields.next()?);
            let v2 = controllers.is_empty();
            (v2 || controllers.split(',').any(|name| name == controller)).then_some((v2, path))
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
                "cgroup" if super_options.split(',').any(|name| name == controller) => false,
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
            let (usage_name, reclaim_fields) = if domain.v2 {
                ("memory.current", ["active_file", "inactive_file"])
            } else {
                (
                    "memory.usage_in_bytes",
                    ["total_active_file", "total_inactive_file"],
                )
            };
            let Some(limit) = read_limit(directory, domain.v2) else {
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
    fn only_ancestor_limits_are_inherited() {
        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path();
        write_memory(mount, "max", 0, 0, true);
        write_memory(&mount.join("slice"), "max", 0, 0, true);
        write_memory(&mount.join("slice/build"), "800", 0, 0, true);
        let mounts = format!("1 0 0:1 / {} rw - cgroup2 cgroup rw", mount.display());
        let found = domains("0::/slice/build\n", &mounts);
        assert_eq!(inherited_limit(&found), None, "the cgroup's own limit");
        write_memory(&mount.join("slice"), "1000", 0, 0, true);
        assert_eq!(
            inherited_limit(&found),
            Some(1000),
            "a limit siblings share"
        );
        // A container's own cgroup is the root of what it can see.
        let own = domains(
            "0::/\n",
            &format!(
                "1 0 0:1 /slice/build {} rw - cgroup2 cgroup rw",
                mount.join("slice/build").display()
            ),
        );
        assert_eq!(inherited_limit(&own), None);
    }

    #[test]
    fn only_ancestor_cpu_quotas_are_inherited() {
        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path();
        let build = mount.join("slice/build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(mount.join("slice/cpu.max"), "max 100000\n").unwrap();
        std::fs::write(build.join("cpu.max"), "200000 100000\n").unwrap();
        let mounts = format!("1 0 0:1 / {} rw - cgroup2 cgroup rw", mount.display());
        let found = domains("0::/slice/build\n", &mounts);
        assert_eq!(inherited_cpus(&found), None, "the cgroup's own quota");
        std::fs::write(mount.join("slice/cpu.max"), "450000 100000\n").unwrap();
        assert_eq!(inherited_cpus(&found), Some(4), "a quota siblings share");
        std::fs::write(mount.join("slice/cpu.max"), "50000 100000\n").unwrap();
        assert_eq!(inherited_cpus(&found), Some(1), "at least one CPU");
    }

    #[test]
    fn parent_cpusets_cap_siblings_without_inheriting_the_leaf_set() {
        for v2 in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let mount = temp.path();
            let build = mount.join("slice/build");
            std::fs::create_dir_all(&build).unwrap();
            let (groups, mounts, effective) = if v2 {
                (
                    "0::/slice/build\n",
                    format!("1 0 0:1 / {} rw - cgroup2 cgroup rw", mount.display()),
                    "cpuset.cpus.effective",
                )
            } else {
                (
                    "3:cpuset:/slice/build\n",
                    format!("1 0 0:1 / {} rw - cgroup cgroup rw,cpuset", mount.display()),
                    "cpuset.effective_cpus",
                )
            };
            std::fs::write(build.join(effective), "0\n").unwrap();
            assert_eq!(
                inherited_cpu_limits(groups, &mounts),
                None,
                "leaf stays local"
            );
            std::fs::write(mount.join(effective), "0-31\n").unwrap();
            std::fs::write(mount.join("slice").join(effective), "0-3\n").unwrap();
            assert_eq!(inherited_cpu_limits(groups, &mounts), Some(4));
            if v2 {
                std::fs::write(mount.join("slice/cpu.max"), "200000 100000\n").unwrap();
                assert_eq!(
                    inherited_cpu_limits(groups, &mounts),
                    Some(2),
                    "quota also applies"
                );
            }
            std::fs::remove_file(mount.join("slice").join(effective)).unwrap();
            std::fs::write(mount.join("slice/cpuset.cpus"), "0-2,8\n").unwrap();
            let domains = domains_for(groups, &mounts, "cpuset");
            assert_eq!(
                inherited_cpuset_cpus(&domains),
                Some(4),
                "configured fallback"
            );
            std::fs::write(mount.join("slice/cpuset.cpus"), "\n").unwrap();
            assert_eq!(inherited_cpuset_cpus(&domains), Some(32), "empty inherits");
        }
    }

    #[test]
    fn v2_partitions_do_not_inherit_the_parents_remaining_cpus() {
        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path();
        let partition = mount.join("partition");
        let build = partition.join("build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(mount.join("cpuset.cpus.effective"), "0\n").unwrap();
        std::fs::write(partition.join("cpuset.cpus.effective"), "1-7\n").unwrap();
        std::fs::write(build.join("cpuset.cpus.effective"), "1\n").unwrap();
        let mounts = format!("1 0 0:1 / {} rw - cgroup2 cgroup rw", mount.display());
        for state in ["root", "isolated"] {
            std::fs::write(partition.join("cpuset.cpus.partition"), state).unwrap();
            assert_eq!(
                inherited_cpu_limits("0::/partition/build\n", &mounts),
                Some(7),
                "the partition's seven CPUs are absent from its parent's effective set"
            );
            assert_eq!(
                inherited_cpu_limits("0::/partition\n", &mounts),
                None,
                "the leaf partition's own restriction stays local"
            );
        }
        std::fs::write(
            partition.join("cpuset.cpus.partition"),
            "root invalid (no cpu)",
        )
        .unwrap();
        assert_eq!(inherited_cpu_limits("0::/partition\n", &mounts), Some(1));
        std::fs::write(mount.join("cpu.max"), "200000 100000\n").unwrap();
        assert_eq!(
            inherited_cpu_limits("0::/partition/build\n", &mounts),
            Some(2),
            "quotas still inherit across partitions"
        );
    }

    #[test]
    fn cpuset_lists_count_ranges_and_reject_malformed_input() {
        assert_eq!(cpuset_size("0-3,8,10-11\n"), Some(7));
        assert_eq!(cpuset_size("3"), Some(1));
        for text in ["", "3-1", "0-3,2", "1,1", "0-", "x", "0,,2"] {
            assert_eq!(cpuset_size(text), None, "{text:?}");
        }
    }

    #[test]
    fn v1_cpu_quotas_are_inherited_from_the_cpu_hierarchy() {
        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path().join("cpu,cpuacct");
        let build = mount.join("slice/build");
        std::fs::create_dir_all(&build).unwrap();
        for (directory, quota) in [(mount.join("slice"), "-1"), (build.clone(), "200000")] {
            std::fs::write(directory.join("cpu.cfs_quota_us"), format!("{quota}\n")).unwrap();
            std::fs::write(directory.join("cpu.cfs_period_us"), "100000\n").unwrap();
        }
        let mounts = format!(
            "1 0 0:1 / {} rw - cgroup cgroup rw,cpu,cpuacct\n2 0 0:2 / /memory rw - cgroup cgroup rw,memory",
            mount.display()
        );
        let groups = "4:memory:/slice/build\n3:cpu,cpuacct:/slice/build\n";
        let found = domains_for(groups, &mounts, "cpu");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].current, build);
        assert_eq!(inherited_cpus(&found), None, "no quota on the parent");
        std::fs::write(mount.join("slice/cpu.cfs_quota_us"), "300000\n").unwrap();
        assert_eq!(inherited_cpus(&found), Some(3));
        assert!(
            domains_for(groups, &mounts, "memory")
                .iter()
                .all(|domain| domain.mount != mount),
            "the memory probes keep reading only the memory hierarchy"
        );
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
    fn a_walk_made_after_reading_a_lease_rules_it_out_whatever_its_clocks() {
        // This process's own cgroup holds nothing that "0:0" names.
        let Some(scope) = own_scope() else {
            return;
        };
        let pool = tempfile::tempdir().unwrap();
        let file = pool.path().join(SUBTREES_DIR).join(format!("{scope}.json"));
        let read = || serde_json::from_slice::<Walk>(&std::fs::read(&file).unwrap()).unwrap();
        assert!(
            contains(pool.path(), scope, &["0:0".into()], None),
            "a record of unknown age"
        );
        // Clocks that cannot be ordered against any walk -- a boot clock
        // offset into the future, as in another time namespace -- still get
        // an answer from the walk made for them.
        let now = Stamp::now();
        let ahead = Some(stamp(now.wall_ms, now.boot_ms + 3_600_000));
        assert!(!contains(pool.path(), scope, &["0:2".into()], ahead));
        let first = read().began;
        // And that walk answers for them on every later poll, rather than
        // each poll walking again.
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            assert!(!contains(pool.path(), scope, &["0:2".into()], ahead));
            assert_eq!(read().began, first, "not walked again");
        }
        // A cached walk the clocks cannot place after a lease it was not
        // made for does not answer for that lease.
        let stepped = Some(stamp(
            now.wall_ms.saturating_sub(60_000),
            now.boot_ms + 60_000,
        ));
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(!contains(pool.path(), scope, &["0:3".into()], stepped));
        assert_ne!(read().began, first, "walked for the new lease");
        // Both leases' cgroups stay vouched for across that walk.
        assert!(read().vouched.contains_key("0:2"));
        assert!(read().vouched.contains_key("0:3"));
    }

    #[test]
    fn a_shared_hybrid_group_does_not_vouch_for_a_new_memory_group() {
        let pool = tempfile::tempdir().unwrap();
        let hierarchy = tempfile::tempdir().unwrap();
        let scope = hierarchy.path().join("v1/limited");
        let shared_v2 = hierarchy.path().join("v2/shared");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::create_dir_all(&shared_v2).unwrap();
        let scope_id = identity(&scope).unwrap();
        let shared_id = identity(&shared_v2).unwrap();
        let file = pool
            .path()
            .join(SUBTREES_DIR)
            .join(format!("{scope_id}.json"));
        let now = stamp(10_000, 10_000);
        let walk = Walk {
            began: now,
            boot: boot_id().to_owned(),
            members: subtree(&scope),
            vouched: BTreeMap::from([(shared_id.clone(), now)]),
        };
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, serde_json::to_vec(&walk).unwrap()).unwrap();

        // A second container shares the v2 group but creates its own v1
        // memory group after the cached walk. Its namespace hides the
        // limited ancestor, so only a new walk can find it inside scope.
        let nested = scope.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let cgroups = [shared_id, identity(&nested).unwrap()];
        assert!(contains_at(
            pool.path(),
            &scope_id,
            &scope,
            &cgroups,
            stamp(10_001, 10_001)
        ));
        let refreshed: Walk = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert!(cgroups.iter().all(|id| refreshed.vouched.contains_key(id)));

        // Once every ID is vouched for, subsequent polls reuse that walk
        // even if the lease's clocks remain unorderable. The controlled
        // clock keeps the cache fresh regardless of test pauses, and each
        // call has a distinct stamp so a rescan would change the file.
        let outside = [cgroups[0].clone(), "0:0".into()];
        assert!(!contains_at(
            pool.path(),
            &scope_id,
            &scope,
            &outside,
            stamp(10_002, 10_002)
        ));
        let cached = std::fs::read(&file).unwrap();
        assert!(!contains_at(
            pool.path(),
            &scope_id,
            &scope,
            &outside,
            stamp(10_003, 10_003)
        ));
        assert_eq!(std::fs::read(&file).unwrap(), cached);
    }

    #[test]
    fn vouching_expires_with_the_refresh_interval() {
        let Some(scope) = own_scope() else {
            return;
        };
        let pool = tempfile::tempdir().unwrap();
        let file = pool.path().join(SUBTREES_DIR).join(format!("{scope}.json"));
        let now = Stamp::now();
        let old = stamp(now.wall_ms - 5_000, now.boot_ms - 5_000);
        let walk = Walk {
            began: now,
            boot: boot_id().to_owned(),
            members: Some(BTreeSet::new()),
            vouched: BTreeMap::from([("0:4".to_string(), old)]),
        };
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, serde_json::to_vec(&walk).unwrap()).unwrap();
        let read = || serde_json::from_slice::<Walk>(&std::fs::read(&file).unwrap()).unwrap();
        let ahead = Some(stamp(now.wall_ms, now.boot_ms + 3_600_000));
        std::thread::sleep(std::time::Duration::from_millis(2));
        // A fresh walk vouching for "0:4" still answers for it...
        assert!(!contains(pool.path(), scope, &["0:4".into()], ahead));
        assert_eq!(read().began, now, "not walked again");
        // ...but the next walk drops it, as its vouching is old.
        assert!(!contains(pool.path(), scope, &["0:5".into()], ahead));
        assert!(!read().vouched.contains_key("0:4"));
        assert!(read().vouched.contains_key("0:5"));
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
        // it found and, for IDs it already checked, in what it did not.
        let file = pool.path().join(SUBTREES_DIR).join(format!("{scope}.json"));
        let now = Stamp::now();
        let shared = Walk {
            began: stamp(now.wall_ms - 500, now.boot_ms - 500),
            boot: boot_id().to_owned(),
            members: Some(BTreeSet::from(["0:5".to_string()])),
            vouched: BTreeMap::from([("0:0".to_string(), now)]),
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
        // An ID not yet checked gets a walk of its own, which other
        // waiters then reuse for it.
        write(&shared);
        let newer = Some(stamp(shared.began.wall_ms + 1, shared.began.boot_ms + 1));
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(!contains(pool.path(), scope, &["0:7".into()], newer));
        let again = read().began;
        assert_ne!(again, shared.began, "walked again");
        assert!(!contains(pool.path(), scope, &["0:7".into()], newer));
        assert_eq!(read().began, again, "not walked a third time");
        // A lease from the walk's own millisecond gets a walk of its own.
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(!contains(pool.path(), scope, &["0:1".into()], Some(again)));
        assert_ne!(read().began, again, "walked for the same-instant lease");
    }

    #[test]
    fn incomplete_identities_never_publish_only_readable_ancestors() {
        let mount = tempfile::tempdir().unwrap();
        let restricted = mount.path().join("restricted");
        let worker = restricted.join("limited/worker");
        std::fs::create_dir_all(&worker).unwrap();
        let paths = vec![
            worker.clone(),
            restricted.clone(),
            mount.path().to_path_buf(),
        ];
        assert_eq!(identify_directories(paths.clone()).len(), 3);
        std::fs::remove_dir(&worker).unwrap();
        // Ancestors remain readable, but publishing only their IDs would
        // let another waiter wrongly exclude this compiler from limited.
        assert!(identify_directories(paths).is_empty());
    }

    #[test]
    fn incomplete_lease_identities_preserve_readable_pressure_domains() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("v2/first");
        let second = root.path().join("v2/second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let missing_v1 = root.path().join("v1/memory");
        let first_paths = vec![first.clone(), missing_v1.clone()];
        let second_paths = vec![second.clone(), missing_v1];
        // A path discovered from proc may disappear or become inaccessible.
        // Such a lease cannot publish a complete placement...
        assert!(identify_directories(first_paths.clone()).is_empty());
        assert!(identify_directories(second_paths.clone()).is_empty());
        // ...but readable v2 probes still belong to distinct cgroups and
        // must not share the host pressure state or each other's state.
        let first_readable = readable_directories(first_paths);
        let second_readable = readable_directories(second_paths);
        assert_eq!(
            first_readable,
            vec![(first.clone(), identity(&first).unwrap())]
        );
        assert_eq!(
            second_readable,
            vec![(second.clone(), identity(&second).unwrap())]
        );
        assert_ne!(first_readable[0].1, second_readable[0].1);
    }

    #[test]
    fn a_v2_only_fallback_keeps_its_lease_placement() {
        let root = tempfile::tempdir().unwrap();
        write_memory(root.path(), "1000", 700, 100, true);
        let fallback = fallback_domains(root.path());
        assert_eq!(fallback.len(), 1, "no v1 hierarchy exists");
        assert!(fallback[0].v2);
        assert_eq!(snapshot(&fallback).headroom, Some(400));
        let placed =
            identify_directories(fallback.into_iter().map(|domain| domain.current).collect());
        assert_eq!(
            placed,
            vec![(root.path().to_path_buf(), identity(root.path()).unwrap())]
        );
        // An existing conventional v1 hierarchy remains a candidate too.
        std::fs::create_dir(root.path().join("memory")).unwrap();
        assert_eq!(fallback_domains(root.path()).len(), 2);
    }

    #[test]
    fn clock_offsets_cannot_vouch_for_a_new_nested_cgroup() {
        let pool = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        let scope_id = identity(scope.path()).unwrap();
        let file = pool
            .path()
            .join(SUBTREES_DIR)
            .join(format!("{scope_id}.json"));
        let walk = Walk {
            began: stamp(1_000_000, 10_000),
            boot: boot_id().to_owned(),
            members: subtree(scope.path()),
            vouched: BTreeMap::new(),
        };
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, serde_json::to_vec(&walk).unwrap()).unwrap();
        let nested = scope.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let cgroups = [identity(&nested).unwrap()];
        // A backward wall step plus a negative time-namespace boot offset
        // makes a newer lease appear older on both clocks.
        let written = stamp(999_910, 9_010);
        assert!(written.before(&walk.began));
        let now = stamp(1_000_050, 10_150);
        assert_eq!(walk.began.until(&now), Some(150));
        assert!(contains_at(
            pool.path(),
            &scope_id,
            scope.path(),
            &cgroups,
            now
        ));
        let refreshed: Walk = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert!(refreshed.vouched.contains_key(&cgroups[0]));
    }

    #[test]
    fn a_renamed_scope_cannot_exclude_its_live_descendants() {
        let tree = tempfile::tempdir().unwrap();
        let pool = tempfile::tempdir().unwrap();
        let scope = tree.path().join("scope");
        let nested = scope.join("container/worker");
        std::fs::create_dir_all(&nested).unwrap();
        let scope_id = identity(&scope).unwrap();
        let cgroups = [identity(&nested).unwrap()];
        assert!(subtree(&scope).unwrap().contains(&cgroups[0]));
        std::fs::rename(&scope, tree.path().join("renamed")).unwrap();
        assert!(subtree(&scope).is_none());
        assert!(contains_at(
            pool.path(),
            &scope_id,
            &scope,
            &cgroups,
            stamp(10_000, 10_000)
        ));
        // The descendant still exists under the renamed path, with its ID.
        assert_eq!(
            identity(&tree.path().join("renamed/container/worker")),
            Some(cgroups[0].clone())
        );
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
