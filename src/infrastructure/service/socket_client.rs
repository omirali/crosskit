// A client's side of the socket: ask once, within bounds, and say whether the service
// answered or the question should go the way it went before there was a service.
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use super::protocol::{Refused, ServiceRequest, refusal_in};
use super::socket_place::connect_socket;

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    /// The service's answer line.
    Answered(String),
    /// The service is there and answered nothing of its own.
    Refused(Refused),
    /// No service, or none that could be asked.
    Absent,
}

/// Asks `request` of the service under `service_directory`, waiting at most `wait` for the
/// answer.
pub fn ask_service(service_directory: &Path, request: &ServiceRequest, wait: Duration) -> Served {
    let Ok(stream) = connect_socket(service_directory) else {
        return Served::Absent;
    };
    let Ok(line) = serde_json::to_string(request) else {
        return Served::Absent;
    };
    if stream.set_read_timeout(Some(wait)).is_err() || stream.set_write_timeout(Some(wait)).is_err()
    {
        return Served::Absent;
    }
    let mut writer = &stream;
    if writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .is_err()
    {
        return Served::Absent;
    }
    let mut answer = String::new();
    match BufReader::new(&stream).read_line(&mut answer) {
        Ok(_) if answer.ends_with('\n') => {
            let answer = answer.trim_end_matches('\n').to_string();
            match refusal_in(&answer) {
                Some(refused) => Served::Refused(refused),
                None => Served::Answered(answer),
            }
        }
        _ => Served::Absent,
    }
}
