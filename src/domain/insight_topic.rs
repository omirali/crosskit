// What a model said a revision is about, after it has been made to fit.
//
// This is where a model's answer stops being an answer and becomes a value: the
// constructor is the only way in, and it refuses everything that is not a short single
// line with a confidence between none and certainty. A model that returns a paragraph, a
// document, an empty string or a confidence of 7 is refused here — which matters for more
// than tidiness, because whatever this type accepts is what gets stored, and a label with
// no ceiling is a way for file content to end up in the graph under another name.
use std::fmt;

/// The longest a topic may be. A label is a handful of words; anything longer is not a
/// label, whatever the model called it.
const MAX_LABEL_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Confidence(f64);

impl Confidence {
    /// How sure the model said it was, between 0 and 1. Anything outside that, and anything
    /// that is not a number at all, is not a confidence.
    pub fn parse(value: f64) -> Result<Self, TopicError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(TopicError::ConfidenceOutOfRange(value));
        }
        Ok(Self(value))
    }

    pub fn value(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RevisionTopic {
    label: String,
    confidence: Confidence,
}

impl RevisionTopic {
    pub fn parse(label: &str, confidence: f64) -> Result<Self, TopicError> {
        let label = label.trim();
        if label.is_empty() {
            return Err(TopicError::EmptyLabel);
        }
        if label.chars().count() > MAX_LABEL_CHARS {
            return Err(TopicError::LabelTooLong {
                chars: label.chars().count(),
                limit: MAX_LABEL_CHARS,
            });
        }
        // A label is one line. A newline is how a paragraph of the file would arrive here
        // wearing a label's name, and a control character is how it would disturb whatever
        // prints it later.
        if label
            .chars()
            .any(|character| character.is_control() || character == '\n' || character == '\r')
        {
            return Err(TopicError::LabelNotOneLine);
        }
        Ok(Self {
            label: label.to_string(),
            confidence: Confidence::parse(confidence)?,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn confidence(&self) -> Confidence {
        self.confidence
    }
}

/// Why an answer is not a topic. It names the shape that was wrong and never quotes what
/// arrived: the thing being refused may be file content, and a refusal is not a reason to
/// copy it somewhere else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TopicError {
    EmptyLabel,
    LabelTooLong { chars: usize, limit: usize },
    LabelNotOneLine,
    ConfidenceOutOfRange(f64),
}

impl fmt::Display for TopicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLabel => f.write_str("a topic label must not be empty"),
            Self::LabelTooLong { chars, limit } => write!(
                f,
                "a topic label is at most {limit} characters, received {chars}"
            ),
            Self::LabelNotOneLine => {
                f.write_str("a topic label must be a single line without control characters")
            }
            Self::ConfidenceOutOfRange(_) => {
                f.write_str("a confidence must be a number between 0 and 1")
            }
        }
    }
}

impl std::error::Error for TopicError {}

#[cfg(test)]
mod tests {
    use super::{Confidence, MAX_LABEL_CHARS, RevisionTopic, TopicError};

    #[test]
    fn a_topic_is_a_short_single_line_with_a_confidence() {
        let topic = RevisionTopic::parse("  quarterly budget planning  ", 0.82).unwrap();

        assert_eq!(topic.label(), "quarterly budget planning");
        assert_eq!(topic.confidence().value(), 0.82);
        assert_eq!(Confidence::parse(0.0).unwrap().value(), 0.0);
        assert_eq!(Confidence::parse(1.0).unwrap().value(), 1.0);
    }

    #[test]
    fn an_answer_that_is_not_a_label_is_refused() {
        assert_eq!(
            RevisionTopic::parse("   ", 0.5).unwrap_err(),
            TopicError::EmptyLabel
        );
        assert_eq!(
            RevisionTopic::parse("", 0.5).unwrap_err(),
            TopicError::EmptyLabel
        );
        // A whole document wearing a label's name: this is the refusal that keeps file
        // content out of the graph by the back door.
        let long = "a".repeat(MAX_LABEL_CHARS + 1);
        assert_eq!(
            RevisionTopic::parse(&long, 0.5).unwrap_err(),
            TopicError::LabelTooLong {
                chars: MAX_LABEL_CHARS + 1,
                limit: MAX_LABEL_CHARS,
            }
        );
        for many_lines in ["first line\nsecond line", "tab\tseparated", "bell\u{7}"] {
            assert_eq!(
                RevisionTopic::parse(many_lines, 0.5).unwrap_err(),
                TopicError::LabelNotOneLine,
                "{many_lines:?}"
            );
        }
    }

    #[test]
    fn a_confidence_outside_none_to_certainty_is_refused() {
        for outside in [-0.1, 1.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                matches!(
                    RevisionTopic::parse("budget", outside),
                    Err(TopicError::ConfidenceOutOfRange(_))
                ),
                "{outside} must be refused"
            );
        }
    }

    #[test]
    fn a_label_at_the_limit_is_accepted() {
        let exactly = "b".repeat(MAX_LABEL_CHARS);
        assert_eq!(
            RevisionTopic::parse(&exactly, 1.0).unwrap().label(),
            exactly
        );
    }
}
