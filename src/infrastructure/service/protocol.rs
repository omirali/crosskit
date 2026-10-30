// The service's protocol: one line of JSON asked, one line answered, over one connection.
//
// An answer that came from a command is the command's own machine answer (schema 2,
// `presentation/machine_answer.rs`), byte for byte what the CLI prints. An answer the service
// gives instead of running anything — it does not hold that context, it is full, it speaks
// another protocol — is a refusal object of its own, which a client takes as "go the way you
// went before".
use serde::{Deserialize, Serialize};

/// The version of the conversation. Grows when a request or a refusal changes meaning; the
/// answers themselves carry the machine answer's own `schema`.
pub const PROTOCOL: u32 = 1;

/// The longest request line read. A question is a few hundred bytes; anything longer is not a
/// question this program asks.
pub const REQUEST_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRequest {
    pub protocol: u32,
    /// `hello`, `contexts`, `search`, `graph`, `knowledge-map`, `tasks`, `task-speech` or
    /// `task-map`.
    pub op: String,
    /// What `--context` named, if anything.
    #[serde(default)]
    pub context: Option<String>,
    /// The command's own arguments, after its name, as the CLI was given them.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// The digest of the configuration the client read. When it is not what the service read,
    /// the service refuses rather than answering from a configuration the client no longer
    /// has.
    #[serde(default)]
    pub config_digest: Option<String>,
}

/// Why the service answered nothing of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Refused {
    ProtocolMismatch,
    /// The service does not hold that context's graph right now: it is starting, waiting or
    /// not one of its contexts.
    NotServed,
    /// As many questions are being answered as the service takes at once.
    Overloaded,
    /// The client read another configuration than the service did.
    ConfigurationChanged,
    /// Not a request: not JSON, too long, an operation this protocol does not have.
    Malformed,
}

#[derive(Debug, Serialize, Deserialize)]
struct RefusalLine {
    protocol: u32,
    refused: Refused,
}

pub fn refusal_line(refused: Refused) -> String {
    serde_json::to_string(&RefusalLine {
        protocol: PROTOCOL,
        refused,
    })
    .unwrap_or_else(|_| format!("{{\"protocol\":{PROTOCOL},\"refused\":\"malformed\"}}"))
}

/// The refusal in an answer line, if the line is one.
pub fn refusal_in(line: &str) -> Option<Refused> {
    serde_json::from_str::<RefusalLine>(line)
        .ok()
        .map(|refusal| refusal.refused)
}

#[cfg(test)]
mod tests {
    use super::{Refused, ServiceRequest, refusal_in, refusal_line};

    #[test]
    fn a_refusal_reads_back_and_a_machine_answer_is_not_one() {
        let line = refusal_line(Refused::NotServed);
        assert_eq!(line, r#"{"protocol":1,"refused":"not-served"}"#);
        assert_eq!(refusal_in(&line), Some(Refused::NotServed));
        assert_eq!(
            refusal_in(r#"{"schema":2,"context":"work","results":[]}"#),
            None
        );
    }

    #[test]
    fn a_request_without_optional_fields_reads() {
        let request: ServiceRequest =
            serde_json::from_str(r#"{"protocol":1,"op":"hello"}"#).unwrap();
        assert_eq!(request.op, "hello");
        assert!(request.arguments.is_empty());
    }
}
