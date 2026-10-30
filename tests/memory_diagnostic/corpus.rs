// A deterministic synthetic corpus for the memory diagnosis: plain text notes only, all
// generated from their index, so two runs produce the same tree and no user file is ever
// read. Small on purpose: this benchmark studies how cost grows with observation history,
// not how it grows with corpus size.
use std::fs;
use std::path::{Path, PathBuf};

// The defaults are the experiment the report and the README quote: 2 000 files and 30
// history passes, measured every 6, which walks the history from 4 000 to 76 000
// observations. Running the documented command with no variables set reproduces exactly
// those numbers; the variables exist to explore around them.

/// The source-identity count used by the scale benchmark and by the isolated
/// current-state diagnostic.
pub const CURRENT_STATE_SOURCE_IDENTITIES: usize = 10_407;

/// Files in the corpus; `CROSSKIT_DIAGNOSTIC_FILES` overrides it.
pub fn file_count() -> usize {
    env_number("CROSSKIT_DIAGNOSTIC_FILES", 2_000)
}

/// Reconcile passes used to build the observation history after the measured baseline;
/// `CROSSKIT_DIAGNOSTIC_PASSES` overrides it.
pub fn history_passes() -> usize {
    env_number("CROSSKIT_DIAGNOSTIC_PASSES", 30)
}

/// How many passes happen between two measured reconciles;
/// `CROSSKIT_DIAGNOSTIC_CHECKPOINT` overrides it.
pub fn checkpoint_every() -> usize {
    env_number("CROSSKIT_DIAGNOSTIC_CHECKPOINT", 6)
}

fn env_number(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

pub struct Corpus {
    pub root: PathBuf,
    pub files: usize,
    pub bytes: u64,
}

impl Corpus {
    pub fn generate(bench: &Path) -> Self {
        Self::generate_named(bench, "corpus", file_count())
    }

    fn generate_named(bench: &Path, directory: &str, files: usize) -> Self {
        let root = bench.join(directory);
        fs::create_dir(&root).unwrap();
        let mut bytes = 0;
        for index in 0..files {
            let relative = root
                .join(format!("d{:03}", index / 100))
                .join(format!("note-{index:06}.md"));
            fs::create_dir_all(relative.parent().unwrap()).unwrap();
            let text = note(index);
            bytes += text.len() as u64;
            fs::write(&relative, text).unwrap();
        }
        Self {
            root: fs::canonicalize(&root).unwrap(),
            files,
            bytes,
        }
    }
}

const WORDS: &[&str] = &[
    "graph",
    "evidence",
    "reconcile",
    "epoch",
    "location",
    "chunk",
    "policy",
    "observation",
];

/// One note, well under the 4 KiB chunk boundary, so every file is exactly one chunk.
fn note(index: usize) -> String {
    let mut seed = (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut text = format!("# note {index}\n\n");
    for _ in 0..(8 + index % 12) {
        for _ in 0..6 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            text.push_str(WORDS[(seed % WORDS.len() as u64) as usize]);
            text.push(' ');
        }
        text.push('\n');
    }
    text
}
