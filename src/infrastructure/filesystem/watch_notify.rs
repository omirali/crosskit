// Filesystem change hints for `index watch`, backed by notify's recommended watcher
// (FSEvents on macOS). An event becomes `Changed`, and its paths go into the subscription's
// set of changes, which says where the next pass should look (ADR 0050) — in memory only:
// never printed, logged or stored, because an event proves nothing about a file. A notice
// that events were lost becomes `EventsLost`, which costs a whole pass. Errors leave only as
// closed categories: a notify error may carry a path of its own.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::application::{
    ChangedPaths, EventDelivery, RootWatch, SubscribeFailure, UnconfirmedReason, WatchError,
    WatchEventSource, WatchSignal, WatchSubscription,
};
use crate::domain::NormalizedPath;
use crate::infrastructure::app_directories;

/// How long a subscription may take to register its roots and prove delivery before the
/// watch gives up on events and starts on the periodic pass alone.
///
/// Stage W measured the start at half a millisecond on a quiet machine and 10–16 s from a
/// binary whose directory held 867 000 entries, plus up to 3 s of probe wait. Thirty seconds
/// lets that slow-but-working start still win, and is short enough that a start which never
/// returns costs a watcher half a minute instead of the rest of its life.
pub const START_LIMIT: Duration = Duration::from_secs(30);

/// One queue of signals for one watch. The filesystem feeds it `Changed` and
/// `StreamError`; the paired `StopHandle` feeds it `Stopped`, so the loop waiting on the
/// subscription wakes for either. A source subscribes at most once.
pub struct NotifyWatchSource {
    sender: Sender<WatchSignal>,
    receiver: Mutex<Option<Receiver<WatchSignal>>>,
    /// Crosskit's own data directory, where the delivery probe may write. `None` means
    /// delivery cannot be confirmed, so every watch from this source starts degraded.
    service_directory: Option<PathBuf>,
    start_limit: Duration,
}

/// Queues a stop request. Safe to call from a signal-handler thread: it never blocks and
/// touches neither the database nor the console.
#[derive(Clone)]
pub struct StopHandle {
    sender: Sender<WatchSignal>,
}

impl StopHandle {
    pub fn request_stop(&self) {
        queue(&self.sender, WatchSignal::Stopped);
    }
}

/// Queues a signal for the loop and reports whether a loop was still listening. A
/// disconnected receiver means the subscription has already ended, which is not an
/// error: there is simply no loop left to inform, so callers may ignore the answer.
fn queue(sender: &Sender<WatchSignal>, signal: WatchSignal) -> bool {
    sender.send(signal).is_ok()
}

impl NotifyWatchSource {
    pub fn new(service_directory: Option<PathBuf>) -> (Self, StopHandle) {
        let (sender, receiver) = mpsc::channel();
        let stop = StopHandle {
            sender: sender.clone(),
        };
        let source = Self {
            sender,
            receiver: Mutex::new(Some(receiver)),
            service_directory,
            start_limit: START_LIMIT,
        };
        (source, stop)
    }

    /// The same source with another start limit, for a test that cannot wait thirty seconds
    /// to see what a start that never answers does.
    #[cfg(test)]
    pub(crate) fn with_start_limit(mut self, start_limit: Duration) -> Self {
        self.start_limit = start_limit;
        self
    }
}

/// The data directory next to the graph database, where the probe's `watch-probe` lives.
pub fn default_probe_service_directory() -> Result<PathBuf, String> {
    app_directories::data_directory().map_err(|error| error.to_string())
}

