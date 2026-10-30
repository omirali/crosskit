// Descriptor-relative traversal: every entry is opened relative to its parent directory
// with O_NOFOLLOW, so a symlink can never redirect the scan outside the root, and every
// entry is offered to the policy before it is stat-ed or opened.
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use crate::application::{
    Admission, ContentSkipReason, EvidenceRead, IndexError, SourceEntry, SourceVisitor,
};
use crate::domain::{
    ExtractionProfile, FileKind, FileLocation, NormalizedPath, ObservedFileIdentity,
};

use super::entry_stat::{EntryKind, EntryStat, FileFingerprint, lstat_at};
use super::plain_text_evidence::read_plain_text_evidence;
use crate::infrastructure::chats::conversation_evidence::{
    TranscriptOutcome, read_conversation_evidence,
};

pub(super) fn walk(
    root: &NormalizedPath,
    visitor: &mut dyn SourceVisitor,
) -> Result<(), IndexError> {
    if visitor.admit(root) == Admission::Skip {
        return Ok(());
    }
    let root_directory = open_root_directory(root).map_err(|error| root_error(root, error))?;
    let mut pending = vec![(root_directory, root.clone())];

    while let Some((directory, path)) = pending.pop() {
        if visitor.admit_directory(&path, &|name| holds_regular_file(&directory, name))
            == Admission::Skip
        {
            continue;
        }
        let names = match sorted_entry_names(&directory) {
            Ok(names) => names,
            Err(error) => {
                visitor.record_failure(&path, directory_error(error));
                continue;
            }
        };
        visit_directory_entries(&directory, &path, names, visitor, &mut pending)?;
    }
    Ok(())
}

/// A root that could not be opened, told apart like any other directory: `EPERM` is the
/// privacy protection of the process running the pass, not a fault of the tree.
fn root_error(root: &NormalizedPath, error: io::Error) -> IndexError {
    let message = format!("open root {root}: {error}");
    match error.raw_os_error() {
        Some(libc::EPERM) => IndexError::NotPermitted(message),
        _ => IndexError::Source(message),
    }
}

pub(super) fn open_root_directory(root: &NormalizedPath) -> io::Result<fs::File> {
    open_directory_at(libc::AT_FDCWD, OsStr::new(root.as_str()))
}

/// Offers each named entry of `directory` to the visitor. A name that is not valid UTF-8
/// cannot be represented to the string-based policy, so it is skipped before any system
/// call touches it; everything else is admitted, then stat-ed, then visited or queued.
pub(super) fn visit_directory_entries(
    directory: &fs::File,
    path: &NormalizedPath,
    names: Vec<OsString>,
    visitor: &mut dyn SourceVisitor,
    pending: &mut Vec<(fs::File, NormalizedPath)>,
) -> Result<(), IndexError> {
    for name in names {
        let Some(utf8_name) = name.to_str() else {
            visitor.skip_unrepresentable_name();
            continue;
        };
        let child_path = match path.join(utf8_name) {
            Ok(child_path) => child_path,
            Err(error) => {
                visitor.record_failure(path, IndexError::Source(error.to_string()));
                continue;
            }
        };
        if visitor.admit(&child_path) == Admission::Skip {
            continue;
        }
        let stat = match lstat_at(directory.as_raw_fd(), &name) {
            Ok(stat) => stat,
            Err(error) => {
                visitor.record_failure(&child_path, io_error(error));
                continue;
            }
        };

        match stat.kind {
            EntryKind::Directory => match open_directory_at(directory.as_raw_fd(), &name) {
                Ok(child_directory) => pending.push((child_directory, child_path)),
                Err(error) => visitor.record_failure(&child_path, directory_error(error)),
            },
            EntryKind::File(kind) => {
                let entry = DirectoryEntry {
                    directory_fd: directory.as_raw_fd(),
                    name: &name,
                    path: child_path,
                    kind,
                    stat: &stat,
                };
                visitor.visit_entry(&entry)?;
            }
        }
    }
    Ok(())
}

