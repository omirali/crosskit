// The service's side of the socket: who may ask, how many at once, and how long anybody may
// take — so that a client that connects and says nothing, or never reads its answer, holds
// neither a pass nor any other client.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use super::protocol::{REQUEST_LIMIT, Refused, ServiceRequest, refusal_line};
use super::socket_place::{PlacedSocket, effective_uid, peer_uid};
use super::stop_broadcast::StopBroadcast;

/// What answers a request. Called on a thread of the connection's own.
pub trait ServedReads: Sync {
    fn answer(&self, request: &ServiceRequest) -> String;
}

/// The limits of one server.
#[derive(Debug, Clone, Copy)]
pub struct ServerLimits {
    /// Connections answered at once; the next one is told `overloaded` and closed.
    pub connections: usize,
    /// How long a client may take to say its one line.
    pub request_wait: Duration,
    /// How long a client may take to take its answer.
    pub answer_wait: Duration,
}

impl Default for ServerLimits {
    /// Eight at once: the panel asks one page at a time, and four clients in parallel is the
    /// load this stage measures. Two seconds to ask is a hundred times what a local client
    /// takes; ten to read the answer covers the largest one, a knowledge map.
    fn default() -> Self {
        Self {
            connections: 8,
            request_wait: Duration::from_secs(2),
            answer_wait: Duration::from_secs(10),
        }
    }
}

/// How long the accepting loop waits before it looks whether the service is stopping.
const STOP_LOOK: i32 = 250;

/// How long a refused client is given to say its question and to take the refusal.
const REFUSAL_WAIT: Duration = Duration::from_millis(100);

