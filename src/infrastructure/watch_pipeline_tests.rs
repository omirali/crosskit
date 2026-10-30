// End-to-end: notify change hints driving the policy-aware reconcile pass against the real
// database. The test thread waits for passes through the observer with bounded timeouts;
// nothing sleeps for a fixed time and no event path is ever inspected.
//
// Whether change hints arrive at all depends on the environment: a sandbox without
// FSEvents access delivers nothing and the OS does not say so. Every scenario therefore
// starts from the watch's own verdict: hints confirmed, or degraded periodic mode. In the
// latter case the event-driven part is skipped and the skip is printed; the periodic
// fallback has its own scenario that never depends on hints.
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use super::filesystem::{DescriptorRelativeWalker, NotifyWatchSource, StopHandle};
use super::index_pipeline_tests::test_directory;
use super::ladybug::LadybugDatabaseManager;
use super::ladybug::test_support::{
    TestDatabase, assert_current_version, count, query_string, status_of,
};
use super::system_clock::SystemClock;
use crate::application::{
    Admission, ContentSkipReason, ContextCompletion, ContextIncomplete, ContextPassReport,
    EventDelivery, EvidenceRead, IncompleteReason, IndexError, PassOutcome, PassRecord, PassScope,
    PassTrigger, ReconcilePass, ReconcileService, RootOutcome, RootWatch, ScanReport, SourceEntry,
    SourceVisitor, SourceWalker, SubscribeFailure, UnconfirmedReason, WatchEventSource, WatchMode,
    WatchObserver, WatchReport, WatchSchedule, WatchService, WatchSignal,
};
use crate::domain::{
    FileLocation, IndexPolicy, NormalizedPath, ObservedFileIdentity, PathPattern, PolicyClass,
    PolicyRule, RuleOrigin,
};

/// Long enough for FSEvents plus a debounce on a loaded machine, short enough to fail
/// a broken test instead of hanging it.
const BOUNDED_WAIT: Duration = Duration::from_secs(20);

/// Debounce 150 ms, retries from 50 ms up to 200 ms, periodic pass as given.
fn fast_schedule(periodic: Duration) -> WatchSchedule {
    WatchSchedule::new(
        Duration::from_millis(150),
        periodic,
        Duration::from_millis(50),
        Duration::from_millis(200),
    )
}

fn include_everything() -> IndexPolicy {
    IndexPolicy::new(vec![], 100 * 1024 * 1024)
}

fn resolve(root: &Path) -> NormalizedPath {
    DescriptorRelativeWalker.resolve_root(root).unwrap()
}

/// How a scenario's watch is wired: where delivery is probed and how often periodic
/// passes run.
struct WatchSetup {
    service_directory: Option<PathBuf>,
    schedule: WatchSchedule,
}

impl WatchSetup {
    /// Probes delivery like production does, from a service directory outside the root;
    /// periodic passes stay out of the way.
    fn probed(service_directory: PathBuf) -> Self {
        Self {
            service_directory: Some(service_directory),
            schedule: fast_schedule(Duration::from_secs(120)),
        }
    }

    /// Probes from `service_directory` with fast periodic passes: the scenario expects
    /// the probe to be refused and the periodic fallback to carry the watch.
    fn probed_with_periodic(service_directory: PathBuf, periodic: Duration) -> Self {
        Self {
            service_directory: Some(service_directory),
            schedule: fast_schedule(periodic),
        }
    }

    /// No probe location, so delivery is unconfirmed and the watch starts degraded.
    fn unprobed(periodic: Duration) -> Self {
        Self {
            service_directory: None,
            schedule: fast_schedule(periodic),
        }
    }
}

/// A pass record carries a whole reconciliation report, which is far larger than a mode,
/// so it travels boxed rather than making every notice that size.
#[derive(Debug)]
enum Notice {
    Started(WatchMode),
    Pass(Box<PassRecord>),
}

/// Forwards the start notice and every pass to the test thread; the rest is lifecycle
/// the console renders and these scenarios do not need.
struct ChannelObserver {
    notices: Sender<Notice>,
}

impl WatchObserver for ChannelObserver {
    fn watching(&mut self, _roots: &[RootWatch]) {}

