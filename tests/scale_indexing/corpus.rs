//! Shared by more than one benchmark, each of which uses a part of it; the parts a
//! given benchmark does not use are not dead code.
#![allow(dead_code)]

// A deterministic synthetic corpus and its manifest: the single source of every expected
// counter. Nothing here comes from the user's files; content is generated from the file
// index, so two runs of the same profile produce byte-identical trees.
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};

/// The extractor splits text into 4 KiB chunks at whitespace; the corpus keeps small files
/// below one chunk and builds large files from 64-byte lines whose total is a multiple of
/// the chunk size, so the expected chunk count is exact without re-implementing the rule.
pub const CHUNK_BYTES: usize = 4 * 1024;
/// The test policy's content limit; the oversized file is written above it.
pub const MAX_CONTENT_MB: u64 = 1;
/// The content ceiling compiled into the policy reader, which no configuration can raise.
/// It is mirrored here rather than shared, because a benchmark reaches the binary only
/// through the CLI. The mirror cannot drift silently: a lower ceiling turns a source of
/// this size into a metadata-only file, and every benchmark that writes one asserts that
/// its content was indexed. The ceiling itself is held by a unit test beside the reader.
pub const CONTENT_CEILING_MIB: u64 = 16;
const OVERSIZED_BYTES: usize = 3 * 512 * 1024;
const LARGE_FILE_CHUNKS: usize = 3;
const LINE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Smoke,
    Scale,
    Stress,
    /// A corpus of exactly this many text files. The three named profiles are the sizes a
    /// person asks for by name; the size curve needs points no adjective names, and a
    /// profile that reads `50000` cannot be misread as any of them.
    Files(usize),
}

impl Profile {
    /// `CROSSKIT_SCALE_PROFILE`: `smoke` (default), `scale`, `stress`, or a number of
    /// text files.
    pub fn from_env() -> Self {
        match std::env::var("CROSSKIT_SCALE_PROFILE").as_deref() {
            Err(_) | Ok("smoke") => Self::Smoke,
            Ok("scale") => Self::Scale,
            Ok("stress") => Self::Stress,
            Ok(other) => match other.parse::<usize>() {
                Ok(files) if files > 0 => Self::Files(files),
                _ => panic!(
                    "unknown CROSSKIT_SCALE_PROFILE {other:?}; use smoke, scale, stress or a number of text files"
                ),
            },
        }
    }

    pub fn name(self) -> String {
        match self {
            Self::Smoke => "smoke".to_string(),
            Self::Scale => "scale".to_string(),
            Self::Stress => "stress".to_string(),
            Self::Files(files) => format!("{files}-files"),
        }
    }

    /// How many text files the corpus of this profile holds. The whole corpus is derived
    /// from it, so it is also the one number a curve point is chosen by.
    pub fn text_files(self) -> usize {
        match self {
            Self::Smoke => 1_000,
            Self::Scale => 10_000,
            Self::Stress => 25_000,
            Self::Files(files) => files,
        }
    }
}

/// One included text file and how many chunks its current content yields.
#[derive(Debug, Clone)]
pub struct TextFile {
    pub relative: PathBuf,
    pub chunks: usize,
}

/// What the cold scan of the corpus must report and store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expected {
    pub indexed_files: usize,
    pub indexed_chunks: usize,
    pub metadata_only_files: usize,
    pub content_skipped_files: usize,
    pub excluded_paths: usize,
    pub file_versions: usize,
    pub chunks_stored: usize,
    pub locations: usize,
}

/// What the mutation left behind: the text files as they exist afterwards, and how many of
/// the created files took the identity of a deleted one. Crosskit identifies a file by its
/// device and inode number, and a filesystem that hands a freed inode number to the next
/// file created makes a new file indistinguishable from a deleted one. ext4 and overlayfs
/// do that; APFS does not. The reconcile then reports the deleted file's retired location
/// as relocated rather than plainly missing, so the benchmark measures how often it
/// happened here instead of asserting what one filesystem happens to do.
pub struct Mutation {
    pub files: Vec<TextFile>,
    pub reused_identities: usize,
}

/// The deterministic mutation applied between the idle and the mutated reconcile.
#[derive(Debug, Clone)]
pub struct MutationPlan {
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub renamed: Vec<(PathBuf, PathBuf)>,
    pub moved_under_exclude: Vec<(PathBuf, PathBuf)>,
    pub created: Vec<PathBuf>,
}

