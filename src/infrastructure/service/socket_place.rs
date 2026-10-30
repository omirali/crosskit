// Where the service's socket lives, held to the standard every file this program creates is
// held to: made and removed relative to a directory descriptor that was checked, never by a
// path that could have been swapped between the check and the use.
//
// - `service/` is opened with `O_DIRECTORY | O_NOFOLLOW`, must be owned by the effective user,
//   and is made exactly 0700 — after which nobody but this user can put a name into it;
// - the socket's name is looked at with `fstatat(…, AT_SYMLINK_NOFOLLOW)`: a symbolic link or
//   anything that is not a socket of this user is refused and left exactly as it is;
// - a socket of this user found there is the one a previous service left when it was killed
//   — the instance lock proves no other service runs — and only that is removed;
// - the socket is bound to its bare name with the process's working directory set to the
//   checked descriptor (`fchdir`), and put back at once: macOS has no `bindat`, and the bare
//   name is also what keeps the address under the 104 bytes a socket address may hold;
// - on the way out it is removed only while the name still denotes the socket bound here, by
//   device and inode.
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Mutex;

use super::super::filesystem::{c_name, open_directory_at};

/// The socket's name inside `service/`.
pub const SOCKET_NAME: &str = "crosskit.sock";

/// Why the socket was not put in place. The service keeps watching either way; only reads
/// through it are unavailable, and readers go the way they went before.
#[derive(Debug)]
pub enum SocketRefusal {
    /// `service/` is not a directory of this user, or could not be made private.
    UntrustedDirectory(String),
    /// Something that is not this user's socket stands at the socket's name.
    Occupied,
    Io(io::Error),
}

impl std::fmt::Display for SocketRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UntrustedDirectory(why) => write!(f, "the service directory is not trusted: {why}"),
            Self::Occupied => f.write_str(
                "something that is not this user's socket stands at the socket's name; it was left alone",
            ),
            Self::Io(error) => write!(f, "the socket could not be put in place: {error}"),
        }
    }
}

/// The listening socket, with what is needed to remove exactly it again.
pub struct PlacedSocket {
    listener: UnixListener,
    directory: fs::File,
    identity: (u64, u64),
}

impl PlacedSocket {
    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }

    /// Removes the socket, only while its name still denotes the socket bound here.
    pub fn remove(self) {
        let still_ours = stat_at(self.directory.as_raw_fd(), SOCKET_NAME)
            .is_ok_and(|stat| identity_of(&stat) == self.identity);
        if still_ours {
            // What failed here is a name left behind, which the next start removes.
            let _ = unlink_at(self.directory.as_raw_fd(), SOCKET_NAME);
        }
    }
}

/// Puts the listening socket in `service_directory`. Called by the service before any thread
/// of its own that could care about the working directory exists.
pub fn place_socket(service_directory: &Path) -> Result<PlacedSocket, SocketRefusal> {
    let directory = trusted_directory(service_directory, true)?;
    match stat_at(directory.as_raw_fd(), SOCKET_NAME) {
        Ok(stat) if is_own_socket(&stat) => {
            unlink_at(directory.as_raw_fd(), SOCKET_NAME).map_err(SocketRefusal::Io)?;
        }
        Ok(_) => return Err(SocketRefusal::Occupied),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(SocketRefusal::Io(error)),
    }
    let listener =
        in_directory(&directory, || UnixListener::bind(SOCKET_NAME)).map_err(SocketRefusal::Io)?;
    let stat = stat_at(directory.as_raw_fd(), SOCKET_NAME).map_err(SocketRefusal::Io)?;
    if !is_own_socket(&stat) {
        return Err(SocketRefusal::Occupied);
    }
    // The directory is already 0700; the socket is made the same, so nothing depends on the
    // umask the service was started with.
    let name = c_name(OsStr::new(SOCKET_NAME)).map_err(SocketRefusal::Io)?;
    // SAFETY: `name` is NUL-terminated and the descriptor is an open directory.
    if unsafe {
        libc::fchmodat(
            directory.as_raw_fd(),
            name.as_ptr(),
            0o600,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(SocketRefusal::Io(io::Error::last_os_error()));
    }
    Ok(PlacedSocket {
        listener,
        identity: identity_of(&stat),
        directory,
    })
}

/// Connects to the service's socket, for a client. `Err` of kind `NotFound` is the ordinary
/// answer when no service runs.
///
/// The directory is held to the same check as the service's, and the server on the other end
/// must be this user: a socket somebody else bound would otherwise be handed the question.
pub fn connect_socket(service_directory: &Path) -> io::Result<UnixStream> {
    let directory =
        trusted_directory(service_directory, false).map_err(|refusal| match refusal {
            SocketRefusal::Io(error) => error,
            other => io::Error::other(other.to_string()),
        })?;
    let stream = in_directory(&directory, || UnixStream::connect(SOCKET_NAME))?;
    if peer_uid(&stream)? != effective_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the socket is served by another user",
        ));
    }
    Ok(stream)
}

/// The uid of the process at the other end, which a server checks too.
pub fn peer_uid(stream: &UnixStream) -> io::Result<libc::uid_t> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: the descriptor is a connected Unix socket and both pointers are valid.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