    fn started(&mut self, mode: WatchMode, _delivery: EventDelivery) {
        self.notices.send(Notice::Started(mode)).unwrap();
    }

    fn pass_finished(&mut self, record: &PassRecord) {
        self.notices
            .send(Notice::Pass(Box::new(record.clone())))
            .unwrap();
    }

    fn retry_scheduled(&mut self, _delay: Duration) {}

    fn analysis_finished(&mut self, _pass_number: usize, pass: &crate::application::AnalysisPass) {
        panic!("no scenario here asks the watcher to work the queue: {pass:?}");
    }

    fn analysis_failed(
        &mut self,
        _pass_number: usize,
        error: &crate::application::RepositoryError,
    ) {
        panic!("no scenario here asks the watcher to work the queue: {error}");
    }

    fn degraded(&mut self, _periodic_interval: Duration) {}

    fn checkpoint_failed(&mut self, error: &crate::application::RepositoryError) {
        panic!("the checkpoint of a real graph must not fail here: {error}");
    }

    fn stopping(&mut self, _final_pass: bool) {}
}

/// The context pass of a watch over exactly one root, which is what these scenarios watch.
struct OneRoot<'a, Walker: SourceWalker> {
    service:
        &'a ReconcileService<'a, Walker, crate::infrastructure::ladybug::LadybugGraphRepository>,
    root: NormalizedPath,
}

impl<Walker: SourceWalker> ReconcilePass for OneRoot<'_, Walker> {
    fn execute(&self, _scope: &PassScope) -> Result<ContextPassReport, IndexError> {
        let mut report = ContextPassReport::default();
        report.record(RootOutcome::Reconciled {
            root: self.root.as_str().to_string(),
            report: Box::new(self.service.execute(&self.root)?),
        });
        Ok(report)
    }
}

/// Runs the watch on this thread while `drive` mutates the tree from another one. `drive`
/// receives every notice, must request the stop when it is done, and returns whatever
/// the scenario needs to know afterwards.
fn watch<Walker: SourceWalker, T: Send>(
    walker: &Walker,
    manager: &Arc<LadybugDatabaseManager>,
    policy: &IndexPolicy,
    root: &NormalizedPath,
    setup: WatchSetup,
    drive: impl FnOnce(&Receiver<Notice>, &StopHandle) -> T + Send,
) -> (WatchReport, T) {
    let repository = manager.repository();
    let analysis_rules = analysing_everything();
    let service = ReconcileService::new(walker, &repository, policy, &analysis_rules);
    // The watcher's pass covers a context; here the context is one root, which is what the
    // presentation hands it when a directory is named.
    let reconcile = OneRoot {
        service: &service,
        root: root.clone(),
    };
    let (events, stop) = NotifyWatchSource::new(setup.service_directory);
    let (sender, receiver) = mpsc::channel();
    let mut observer = ChannelObserver { notices: sender };
    thread::scope(|scope| {
        let driver = scope.spawn(move || drive(&receiver, &stop));
        let outcome = WatchService::new(
            &reconcile,
            &repository,
            &events,
            &SystemClock,
            &mut observer,
            setup.schedule,
        )
        .run(std::slice::from_ref(root));
        let driven = driver.join().unwrap();
        (outcome.unwrap(), driven)
    })
}

/// The watch's opening notice. `false` when this environment delivers no change hints; the
/// caller then skips the event-driven part of its scenario and says so.
fn hints_arrive(notices: &Receiver<Notice>) -> bool {
    match notices.recv_timeout(BOUNDED_WAIT) {
        Ok(Notice::Started(WatchMode::EventDriven)) => true,
        Ok(Notice::Started(WatchMode::DegradedPeriodic)) => {
            eprintln!(
                "skipped: no filesystem change hints arrive in this environment; the watch runs in degraded periodic mode"
            );
            false
        }
        other => panic!(
            "expected the start notice, received {other:?}{}",
            crowded_binary_directory()
        ),
    }
}

/// Above this many entries beside the test binary, a slow stream start has a known cause.
/// Stage W measured the start at 0.1 s with 50 000 entries and 9–14 s with 870 000.
const CROWDED_DIRECTORY: usize = 100_000;

