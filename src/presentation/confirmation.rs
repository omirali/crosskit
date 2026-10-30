// The one yes-or-no question a command asks before it writes something the person did not
// write themselves.
use std::io::{self, BufRead, Write};

/// `question [y/N] `, answered on stdin. Only `y` or `yes` is a yes; anything else, and a
/// closed input, is a no.
pub(super) fn confirmed(question: &str) -> Result<bool, String> {
    print!("{question} [y/N] ");
    io::stdout()
        .flush()
        .map_err(|error| format!("the question could not be printed: {error}"))?;
    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|error| format!("the answer could not be read: {error}"))?;
    let answer = answer.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}
