// The delivery probe of `index watch`: proves that filesystem change hints arrive for the
// root's filesystem by writing one file under a private directory and waiting for its
// hint. The OS never reports a stream that started but does not deliver, so this is the
// only proof available; it is done to the same standard as the scanner, descriptor-
// relative and never through a path that could have been swapped:
//
// - the service directory is opened as a directory, then `watch-probe` is created and
//   opened relative to it with `O_DIRECTORY | O_NOFOLLOW`, so a symlink planted there is
//   refused instead of followed;
// - the probe file is created relative to that descriptor with `O_CREAT | O_EXCL |
//   O_NOFOLLOW` and mode 0600 under an unpredictable name, so an existing file or a
//   planted symlink is refused and nothing is ever truncated or followed;
// - both directories must be owned by the effective user and writable by nobody else
//   (`watch-probe` exactly 0700), so no other principal can swap what the probe creates;
// - the payload is written to the open descriptor only, and the file is removed with
//   `unlinkat` relative to the trusted directory descriptor on every path after it was
//   created: hint, no hint, or a failed write. Before that `unlinkat`, a descriptor-
//   relative `fstatat` without following symlinks must still show the very regular file
//   the probe created, by device and inode; anything else is left alone and fails the
//   probe, so a swapped file is never deleted.
//
// Before anything is created the placement is checked: a probe inside the watched root
// would create events in the tree being scanned, and a probe on another filesystem says
// nothing about the root's, so both leave delivery unconfirmed without touching the disk.
use std::ffi::OsStr;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::application::{EventDelivery, SubscribeFailure, UnconfirmedReason};
use crate::domain::{FileKind, NormalizedPath};

use super::entry_stat::{EntryKind, FileFingerprint, lstat_at};
use super::unix_walk::{c_name, create_exclusive_at, open_directory_at};

/// How long the probe change may take to come back as a hint. FSEvents answers within
/// milliseconds; a stream silent this long does not start delivering later.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_DIRECTORY: &str = "watch-probe";
const PROBE_PAYLOAD: &[u8] = b"crosskit watch delivery probe\n";

/// The stream under test: the probe directory is registered on it, one hint is waited for,
/// and the directory is unregistered again. Watching happens by path, because a watcher
/// only observes: it never writes or removes anything.
///
/// Registering and waiting are separate calls, and the stream is the caller's, because that
/// is what Stage 36 changed. A probe with a stream of its own proved that *some* stream
/// delivers; only a probe on the stream the roots are on proves that *this* one does — and
/// `notify` ignores what `FSEventStreamStart` returns, so a stream that never started is
/// exactly the failure worth catching.
pub(super) trait HintSource {
    fn watch(&mut self, directory: &Path) -> Result<(), SubscribeFailure>;

    /// Whether any hint arrived on this stream within the wait.
    ///
    /// Any hint: signals carry no path, so a change somebody else made under a watched root
    /// answers this as well as the probe's own file does. That is not a weakness — the
    /// question is whether this stream delivers, and either answer proves it does.
    fn hint_within(&mut self, timeout: Duration) -> bool;

    fn unwatch(&mut self, directory: &Path);
}

/// Where the probe may run, decided before any file is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProbePlacement {
    Allowed,
    InsideRoot,
    DifferentDevice,
}

/// A probe inside any watched root would make the watcher the source of events in a tree it
/// scans; a probe on another device than a root proves nothing about that root's
/// filesystem. With several roots both questions are asked of all of them, and the strictest
/// answer wins: the probe speaks for every root or it is not trusted for any.
pub(super) fn probe_placement(
    roots: &[(NormalizedPath, u64)],
    probe_directory: &NormalizedPath,
    probe_device: u64,
) -> ProbePlacement {
    if roots
        .iter()
        .any(|(root, _)| probe_directory.is_within(root))
    {
        return ProbePlacement::InsideRoot;
    }
    if roots.iter().any(|(_, device)| *device != probe_device) {
        return ProbePlacement::DifferentDevice;
    }
    ProbePlacement::Allowed
}

pub(super) struct DeliveryProbe {
    service_directory: PathBuf,
}

impl DeliveryProbe {
    /// `service_directory` is Crosskit's own data directory; the probe lives in its
    /// `watch-probe` subdirectory.
    pub(super) fn new(service_directory: PathBuf) -> Self {
        Self { service_directory }
    }

