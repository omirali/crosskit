// Exactly what would go to a model for the part no free signal decided — assembled and weighed,
// never sent.
//
// Names only: of a project its directory's name, of a folder its name, of a conversation its
// title. No path, no file list, no line of content. Beside the questions go the groups the rule
// already decided, by name, as the pattern the model is to follow. The text is built here in
// full so that its size is the size of a real request rather than an estimate of one; nothing
// in this program has a way to send it, and this file does not reach for one.
//
// What is asked is the least that has an answer of its own. A conversation filed under a
// project that is itself a question is not asked about: its answer is the project's.
use super::conversation_placement::{PlacedConversation, PlacedFolder};
use super::placement::Repositories;
use crate::domain::context_split::{AskBecause, ContextSide, SplitDecision};

/// What one question is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Asked {
    Project,
    Folder,
    Conversation,
}

impl Asked {
    fn word(self) -> &'static str {
        match self {
            Self::Project => "проект",
            Self::Folder => "папка",
            Self::Conversation => "беседа",
        }
    }
}

/// The request that would be made, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelRemainder {
    text: String,
    questions: usize,
    examples: usize,
}

impl ModelRemainder {
    pub(super) fn assemble(
        examples: &[(ContextSide, String)],
        questions: &[(Asked, String)],
    ) -> Self {
        let mut text = String::from(
            "Разложи имена по двум контекстам: work или personal; skip — если это ничья работа.\n",
        );
        text.push_str("Уже разложено, образец:\n");
        for side in [ContextSide::Work, ContextSide::Personal] {
            let names: Vec<&str> = examples
                .iter()
                .filter(|(of, _)| *of == side)
                .map(|(_, name)| name.as_str())
                .collect();
            if !names.is_empty() {
                text.push_str(&format!("{}: {}\n", side.as_str(), names.join(", ")));
            }
        }
        text.push_str("Ответь строкой на номер: <номер> work|personal|skip\n");
        for (number, (asked, name)) in questions.iter().enumerate() {
            text.push_str(&format!("{}. {}: {name}\n", number + 1, asked.word()));
        }
        Self {
            text,
            questions: questions.len(),
            examples: examples.len(),
        }
    }

    pub(super) fn bytes(&self) -> usize {
        self.text.len()
    }

    pub(super) fn questions(&self) -> usize {
        self.questions
    }

    pub(super) fn examples(&self) -> usize {
        self.examples
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }
}

/// The request for this machine, and how many conversations could not be in it because they
/// have no name to ask by.
pub(super) fn remainder_of(
    repositories: &Repositories,
    folders: &[PlacedFolder],
    conversations: &[PlacedConversation],
) -> (ModelRemainder, usize) {
    let examples: Vec<(ContextSide, String)> = repositories
        .outermost()
        .filter_map(|placed| {
            let side = placed.effective.side()?;
            Some((side, placed.found.root.file_name().to_string()))
        })
        .collect();
    let mut questions: Vec<(Asked, String)> = repositories
        .outermost()
        .filter(|placed| placed.effective.side().is_none())
        .map(|placed| (Asked::Project, placed.found.root.file_name().to_string()))
        .collect();
    questions.extend(
        folders
            .iter()
            .filter(|placed| placed.decision.side().is_none())
            .map(|placed| (Asked::Folder, placed.folder.path.file_name().to_string())),
    );
    let mut untitled = 0;
    for placed in conversations {
        if !asks_on_its_own(&placed.decision) {
            continue;
        }
        match &placed.record.title {
            Some(title) => questions.push((Asked::Conversation, title.clone())),
            None => untitled += 1,
        }
    }
    (ModelRemainder::assemble(&examples, &questions), untitled)
}

/// Whether a thing's question is its own. One that follows a question asked above is answered
/// by that answer and is not asked twice.
fn asks_on_its_own(decision: &SplitDecision) -> bool {
    match decision {
        SplitDecision::Decided { .. } => false,
        SplitDecision::Ask(AskBecause::FollowsAQuestion) => false,
        SplitDecision::Ask(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{Asked, ModelRemainder};
    use crate::domain::context_split::ContextSide;

    #[test]
    fn the_request_holds_the_names_it_was_given_and_is_weighed_in_bytes() {
        let remainder = ModelRemainder::assemble(
            &[
                (ContextSide::Work, "payments-api".to_string()),
                (ContextSide::Work, "billing".to_string()),
            ],
            &[
                (Asked::Project, "garden-planner".to_string()),
                (Asked::Conversation, "Сверка счетов".to_string()),
            ],
        );

        assert_eq!(remainder.questions(), 2);
        assert_eq!(remainder.examples(), 2);
        assert!(remainder.text().contains("work: payments-api, billing\n"));
        assert!(remainder.text().contains("1. проект: garden-planner\n"));
        assert!(remainder.text().contains("2. беседа: Сверка счетов\n"));
        assert!(
            !remainder.text().contains("personal:"),
            "a side with no example is not offered as one"
        );
        assert_eq!(
            remainder.bytes(),
            remainder.text().len(),
            "bytes, not characters: that is what a request costs"
        );
    }
}
