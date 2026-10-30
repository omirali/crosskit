// One service per data directory. Two would both try to write every context's graph; the
// graph's own lock would refuse the second, but only after it had opened, probed and
// written its status over the first one's.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

/// Held for as long as the service runs; the lock goes with the process, however it ends.
pub struct InstanceLock {
    _file: File,
}

#[derive(Debug)]
pub enum InstanceLockError {
    /// Another service already runs, as this process.
    HeldBy(i32),
    Io(io::Error),
}

impl InstanceLock {
    pub fn acquire(path: &Path) -> Result<Self, InstanceLockError> {
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory).map_err(InstanceLockError::Io)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(InstanceLockError::Io)?;
        let mut lock = whole_file_write_lock();
        // Safety: `lock` is a fully initialized `flock` and the descriptor is open for
        // writing, which F_SETLK with a write lock requires.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &mut lock) } == 0 {
            return Ok(Self { _file: file });
        }
        let error = io::Error::last_os_error();
        match instance_holder(path) {
            Some(pid) => Err(InstanceLockError::HeldBy(pid)),
            None => Err(InstanceLockError::Io(error)),
        }
    }
}

/// The process that holds the lock, asked without taking it — which is also how `status`
/// learns that a service runs whether or not launchd started it.
///
/// A POSIX record lock and not `flock`, because only a record lock names its holder. Never
/// call this from the process that holds the lock: closing the descriptor opened here drops
/// that process's lock on the file.
pub fn instance_holder(path: &Path) -> Option<i32> {
    let file = File::open(path).ok()?;
    let mut lock = whole_file_write_lock();
    // Safety: as above; F_GETLK only reads the lock table.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) } == -1 {
        return None;
    }
    (lock.l_type != libc::F_UNLCK as libc::c_short).then_some(lock.l_pid)
}

#[allow(
    clippy::unnecessary_cast,
    reason = "F_WRLCK is c_short on macOS and c_int on Linux; l_type is c_short on both"
)]
fn whole_file_write_lock() -> libc::flock {
    // Safety: an all-zero `flock` is a valid value of the C struct.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as libc::c_short;
    lock.l_whence = libc::SEEK_SET as libc::c_short;
    lock
}