/// Names the one cause of a missing start notice that is not the code: macOS reads the
/// directory of a process that starts an FSEvents stream before the stream starts, so a
/// test binary among hundreds of thousands of leftover objects waits seconds for nothing.
fn crowded_binary_directory() -> String {
    let Some(directory) = std::env::current_exe()
        .ok()
        .and_then(|binary| binary.parent().map(Path::to_path_buf))
    else {
        return String::new();
    };
    let entries = fs::read_dir(&directory).map(Iterator::count).unwrap_or(0);
    if entries < CROWDED_DIRECTORY {
        return String::new();
    }
    format!(
        "\nthe directory of this test binary holds {entries} entries, and macOS reads it before \
         an FSEvents stream starts (Stage W); remove the leftovers with \
         `find target/debug/deps -name '*.rcgu.o' -delete`"
    )
}

fn next_pass(notices: &Receiver<Notice>) -> PassRecord {
    match notices.recv_timeout(BOUNDED_WAIT) {
        Ok(Notice::Pass(record)) => *record,
        other => panic!("expected a reconcile pass within the bounded wait, received {other:?}"),
    }
}

fn completed_pass(notices: &Receiver<Notice>) -> PassRecord {
    let record = next_pass(notices);
    assert!(
        record.outcome().is_completed(),
        "expected a completed pass, received {record:?}"
    );
    record
}

/// Passes that FSEvents may still trigger after the last mutation, up to a short quiet gap.
fn drain_passes(notices: &Receiver<Notice>) -> Vec<PassRecord> {
    let mut drained = Vec::new();
    loop {
        match notices.recv_timeout(Duration::from_secs(2)) {
            Ok(Notice::Pass(record)) => drained.push(*record),
            Ok(Notice::Started(mode)) => panic!("unexpected second start notice: {mode:?}"),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return drained,
        }
    }
}

/// These scenarios watch one root, so the pass has exactly one reconciliation in it.
fn scan_of(record: &PassRecord) -> &ScanReport {
    match record.outcome() {
        PassOutcome::Completed(report) | PassOutcome::Incomplete(report) => report
            .reconciliations()
            .next()
            .expect("a pass over one root reconciles it")
            .scan(),
        PassOutcome::Failed(error) => panic!("pass failed: {error}"),
    }
}

fn run_count(manager: &LadybugDatabaseManager, status: &str) -> i64 {
    count(
        manager,
        &format!("MATCH (:ReconcileRun {{status: '{status}'}}) RETURN COUNT(*);"),
    )
}

/// The probe directory the watch creates under a service directory holds nothing once
/// the probe is done.
fn assert_probe_left_nothing_behind(service_directory: &Path) {
    let probe_directory = service_directory.join("watch-probe");
    if !probe_directory.exists() {
        return;
    }
    let leftovers: Vec<_> = fs::read_dir(&probe_directory).unwrap().collect();
    assert!(
        leftovers.is_empty(),
        "probe files left behind: {leftovers:?}"
    );
}

/// A start that has not answered within the limit is not waited on: the watch goes on with
/// the periodic pass, every root says why it has no events, and a stop still reaches it.
///
/// A limit of zero stands in for a stream that never starts — nothing a test can do makes
/// `FSEventStreamStart` hang on purpose, and any real start takes longer than no time.
#[test]
fn a_start_past_its_limit_leaves_the_watch_periodic_and_still_stoppable() {
    let root_dir = test_directory("watch-start-limit");
    let probe_dir = test_directory("watch-start-limit-probe");
    let root = resolve(&root_dir);
    let (source, stop) = NotifyWatchSource::new(Some(probe_dir));
    let source = source.with_start_limit(Duration::ZERO);

    let mut subscription = source.subscribe(std::slice::from_ref(&root)).unwrap();

    assert_eq!(
        subscription.event_delivery(),
        EventDelivery::Unconfirmed(UnconfirmedReason::StartTimedOut)
    );
    assert_eq!(subscription.roots().len(), 1);
    assert_eq!(
        subscription.roots()[0].failure(),
        Some(SubscribeFailure::StartTimedOut)
    );
    stop.request_stop();
    assert_eq!(subscription.next_signal(BOUNDED_WAIT), WatchSignal::Stopped);
}