/// Whether `directory` holds a regular file called `name`. One `lstat` relative to the
/// already open directory: no listing, no path built, and nothing followed — a marker that
/// is a symlink or a directory is not the marker its convention describes.
fn holds_regular_file(directory: &fs::File, name: &str) -> bool {
    matches!(
        lstat_at(directory.as_raw_fd(), OsStr::new(name)),
        Ok(stat) if matches!(stat.kind, EntryKind::File(FileKind::RegularFile))
    )
}

/// A non-directory entry, addressed relative to its open parent directory.
struct DirectoryEntry<'a> {
    directory_fd: RawFd,
    name: &'a OsStr,
    path: NormalizedPath,
    kind: FileKind,
    stat: &'a EntryStat,
}

impl SourceEntry for DirectoryEntry<'_> {
    fn path(&self) -> &NormalizedPath {
        &self.path
    }

    fn observed_identity(&self) -> Result<ObservedFileIdentity, IndexError> {
        Ok(self.stat.identity)
    }

    fn location(&self, file_id: &str) -> Result<FileLocation, IndexError> {
        let fingerprint = &self.stat.fingerprint;
        FileLocation::observed(
            file_id,
            self.path.clone(),
            self.kind,
            fingerprint.size_bytes(),
            fingerprint.modified_at().unix_ms().map_err(io_error)?,
            self.stat.observed_at_unix_ms,
        )
        .map_err(|error| IndexError::Source(error.to_string()))
    }

    /// `expected` must equal this entry's own description, so the commit pairs content
    /// with the location it was read under. Stability is judged against the exact
    /// `lstat` fingerprint rather than the millisecond timestamp in `expected`: a
    /// same-size rewrite inside one millisecond must still be detected.
    fn read_evidence(
        &self,
        expected: &FileLocation,
        max_content_bytes: u64,
    ) -> Result<EvidenceRead, IndexError> {
        if *expected != self.location(expected.file_id())? {
            return Err(IndexError::Source(format!(
                "location {} is not the description of entry {}; evidence is only read for the location that will be committed",
                expected.id(),
                self.path
            )));
        }
        let mut file = match open_file_at(self.directory_fd, self.name) {
            Ok(file) => file,
            // The entry turned into a symlink after it was described: never follow it.
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                return Ok(EvidenceRead::Skipped(
                    ContentSkipReason::ChangedWhileReading,
                ));
            }
            Err(error) => return Err(io_error(error)),
        };
        let metadata = file.metadata().map_err(io_error)?;
        let unchanged_since_stat =
            metadata.is_file() && FileFingerprint::of_open_file(&metadata) == self.stat.fingerprint;
        if !unchanged_since_stat {
            return Ok(EvidenceRead::Skipped(
                ContentSkipReason::ChangedWhileReading,
            ));
        }
        // The first dispatch by kind of file this program has ever had. Until Stage 32 every
        // file went through `plain-text`, which read a transcript of a coding session as a
        // log; the profile is chosen from the name, and a name that promised a transcript and
        // did not hold one falls back to the text profile rather than publishing nothing.
        match ExtractionProfile::of_path(&self.path) {
            ExtractionProfile::Conversation => {
                let outcome = read_conversation_evidence(
                    &mut file,
                    &metadata,
                    expected.file_id(),
                    expected.display_name(),
                    max_content_bytes,
                )
                .map_err(io_error)?;
                match outcome {
                    TranscriptOutcome::Read(read) => Ok(*read),
                    TranscriptOutcome::NotATranscript => read_plain_text_evidence(
                        &mut file,
                        &metadata,
                        expected.file_id(),
                        max_content_bytes,
                    )
                    .map_err(io_error),
                }
            }
            ExtractionProfile::PlainText => read_plain_text_evidence(
                &mut file,
                &metadata,
                expected.file_id(),
                max_content_bytes,
            )
            .map_err(io_error),
        }
    }
}

fn io_error(error: io::Error) -> IndexError {
    IndexError::Source(error.to_string())
}

