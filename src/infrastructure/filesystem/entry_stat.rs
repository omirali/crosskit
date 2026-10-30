// Descriptor-relative `lstat`: the metadata of a directory entry itself, taken without
// opening it and without following a symlink, plus the exact fingerprint that the opened
// descriptor must still match before content is published.
use std::ffi::{CStr, CString, OsStr};
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;

use crate::domain::{FileKind, ObservedFileIdentity};

use super::plain_text_evidence::current_unix_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryKind {
    Directory,
    File(FileKind),
}

#[derive(Debug, Clone)]
pub(super) struct EntryStat {
    pub(super) kind: EntryKind,
    /// What the filesystem says about which file this is, birth time included. The graph
    /// decides what to publish it under; the adapter only reports.
    pub(super) identity: ObservedFileIdentity,
    pub(super) fingerprint: FileFingerprint,
    pub(super) observed_at_unix_ms: i64,
}

/// The modification time exactly as the kernel reports it. Stability checks compare it
/// field by field, so a rewrite inside one millisecond is still detected; the domain only
/// keeps the millisecond form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModificationTime {
    seconds: i64,
    nanoseconds: i64,
}

impl ModificationTime {
    pub(super) fn new(seconds: i64, nanoseconds: i64) -> Self {
        Self {
            seconds,
            nanoseconds,
        }
    }

    pub(super) fn unix_ms(self) -> io::Result<i64> {
        let milliseconds = self
            .seconds
            .checked_mul(1_000)
            .and_then(|value| value.checked_add(self.nanoseconds / 1_000_000));
        milliseconds.ok_or_else(|| io::Error::other("modification time exceeds i64 milliseconds"))
    }
}

/// Scan-only identity of one exact file state: identity, size and exact modification
/// time. The `lstat` snapshot and the opened descriptor must produce equal fingerprints
/// before any content may be published together with the described location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileFingerprint {
    /// The kernel's device-and-inode identity, in the one spelling the domain gives it.
    /// Stability is a question about this exact file, never about which file the graph
    /// will attach it to, so the birth time plays no part here.
    file_id: String,
    size_bytes: u64,
    modified_at: ModificationTime,
}

impl FileFingerprint {
    pub(super) fn of_open_file(metadata: &fs::Metadata) -> Self {
        Self {
            file_id: ObservedFileIdentity::new(metadata.dev(), metadata.ino(), None).base_id(),
            size_bytes: metadata.len(),
            modified_at: ModificationTime::new(metadata.mtime(), metadata.mtime_nsec()),
        }
    }

    pub(super) fn file_id(&self) -> &str {
        &self.file_id
    }

    pub(super) fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub(super) fn modified_at(&self) -> ModificationTime {
        self.modified_at
    }
}

pub(super) fn lstat_at(directory_fd: RawFd, name: &OsStr) -> io::Result<EntryStat> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `name` is NUL-terminated and `stat` points at a writable `libc::stat`; the
    // buffer is only read after fstatat reported success.
    let result = unsafe {
        libc::fstatat(
            directory_fd,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };

    let identity = ObservedFileIdentity::new(
        device_id(stat.st_dev),
        inode_number(stat.st_ino),
        birth_time(directory_fd, &name, &stat),
    );
    Ok(EntryStat {
        kind: kind_of(stat.st_mode),
        fingerprint: FileFingerprint {
            file_id: identity.base_id(),
            size_bytes: u64::try_from(stat.st_size)
                .map_err(|_| io::Error::other("entry reports a negative size"))?,
            modified_at: ModificationTime::new(
                widen_signed(stat.st_mtime),
                widen_signed(stat.st_mtime_nsec),
            ),
        },
        identity,
        observed_at_unix_ms: current_unix_ms()?,
    })
}

/// When the filesystem says this file was created, in whole nanoseconds since the Unix
/// epoch. It is the only thing that tells a recycled inode number from the file that held
/// it, so a value the filesystem does not record is reported absent rather than guessed.
/// macOS carries it in the same `stat` the entry was described by.
#[cfg(target_os = "macos")]
fn birth_time(_directory_fd: RawFd, _name: &CStr, stat: &libc::stat) -> Option<i64> {
    unix_nanoseconds(
        widen_signed(stat.st_birthtime),
        widen_signed(stat.st_birthtime_nsec),
    )
}

/// Linux reports it only through `statx`, so the entry is asked a second time, again
/// relative to its parent descriptor and again without following a symlink. The two calls
/// are not one atomic look: a file replaced between them pairs an old inode number with a
/// new birth time, which resolves to a new identity — the safe direction — and content is
/// still guarded by the fingerprint check at open time.
#[cfg(target_os = "linux")]
fn birth_time(directory_fd: RawFd, name: &CStr, _stat: &libc::stat) -> Option<i64> {
    let mut extended = MaybeUninit::<libc::statx>::uninit();
    // SAFETY: `name` is NUL-terminated and `extended` points at a writable `libc::statx`;
    // the buffer is only read after statx reported success.
    let result = unsafe {
        libc::statx(
            directory_fd,
            name.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW | libc::AT_STATX_SYNC_AS_STAT,
            libc::STATX_BTIME,
            extended.as_mut_ptr(),
        )
    };
    if result != 0 {
        return None;
    }
    let extended = unsafe { extended.assume_init() };
    if extended.stx_mask & libc::STATX_BTIME == 0 {
        return None;
    }
    unix_nanoseconds(
        extended.stx_btime.tv_sec,
        i64::from(extended.stx_btime.tv_nsec),
    )
}

/// Everywhere else the birth time is not available, and the recycling it guards against
/// stays undetectable on that platform.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn birth_time(_directory_fd: RawFd, _name: &CStr, _stat: &libc::stat) -> Option<i64> {
    None
}

/// Whether this platform asks the filesystem for a creation time at all. A filesystem it
/// does ask may still record none, which is why a scan counts the sources it met without
/// one instead of answering the question once for the whole machine.
pub const fn reads_creation_times() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}

/// A filesystem that records no birth time reports zero, which is not a moment any file
/// was created at; so is a time that does not fit in nanoseconds.
fn unix_nanoseconds(seconds: i64, nanoseconds: i64) -> Option<i64> {
    if seconds <= 0 {
        return None;
    }
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(nanoseconds))
}

fn kind_of(mode: libc::mode_t) -> EntryKind {
    match mode & libc::S_IFMT {
        libc::S_IFDIR => EntryKind::Directory,
        libc::S_IFREG => EntryKind::File(FileKind::RegularFile),
        libc::S_IFLNK => EntryKind::File(FileKind::Symlink),
        _ => EntryKind::File(FileKind::Special),
    }
}

/// Widened exactly like `std::os::unix::fs::MetadataExt::dev`, so both stat paths agree.
#[cfg(target_os = "macos")]
fn device_id(device: libc::dev_t) -> u64 {
    device as u64
}

#[cfg(not(target_os = "macos"))]
fn device_id(device: libc::dev_t) -> u64 {
    widen_unsigned(device)
}

fn inode_number(inode: libc::ino_t) -> u64 {
    widen_unsigned(inode)
}

/// libc aliases differ in width per platform; converting through `Into` keeps the code
/// portable without a cast that would be redundant on one platform and lossy on another.
fn widen_unsigned(value: impl Into<u64>) -> u64 {
    value.into()
}

fn widen_signed(value: impl Into<i64>) -> i64 {
    value.into()
}