impl WatchEventSource for NotifyWatchSource {
    /// One watcher for every root, which on macOS is one FSEvents stream over all of them.
    ///
    /// Measured before it was chosen, on the owner's ten roots: one watcher costs one
    /// background thread and one run loop whatever the number of paths, ten watchers cost
    /// ten of each. `notify`'s FSEvents backend keeps one `CFMutableArray` of paths and
    /// rebuilds the single stream on every `watch` call, so adding a root stops and
    /// restarts that one stream — which is also why every root is registered here, before
    /// the first pass, and never later.
    ///
    /// A root the backend refuses does not refuse the subscription. `append_path` fails
    /// before the path reaches the array, so the stream restarts with the roots that did
    /// register and they keep delivering; the refused root is reported and still walked by
    /// every pass. Only a subscription with nothing left to watch is a failure.
    ///
    /// The start is bounded. Registering and probing run on a thread of their own, and a
    /// start that has not answered within the limit leaves the watch to the periodic pass:
    /// nothing in the OS bounds `FSEventStreamStart`, and a watcher that waits on it forever
    /// is a context nobody keeps current — with the reason nowhere but in its silence.
    fn subscribe(
        &self,
        roots: &[NormalizedPath],
    ) -> Result<Box<dyn WatchSubscription>, WatchError> {
        let receiver = self
            .receiver
            .lock()
            .map_err(|_| WatchError::Subscribe(SubscribeFailure::SignalQueuePoisoned))?
            .take()
            .ok_or(WatchError::Subscribe(SubscribeFailure::AlreadySubscribed))?;

        let (answer, started) = mpsc::sync_channel(1);
        let sender = self.sender.clone();
        let service_directory = self.service_directory.clone();
        let wanted = roots.to_vec();
        thread::Builder::new()
            .name("crosskit-watch-start".to_string())
            .spawn(move || {
                let stream = start_stream(sender, &wanted, service_directory.as_deref());
                // Nobody is listening once the watch gave up waiting. The stream is then
                // dropped here, which stops it: a late start must not begin delivering
                // hints to a watch that already said it runs without them.
                if let Err(late) = answer.send(stream) {
                    drop(late);
                }
            })
            .map_err(|_| WatchError::Subscribe(SubscribeFailure::BackendUnavailable))?;

        let stream = match started.recv_timeout(self.start_limit) {
            Ok(stream) => stream?,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                return Ok(Box::new(NotifyWatchSubscription {
                    _watcher: None,
                    changes: None,
                    receiver,
                    _sender: self.sender.clone(),
                    delivery: EventDelivery::Unconfirmed(UnconfirmedReason::StartTimedOut),
                    roots: roots
                        .iter()
                        .map(|root| {
                            RootWatch::refused(root.clone(), SubscribeFailure::StartTimedOut)
                        })
                        .collect(),
                }));
            }
        };
        // Everything that arrived while the stream started — the probe's own file among it —
        // is covered by the bootstrap pass that follows at once. Left in the queue, it would
        // start a second full pass right behind the first for nothing.
        drain_hints(&receiver, &self.sender);
        forget_changes(&stream.changes);
        Ok(Box::new(NotifyWatchSubscription {
            _watcher: stream.watcher,
            changes: Some(stream.changes),
            receiver,
            _sender: self.sender.clone(),
            delivery: stream.delivery,
            roots: stream.roots,
        }))
    }
}

/// What the start thread hands back: the live watcher, what became of each root and what the
/// probe proved.
struct StartedStream {
    /// `None` when no root registered: there is nothing to keep alive.
    watcher: Option<RecommendedWatcher>,
    roots: Vec<RootWatch>,
    delivery: EventDelivery,
    changes: Changes,
}

/// Where the hints since the last pass said something changed, shared with the callback.
type Changes = Arc<Mutex<ChangedPaths>>;

/// The changes the start itself produced — the probe's file, anything written while the
/// roots registered — are covered by the bootstrap pass, which is whole.
fn forget_changes(changes: &Changes) {
    if let Ok(mut changes) = changes.lock() {
        *changes = ChangedPaths::default();
    }
}

