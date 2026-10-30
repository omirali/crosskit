// The fifteen questions with a known answer, read at measurement time.
//
// The file is never built into the binary and no longer lives in the repository. It holds the
// owner's real paths, their real working questions and the names of their conversations, so
// it sits in a directory of the owner's choosing named by `CROSSKIT_SEARCH_QUALITY_DIR`,
// beside the report written from it. Without the variable the measurement says what to set and
// measures nothing. `no_fixture_is_embedded_in_the_program` still fails the build on any
// `include_str!` that would carry a fixture.
//
// The three-field line format is the one Stage 29's own measurement reads. It is read again
// here rather than shared, because that one is an integration test that drives the compiled
// binary and this is a unit test inside the library: there is no module both can see, and
// making the library export a fixture reader so two tests could share twenty lines would put
// the measurement into the product.
use std::path::PathBuf;

/// The variable naming the owner's directory that holds the question set and the report.
const SEARCH_QUALITY_VARIABLE: &str = "CROSSKIT_SEARCH_QUALITY_DIR";
const QUESTIONS_FILE: &str = "chats-questions.txt";

/// One question: how it was asked, which files answer it, and why those.
#[derive(Debug, Clone)]
pub(crate) struct Question {
    text: String,
    answers: Vec<String>,
    why: String,
}

impl Question {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn answers(&self) -> &[String] {
        &self.answers
    }

    pub(crate) fn why(&self) -> &str {
        &self.why
    }
}

/// The set, and whether the owner has confirmed it.
#[derive(Debug, Clone)]
pub(crate) struct QuestionSet {
    confirmed: bool,
    questions: Vec<Question>,
}

impl QuestionSet {
    /// The set from the owner's directory. The error says what to set or what failed, and is
    /// meant to be printed as it is.
    pub(crate) fn read() -> Result<Self, String> {
        let Some(directory) = std::env::var_os(SEARCH_QUALITY_VARIABLE) else {
            return Err(format!(
                "{SEARCH_QUALITY_VARIABLE} не задан: назовите каталог владельца, где лежит \
                 {QUESTIONS_FILE}. Ничего не измерено."
            ));
        };
        let path = PathBuf::from(directory).join(QUESTIONS_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}. Ничего не измерено.", path.display()))?;
        Ok(Self::parse(&text))
    }

    /// The set a text of the three-field format describes.
    pub(crate) fn parse(text: &str) -> Self {
        let mut confirmed = false;
        let mut questions = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("confirmed:") {
                confirmed = value.trim() == "yes";
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Three parts and not every `|`: the explanation is the last field and has bars
            // of its own in it.
            let mut parts = line.splitn(3, '|');
            let Some(text) = parts.next().map(str::trim).filter(|text| !text.is_empty()) else {
                continue;
            };
            let answers: Vec<String> = parts
                .next()
                .unwrap_or_default()
                .split(';')
                .map(|path| path.trim().to_string())
                .filter(|path| !path.is_empty())
                .collect();
            questions.push(Question {
                text: text.to_string(),
                answers,
                why: parts.next().unwrap_or_default().trim().to_string(),
            });
        }
        Self {
            confirmed,
            questions,
        }
    }

    /// Whether the owner has gone through the fifteen lines and said the named conversation is
    /// the right one. While this is false every number below is provisional and says so.
    pub(crate) fn confirmed(&self) -> bool {
        self.confirmed
    }

    pub(crate) fn questions(&self) -> &[Question] {
        &self.questions
    }
}

#[cfg(test)]
mod tests {
    use super::QuestionSet;

    /// A made-up set in the owner's format: the reader, not the owner's questions, is what a
    /// unit test can check. The owner's set is checked where it is read, by the measurement.
    const INVENTED_SET: &str = "\
# вопрос | ожидаемые файлы через ; | почему
confirmed: yes
где сверяли счета | /Users/alice/.claude/projects/-x/a.jsonl | имя беседы «Сверка счетов» | с палкой
про договор | /Users/alice/.claude/projects/-x/b.jsonl; /Users/alice/.claude/projects/-x/c.jsonl |
";

    #[test]
    fn a_set_is_read_as_question_answers_and_reason() {
        let set = QuestionSet::parse(INVENTED_SET);

        assert!(set.confirmed());
        assert_eq!(set.questions().len(), 2);
        let first = &set.questions()[0];
        assert_eq!(first.text(), "где сверяли счета");
        assert_eq!(
            first.answers(),
            ["/Users/alice/.claude/projects/-x/a.jsonl"]
        );
        assert_eq!(
            first.why(),
            "имя беседы «Сверка счетов» | с палкой",
            "the reason is the last field and keeps its own bars"
        );
        assert_eq!(set.questions()[1].answers().len(), 2);
    }

    #[test]
    fn a_set_without_the_confirmation_line_is_not_confirmed() {
        assert!(!QuestionSet::parse("про договор | /a/b.jsonl | почему\n").confirmed());
    }
}
