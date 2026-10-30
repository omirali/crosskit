// Stages "background service" and "reads through the service": what a reader beside the
// running service gets, measured on a real data directory.
//
// A real service over the data directory the variables name. After its first pass:
//
// 1. without edits, `search --json` every quarter of a second — the share of `busy`, and how
//    long an answer takes through the command (which asks the service's socket first);
// 2. the socket asked directly, the way the panel asks it: one client, then four at once;
// 3. with an edit under one of the context's roots every fifteen seconds, the same query loop,
//    and for each edit the time until `search` finds the word it wrote;
//
// with the service's processor time and peak memory over the edits. Run it on a copy of the
// data directory, never on the one in use: the service migrates and writes the graphs it
// opens. Nothing is printed but counts and milliseconds.
//
//   CROSSKIT_BUSY_DATA_DIR=<copy of a data directory> CROSSKIT_BUSY_CONTEXT=work \
//   CROSSKIT_BUSY_EDITS=<a directory inside one of that context's roots> \
//   [CROSSKIT_BUSY_MINUTES=10] [CROSSKIT_BUSY_QUERY=readme] \
//     cargo test --release --offline --test service_busy -- --ignored --nocapture
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const QUERY_EVERY: Duration = Duration::from_millis(250);
const EDIT_EVERY: Duration = Duration::from_secs(15);
const SOCKET_PHASE: Duration = Duration::from_secs(60);
const FIRST_PASS_WAIT: Duration = Duration::from_secs(3600);
const FOUND_WAIT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Rows,
    Busy,
    Other,
}

struct Sample {
    at: Instant,
    answer: Answer,
    took: Duration,
}

#[test]
#[ignore = "measurement: readers beside a running service on a real data directory"]
fn readers_beside_the_service() {
    let (Some(data), Some(context), Some(edits)) = (
        std::env::var_os("CROSSKIT_BUSY_DATA_DIR").map(PathBuf::from),
        std::env::var("CROSSKIT_BUSY_CONTEXT").ok(),
        std::env::var_os("CROSSKIT_BUSY_EDITS").map(PathBuf::from),
    ) else {
        eprintln!(
            "set CROSSKIT_BUSY_DATA_DIR to a copy of a data directory, CROSSKIT_BUSY_CONTEXT to \
             one of its contexts and CROSSKIT_BUSY_EDITS to a directory under one of that \
             context's roots; nothing was measured"
        );
        return;
    };
    let minutes: u64 = std::env::var("CROSSKIT_BUSY_MINUTES")
        .ok()
        .and_then(|minutes| minutes.parse().ok())
        .unwrap_or(10);
    let query = std::env::var("CROSSKIT_BUSY_QUERY").unwrap_or_else(|_| "readme".to_string());
    let phase = Duration::from_secs(minutes * 60);

    let started = Instant::now();
    let service = start_service(&data);
    wait_for_first_pass(&data, &context);
    println!(
        "first pass of {context} done after {:.0} s",
        started.elapsed().as_secs_f64()
    );

    let quiet = ask_for(&data, &context, &query, phase);
    report("without edits, through the command", &quiet, phase);

    socket_latency(&data, &context, &query, 1);
    socket_latency(&data, &context, &query, 4);

    let pid = service.id();
    let cpu_before = processor_seconds(pid);
    let peak = Arc::new(Mutex::new(0u64));
    let sampling = Arc::new(AtomicBool::new(true));
    let sampler = {
        let (peak, sampling) = (Arc::clone(&peak), Arc::clone(&sampling));
        thread::spawn(move || {
            while sampling.load(Ordering::SeqCst) {
                let rss = resident_kib(pid);
                let mut peak = peak.lock().unwrap();
                *peak = (*peak).max(rss);
                drop(peak);
                thread::sleep(Duration::from_secs(1));
            }
        })
    };
    let stop = Arc::new(AtomicBool::new(false));
    let editing = {
        let (data, context, edits, stop) = (
            data.clone(),
            context.clone(),
            edits.clone(),
            Arc::clone(&stop),
        );
        thread::spawn(move || edit_and_find(&data, &context, &edits, &stop))
    };
    let edited = ask_for(&data, &context, &query, phase);
    stop.store(true, Ordering::SeqCst);
    let found = editing.join().unwrap();
    sampling.store(false, Ordering::SeqCst);
    sampler.join().unwrap();
    report("an edit every 15 s, through the command", &edited, phase);
    report_found(&found);
    println!(
        "service over the edits: {:.1} s of processor time in {} min, peak resident {} MiB",
        processor_seconds(pid) - cpu_before,
        minutes,
        *peak.lock().unwrap() / 1024
    );

    stop_service(service);
}

