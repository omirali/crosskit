// The measurement fixtures stay fixtures.
//
// `tests/fixtures/kazakh/corpus.txt` is somebody else's text, included under CC BY-SA 4.0.
// A programme that carried it inside itself would be distributing that text to every user,
// under obligations nothing in a released binary could state — so it must be read at
// measurement time and never built in.
//
// This is a guard, not a ceremony: `include_str!` and `include_bytes!` are the two ways a
// file becomes part of the binary, and both are one line somebody could add without meaning
// anything by it.
use std::fs;
use std::path::{Path, PathBuf};

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(directory: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
}

#[test]
fn no_fixture_is_embedded_in_the_program() {
    let mut files = Vec::new();
    rust_files(&source_root(), &mut files);
    assert!(files.len() > 50, "the source tree was not found");

    let mut embedded = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).unwrap();
        for (number, line) in source.lines().enumerate() {
            let embeds = line.contains("include_str!") || line.contains("include_bytes!");
            if embeds && (line.contains("fixtures") || line.contains("corpus")) {
                embedded.push(format!("{}:{}", file.display(), number + 1));
            }
        }
    }

    assert!(
        embedded.is_empty(),
        "a measurement fixture would be carried inside the binary, and its licence cannot \
         travel with it: {embedded:?}"
    );
}