pub struct Corpus {
    pub root: PathBuf,
    pub profile: Profile,
    pub text_files: Vec<TextFile>,
    pub total_bytes: u64,
    /// Of those, the bytes of the files whose content is indexed. The search index is
    /// measured against this rather than against the whole tree, which also holds binaries,
    /// a metadata-only directory and a file above the content limit.
    pub text_bytes: u64,
    pub regular_files: usize,
    pub hard_link: bool,
    pub metadata_directory: PathBuf,
    pub symlink_sentinel: &'static str,
}

impl Corpus {
    /// Builds the corpus under `bench/corpus`; the symlink target lives in `bench/outside`
    /// so the scan can prove it never followed the link.
    pub fn generate(bench: &Path, profile: Profile) -> Self {
        let root = bench.join("corpus");
        fs::create_dir(&root).unwrap();
        let mut total_bytes = 0u64;
        let mut regular_files = 0usize;
        let mut write = |relative: &Path, bytes: &[u8]| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            total_bytes += bytes.len() as u64;
            regular_files += 1;
        };

        let count = profile.text_files();
        let mut text_files = Vec::with_capacity(count);
        let mut text_bytes = 0u64;
        for index in 0..count {
            let relative = text_relative(index);
            let (bytes, chunks) = text_content(index);
            text_bytes += bytes.len() as u64;
            write(&relative, &bytes);
            text_files.push(TextFile { relative, chunks });
        }
        let side_count = (count / 50).max(10);
        for index in 0..side_count {
            write(
                &PathBuf::from(format!("assets/blob-{index:04}.bin")),
                &binary_content(index),
            );
            write(
                &PathBuf::from(format!("metadata/scan-{index:04}.txt")),
                format!("metadata only #{index}\n").as_bytes(),
            );
        }
        write(Path::new("big/oversized.txt"), &vec![b'o'; OVERSIZED_BYTES]);
        // Built-in Exclude: each directory counts once, each excluded file once.
        for (relative, content) in [
            (".git/HEAD", "ref: refs/heads/main\n"),
            (".git/config", "[core]\n"),
            ("node_modules/left-pad/index.js", "module.exports = 1;\n"),
            ("target/debug/crosskit.d", "dep\n"),
            (".env", "TOKEN=never-read\n"),
            ("secrets/server.key", "-----BEGIN PRIVATE KEY-----\n"),
        ] {
            write(Path::new(relative), content.as_bytes());
        }

        let outside = bench.join("outside");
        fs::create_dir(&outside).unwrap();
        let symlink_sentinel = "SYMLINK-TARGET-MUST-NEVER-BE-READ";
        fs::write(outside.join("secret.txt"), symlink_sentinel).unwrap();
        fs::create_dir(root.join("links")).unwrap();
        symlink(
            outside.join("secret.txt"),
            root.join("links/to-outside.txt"),
        )
        .unwrap();
        let hard_link = fs::hard_link(
            root.join(&text_files[0].relative),
            root.join("links/hard-note.md"),
        )
        .is_ok();