fn crosskit(data: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crosskit"));
    command
        .env("CROSSKIT_DATA_DIR", data)
        .env_remove("CROSSKIT_CONFIG")
        .env("NO_COLOR", "1");
    command
}

fn start_service(data: &Path) -> Child {
    crosskit(data)
        .args(["service", "run"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the service starts")
}

/// The journal says when a context's first pass is over, whatever it found.
fn wait_for_first_pass(data: &Path, context: &str) {
    let journal = data.join("service").join("service.log");
    let needle = format!("{context}: Pass 1 (bootstrap)");
    let deadline = Instant::now() + FIRST_PASS_WAIT;
    while !fs::read_to_string(&journal).is_ok_and(|text| text.contains(&needle)) {
        assert!(Instant::now() < deadline, "no first pass within the wait");
        thread::sleep(Duration::from_secs(1));
    }
}

fn search(data: &Path, context: &str, query: &str) -> (Answer, String) {
    let output = crosskit(data)
        .args([
            "--context",
            context,
            "search",
            query,
            "--json",
            "--limit",
            "8",
        ])
        .output()
        .expect("search runs");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let answer = if text.contains("\"category\":\"busy\"") {
        Answer::Busy
    } else if output.status.success() && text.contains("\"schema\"") {
        Answer::Rows
    } else {
        Answer::Other
    };
    (answer, text)
}

fn ask_for(data: &Path, context: &str, query: &str, phase: Duration) -> Vec<Sample> {
    let end = Instant::now() + phase;
    let mut samples = Vec::new();
    while Instant::now() < end {
        let at = Instant::now();
        let (answer, _) = search(data, context, query);
        samples.push(Sample {
            at,
            answer,
            took: at.elapsed(),
        });
        thread::sleep(QUERY_EVERY.saturating_sub(at.elapsed()));
    }
    samples
}

/// One question on the socket, the way the panel asks it: no process, one line each way.
fn ask_socket(service: &Path, context: &str, query: &str) -> Option<Duration> {
    let started = Instant::now();
    let mut stream = UnixStream::connect(service.join("crosskit.sock"))
        .or_else(|_| UnixStream::connect("crosskit.sock"))
        .ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    let request = format!(
        "{{\"protocol\":1,\"op\":\"search\",\"context\":\"{context}\",\"arguments\":[\"{query}\",\"--json\",\"--limit\",\"8\"]}}\n"
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).ok()?;
    (line.contains("\"results\"") && !line.contains("\"error\"")).then(|| started.elapsed())
}

/// The socket asked by `clients` at once for a minute.
fn socket_latency(data: &Path, context: &str, query: &str, clients: usize) {
    let service = data.join("service");
    // The full path of the socket does not fit a socket address in a long directory, so the
    // bare name is asked from inside it; this test is the only thing in its process.
    std::env::set_current_dir(&service).unwrap();
    let end = Instant::now() + SOCKET_PHASE;
    let results: Vec<Option<Duration>> = thread::scope(|scope| {
        // Spawned all first and joined after: joining each as it is spawned would run the
        // clients one after another.
        let handles: Vec<_> = (0..clients)
            .map(|_| {
                let service = &service;
                scope.spawn(move || {
                    let mut taken = Vec::new();
                    while Instant::now() < end {
                        let answer = ask_socket(service, context, query);
                        if answer.is_none() {
                            // Not answered: counted, and not asked again at once.
                            thread::sleep(Duration::from_millis(50));
                        }
                        taken.push(answer);
                    }
                    taken
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    let mut answered: Vec<f64> = results
        .iter()
        .flatten()
        .map(|took| took.as_secs_f64() * 1000.0)
        .collect();
    answered.sort_by(f64::total_cmp);
    println!(
        "socket, {clients} client(s) for {} s: {} asked, {} not answered, p50 {:.1} ms, p95 {:.1} ms",
        SOCKET_PHASE.as_secs(),
        results.len(),
        results.iter().filter(|result| result.is_none()).count(),
        percentile(&answered, 0.5),
        percentile(&answered, 0.95)
    );
}

/// Every fifteen seconds a new word in a file under a root, and the time until `search` finds
/// it — the delay a person sees between saving a file and finding it.
fn edit_and_find(
    data: &Path,
    context: &str,
    edits: &Path,
    stop: &AtomicBool,
) -> Vec<Option<Duration>> {
    let file = edits.join("busy-edit.md");
    let mut found = Vec::new();
    let mut number = 0;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        number += 1;
        let word = format!("quetzal{}x{number}", std::process::id());
        let mut out = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .expect("the edited file opens");
        writeln!(out, "{word}").expect("the edit is written");
        drop(out);
        let deadline = started + FOUND_WAIT;
        let mut seen = None;
        while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
            let (_, text) = search(data, context, &word);
            if text.contains("busy-edit.md") {
                seen = Some(started.elapsed());
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        found.push(seen);
        thread::sleep(EDIT_EVERY.saturating_sub(started.elapsed()));
    }
    found
}

fn report(phase: &str, samples: &[Sample], length: Duration) {
    let count = |wanted: Answer| samples.iter().filter(|s| s.answer == wanted).count();
    let (rows, busy, other) = (
        count(Answer::Rows),
        count(Answer::Busy),
        count(Answer::Other),
    );
    let mut answered: Vec<f64> = samples
        .iter()
        .filter(|sample| sample.answer == Answer::Rows)
        .map(|sample| sample.took.as_secs_f64() * 1000.0)
        .collect();
    answered.sort_by(f64::total_cmp);
    let mut waits: Vec<f64> = samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.answer == Answer::Busy)
        .filter_map(|(index, sample)| {
            samples[index..]
                .iter()
                .find(|later| later.answer == Answer::Rows)
                .map(|later| (later.at - sample.at).as_secs_f64())
        })
        .collect();
    waits.sort_by(f64::total_cmp);
    println!(
        "{phase} ({} min): {} queries, {rows} answered, {busy} busy ({:.1} %), {other} other",
        length.as_secs() / 60,
        samples.len(),
        100.0 * busy as f64 / samples.len().max(1) as f64
    );
    println!(
        "  answered: p50 {:.0} ms, p95 {:.0} ms",
        percentile(&answered, 0.5),
        percentile(&answered, 0.95)
    );
    if !waits.is_empty() {
        println!(
            "  busy → next answer: p50 {:.1} s, p95 {:.1} s, max {:.1} s",
            percentile(&waits, 0.5),
            percentile(&waits, 0.95),
            waits.last().copied().unwrap_or(0.0)
        );
    }
}

fn report_found(found: &[Option<Duration>]) {
    let mut seconds: Vec<f64> = found.iter().flatten().map(Duration::as_secs_f64).collect();
    seconds.sort_by(f64::total_cmp);
    println!(
        "edit → found: {} edits, {} not found within {} s, p50 {:.1} s, p95 {:.1} s, max {:.1} s",
        found.len(),
        found.iter().filter(|found| found.is_none()).count(),
        FOUND_WAIT.as_secs(),
        percentile(&seconds, 0.5),
        percentile(&seconds, 0.95),
        seconds.last().copied().unwrap_or(0.0)
    );
}

fn percentile(sorted: &[f64], share: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * share).round() as usize]
}

/// The processor time `ps` reports for the process, user and system together.
fn processor_seconds(pid: u32) -> f64 {
    let output = Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .expect("ps runs");
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .split(':')
        .rev()
        .enumerate()
        .map(|(position, part)| part.parse::<f64>().unwrap_or(0.0) * 60f64.powi(position as i32))
        .sum()
}

fn resident_kib(pid: u32) -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

fn stop_service(mut service: Child) {
    let pid = service.id().to_string();
    Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("kill runs");
    let deadline = Instant::now() + Duration::from_secs(300);
    while service.try_wait().expect("the service is asked").is_none() {
        assert!(Instant::now() < deadline, "the service did not stop");
        thread::sleep(Duration::from_millis(200));
    }
}
