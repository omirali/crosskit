// The only place in this program that touches the network.
//
// It is a port of its own, below the provider, for one reason: every other test of the
// model path has to be able to run without a network and without a key, and a test that
// needs the real service proves nothing about the code around it. What crosses this
// boundary is a request body in and a response body out — or a category, never a status
// line, a header or a word the service wrote.
use std::time::Duration;

use ureq::Agent;

use crate::domain::InsightFailureCategory;

/// One call to the model service.
pub(super) trait MessageTransport {
    fn post_message(&self, body: &str) -> Result<String, InsightFailureCategory>;
}

/// How long one call may take in total, and how long it may spend only getting connected.
/// Both are set explicitly: a worker that hangs on a socket holds its claim until the lease
/// runs out, which turns one slow call into a job that looks abandoned.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The largest answer that is read. A reply to this question is a few hundred bytes; the
/// limit is what stops a misdirected endpoint from streaming a file into memory.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

const API_VERSION: &str = "2023-06-01";

pub(super) struct HttpsTransport {
    agent: Agent,
    endpoint: String,
    api_key: String,
}

impl HttpsTransport {
    pub(super) fn new(endpoint: String, api_key: String) -> Self {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(CALL_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            // An API call is not a page: a redirect here means the request did not reach
            // what it was addressed to, and following it would send the content somewhere
            // nobody configured.
            .max_redirects(0)
            .build()
            .into();
        Self {
            agent,
            endpoint,
            api_key,
        }
    }
}

impl MessageTransport for HttpsTransport {
    fn post_message(&self, body: &str) -> Result<String, InsightFailureCategory> {
        let response = self
            .agent
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .send(body);

        match response {
            Ok(mut answer) => answer
                .body_mut()
                .with_config()
                .limit(MAX_RESPONSE_BYTES)
                .read_to_string()
                .map_err(|_| InsightFailureCategory::Unavailable),
            Err(ureq::Error::StatusCode(code)) => Err(category_of(code)),
            // A timeout, a refused connection, a name that would not resolve, a TLS failure:
            // every one of them is "not now", and the next run tries again.
            Err(_) => Err(InsightFailureCategory::Unavailable),
        }
    }
}

/// What an HTTP status means for a job.
///
/// 429 and every 5xx — the service's own overload code among them — are the service asking
/// to be tried later, so the job goes back in the queue. 413 is the one refusal that is
/// about this revision and no other: it is too large, and it will be just as large next
/// time. Everything else the service rejects is this program or its configuration being
/// wrong, which no retry fixes either, but which is not the revision's fault; it is
/// reported as the handler failing so an operator sees it after the attempts rather than
/// finding jobs quietly retired.
fn category_of(status: u16) -> InsightFailureCategory {
    match status {
        429 | 500..=599 => InsightFailureCategory::Unavailable,
        413 => InsightFailureCategory::InputTooLarge,
        _ => InsightFailureCategory::Handler,
    }
}

#[cfg(test)]
mod tests {
    use super::category_of;
    use crate::domain::{FailureDisposition, InsightFailureCategory};

    #[test]
    fn a_service_that_asks_to_be_tried_later_is_temporary() {
        for status in [429, 500, 502, 503, 529] {
            assert_eq!(
                category_of(status),
                InsightFailureCategory::Unavailable,
                "{status}"
            );
            assert_eq!(
                category_of(status).disposition(),
                FailureDisposition::Retry,
                "{status}"
            );
        }
    }

    #[test]
    fn a_request_the_service_calls_too_large_is_about_this_revision_for_good() {
        assert_eq!(category_of(413), InsightFailureCategory::InputTooLarge);
        assert_eq!(
            category_of(413).disposition(),
            FailureDisposition::Permanent
        );
    }

    #[test]
    fn everything_else_the_service_rejects_is_reported_as_the_handler_failing() {
        for status in [400, 401, 403, 404] {
            assert_eq!(
                category_of(status),
                InsightFailureCategory::Handler,
                "{status}"
            );
        }
    }
}