/// Creates the watcher, registers every root and probes delivery on that same stream.
fn start_stream(
    sender: Sender<WatchSignal>,
    roots: &[NormalizedPath],
    service_directory: Option<&Path>,
) -> Result<StartedStream, WatchError> {
    let hints = Arc::new(HintCount::default());
    let counted = Arc::clone(&hints);
    let changes: Changes = Arc::default();
    let collected = Arc::clone(&changes);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        let signal = signal_for(event, &collected);
        if signal == WatchSignal::Changed {
            counted.note();
        }
        queue(&sender, signal);
    })
    .map_err(|error| WatchError::Subscribe(failure_category(error)))?;

    let watched = register_all(&mut watcher, roots)?;
    // The probe speaks for the roots that are actually watched, not for the ones the
    // configuration names: a root the backend refused is not on this stream, and asking
    // the filesystem about it — for its device — would fail the probe over a root that
    // was already reported as not watched.
    let registered: Vec<NormalizedPath> = watched
        .iter()
        .filter(|root| root.is_registered())
        .map(|root| root.root().clone())
        .collect();

    let delivery = confirm(service_directory, &mut watcher, &registered, &hints)
        .map_err(WatchError::Subscribe)?;
    Ok(StartedStream {
        watcher: (!registered.is_empty()).then_some(watcher),
        roots: watched,
        delivery,
        changes,
    })
}

/// Takes every hint already queued and puts back anything that is not one, so a stop
/// requested while the stream started is still the loop's to see.
fn drain_hints(receiver: &Receiver<WatchSignal>, sender: &Sender<WatchSignal>) {
    let mut kept = Vec::new();
    while let Ok(signal) = receiver.try_recv() {
        if signal != WatchSignal::Changed {
            kept.push(signal);
        }
    }
    for signal in kept {
        queue(sender, signal);
    }
}

/// How many change hints the stream has delivered, for the probe to wait on without taking
/// them off the queue the watch reads.
#[derive(Default)]
struct HintCount {
    seen: Mutex<u64>,
    arrived: Condvar,
}

impl HintCount {
    fn note(&self) {
        if let Ok(mut seen) = self.seen.lock() {
            *seen += 1;
            self.arrived.notify_all();
        }
    }

    /// Whether any hint has arrived, waiting at most `timeout` for the first.
    fn any_within(&self, timeout: Duration) -> bool {
        let Ok(seen) = self.seen.lock() else {
            return false;
        };
        match self
            .arrived
            .wait_timeout_while(seen, timeout, |seen| *seen == 0)
        {
            Ok((seen, _)) => *seen > 0,
            Err(_) => false,
        }
    }
}

/// Every root on the shared watcher, in **one** restart of the stream.
///
/// `watch` per path would do: `notify`'s FSEvents backend stops the stream, appends the
/// path and starts it again on every call, so ten roots would be ten tear-downs and ten
/// thread spawns before the first pass. `paths_mut` is the same backend's batching
/// contract — stop once, add each path, start once — and it still answers per path, which
/// is what keeps one refused root from being the answer for the others.
///
/// A recursive watch the OS refuses is not a reason to refuse the whole watch. On Linux one
/// directory inside a root that this user cannot read denies that root's subscription —
/// measured — and the watcher then delivers nothing for the readable part either, so
/// keeping it would claim hints that never come. On macOS the same tree subscribes without
/// complaint. Refusing would make `index watch` unusable on Linux for a tree holding one
/// directory of somebody else's, while the pass that follows handles exactly that case by
/// counting it as a failed path.
fn register_all(
    watcher: &mut RecommendedWatcher,
    roots: &[NormalizedPath],
) -> Result<Vec<RootWatch>, WatchError> {
    let mut paths = watcher.paths_mut();
    let watched = roots
        .iter()
        .map(
            |root| match paths.add(Path::new(root.as_str()), RecursiveMode::Recursive) {
                Ok(()) => RootWatch::registered(root.clone()),
                Err(error) => RootWatch::refused(root.clone(), failure_category(error)),
            },
        )
        .collect();
    paths
        .commit()
        .map_err(|error| WatchError::Subscribe(failure_category(error)))?;
    Ok(watched)
}