        let root = fs::canonicalize(&root).unwrap();
        let metadata_directory = root.join("metadata");
        Self {
            root,
            profile,
            text_files,
            total_bytes,
            text_bytes,
            regular_files,
            hard_link,
            metadata_directory,
            symlink_sentinel,
        }
    }

    pub fn side_count(&self) -> usize {
        (self.profile.text_files() / 50).max(10)
    }

    /// What `index init` and an idle `index reconcile` must report.
    pub fn expected_initial(&self) -> Expected {
        let hard_link = usize::from(self.hard_link);
        let chunks: usize = self.text_files.iter().map(|file| file.chunks).sum();
        Expected {
            indexed_files: self.text_files.len() + hard_link,
            indexed_chunks: chunks + hard_link * self.text_files[0].chunks,
            metadata_only_files: self.side_count(),
            content_skipped_files: self.side_count() + 2,
            excluded_paths: 5,
            file_versions: self.text_files.len(),
            chunks_stored: chunks,
            locations: self.text_files.len() + hard_link + 2 * self.side_count() + 2,
        }
    }

    /// How many chunks the current versions hold after the mutation: what an index in step
    /// with the graph has a document for.
    ///
    /// Not the same as `chunks_stored`, and the difference is the point of the index: the
    /// graph keeps every revision, the index keeps the current one. A file that was edited
    /// leaves its old chunks in the graph and none in the index; a file that was deleted or
    /// moved out of scope keeps its last version in both, because its content is still the
    /// current version of a file the graph knows.
    pub fn expected_current_chunks(&self, plan: &MutationPlan) -> usize {
        let mut chunks: usize = self.text_files.iter().map(|file| file.chunks).sum();
        for relative in &plan.modified {
            chunks -= self.chunks_of(relative);
            chunks += modified_content(relative).1;
        }
        for relative in &plan.created {
            chunks += created_content(relative).1;
        }
        chunks
    }

    fn chunks_of(&self, relative: &Path) -> usize {
        self.text_files
            .iter()
            .find(|file| file.relative == relative)
            .unwrap_or_else(|| panic!("{relative:?} is not a corpus text file"))
            .chunks
    }

    /// Picks files by index: the hard-linked first file is never touched, so the link keeps
    /// pointing at unchanged content.
    pub fn plan_mutation(&self) -> MutationPlan {
        let mut plan = MutationPlan {
            modified: Vec::new(),
            deleted: Vec::new(),
            renamed: Vec::new(),
            moved_under_exclude: Vec::new(),
            created: Vec::new(),
        };
        let moved_cap = (self.text_files.len() / 100).clamp(3, 50);
        for (index, file) in self.text_files.iter().enumerate().skip(1) {
            match index % 20 {
                1 => plan.modified.push(file.relative.clone()),
                2 => plan.deleted.push(file.relative.clone()),
                3 => {
                    let renamed = file
                        .relative
                        .with_file_name(format!("renamed-{index:06}.md"));
                    plan.renamed.push((file.relative.clone(), renamed));
                }
                4 if plan.moved_under_exclude.len() < moved_cap => {
                    let hidden = PathBuf::from("node_modules/moved")
                        .join(file.relative.file_name().unwrap());
                    plan.moved_under_exclude
                        .push((file.relative.clone(), hidden));
                }
                _ => {}
            }
        }
        for index in 0..(self.text_files.len() / 20).max(5) {
            plan.created
                .push(PathBuf::from(format!("docs/new/created-{index:06}.md")));
        }
        plan
    }

    /// Applies the plan and reports what it left behind.
    pub fn apply_mutation(&self, plan: &MutationPlan) -> Mutation {
        let mut files: Vec<TextFile> = self.text_files.clone();
        for relative in &plan.modified {
            let (bytes, chunks) = modified_content(relative);
            fs::write(self.root.join(relative), bytes).unwrap();
            files
                .iter_mut()
                .find(|f| &f.relative == relative)
                .unwrap()
                .chunks = chunks;
        }
        let mut deleted_identities = HashSet::new();
        for relative in &plan.deleted {
            let path = self.root.join(relative);
            deleted_identities.insert(identity_of(&path));
            fs::remove_file(&path).unwrap();
            files.retain(|f| &f.relative != relative);
        }
        for (from, to) in &plan.renamed {
            fs::rename(self.root.join(from), self.root.join(to)).unwrap();
            files
                .iter_mut()
                .find(|f| &f.relative == from)
                .unwrap()
                .relative = to.clone();
        }
        fs::create_dir_all(self.root.join("node_modules/moved")).unwrap();
        for (from, to) in &plan.moved_under_exclude {
            fs::rename(self.root.join(from), self.root.join(to)).unwrap();
            files.retain(|f| &f.relative != from);
        }
        fs::create_dir_all(self.root.join("docs/new")).unwrap();
        let mut reused_identities = 0;
        for relative in &plan.created {
            let (bytes, chunks) = created_content(relative);
            let path = self.root.join(relative);
            fs::write(&path, bytes).unwrap();
            if deleted_identities.contains(&identity_of(&path)) {
                reused_identities += 1;
            }
            files.push(TextFile {
                relative: relative.clone(),
                chunks,
            });
        }
        Mutation {
            files,
            reused_identities,
        }
    }

    /// What the reconcile right after the mutation must report and store.
    pub fn expected_after_mutation(&self, plan: &MutationPlan, files: &[TextFile]) -> Expected {
        let initial = self.expected_initial();
        let hard_link = usize::from(self.hard_link);
        let chunks: usize = files.iter().map(|file| file.chunks).sum();
        let new_versions = plan.modified.len() + plan.created.len();
        Expected {
            indexed_files: files.len() + hard_link,
            indexed_chunks: chunks + hard_link * self.text_files[0].chunks,
            metadata_only_files: initial.metadata_only_files,
            content_skipped_files: initial.content_skipped_files,
            excluded_paths: initial.excluded_paths,
            file_versions: initial.file_versions + new_versions,
            chunks_stored: initial.chunks_stored
                + plan
                    .modified
                    .iter()
                    .map(|relative| modified_content(relative).1)
                    .sum::<usize>()
                + plan
                    .created
                    .iter()
                    .map(|relative| created_content(relative).1)
                    .sum::<usize>(),
            locations: initial.locations + plan.renamed.len() + plan.created.len(),
        }
    }
}

