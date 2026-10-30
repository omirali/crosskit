// The corpus of transcripts, read the way Crosskit reads it.
//
// "The way Crosskit reads it" is the point. The exclusions that keep tool output and
// sub-agent transcripts out of the `chats` context are in the owner's configuration, not in
// this file, and a measurement that wrote its own copy of them would be measuring a corpus
// the product does not see. So the policy is loaded and asked, exactly as a pass asks it.
//
// The walk is this file's own, and small: one directory tree, `.jsonl` files, no symbolic
// links followed. A measurement has no business inheriting the pass's walker, which exists to
// keep a file's identity stable across renames while the tree changes underneath it.
use std::fs;
use std::path::{Path, PathBuf};

use super::transcript_reader::read_transcript;
use crate::application::conversation_assembly::{
    ConversationAssembly, ReadTranscript, TranscriptTally,
};
use crate::domain::conversation::Conversation;
use crate::domain::{IndexPolicy, NormalizedPath, PolicyClass};
use crate::infrastructure::config::load_index_policy;

/// Where the corpus is, unless the environment names somewhere else. Named so the
/// measurement can be run against a small tree without the owner's correspondence in it.
const CORPUS_ROOT: &str = "CROSSKIT_CHATS_ROOT";

/// One transcript, and the conversation it held.
#[derive(Debug, Clone)]
pub(crate) struct Transcript {
    path: String,
    file_bytes: u64,
    lines: u64,
    conversation: Conversation,
    tally: TranscriptTally,
}

impl Transcript {
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub(crate) fn file_bytes(&self) -> u64 {
        self.file_bytes
    }

    pub(crate) fn lines(&self) -> u64 {
        self.lines
    }

    pub(crate) fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    /// What reading this one transcript met. Kept per file as well as folded into the corpus,
    /// because "how many name lines does one transcript hold" is a question about one file.
    pub(crate) fn tally(&self) -> &TranscriptTally {
        &self.tally
    }
}

/// A corpus of transcripts and everything reading it met.
#[derive(Debug)]
pub(crate) struct ChatCorpus {
    root: String,
    transcripts: Vec<Transcript>,
    tally: TranscriptTally,
    excluded_directories: u64,
    excluded_files: u64,
    excluded_bytes: u64,
    other_files: u64,
    other_bytes: u64,
    unreadable_files: Vec<(String, String)>,
}

impl ChatCorpus {
    /// Reads every transcript the owner's index policy admits under the corpus root.
    pub(crate) fn read() -> Result<Self, CorpusError> {
        let root = corpus_root()?;
        let scan_root =
            NormalizedPath::from_absolute(&root).map_err(|_| CorpusError::RootNotAbsolute(root))?;
        let loaded = load_index_policy().map_err(|error| CorpusError::Policy(error.to_string()))?;
        let policy = loaded.policy().with_scan_root(&scan_root);

        let mut corpus = Self {
            root: scan_root.as_str().to_string(),
            transcripts: Vec::new(),
            tally: TranscriptTally::default(),
            excluded_directories: 0,
            excluded_files: 0,
            excluded_bytes: 0,
            other_files: 0,
            other_bytes: 0,
            unreadable_files: Vec::new(),
        };
        corpus.walk(Path::new(scan_root.as_str()), &scan_root, &policy)?;
        corpus
            .transcripts
            .sort_by(|left, right| left.path.cmp(&right.path));
        Ok(corpus)
    }

