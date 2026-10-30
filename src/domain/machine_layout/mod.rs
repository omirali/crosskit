// What `setup` finds on a machine, the groups it asks about, and the contexts the answers make.
//
// The system knows no sides. A context is a name the person gives, and there may be two or five
// of them; nothing here says "work" or "personal". What the system does know is which things
// belong together and why — one group of a remote's `origin`, the conversations filed under a
// project, a folder those conversations worked in — and it asks about each such group once
// instead of about each thing in it (ADR 0042).
//
// The map:
//
//   * `survey` — what the walk and the censuses found, as the rule reads it: transcript folders
//     with the working directories their conversations start in, and folders without git.
//   * `members` — what one group is made of, and what is left out of every group and why.
//   * `projects` — the projects of a walked tree, grouped by the group of their `origin`.
//   * `transcripts` — a transcript folder goes with the project whose directory its name is the
//     encoding of, checked against the first working directory recorded inside it.
//   * `grouping` — the three put together, with the folders without git that follow them.
//   * `answers` — what the person said about each group, and the contexts that makes.
mod answers;
mod grouping;
mod members;
mod projects;
mod survey;
/// A synthetic machine, shared by the tests of this module and of the dialog that asks about it.
#[cfg(test)]
pub(crate) mod test_machine;
mod transcripts;

pub use answers::{
    ContextLayout, GroupAnswer, LaidOutContext, LaidOutRoot, RemainderAnswer, RootKind, RootSource,
    SetupAnswers,
};
pub use grouping::{MachineGroups, places_of_document_folders};
pub use members::{GroupMembers, GroupReason, LeftOutKind, LeftOutReason, NoSignal};
pub use survey::{ConversationStart, DocumentFolderSurvey, TranscriptFolderSurvey};
