// The conversation `setup` has with the person: the contexts first, then the remainder, then
// every group, one at a time — and, when the remainder as a whole went nowhere, each of its
// folders after the groups.
//
// The person names the contexts; none is suggested, because a suggestion is a guess about what
// the person's life is divided into, and the program has no way to know. Every question accepts
// only its own answers and asks again otherwise. A closed input is an answer to nothing: the
// conversation ends there and nothing is written.
use std::io::{BufRead, Write};

use crate::domain::{
    GroupAnswer, GroupMembers, MachineGroups, NormalizedPath, RemainderAnswer, SetupAnswers,
    WorkspaceName,
};

use super::context_wording::counted;
use super::setup_wording::{ORGANIZATION_MARK, group_reason, shown};

/// One conversation over one input and one output.
pub(super) struct Dialog<'a> {
    input: &'a mut dyn BufRead,
    output: &'a mut dyn Write,
    home: Option<&'a NormalizedPath>,
}

impl<'a> Dialog<'a> {
    pub(super) fn new(
        input: &'a mut dyn BufRead,
        output: &'a mut dyn Write,
        home: Option<&'a NormalizedPath>,
    ) -> Self {
        Self {
            input,
            output,
            home,
        }
    }

    /// Every answer about `groups`, or nothing when the input closed before the last one.
    pub(super) fn answers(
        &mut self,
        groups: &MachineGroups,
    ) -> Result<Option<SetupAnswers>, String> {
        let Some(names) = self.context_names()? else {
            return Ok(None);
        };
        let Some(remainder) = self.remainder(groups.remainder(), &names)? else {
            return Ok(None);
        };
        let total = groups.groups().len();
        let mut answers = Vec::with_capacity(total);
        for (index, group) in groups.groups().iter().enumerate() {
            let mark = match group.looks_like_your_organization() {
                true => format!(" — {ORGANIZATION_MARK}"),
                false => String::new(),
            };
            let heading = format!(
                "Group {} of {total}: {}{mark}",
                index + 1,
                group_reason(group.reason())
            );
            let Some(answer) = self.ask_about(&heading, group.members(), &names, remainder)? else {
                return Ok(None);
            };
            answers.push(answer);
        }
        let remainder = match remainder {
            Some(context) => RemainderAnswer::Context(context),
            None => {
                let Some(folders) = self.remainder_folders(groups, &names)? else {
                    return Ok(None);
                };
                RemainderAnswer::OneByOne(folders)
            }
        };
        SetupAnswers::new(names, remainder, answers)
            .map(Some)
            .map_err(|error| format!("the answers do not describe contexts: {error:?}"))
    }

    /// The remainder folder by folder, after the groups: asked only when the remainder as a
    /// whole went to no context, so a folder that is plainly one context's is not lost with
    /// the ones that are nobody's.
    fn remainder_folders(
        &mut self,
        groups: &MachineGroups,
        names: &[WorkspaceName],
    ) -> Result<Option<Vec<GroupAnswer>>, String> {
        let folders = groups.remainder_folders();
        let total = folders.len();
        let mut answers = Vec::with_capacity(total);
        for (index, folder) in folders.iter().enumerate() {
            let heading = format!(
                "Folder {} of {total}: no group's conversations worked in it",
                index + 1
            );
            let Some(answer) = self.ask_about(&heading, folder, names, None)? else {
                return Ok(None);
            };
            answers.push(answer);
        }
        Ok(Some(answers))
    }

    /// `[y/N]` after the whole file: only a yes writes it.
    pub(super) fn confirmed(&mut self, question: &str) -> Result<bool, String> {
        self.say(&format!("{question} [y/N] "))?;
        let Some(answer) = self.line()? else {
            return Ok(false);
        };
        let answer = answer.to_lowercase();
        Ok(answer == "y" || answer == "yes")
    }

    fn context_names(&mut self) -> Result<Option<Vec<WorkspaceName>>, String> {
        self.say(
            "\nName the contexts, one per line, and end with an empty line. The names are yours: \
             nothing is suggested, and a context is only what you send to it.\n",
        )?;
        let mut names: Vec<WorkspaceName> = Vec::new();
        loop {
            self.say(&format!("Context {}: ", names.len() + 1))?;
            let Some(line) = self.line()? else {
                return Ok(None);
            };
            if line.is_empty() {
                if names.is_empty() {
                    self.say("At least one context is needed.\n")?;
                    continue;
                }
                return Ok(Some(names));
            }
            match WorkspaceName::parse(&line) {
                Ok(name) if names.contains(&name) => {
                    self.say(&format!("{name} is already named.\n"))?;
                }
                Ok(name) => names.push(name),
                Err(error) => self.say(&format!("{line:?} cannot name a context: {error}\n"))?,
            }
        }
    }

