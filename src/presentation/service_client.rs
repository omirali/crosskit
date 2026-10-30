// A reading command's question to the background service, before it opens anything itself.
//
// Only the machine answers ask: they are what the panel and other programs read, and they
// are the ones a pass beside them turned into `busy`. Whatever the service does not answer
// with an answer of its own — no service, another protocol, a context it does not hold, a
// configuration it did not read — the command answers the way it always has. A refusal the
// service gives as a machine answer (an unknown context, a query that does not parse) is
// answered here again too: the command's own run says it with the sentence for a person on
// stderr, which a line from the socket does not carry.
use std::time::Duration;

use crate::infrastructure::config_file;
use crate::infrastructure::service::{
    PROTOCOL, Served, ServiceLayout, ServiceRequest, ask_service, content_digest,
};

/// As long as the panel waits for a process today.
const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// The service's answer line to `op`, or nothing when the command should answer by itself.
pub(super) fn answered(op: &str, arguments: &[String], context: Option<&str>) -> Option<String> {
    asked(op, arguments, context).filter(|line| !is_a_refusal(line))
}

/// The service's answer line to `op` whatever it says, refusals included, or nothing when the
/// service did not answer at all. For a command whose refusal from the service carries what it
/// needs to be worded, and whose own run could only say the graph is busy.
pub(super) fn asked(op: &str, arguments: &[String], context: Option<&str>) -> Option<String> {
    let layout = ServiceLayout::resolve().ok()?;
    let request = ServiceRequest {
        protocol: PROTOCOL,
        op: op.to_string(),
        context: context.map(str::to_string),
        arguments: arguments.to_vec(),
        config_digest: config_file()
            .ok()
            .and_then(|path| content_digest(&path).ok()),
    };
    match ask_service(&layout.service_directory(), &request, ANSWER_WAIT) {
        Served::Answered(line) => Some(line),
        Served::Refused(_) | Served::Absent => None,
    }
}

fn is_a_refusal(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .map_or(true, |value| value.get("error").is_some())
}
