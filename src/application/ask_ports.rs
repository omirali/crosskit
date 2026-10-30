// Ports through which a question reaches a model and comes back, and the values that
// cross them.
//
// Two of them carry the whole of this stage's boundary. The request holds the context's
// permit by value, so a context that refuses to let its content leave cannot build one —
// the object that would carry a question outside does not exist there. And an excerpt
// carries its number beside its path: the number is what the model is given, the path is
// what this program prints, and nothing converts one into the other except this list.
use crate::domain::{
    Answer, AskCacheKey, AskError, ContentEgressPermit, NormalizedPath, Period, Question,
};

use super::period_scope::PeriodEvent;
use super::search_ports::SearchError;

/// One excerpt as it is sent: its number, where it came from, and the text itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excerpt {
    ordinal: usize,
    chunk_id: String,
    path: NormalizedPath,
    text: String,
    /// What the analysis concluded this revision is about, when it was analysed. Carried
    /// beside the text and never inside it: a label glued into the excerpt would be
    /// indistinguishable from a sentence the file actually contains, and a model asked to
    /// answer from the file would be answering from us.
    topic: Option<String>,
    /// The event that put it inside the period the question was about, when it was about
    /// one. Printed beside the path and never sent: the model is told the period, and which
    /// moment admitted which excerpt is this program's evidence, not the model's.
    event: Option<PeriodEvent>,
}

impl Excerpt {
    /// The chunk this text came from. Names the source in a cache key without carrying it:
    /// the id is derived from the file's identity and the hash of its content.
    pub fn chunk_id(&self) -> &str {
        &self.chunk_id
    }

    /// One excerpt as it will be sent. `ordinal` is its place in the list, numbered from
    /// one, because that number is the only name the model is given for it.
    pub fn new(
        ordinal: usize,
        chunk_id: impl Into<String>,
        path: NormalizedPath,
        text: impl Into<String>,
        topic: Option<String>,
    ) -> Self {
        Self {
            ordinal,
            chunk_id: chunk_id.into(),
            path,
            text: text.into(),
            topic,
            event: None,
        }
    }

    /// The same excerpt, with the event that put it inside a period.
    pub fn admitted_by(mut self, event: Option<PeriodEvent>) -> Self {
        self.event = event;
        self
    }

    pub fn event(&self) -> Option<PeriodEvent> {
        self.event
    }

    pub fn ordinal(&self) -> usize {
        self.ordinal
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn topic(&self) -> Option<&str> {
        self.topic.as_deref()
    }
}

/// What is sent to a model. Holding the permit by value is what makes this type impossible
/// to build for a context that refuses to let its content leave.
pub struct AnswerRequest<'a> {
    question: &'a Question,
    excerpts: &'a [Excerpt],
    period: Option<&'a Period>,
    not_shown: Option<String>,
    /// Never read, and that is the point: it exists so this type cannot be constructed
    /// without the proof.
    _permit: ContentEgressPermit,
}

impl<'a> AnswerRequest<'a> {
    pub fn new(
        question: &'a Question,
        excerpts: &'a [Excerpt],
        permit: ContentEgressPermit,
    ) -> Self {
        Self {
            question,
            excerpts,
            period: None,
            not_shown: None,
            _permit: permit,
        }
    }

    /// The same request, for a question about a period, and what that period could not show
    /// (`PeriodGaps::sentence`), so the model does not present a short answer as a whole one.
    pub fn about(mut self, period: Option<&'a Period>, not_shown: Option<String>) -> Self {
        self.period = period;
        self.not_shown = not_shown;
        self
    }

    pub fn question(&self) -> &Question {
        self.question
    }

    /// The one line a model is told about the period, when the question is about one.
    ///
    /// The days, so it does not have to guess them from "last week"; that every excerpt is
    /// from them, so it does not answer about another week from its own knowledge; and that a
    /// date the excerpts do not hold is not to be named. One line for every engine, built
    /// here, so two engines cannot tell a model two different things.
    pub fn period_line(&self) -> Option<String> {
        self.period.map(|period| {
            let mut line = format!(
                "Period: the question is about {period}, local time. Every excerpt below has \
                 an event inside it. Do not name a date the excerpts do not contain."
            );
            if let Some(not_shown) = &self.not_shown {
                line.push(' ');
                line.push_str(not_shown);
                line.push_str(" Say that the answer may be incomplete for this reason.");
            }
            line
        })
    }

    pub fn excerpts(&self) -> &[Excerpt] {
        self.excerpts
    }
}