/// A file's identity as the indexer takes it: device and inode number.
fn identity_of(path: &Path) -> (u64, u64) {
    let metadata = fs::metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

fn text_relative(index: usize) -> PathBuf {
    PathBuf::from(format!(
        "docs/d{:03}/s{:02}/note-{index:06}.md",
        index / 250,
        (index / 10) % 25
    ))
}

/// Small notes stay under one chunk; every 40th file is a large file of exactly three.
///
/// Public because a benchmark that writes a tree of its own must write the same notes this
/// corpus does, or its numbers could not be put beside the curve's.
pub fn text_content(index: usize) -> (Vec<u8>, usize) {
    if index % 40 == 39 {
        return (large_text(index, LARGE_FILE_CHUNKS), LARGE_FILE_CHUNKS);
    }
    (small_text(index, "note"), 1)
}

fn modified_content(relative: &Path) -> (Vec<u8>, usize) {
    (small_text(seed_of(relative), "modified"), 1)
}

fn created_content(relative: &Path) -> (Vec<u8>, usize) {
    (small_text(seed_of(relative), "created"), 1)
}

fn seed_of(relative: &Path) -> usize {
    relative
        .to_string_lossy()
        .bytes()
        .fold(7usize, |seed, byte| {
            seed.wrapping_mul(31).wrapping_add(byte as usize)
        })
}

const WORDS: &[&str] = &[
    "graph",
    "evidence",
    "reconcile",
    "epoch",
    "location",
    "chunk",
    "policy",
    "watcher",
    "outbox",
    "revision",
    "заметка",
    "индекс",
    "источник",
    "проверка",
    "хранилище",
];

fn small_text(seed: usize, label: &str) -> Vec<u8> {
    let mut random = Xorshift::new(seed as u64 ^ 0x9E37_79B9_7F4A_7C15);
    let mut text = format!("# {label} {seed}\n\n");
    let lines = 2 + random.below(24);
    for _ in 0..lines {
        let words = 3 + random.below(12);
        for _ in 0..words {
            text.push_str(WORDS[random.below(WORDS.len())]);
            text.push(' ');
        }
        text.push('\n');
    }
    assert!(
        text.len() < CHUNK_BYTES,
        "a small note must stay inside one chunk"
    );
    text.into_bytes()
}

/// `chunks` × 4096 bytes of 64-byte lines with no inner whitespace, so every chunk ends
/// exactly on a line end.
/// `chunks` whole chunks of ASCII text built from 64-byte lines, so the chunk count of the
/// result is exact rather than approximate: a boundary always falls on a line end.
pub fn large_text(seed: usize, chunks: usize) -> Vec<u8> {
    let lines = chunks * CHUNK_BYTES / LINE_BYTES;
    let mut text = String::with_capacity(chunks * CHUNK_BYTES);
    for line in 0..lines {
        let mut content = format!("large-{seed:08}-line-{line:06}-");
        while content.len() < LINE_BYTES - 1 {
            content.push('x');
        }
        text.push_str(&content);
        text.push('\n');
    }
    assert_eq!(text.len(), chunks * CHUNK_BYTES);
    text.into_bytes()
}

fn binary_content(index: usize) -> Vec<u8> {
    let mut random = Xorshift::new(0xB10B_0000 + index as u64);
    let mut bytes = vec![0u8; 512 + random.below(2048)];
    for byte in bytes.iter_mut() {
        *byte = random.below(256) as u8;
    }
    bytes[0] = 0;
    bytes
}

struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn below(&mut self, bound: usize) -> usize {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x % bound as u64) as usize
    }
}