#[test]
fn the_notify_source_subscribes_recursively_and_yields_only_path_free_signals() {
    let root_dir = test_directory("watch-subscribe");
    let probe_dir = test_directory("watch-subscribe-probe");
    fs::create_dir_all(root_dir.join("nested")).unwrap();
    let root = resolve(&root_dir);
    let (source, stop) = NotifyWatchSource::new(Some(probe_dir.clone()));

    let mut subscription = source.subscribe(std::slice::from_ref(&root)).unwrap();
    assert_probe_left_nothing_behind(&probe_dir);
    match subscription.event_delivery() {
        EventDelivery::Confirmed => {
            fs::write(root_dir.join("nested").join("new.md"), "born under watch").unwrap();
            assert_eq!(subscription.next_signal(BOUNDED_WAIT), WatchSignal::Changed);
            // Whatever else the burst produced is more hints, never anything else.
            loop {
                match subscription.next_signal(Duration::from_millis(500)) {
                    WatchSignal::Changed => {}
                    WatchSignal::Timeout => break,
                    other => panic!("unexpected signal {other:?}"),
                }
            }
        }
        EventDelivery::Unconfirmed(reason) => {
            eprintln!("skipped: delivery unconfirmed in this environment ({reason:?})");
        }
    }
    stop.request_stop();
    assert_eq!(subscription.next_signal(BOUNDED_WAIT), WatchSignal::Stopped);

    let again = source.subscribe(std::slice::from_ref(&root));
    assert!(again.is_err(), "a source subscribes at most once");
    fs::remove_dir_all(&root_dir).unwrap();
    fs::remove_dir_all(&probe_dir).unwrap();
}

#[test]
fn a_modified_file_is_reconciled_without_duplicate_evidence() {
    let database = TestDatabase::new("watch-modify");
    let root_dir = test_directory("watch-modify");
    let probe_dir = test_directory("watch-modify-probe");
    fs::write(root_dir.join("notes.md"), "first draft").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let root = resolve(&root_dir);

    let (report, hints) = watch(
        &DescriptorRelativeWalker,
        &manager,
        &include_everything(),
        &root,
        WatchSetup::probed(probe_dir.clone()),
        |notices, stop| {
            let hints = hints_arrive(notices);
            let bootstrap = completed_pass(notices);
            assert_eq!(bootstrap.trigger(), PassTrigger::Bootstrap);
            assert_eq!(scan_of(&bootstrap).indexed_files(), 1);
            if !hints {
                stop.request_stop();
                return false;
            }

            fs::write(root_dir.join("notes.md"), "second draft").unwrap();
            let after_edit = completed_pass(notices);
            assert_eq!(after_edit.trigger(), PassTrigger::Events);

            // The same content written again is a new sighting, not new evidence.
            fs::write(root_dir.join("notes.md"), "second draft").unwrap();
            completed_pass(notices);
            drain_passes(notices);
            stop.request_stop();
            true
        },
    );

    assert_probe_left_nothing_behind(&probe_dir);
    fs::remove_dir_all(&probe_dir).unwrap();
    if !hints {
        fs::remove_dir_all(&root_dir).unwrap();
        return;
    }
    assert!(report.completed_passes() >= 3, "{report:?}");
    assert_eq!(report.failed_passes() + report.incomplete_passes(), 0);
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(count(&manager, "MATCH (:ContentChunk) RETURN COUNT(*);"), 2);
    // The watcher never writes a job itself: each revision the passes committed carries
    // exactly one queued insight job, and re-observing the same content added none.
    assert_eq!(count(&manager, "MATCH (:InsightJob) RETURN COUNT(*);"), 2);
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileVersion)-[:REQUESTS_INSIGHT]->(:InsightJob {state: 'queued'}) RETURN COUNT(*);"
        ),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:CURRENT_VERSION]->(version:FileVersion)-[:HAS_CHUNK]->(chunk:ContentChunk {text: 'second draft'}) RETURN COUNT(*);"
        ),
        1
    );
    let notes = root.join("notes.md").unwrap();
    assert_eq!(status_of(&manager, notes.as_str()), "present");
    fs::remove_dir_all(&root_dir).unwrap();
}

