// Stage W measurement: what one FSEvents stream costs to start on this machine, outside the
// product.
//
// The watcher's subscription took 16 s to register its roots and another 22 s to probe
// delivery on the owner's machine, while the probe's own wait is bounded at 3 s. Each of
// those steps starts an FSEvents stream, so this asks the system directly: every call
// `notify` makes to start and stop a stream, timed one by one, and the time until the first
// event — once through `notify` itself, once through the framework with no crate between.
//
// Ignored: it measures the machine, not the code. Run it with
//
//     cargo test --offline --test fsevents_startup -- --ignored --nocapture
//
// and, to try another place, `FSEVENTS_STARTUP_DIR=<directory>` — a fresh directory is made
// inside it and removed afterwards. Nothing is printed about the place but what was named.
#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

const EVENT_WAIT: Duration = Duration::from_secs(60);

fn fresh_directory(name: &str) -> PathBuf {
    let parent = std::env::var_os("FSEVENTS_STARTUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let directory = parent.join(format!("crosskit-fsevents-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    fs::canonicalize(&directory).unwrap()
}

#[test]
#[ignore = "measures this machine's FSEvents, not the code"]
fn notify_watch_and_first_event() {
    let directory = fresh_directory("notify");
    let (sender, receiver) = mpsc::channel();
    let start = Instant::now();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    watcher.watch(&directory, RecursiveMode::Recursive).unwrap();
    let watched = start.elapsed();
    let written = Instant::now();
    fs::write(directory.join("probe.txt"), "probe").unwrap();
    let event = receiver.recv_timeout(EVENT_WAIT);
    println!(
        "notify: watch() {watched:?}, write → first event {:?} ({})",
        written.elapsed(),
        if event.is_ok() { "arrived" } else { "none" }
    );
    let stopping = Instant::now();
    drop(watcher);
    println!("notify: drop (stream stop) {:?}", stopping.elapsed());
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
#[ignore = "measures this machine's FSEvents, not the code"]
fn framework_calls_one_by_one() {
    let directory = fresh_directory("framework");
    let timings = framework::start_write_stop(&directory);
    for (step, took) in timings {
        println!("framework: {step:<38} {took:?}");
    }
    fs::remove_dir_all(&directory).unwrap();
}

/// The calls `notify` 8.2 makes, in its order, with nothing in between.
mod framework {
    use super::*;

    type CFRef = *const c_void;
    type FSEventStreamRef = *mut c_void;

    #[repr(C)]
    struct FSEventStreamContext {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
    }

    type Callback =
        extern "C" fn(FSEventStreamRef, *mut c_void, usize, *mut c_void, *const u32, *const u64);

    const UTF8: u32 = 0x0800_0100;
    const SINCE_NOW: u64 = u64::MAX;
    const FILE_EVENTS_NO_DEFER: u32 = 0x10 | 0x02;

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        static kCFAllocatorDefault: CFRef;
        static kCFRunLoopDefaultMode: CFRef;
        static kCFTypeArrayCallBacks: c_void;
        fn CFStringCreateWithBytes(a: CFRef, b: *const u8, n: isize, e: u32, x: bool) -> CFRef;
        fn CFArrayCreate(a: CFRef, v: *const CFRef, n: isize, cb: *const c_void) -> CFRef;
        fn CFRunLoopGetCurrent() -> CFRef;
        fn CFRunLoopRunInMode(mode: CFRef, seconds: f64, once: bool) -> i32;
        fn FSEventStreamCreate(
            a: CFRef,
            cb: Callback,
            c: *const FSEventStreamContext,
            paths: CFRef,
            since: u64,
            latency: f64,
            flags: u32,
        ) -> FSEventStreamRef;
        fn FSEventStreamScheduleWithRunLoop(s: FSEventStreamRef, rl: CFRef, mode: CFRef);
        fn FSEventStreamStart(s: FSEventStreamRef) -> bool;
        fn FSEventStreamStop(s: FSEventStreamRef);
        fn FSEventsGetCurrentEventId() -> u64;
        fn FSEventStreamGetDeviceBeingWatched(s: FSEventStreamRef) -> i32;
        fn FSEventsPurgeEventsForDeviceUpToEventId(device: i32, id: u64) -> bool;
        fn FSEventStreamInvalidate(s: FSEventStreamRef);
        fn FSEventStreamRelease(s: FSEventStreamRef);
    }

    extern "C" fn callback(
        _stream: FSEventStreamRef,
        info: *mut c_void,
        _count: usize,
        _paths: *mut c_void,
        _flags: *const u32,
        _ids: *const u64,
    ) {
        // Safety: `info` is the sender `start_write_stop` keeps alive for the stream's life.
        let sender = unsafe { &*(info as *const Sender<()>) };
        let _ = sender.send(());
    }

    pub(super) fn start_write_stop(directory: &Path) -> Vec<(&'static str, Duration)> {
        let mut timings = Vec::new();
        let mut step = |name, since: &mut Instant| {
            timings.push((name, since.elapsed()));
            *since = Instant::now();
        };
        let (sender, receiver) = mpsc::channel::<()>();
        let context = FSEventStreamContext {
            version: 0,
            info: &sender as *const Sender<()> as *mut c_void,
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        let path = directory.to_str().unwrap();
        let mut since = Instant::now();
        // Safety: every pointer handed over lives until the stream is released below, and
        // the stream is scheduled, run and torn down on this one thread.
        unsafe {
            let string = CFStringCreateWithBytes(
                kCFAllocatorDefault,
                path.as_ptr(),
                path.len() as isize,
                UTF8,
                false,
            );
            let paths = CFArrayCreate(
                kCFAllocatorDefault,
                &string,
                1,
                &kCFTypeArrayCallBacks as *const c_void,
            );
            let stream = FSEventStreamCreate(
                kCFAllocatorDefault,
                callback,
                &context,
                paths,
                SINCE_NOW,
                0.0,
                FILE_EVENTS_NO_DEFER,
            );
            step("FSEventStreamCreate", &mut since);
            FSEventStreamScheduleWithRunLoop(stream, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
            step("FSEventStreamScheduleWithRunLoop", &mut since);
            let started = FSEventStreamStart(stream);
            step(
                if started {
                    "FSEventStreamStart (true)"
                } else {
                    "FSEventStreamStart (FALSE)"
                },
                &mut since,
            );
            fs::write(directory.join("probe.txt"), "probe").unwrap();
            let deadline = Instant::now() + EVENT_WAIT;
            let arrived = loop {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, true);
                if receiver.try_recv().is_ok() {
                    break true;
                }
                if Instant::now() > deadline {
                    break false;
                }
            };
            step(
                if arrived {
                    "write → first event"
                } else {
                    "write → no event within the wait"
                },
                &mut since,
            );
            FSEventStreamStop(stream);
            step("FSEventStreamStop", &mut since);
            let id = FSEventsGetCurrentEventId();
            step("FSEventsGetCurrentEventId", &mut since);
            let device = FSEventStreamGetDeviceBeingWatched(stream);
            FSEventsPurgeEventsForDeviceUpToEventId(device, id);
            step("FSEventsPurgeEventsForDeviceUpToEventId", &mut since);
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
            step("FSEventStreamInvalidate + Release", &mut since);
        }
        timings
    }
}
