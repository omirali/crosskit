// The stage «Карта решений»: what a model makes of one confirmed task's conversations as a map
// of forks, branches and risks, what it costs, and how much of it a quote proves (ADR 0054,
// `docs/decision-map.md`).
//
// Nothing here is compiled into a released Crosskit: every module is declared
// `cfg(all(test, feature = "decision-map-measurement"))`. It reads the graph read-only, writes
// nothing to it, and calls a model only with the context's permission to send content out.
//
//   * `speech` — a stored conversation as numbered replies, each with its speaker and text.
//   * `question` — what the model is asked: the rules of the map, the schema, the replies.
//   * `verdict` — every node checked against the reply it names, by the knowledge layer's
//     quote rule.
//   * `measurement` — the measurement.
mod measurement;
mod question;
mod speech;
mod verdict;
