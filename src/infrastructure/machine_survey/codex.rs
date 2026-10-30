// How many Codex sessions lie on this machine — counted, not read.
//
// Codex files its sessions by date and not by project, and Crosskit has no reader for them, so
// `setup` puts none of them in a context and says how many there are. A count is all that is
// taken: no session is opened.
use crate::domain::NormalizedPath;

use super::super::files_with_extension;

const SESSIONS: &str = ".codex/sessions";

/// The number of session files under the home's Codex directory; zero when there is none.
pub fn codex_sessions(home: &NormalizedPath) -> usize {
    files_with_extension(&std::path::Path::new(home.as_str()).join(SESSIONS), "jsonl").len()
}
