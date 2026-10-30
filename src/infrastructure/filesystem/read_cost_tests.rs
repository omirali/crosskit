// What each copy of the content costs while a source is read. Stage 2R left the slope of
// one source at about 3 MiB of RSS per MiB of content; this attributes that slope to the
// copies that exist at once, so the one worth removing is known rather than guessed.
//
// macOS-only and ignored by default: it writes and reads hundreds of megabytes.
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

/// One stage of the read, held until the process ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    /// The file as bytes.
    Bytes,
    /// The same buffer as a checked string: `String::from_utf8` reuses it, so this is the
    /// same allocation, not a second one.
    Text,
    /// Text plus the chunks cut from it, which is what the production read holds.
    TextAndChunks,
    /// The chunks alone, with the text released: what a streaming read would hold.
    ChunksOnly,
}

impl Held {
    fn parse(name: &str) -> Self {
        match name {
            "bytes" => Self::Bytes,
            "text" => Self::Text,
            "text-and-chunks" => Self::TextAndChunks,
            "chunks-only" => Self::ChunksOnly,
            other => panic!("unknown read stage {other:?}"),
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "read cost diagnostic worker"]
fn read_cost_worker() {
    let Ok(stage) = std::env::var("CROSSKIT_READ_COST_STAGE") else {
        return;
    };
    let stage = Held::parse(&stage);
    let path = std::env::var("CROSSKIT_READ_COST_FILE").expect("the worker needs its file");
    let mut file = fs::File::open(&path).unwrap();
    let before = file.metadata().unwrap();

    let mut bytes = Vec::with_capacity(before.len() as usize);
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    if stage == Held::Bytes {
        return report(&bytes);
    }

    let text = String::from_utf8(bytes).unwrap();
    if stage == Held::Text {
        return report(&text);
    }

    // The same cut the production read performs, through the same entry point.
    let evidence = super::plain_text_evidence::read_plain_text_evidence(
        &mut fs::File::open(&path).unwrap(),
        &before,
        "file:diagnostic",
        u64::MAX,
    )
    .unwrap();
    match stage {
        Held::TextAndChunks => report(&(text, evidence)),
        Held::ChunksOnly => {
            drop(text);
            report(&evidence)
        }
        _ => unreachable!("earlier stages have returned"),
    }
}

fn report<T>(held: &T) {
    if std::hint::black_box(std::ptr::from_ref(held)).is_null() {
        panic!("unreachable");
    }
}

#[cfg(target_os = "macos")]
fn measure(file: &Path, stage: &str) -> u64 {
    let output = Command::new("/usr/bin/time")
        .arg("-l")
        .arg(std::env::current_exe().unwrap())
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .arg("infrastructure::filesystem::read_cost_tests::read_cost_worker")
        .env("CROSSKIT_READ_COST_STAGE", stage)
        .env("CROSSKIT_READ_COST_FILE", file)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stage} failed: {report}");
    report
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_suffix("maximum resident set size")?
                .trim()
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("no peak resident set size in:\n{report}"))
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "read cost diagnostic: cargo test --release --offline --bins read_cost -- --ignored --nocapture"]
fn the_copies_of_one_source_are_attributed() {
    let root = std::env::temp_dir().join(format!("crosskit-read-cost-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    println!("megabytes | stage | peak RSS MiB");
    for megabytes in [64u64, 100] {
        let file = root.join(format!("source-{megabytes}.txt"));
        let line = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod\n";
        let mut sink = std::io::BufWriter::new(fs::File::create(&file).unwrap());
        let mut written = 0;
        while written < megabytes * 1024 * 1024 - line.len() as u64 {
            sink.write_all(line.as_bytes()).unwrap();
            written += line.len() as u64;
        }
        sink.flush().unwrap();
        drop(sink);

        for stage in ["bytes", "text", "text-and-chunks", "chunks-only"] {
            let peak = measure(&file, stage) as f64 / (1024.0 * 1024.0);
            println!("{megabytes} | {stage} | {peak:.0}");
        }
        fs::remove_file(&file).unwrap();
    }
    fs::remove_dir_all(&root).unwrap();
}
