// Diagnostic: what a real tree still lets through the exclusions, and what each candidate
// rule would take out of it.
//
// The question Stage 20 has to answer is not which names sound like build output — it is
// how much of somebody's actual machine each rule removes. So this walks a named tree with
// the real walker under the real policy and writes down every file the policy admits, with
// its size. Nothing is indexed and no database is opened; the tree is only read.
//
// Ignored by default, because it needs a tree to be pointed at:
//
//   CROSSKIT_EXCLUSION_TREE=<directory> CROSSKIT_EXCLUSION_REPORT=<file> \
//     cargo test --release --offline --bin crosskit -- --ignored --nocapture exclusion_noise
//
// The report is one `size<TAB>path` line per admitted file. It names paths on the machine
// it ran on, so it belongs beside the run and not in a commit.
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::application::{Admission, IndexError, SourceEntry, SourceVisitor, SourceWalker};
use crate::domain::{IndexPolicy, NormalizedPath, PolicyClass, generated_directory_marker};

use super::config::load_index_policy;
use super::filesystem::DescriptorRelativeWalker;

/// Every file the policy admits, with its size. Directories the policy excludes are never
/// descended into, exactly as a real pass would not descend into them.
struct AdmittedFiles<'a> {
    policy: &'a IndexPolicy,
    report: fs::File,
    files: usize,
    bytes: u64,
    excluded: usize,
    generated: usize,
    failures: usize,
}

impl SourceVisitor for AdmittedFiles<'_> {
    fn admit(&mut self, path: &NormalizedPath) -> Admission {
        if self.policy.decide(path).class() == PolicyClass::Exclude {
            self.excluded += 1;
            return Admission::Skip;
        }
        Admission::Inspect
    }

    /// The same answer the policy scan gives, or this would measure a pass nobody runs.
    fn admit_directory(
        &mut self,
        _path: &NormalizedPath,
        holds: &dyn Fn(&str) -> bool,
    ) -> Admission {
        if generated_directory_marker(holds).is_some() {
            self.generated += 1;
            return Admission::Skip;
        }
        Admission::Inspect
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        if self.policy.decide(entry.path()).class() == PolicyClass::Exclude {
            self.excluded += 1;
            return Ok(());
        }
        // Metadata-only files are admitted too: they still take a location, a job and a row.
        let location = match entry.location("file:diagnostic") {
            Ok(location) => location,
            Err(_) => {
                self.failures += 1;
                return Ok(());
            }
        };
        self.files += 1;
        self.bytes += location.size_bytes();
        let _ = writeln!(
            self.report,
            "{}\t{}",
            location.size_bytes(),
            entry.path().as_str()
        );
        Ok(())
    }

    fn skip_unrepresentable_name(&mut self) {
        self.excluded += 1;
    }

    fn record_failure(&mut self, _path: &NormalizedPath, _error: IndexError) {
        self.failures += 1;
    }
}

#[test]
#[ignore = "diagnostic: needs CROSSKIT_EXCLUSION_TREE and CROSSKIT_EXCLUSION_REPORT"]
fn what_the_exclusions_still_admit_on_a_real_tree() {
    let tree = std::env::var("CROSSKIT_EXCLUSION_TREE")
        .expect("CROSSKIT_EXCLUSION_TREE must name the directory to measure");
    let report_path = std::env::var("CROSSKIT_EXCLUSION_REPORT")
        .expect("CROSSKIT_EXCLUSION_REPORT must name the file to write");

    let loaded = load_index_policy().expect("the policy must load");
    let walker = DescriptorRelativeWalker;
    let root = walker
        .resolve_root(Path::new(&tree))
        .expect("the tree must resolve");
    let policy = loaded.policy().with_scan_root(&root);

    let mut visitor = AdmittedFiles {
        policy: &policy,
        report: fs::File::create(&report_path).expect("the report must be writable"),
        files: 0,
        bytes: 0,
        excluded: 0,
        generated: 0,
        failures: 0,
    };
    walker.walk(&root, &mut visitor).expect("the walk must run");

    println!(
        "admitted-files={} admitted-bytes={} excluded-entries={} generated-directories={} failures={}",
        visitor.files, visitor.bytes, visitor.excluded, visitor.generated, visitor.failures
    );
}
