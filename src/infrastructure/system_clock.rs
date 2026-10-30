// Monotonic time for the watch loop's deadlines, and the wall time a decision is stamped with.
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::application::WatchClock;

pub struct SystemClock;

impl WatchClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Milliseconds since the Unix epoch, for the moment an owner's decision is taken. A clock set
/// before 1970 reads as the epoch.
pub fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}
