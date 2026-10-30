// Ports through which the watch use case sees change hints, time, the reconcile pass and
// the console. None of them carries a path: a filesystem event is a hint that something
// under the root may have changed, never evidence about a particular file, and the
// reconcile pass that follows is the only source of truth about the graph.
use std::fmt;
use std::time::{Duration, Instant};

use crate::domain::NormalizedPath;

use super::context_pass::ContextPassReport;
use super::initial_scan::IndexError;
use super::repository::RepositoryError;
use super::watch_report::{AnalysisPass, PassRecord, WatchMode, WatchReport};
use super::watch_scope::{ChangedPaths, PassScope};

/// What a subscription tells the watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSignal {
    /// Something under the root may have changed.
    Changed,
    /// The event stream failed or overflowed: events may have been lost and may keep being
    /// lost, so event watching can no longer be trusted.
    StreamError,
    /// The OS said events were dropped or the watched root itself moved (`MustScanSubDirs`,
    /// `UserDropped`, `KernelDropped`, `RootChanged`): what changed is unknown, so the whole
    /// context is walked at once. The stream itself goes on delivering.
    EventsLost,
    /// Nothing arrived within the requested wait.
    Timeout,
    /// The operator asked the watcher to stop.
    Stopped,
}

/// Why a subscription's delivery could not be confirmed. Closed categories only: the
/// console prints a fixed sentence for each, never a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnconfirmedReason {
    /// The probe change produced no hint within the adapter's wait.
    NoHint,
    /// The probe location lies on another filesystem than the root, whose delivery it
    /// therefore cannot vouch for.
    ProbeOnOtherDevice,
    /// The probe location lies inside the watched root; the watcher must not create
    /// events in the tree it scans.
    ProbeInsideRoot,
    /// No probe could run: no probe location was configured, or this platform offers no
    /// probe with the required safety guarantees.
    NoProbe,
    /// No root could be watched at all, so there is no stream to probe.
    NothingWatched,
    /// The event backend did not finish starting within the adapter's limit, so the watch
    /// went ahead without it rather than waiting on it for good.
    StartTimedOut,
}

/// Whether the adapter proved, at subscription time, that change hints really arrive for
/// the root. A stream can be created and started without ever delivering (a sandbox
/// without event access, a volume without event logging), and the OS does not say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDelivery {
    /// A probe change on the root's own filesystem came back as a hint.
    Confirmed,
    /// Hints are not trusted; the watch starts in degraded periodic mode.
    Unconfirmed(UnconfirmedReason),
}

impl EventDelivery {
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

pub trait WatchSubscription {
    fn event_delivery(&self) -> EventDelivery;

    /// Every root the watcher was asked to watch, with what the backend said about each.
    fn roots(&self) -> &[RootWatch];

    /// Blocks until a signal arrives or `wait` elapses.
    fn next_signal(&mut self, wait: Duration) -> WatchSignal;

    /// Where the hints since the last call said something changed. A source that keeps no
    /// paths says it does not know, and every pass after its hints walks the whole context.
    fn take_changes(&mut self) -> ChangedPaths {
        ChangedPaths::unknown()
    }
}

pub trait WatchEventSource {
    /// Subscribes recursively to every root and returns only once the subscription is
    /// live. The watcher subscribes before its first pass, so a change made during that
    /// pass is queued and starts the next pass instead of being lost.
    ///
    /// A root the backend refuses does not refuse the subscription: its failure is one
    /// entry in [`WatchSubscription::roots`], the rest are watched, and the passes that
    /// follow walk the whole context whichever root a hint came from. The subscription
    /// fails only when nothing at all could be watched.
    fn subscribe(&self, roots: &[NormalizedPath])
    -> Result<Box<dyn WatchSubscription>, WatchError>;
}

/// What became of one root when the watcher subscribed.
///
/// Registration, and only registration: the backend answers whether it took the path, and
/// nothing anywhere answers whether events for that path will actually be delivered. See
/// `EventDelivery` for what is proven about delivery, and for what is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootWatch {
    root: NormalizedPath,
    registered: Result<(), SubscribeFailure>,
}

impl RootWatch {
    pub fn registered(root: NormalizedPath) -> Self {
        Self {
            root,
            registered: Ok(()),
        }
    }

    pub fn refused(root: NormalizedPath, failure: SubscribeFailure) -> Self {
        Self {
            root,
            registered: Err(failure),
        }
    }

    pub fn root(&self) -> &NormalizedPath {
        &self.root
    }

    pub fn is_registered(&self) -> bool {
        self.registered.is_ok()
    }

    pub fn failure(&self) -> Option<SubscribeFailure> {
        self.registered.err()
    }
}

