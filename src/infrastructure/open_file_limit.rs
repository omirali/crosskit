// How many files this process may hold open, raised once at the start of the service.
//
// launchd starts an agent with a soft limit of 256 open files. The service holds the storage
// engine's files for two graphs, a search index per context whose segments are files of their
// own, a change stream and a socket. On 2026-10-30, after 18 hours of passes every few seconds,
// a checkpoint could not open its log — "Too many open files" — and the engine died on its next
// read inside that checkpoint, leaving both graphs unopenable (ADR 0060). The hard limit is the
// system's; the soft one is the process's own to raise up to it.
use std::io;

/// What the service asks for. macOS refuses a soft limit above `OPEN_MAX`, 10 240, even when
/// the hard limit is unlimited.
const WANTED: libc::rlim_t = 10_240;

/// The soft limit before and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFileLimit {
    pub before: u64,
    pub now: u64,
}

/// Raises the soft limit on open files to `WANTED`, or to the hard limit when that is lower.
/// A limit already as high is left as it is.
pub fn raise_open_file_limit() -> io::Result<OpenFileLimit> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a live, writable `libc::rlimit` for the call to fill.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let before = limit.rlim_cur;
    let wanted = limit.rlim_max.min(WANTED);
    if wanted <= before {
        return Ok(OpenFileLimit {
            before,
            now: before,
        });
    }
    limit.rlim_cur = wanted;
    // SAFETY: `limit` is a valid `libc::rlimit` read back from the same call's counterpart.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(OpenFileLimit {
        before,
        now: wanted,
    })
}

#[cfg(test)]
mod tests {
    use super::{WANTED, raise_open_file_limit};

    /// Raising never lowers: a process started with more keeps it, and one started with less
    /// ends with at least what it asked for or the hard limit.
    #[test]
    fn the_limit_is_raised_and_never_lowered() {
        let limit = raise_open_file_limit().unwrap();
        assert!(limit.now >= limit.before);
        assert!(limit.now >= WANTED || limit.now == limit.before);
        assert_eq!(raise_open_file_limit().unwrap().before, limit.now);
    }
}
