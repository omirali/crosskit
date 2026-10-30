// The stage «Сшивка бесед»: whether the owner's conversations of two tools stitch into tasks by
// facts their logs record (ADR 0051).
//
// Nothing here is compiled into a released Crosskit: every module is declared
// `cfg(all(test, feature = "task-stitching-measurement"))`. It opens no database, runs no `git`,
// calls no model and writes into the repository nothing at all; the one file it may write is
// the owner's list of the seed task, at a path outside the repository named in the command.
//
//   * `layout` — the home directory, the seed session and the list file, from the command.
//   * `claude_sessions` — every Claude Code transcript as session facts, through the product's
//     own transcript reader.
//   * `codex_sessions` — every Codex session file as session facts, including the Claude Code
//     conversations Codex imported.
//   * `labels` — the owner's marks on the list, and the score stitching gets against them.
//   * `report` — the numbers, and the owner's list of the seed task's project.
//   * `measurement` — the measurement.
mod claude_sessions;
mod codex_sessions;
mod labels;
mod layout;
mod measurement;
mod report;