/// Answers connections until the service stops, then removes the socket.
pub fn serve(
    socket: PlacedSocket,
    reads: &dyn ServedReads,
    limits: ServerLimits,
    stop: &StopBroadcast,
    said: &(dyn Fn(&str) + Sync),
) {
    let active = Arc::new(AtomicUsize::new(0));
    thread::scope(|scope| {
        while !stop.wait(Duration::ZERO) {
            if !readable(socket.listener(), STOP_LOOK) {
                continue;
            }
            let stream = match socket.listener().accept() {
                Ok((stream, _)) => stream,
                Err(error) => {
                    said(&format!("A connection could not be accepted: {error}"));
                    continue;
                }
            };
            if peer_uid(&stream).ok() != Some(effective_uid()) {
                // Another user's process: no answer at all.
                continue;
            }
            if active.fetch_add(1, Ordering::SeqCst) >= limits.connections {
                active.fetch_sub(1, Ordering::SeqCst);
                refuse_at_once(stream, Refused::Overloaded);
                continue;
            }
            let active = Arc::clone(&active);
            scope.spawn(move || {
                answer_one(stream, reads, limits);
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    socket.remove();
}

/// One connection: a bounded wait for one bounded line, the answer, a bounded wait for it to
/// be taken. Whatever the client does, the thread ends within the two waits plus the answer.
fn answer_one(stream: UnixStream, reads: &dyn ServedReads, limits: ServerLimits) {
    if stream.set_read_timeout(Some(limits.request_wait)).is_err()
        || stream.set_write_timeout(Some(limits.answer_wait)).is_err()
    {
        return;
    }
    let mut line = String::new();
    let mut reader = BufReader::new((&stream).take(REQUEST_LIMIT));
    let answer = match reader.read_line(&mut line) {
        Ok(_) if line.ends_with('\n') => match serde_json::from_str::<ServiceRequest>(&line) {
            Ok(request) => reads.answer(&request),
            Err(_) => refusal_line(Refused::Malformed),
        },
        // Too long, cut off, or nothing said within the wait.
        Ok(_) | Err(_) => return,
    };
    let mut writer = &stream;
    // A client that went away or does not read is its own loss; nothing waits on this.
    let _ = writer
        .write_all(answer.as_bytes())
        .and_then(|()| writer.write_all(b"\n"));
}

/// `overloaded`, written without waiting: the line is far smaller than a socket's buffer, and
/// a client that does not read it is not waited for.
///
/// The question is read first, for a moment. A connection closed with the client's question
/// still on its way ends the client's write with an error, and the client gives up before it
/// reads the refusal waiting for it: on 2026-10-30 that was three runs in five of the test
/// below, and five in five on the commit before. A client that says nothing within the moment
/// is not waited for any longer.
fn refuse_at_once(stream: UnixStream, refused: Refused) {
    if stream.set_read_timeout(Some(REFUSAL_WAIT)).is_ok() {
        let mut question = String::new();
        let _ = BufReader::new((&stream).take(REQUEST_LIMIT)).read_line(&mut question);
    }
    if stream.set_write_timeout(Some(REFUSAL_WAIT)).is_ok() {
        let mut writer = &stream;
        let _ = writer.write_all(format!("{}\n", refusal_line(refused)).as_bytes());
    }
}

/// Whether a connection is waiting, within `timeout_ms`.
fn readable(listener: &std::os::unix::net::UnixListener, timeout_ms: i32) -> bool {
    let mut descriptor = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the duration of the call.
    unsafe { libc::poll(&mut descriptor, 1, timeout_ms) > 0 }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{ServedReads, ServerLimits, serve};
    use crate::infrastructure::service::protocol::{PROTOCOL, Refused, ServiceRequest};
    use crate::infrastructure::service::socket_client::{Served, ask_service};
    use crate::infrastructure::service::socket_place::{connect_socket, place_socket};
    use crate::infrastructure::service::stop_broadcast::StopBroadcast;

    /// Answers with the operation's name, after `delay`.
    struct Echo {
        delay: Duration,
    }

    impl ServedReads for Echo {
        fn answer(&self, request: &ServiceRequest) -> String {
            thread::sleep(self.delay);
            format!("{{\"schema\":2,\"op\":\"{}\"}}", request.op)
        }
    }

    fn request(op: &str) -> ServiceRequest {
        ServiceRequest {
            protocol: PROTOCOL,
            op: op.to_string(),
            context: None,
            arguments: Vec::new(),
            config_digest: None,
        }
    }

    fn running(
        name: &str,
        reads: &'static Echo,
        limits: ServerLimits,
        scenario: impl FnOnce(&std::path::Path) + Send,
    ) {
        let root =
            std::env::temp_dir().join(format!("crosskit-serve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let service = root.join("service");
        let socket = place_socket(&service).unwrap();
        let stop = StopBroadcast::default();
        thread::scope(|scope| {
            let stop_ref = &stop;
            scope.spawn(move || serve(socket, reads, limits, stop_ref, &|_| {}));
            // Stopped however the scenario ends: a failed assertion inside the scope would
            // otherwise leave the server waiting for a stop and the scope waiting for the
            // server, for ever.
            let _stopping = StopOnDrop(&stop);
            scenario(&service);
        });
        std::fs::remove_dir_all(&root).unwrap();
    }

    struct StopOnDrop<'a>(&'a StopBroadcast);

    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.stop();
        }
    }

    const QUICK: Echo = Echo {
        delay: Duration::ZERO,
    };

    /// A client that connects and says nothing holds neither the server nor anybody else, and
    /// is let go when its wait runs out.
    #[test]
    fn a_silent_client_holds_nobody_and_is_let_go() {
        static READS: Echo = QUICK;
        let limits = ServerLimits {
            request_wait: Duration::from_millis(300),
            ..ServerLimits::default()
        };
        running("silent", &READS, limits, |service| {
            let mut silent = connect_socket(service).unwrap();
            let started = Instant::now();
            assert_eq!(
                ask_service(service, &request("hello"), Duration::from_secs(5)),
                Served::Answered("{\"schema\":2,\"op\":\"hello\"}".to_string())
            );
            assert!(started.elapsed() < Duration::from_millis(300));
            silent
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut rest = Vec::new();
            // The server closes it: end of stream, with nothing said.
            assert_eq!(silent.read_to_end(&mut rest).unwrap(), 0);
        });
    }

    /// More clients at once than the limit: the extra one is told so at once, not queued.
    #[test]
    fn a_client_past_the_limit_is_told_overloaded_at_once() {
        // Long enough that the two first questions are still being answered when the third
        // arrives, however busy the machine running the tests is.
        static READS: Echo = Echo {
            delay: Duration::from_secs(3),
        };
        let limits = ServerLimits {
            connections: 2,
            ..ServerLimits::default()
        };
        running("overloaded", &READS, limits, |service| {
            let busy: Vec<UnixStream> = (0..2)
                .map(|_| {
                    let mut stream = connect_socket(service).unwrap();
                    // Set at once: on macOS a socket the server has already answered and
                    // closed refuses `setsockopt`, and a slow machine may get there first.
                    stream
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    stream
                        .write_all(b"{\"protocol\":1,\"op\":\"hello\"}\n")
                        .unwrap();
                    stream
                })
                .collect();
            thread::sleep(Duration::from_millis(500));
            let started = Instant::now();
            assert_eq!(
                ask_service(service, &request("hello"), Duration::from_secs(5)),
                Served::Refused(Refused::Overloaded)
            );
            // Told at once, not after the two questions ahead of it.
            assert!(started.elapsed() < Duration::from_secs(2));
            for stream in busy {
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                assert!(line.contains("hello"), "{line}");
            }
        });
    }

    #[test]
    fn a_line_that_is_not_a_request_is_refused_as_malformed() {
        static READS: Echo = QUICK;
        running("malformed", &READS, ServerLimits::default(), |service| {
            let mut stream = connect_socket(service).unwrap();
            stream.write_all(b"not json\n").unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            assert!(line.contains("\"refused\":\"malformed\""), "{line}");
        });
    }

    /// No service: the client says so at once instead of waiting.
    #[test]
    fn without_a_service_the_client_answers_absent_at_once() {
        let service =
            std::env::temp_dir().join(format!("crosskit-no-service-{}", std::process::id()));
        let started = Instant::now();
        assert_eq!(
            ask_service(&service, &request("hello"), Duration::from_secs(5)),
            Served::Absent
        );
        assert!(started.elapsed() < Duration::from_millis(200));
    }
}
