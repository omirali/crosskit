// Stitching conversations into tasks: which sessions of two tools belong to one piece of work,
// by facts their logs record (ADR 0051).
//
// The product stitches the conversations its graph holds (ADR 0052); the measurement of the
// stage «Сшивка бесед» stitches the files of both tools with the same rules (ADR 0051).
//
//   * `mentions` — paths, patched files and tracker links found in text, and the key a path is
//     compared by.
//   * `session` — what is known of one session: where, when, by whom, what it wrote and named.
//   * `stitching` — the signals between two sessions and the candidate tasks they add up to.
pub(crate) mod mentions;
pub(crate) mod session;
pub(crate) mod stitching;