    pub(super) fn confirm(
        &self,
        roots: &[NormalizedPath],
        hints: &mut dyn HintSource,
    ) -> Result<EventDelivery, SubscribeFailure> {
        self.confirm_with(roots, &random_leaf_name(), hints, &|file| {
            file.write_all(PROBE_PAYLOAD)
        })
    }

    /// The full lifecycle with the leaf name and the payload write injectable, so the
    /// security invariants can be tested against planted files without a real stream.
    pub(super) fn confirm_with(
        &self,
        roots: &[NormalizedPath],
        leaf_name: &str,
        hints: &mut dyn HintSource,
        write: &dyn Fn(&mut fs::File) -> io::Result<()>,
    ) -> Result<EventDelivery, SubscribeFailure> {
        let service_path = fs::canonicalize(&self.service_directory).map_err(probe_unavailable)?;
        let probe_path = NormalizedPath::from_absolute(&service_path.to_string_lossy())
            .and_then(|service| service.join(PROBE_DIRECTORY))
            .map_err(|_| SubscribeFailure::ProbeUnavailable)?;
        let service = open_directory_at(libc::AT_FDCWD, service_path.as_os_str())
            .map_err(probe_unavailable)?;
        require_private_directory(&service, DirectoryMode::NoOtherWriters)
            .map_err(probe_unavailable)?;
        let service_device = service.metadata().map_err(probe_unavailable)?.dev();
        let mut devices = Vec::with_capacity(roots.len());
        for root in roots {
            let device = fs::metadata(root.as_str())
                .map_err(probe_unavailable)?
                .dev();
            devices.push((root.clone(), device));
        }
        match probe_placement(&devices, &probe_path, service_device) {
            ProbePlacement::InsideRoot => {
                return Ok(EventDelivery::Unconfirmed(
                    UnconfirmedReason::ProbeInsideRoot,
                ));
            }
            ProbePlacement::DifferentDevice => {
                return Ok(EventDelivery::Unconfirmed(
                    UnconfirmedReason::ProbeOnOtherDevice,
                ));
            }
            ProbePlacement::Allowed => {}
        }

        make_directory_at(service.as_raw_fd(), PROBE_DIRECTORY).map_err(probe_unavailable)?;
        let probe_directory = open_directory_at(service.as_raw_fd(), OsStr::new(PROBE_DIRECTORY))
            .map_err(probe_unavailable)?;
        require_private_directory(&probe_directory, DirectoryMode::Exactly(0o700))
            .map_err(probe_unavailable)?;
        // A mount planted at `watch-probe` would again prove nothing about the root.
        if probe_directory.metadata().map_err(probe_unavailable)?.dev() != service_device {
            return Ok(EventDelivery::Unconfirmed(
                UnconfirmedReason::ProbeOnOtherDevice,
            ));
        }

        let watched_path = Path::new(probe_path.as_str());
        hints.watch(watched_path)?;
        let leaf = create_exclusive_at(probe_directory.as_raw_fd(), OsStr::new(leaf_name), 0o600)
            .map_err(probe_unavailable);
        let observed = observe(hints, leaf, write, probe_directory.as_raw_fd(), leaf_name);
        // Unregistered whatever happened: the probe directory is not part of what this
        // watch is about, and a hint from it afterwards would start a pass for a file
        // nobody indexed.
        hints.unwatch(watched_path);
        let observed = observed?;

        match observed {
            true => Ok(EventDelivery::Confirmed),
            false => Ok(EventDelivery::Unconfirmed(UnconfirmedReason::NoHint)),
        }
    }
}

/// Creates the probe file, writes it, waits for a hint and removes it again. Apart from
/// `confirm_with` so that the removal and the unregistering happen on every path out,
/// including the ones where the file was never created.
fn observe(
    hints: &mut dyn HintSource,
    leaf: Result<fs::File, SubscribeFailure>,
    write: &dyn Fn(&mut fs::File) -> io::Result<()>,
    directory_fd: RawFd,
    leaf_name: &str,
) -> Result<bool, SubscribeFailure> {
    let mut leaf = leaf?;
    let created = identity_of_created_leaf(&leaf).map_err(probe_unavailable)?;
    // From here on the file exists and must go away on every path, but only that file: the
    // removal below re-checks that the name still means it.
    let observed = write(&mut leaf).map(|()| hints.hint_within(PROBE_TIMEOUT));
    drop(leaf);
    remove_created_leaf(directory_fd, leaf_name, &created)?;
    observed.map_err(probe_unavailable)
}

