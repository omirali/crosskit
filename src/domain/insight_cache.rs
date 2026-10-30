// What makes two analyses the same piece of work.
//
// An answer depends on five things and nothing else: the content that was sent, the
// pipeline that asked, the model that answered, the wording of the question and the types it
// let the answer choose from. Two
// revisions of two different files with byte-identical content share all four, so the
// second one is not paid for. Change any of them and it is a different question, so the
// key changes with it and the old answer is not reused.
//
// The key is composed rather than hashed. Every part is length-prefixed, so no part can
// end in a way that makes it look like the start of the next one, and the key stays
// readable in the graph without carrying a path or any of the content itself.
use std::fmt;

use super::cache_key::compose;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InsightCacheKey(String);

impl InsightCacheKey {
    /// The vocabulary is part of the question since the knowledge layer (ADR 0048): an answer
    /// chosen from one list of types must not be served for another list.
    pub fn of(
        input_content_hash: &str,
        pipeline_version: &str,
        model_id: &str,
        prompt_version: &str,
        vocabulary_fingerprint: &str,
    ) -> Self {
        Self(compose(
            "insight-cache",
            &[
                input_content_hash,
                pipeline_version,
                model_id,
                prompt_version,
                vocabulary_fingerprint,
            ],
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InsightCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::InsightCacheKey;

    fn key(hash: &str, pipeline: &str, model: &str, prompt: &str) -> String {
        InsightCacheKey::of(hash, pipeline, model, prompt, "vocabulary")
            .as_str()
            .to_string()
    }

    #[test]
    fn the_same_five_things_are_the_same_question() {
        assert_eq!(
            key("sha256:a", "insight-outbox-v1", "a-model", "v1"),
            key("sha256:a", "insight-outbox-v1", "a-model", "v1")
        );
    }

    #[test]
    fn changing_any_of_the_five_asks_a_different_question() {
        let base = key("sha256:a", "insight-outbox-v1", "a-model", "v1");

        for other in [
            key("sha256:b", "insight-outbox-v1", "a-model", "v1"),
            key("sha256:a", "insight-outbox-v2", "a-model", "v1"),
            key("sha256:a", "insight-outbox-v1", "other-model", "v1"),
            key("sha256:a", "insight-outbox-v1", "a-model", "v2"),
            InsightCacheKey::of("sha256:a", "insight-outbox-v1", "a-model", "v1", "other")
                .as_str()
                .to_string(),
        ] {
            assert_ne!(base, other);
        }
    }

    /// Without the length prefixes these two would compose to the same string, and one
    /// model's answers would be served for another's.
    #[test]
    fn a_part_can_never_be_read_as_the_start_of_the_next_one() {
        assert_ne!(
            key("sha256:a", "pipe", "modelv", "1"),
            key("sha256:a", "pipe", "model", "v1")
        );
    }

    #[test]
    fn a_key_carries_no_content_and_no_path() {
        let composed = key(
            "sha256:abc",
            "insight-outbox-v1",
            "a-model",
            "revision-topic-v1",
        );

        assert!(composed.starts_with("insight-cache:"));
        assert!(!composed.contains('/'), "{composed}");
    }
}
