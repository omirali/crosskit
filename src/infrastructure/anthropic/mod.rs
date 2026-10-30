// The model provider: everything that knows a vendor, a protocol or a model name lives
// under here and nowhere else. The application above it sees a port that takes content and
// answers with a value or a category.
mod answer_provider;
mod https_transport;
mod topic_provider;

use crate::application::{AnswerProvider, InsightProvider};

pub use answer_provider::ASK_PROMPT_VERSION;
use answer_provider::AnswerProviderOverHttps;
use https_transport::HttpsTransport;
pub use topic_provider::PROMPT_VERSION;
use topic_provider::TopicProvider;

/// Where requests go. Configurable only through this constant: a program that could be
/// pointed at another endpoint by configuration is a program whose contexts' content can
/// be redirected by editing a file.
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// The environment variable the key is read from. The key is never read from the policy
/// file: a configuration file is copied, diffed and pasted, and a key in one is a key that
/// has left already.
pub const API_KEY_VARIABLE: &str = "ANTHROPIC_API_KEY";

/// What the operator chose. The model is configurable because a first slice should not
/// decide for a whole installation which model is worth its bulk work, and the effort level
/// with it, because not every model accepts one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSettings {
    model_id: String,
    effort: String,
}

impl ModelSettings {
    pub fn new(model_id: String, effort: String) -> Self {
        Self { model_id, effort }
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// The provider for these settings, or nothing when no key is configured.
///
/// Returning nothing rather than a provider that fails on every call is deliberate: a
/// worker with no key must not claim jobs, count attempts and retire them. The caller is
/// told there is no provider before a single job moves.
pub fn configured_provider(settings: &ModelSettings) -> Option<Box<dyn InsightProvider>> {
    let api_key = std::env::var(API_KEY_VARIABLE)
        .ok()
        .filter(|key| !key.is_empty())?;
    Some(Box::new(TopicProvider::new(
        HttpsTransport::new(ENDPOINT.to_string(), api_key),
        settings.model_id.clone(),
        settings.effort.clone(),
    )))
}

/// The provider that answers questions, for these settings, or nothing when no key is
/// configured. The same key, the same endpoint and the same transport as the worker's: one
/// place in this program knows a vendor.
pub fn configured_answer_provider(settings: &ModelSettings) -> Option<Box<dyn AnswerProvider>> {
    let api_key = std::env::var(API_KEY_VARIABLE)
        .ok()
        .filter(|key| !key.is_empty())?;
    Some(Box::new(AnswerProviderOverHttps::new(
        HttpsTransport::new(ENDPOINT.to_string(), api_key),
        settings.model_id.clone(),
        settings.effort.clone(),
    )))
}