/// Only the category leaves: an I/O error's text may name a path.
fn probe_unavailable(_error: io::Error) -> SubscribeFailure {
    SubscribeFailure::ProbeUnavailable
}

/// What the probe demands of a directory's permission bits.
#[derive(Debug, Clone, Copy)]
enum DirectoryMode {
    /// Group and others may not write; the owner's bits are the owner's business.
    NoOtherWriters,
    /// Exactly these bits: the directory is the probe's own and private.
    Exactly(u32),
}

/// A directory the probe may trust: owned by this process's effective user and writable
/// by nobody else, read through the open descriptor so the check and the use agree.
fn require_private_directory(directory: &fs::File, mode: DirectoryMode) -> io::Result<()> {
    let metadata = directory.metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other("probe location is not a directory"));
    }
    // SAFETY: geteuid takes no arguments, has no preconditions and cannot fail.
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::other(
            "probe directory is not owned by the current user",
        ));
    }
    let permissions = metadata.mode() & 0o777;
    let private = match mode {
        DirectoryMode::NoOtherWriters => permissions & 0o022 == 0,
        DirectoryMode::Exactly(required) => permissions == required,
    };
    if !private {
        return Err(io::Error::other("probe directory admits other writers"));
    }
    Ok(())
}

/// Device and inode of the file the probe just created, taken from its own descriptor.
fn identity_of_created_leaf(leaf: &fs::File) -> io::Result<String> {
    let metadata = leaf.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("probe leaf is not a regular file"));
    }
    Ok(FileFingerprint::of_open_file(&metadata)
        .file_id()
        .to_string())
}

/// Removes the leaf only while its name still denotes the very regular file the probe
/// created, checked relative to the trusted directory descriptor without following
/// symlinks. Anything else is left in place and fails the probe: a file swapped in
/// under the same name is never deleted.
fn remove_created_leaf(
    directory_fd: RawFd,
    name: &str,
    created: &str,
) -> Result<(), SubscribeFailure> {
    let current = lstat_at(directory_fd, OsStr::new(name)).map_err(probe_unavailable)?;
    let same_file = current.kind == EntryKind::File(FileKind::RegularFile)
        && current.fingerprint.file_id() == created;
    if !same_file {
        return Err(SubscribeFailure::ProbeUnavailable);
    }
    unlink_at(directory_fd, name).map_err(probe_unavailable)
}

/// An unpredictable name, so nobody can plant a file where the probe will be created; the
/// exclusive create is what refuses a collision, the name only makes one unlikely.
fn random_leaf_name() -> String {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos()),
    );
    format!("probe-{:016x}", hasher.finish())
}