#[test]
fn a_deleted_file_becomes_missing_only_through_a_reconcile_pass() {
    let database = TestDatabase::new("watch-delete");
    let root_dir = test_directory("watch-delete");
    let probe_dir = test_directory("watch-delete-probe");
    fs::write(root_dir.join("kept.md"), "stays").unwrap();
    fs::write(root_dir.join("gone.md"), "leaves").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let root = resolve(&root_dir);

    let (report, hints) = watch(
        &DescriptorRelativeWalker,
        &manager,
        &include_everything(),
        &root,
        WatchSetup::probed(probe_dir.clone()),
        |notices, stop| {
            let hints = hints_arrive(notices);
            assert_eq!(scan_of(&completed_pass(notices)).indexed_files(), 2);
            if !hints {
                stop.request_stop();
                return false;
            }
            fs::remove_file(root_dir.join("gone.md")).unwrap();
            let after_delete = completed_pass(notices);
            assert_eq!(after_delete.trigger(), PassTrigger::Events);
            drain_passes(notices);
            stop.request_stop();
            true
        },
    );

    fs::remove_dir_all(&probe_dir).unwrap();
    if !hints {
        fs::remove_dir_all(&root_dir).unwrap();
        return;
    }
    let kept = root.join("kept.md").unwrap();
    let gone = root.join("gone.md").unwrap();
    assert_eq!(status_of(&manager, kept.as_str()), "present");
    assert_eq!(status_of(&manager, gone.as_str()), "missing");
    // The tombstone came from a completed run; the row and its evidence stay.
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 2);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(
        run_count(&manager, "completed"),
        i64::try_from(report.completed_passes()).unwrap()
    );
    assert_eq!(run_count(&manager, "running"), 0);
    fs::remove_dir_all(&root_dir).unwrap();
}

#[test]
fn an_excluded_file_never_reaches_the_observer_or_the_graph() {
    let database = TestDatabase::new("watch-excluded");
    let root_dir = test_directory("watch-excluded");
    let probe_dir = test_directory("watch-excluded-probe");
    fs::write(root_dir.join("notes.md"), "public").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let root = resolve(&root_dir);
    let policy = IndexPolicy::new(
        vec![PolicyRule::new(
            PolicyClass::Exclude,
            PathPattern::parse("**/*.confidential").unwrap(),
            RuleOrigin::Configuration,
        )],
        100 * 1024 * 1024,
    );

    let (report, records) = watch(
        &DescriptorRelativeWalker,
        &manager,
        &policy,
        &root,
        WatchSetup::probed(probe_dir.clone()),
        |notices, stop| {
            let hints = hints_arrive(notices);
            let mut records = vec![completed_pass(notices)];
            if !hints {
                stop.request_stop();
                return None;
            }
            fs::write(root_dir.join("ledger.confidential"), "not for the index").unwrap();
            let after_write = completed_pass(notices);
            assert_eq!(scan_of(&after_write).excluded_paths(), 1);
            records.push(after_write);
            records.extend(drain_passes(notices));
            stop.request_stop();
            Some(records)
        },
    );

    fs::remove_dir_all(&probe_dir).unwrap();
    let Some(records) = records else {
        fs::remove_dir_all(&root_dir).unwrap();
        return;
    };
    assert!(report.coalesced_batches() >= 1, "{report:?}");
    for record in &records {
        let rendered = format!("{record:?}");
        assert!(
            !rendered.contains("ledger") && !rendered.contains("confidential"),
            "the observer must never see the excluded name: {rendered}"
        );
    }
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (location:FileLocation) WHERE ends_with(location.normalized_path, '.confidential') RETURN COUNT(*);"
        ),
        0
    );
    fs::remove_dir_all(&root_dir).unwrap();
}

