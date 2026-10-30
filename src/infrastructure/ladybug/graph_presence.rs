// What can be said about a context's graph without opening it.
//
// Every other command asks the engine: it opens the database and acts on what it gets.
// A report over every context may not, because that opening writes. It creates a database
// for a context nobody has indexed, it migrates one written by an older build, and it folds
// the write-ahead log into the database when it closes. A report that does any of those is
// not a report — and the one state it most needs to name is exactly the state in which the
// engine sometimes dies instead of answering.
//
// Two facts lying on disk answer all of it, and neither costs an open:
//
//   * the write-ahead log. The engine keeps committed batches in it and folds them in when
//     it checkpoints — on its own once the log grows, and when the database is closed. So a
//     log exists while a pass is writing, and after a process that stopped without closing.
//     A clean close leaves none.
//   * the lock. The engine takes a POSIX write lock on the database file for a writer and
//     takes none at all for a reader. `fcntl(F_GETLK)` asks who holds a lock and never
//     takes one, so it names the process that is writing right now — and says `free` when
//     the process that left the log is gone.
//
// Crossed, they separate the four states an operator acts on differently. Measured on
// macOS 26.4 against lbug 0.20.4, one context put into each state:
//
//   idle, closed cleanly       log absent    lock free
//   watcher between passes     log absent    lock held by the watcher
//   pass writing right now     log present   lock held by that pass
//   pass killed mid-write      log present   lock free
//
// The last row is the whole point: a log whose writer is gone is a pass that was stopped,
// which is the state `index recover` exists for, and it is named here without opening the
// database the engine may not survive.
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use crate::application::RepositoryError;
use crate::domain::WorkspaceName;

use super::manager::{workspace_database_path, write_ahead_log_of};

/// What a context's graph is doing, as far as its files on disk say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphPresence {
    /// No database: nothing has ever indexed this context.
    NeverIndexed,
    /// It opens, and nobody is writing to it.
    Ready,
    /// Nothing is left in a log, and a writer holds the database between its passes. A
    /// reader opens it and answers from what that writer has folded in.
    HeldByWriter { pid: i32 },
    /// A pass is writing right now. What it has committed is in a log only it can replay,
    /// so a reader gets nothing of it until the pass ends.
    BeingWritten { pid: i32 },
    /// A log with nobody writing it: a pass was stopped without closing the database.
    StoppedUncleanly { log: PathBuf },
    /// The files are there and the question could not be put. Said rather than guessed,
    /// because the guess would be about whether a context is broken.
    Undetermined { reason: String },
}

/// The state of one context's graph, read from the filesystem alone.
pub fn presence_of(workspace: &WorkspaceName) -> Result<GraphPresence, RepositoryError> {
    Ok(presence_at(&workspace_database_path(workspace)?))
}

/// The same for a database named by its path, for a directory that is not the data
/// directory this run resolves — the one under the product's earlier name.
pub fn presence_at(database: &Path) -> GraphPresence {
    if !database.exists() {
        return GraphPresence::NeverIndexed;
    }
    let log = write_ahead_log_of(database);
    match (log.exists(), write_lock_of(database)) {
        (_, WriteLock::Unanswered(reason)) => GraphPresence::Undetermined { reason },
        (false, WriteLock::Free) => GraphPresence::Ready,
        (false, WriteLock::Held { pid }) => GraphPresence::HeldByWriter { pid },
        (true, WriteLock::Held { pid }) => GraphPresence::BeingWritten { pid },
        (true, WriteLock::Free) => GraphPresence::StoppedUncleanly { log },
    }
}

/// Who holds a write lock on the database file, asked without taking one.
enum WriteLock {
    Free,
    Held { pid: i32 },
    Unanswered(String),
}

/// `fcntl(F_GETLK)` with a write lock as the question: it reports the first lock that would
/// conflict, which for a file the engine locks only for writers is the writer itself, and it
/// acquires nothing whatever the answer is.
///
/// The file is opened read-only, which is what `F_GETLK` needs and all this asks for. A lock
/// this process itself held would come back as free — POSIX locks belong to the process, not
/// the descriptor — and that is not a case this program has: nothing that reports on a
/// context is also writing to it.
///
/// The casts are not decoration. `F_WRLCK` and `F_UNLCK` are `c_short` on macOS and `c_int`
/// on Linux, while `flock.l_type` is `c_short` on both, so on one platform each cast is a
/// widening and on the other it is a no-op. Writing them without the cast compiles on macOS
/// and fails on Linux, which is how this was found.
#[allow(
    clippy::unnecessary_cast,
    reason = "F_WRLCK and F_UNLCK are c_short on macOS and c_int on Linux; l_type is c_short on both"
)]
fn write_lock_of(database: &Path) -> WriteLock {
    let file = match File::open(database) {
        Ok(file) => file,
        Err(error) => return WriteLock::Unanswered(format!("{database:?} did not open: {error}")),
    };
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as libc::c_short;
    lock.l_whence = libc::SEEK_SET as libc::c_short;
    lock.l_start = 0;
    lock.l_len = 0;
    // Safety: `lock` is a fully initialized `flock` and the descriptor is open for reading,
    // which is what F_GETLK requires. The call only reads the lock table.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) } == -1 {
        return WriteLock::Unanswered(format!(
            "the lock on this database could not be asked about: {}",
            std::io::Error::last_os_error()
        ));
    }
    if lock.l_type == libc::F_UNLCK as libc::c_short {
        return WriteLock::Free;
    }
    WriteLock::Held { pid: lock.l_pid }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{GraphPresence, presence_at};

    /// The three states a lock nobody holds can be in, told apart by the two files alone.
    ///
    /// Nothing here opens a database, which is the point: the answers come from whether the
    /// two files exist. The two states that need a live writer are put together by the CLI
    /// tests, which have a second process to hold the lock with.
    #[test]
    fn with_no_writer_the_two_files_alone_say_which_state_a_context_is_in() {
        let directory =
            std::env::temp_dir().join(format!("crosskit-presence-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let database = directory.join("graph-test.lbug");

        assert_eq!(presence_at(&database), GraphPresence::NeverIndexed);

        fs::write(&database, "not a real database, and never opened as one").unwrap();
        assert_eq!(presence_at(&database), GraphPresence::Ready);

        let log = directory.join("graph-test.lbug.wal");
        fs::write(&log, "a log nobody is writing").unwrap();
        assert_eq!(
            presence_at(&database),
            GraphPresence::StoppedUncleanly { log }
        );

        fs::remove_dir_all(&directory).unwrap();
    }
}