/// Whether this stream really delivers, proved on the stream the roots are on.
///
/// The probe directory is registered on the **same** watcher as the roots, so the hint
/// it waits for comes out of the same FSEvents stream. That is the whole of what
/// changed in Stage 36 and the whole of what it buys: the old probe made a watcher of
/// its own, so a root stream that was created and never started — `notify` ignores what
/// `FSEventStreamStart` returns — passed the probe anyway.
///
/// What it still does not prove is per root. See `EventDelivery`.
fn confirm(
    service_directory: Option<&Path>,
    watcher: &mut RecommendedWatcher,
    registered: &[NormalizedPath],
    hints: &HintCount,
) -> Result<EventDelivery, SubscribeFailure> {
    if registered.is_empty() {
        return Ok(EventDelivery::Unconfirmed(
            UnconfirmedReason::NothingWatched,
        ));
    }
    let Some(service_directory) = service_directory else {
        return Ok(EventDelivery::Unconfirmed(UnconfirmedReason::NoProbe));
    };
    confirm_delivery(service_directory, registered, watcher, hints)
}

#[cfg(unix)]
fn confirm_delivery(
    service_directory: &Path,
    roots: &[NormalizedPath],
    watcher: &mut RecommendedWatcher,
    hints: &HintCount,
) -> Result<EventDelivery, SubscribeFailure> {
    super::watch_probe::DeliveryProbe::new(service_directory.to_path_buf())
        .confirm(roots, &mut SharedStreamHints { watcher, hints })
}

/// No descriptor-relative probe exists here, and a path-based one would be an unsafe
/// fallback, so delivery stays unconfirmed and the watch runs degraded.
#[cfg(not(unix))]
fn confirm_delivery(
    _service_directory: &Path,
    _roots: &[NormalizedPath],
    _watcher: &mut RecommendedWatcher,
    _hints: &HintCount,
) -> Result<EventDelivery, SubscribeFailure> {
    Ok(EventDelivery::Unconfirmed(UnconfirmedReason::NoProbe))
}

/// The probe's view of the event backend: the roots' own watcher, so what the probe waits
/// for is the stream the roots are on.
///
/// The probe counts hints instead of taking them off the roots' queue. The queue belongs to
/// the watch, and a start that runs past its limit leaves this thread still probing while the
/// watch already reads the queue — two readers of one queue would each take what the other
/// was waiting for.
#[cfg(unix)]
struct SharedStreamHints<'a> {
    watcher: &'a mut RecommendedWatcher,
    hints: &'a HintCount,
}

#[cfg(unix)]
impl super::watch_probe::HintSource for SharedStreamHints<'_> {
    fn watch(&mut self, directory: &Path) -> Result<(), SubscribeFailure> {
        self.watcher
            .watch(directory, RecursiveMode::NonRecursive)
            .map_err(failure_category)
    }

    fn hint_within(&mut self, timeout: Duration) -> bool {
        self.hints.any_within(timeout)
    }

    fn unwatch(&mut self, directory: &Path) {
        // The probe directory stays registered only for as long as the probe runs: it is
        // not part of what this watch is about, and a hint from it later would start a pass
        // over the context for a file nobody indexed. A directory that will not unregister
        // costs one extra pass per change in the data directory, which is not worth failing
        // a watch that has already proved it delivers.
        let _ = self.watcher.unwatch(directory);
    }
}

/// What one event says. A rescan notice — FSEvents' `MustScanSubDirs`, which comes with
/// `UserDropped` or `KernelDropped` when the queue overflowed — and a moved root mean events
/// were lost; an error means the stream itself is not to be trusted. Anything else is a change
/// at the paths it names, which are noted for the next pass; an event that names none, or one
/// that is not a path this program can represent, makes the next pass whole.
fn signal_for(event: notify::Result<Event>, changes: &Mutex<ChangedPaths>) -> WatchSignal {
    let event = match event {
        Ok(event) => event,
        Err(_) => return WatchSignal::StreamError,
    };
    if event.need_rescan() || event.info() == Some(ROOT_CHANGED) {
        return WatchSignal::EventsLost;
    }
    let Ok(mut changes) = changes.lock() else {
        return WatchSignal::EventsLost;
    };
    if event.paths.is_empty() {
        changes.note_unknown();
    }
    for path in &event.paths {
        match path
            .to_str()
            .and_then(|path| NormalizedPath::from_absolute(path).ok())
        {
            Some(path) => changes.note(path),
            None => changes.note_unknown(),
        }
    }
    WatchSignal::Changed
}

