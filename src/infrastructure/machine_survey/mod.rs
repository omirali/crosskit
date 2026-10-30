// What `setup` reads beside the walk of the tree: the folders Claude Code filed conversations
// under, the folders without git where documents are kept, how many Codex sessions there are,
// and the mail domain git is configured with.
//
// Each reads as little as its question needs. A transcript is read for one field of each line —
// the working directory — and nothing said in it is kept. A folder without git is listed and
// its files counted, none opened. A Codex session is counted and not opened. A git
// configuration gives the domain of its `[user] email` and nothing else (ADR 0042).
//
// The map:
//
//   * `transcripts` — every folder under `~/.claude/projects` and the working directories of
//     the conversations lying directly in it.
//   * `documents` — the folders directly in the places the grouping rule names.
//   * `git_identity` — the mail domain of the global git configuration.
//   * `codex` — the number of Codex sessions.
mod codex;
mod documents;
mod git_identity;
mod transcripts;

pub use codex::codex_sessions;
pub use documents::document_folders;
pub use git_identity::owner_mail_domain;
#[cfg(all(
    test,
    any(
        feature = "context-split-measurement",
        feature = "task-stitching-measurement"
    )
))]
pub(crate) use transcripts::entries_in;
#[cfg(all(test, feature = "context-split-measurement"))]
pub(crate) use transcripts::working_directories_in;
pub use transcripts::{transcript_folders, transcripts_root};