    /// Where everything left without an answer goes, asked before the first group so that an
    /// empty answer to a group means one thing all the way through.
    fn remainder(
        &mut self,
        remainder: &GroupMembers,
        names: &[WorkspaceName],
    ) -> Result<Option<Option<usize>>, String> {
        let mut text = String::from("\n");
        if remainder.is_empty() {
            text.push_str("Every folder found follows a group.\n");
        } else {
            text.push_str("Folders no group's conversations worked in:\n");
            text.push_str(&self.members(remainder));
        }
        text.push_str(&format!(
            "Everything left without an answer{} goes to context {} / [n]o, ask about each \
             folder after the groups? ",
            if remainder.is_empty() {
                ""
            } else {
                ", these folders included,"
            },
            choices(names)
        ));
        self.say(&text)?;
        loop {
            let Some(line) = self.line()? else {
                return Ok(None);
            };
            if matches!(line.to_lowercase().as_str(), "n" | "no") {
                return Ok(Some(None));
            }
            if let Some(index) = context_number(&line, names.len()) {
                return Ok(Some(Some(index)));
            }
            self.say(&format!(
                "Answer with a number from 1 to {}, or n: ",
                names.len()
            ))?;
        }
    }

    /// One question about one group or one folder: what it holds, and where it goes. An empty
    /// answer goes to the remainder's context when there is one, and nowhere otherwise.
    fn ask_about(
        &mut self,
        heading: &str,
        members: &GroupMembers,
        names: &[WorkspaceName],
        remainder: Option<usize>,
    ) -> Result<Option<GroupAnswer>, String> {
        let empty = match remainder {
            Some(context) => names[context].as_str().to_string(),
            None => "no context".to_string(),
        };
        self.say(&format!(
            "\n{heading}\n{}Context: {} / [s]kip / empty: {empty}? ",
            self.members(members),
            choices(names)
        ))?;
        loop {
            let Some(line) = self.line()? else {
                return Ok(None);
            };
            if line.is_empty() {
                return Ok(Some(GroupAnswer::Unanswered));
            }
            if matches!(line.to_lowercase().as_str(), "s" | "skip") {
                return Ok(Some(GroupAnswer::Skip));
            }
            if let Some(context) = context_number(&line, names.len()) {
                return Ok(Some(GroupAnswer::Context(context)));
            }
            self.say(&format!(
                "Answer with a number from 1 to {}, s, or an empty line: ",
                names.len()
            ))?;
        }
    }

    /// One line per member, the nested repositories under the project that covers them.
    fn members(&self, members: &GroupMembers) -> String {
        let mut text = String::new();
        for project in members.projects() {
            text.push_str(&format!(
                "  project        {}\n",
                shown(project.root(), self.home)
            ));
            for nested in project.nested() {
                text.push_str(&format!(
                    "                   with the repository inside it: {}\n",
                    shown(nested, self.home)
                ));
            }
        }
        for transcripts in members.transcripts() {
            text.push_str(&format!(
                "  conversations  {}  ({}, started in {})\n",
                shown(transcripts.path(), self.home),
                counted(transcripts.conversations() as u64, "conversation"),
                shown(transcripts.directory(), self.home)
            ));
        }
        for document in members.documents() {
            text.push_str(&format!(
                "  folder         {}  ({})\n",
                shown(document.path(), self.home),
                counted(document.files(), "file")
            ));
        }
        text
    }

    fn say(&mut self, text: &str) -> Result<(), String> {
        self.output
            .write_all(text.as_bytes())
            .and_then(|()| self.output.flush())
            .map_err(|error| format!("the question could not be printed: {error}"))
    }

    /// One answer, trimmed, or nothing when the input is closed.
    fn line(&mut self) -> Result<Option<String>, String> {
        let mut line = String::new();
        let read = self
            .input
            .read_line(&mut line)
            .map_err(|error| format!("the answer could not be read: {error}"))?;
        if read == 0 {
            return Ok(None);
        }
        Ok(Some(line.trim().to_string()))
    }
}

/// `[1] one / [2] two / [3] three`.
fn choices(names: &[WorkspaceName]) -> String {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| format!("[{}] {name}", index + 1))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The index of the context a typed number names, counting from one.
fn context_number(line: &str, contexts: usize) -> Option<usize> {
    let number: usize = line.parse().ok()?;
    (1..=contexts).contains(&number).then(|| number - 1)
}

#[cfg(test)]
#[path = "setup_dialog_tests.rs"]
mod tests;