/// The note `notify` puts on the event FSEvents sends when a watched root itself was moved or
/// removed (`kFSEventStreamEventFlagRootChanged`).
const ROOT_CHANGED: &str = "root changed";

/// notify's `Display` appends the paths an error is about, its generic messages are free
/// text and an I/O error's text may name a path, so only a fixed category survives.
fn failure_category(error: notify::Error) -> SubscribeFailure {
    match error.kind {
        notify::ErrorKind::PathNotFound => SubscribeFailure::RootNotFound,
        notify::ErrorKind::MaxFilesWatch => SubscribeFailure::ResourceLimit,
        notify::ErrorKind::Io(error) => match error.kind() {
            std::io::ErrorKind::NotFound => SubscribeFailure::RootNotFound,
            std::io::ErrorKind::PermissionDenied => SubscribeFailure::OsDenied,
            _ => SubscribeFailure::BackendUnavailable,
        },
        notify::ErrorKind::Generic(_)
        | notify::ErrorKind::WatchNotFound
        | notify::ErrorKind::InvalidConfig(_) => SubscribeFailure::BackendUnavailable,
    }
}

/// Keeps the OS watch alive for as long as the subscription exists. `None` when the OS
/// refused to watch part of the tree: there is then nothing to keep alive, and the
/// subscription answers every wait with a timeout, which is the degraded periodic mode.
struct NotifyWatchSubscription {
    _watcher: Option<RecommendedWatcher>,
    /// `None` when the stream never started: then nothing is known about where.
    changes: Option<Changes>,
    receiver: Receiver<WatchSignal>,
    /// Held so the queue can never disconnect while the subscription is in use.
    _sender: Sender<WatchSignal>,
    delivery: EventDelivery,
    roots: Vec<RootWatch>,
}

impl WatchSubscription for NotifyWatchSubscription {
    fn event_delivery(&self) -> EventDelivery {
        self.delivery
    }

    fn roots(&self) -> &[RootWatch] {
        &self.roots
    }

    fn next_signal(&mut self, wait: Duration) -> WatchSignal {
        match self.receiver.recv_timeout(wait) {
            Ok(signal) => signal,
            Err(RecvTimeoutError::Timeout) => WatchSignal::Timeout,
            // Unreachable while `_sender` is held; a vanished queue is a lost stream.
            Err(RecvTimeoutError::Disconnected) => WatchSignal::StreamError,
        }
    }

