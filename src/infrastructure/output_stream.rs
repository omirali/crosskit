// Whether anybody is still reading what this program prints.
//
// `crosskit status | head` closes the reading end of the pipe as soon as `head` has what it
// wants. Every write after that fails, and the standard library turns a failed `println!`
// into a panic — so a command that printed more than a reader wanted ended in a crash
// report about a pipe. That is not a fault of the run: it did what was asked, and the person
// who asked stopped listening.
//
// The question is put to the operating system rather than guessed from the text of a panic:
// `poll` on the writing end reports `POLLHUP` (macOS) or `POLLERR` (Linux) once the reading
// end is closed, and reports `POLLOUT` for a terminal, a file or a pipe somebody is still
// reading. It takes nothing and changes nothing, and it does not depend on the wording the
// standard library happens to use.
use std::os::unix::io::RawFd;

/// The stream a question is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl OutputStream {
    fn descriptor(self) -> RawFd {
        match self {
            Self::Stdout => 1,
            Self::Stderr => 2,
        }
    }
}

/// Whether the far end of this stream has been closed.
///
/// `false` when the question cannot be put at all, because a program that cannot ask has not
/// been told that nobody is reading, and treating silence as "nobody is there" would turn
/// every unrelated failure into a successful run.
pub fn reading_end_is_gone(stream: OutputStream) -> bool {
    let mut watched = libc::pollfd {
        fd: stream.descriptor(),
        events: libc::POLLOUT,
        revents: 0,
    };
    // Safety: one initialized `pollfd`, and a zero timeout, so this asks the current state
    // and returns at once. `poll` reads; it acquires nothing.
    let ready = unsafe { libc::poll(&mut watched, 1, 0) };
    if ready < 1 {
        return false;
    }
    watched.revents & (libc::POLLHUP | libc::POLLERR) != 0
}

#[cfg(test)]
mod tests {
    use std::os::unix::io::{AsRawFd, RawFd};

    use super::{OutputStream, reading_end_is_gone};

    /// The same question `reading_end_is_gone` puts, about a descriptor a test owns.
    fn gone(fd: RawFd) -> bool {
        let mut watched = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut watched, 1, 0) };
        ready >= 1 && watched.revents & (libc::POLLHUP | libc::POLLERR) != 0
    }

    /// A pipe answers differently once its reading end is closed, and that is the whole
    /// signal this module stands on. A file never answers that way, so an ordinary run
    /// redirected into one is never mistaken for a reader that left.
    #[test]
    fn a_pipe_says_so_when_its_reading_end_closes_and_a_file_never_does() {
        let mut ends = [0 as RawFd; 2];
        assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
        let (reading, writing) = (ends[0], ends[1]);

        assert!(!gone(writing), "both ends are open");
        assert_eq!(unsafe { libc::close(reading) }, 0);
        assert!(gone(writing), "the reading end is closed");
        unsafe { libc::close(writing) };

        let file = std::fs::File::create(
            std::env::temp_dir().join(format!("crosskit-output-stream-{}", std::process::id())),
        )
        .unwrap();
        assert!(!gone(file.as_raw_fd()), "a file is always writable");
    }

    /// A test binary's own streams are open, whatever the harness attached to them.
    #[test]
    fn the_streams_of_a_running_program_are_not_gone() {
        assert!(!reading_end_is_gone(OutputStream::Stdout));
        assert!(!reading_end_is_gone(OutputStream::Stderr));
    }
}
