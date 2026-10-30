// How a run ends, as one rule.
//
// Four situations, three codes, and nothing anywhere else in the program decides this:
//
// | the run                                                           | code |
// | ----------------------------------------------------------------- | ---- |
// | did what was asked                                                |  0   |
// | answered a question about the program (`--help`, `--version`)     |  0   |
// | printed into an output nobody was reading any more                |  0   |
// | did not do what was asked                                         |  1   |
// | failed as a program: it panicked                                  | 101  |
//
// The third row is the one that had to be built. `crosskit status | head` closes the pipe
// as soon as `head` has its lines, and the standard library turns the next `println!` into a
// panic — so a run that did exactly what was asked used to end in a crash report about a
// pipe, with code 101. It now ends silently with 0: the command did its work, and the person
// who asked stopped listening.
//
// The fourth row covers three things that are different to a person and the same to a shell:
// a request this program could not parse, a context it could not open, and a pass it could
// not finish. Separating them into codes of their own would mean a typed error on every
// command's return, which is a change to every command rather than to this one place; what
// tells them apart today is the sentence on stderr, and each of those sentences says what to
// do next.
//
// 101 is what the standard library uses for a panic, kept so a crash looks the same whether
// or not it was caught here.
use std::io::{self, Write};
use std::panic;

use crate::infrastructure::{OutputStream, reading_end_is_gone};

const DID_WHAT_WAS_ASKED: i32 = 0;
const DID_NOT: i32 = 1;
const PANICKED: i32 = 101;

/// Runs the command and returns the code the process should end with.
///
/// Everything that can end a run passes through here: the command's own answer, the flush of
/// what it printed, and a panic from inside it. A panic is caught rather than left to unwind
/// out of `main` because one kind of panic is not a fault — the one raised by printing into
/// an output whose reader is gone — and telling them apart needs a live process.
pub fn run_to_exit_code() -> i32 {
    quiet_about_an_output_nobody_reads();
    let outcome = panic::catch_unwind(super::run);
    let Ok(outcome) = outcome else {
        return if reading_end_is_gone(OutputStream::Stdout) {
            DID_WHAT_WAS_ASKED
        } else {
            PANICKED
        };
    };
    // Flushed here rather than left to the exit path, because the process ends by
    // `process::exit`, which runs no destructors: whatever a command printed has to reach
    // the descriptor before then. A flush that fails because the reader is gone is the same
    // situation as the panic above and ends the same way.
    if let Err(error) = io::stdout().flush()
        && error.kind() == io::ErrorKind::BrokenPipe
    {
        return DID_WHAT_WAS_ASKED;
    }
    match outcome {
        Ok(()) => DID_WHAT_WAS_ASKED,
        Err(failure) => {
            report(&failure);
            DID_NOT
        }
    }
}

/// Says what went wrong, unless there is nobody left to say it to.
fn report(failure: &str) {
    if reading_end_is_gone(OutputStream::Stderr) {
        return;
    }
    eprintln!("crosskit failed: {failure}");
}

/// Keeps the panic report for panics that are worth reporting.
///
/// A panic raised because the reading end of stdout is closed says nothing a person can act
/// on — it names a pipe, not a mistake — and printing it would itself be printing into an
/// output nobody is reading. Every other panic is reported exactly as the standard library
/// would report it.
fn quiet_about_an_output_nobody_reads() {
    let standard = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if reading_end_is_gone(OutputStream::Stdout) || reading_end_is_gone(OutputStream::Stderr) {
            return;
        }
        standard(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::{DID_NOT, DID_WHAT_WAS_ASKED, PANICKED};

    /// The codes are a contract with whoever runs this program, so they are stated here and
    /// not derived from anything that could move under them.
    #[test]
    fn the_three_codes_are_the_ones_the_table_names() {
        assert_eq!(DID_WHAT_WAS_ASKED, 0);
        assert_eq!(DID_NOT, 1);
        assert_eq!(
            PANICKED, 101,
            "a caught panic must look like an uncaught one"
        );
    }
}