    fn take_changes(&mut self) -> ChangedPaths {
        self.changes
            .as_ref()
            .and_then(|changes| {
                changes
                    .lock()
                    .ok()
                    .map(|mut changes| std::mem::take(&mut *changes))
            })
            .unwrap_or_else(ChangedPaths::unknown)
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use super::{RecommendedWatcher, RecursiveMode, failure_category};
    use crate::application::{SubscribeFailure, WatchError};

    const SENTINEL: &str = "/Users/someone/very-secret-folder";

    fn rendered(error: notify::Error) -> String {
        WatchError::Subscribe(failure_category(error)).to_string()
    }

    /// What one watcher over N paths costs against N watchers over one path each.
    ///
    /// The choice Stage 36 had to make, measured rather than argued: `notify`'s FSEvents
    /// backend keeps one `CFMutableArray` of paths and rebuilds a single stream on every
    /// `watch` call, so one watcher is one stream, one run loop and one thread whatever the
    /// number of roots. Ten watchers are ten of each, for a context that has ten roots and
    /// one queue of signals.
    ///
    /// Ignored because it counts threads of the running process, which is a property of the
    /// whole test binary and not of one test:
    ///
    ///   cargo test --release --offline --lib watch_notify::tests::the_cost_of -- --ignored --nocapture
    #[test]
    #[ignore = "замер формы наблюдения: см. команду в комментарии"]
    fn the_cost_of_one_watcher_over_many_paths_against_many_watchers() {
        use notify::Watcher as _;

        let root =
            std::env::temp_dir().join(format!("crosskit-watch-shape-{}", std::process::id()));
        let paths: Vec<PathBuf> = (0..10)
            .map(|index| {
                let path = root.join(format!("root-{index}"));
                std::fs::create_dir_all(&path).unwrap();
                path
            })
            .collect();

        let before = threads_of_this_process();
        // One watcher over one path first, so the comparison below says whether the cost of
        // a watcher grows with the number of paths on it, and not only what ten cost.
        let mut single = notify::recommended_watcher(|_| {}).unwrap();
        single.watch(&paths[0], RecursiveMode::Recursive).unwrap();
        let with_single = threads_of_this_process();
        drop(single);

        let mut one = notify::recommended_watcher(|_| {}).unwrap();
        for path in &paths {
            one.watch(path, RecursiveMode::Recursive).unwrap();
        }
        let with_one = threads_of_this_process();

        let mut many: Vec<RecommendedWatcher> = Vec::new();
        for path in &paths {
            let mut watcher = notify::recommended_watcher(|_| {}).unwrap();
            watcher.watch(path, RecursiveMode::Recursive).unwrap();
            many.push(watcher);
        }
        let with_many = threads_of_this_process();

        println!(
            "threads: idle {before}; one watcher over 1 path {with_single} (+{single_cost}); \
             one watcher over {roots} paths {with_one} (+{one_cost}); {roots} watchers \
             {with_many} (+{many_cost})",
            roots = paths.len(),
            single_cost = with_single - before,
            one_cost = with_one - before,
            many_cost = with_many - with_one
        );
        drop(one);
        drop(many);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Threads of the running process, counted the way the OS counts them.
    fn threads_of_this_process() -> usize {
        let output = std::process::Command::new("ps")
            .args(["-M", "-p", &std::process::id().to_string()])
            .output()
            .expect("ps must run");
        // One header line, then one line per thread.
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .count()
            .saturating_sub(1)
    }

    #[test]
    fn backend_errors_leave_as_fixed_categories_without_their_text_or_paths() {
        let generic = notify::Error::generic(&format!("cannot watch {SENTINEL}"))
            .add_path(PathBuf::from(SENTINEL));
        let io = notify::Error::io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("denied: {SENTINEL}"),
        ))
        .add_path(PathBuf::from(SENTINEL));
        let other_io = notify::Error::io(io::Error::other(format!("odd failure at {SENTINEL}")))
            .add_path(PathBuf::from(SENTINEL));
        let missing = notify::Error::path_not_found().add_path(PathBuf::from(SENTINEL));

        for error in [generic, io, other_io, missing] {
            let text = rendered(error);
            assert!(
                !text.contains("secret") && !text.contains("someone") && !text.contains('/'),
                "a path leaked into {text:?}"
            );
        }
        assert_eq!(
            failure_category(notify::Error::generic(SENTINEL)),
            SubscribeFailure::BackendUnavailable
        );
        assert_eq!(
            failure_category(notify::Error::io(io::Error::from(
                io::ErrorKind::PermissionDenied
            ))),
            SubscribeFailure::OsDenied
        );
        assert_eq!(
            failure_category(notify::Error::io(io::Error::from(io::ErrorKind::NotFound))),
            SubscribeFailure::RootNotFound
        );
        assert_eq!(
            failure_category(notify::Error::path_not_found()),
            SubscribeFailure::RootNotFound
        );
        assert_eq!(
            failure_category(notify::Error::new(notify::ErrorKind::MaxFilesWatch)),
            SubscribeFailure::ResourceLimit
        );
    }
}
