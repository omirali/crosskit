// CLI smoke: a corpus whose texts carry the spaces that are not one byte wide.
//
// This is the shape that killed the first run on a real tree. The cut ends a chunk after the
// last whitespace inside it, and it used to end it one byte past where that whitespace
// *starts* — inside a `U+200A`, which is three bytes. The synthetic corpora never reached
// it, and not by luck: their small notes are separated by ASCII spaces, and their large
// files are built out of lines with no inner whitespace at all, so the last whitespace
// before a cut is always one byte wide. Real documents are not built that way. `U+00A0`
// alone is in most Russian text that has ever been through a word processor.
mod common;

use std::fs;

use common::Sandbox;

/// Every wide space a document is likely to carry, with its name for a failure message.
const WIDE_SPACES: [(char, &str); 6] = [
    ('\u{00a0}', "no-break space"),
    ('\u{2009}', "thin space"),
    ('\u{200a}', "hair space"),
    ('\u{202f}', "narrow no-break space"),
    ('\u{3000}', "ideographic space"),
    ('\u{2028}', "line separator"),
];

#[test]
fn a_corpus_of_texts_with_wide_spaces_is_indexed_whole() {
    let sandbox = Sandbox::new("wide-spaces");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    for (space, name) in WIDE_SPACES {
        fs::write(
            sandbox.documents.join(format!("{:04x}.md", space as u32)),
            text_whose_last_space_before_the_cut_is(space, name),
        )
        .unwrap();
    }

    let indexed = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);

    let stdout = String::from_utf8_lossy(&indexed.stdout);
    assert!(
        indexed.status.success(),
        "the pass must not die on a document: {}\n{stdout}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    assert!(
        stdout.contains(&format!(
            "Indexed files (metadata and content): {}",
            WIDE_SPACES.len()
        )),
        "every document must be read: {stdout}"
    );
    assert!(stdout.contains("Failed paths: 0"), "{stdout}");

    // Read back, so the assertion is about what was indexed and not only about an exit code.
    for (space, name) in WIDE_SPACES {
        let found = search(&sandbox, &sentinel(name));
        assert!(
            found.contains("Results: 1"),
            "the text around {space:?} must be searchable: {found}"
        );
    }
}

/// A document in the shape that broke: a wide space, then a run long enough that no chunk
/// can hold it whole and with no whitespace anywhere in it, so the cut has nowhere to go but
/// back to that space. The run is longer than any chunk size this program could be built
/// with, which is why the test states no chunk size of its own.
fn text_whose_last_space_before_the_cut_is(space: char, name: &str) -> String {
    let mut text = String::from("Обычный русский абзац перед длинным фрагментом. ");
    text.push(space);
    text.push_str(&"0123456789abcdef".repeat(4 * 1024));
    text.push_str(&format!("\n\n{}\n", sentinel(name)));
    text
}

fn sentinel(name: &str) -> String {
    format!("sentinel-{}", name.replace(' ', "-"))
}

fn search(sandbox: &Sandbox, query: &str) -> String {
    let output = sandbox.crosskit(&["search", query]);
    assert!(
        output.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