#[test]
fn an_event_burst_runs_passes_one_at_a_time_and_coalesces_hints() {
    let database = TestDatabase::new("watch-burst");
    let root_dir = test_directory("watch-burst");
    let probe_dir = test_directory("watch-burst-probe");
    fs::write(root_dir.join("notes.md"), "seed").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let root = resolve(&root_dir);
    const BURST: usize = 30;

    let (report, hints) = watch(
        &DescriptorRelativeWalker,
        &manager,
        &include_everything(),
        &root,
        WatchSetup::probed(probe_dir.clone()),
        |notices, stop| {
            let hints = hints_arrive(notices);
            completed_pass(notices);
            if !hints {
                stop.request_stop();
                return false;
            }
            for index in 0..BURST {
                fs::write(root_dir.join(format!("burst-{index}.md")), "burst").unwrap();
            }
            completed_pass(notices);
            drain_passes(notices);
            stop.request_stop();
            true
        },
    );

    fs::remove_dir_all(&probe_dir).unwrap();
    if !hints {
        fs::remove_dir_all(&root_dir).unwrap();
        return;
    }
    assert!(report.changed_signals() >= 1, "{report:?}");
    assert!(
        report.coalesced_batches() < BURST,
        "hints must coalesce into fewer passes than files: {report:?}"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileLocation {status: 'present'}) RETURN COUNT(*);"
        ),
        i64::try_from(BURST + 1).unwrap()
    );
    // One run per pass and none left open: passes never overlapped.
    assert_eq!(
        run_count(&manager, "completed"),
        i64::try_from(report.completed_passes()).unwrap()
    );
    assert_eq!(run_count(&manager, "running"), 0);
    assert_eq!(report.incomplete_passes() + report.failed_passes(), 0);
    fs::remove_dir_all(&root_dir).unwrap();
}

/// The fallback that never depends on hints: with the service directory inside the root
/// the probe is refused before it creates anything, the watch starts degraded and still
/// picks up a change on a periodic pass, leaving the root exactly as the user left it.
#[test]
fn a_probe_inside_the_root_starts_degraded_and_periodic_passes_still_reconcile() {
    let database = TestDatabase::new("watch-degraded");
    let root_dir = test_directory("watch-degraded");
    fs::write(root_dir.join("notes.md"), "seed").unwrap();
    let service_directory = root_dir.join("service");
    fs::create_dir(&service_directory).unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let root = resolve(&root_dir);

    let (report, picked_up_by) = watch(
        &DescriptorRelativeWalker,
        &manager,
        &include_everything(),
        &root,
        WatchSetup::probed_with_periodic(service_directory.clone(), Duration::from_millis(300)),
        |notices, stop| {
            assert!(
                matches!(
                    notices.recv_timeout(BOUNDED_WAIT),
                    Ok(Notice::Started(WatchMode::DegradedPeriodic))
                ),
                "a probe inside the root must start the watch degraded"
            );
            assert_eq!(scan_of(&completed_pass(notices)).indexed_files(), 1);
            fs::write(root_dir.join("late.md"), "seen without any hint").unwrap();
            // Periodic passes keep coming; one of them sees the new file.
            let picked_up_by = (0..30)
                .map(|_| completed_pass(notices))
                .find(|pass| scan_of(pass).indexed_files() == 2)
                .expect("a periodic pass must index the new file");
            stop.request_stop();
            picked_up_by.trigger()
        },
    );

    assert_eq!(picked_up_by, PassTrigger::Periodic);
    assert_eq!(report.mode(), WatchMode::DegradedPeriodic);
    assert_eq!(report.changed_signals(), 0);
    assert_eq!(report.coalesced_batches(), 0);
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileLocation {status: 'present'}) RETURN COUNT(*);"
        ),
        2
    );
    assert_eq!(run_count(&manager, "running"), 0);
    // The watcher created nothing in the tree it scans: no probe directory, no file.
    let mut root_entries: Vec<String> = fs::read_dir(&root_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    root_entries.sort();
    assert_eq!(root_entries, ["late.md", "notes.md", "service"]);
    assert!(fs::read_dir(&service_directory).unwrap().next().is_none());
    fs::remove_dir_all(&root_dir).unwrap();
}

/// The real walker, except that the first `unstable_reads` content reads report the file
/// as changed while being read, exactly as the adapter does when it catches a write.
struct FlakyWalker {
    inner: DescriptorRelativeWalker,
    unstable_reads: Cell<usize>,
}

impl SourceWalker for FlakyWalker {
    fn resolve_root(&self, root: &Path) -> Result<NormalizedPath, IndexError> {
        self.inner.resolve_root(root)
    }

    fn walk(
        &self,
        root: &NormalizedPath,
        visitor: &mut dyn SourceVisitor,
    ) -> Result<(), IndexError> {
        let mut flaky = FlakyVisitor {
            inner: visitor,
            unstable_reads: &self.unstable_reads,
        };
        self.inner.walk(root, &mut flaky)
    }
}

