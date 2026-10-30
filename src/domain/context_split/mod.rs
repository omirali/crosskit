// Stage 38: whether the owner's machine splits into a work and a personal context by itself.
//
// Nothing here is compiled into a released Crosskit. The module is declared
// `cfg(all(test, feature = "context-split-measurement"))`, the shape Stage 31 established for a
// measurement whose rules are not the product's: the split is measured before anybody designs a
// command around it.
//
// The map:
//
//   * `side` — the answer about one thing: which context it goes to and what decided it, or
//     why nothing could and the owner has to be asked.
//   * `git_identity` — the mail domains a reflog records; the domain itself, the `[user]` of a
//     configuration and the owner's one domain are the product's (`domain::git_identity`).
//   * `project_rule` — the ordered rule that turns a repository's signals into an answer.
//   * `conversation_rule` — a conversation follows the folder it is filed under, checked
//     against the working directories inside it; a folder without git follows the
//     conversations that worked in it.
//   * `owner_truth` — the owner's own labels, and the difference between a confident mistake
//     and a question.
//
// The transcript folder rule, the owner of a remote and the mail domain were written here and
// moved into the product at Stage 39, which builds `setup` on them; this measurement now reads
// them from there and keeps only what is its own — the work/personal sides, the rule that
// assigns them, and the owner's labels.
mod conversation_rule;
mod git_identity;
mod owner_truth;
mod project_rule;
mod side;

pub(crate) use crate::domain::git_identity::{
    Identity, MailDomain, OwnerDomain, identity_in_git_config,
};
pub(crate) use crate::domain::git_remote::remote_owner::Origin;
pub(crate) use crate::domain::transcript_folder::{
    is_temporary_folder_name, is_temporary_place, transcript_folder_name,
};
pub(crate) use conversation_rule::{
    FolderPlace, WorkingDirectoryEvidence, place_conversation, place_folder_without_git,
};
pub(crate) use git_identity::ReflogIdentities;
pub(crate) use owner_truth::{Agreement, OwnerTruth, agreement};
pub(crate) use project_rule::{RepositoryEvidence, decide_repository};
pub(crate) use side::{AskBecause, ContextSide, DecidedBy, SplitDecision};