/// A model that answers a question from the excerpts it was given.
pub trait AnswerProvider {
    fn answer(&self, request: AnswerRequest<'_>) -> Result<Answer, AskFailure>;
}

/// As for the insight port: the chosen engine has to have one type.
impl AnswerProvider for Box<dyn AnswerProvider> {
    fn answer(&self, request: AnswerRequest<'_>) -> Result<Answer, AskFailure> {
        (**self).answer(request)
    }
}

/// What the same question with the same sources answered before.
pub trait AnswerCache {
    fn lookup(&self, key: &AskCacheKey) -> Result<Option<Answer>, AskFailure>;
    fn store(&self, key: &AskCacheKey, answer: &Answer) -> Result<(), AskFailure>;
}

/// Everything that can stop a question from being answered, as a category.
///
/// Nothing a provider wrote reaches this type: a body, a message or a prompt would be the
/// provider's words about this context's content, and none of them are anybody's business
/// downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskFailure {
    /// The index or the graph could not be read.
    Search(SearchError),
    /// The question or the answer is not one this program accepts.
    Shape(AskError),
    /// Nothing in this context matched, so there is nothing to answer from.
    NothingToAnswerFrom,
    /// The question was about a period and nothing inside it matched. How many files had
    /// any event in it tells "nothing happened then" from "nothing that happened matched".
    NothingInPeriod { files_with_events: usize },
    /// A provider could not be reached, was rate limited or was overloaded.
    Unavailable,
    /// What came back does not fit the schema this program accepts.
    Schema,
    /// What this program sent was refused for its size before any model saw it. Nothing
    /// came back, so nothing came back in a shape to have an opinion about.
    RequestTooLarge,
    /// The model declined to answer.
    Refused,
    /// The cache could not be read or written. The answer is unaffected.
    Cache,
}

impl std::fmt::Display for AskFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Search(error) => write!(f, "{error}"),
            Self::Shape(error) => write!(f, "{error}"),
            Self::NothingToAnswerFrom => {
                write!(f, "nothing in this context matched the question")
            }
            Self::NothingInPeriod { files_with_events } => write!(
                f,
                "nothing inside the period matched the question ({files_with_events} files have an event in it)"
            ),
            Self::Unavailable => write!(f, "the model could not be reached"),
            Self::Schema => write!(
                f,
                "the model answered in a shape this program does not accept"
            ),
            Self::RequestTooLarge => {
                write!(f, "the question was too large to send, so it was not sent")
            }
            Self::Refused => write!(f, "the model declined to answer"),
            Self::Cache => write!(f, "the answer cache could not be used"),
        }
    }
}

impl std::error::Error for AskFailure {}

#[cfg(test)]
mod tests {
    use super::AnswerRequest;
    use crate::domain::{CivilDate, ContentEgress, LocalDay, Period, PeriodPhrase, Question};

    /// The model is told the days, and only when the question is about some: a question
    /// without a period is sent exactly as before.
    #[test]
    fn the_model_is_told_the_period_in_one_line_and_only_when_there_is_one() {
        let question = Question::parse("над чем я работал на прошлой неделе?").unwrap();
        let permit = || ContentEgress::Allowed.permit().unwrap();
        let period = Period::of_phrase(
            PeriodPhrase::LastWeek,
            LocalDay::new(CivilDate::new(2026, 10, 1).unwrap(), 5 * 3600),
        );

        let line = AnswerRequest::new(&question, &[], permit())
            .about(Some(&period), None)
            .period_line()
            .unwrap();
        assert!(
            line.starts_with("Period: the question is about 21–27 Sep 2026, local time."),
            "{line}"
        );
        assert!(!line.contains('\n'));
        assert_eq!(
            AnswerRequest::new(&question, &[], permit()).period_line(),
            None
        );
    }

    /// What the period could not show reaches the model in the same line, and the model is
    /// asked to say the answer may be incomplete.
    #[test]
    fn the_model_is_told_what_the_period_could_not_show() {
        let question = Question::parse("что было на прошлой неделе?").unwrap();
        let period = Period::of_phrase(
            PeriodPhrase::LastWeek,
            LocalDay::new(CivilDate::new(2026, 10, 1).unwrap(), 5 * 3600),
        );
        let line = AnswerRequest::new(&question, &[], ContentEgress::Allowed.permit().unwrap())
            .about(
                Some(&period),
                Some("Not shown: 1 conversation(s).".to_string()),
            )
            .period_line()
            .unwrap();
        assert!(line.contains("Not shown: 1 conversation(s)."), "{line}");
        assert!(line.contains("may be incomplete"), "{line}");
        assert!(!line.contains('\n'));
    }
}
