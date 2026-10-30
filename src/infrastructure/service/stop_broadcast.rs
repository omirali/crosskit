// One stop for every watcher of the service.
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use super::super::filesystem::StopHandle;

/// The service's stop: launchd's termination signal reaches every context's watcher, and a
/// context waiting to retry wakes at once instead of sleeping through it.
#[derive(Clone, Default)]
pub struct StopBroadcast {
    shared: Arc<(Mutex<Stopping>, Condvar)>,
}

#[derive(Default)]
struct Stopping {
    stopped: bool,
    /// The watcher each context runs now; a context that starts its watch again replaces it.
    watchers: BTreeMap<String, StopHandle>,
}

impl StopBroadcast {
    /// The watcher `context` runs from now on. A stop that already came stops it at once.
    pub fn register(&self, context: &str, watcher: StopHandle) {
        let (stopping, _) = &*self.shared;
        let mut stopping = stopping.lock().unwrap_or_else(PoisonError::into_inner);
        if stopping.stopped {
            watcher.request_stop();
        }
        stopping.watchers.insert(context.to_string(), watcher);
    }

    pub fn stop(&self) {
        let (stopping, woken) = &*self.shared;
        let mut stopping = stopping.lock().unwrap_or_else(PoisonError::into_inner);
        stopping.stopped = true;
        for watcher in stopping.watchers.values() {
            watcher.request_stop();
        }
        woken.notify_all();
    }

    /// Waits at most `timeout`, and answers whether the service is stopping.
    pub fn wait(&self, timeout: Duration) -> bool {
        let (stopping, woken) = &*self.shared;
        let stopping = stopping.lock().unwrap_or_else(PoisonError::into_inner);
        let (stopping, _) = woken
            .wait_timeout_while(stopping, timeout, |stopping| !stopping.stopped)
            .unwrap_or_else(PoisonError::into_inner);
        stopping.stopped
    }

    /// Stops the service on SIGTERM — what launchd sends — and on SIGINT and SIGHUP, so a
    /// service run by hand stops the same way.
    pub fn stop_on_termination(&self) -> Result<(), String> {
        let broadcast = self.clone();
        ctrlc::set_handler(move || broadcast.stop())
            .map_err(|error| format!("install the termination handler: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::StopBroadcast;
    use crate::application::{WatchEventSource, WatchSignal};
    use crate::infrastructure::NotifyWatchSource;

    #[test]
    fn a_stop_reaches_a_watcher_registered_after_it_and_wakes_a_waiting_context() {
        let broadcast = StopBroadcast::default();
        assert!(!broadcast.wait(Duration::from_millis(1)));
        broadcast.stop();
        assert!(broadcast.wait(Duration::from_secs(5)));

        let (source, handle) = NotifyWatchSource::new(None);
        broadcast.register("work", handle);
        let mut subscription = source.subscribe(&[]).unwrap();
        assert_eq!(
            subscription.next_signal(Duration::from_secs(5)),
            WatchSignal::Stopped
        );
    }
}