/// Why a subscription could not be made: a closed set of categories. No adapter text,
/// path or OS error type ever reaches it, because a backend message may name a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeFailure {
    /// The root vanished between resolution and subscription.
    RootNotFound,
    /// The OS denied access to the root.
    OsDenied,
    /// A watch or descriptor limit of the OS was reached.
    ResourceLimit,
    /// The event backend failed for a reason of its own.
    BackendUnavailable,
    /// The delivery probe could not be performed safely: its directory or file was not
    /// what the watcher created, or could not be created, written or removed.
    ProbeUnavailable,
    AlreadySubscribed,
    SignalQueuePoisoned,
    /// The backend did not answer whether it took the root before the start limit ran out.
    StartTimedOut,
}

impl fmt::Display for SubscribeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RootNotFound => "the root no longer exists",
            Self::OsDenied => "the OS denied access to the root",
            Self::ResourceLimit => "an OS watch or descriptor limit was reached",
            Self::BackendUnavailable => "the filesystem event backend rejected the watch",
            Self::ProbeUnavailable => {
                "the delivery probe could not be performed safely in the data directory"
            }
            Self::AlreadySubscribed => "the event source is already subscribed",
            Self::SignalQueuePoisoned => "the watch signal queue is poisoned",
            Self::StartTimedOut => "the filesystem event backend did not start in time",
        })
    }
}

/// Monotonic time for debounce, periodic and retry deadlines. File timestamps never
/// take part in these decisions.
pub trait WatchClock {
    fn now(&self) -> Instant;
}

/// One full policy-aware pass over everything this watcher watches: scan, commit and
/// tombstone, for every root of the context. The watcher owns no other way to change the
/// graph.
///
/// A whole pass covers the context, not the root a hint came from — which is why a root whose
/// events the backend refused is still reconciled by every periodic pass. A pass after hints
/// that named their paths walks only the directories they lie in (ADR 0050).
pub trait ReconcilePass {
    fn execute(&self, scope: &PassScope) -> Result<ContextPassReport, IndexError>;
}

/// One bounded sweep of the context's insight queue, as the watcher asks for it after a
/// pass that completed.
///
/// The watcher knows nothing about what the work is. A context that keeps its content to
/// itself and a context that asks a model reach the queue through this one port, and which
/// of the two exists at all is decided before the watch starts — so no pass can turn into
/// the wrong kind of analysis halfway through a session.
pub trait QueueAnalysis {
    /// Works at most `limit` jobs, oldest first, and answers with what the sweep did and
    /// what the queue still holds.
    fn analyze(&self, limit: usize) -> Result<AnalysisPass, RepositoryError>;
}

/// The sanitized lifecycle of a watch, as it happens. Nothing handed to an observer names
/// a changed or excluded path.
pub trait WatchObserver {
    fn started(&mut self, mode: WatchMode, delivery: EventDelivery);
    fn pass_finished(&mut self, record: &PassRecord);
    fn retry_scheduled(&mut self, delay: Duration);
    /// Event watching is no longer trusted; only periodic passes follow, this far apart.
    fn degraded(&mut self, periodic_interval: Duration);
    /// The pass was committed but could not be folded into the database file, so it stays
    /// in the log the engine would refuse after an unclean stop. Not a failure of the
    /// pass: what it wrote is in the graph either way.
    fn checkpoint_failed(&mut self, error: &RepositoryError);
    /// A sweep of the insight queue ended, after the pass of this number.
    fn analysis_finished(&mut self, pass_number: usize, pass: &AnalysisPass);
    /// The queue could not be worked at all. Not a failure of the pass: what the pass wrote
    /// is committed, and the watch carries on — analysis is work the watcher does beside
    /// watching, never instead of it.
    fn analysis_failed(&mut self, pass_number: usize, error: &RepositoryError);
    /// Which roots the backend took and which it refused, said once when the watch starts.
    /// A refused root is not a silent one: every pass walks it anyway, so what it loses is
    /// the latency of an event, not the coverage of the context.
    fn watching(&mut self, roots: &[RootWatch]);
    /// The watcher accepts no further batches; `final_pass` says whether pending work still
    /// gets one last pass.
    fn stopping(&mut self, final_pass: bool);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchError {
    Subscribe(SubscribeFailure),
    /// The pass run at shutdown did not complete, so missing statuses were left as they
    /// were. The report covers the whole watch.
    FinalPassFailed(WatchReport),
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Subscribe(failure) => write!(f, "cannot watch the root: {failure}"),
            Self::FinalPassFailed(_) => f.write_str(
                "the final reconcile pass did not complete; missing statuses were left unchanged",
            ),
        }
    }
}

impl std::error::Error for WatchError {}