/// A directory that could not be opened or listed. `EPERM` is told apart by its number: it
/// is what macOS answers for a place behind privacy protection, where `EACCES` is ordinary
/// permissions and stays an ordinary failure.
fn directory_error(error: io::Error) -> IndexError {
    match error.raw_os_error() {
        Some(libc::EPERM) => IndexError::NotPermitted(error.to_string()),
        _ => io_error(error),
    }
}

pub(crate) fn open_directory_at(parent_fd: RawFd, name: &OsStr) -> io::Result<fs::File> {
    open_at(
        parent_fd,
        name,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    )
}

/// O_NONBLOCK keeps a FIFO that replaced the file from blocking the scan.
fn open_file_at(parent_fd: RawFd, name: &OsStr) -> io::Result<fs::File> {
    open_at(
        parent_fd,
        name,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
    )
}

fn open_at(parent_fd: RawFd, name: &OsStr, flags: libc::c_int) -> io::Result<fs::File> {
    let name = c_name(name)?;
    let fd = unsafe { libc::openat(parent_fd, name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

/// Creates `name` under `parent_fd` as a fresh regular file with `mode`, or fails: with
/// `O_EXCL | O_NOFOLLOW` an existing file, a planted symlink or anything else at that
/// name is never opened, truncated or followed.
pub(super) fn create_exclusive_at(
    parent_fd: RawFd,
    name: &OsStr,
    mode: libc::c_uint,
) -> io::Result<fs::File> {
    let name = c_name(name)?;
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: `name` is NUL-terminated; `openat` with O_CREAT reads `mode` as its fourth
    // argument, passed as the promoted integer the variadic C signature expects.
    let fd = unsafe { libc::openat(parent_fd, name.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

pub(crate) fn c_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn sorted_entry_names(directory: &fs::File) -> io::Result<Vec<OsString>> {
    let duplicated_fd = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicated_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(duplicated_fd) };
    if stream.is_null() {
        unsafe { libc::close(duplicated_fd) };
        return Err(io::Error::last_os_error());
    }

    let mut names = Vec::new();
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(OsStr::from_bytes(name).to_os_string());
        }
    }
    if unsafe { libc::closedir(stream) } != 0 {
        return Err(io::Error::last_os_error());
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
pub(super) fn refuses_replaced_directory(root: &std::path::Path, name: &OsStr) -> bool {
    let root = match open_directory_at(libc::AT_FDCWD, root.as_os_str()) {
        Ok(root) => root,
        Err(_) => return false,
    };
    open_directory_at(root.as_raw_fd(), name).is_err()
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{directory_error, root_error};
    use crate::application::{IndexError, PassFailure};
    use crate::domain::NormalizedPath;

    /// Told apart by the number the system answers with, never by the words of the message.
    #[test]
    fn only_eperm_on_a_directory_is_the_system_keeping_it_closed() {
        assert!(matches!(
            directory_error(io::Error::from_raw_os_error(libc::EPERM)),
            IndexError::NotPermitted(_)
        ));
        assert!(matches!(
            directory_error(io::Error::from_raw_os_error(libc::EACCES)),
            IndexError::Source(_)
        ));
        assert!(matches!(
            directory_error(io::Error::other("Operation not permitted")),
            IndexError::Source(_)
        ));
    }

    /// A root macOS keeps closed is its own category of a failed pass, so the person reads
    /// that the setting is missing rather than that the tree is broken.
    #[test]
    fn a_root_the_system_keeps_closed_fails_the_pass_as_not_permitted() {
        let root = NormalizedPath::from_absolute("/Users/alice/Documents").unwrap();
        let closed = root_error(&root, io::Error::from_raw_os_error(libc::EPERM));
        assert_eq!(
            PassFailure::of_index_error(&closed),
            PassFailure::NotPermitted
        );
        let gone = root_error(&root, io::Error::from_raw_os_error(libc::ENOENT));
        assert_eq!(PassFailure::of_index_error(&gone), PassFailure::Source);
    }
}
