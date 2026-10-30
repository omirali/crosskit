// The service's journal: what it did, one line at a time, bounded in size.
//
// The lines are the ones `index watch` prints — counters, roots the configuration names,
// fixed sentences — so nothing here decides what is safe to write. What it decides is how
// much: launchd keeps a file it was given forever and never turns it over, so the journal is
// written by the service itself and never grows past twice its limit.
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::super::utc_stamp::utc_moment;
use super::file_stamp::now_ms;

/// One megabyte of lines, and the megabyte before it. A pass line is about two hundred bytes,
/// so this is some five thousand passes of history — days of a context under edit — which is
/// more than anybody reads and little enough that nobody has to clean it up.
const JOURNAL_LIMIT: u64 = 1024 * 1024;

pub struct Journal {
    path: PathBuf,
    limit: u64,
    file: Mutex<Option<File>>,
}

impl Journal {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::with_limit(path, JOURNAL_LIMIT)
    }

    fn with_limit(path: &Path, limit: u64) -> io::Result<Self> {
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            limit,
            file: Mutex::new(Some(append(path)?)),
        })
    }

    /// Writes `text`, each of its lines stamped with the moment and, when there is one, the
    /// context it is about. A journal that cannot be written loses the line and keeps the
    /// service: the status file says what the service is doing either way.
    pub fn write(&self, context: Option<&str>, text: &str) {
        let Ok(mut file) = self.file.lock() else {
            return;
        };
        let stamp = utc_moment(now_ms());
        let mut written = String::new();
        for line in text.lines() {
            match context {
                Some(context) => written.push_str(&format!("{stamp} {context}: {line}\n")),
                None => written.push_str(&format!("{stamp} {line}\n")),
            }
        }
        if let Some(open) = file.as_mut()
            && open.write_all(written.as_bytes()).is_ok()
        {
            self.turn_over_if_full(&mut file);
        }
    }

    /// Keeps the journal under its limit: a full journal becomes the previous one, replacing
    /// what was there, and a new one starts.
    fn turn_over_if_full(&self, file: &mut Option<File>) {
        let full = file
            .as_ref()
            .and_then(|open| open.metadata().ok())
            .is_some_and(|metadata| metadata.len() >= self.limit);
        if !full {
            return;
        }
        *file = fs::rename(&self.path, previous(&self.path))
            .and_then(|()| append(&self.path))
            .ok();
    }

    /// The last `count` lines, from the previous journal when the current one is short.
    pub fn tail(path: &Path, count: usize) -> io::Result<Vec<String>> {
        let mut lines = match lines_of(&previous(path)) {
            Ok(lines) => lines,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        lines.extend(lines_of(path)?);
        let skip = lines.len().saturating_sub(count);
        Ok(lines.split_off(skip))
    }
}

/// launchd's own capture of the process, cut back when it has grown: it only holds what the
/// service could not write into its journal, but a service that fails at every start would
/// add to it once a minute for as long as nobody looks.
pub fn trim_launchd_output(path: &Path) {
    let large = fs::metadata(path).is_ok_and(|metadata| metadata.len() > JOURNAL_LIMIT);
    if large {
        // Truncating rather than removing: launchd holds the file open for the process and
        // keeps writing to the same descriptor.
        let _ = OpenOptions::new().write(true).truncate(true).open(path);
    }
}

fn previous(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

fn append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn lines_of(path: &Path) -> io::Result<Vec<String>> {
    BufReader::new(File::open(path)?).lines().collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::Journal;

    #[test]
    fn a_full_journal_turns_over_and_never_grows_past_twice_its_limit() {
        let directory =
            std::env::temp_dir().join(format!("crosskit-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let path = directory.join("service.log");
        let journal = Journal::with_limit(&path, 400).unwrap();

        for pass in 0..100 {
            journal.write(Some("work"), &format!("Pass {pass} (events) completed."));
        }

        let current = fs::metadata(&path).unwrap().len();
        let previous = fs::metadata(directory.join("service.log.1")).unwrap().len();
        assert!(
            current < 400 && previous < 400 + 100,
            "{current} + {previous}"
        );
        let tail = Journal::tail(&path, 3).unwrap();
        assert_eq!(tail.len(), 3);
        assert!(
            tail[2].ends_with("work: Pass 99 (events) completed."),
            "{tail:?}"
        );
        fs::remove_dir_all(&directory).unwrap();
    }
}
