// Monotonic wall time for the watch loop's deadlines.
use std::time::Instant;

use crate::application::WatchClock;

pub struct SystemClock;

impl WatchClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}