struct FlakyVisitor<'a> {
    inner: &'a mut dyn SourceVisitor,
    unstable_reads: &'a Cell<usize>,
}

impl SourceVisitor for FlakyVisitor<'_> {
    fn admit(&mut self, path: &NormalizedPath) -> Admission {
        self.inner.admit(path)
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        self.inner.visit_entry(&FlakyEntry {
            inner: entry,
            unstable_reads: self.unstable_reads,
        })
    }

    fn skip_unrepresentable_name(&mut self) {
        self.inner.skip_unrepresentable_name();
    }

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        self.inner.record_failure(path, error);
    }
}

struct FlakyEntry<'a> {
    inner: &'a dyn SourceEntry,
    unstable_reads: &'a Cell<usize>,
}

impl SourceEntry for FlakyEntry<'_> {
    fn path(&self) -> &NormalizedPath {
        self.inner.path()
    }

    fn observed_identity(&self) -> Result<ObservedFileIdentity, IndexError> {
        self.inner.observed_identity()
    }

    fn location(&self, file_id: &str) -> Result<FileLocation, IndexError> {
        self.inner.location(file_id)
    }

    fn read_evidence(
        &self,
        expected: &FileLocation,
        max_content_bytes: u64,
    ) -> Result<EvidenceRead, IndexError> {
        let remaining = self.unstable_reads.get();
        if remaining > 0 {
            self.unstable_reads.set(remaining - 1);
            return Ok(EvidenceRead::Skipped(
                ContentSkipReason::ChangedWhileReading,
            ));
        }
        self.inner.read_evidence(expected, max_content_bytes)
    }
}

/// Independent of hints: the bootstrap pass is unstable, the retry publishes the file.
#[test]
fn an_unstable_read_publishes_nothing_and_the_retry_pass_publishes_the_file() {
    let database = TestDatabase::new("watch-unstable");
    let root_dir = test_directory("watch-unstable");
    fs::write(root_dir.join("notes.md"), "written while read").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let root = resolve(&root_dir);
    let walker = FlakyWalker {
        inner: DescriptorRelativeWalker,
        unstable_reads: Cell::new(1),
    };

    let (report, ()) = watch(
        &walker,
        &manager,
        &include_everything(),
        &root,
        WatchSetup::unprobed(Duration::from_secs(120)),
        |notices, stop| {
            assert!(matches!(
                notices.recv_timeout(BOUNDED_WAIT),
                Ok(Notice::Started(_))
            ));
            let unstable = next_pass(notices);
            assert_eq!(unstable.trigger(), PassTrigger::Bootstrap);
            let PassOutcome::Incomplete(report) = unstable.outcome() else {
                panic!("the unstable read must leave the pass incomplete: {unstable:?}");
            };
            assert_eq!(
                report.completion(),
                ContextCompletion::Incomplete(vec![ContextIncomplete::RootIncomplete(
                    IncompleteReason::UnstableSources(1)
                )]),
                "the one root's own reason carries up to the context's"
            );

            let retried = completed_pass(notices);
            assert_eq!(retried.trigger(), PassTrigger::Retry);
            assert_eq!(scan_of(&retried).indexed_files(), 1);
            stop.request_stop();
        },
    );

    assert_eq!(report.incomplete_passes(), 1);
    assert_eq!(report.retries(), 1);
    assert!(report.completed_passes() >= 1);
    // Nothing partial: the unstable pass left no location, version or chunk behind.
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:ContentChunk) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_eq!(run_count(&manager, "incomplete"), 1);
    let version_id = query_string(
        &manager,
        "MATCH (version:FileVersion) RETURN version.id;",
        vec![],
    );
    assert_current_version(&manager, &version_id);
    fs::remove_dir_all(&root_dir).unwrap();
}

/// These tests are about the watcher, not about which revisions are worth analysing, so
/// every path is one the installation analyses.
fn analysing_everything() -> crate::domain::AnalysisRules {
    crate::domain::AnalysisRules::new(
        vec![crate::domain::PathPattern::parse("**/*").unwrap()],
        Vec::new(),
    )
}