pub fn effective_uid() -> libc::uid_t {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// `service/`, opened without following a link, owned by this user and private. The service
/// makes it private when it is not; a client only checks.
fn trusted_directory(path: &Path, make_private: bool) -> Result<fs::File, SocketRefusal> {
    if make_private && !path.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(SocketRefusal::Io)?;
    }
    let directory =
        open_directory_at(libc::AT_FDCWD, path.as_os_str()).map_err(SocketRefusal::Io)?;
    let metadata = directory.metadata().map_err(SocketRefusal::Io)?;
    if metadata.uid() != effective_uid() {
        return Err(SocketRefusal::UntrustedDirectory(
            "it is owned by another user".to_string(),
        ));
    }
    if metadata.mode() & 0o777 != 0o700 {
        if !make_private {
            return Err(SocketRefusal::UntrustedDirectory(
                "others may write to it".to_string(),
            ));
        }
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(SocketRefusal::Io)?;
    }
    Ok(directory)
}

/// Runs `operation` with the working directory set to `directory`, and puts it back.
///
/// The working directory belongs to the whole process. Every change of it here is taken under
/// one lock, so two of these never interleave — found the hard way: tests running in parallel
/// bound and connected each other's sockets. Nothing else in this program reads the working
/// directory while the service or a client runs, which is what makes the window harmless.
fn in_directory<T>(
    directory: &fs::File,
    operation: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    static WORKING_DIRECTORY: Mutex<()> = Mutex::new(());
    let _held = WORKING_DIRECTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = fs::File::open(".")?;
    // SAFETY: both descriptors are open directories.
    if unsafe { libc::fchdir(directory.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let outcome = operation();
    // SAFETY: as above.
    if unsafe { libc::fchdir(previous.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    outcome
}

fn stat_at(directory: RawFd, name: &str) -> io::Result<libc::stat> {
    let name = c_name(OsStr::new(name))?;
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `name` is NUL-terminated, the descriptor is an open directory and `stat` is a
    // valid out-pointer that fstatat fills completely on success.
    if unsafe {
        libc::fstatat(
            directory,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat returned 0, so the struct is initialized.
    Ok(unsafe { stat.assume_init() })
}

fn unlink_at(directory: RawFd, name: &str) -> io::Result<()> {
    let name = c_name(OsStr::new(name))?;
    // SAFETY: `name` is NUL-terminated and the descriptor is an open directory.
    if unsafe { libc::unlinkat(directory, name.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[allow(
    clippy::unnecessary_cast,
    reason = "st_mode is u16 on macOS and u32 on Linux; S_IFMT and S_IFSOCK follow it"
)]
fn is_own_socket(stat: &libc::stat) -> bool {
    (stat.st_mode as u32 & libc::S_IFMT as u32) == libc::S_IFSOCK as u32
        && stat.st_uid == effective_uid()
}

#[allow(
    clippy::unnecessary_cast,
    reason = "st_dev and st_ino differ in width between platforms"
)]
fn identity_of(stat: &libc::stat) -> (u64, u64) {
    (stat.st_dev as u64, stat.st_ino as u64)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::{SOCKET_NAME, SocketRefusal, connect_socket, place_socket};

    fn directory(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("crosskit-sock-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root.join("service")
    }

    /// A long temporary directory is the point: its socket address would not fit the 104
    /// bytes of a path-based bind.
    #[test]
    fn the_socket_is_made_private_answers_and_is_removed_when_done() {
        let service = directory("placed-with-a-name-long-enough-to-overflow-a-socket-address");
        let placed = place_socket(&service).unwrap();
        assert!(service.join(SOCKET_NAME).as_os_str().len() > 104);
        assert_eq!(
            fs::metadata(&service).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(connect_socket(&service).is_ok());

        placed.remove();
        assert!(!service.join(SOCKET_NAME).exists());
        assert!(connect_socket(&service).is_err());
        fs::remove_dir_all(service.parent().unwrap()).unwrap();
    }

    /// A link planted at the socket's name is refused, and neither it nor what it points at
    /// is touched.
    #[test]
    fn a_planted_link_is_refused_and_left_alone() {
        let service = directory("planted");
        fs::create_dir_all(&service).unwrap();
        fs::set_permissions(&service, fs::Permissions::from_mode(0o700)).unwrap();
        let target = service.parent().unwrap().join("precious");
        fs::write(&target, "keep me").unwrap();
        symlink(&target, service.join(SOCKET_NAME)).unwrap();

        assert!(matches!(
            place_socket(&service),
            Err(SocketRefusal::Occupied)
        ));
        assert!(
            fs::symlink_metadata(service.join(SOCKET_NAME))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
        fs::remove_dir_all(service.parent().unwrap()).unwrap();
    }

    /// A plain file at the name is not a socket of this service's: refused, kept.
    #[test]
    fn a_file_at_the_name_is_refused_and_kept() {
        let service = directory("file");
        fs::create_dir_all(&service).unwrap();
        fs::write(service.join(SOCKET_NAME), "not a socket").unwrap();
        assert!(matches!(
            place_socket(&service),
            Err(SocketRefusal::Occupied)
        ));
        assert_eq!(
            fs::read_to_string(service.join(SOCKET_NAME)).unwrap(),
            "not a socket"
        );
        fs::remove_dir_all(service.parent().unwrap()).unwrap();
    }

    /// A socket a killed service left behind is the one thing at that name that is removed.
    #[test]
    fn a_socket_a_killed_service_left_is_replaced() {
        let service = directory("stale");
        let first = place_socket(&service).unwrap();
        std::mem::forget(first);
        let second = place_socket(&service).unwrap();
        assert!(connect_socket(&service).is_ok());
        second.remove();
        fs::remove_dir_all(service.parent().unwrap()).unwrap();
    }

    /// A client does not talk through a directory others may write to.
    #[test]
    fn a_client_refuses_a_directory_others_may_write() {
        let service = directory("open");
        let placed = place_socket(&service).unwrap();
        fs::set_permissions(&service, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(connect_socket(&service).is_err());
        placed.remove();
        fs::remove_dir_all(service.parent().unwrap()).unwrap();
    }
}
