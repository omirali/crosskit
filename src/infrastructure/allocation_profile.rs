// Counting what Crosskit's own code allocates. Peak resident set size says how much
// memory the process held, but it moves with the host's memory pressure — under pressure
// pages are compressed and stop being resident — so two numbers taken hours apart cannot
// be compared and a regression in the read, projection or marshalling path hides in the
// noise. This counter can be compared: it sums the live Rust allocations and keeps the
// highest that sum reached.
//
// What it does not count: the storage engine is C++ and takes its memory from the system
// allocator directly, never through this one. The number is Crosskit's own side of the
// process, not the process. It is a regression detector for the layers Stages 2Q and 2S
// worked on, not a replacement for a memory limit.
//
// Compiled only under the `allocation-profile` feature; a released Crosskit keeps the
// system allocator and reads nothing about allocation from anywhere.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The live total, the high-water mark it reached, and how many allocation calls raised
/// it. Relaxed ordering is enough: every counter is a running total whose final value does
/// not depend on the order two threads add to it.
pub struct AllocationCounters {
    live_bytes: AtomicUsize,
    peak_bytes: AtomicUsize,
    allocations: AtomicU64,
}

impl AllocationCounters {
    const fn new() -> Self {
        Self {
            live_bytes: AtomicUsize::new(0),
            peak_bytes: AtomicUsize::new(0),
            allocations: AtomicU64::new(0),
        }
    }

    /// An allocation call that added `bytes` to what is live. Zero is still a call: a
    /// `realloc` to the same size allocates without growing anything.
    fn grew_by(&self, bytes: usize) {
        let live = self.live_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak_bytes.fetch_max(live, Ordering::Relaxed);
        self.allocations.fetch_add(1, Ordering::Relaxed);
    }

    fn shrank_by(&self, bytes: usize) {
        self.live_bytes.fetch_sub(bytes, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> AllocationProfile {
        AllocationProfile {
            peak_bytes: self.peak_bytes.load(Ordering::Relaxed) as u64,
            live_bytes: self.live_bytes.load(Ordering::Relaxed) as u64,
            allocations: self.allocations.load(Ordering::Relaxed),
        }
    }
}

/// What the counters hold at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationProfile {
    pub peak_bytes: u64,
    pub live_bytes: u64,
    pub allocations: u64,
}

static COUNTERS: AllocationCounters = AllocationCounters::new();

/// The system allocator with the counters in front of it.
pub struct CountingAllocator;

// Every method hands the same pointer and layout to the system allocator it was given, and
// only adds the accounting around it.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            COUNTERS.grew_by(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            COUNTERS.grew_by(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        COUNTERS.shrank_by(layout.size());
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if moved.is_null() {
            // The old block is still live and still counted: nothing changed.
            return moved;
        }
        match new_size.checked_sub(layout.size()) {
            Some(growth) => COUNTERS.grew_by(growth),
            None => COUNTERS.shrank_by(layout.size() - new_size),
        }
        moved
    }
}

/// What this process has allocated so far.
pub fn profile() -> AllocationProfile {
    COUNTERS.snapshot()
}

/// One line on stderr, so that a measured command's stdout keeps carrying only its own
/// counters. The numbers are read before the line is built, because building it allocates.
pub fn report_to_stderr() {
    let profile = profile();
    eprintln!(
        "Allocation profile: peak {} bytes, live at exit {} bytes, allocations {}",
        profile.peak_bytes, profile.live_bytes, profile.allocations
    );
}

#[cfg(test)]
mod tests {
    use super::{AllocationCounters, profile};

    /// The rule, checked on counters of its own: the peak is the high-water mark of what
    /// is live, and it never follows the live total back down.
    #[test]
    fn the_peak_is_the_high_water_mark_of_what_is_live() {
        let counters = AllocationCounters::new();
        counters.grew_by(4_096);
        counters.grew_by(1_000_000);
        assert_eq!(counters.snapshot().live_bytes, 1_004_096);
        assert_eq!(counters.snapshot().peak_bytes, 1_004_096);

        counters.shrank_by(1_000_000);
        let after = counters.snapshot();
        assert_eq!(after.live_bytes, 4_096);
        assert_eq!(after.peak_bytes, 1_004_096);
        assert_eq!(after.allocations, 2);

        counters.grew_by(8_192);
        let regrown = counters.snapshot();
        assert_eq!(regrown.live_bytes, 12_288);
        assert_eq!(
            regrown.peak_bytes, 1_004_096,
            "the peak is not a live total"
        );
        assert_eq!(regrown.allocations, 3);
    }

    /// The global allocator is the one this process runs on. The count of allocation calls
    /// only ever grows, so it stays a true statement while other tests allocate in
    /// parallel, which no assertion about a live total would.
    #[test]
    fn the_process_allocates_through_the_counting_allocator() {
        let before = profile();
        let held = vec![7u8; 1_000_000];
        let during = profile();
        assert!(
            during.allocations > before.allocations,
            "{before:?} -> {during:?}"
        );
        assert!(during.peak_bytes >= 1_000_000, "{during:?}");
        drop(held);
    }
}
