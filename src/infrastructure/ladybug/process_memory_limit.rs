// How much memory this process is actually allowed, as the operating system will enforce
// it — which is not always how much the machine has.
//
// The storage engine sizes its buffer pool from `sysconf(_SC_PHYS_PAGES) * _SC_PAGESIZE`
// and keeps 80% of it. Inside a container those calls answer with the host's memory, not
// with the limit the process runs under: measured at 19 971 MiB reported against a cgroup
// limit of 1 024 MiB. The pool then aims at a ceiling it can never reach, so it never
// evicts a page, and the kernel ends the process once the pool has outgrown the real limit.
// That is why a worker died at a particular number of jobs instead of in proportion to
// anything Crosskit allocated.
//
// Nothing here is configurable. The limit is read from the kernel, never from the
// environment or the policy file: a number a person could type would be a second place for
// the truth about a limit the kernel already knows.
use std::fs;
use std::path::{Path, PathBuf};

/// Where cgroup v2 publishes the limit of the process's own cgroup. Inside a container the
/// cgroup namespace root is mounted here, so this is that container's limit.
const CGROUP_V2_ROOT: &str = "/sys/fs/cgroup";

/// The limit this process runs under, when it is stricter than the machine it runs on.
///
/// `None` means the process may use what the machine has, which is what the engine already
/// assumes and sizes itself for.
pub(super) fn enforced_bytes() -> Option<u64> {
    let limit = cgroup_limit_bytes()?;
    // A limit at or above the machine's memory is not a limit: the engine's own sizing is
    // already bounded by the same memory, and capping to it would change nothing while
    // making every report claim a constraint that is not there.
    match physical_memory_bytes() {
        Some(physical) if limit >= physical => None,
        _ => Some(limit),
    }
}

/// The limit of this process's cgroup, under either version of the interface.
fn cgroup_limit_bytes() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        // No cgroups, so the machine's memory is the limit and the engine already reads it.
        return None;
    }
    v2_limit_bytes().or_else(v1_limit_bytes)
}

fn v2_limit_bytes() -> Option<u64> {
    for candidate in v2_candidates() {
        if let Some(limit) = read_limit(&candidate) {
            return Some(limit);
        }
    }
    None
}

/// The cgroup's own `memory.max` first, then the one named by `/proc/self/cgroup` under the
/// same mount. The first is what a container sees; the second is what a process inside a
/// nested cgroup on a host sees, where the mount shows the whole tree.
fn v2_candidates() -> Vec<PathBuf> {
    let root = Path::new(CGROUP_V2_ROOT);
    let mut candidates = vec![root.join("memory.max")];
    if let Some(relative) = own_cgroup_path() {
        candidates.push(
            root.join(relative.trim_start_matches('/'))
                .join("memory.max"),
        );
    }
    candidates
}

/// The path in the line `0::/some/path` of `/proc/self/cgroup`, which is the v2 entry.
fn own_cgroup_path() -> Option<String> {
    fs::read_to_string("/proc/self/cgroup")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("0::").map(str::to_string))
}

fn v1_limit_bytes() -> Option<u64> {
    read_limit(Path::new("/sys/fs/cgroup/memory/memory.limit_in_bytes"))
}

/// Anything at or above this is the interface's own way of saying "unlimited" rather than a
/// size: cgroup v1 writes `LONG_MAX` rounded down to a page, about 9.2 exbibytes, where v2
/// writes `max`. No machine has four exbibytes, so a number this large is a word.
const NOT_A_LIMIT: u64 = 1 << 62;

/// One limit file. `max` and any value large enough to be that interface's own "unlimited"
/// are read as no limit, because both mean the same thing and neither is a number to size
/// anything from.
fn read_limit(path: &Path) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text == "max" {
        return None;
    }
    let limit: u64 = text.parse().ok()?;
    (limit > 0 && limit < NOT_A_LIMIT).then_some(limit)
}

/// What the machine has, read the same way the engine reads it, so the comparison above is
/// between the same two things the engine would compare.
fn physical_memory_bytes() -> Option<u64> {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    (pages > 0 && page_size > 0).then(|| pages as u64 * page_size as u64)
}

#[cfg(test)]
mod tests {
    use super::read_limit;
    use std::fs;

    fn limit_file(name: &str, contents: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("crosskit-limit-{}-{name}", std::process::id()));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn a_limit_file_is_a_limit_only_when_it_names_one() {
        let path = limit_file("max", "max\n");
        assert_eq!(read_limit(&path), None, "`max` is cgroup v2 for no limit");

        let path = limit_file("bytes", "1073741824\n");
        assert_eq!(read_limit(&path), Some(1_073_741_824));

        // cgroup v1 spells no limit as a number near the top of the range rather than a
        // word, and sizing a buffer pool from it would be sizing it from nothing.
        let path = limit_file("v1-unlimited", "9223372036854771712\n");
        assert_eq!(read_limit(&path), None);

        let path = limit_file("zero", "0\n");
        assert_eq!(read_limit(&path), None);

        let path = limit_file("nonsense", "quite a lot\n");
        assert_eq!(read_limit(&path), None);

        assert_eq!(
            read_limit(std::path::Path::new("/no/such/limit/file")),
            None,
            "a machine without cgroups answers nothing, not zero"
        );
        let _ = fs::remove_file(limit_file("cleanup", ""));
    }
}