/// Creates the directory relative to `parent_fd` with mode 0700; an existing entry is
/// left alone and is verified by the caller's `O_DIRECTORY | O_NOFOLLOW` open.
fn make_directory_at(parent_fd: RawFd, name: &str) -> io::Result<()> {
    let name = c_name(OsStr::new(name))?;
    // SAFETY: `name` is NUL-terminated and `parent_fd` is an open directory descriptor.
    if unsafe { libc::mkdirat(parent_fd, name.as_ptr(), 0o700) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::AlreadyExists {
        return Ok(());
    }
    Err(error)
}

fn unlink_at(parent_fd: RawFd, name: &str) -> io::Result<()> {
    let name = c_name(OsStr::new(name))?;
    // SAFETY: `name` is NUL-terminated and `parent_fd` is an open directory descriptor.
    if unsafe { libc::unlinkat(parent_fd, name.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::{DeliveryProbe, HintSource, PROBE_DIRECTORY, ProbePlacement, probe_placement};
    use crate::application::{EventDelivery, SubscribeFailure, UnconfirmedReason};
    use crate::domain::NormalizedPath;
    use crate::infrastructure::index_pipeline_tests::test_directory;

    const LEAF: &str = "probe-fixed-for-tests";
    const SENTINEL: &[u8] = b"keep me exactly as I am\n";

    /// A stream that never touches the disk and answers as scripted. It records whether
    /// the probe directory was unregistered again, because leaving it registered would
    /// turn every later change in the data directory into a pass over the context.
    struct ScriptedHints {
        hint: bool,
        watched: bool,
    }

    impl ScriptedHints {
        fn new(hint: bool) -> Self {
            Self {
                hint,
                watched: false,
            }
        }
    }

    impl HintSource for ScriptedHints {
        fn watch(&mut self, _directory: &Path) -> Result<(), SubscribeFailure> {
            self.watched = true;
            Ok(())
        }

        fn hint_within(&mut self, _timeout: Duration) -> bool {
            self.hint
        }

        fn unwatch(&mut self, _directory: &Path) {
            self.watched = false;
        }
    }

    fn real_write(file: &mut fs::File) -> io::Result<()> {
        io::Write::write_all(file, b"probe payload\n")
    }

    fn failing_write(_file: &mut fs::File) -> io::Result<()> {
        Err(io::Error::other("injected write failure"))
    }

    fn canonical(path: &Path) -> NormalizedPath {
        NormalizedPath::from_absolute(&fs::canonicalize(path).unwrap().to_string_lossy()).unwrap()
    }

    /// A watched root and a separate service directory, both on the same filesystem.
    struct Fixture {
        root_dir: PathBuf,
        root: NormalizedPath,
        service: PathBuf,
        external: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root_dir = test_directory(&format!("probe-{name}-root"));
            let service = test_directory(&format!("probe-{name}-service"));
            let external = test_directory(&format!("probe-{name}-external"));
            Self {
                root: canonical(&root_dir),
                root_dir,
                service,
                external,
            }
        }

        fn probe_directory(&self) -> PathBuf {
            self.service.join(PROBE_DIRECTORY)
        }

        /// The directory exactly as production creates it: private to the owner.
        fn create_private_probe_directory(&self) {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(self.probe_directory())
                .unwrap();
        }

        fn leaf_path(&self) -> PathBuf {
            self.probe_directory().join(LEAF)
        }

        fn confirm(
            &self,
            hint: bool,
            write: &dyn Fn(&mut fs::File) -> io::Result<()>,
        ) -> Result<EventDelivery, SubscribeFailure> {
            DeliveryProbe::new(self.service.clone()).confirm_with(
                std::slice::from_ref(&self.root),
                LEAF,
                &mut ScriptedHints::new(hint),
                write,
            )
        }

        fn entries(&self, directory: &Path) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for directory in [&self.root_dir, &self.service, &self.external] {
                fs::remove_dir_all(directory).unwrap();
            }
        }
    }

    fn path(path: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(path).unwrap()
    }

    #[test]
    fn placement_allows_only_a_probe_outside_every_root_on_their_device() {
        let roots = [(path("/data/docs"), 7)];
        let elsewhere = path("/var/crosskit/watch-probe");
        let inside = path("/data/docs/.crosskit/watch-probe");

        assert_eq!(
            probe_placement(&roots, &elsewhere, 7),
            ProbePlacement::Allowed
        );
        assert_eq!(
            probe_placement(&roots, &elsewhere, 8),
            ProbePlacement::DifferentDevice
        );
        // Inside the root is refused even on the same device, and before the device check.
        assert_eq!(
            probe_placement(&roots, &inside, 7),
            ProbePlacement::InsideRoot
        );
        assert_eq!(
            probe_placement(&roots, &inside, 8),
            ProbePlacement::InsideRoot
        );
    }

    /// With several roots the strictest answer wins: one probe speaks for every root of the
    /// context or it is not trusted for any of them.
    #[test]
    fn one_root_on_another_device_or_holding_the_probe_refuses_the_whole_placement() {
        let probe = path("/var/crosskit/watch-probe");
        let same_device = [(path("/data/one"), 7), (path("/data/two"), 7)];
        assert_eq!(
            probe_placement(&same_device, &probe, 7),
            ProbePlacement::Allowed
        );

        let other_device = [(path("/data/one"), 7), (path("/volume/two"), 8)];
        assert_eq!(
            probe_placement(&other_device, &probe, 7),
            ProbePlacement::DifferentDevice,
            "a probe cannot vouch for a root on another filesystem"
        );

        let holding_the_probe = [(path("/data/one"), 7), (path("/var/crosskit"), 7)];
        assert_eq!(
            probe_placement(&holding_the_probe, &probe, 7),
            ProbePlacement::InsideRoot,
            "the watcher must not make events in any tree it scans"
        );
    }

    /// What Stage 36 changed, checked on the two answers that matter: the probe registers
    /// the probe directory on the stream it was handed, and a stream that delivers nothing
    /// is told apart from one that delivers.
    ///
    /// A stream that was created and never started is exactly this second case — `notify`
    /// ignores what `FSEventStreamStart` returns — and before Stage 36 the probe made a
    /// stream of its own, so a dead root stream passed it.
    #[test]
    fn a_stream_that_delivers_is_told_apart_from_one_that_does_not() {
        let fixture = Fixture::new("stream-alive");

        let mut alive = ScriptedHints::new(true);
        assert_eq!(
            DeliveryProbe::new(fixture.service.clone()).confirm_with(
                std::slice::from_ref(&fixture.root),
                LEAF,
                &mut alive,
                &real_write,
            ),
            Ok(EventDelivery::Confirmed)
        );

        let mut silent = ScriptedHints::new(false);
        assert_eq!(
            DeliveryProbe::new(fixture.service.clone()).confirm_with(
                std::slice::from_ref(&fixture.root),
                LEAF,
                &mut silent,
                &real_write,
            ),
            Ok(EventDelivery::Unconfirmed(UnconfirmedReason::NoHint))
        );
    }

    /// A signal that is not a hint must not be eaten by the probe.
    ///
    /// The probe reads the roots' own queue, so a stop requested while it runs arrives
    /// there. The adapter puts it back; this checks the half that belongs here — the probe
    /// treats "not a hint" as "no hint" and does not go on waiting for one.
    #[test]
    fn a_signal_that_is_not_a_hint_ends_the_wait_without_confirming() {
        let fixture = Fixture::new("stream-other-signal");
        let mut hints = ScriptedHints::new(false);

        assert_eq!(
            DeliveryProbe::new(fixture.service.clone()).confirm_with(
                std::slice::from_ref(&fixture.root),
                LEAF,
                &mut hints,
                &real_write,
            ),
            Ok(EventDelivery::Unconfirmed(UnconfirmedReason::NoHint))
        );
    }

    /// The probe directory is unregistered again whatever the answer was. Left registered,
    /// every later change in the data directory would start a pass over the context for a
    /// file nobody indexes.
    #[test]
    fn the_probe_directory_does_not_stay_on_the_stream() {
        let fixture = Fixture::new("stream-unwatched");

        for hint in [true, false] {
            let mut hints = ScriptedHints::new(hint);
            let _ = DeliveryProbe::new(fixture.service.clone()).confirm_with(
                std::slice::from_ref(&fixture.root),
                LEAF,
                &mut hints,
                &real_write,
            );
            assert!(
                !hints.watched,
                "the probe directory must be unregistered after a probe that answered {hint}"
            );
        }
    }

    #[test]
    fn a_symlink_in_place_of_the_probe_directory_is_refused_and_its_target_untouched() {
        let fixture = Fixture::new("dir-symlink");
        fs::write(fixture.external.join("sentinel.txt"), SENTINEL).unwrap();
        symlink(&fixture.external, fixture.probe_directory()).unwrap();

        let outcome = fixture.confirm(true, &real_write);

        assert_eq!(outcome, Err(SubscribeFailure::ProbeUnavailable));
        assert_eq!(fixture.entries(&fixture.external), ["sentinel.txt"]);
        assert_eq!(
            fs::read(fixture.external.join("sentinel.txt")).unwrap(),
            SENTINEL
        );
        assert!(
            fs::symlink_metadata(fixture.probe_directory())
                .unwrap()
                .is_symlink()
        );
    }

    #[test]
    fn a_symlink_at_the_leaf_is_refused_and_its_target_is_neither_overwritten_nor_removed() {
        let fixture = Fixture::new("leaf-symlink");
        let sentinel = fixture.external.join("sentinel.txt");
        fs::write(&sentinel, SENTINEL).unwrap();
        fixture.create_private_probe_directory();
        symlink(&sentinel, fixture.leaf_path()).unwrap();

        let outcome = fixture.confirm(true, &real_write);

        assert_eq!(outcome, Err(SubscribeFailure::ProbeUnavailable));
        assert_eq!(fs::read(&sentinel).unwrap(), SENTINEL);
        assert!(
            fs::symlink_metadata(fixture.probe_directory().join(LEAF))
                .unwrap()
                .is_symlink(),
            "the planted symlink must be left in place, not removed"
        );
    }

    #[test]
    fn an_existing_leaf_is_never_truncated() {
        let fixture = Fixture::new("leaf-exists");
        fixture.create_private_probe_directory();
        let planted = fixture.leaf_path();
        fs::write(&planted, SENTINEL).unwrap();

        let outcome = fixture.confirm(true, &real_write);

        assert_eq!(outcome, Err(SubscribeFailure::ProbeUnavailable));
        assert_eq!(fs::read(&planted).unwrap(), SENTINEL);
        assert_eq!(fixture.entries(&fixture.probe_directory()), [LEAF]);
    }

    #[test]
    fn the_probe_file_is_gone_after_a_hint_no_hint_and_a_failed_write() {
        let fixture = Fixture::new("cleanup");

        assert_eq!(
            fixture.confirm(true, &real_write),
            Ok(EventDelivery::Confirmed)
        );
        assert!(fixture.entries(&fixture.probe_directory()).is_empty());

        assert_eq!(
            fixture.confirm(false, &real_write),
            Ok(EventDelivery::Unconfirmed(UnconfirmedReason::NoHint))
        );
        assert!(fixture.entries(&fixture.probe_directory()).is_empty());

        assert_eq!(
            fixture.confirm(true, &failing_write),
            Err(SubscribeFailure::ProbeUnavailable)
        );
        assert!(fixture.entries(&fixture.probe_directory()).is_empty());
    }

    #[test]
    fn a_probe_inside_the_root_is_refused_before_anything_is_created() {
        let root_dir = test_directory("probe-inside-root");
        let service = root_dir.join("service");
        fs::create_dir(&service).unwrap();
        let root = canonical(&root_dir);

        let outcome = DeliveryProbe::new(service.clone()).confirm_with(
            std::slice::from_ref(&root),
            LEAF,
            &mut ScriptedHints::new(true),
            &real_write,
        );

        assert_eq!(
            outcome,
            Ok(EventDelivery::Unconfirmed(
                UnconfirmedReason::ProbeInsideRoot
            ))
        );
        assert!(
            !service.join(PROBE_DIRECTORY).exists(),
            "no probe directory may appear inside the root"
        );
        fs::remove_dir_all(&root_dir).unwrap();
    }

    #[test]
    fn a_probe_directory_writable_by_others_is_refused_before_any_file_is_created() {
        let fixture = Fixture::new("dir-world-writable");
        fs::create_dir(fixture.probe_directory()).unwrap();
        fs::set_permissions(fixture.probe_directory(), fs::Permissions::from_mode(0o777)).unwrap();

        let outcome = fixture.confirm(true, &real_write);

        assert_eq!(outcome, Err(SubscribeFailure::ProbeUnavailable));
        assert!(fixture.entries(&fixture.probe_directory()).is_empty());
    }

    #[test]
    fn a_service_directory_writable_by_others_is_refused_before_the_probe_directory_exists() {
        let fixture = Fixture::new("service-group-writable");
        fs::set_permissions(&fixture.service, fs::Permissions::from_mode(0o775)).unwrap();

        let outcome = fixture.confirm(true, &real_write);

        assert_eq!(outcome, Err(SubscribeFailure::ProbeUnavailable));
        assert!(fixture.entries(&fixture.service).is_empty());
    }

    /// The leaf is replaced between its creation and its removal, as another writer with
    /// access to the directory could do: the probe must notice and delete nothing.
    #[test]
    fn a_leaf_swapped_during_the_write_is_neither_removed_nor_followed() {
        let fixture = Fixture::new("leaf-swapped");
        fixture.create_private_probe_directory();
        let sentinel = fixture.external.join("sentinel.txt");
        fs::write(&sentinel, SENTINEL).unwrap();
        let leaf_path = fixture.leaf_path();

        let swap_for_symlink = |_leaf: &mut fs::File| -> io::Result<()> {
            fs::remove_file(&leaf_path)?;
            symlink(&sentinel, &leaf_path)
        };
        let outcome = fixture.confirm(true, &swap_for_symlink);
        assert_eq!(outcome, Err(SubscribeFailure::ProbeUnavailable));
        assert_eq!(fs::read(&sentinel).unwrap(), SENTINEL);
        assert!(
            fs::symlink_metadata(&leaf_path).unwrap().is_symlink(),
            "the swapped-in symlink must be left in place"
        );
        fs::remove_file(&leaf_path).unwrap();

        let swap_for_file = |_leaf: &mut fs::File| -> io::Result<()> {
            fs::remove_file(&leaf_path)?;
            fs::write(&leaf_path, SENTINEL)
        };
        let outcome = fixture.confirm(true, &swap_for_file);
        assert_eq!(outcome, Err(SubscribeFailure::ProbeUnavailable));
        assert_eq!(
            fs::read(&leaf_path).unwrap(),
            SENTINEL,
            "a different file under the probe's name must survive untouched"
        );
        assert_eq!(fs::read(&sentinel).unwrap(), SENTINEL);
    }
}