    fn walk(
        &mut self,
        directory: &Path,
        normalized: &NormalizedPath,
        policy: &IndexPolicy,
    ) -> Result<(), CorpusError> {
        let entries = fs::read_dir(directory).map_err(|error| CorpusError::Unreadable {
            path: directory.display().to_string(),
            reason: error.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| CorpusError::Unreadable {
                path: directory.display().to_string(),
                reason: error.to_string(),
            })?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(here) = normalized.join(&name) else {
                continue;
            };
            // Symbolic links are not followed. A corpus that follows one can walk out of the
            // context the policy was asked about, and then the numbers belong to some other
            // tree.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                // A directory the policy excludes is not descended into, so what is inside it
                // is never counted or even listed. That is what the pass does too, and it is
                // why the report says how many directories were pruned rather than claiming a
                // number of files it did not look at.
                if policy.decide(&here).class() == PolicyClass::Exclude {
                    self.excluded_directories += 1;
                    continue;
                }
                self.walk(&entry.path(), &here, policy)?;
                continue;
            }
            let size = entry.metadata().map(|data| data.len()).unwrap_or_default();
            if policy.decide(&here).class() == PolicyClass::Exclude {
                self.excluded_files += 1;
                self.excluded_bytes += size;
                continue;
            }
            if !name.ends_with(".jsonl") {
                self.other_files += 1;
                self.other_bytes += size;
                continue;
            }
            self.absorb_transcript(&entry.path(), here.as_str(), size, &name);
        }
        Ok(())
    }

    /// One transcript read into one conversation. A file that cannot be opened or read is
    /// recorded with its reason rather than ending the run: a corpus is live, a session can
    /// be writing into it right now, and one unreadable file is a line of the report.
    fn absorb_transcript(&mut self, path: &Path, normalized: &str, size: u64, name: &str) {
        let fallback = name.strip_suffix(".jsonl").unwrap_or(name);
        // No ceiling: this measurement weighs a corpus, and a conversation it dropped would
        // be a byte it failed to count. The pass hands the assembly its policy's limit.
        let mut assembly = ConversationAssembly::for_session(fallback, u64::MAX);
        match read_transcript(path, |line| {
            // This measurement has no ceiling, so the assembly never asks to stop.
            let _stop = assembly.absorb(line);
        }) {
            Ok(lines) => {
                let read: ReadTranscript = assembly.finish();
                self.tally.absorb(read.tally());
                let (conversation, tally) = read.into_parts();
                self.transcripts.push(Transcript {
                    path: normalized.to_string(),
                    file_bytes: size,
                    lines,
                    conversation,
                    tally,
                });
            }
            Err(error) => self
                .unreadable_files
                .push((normalized.to_string(), error.to_string())),
        }
    }

    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn transcripts(&self) -> &[Transcript] {
        &self.transcripts
    }

    pub(crate) fn tally(&self) -> &TranscriptTally {
        &self.tally
    }

    pub(crate) fn file_bytes(&self) -> u64 {
        self.transcripts
            .iter()
            .map(|transcript| transcript.file_bytes)
            .sum()
    }

    pub(crate) fn spoken_bytes(&self) -> u64 {
        self.transcripts
            .iter()
            .map(|transcript| transcript.conversation.spoken_bytes())
            .sum()
    }

    pub(crate) fn lines(&self) -> u64 {
        self.transcripts
            .iter()
            .map(|transcript| transcript.lines)
            .sum()
    }

    /// What the policy kept out: directories pruned whole, and files refused one by one.
    /// The bytes are only of the files, because a pruned directory was never walked.
    pub(crate) fn excluded(&self) -> (u64, u64, u64) {
        (
            self.excluded_directories,
            self.excluded_files,
            self.excluded_bytes,
        )
    }

    /// Files the policy admits that are not transcripts — the hundred `.md` and the `.txt`
    /// lying beside sessions. Counted so the report can say what this stage did not read.
    pub(crate) fn other_admitted(&self) -> (u64, u64) {
        (self.other_files, self.other_bytes)
    }

    pub(crate) fn unreadable_files(&self) -> &[(String, String)] {
        &self.unreadable_files
    }
}

/// Why a corpus could not be read at all. Every one of these ends the measurement, because
/// none of them leaves a number worth printing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CorpusError {
    /// Neither `CROSSKIT_CHATS_ROOT` nor `HOME` says where the corpus is.
    RootUnknown,
    RootNotAbsolute(String),
    Policy(String),
    Unreadable {
        path: String,
        reason: String,
    },
}

impl std::fmt::Display for CorpusError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootUnknown => write!(
                formatter,
                "не задан ни {CORPUS_ROOT}, ни HOME: измерению негде взять корпус"
            ),
            Self::RootNotAbsolute(path) => {
                write!(formatter, "корень корпуса не абсолютный путь: {path}")
            }
            Self::Policy(reason) => write!(formatter, "не читается конфигурация: {reason}"),
            Self::Unreadable { path, reason } => {
                write!(formatter, "не читается каталог {path}: {reason}")
            }
        }
    }
}

fn corpus_root() -> Result<String, CorpusError> {
    if let Some(named) = std::env::var_os(CORPUS_ROOT).and_then(|value| value.into_string().ok()) {
        return Ok(named);
    }
    let home = std::env::var_os("HOME")
        .and_then(|value| value.into_string().ok())
        .ok_or(CorpusError::RootUnknown)?;
    Ok(PathBuf::from(home)
        .join(".claude/projects")
        .display()
        .to_string())
}
