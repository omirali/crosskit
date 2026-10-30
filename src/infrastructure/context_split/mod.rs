// Stage 38's measurement on the owner's machine: whether it splits into a work and a personal
// context by signals that cost nothing, how often those signals are confidently wrong, and what
// would be left for a model.
//
// Nothing here is compiled into a released Crosskit: every module is declared
// `cfg(all(test, feature = "context-split-measurement"))`, the shape `coverage` set. It writes
// nothing, opens no database, runs no `git` and calls no model.
//
// The map:
//
//   * `home_layout` — the home directory named in the command, and where under it to look.
//   * `repository_census` — every git repository under the project roots, found by the
//     product's walker and read from its own `config` and `logs/HEAD`.
//   * `transcript_census` — every Claude Code transcript folder, and of every conversation its
//     working directories, name and touched files.
//   * `codex_census` — every Codex session, and of each only its working directory.
//   * `document_census` — the folders without git at the top of the document roots.
//   * `placement` — the domain's rules applied to repositories, and what every transcript
//     folder's name is the encoding of.
//   * `conversation_placement` — the same rules applied to conversations, folders without git
//     and Codex sessions.
//   * `model_remainder` — exactly what would go to a model, assembled and weighed, not sent.
//   * `report` — the words for every answer, and what every section shares.
//   * `inventory_report`, `repository_report`, `conversation_report`, `owner_truth_check` —
//     the sections, in the order they print.
//   * `measurement` — the measurement.
mod codex_census;
mod conversation_placement;
mod conversation_report;
mod document_census;
mod home_layout;
mod inventory_report;
mod measurement;
mod model_remainder;
mod owner_truth_check;
mod placement;
mod report;
mod repository_census;
mod repository_report;
mod transcript_census;
